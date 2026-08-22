//! resolve —— 递归依赖解析引擎（方案 §8）：REF 跨表 + 三色环检测 + 两遍法取数。
//!
//! 一次 `compute_report` 是一个**解析会话**：对主报表每个有公式的格做记忆化 DFS——
//!   · 遇 QM/QC/FS/JE 取数叶子 → 收集 BalanceKey，批量经 DataProvider 取回（两遍法）；
//!   · 遇本表单元格引用 C5 → 递归解析该格（DFS 展开即拓扑序）；
//!   · 遇 REF(他表,…) → 懒装载他表快照进会话，递归解析目标格；
//!   · 再遇「灰」（正在解析栈上的）目标 → 循环引用，返 `#REF!` 斩断，环链标 error。
//! `memo` 保证每个全局地址只算一次（钻石依赖不重算）；跨报表共享一份 memo/resolving。
//!
//! 求值本身是同步 `eval::eval_node`——本模块负责把它需要的外部值（fetch/cell/ref）
//! 异步预填进 `Scope`，再同步求值。递归用 `Box::pin` 装箱（async 递归标准手法）。

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::pin::Pin;

use crate::ast::{Node, expand_range, parse};
use crate::eval::{EvalCtx, Scope, addr_key, derive_balance_key, derive_ref_addr, eval_node};
use crate::provider::{BalanceKey, DataProvider};
use crate::report::{CellInput, ComputeOutcome, ComputedCell, ReportSnapshot};
use crate::value::FValue;

/// 三色标记状态（会话内存态，不落库）。
#[derive(Clone, Copy, PartialEq)]
enum Color {
    Gray,  // 正在解析（在递归栈上）
    Black, // 已解析（memo 命中）
}

/// 解析会话：跨报表共享的记忆化 + 环检测 + 已装载快照缓存。
struct Session<'p> {
    provider: &'p dyn DataProvider,
    ctx_periods: Vec<String>,
    org_parent: HashMap<String, String>,
    /// 全局地址 → 已解析值（黑）。
    memo: HashMap<String, FValue>,
    /// 全局地址 → 颜色（灰=在栈上）。
    color: HashMap<String, Color>,
    /// 已装载的报表快照缓存（report|version|org|period → snapshot）。
    reports: HashMap<String, ReportSnapshot>,
    /// 命中的循环引用链（人类可读，回报给用户）。
    cycle_errors: Vec<String>,
}

/// 报表级地址（定位一个快照）。
fn report_key(report: &str, version: &str, org: &str, period: &str) -> String {
    format!("{report}|{version}|{org}|{period}")
}

impl<'p> Session<'p> {
    /// 构造当前上下文（相对期间/组织解析所需字典对所有报表一致）。
    fn ctx_for(&self, snap: &ReportSnapshot) -> EvalCtx {
        EvalCtx {
            org: snap.org.clone(),
            period: snap.period.clone(),
            periods: self.ctx_periods.clone(),
            org_parent: self.org_parent.clone(),
        }
    }

