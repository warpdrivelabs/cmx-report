//! provider —— 取数数据源抽象（方案 §7 唯一"真源"接缝）。
//!
//! 引擎、DSL、注册、向导全部与数据源解耦：取数函数（QM/QC/FS/JE）经 `DataProvider`
//! 拿余额/发生额，REF 经它懒加载他表单元格。首版 `MockProvider`（本 crate 自带，返
//! 固定/公式派生值）让整条链路（设计→算→落库→读）先跑通；GL 就绪后 store-pg 换成
//! `GlBalanceProvider`（查 cv_gl_balance）或 `VoucherAggProvider`（聚合 cv_* 凭证），引擎零改。

use std::collections::HashMap;

use async_trait::async_trait;
use rust_decimal::Decimal;

use crate::report::ReportSnapshot;

/// 取数种类（BalanceKey 的一维）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FetchKind {
    /// 期末余额 QM。
    EndBalance,
    /// 期初余额 QC。
    BeginBalance,
    /// 借方发生额。
    DebitAmount,
    /// 贷方发生额。
    CreditAmount,
    /// 净发生额（借-贷）。
    NetAmount,
    /// 合并数(CG):cg_consol_data.consolidated。org=合并节点,object=集团科目。
    Consolidated,
    /// 个别合计(IND):cg_consol_data.individual(未抵销)。
    Individual,
    /// 抵销额(ELIM):cg_consol_data.elim。
    Elimination,
    /// 现金流量项目合并数(CF):cg_cash_flow_item.amount(合并节点聚合,借方正)。org=合并节点,object=现金流量项目码。
    CashFlow,
    /// 权益变动列合并数(EQC):cg_equity_change.amount(合并节点聚合,借方正)。org=合并节点,object=权益变动列码。
    EquityChange,
}

/// 一次取数的完整查询键（pass1 收集、批量去重、pass2 命中）。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct BalanceKey {
    pub kind: FetchKind,
    /// 已解析成绝对期间码（如 2026-06）。
    pub period: String,
    /// 已解析成绝对组织码。
    pub org: String,
    /// 取数对象：科目码或元素码（元素码已在参数解析阶段转成科目/派生）。
    pub object: String,
}

/// 取数数据源。async，因为要打 DB/缓存。
#[async_trait]
pub trait DataProvider: Send + Sync {
    /// 批量取余额/发生额（QM/QC/FS/JE）——一次 IO 取回一批键（两遍法 pass1 的批量点）。
    /// 未命中的键可省略（调用方按 0 处理）。
    async fn batch_balance(
        &self,
        keys: &[BalanceKey],
    ) -> Result<HashMap<BalanceKey, Decimal>, String>;

    /// 表间取数（REF）：**懒装载**另一报表 org+period 的完整快照（公式 + 已存值）进会话，
    /// 引擎据此对他表目标格递归求值（§8）。返回 None 表示该 report/version 不存在。
    /// 本表引用不经此路（引擎已持有主快照）。
    async fn load_report(
        &self,
        report: &str,
        version: &str,
        org: &str,
        period: &str,
    ) -> Result<Option<ReportSnapshot>, String>;
}

/// 首版内存 Mock：QM/QC 返固定演示值，REF 返 None。让 P1 链路无 DB 即可跑通、单测自足。
pub struct MockProvider {
    /// 可选：预置的余额（键=object 科目码）→ 值，未命中返回 `default`。
    pub balances: HashMap<String, Decimal>,
    pub default: Decimal,
}

impl Default for MockProvider {
    fn default() -> Self {
        MockProvider {
            balances: HashMap::new(),
            default: Decimal::from(1234),
        }
    }
}

#[async_trait]
impl DataProvider for MockProvider {
    async fn batch_balance(
        &self,
        keys: &[BalanceKey],
    ) -> Result<HashMap<BalanceKey, Decimal>, String> {
        let mut out = HashMap::new();
        for k in keys {
            let v = self
                .balances
                .get(&k.object)
                .copied()
                .unwrap_or(self.default);
            out.insert(k.clone(), v);
        }
        Ok(out)
    }

    async fn load_report(
        &self,
        _report: &str,
        _version: &str,
        _org: &str,
        _period: &str,
    ) -> Result<Option<ReportSnapshot>, String> {
        Ok(None)
    }
}
