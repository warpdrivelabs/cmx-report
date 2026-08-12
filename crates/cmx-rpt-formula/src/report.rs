//! report —— 引擎的输入/输出模型（语义中立，store-pg 构建、引擎消费）。
//!
//! `ReportSnapshot` 是一张报表某 org+period 的**装载态**：每格的元素码 + 单元格级公式 +
//! 已存值，外加元素级公式表（§6 继承源）。主报表由 store-pg 预装载传入 `compute_report`；
//! REF 命中他表时经 `DataProvider::load_report` 懒装载同型快照进会话。
//! `ComputedCell` 是产出：算好的格 + 状态（computed/error），由 store-pg UPSERT 回 cr_cell_data。

use std::collections::HashMap;

use crate::value::FValue;

/// 一个单元格的装载态（对应 cr_cell_element_map 一行 + 其 cr_cell_data 已存值）。
#[derive(Debug, Clone)]
pub struct CellInput {
    /// A1 引用（大写），公式引用与依赖解析的地址。
    pub cell_ref: String,
    /// 绑定的数据元素码（§6 继承源）。
    pub element_code: Option<String>,
    /// 单元格级公式（cr_cell_element_map.calc_formula）——最高优先。
    pub cell_formula: Option<String>,
    /// 已存值（cr_cell_data，手工/上次算的），叶子格直接返它。
    pub stored: Option<FValue>,
    /// 手工覆盖：compute 时跳过不重算、不落库。
    pub is_manual: bool,
}

/// 一张报表某 org+period 的装载快照。
#[derive(Debug, Clone)]
pub struct ReportSnapshot {
    pub report: String,
    pub version: String,
    pub org: String,
    pub period: String,
    pub cells: Vec<CellInput>,
    /// 元素码 → 元素级公式（cr_data_element.calc_formula），§6 继承源。
    pub element_formulas: HashMap<String, String>,
}

impl ReportSnapshot {
    pub fn find_cell(&self, cell_ref: &str) -> Option<&CellInput> {
        self.cells.iter().find(|c| c.cell_ref == cell_ref)
    }

    /// §6 优先级阶梯：单元格级公式 &gt; 元素级继承公式 &gt; 无（叶子）。
    pub fn effective_formula(&self, cell: &CellInput) -> Option<String> {
        if let Some(f) = &cell.cell_formula
            && !f.trim().is_empty()
        {
            return Some(f.clone());
        }
        cell.element_code
            .as_ref()
            .and_then(|ec| self.element_formulas.get(ec))
            .filter(|f| !f.trim().is_empty())
            .cloned()
    }
}

/// 算好的单元格（产出，回写 cr_cell_data）。
#[derive(Debug, Clone)]
pub struct ComputedCell {
    pub report: String,
    pub version: String,
    pub org: String,
    pub period: String,
    pub cell_ref: String,
    pub value: FValue,
    /// computed / error。
    pub status: &'static str,
}

/// compute 结果汇总。
#[derive(Debug, Clone, Default)]
pub struct ComputeOutcome {
    pub cells: Vec<ComputedCell>,
    /// 人类可读错误（含循环引用链）。
    pub errors: Vec<String>,
    pub computed: usize,
    pub error_count: usize,
}
