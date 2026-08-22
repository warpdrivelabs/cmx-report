//! consol —— 合并报表薄 handler + 路由表 `consol_routes::<S>()`。
//!
//! 与报表 handler 同姿态:只带 Query/Json 提取器 → 调 cmx_consol_store_pg 服务 → ApiResp 信封。
//! 对任意 state 泛型 S 成立,平台壳/独立壳复用同一核。

use axum::Json;
use axum::extract::Query;
use axum::routing::{get, post};
use axum::Router;
use serde::Deserialize;
use serde_json::Value;

use cmx_api_types::{ApiResp, Result};

use cmx_consol_store_pg as store;

#[derive(Debug, Deserialize)]
pub struct RunBody {
    pub scheme: String,
    pub period: String,
}

#[derive(Debug, Deserialize, Default)]
pub struct WsQuery {
    pub scheme: Option<String>,
    pub period: Option<String>,
    pub node: Option<String>,
    pub account: Option<String>,
}

// —— 主数据/输入录入 ——
pub async fn scheme_upsert(Json(b): Json<Value>) -> Result<Json<ApiResp<Value>>> {
    Ok(Json(ApiResp::ok(store::crud::upsert_scheme(&b).await?)))
}
pub async fn group_accounts_upsert(Json(b): Json<Value>) -> Result<Json<ApiResp<Value>>> {
    Ok(Json(ApiResp::ok(store::crud::upsert_group_accounts(&b).await?)))
}
pub async fn coa_mapping_upsert(Json(b): Json<Value>) -> Result<Json<ApiResp<Value>>> {
    Ok(Json(ApiResp::ok(store::crud::upsert_coa_mapping(&b).await?)))
}
pub async fn scope_upsert(Json(b): Json<Value>) -> Result<Json<ApiResp<Value>>> {
    Ok(Json(ApiResp::ok(store::crud::upsert_scope(&b).await?)))
}
pub async fn entity_balances_upsert(Json(b): Json<Value>) -> Result<Json<ApiResp<Value>>> {
    Ok(Json(ApiResp::ok(store::crud::upsert_entity_balances(&b).await?)))
}
pub async fn elim_rules_upsert(Json(b): Json<Value>) -> Result<Json<ApiResp<Value>>> {
    Ok(Json(ApiResp::ok(store::crud::upsert_elim_rules(&b).await?)))
}
pub async fn ic_matches_upsert(Json(b): Json<Value>) -> Result<Json<ApiResp<Value>>> {
    Ok(Json(ApiResp::ok(store::crud::upsert_ic_matches(&b).await?)))
}
pub async fn ic_declarations_upsert(Json(b): Json<Value>) -> Result<Json<ApiResp<Value>>> {
    Ok(Json(ApiResp::ok(store::crud::upsert_ic_declarations(&b).await?)))
}
// —— C4 内部往来对账 ——
pub async fn ic_reconcile(Json(b): Json<RunBody>) -> Result<Json<ApiResp<Value>>> {
    Ok(Json(ApiResp::ok(store::run_ic_reconciliation(&b.scheme, &b.period).await?)))
}
pub async fn ic_recon(Query(q): Query<WsQuery>) -> Result<Json<ApiResp<Value>>> {
    Ok(Json(ApiResp::ok(
        store::get_ic_recon(q.scheme.as_deref().unwrap_or(""), q.period.as_deref().unwrap_or("")).await?,
    )))
}
pub async fn fx_rates_upsert(Json(b): Json<Value>) -> Result<Json<ApiResp<Value>>> {
    Ok(Json(ApiResp::ok(store::crud::upsert_fx_rates(&b).await?)))
}
pub async fn interim_profit_upsert(Json(b): Json<Value>) -> Result<Json<ApiResp<Value>>> {
    Ok(Json(ApiResp::ok(store::crud::upsert_interim_profit(&b).await?)))
}
pub async fn goodwill_impair_upsert(Json(b): Json<Value>) -> Result<Json<ApiResp<Value>>> {
    Ok(Json(ApiResp::ok(store::crud::upsert_goodwill_impair(&b).await?)))
}
// —— L2 分步取得/处置交易 ——
pub async fn step_txn_upsert(Json(b): Json<Value>) -> Result<Json<ApiResp<Value>>> {
    Ok(Json(ApiResp::ok(store::crud::upsert_step_txns(&b).await?)))
}
// —— L3 固定资产内部交易未实现利润 ——
pub async fn fa_profit_upsert(Json(b): Json<Value>) -> Result<Json<ApiResp<Value>>> {
    Ok(Json(ApiResp::ok(store::crud::upsert_fa_profit(&b).await?)))
}
// —— L6 交叉持股·有效持股 ——
pub async fn shareholding_upsert(Json(b): Json<Value>) -> Result<Json<ApiResp<Value>>> {
    Ok(Json(ApiResp::ok(store::upsert_shareholdings(&b).await?)))
}
pub async fn effective_ownership(Query(q): Query<WsQuery>) -> Result<Json<ApiResp<Value>>> {
    Ok(Json(ApiResp::ok(store::compute_effective_ownership(
        q.scheme.as_deref().unwrap_or(""), q.period.as_deref().unwrap_or(""),
    ).await?)))
}
// —— L7 合并附注自动生成 ——
pub async fn notes(Query(q): Query<WsQuery>) -> Result<Json<ApiResp<Value>>> {
    Ok(Json(ApiResp::ok(store::generate_notes(
        q.scheme.as_deref().unwrap_or(""), q.period.as_deref().unwrap_or(""), q.node.as_deref(),
    ).await?)))
}
// —— N2 现金流量/权益变动流水录入 + 聚合 ——
pub async fn cash_flow_upsert(Json(b): Json<Value>) -> Result<Json<ApiResp<Value>>> {
    Ok(Json(ApiResp::ok(store::crud::upsert_cash_flow_items(&b).await?)))
}
pub async fn equity_change_upsert(Json(b): Json<Value>) -> Result<Json<ApiResp<Value>>> {
    Ok(Json(ApiResp::ok(store::crud::upsert_equity_changes(&b).await?)))
}
pub async fn cashflow_run(Json(b): Json<RunBody>) -> Result<Json<ApiResp<Value>>> {
    Ok(Json(ApiResp::ok(store::run_cashflow(&b.scheme, &b.period).await?)))
}
pub async fn equity_run(Json(b): Json<RunBody>) -> Result<Json<ApiResp<Value>>> {
    Ok(Json(ApiResp::ok(store::run_equity_change(&b.scheme, &b.period).await?)))
}
// —— L5 现金流量表·工作底稿法 ——
#[derive(Debug, Deserialize)]
pub struct CfWorksheetBody {
    pub scheme: String,
    pub period: String,
    #[serde(default)]
    pub prev_period: Option<String>,
}
pub async fn cashflow_worksheet_run(Json(b): Json<CfWorksheetBody>) -> Result<Json<ApiResp<Value>>> {
    Ok(Json(ApiResp::ok(
        store::run_cashflow_worksheet(&b.scheme, &b.period, b.prev_period.as_deref()).await?,
    )))
}
pub async fn cash_flow_get(Query(q): Query<WsQuery>) -> Result<Json<ApiResp<Value>>> {
    Ok(Json(ApiResp::ok(store::get_cash_flow(
        q.scheme.as_deref().unwrap_or(""), q.period.as_deref().unwrap_or(""), q.node.as_deref().unwrap_or(""),
    ).await?)))
}
pub async fn equity_change_get(Query(q): Query<WsQuery>) -> Result<Json<ApiResp<Value>>> {
    Ok(Json(ApiResp::ok(store::get_equity_change(
        q.scheme.as_deref().unwrap_or(""), q.period.as_deref().unwrap_or(""), q.node.as_deref().unwrap_or(""),
    ).await?)))
}

