//! registry —— 报表函数的注册契约 + 编译期注册（inventory）+ 目录导出。
//!
//! 五元注册（方案 §2）：函数名 · 原型 · 后端算法 · 前端算法(前端镜像同名实现) · 向导定义。
//! 后端 `inventory::collect!(RegisteredRptFn)` 全局收集（母版 `cmx-core iam::registry`），
//! 每个函数在 `functions/*.rs` 里 `inventory::submit!` 一条；`GET /report-design/functions`
//! 经 `catalog_json()` 把元数据（名/分类/原型/向导/帮助/示例）导给前端向导渲染。
//!
//! 算法本体不在这里——`eval.rs` 按函数名分发内存运算，取数函数经 `provider.rs` 取数。
//! 注册项是**元数据 + 分类标记**，让引擎/向导/目录端点共享同一份真源。

use serde::Serialize;

/// 函数分类（决定向导分组 + 是否需要取数 IO）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum FnCategory {
    /// 取数：期末/期初/发生额/净额（打 GL/DataProvider）。
    Fetch,
    /// 引用：REF 表间/表内取另一单元格（递归解析）。
    Ref,
    /// 汇总：SUM 等。
    Agg,
    /// 逻辑：IF/AND/OR/NOT。
    Logic,
    /// 数学：ABS/ROUND/MIN/MAX。
    Math,
}

impl FnCategory {
    pub fn as_str(&self) -> &'static str {
        match self {
            FnCategory::Fetch => "fetch",
            FnCategory::Ref => "ref",
            FnCategory::Agg => "agg",
            FnCategory::Logic => "logic",
            FnCategory::Math => "math",
        }
    }
}

/// 返回值类型（向导预览格式化 + value_type 落库参考）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ValueType {
    Amount,
    Qty,
    Rate,
    Text,
    Date,
    Bool,
}

/// 参数种类：决定向导渲染什么控件 + 参数解析走哪条规则。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ParamKind {
    /// 期间：0/-1/-2/-12 相对，或绝对期间码。向导=期间选择器。
    Period,
    /// 组织：@current/组织码。向导=组织树。
    Org,
    /// 取数对象：元素码/科目码。向导=元素/科目选择器。
    Object,
    /// 借贷方向（FS 用）：debit/credit。向导=方向下拉。
    Direction,
    /// 报表编码（REF）。向导=报表选择器。
    Report,
    /// 版本编码（REF）。向导=版本下拉。
    Version,
    /// 单元格引用（REF/SUM）。向导=单元格拾取器。
    CellRef,
    /// 任意数值。向导=数值框。
    Number,
    /// 任意文本。向导=文本框。
    Text,
    /// 任意子表达式（IF 的分支、SUM 的加数）。向导=表达式框/嵌套向导。
    Expr,
}

/// 单个参数签名。
#[derive(Debug, Clone, Serialize)]
pub struct Param {
    pub name: &'static str,
    pub kind: ParamKind,
    pub required: bool,
    /// 缺省字面量（期间默认 "0" 本期）。
    pub default: Option<&'static str>,
    /// 向导提示文本。
    pub hint: &'static str,
}

/// 函数原型：固定参 + 尾部可变参。
#[derive(Debug, Clone, Serialize)]
pub struct Prototype {
    pub params: Vec<Param>,
    /// 变参签名（SUM(a,b,…) / IF 无）。存在则末尾可重复。
    pub variadic: Option<Param>,
}

/// 前端向导定义：每个参数的控件由 `Param.kind` 决定，这里带整体开关。
#[derive(Debug, Clone, Serialize)]
pub struct WizardSpec {
    /// 向导内是否实时预览取数结果（取数函数 true，纯逻辑 false）。
    pub preview: bool,
}

/// 函数注册元数据（五元 + 目录字段）。算法本体在 eval.rs 按 name 分发。
#[derive(Debug, Clone, Serialize)]
pub struct RptFunction {
    pub name: &'static str,
    pub category: FnCategory,
    pub ret_type: ValueType,
    /// 是否需要取数 IO（QM/QC/FS/JE=true 走两遍法预取；REF 递归；纯内存=false）。
    pub is_fetch: bool,
    pub prototype: Prototype,
    pub help: &'static str,
    pub example: &'static str,
    pub wizard: WizardSpec,
}

/// inventory 收集包装（母版：cmx-core RegisteredPermission）。
pub struct RegisteredRptFn {
    pub def: fn() -> RptFunction,
}

inventory::collect!(RegisteredRptFn);

/// 枚举全部已注册函数（启动/请求期）。
pub fn all_functions() -> Vec<RptFunction> {
    inventory::iter::<RegisteredRptFn>()
        .map(|r| (r.def)())
        .collect()
}

/// 目录导出为 JSON（GET /report-design/functions）：按分类排序，供前端向导渲染。
pub fn catalog_json() -> serde_json::Value {
    let mut fns = all_functions();
    fns.sort_by(|a, b| {
        a.category
            .as_str()
            .cmp(b.category.as_str())
            .then(a.name.cmp(b.name))
    });
    serde_json::json!({ "functions": fns })
}

/// 便捷构造：必填参数。
pub const fn p(name: &'static str, kind: ParamKind, hint: &'static str) -> Param {
    Param {
        name,
        kind,
        required: true,
        default: None,
        hint,
    }
}

/// 便捷构造：带默认值的可选参数。
pub const fn p_opt(
    name: &'static str,
    kind: ParamKind,
    default: Option<&'static str>,
    hint: &'static str,
) -> Param {
    Param {
        name,
        kind,
        required: false,
        default,
        hint,
    }
}
