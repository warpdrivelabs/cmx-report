//! types —— 合并领域类型。

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

/// 科目性质(决定 NCI 归属、报表分类、展示符号)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AccountType {
    /// 资产(借方正)。
    Asset,
    /// 负债(贷方正)。
    Liability,
    /// 所有者权益(贷方正;NCI 划分的对象)。
    Equity,
    /// 收入(贷方正)。
    Income,
    /// 费用(借方正)。
    Expense,
    /// 少数股东权益(权益的子类,单列)。
    Nci,
}

impl AccountType {
    /// 借方正约定下,该科目性质的"正常余额方向"是否为借方。
    pub fn normal_debit(self) -> bool {
        matches!(self, AccountType::Asset | AccountType::Expense)
    }
    /// 是否权益类(含 NCI)——NCI 计算的对象。
    pub fn is_equity(self) -> bool {
        matches!(self, AccountType::Equity | AccountType::Nci)
    }
    /// 是否损益类(收入/费用)——少数股东损益的对象。
    pub fn is_pl(self) -> bool {
        matches!(self, AccountType::Income | AccountType::Expense)
    }
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s.to_ascii_lowercase().as_str() {
            "asset" | "资产" => AccountType::Asset,
            "liability" | "liab" | "负债" => AccountType::Liability,
            "equity" | "权益" => AccountType::Equity,
            "income" | "revenue" | "收入" => AccountType::Income,
            "expense" | "cost" | "费用" => AccountType::Expense,
            "nci" | "少数股东权益" => AccountType::Nci,
            _ => return None,
        })
    }
}

/// 合并方法。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConsolMethod {
    /// 全额合并(子公司,控制):100% 并入 + 抵销 + 确认 NCI。
    Full,
    /// 权益法(联营/合营):按份额确认,不逐行并入。
    Equity,
    /// 比例合并(罕见):按比例并入。
    Proportional,
    /// 成本法:仅个别报表,合并层不调整。
    Cost,
}

impl ConsolMethod {
    pub fn parse(s: &str) -> Self {
        match s.to_ascii_lowercase().as_str() {
            "full" | "全额" | "全额合并" => ConsolMethod::Full,
            "equity" | "权益" | "权益法" => ConsolMethod::Equity,
            "proportional" | "proportion" | "比例" => ConsolMethod::Proportional,
            _ => ConsolMethod::Cost,
        }
    }
    /// 并入比例(Full=1、Proportional=持股、Equity/Cost=0 不逐行并入)。
    pub fn include_ratio(self, ownership: Decimal) -> Decimal {
        match self {
            ConsolMethod::Full => Decimal::ONE,
            ConsolMethod::Proportional => ownership,
            ConsolMethod::Equity | ConsolMethod::Cost => Decimal::ZERO,
        }
    }
}

/// 集团科目(集团统一科目表一行)。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupAccount {
    pub code: String,
    pub name: String,
    pub account_type: AccountType,
}

/// 某主体某集团科目的个别余额(已映射、已折算;借方正 signed)。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntityBalance {
    /// 法人主体/合并节点代码。
    pub entity: String,
    /// 集团科目代码。
    pub account: String,
    /// 借方正净额。
    pub amount: Decimal,
}

/// 合并范围节点(承接 cr_consol_org 层级 + 方法 + 持股)。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScopeNode {
    /// 组织代码(合并节点或叶子主体)。
    pub code: String,
    pub name: String,
    /// 上级节点代码(顶层为 None)。
    pub parent: Option<String>,
    /// 该节点纳入上级合并的方法。
    pub method: ConsolMethod,
    /// 母公司对本主体的持股比例(0~1)。
    pub ownership: Decimal,
    /// 是否叶子(直接持有个别报表数据)。
    pub is_leaf: bool,
    /// 层级深度(1=顶)。
    pub level: i32,
}

/// 抵销分录一行(借方正:net = dr − cr)。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ElimLine {
    pub account: String,
    /// 借方额(≥0)。
    pub dr: Decimal,
    /// 贷方额(≥0)。
    pub cr: Decimal,
    /// 往来对手(债务/购销抵销标注,可空)。
    pub partner: Option<String>,
}

impl ElimLine {
    pub fn new(account: &str, dr: Decimal, cr: Decimal) -> Self {
        Self { account: account.to_string(), dr, cr, partner: None }
    }
    /// 借方正净额。
    pub fn net(&self) -> Decimal {
        self.dr - self.cr
    }
}

/// 一张抵销/调整凭证(合并分类账)。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ElimEntry {
    /// 抵销类型(capital/debt/sales/inventory_profit/dividend/nci/fx_translation/…)。
    pub elim_type: String,
    /// 生成本凭证的规则代码(可追溯)。
    pub source_rule: String,
    /// 是否期初结转凭证。
    pub is_opening: bool,
    /// 借贷行。
    pub lines: Vec<ElimLine>,
}

impl ElimEntry {
    /// 凭证是否借贷平衡(各行 net 之和 = 0)。
    pub fn is_balanced(&self) -> bool {
        self.lines.iter().map(|l| l.net()).sum::<Decimal>() == Decimal::ZERO
    }
    /// 该凭证对某科目的净影响(借方正)。
    pub fn net_for(&self, account: &str) -> Decimal {
        self.lines
            .iter()
            .filter(|l| l.account == account)
            .map(|l| l.net())
            .sum()
    }
}

/// 工作底稿一格(某合并节点某科目的四栏)。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorksheetCell {
    pub node: String,
    pub account: String,
    /// 个别合计(下级并入,借方正)。
    pub individual: Decimal,
    /// 调整(权益法/折算等)。
    pub adjust: Decimal,
    /// 抵销。
    pub elim: Decimal,
    /// 合并数 = individual + adjust + elim。
    pub consolidated: Decimal,
}