// —— N3 关账编排 ——
#[derive(Debug, Deserialize)]
pub struct CloseAdvanceBody {
    pub scheme: String,
    pub period: String,
    #[serde(default)]
    pub step: Option<String>,
    #[serde(default)]
    pub approve: bool,
}
pub async fn close_start(Json(b): Json<RunBody>) -> Result<Json<ApiResp<Value>>> {
    Ok(Json(ApiResp::ok(store::start_close(&b.scheme, &b.period).await?)))
}
pub async fn close_advance(Json(b): Json<CloseAdvanceBody>) -> Result<Json<ApiResp<Value>>> {
    Ok(Json(ApiResp::ok(
        store::advance_close(&b.scheme, &b.period, b.step.as_deref(), b.approve).await?,
    )))
}
pub async fn close_reopen(Json(b): Json<RunBody>) -> Result<Json<ApiResp<Value>>> {
    Ok(Json(ApiResp::ok(store::reopen_close(&b.scheme, &b.period).await?)))
}
pub async fn close_status(Query(q): Query<WsQuery>) -> Result<Json<ApiResp<Value>>> {
    Ok(Json(ApiResp::ok(store::get_close_status(
        q.scheme.as_deref().unwrap_or(""), q.period.as_deref().unwrap_or(""),
    ).await?)))
}

// —— 运行合并 ——
pub async fn run(Json(b): Json<RunBody>) -> Result<Json<ApiResp<Value>>> {
    Ok(Json(ApiResp::ok(store::run_consolidation(&b.scheme, &b.period).await?)))
}
// —— C7 出表:seed 合并四表模板 ——
pub async fn seed_statements(Json(_b): Json<Value>) -> Result<Json<ApiResp<Value>>> {
    Ok(Json(ApiResp::ok(store::seed_consol_statements().await?)))
}
// —— C7 范围变动 ——
#[derive(Debug, Deserialize)]
pub struct ScopeChangeBody {
    pub scheme: String,
    pub period: String,
    #[serde(default)]
    pub prev_period: Option<String>,
}
pub async fn scope_change_run(Json(b): Json<ScopeChangeBody>) -> Result<Json<ApiResp<Value>>> {
    Ok(Json(ApiResp::ok(
        store::run_scope_change(&b.scheme, &b.period, b.prev_period.as_deref()).await?,
    )))
}
pub async fn scope_change(Query(q): Query<WsQuery>) -> Result<Json<ApiResp<Value>>> {
    Ok(Json(ApiResp::ok(
        store::get_scope_change(q.scheme.as_deref().unwrap_or(""), q.period.as_deref().unwrap_or("")).await?,
    )))
}

