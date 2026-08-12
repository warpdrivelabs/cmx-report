//! value —— 报表公式求值的运行时值。
//!
//! 与 `cmx-biz/src/doc/formula.rs` 的 `FValue` 同构，但数值用 `Decimal`（对齐
//! `cr_cell_data.num_value`，避免 f64 精度漂移），并新增 `Error` 变体承载 Excel 式
//! 错误值（`#REF!`/`#CYCLE!`）——错误在算术里逐层传播，不静默当 0。

use rust_decimal::Decimal;
use rust_decimal::prelude::*;

/// 求值结果值。
#[derive(Debug, Clone, PartialEq)]
pub enum FValue {
    Num(Decimal),
    Str(String),
    Bool(bool),
    Null,
    /// 错误值（如 `#REF!` 循环引用、`#DIV/0!`）。参与运算时整体传播为同一错误。
    Error(String),
}

impl FValue {
    /// 数值视图：Bool→0/1，Str 尝试解析，Null→0，Error→0（仅在已排除 Error 后调用）。
    pub fn as_num(&self) -> Decimal {
        match self {
            FValue::Num(n) => *n,
            FValue::Bool(b) => {
                if *b {
                    Decimal::ONE
                } else {
                    Decimal::ZERO
                }
            }
            FValue::Str(s) => Decimal::from_str(s.trim())
                .or_else(|_| Decimal::from_scientific(s.trim()))
                .unwrap_or(Decimal::ZERO),
            FValue::Null => Decimal::ZERO,
            FValue::Error(_) => Decimal::ZERO,
        }
    }

    /// 布尔视图：Num≠0 为真，非空串为真，Null/Error 为假。
    pub fn as_bool(&self) -> bool {
        match self {
            FValue::Bool(b) => *b,
            FValue::Num(n) => !n.is_zero(),
            FValue::Str(s) => !s.is_empty(),
            FValue::Null => false,
            FValue::Error(_) => false,
        }
    }

    /// 文本视图（写 text_value / 拼接 / 作为取数对象码）。
    pub fn as_text(&self) -> String {
        match self {
            FValue::Str(s) => s.clone(),
            FValue::Num(n) => n.normalize().to_string(),
            FValue::Bool(b) => b.to_string(),
            FValue::Null => String::new(),
            FValue::Error(e) => e.clone(),
        }
    }

    pub fn is_empty(&self) -> bool {
        matches!(self, FValue::Null) || matches!(self, FValue::Str(s) if s.is_empty())
    }

    pub fn is_error(&self) -> bool {
        matches!(self, FValue::Error(_))
    }

    /// 若自身或任一给定值是错误，返回第一个错误（用于运算前的传播检查）。
    pub fn first_error<'a>(vals: impl IntoIterator<Item = &'a FValue>) -> Option<FValue> {
        vals.into_iter().find(|v| v.is_error()).cloned()
    }
}

impl FValue {
    /// 便捷构造数值。
    pub fn num<N: Into<Decimal>>(n: N) -> FValue {
        FValue::Num(n.into())
    }
}
