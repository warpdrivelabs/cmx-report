//! cmx-rpt-formula —— 报表自定义函数引擎（语义中立，无 DB/HTTP）。
//!
//! 方案 `docs/报表自定义函数方案.html` 的后端计算引擎落地：
//!   - DSL 词法/语法/AST（`ast`）—— 承 doc/formula.rs，扩单元格引用/区间/组织记号；
//!   - 同步求值（`eval`）—— 两遍法 pass2，纯内存，错误值传播；
//!   - 函数注册（`registry` + `functions`）—— inventory 编译期收集，五元契约，目录导出；
//!   - 取数抽象（`provider`）—— DataProvider trait（唯一真源接缝）+ MockProvider；
//!   - 装载模型（`report`）—— ReportSnapshot（§6 单元格级▸元素继承）+ ComputeOutcome；
//!   - 递归解析（`resolve`）—— REF 跨表 + 三色环检测 + 两遍法批量取数（§8）。
//!
//! 入口 `compute_report(snapshot, provider, periods, org_parent)`：对主报表全表求值。
//! store-pg 提供 DataProvider 实现（读 cr_* 表）+ 装载快照 + 回写 cr_cell_data。

pub mod ast;
pub mod eval;
pub mod functions;
pub mod provider;
pub mod registry;
pub mod report;
pub mod resolve;
pub mod value;

// 便捷再导出
pub use eval::{EvalCtx, Scope, eval_node};
pub use provider::{BalanceKey, DataProvider, FetchKind, MockProvider};
pub use registry::{RptFunction, all_functions, catalog_json};
pub use report::{CellInput, ComputeOutcome, ComputedCell, ReportSnapshot};
pub use resolve::compute_report;
pub use value::FValue;