// —— 查询 ——
pub async fn schemes(_q: Query<WsQuery>) -> Result<Json<ApiResp<Value>>> {
    Ok(Json(ApiResp::ok(store::list_schemes().await?)))
}
pub async fn periods(Query(q): Query<WsQuery>) -> Result<Json<ApiResp<Value>>> {
    Ok(Json(ApiResp::ok(store::list_periods(q.scheme.as_deref().unwrap_or("")).await?)))
}
pub async fn nodes(Query(q): Query<WsQuery>) -> Result<Json<ApiResp<Value>>> {
    Ok(Json(ApiResp::ok(
        store::list_nodes(q.scheme.as_deref().unwrap_or(""), q.period.as_deref().unwrap_or("")).await?,
    )))
}
pub async fn accounts(Query(q): Query<WsQuery>) -> Result<Json<ApiResp<Value>>> {
    Ok(Json(ApiResp::ok(store::list_accounts(q.scheme.as_deref().unwrap_or("")).await?)))
}
pub async fn worksheet(Query(q): Query<WsQuery>) -> Result<Json<ApiResp<Value>>> {
    Ok(Json(ApiResp::ok(
        store::get_worksheet(
            q.scheme.as_deref().unwrap_or(""),
            q.period.as_deref().unwrap_or(""),
            q.node.as_deref().unwrap_or(""),
        )
        .await?,
    )))
}
pub async fn journal(Query(q): Query<WsQuery>) -> Result<Json<ApiResp<Value>>> {
    Ok(Json(ApiResp::ok(
        store::get_elim_journal(
            q.scheme.as_deref().unwrap_or(""),
            q.period.as_deref().unwrap_or(""),
            q.node.as_deref().unwrap_or(""),
        )
        .await?,
    )))
}
pub async fn value(Query(q): Query<WsQuery>) -> Result<Json<ApiResp<Value>>> {
    Ok(Json(ApiResp::ok(
        store::consol_value(
            q.scheme.as_deref().unwrap_or(""),
            q.period.as_deref().unwrap_or(""),
            q.node.as_deref().unwrap_or(""),
            q.account.as_deref().unwrap_or(""),
        )
        .await?,
    )))
}

/// 合并路由表(对任意 state 泛型 S 成立)。宿主 merge 或 nest("/api", …)。
pub fn consol_routes<S>() -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    Router::new()
        .route("/consol/schemes", get(schemes).post(scheme_upsert))
        .route("/consol/periods", get(periods))
        .route("/consol/nodes", get(nodes))
        .route("/consol/accounts", get(accounts))
        .route("/consol/group-accounts", post(group_accounts_upsert))
        .route("/consol/coa-mapping", post(coa_mapping_upsert))
        .route("/consol/scope", post(scope_upsert))
        .route("/consol/entity-balances", post(entity_balances_upsert))
        .route("/consol/elim-rules", post(elim_rules_upsert))
        .route("/consol/ic-matches", post(ic_matches_upsert))
        .route("/consol/ic-declarations", post(ic_declarations_upsert))
        .route("/consol/ic-reconcile", post(ic_reconcile))
        .route("/consol/ic-recon", get(ic_recon))
        .route("/consol/fx-rates", post(fx_rates_upsert))
        .route("/consol/interim-profit", post(interim_profit_upsert))
        .route("/consol/goodwill-impair", post(goodwill_impair_upsert))
        .route("/consol/step-txn", post(step_txn_upsert))
        .route("/consol/fa-profit", post(fa_profit_upsert))
        .route("/consol/shareholding", post(shareholding_upsert))
        .route("/consol/effective-ownership", get(effective_ownership))
        .route("/consol/notes", get(notes))
        .route("/consol/cash-flow", post(cash_flow_upsert).get(cash_flow_get))
        .route("/consol/equity-change", post(equity_change_upsert).get(equity_change_get))
        .route("/consol/cashflow/run", post(cashflow_run))
        .route("/consol/cashflow/worksheet", post(cashflow_worksheet_run))
        .route("/consol/equity/run", post(equity_run))
        .route("/consol/close/start", post(close_start))
        .route("/consol/close/advance", post(close_advance))
        .route("/consol/close/reopen", post(close_reopen))
        .route("/consol/close/status", get(close_status))
        .route("/consol/run", post(run))
        .route("/consol/seed-statements", post(seed_statements))
        .route("/consol/scope-change", get(scope_change).post(scope_change_run))
        .route("/consol/worksheet", get(worksheet))
        .route("/consol/journal", get(journal))
        .route("/consol/value", get(value))
}