    /// 解析一个全局单元格地址 → 值（记忆化 DFS + 三色环检测）。
    /// 返回的 Future 显式 `+ Send`——否则 async 递归 + `dyn DataProvider` 会让整条
    /// compute 链非 Send，axum handler 的 `Handler` bound 无法满足。
    fn resolve<'a>(
        &'a mut self,
        report: String,
        version: String,
        org: String,
        period: String,
        cell: String,
    ) -> Pin<Box<dyn Future<Output = FValue> + Send + 'a>> {
        Box::pin(async move {
            let addr = addr_key(&report, &version, &org, &period, &cell);

            // 黑：命中记忆
            if let Some(v) = self.memo.get(&addr) {
                return v.clone();
            }
            // 灰：又绕回正在解析的格 → 循环引用
            if matches!(self.color.get(&addr), Some(Color::Gray)) {
                self.cycle_errors
                    .push(format!("循环引用: {report}!{cell} ({org}/{period})"));
                return FValue::Error("#REF!".into());
            }

            // 白 → 灰：压栈
            self.color.insert(addr.clone(), Color::Gray);

            // 装载目标格所在快照（本表已在 reports 缓存；他表懒装载）
            let rk = report_key(&report, &version, &org, &period);
            if !self.reports.contains_key(&rk) {
                match self
                    .provider
                    .load_report(&report, &version, &org, &period)
                    .await
                {
                    Ok(Some(snap)) => {
                        self.reports.insert(rk.clone(), snap);
                    }
                    Ok(None) => {
                        self.color.insert(addr.clone(), Color::Black);
                        let v = FValue::Error("#REF!".into());
                        self.memo.insert(addr, v.clone());
                        return v;
                    }
                    Err(e) => {
                        self.color.insert(addr.clone(), Color::Black);
                        self.cycle_errors
                            .push(format!("装载报表失败 {report}: {e}"));
                        let v = FValue::Error("#REF!".into());
                        self.memo.insert(addr, v.clone());
                        return v;
                    }
                }
            }

            // 取该格输入（clone 出来避免与 &mut self 冲突）
            let (formula, stored, is_manual) = {
                let snap = self.reports.get(&rk).unwrap();
                match snap.find_cell(&cell) {
                    Some(ci) => (snap.effective_formula(ci), ci.stored.clone(), ci.is_manual),
                    None => (None, None, false),
                }
            };

            // 手工覆盖 或 无公式 → 叶子，取已存值
            let value = match formula {
                _ if is_manual => stored.unwrap_or(FValue::Null),
                None => stored.unwrap_or(FValue::Null),
                Some(f) => match parse(&f) {
                    Ok(ast) => self.eval_ast(&ast, &report, &version, &org, &period).await,
                    Err(e) => {
                        self.cycle_errors
                            .push(format!("{report}!{cell} 公式解析失败: {e}"));
                        FValue::Error("#FORMULA!".into())
                    }
                },
            };

            // 灰 → 黑：记忆、出栈
            self.color.insert(addr.clone(), Color::Black);
            self.memo.insert(addr, value.clone());
            value
        })
    }

    /// 求值一个已解析报表内的 AST：先异步预解析所有外部依赖填 Scope，再同步 eval。
    async fn eval_ast(
        &mut self,
        ast: &Node,
        report: &str,
        version: &str,
        org: &str,
        period: &str,
    ) -> FValue {
        let rk = report_key(report, version, org, period);
        let snap = self.reports.get(&rk).unwrap().clone();
        let ctx = self.ctx_for(&snap);

        // ── 收集依赖：取数键 / 本表单元格 / REF 地址（用一个临时 scope 做参数派生） ──
        let mut fetch_keys: Vec<BalanceKey> = Vec::new();
        let mut cell_deps: HashSet<String> = HashSet::new();
        let mut ref_deps: Vec<(String, String, String, String, String)> = Vec::new();
        {
            let probe = Scope::new(&ctx);
            collect_deps(ast, &probe, &mut fetch_keys, &mut cell_deps, &mut ref_deps);
        }

        // ── 批量取数（两遍法 pass1） ──
        let mut fetches: HashMap<BalanceKey, rust_decimal::Decimal> = HashMap::new();
        if !fetch_keys.is_empty() {
            fetch_keys.sort_by(|a, b| a.object.cmp(&b.object));
            fetch_keys.dedup();
            match self.provider.batch_balance(&fetch_keys).await {
                Ok(m) => fetches = m,
                Err(e) => self.cycle_errors.push(format!("取数失败: {e}")),
            }
        }

        // ── 递归解析本表单元格依赖 ──
        let mut cells: HashMap<String, FValue> = HashMap::new();
        for c in &cell_deps {
            let v = self
                .resolve(
                    report.to_string(),
                    version.to_string(),
                    org.to_string(),
                    period.to_string(),
                    c.clone(),
                )
                .await;
            cells.insert(c.clone(), v);
        }

        // ── 递归解析 REF 目标（跨表/表内 + 区间展开） ──
        let mut refs: HashMap<String, FValue> = HashMap::new();
        for (r, v, o, p, c) in &ref_deps {
            let v_ = self
                .resolve(r.clone(), v.clone(), o.clone(), p.clone(), c.clone())
                .await;
            refs.insert(addr_key(r, v, o, p, c), v_);
        }

        // ── pass2：同步求值 ──
        let mut scope = Scope::new(&ctx);
        scope.fetches = fetches;
        scope.cells = cells;
        scope.refs = refs;
        eval_node(ast, &scope)
    }
}

