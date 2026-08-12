//! cmx-rpt-model —— 报表模块的语义中立层。
//!
//! 对外导出：报表数据源 id、默认区域常量，以及报表工作台/设计器/应用器共享的请求 DTO
//! （既被 cmx-rpt-api 当 axum `Query`/`Json` 提取器，又被 cmx-rpt-store-pg 当查询构建输入）。
//! 本 crate 不依赖任何数据库/HTTP 框架，可被上层自由复用。

use serde::Deserialize;

/// 报表数据源 id（固定 fico-db 中的报表数据字典物理表）。
pub const RPT_DB_ID: &str = "fico-db";

/// 默认区域编码：不定义区域时，整个 sheet 的行/列/单元格挂到该默认区域下。
pub const DEFAULT_REGION: &str = "__default__";

/// 报表列表查询（GET /report-design/reports）：按类别/期间类型/关键字过滤。
#[derive(Debug, Deserialize, Default)]
pub struct ReportListQuery {
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub period_type: Option<String>,
    #[serde(default)]
    pub q: Option<String>,
}

/// 报表详情查询（GET /report-design/reports/{code}）：可选版本。
#[derive(Debug, Deserialize, Default)]
pub struct ReportDetailQuery {
    #[serde(default)]
    pub version: Option<String>,
}

/// 建版本请求体（POST /report-design/reports/{code}/versions）。
#[derive(Debug, Deserialize, Default)]
pub struct CreateVersionBody {
    #[serde(default)]
    pub code: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub base_version_code: Option<String>,
    #[serde(default)]
    pub change_summary: Option<String>,
    #[serde(default)]
    pub is_current: Option<bool>,
}

/// 版式加载查询（GET /report-design/reports/{code}/layout）：可选版本。
#[derive(Debug, Deserialize, Default)]
pub struct LayoutQuery {
    #[serde(default)]
    pub version: Option<String>,
}