/// 快速求值一个独立表达式（无外部依赖，测试/预览用）。取数/单元格引用按 0。
pub fn eval_standalone(expr: &str, ctx: &EvalCtx) -> Result<FValue, String> {
    let ast = ast::parse(expr)?;
    let scope = Scope::new(ctx);
    Ok(eval_node(&ast, &scope))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal::Decimal;
    use std::collections::HashMap;

    fn cell(cell_ref: &str, formula: Option<&str>, element: Option<&str>) -> CellInput {
        CellInput {
            cell_ref: cell_ref.to_string(),
            element_code: element.map(str::to_string),
            cell_formula: formula.map(str::to_string),
            stored: None,
            is_manual: false,
        }
    }

    fn snapshot(cells: Vec<CellInput>, elem: HashMap<String, String>) -> ReportSnapshot {
        ReportSnapshot {
            report: "BS".into(),
            version: "v1".into(),
            org: "HQ".into(),
            period: "2026-06".into(),
            cells,
            element_formulas: elem,
        }
    }

    #[test]
    fn registry_has_first_batch() {
        let fns = all_functions();
        let names: Vec<_> = fns.iter().map(|f| f.name).collect();
        for want in [
            "QM", "QC", "FS", "JE", "REF", "SUM", "IF", "ROUND", "ABS", "MIN", "MAX",
        ] {
            assert!(names.contains(&want), "缺函数 {want}");
        }
    }

    #[test]
    fn catalog_json_serializes() {
        let j = catalog_json();
        assert!(j.get("functions").and_then(|f| f.as_array()).is_some());
        let s = serde_json::to_string(&j).unwrap();
        assert!(s.contains("\"QM\""));
        assert!(s.contains("\"category\":\"fetch\""));
    }

    #[tokio::test]
    async fn two_pass_fetch_qm() {
        // C5 = QM(0,@current,'1001') → Mock 返 1234
        let snap = snapshot(
            vec![cell("C5", Some("QM(0,@current,'1001')"), None)],
            HashMap::new(),
        );
        let out = compute_report(snap, &MockProvider::default(), vec![], HashMap::new()).await;
        assert_eq!(out.computed, 1);
        assert_eq!(out.cells[0].value, FValue::Num(Decimal::from(1234)));
        assert_eq!(out.cells[0].status, "computed");
    }

    #[tokio::test]
    async fn element_formula_inherited() {
        // C5 无公式，但绑元素 CASH，元素有公式 → §6 继承
        let mut elem = HashMap::new();
        elem.insert("CASH".to_string(), "QM(0,@current,'1001')".to_string());
        let snap = snapshot(vec![cell("C5", None, Some("CASH"))], elem);
        let out = compute_report(snap, &MockProvider::default(), vec![], HashMap::new()).await;
        assert_eq!(out.computed, 1);
        assert_eq!(out.cells[0].value, FValue::Num(Decimal::from(1234)));
    }

    #[tokio::test]
    async fn cell_formula_overrides_element() {
        // C5 自身公式 = 100，元素公式 = QM(...)=1234 → 自身优先
        let mut elem = HashMap::new();
        elem.insert("CASH".to_string(), "QM(0,@current,'1001')".to_string());
        let snap = snapshot(vec![cell("C5", Some("100"), Some("CASH"))], elem);
        let out = compute_report(snap, &MockProvider::default(), vec![], HashMap::new()).await;
        assert_eq!(out.cells[0].value, FValue::Num(Decimal::from(100)));
    }

    #[tokio::test]
    async fn cell_reference_topo_order() {
        // C5 = 10; C6 = C5 + 5; C7 = SUM(C5:C6) → 10,15,25
        let snap = snapshot(
            vec![
                cell("C5", Some("10"), None),
                cell("C6", Some("C5 + 5"), None),
                cell("C7", Some("SUM(C5:C6)"), None),
            ],
            HashMap::new(),
        );
        let out = compute_report(snap, &MockProvider::default(), vec![], HashMap::new()).await;
        let by_ref: HashMap<_, _> = out
            .cells
            .iter()
            .map(|c| (c.cell_ref.as_str(), &c.value))
            .collect();
        assert_eq!(by_ref["C5"], &FValue::Num(Decimal::from(10)));
        assert_eq!(by_ref["C6"], &FValue::Num(Decimal::from(15)));
        assert_eq!(by_ref["C7"], &FValue::Num(Decimal::from(25)));
    }

    #[tokio::test]
    async fn self_cycle_detected() {
        // C5 = C6 + 1; C6 = C5 + 1 → 循环引用
        let snap = snapshot(
            vec![
                cell("C5", Some("C6 + 1"), None),
                cell("C6", Some("C5 + 1"), None),
            ],
            HashMap::new(),
        );
        let out = compute_report(snap, &MockProvider::default(), vec![], HashMap::new()).await;
        assert!(out.error_count >= 1, "应检出循环引用");
        assert!(out.cells.iter().any(|c| c.value.is_error()));
        assert!(!out.errors.is_empty());
    }

    #[tokio::test]
    async fn relative_period_offset() {
        // 期间序列 [03,04,05,06]，当前 06，QM(-1)=05 → 用带期间键的 Mock 验证解析
        use async_trait::async_trait;
        struct PeriodProbe;
        #[async_trait]
        impl DataProvider for PeriodProbe {
            async fn batch_balance(
                &self,
                keys: &[BalanceKey],
            ) -> Result<HashMap<BalanceKey, Decimal>, String> {
                // 返回：值=期间末两位数字，便于断言解析出的期间
                let mut m = HashMap::new();
                for k in keys {
                    let tail: Decimal = k
                        .period
                        .rsplit('-')
                        .next()
                        .unwrap_or("0")
                        .parse()
                        .unwrap_or(Decimal::ZERO);
                    m.insert(k.clone(), tail);
                }
                Ok(m)
            }
            async fn load_report(
                &self,
                _r: &str,
                _v: &str,
                _o: &str,
                _p: &str,
            ) -> Result<Option<ReportSnapshot>, String> {
                Ok(None)
            }
        }
        let snap = snapshot(
            vec![cell("C5", Some("QM(-1,@current,'1001')"), None)],
            HashMap::new(),
        );
        let periods = vec![
            "2026-03".to_string(),
            "2026-04".to_string(),
            "2026-05".to_string(),
            "2026-06".to_string(),
        ];
        let out = compute_report(snap, &PeriodProbe, periods, HashMap::new()).await;
        // QM(-1) 从 06 回退到 05 → 值 5
        assert_eq!(out.cells[0].value, FValue::Num(Decimal::from(5)));
    }

    #[tokio::test]
    async fn cross_report_ref_and_cycle() {
        use async_trait::async_trait;
        // A!C5 = REF('B','v1',C5); B!C5 = REF('A','v1',C5) → 跨表环
        struct TwoReports;
        #[async_trait]
        impl DataProvider for TwoReports {
            async fn batch_balance(
                &self,
                _keys: &[BalanceKey],
            ) -> Result<HashMap<BalanceKey, Decimal>, String> {
                Ok(HashMap::new())
            }
            async fn load_report(
                &self,
                report: &str,
                version: &str,
                org: &str,
                period: &str,
            ) -> Result<Option<ReportSnapshot>, String> {
                let other = if report == "A" { "B" } else { "A" };
                Ok(Some(ReportSnapshot {
                    report: report.to_string(),
                    version: version.to_string(),
                    org: org.to_string(),
                    period: period.to_string(),
                    cells: vec![CellInput {
                        cell_ref: "C5".into(),
                        element_code: None,
                        cell_formula: Some(format!("REF('{other}','v1',C5)")),
                        stored: None,
                        is_manual: false,
                    }],
                    element_formulas: HashMap::new(),
                }))
            }
        }
        let snap = ReportSnapshot {
            report: "A".into(),
            version: "v1".into(),
            org: "HQ".into(),
            period: "2026-06".into(),
            cells: vec![CellInput {
                cell_ref: "C5".into(),
                element_code: None,
                cell_formula: Some("REF('B','v1',C5)".into()),
                stored: None,
                is_manual: false,
            }],
            element_formulas: HashMap::new(),
        };
        let out = compute_report(snap, &TwoReports, vec![], HashMap::new()).await;
        assert!(out.error_count >= 1, "跨表环应被检出");
        assert!(out.errors.iter().any(|e| e.contains("循环引用")));
    }

    #[tokio::test]
    async fn manual_cell_skipped() {
        let mut c = cell("C5", Some("QM(0,@current,'1001')"), None);
        c.is_manual = true;
        c.stored = Some(FValue::Num(Decimal::from(999)));
        let snap = snapshot(vec![c], HashMap::new());
        let out = compute_report(snap, &MockProvider::default(), vec![], HashMap::new()).await;
        // 手工格跳过：不产出计算 cell
        assert_eq!(out.computed, 0);
        assert!(out.cells.is_empty());
    }
}
