//! 报表平台监控大盘（对标 cmx-flow-app 的 dashboard + stats）。
//!
//! 两件事都自包含、独立 server 与平台内嵌壳皆可用：
//!   - [`dashboard`]：`GET /` 返回自包含单页 HTML（内联 CSS/JS，light/dark，轮询 stats），
//!     `include_str!` 编进二进制，distroless 亦可用。
//!   - [`rpt_stats`]：`GET /api/rpt/stats` 聚合报表域指标。只读、失败降级空/零，保证大盘总能出盘。
//!
//! 数据来自 store 层公共读服务 [`store::overview`]（categories/periods/reports），不在 app 层直连 DB。

use axum::Json;
use axum::response::Html;
use serde_json::{Value, json};

use cmx_api_types::ApiResp;
use cmx_rpt_store_pg as store;

/// 大盘页（自包含 HTML，轮询 `/api/rpt/stats`）。
pub async fn dashboard() -> Html<&'static str> {
    Html(include_str!("dashboard.html"))
}

/// 报表域指标聚合（KPI + 分类/期间/报表清单）。失败降级空集，绝不让大盘 500。
pub async fn rpt_stats() -> Json<ApiResp<Value>> {
    // store::overview() 返回 { dbId, categories[], periods[], reports[] }；任何失败均降级为空对象。
    let ov = store::overview().await.unwrap_or_else(|_| json!({}));
    let arr = |k: &str| ov.get(k).and_then(|v| v.as_array()).cloned().unwrap_or_default();
    let categories = arr("categories");
    let periods = arr("periods");
    let reports = arr("reports");
    let db_id = ov.get("dbId").cloned().unwrap_or(Value::Null);

    Json(ApiResp::ok(json!({
        "dbId": db_id,
        "kpi": {
            "reports": reports.len(),
            "categories": categories.len(),
            "periods": periods.len(),
        },
        "categories": categories,
        "periods": periods,
        "reports": reports,
    })))
}