/// 遍历 AST 收集三类外部依赖（参数派生用 probe scope，仅解析字面量/上下文）。
fn collect_deps(
    node: &Node,
    scope: &Scope,
    fetch_keys: &mut Vec<BalanceKey>,
    cell_deps: &mut HashSet<String>,
    ref_deps: &mut Vec<(String, String, String, String, String)>,
) {
    match node {
        Node::Cell(c) => {
            cell_deps.insert(c.clone());
        }
        Node::Range(a, b) => {
            for c in expand_range(a, b) {
                cell_deps.insert(c);
            }
        }
        Node::Unary(_, x) => collect_deps(x, scope, fetch_keys, cell_deps, ref_deps),
        Node::Binary(_, l, r) => {
            collect_deps(l, scope, fetch_keys, cell_deps, ref_deps);
            collect_deps(r, scope, fetch_keys, cell_deps, ref_deps);
        }
        Node::Call(name, args) => {
            match name.as_str() {
                "QM" | "QC" | "FS" | "JE" | "CG" | "IND" | "ELIM" | "CF" | "EQC" => {
                    if let Some(k) = derive_balance_key(name, args, scope) {
                        fetch_keys.push(k);
                    }
                }
                "REF" => {
                    if let Some(addr) = derive_ref_addr(args, scope) {
                        ref_deps.push(addr);
                    }
                }
                _ => {}
            }
            // 参数内部可能还有嵌套依赖（如 SUM(C5, QM(...))）
            for a in args {
                collect_deps(a, scope, fetch_keys, cell_deps, ref_deps);
            }
        }
        _ => {}
    }
}

/// 主入口：对主报表快照做全表 compute（方案 §7/§8 全流程）。
///
/// - `snapshot`：主报表某 org+period 装载态（store-pg 预装载）。
/// - `provider`：取数 + REF 懒装载他表。
/// - `periods`：升序叶子期间序列（相对期间偏移）；`org_parent`：@parent 解析。
pub async fn compute_report(
    snapshot: ReportSnapshot,
    provider: &dyn DataProvider,
    periods: Vec<String>,
    org_parent: HashMap<String, String>,
) -> ComputeOutcome {
    let (report, version, org, period) = (
        snapshot.report.clone(),
        snapshot.version.clone(),
        snapshot.org.clone(),
        snapshot.period.clone(),
    );
    let rk = report_key(&report, &version, &org, &period);

    let mut session = Session {
        provider,
        ctx_periods: periods,
        org_parent,
        memo: HashMap::new(),
        color: HashMap::new(),
        reports: HashMap::new(),
        cycle_errors: Vec::new(),
    };

    // 主报表要算的格（有生效公式且非手工），先建 cell_ref 列表（避免借用冲突）
    let targets: Vec<CellInput> = snapshot.cells.clone();
    session.reports.insert(rk, snapshot);

    let mut outcome = ComputeOutcome::default();
    for ci in &targets {
        // 手工格：跳过（不重算不落库）
        if ci.is_manual {
            continue;
        }
        // 无生效公式：叶子，不产出计算值（保留其手工/取数默认值原样）
        let has_formula = {
            let snap = session.reports.values().next().unwrap();
            snap.effective_formula(ci).is_some()
        };
        if !has_formula {
            continue;
        }
        let v = session
            .resolve(
                report.clone(),
                version.clone(),
                org.clone(),
                period.clone(),
                ci.cell_ref.clone(),
            )
            .await;
        let status = if v.is_error() { "error" } else { "computed" };
        if v.is_error() {
            outcome.error_count += 1;
        } else {
            outcome.computed += 1;
        }
        outcome.cells.push(ComputedCell {
            report: report.clone(),
            version: version.clone(),
            org: org.clone(),
            period: period.clone(),
            cell_ref: ci.cell_ref.clone(),
            value: v,
            status,
        });
    }
    outcome.errors = session.cycle_errors;
    outcome
}
