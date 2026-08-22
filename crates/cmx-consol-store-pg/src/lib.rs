//! cmx-consol-store-pg —— 合并报表持久化 + 引擎编排(PostgreSQL)。
//!
//! 承载 cg_* 表读写与合并七段流水线的 DB 编排:装载(方案/范围/个别数/规则/往来)→
//! 逐级合并(自底向上)→ 生成抵销凭证 + 工作底稿 → 落 cg_elim_journal / cg_consol_data。
//! 纯算法在 `cmx-consol-model`(借方正 signed),本层只做装载/落库/编排。

use std::collections::{BTreeMap, HashMap, HashSet};

use rust_decimal::Decimal;
use serde_json::{Value, json};

use cmx_core::model::cell::{DataValue, SqlTypeMarker};
use cmx_database_pg::get_default_pg_db_manager;

use cmx_consol_model::{
    AccountType, CONSOL_DB_ID, CapitalCfg, Contribution, ConsolMethod, ElimEntry, FixedAssetProfit,
    FxRates, IcMatch, IcDeclaration, InventoryProfit, ScopeChange, ScopeNode, aggregate,
    capital_elimination, common_control_elimination, debt_elimination, diff_scope, disposal,
    dividend_elimination, equity_pickup, fixed_asset_profit_elimination, goodwill_impairment,
    inventory_profit_elimination, minority_pl, reconcile, sales_elimination, step_acquisition,
    translate_entity, worksheet,
};

pub use cmx_api_types::{Error, Result};
pub use cmx_biz::api_err;

pub mod crud;
pub mod cashflow;
pub mod close;
pub mod crossholding;
pub mod flow_client;
pub mod notes;
pub mod statements;
pub use cashflow::{run_cashflow, run_equity_change, run_cashflow_worksheet, get_cash_flow, get_equity_change};
pub use close::{advance_close, get_close_status, reopen_close, start_close};
pub use crossholding::{compute_effective_ownership, upsert_shareholdings};
pub use notes::generate_notes;
pub use statements::seed_consol_statements;

// ============================================================================
// DB 门面
// ============================================================================

fn db_err(e: cmx_database_pg::Error) -> Error {
    cmx_biz::BizError::from_db_error(&cmx_database_pg::pg_detail(&e)).into()
}

/// DataValue 参数查询 → 行数组。
pub(crate) async fn query_rows(sql: &str, params: Vec<DataValue>, label: &str) -> Result<Vec<Value>> {
    let mm = get_default_pg_db_manager();
    let ds = mm
        .query_sql_with_datavalues(CONSOL_DB_ID, None, sql, params, label)
        .await
        .map_err(|e| {
            tracing::error!(target: "consol::store", query_label=label, query_sql=sql, pg_detail=%cmx_database_pg::pg_detail(&e), "合并查询失败");
            db_err(e)
        })?;
    let v = serde_json::to_value(&ds).map_err(|e| api_err(&format!("结果序列化失败: {e}")))?;
    Ok(v.get("rows").and_then(|r| r.as_array()).cloned().unwrap_or_default())
}

/// 非事务执行。
pub(crate) async fn execute(sql: &str, params: Vec<DataValue>) -> Result<()> {
    let mm = get_default_pg_db_manager();
    mm.execute_sql_with_datavalues(CONSOL_DB_ID, None, sql, params)
        .await
        .map_err(|e| {
            tracing::error!(target: "consol::store", sql=sql, pg_detail=%cmx_database_pg::pg_detail(&e), "合并落库失败");
            db_err(e)
        })?;
    Ok(())
}

// —— 值助手 ——
pub(crate) fn sv(r: &Value, k: &str) -> Option<String> {
    r.get(k).and_then(|v| v.as_str()).map(str::to_owned)
}
pub(crate) fn iv(r: &Value, k: &str) -> Option<i64> {
    r.get(k).and_then(|v| match v {
        Value::Number(n) => n.as_i64(),
        Value::String(s) => s.parse().ok(),
        Value::Bool(b) => Some(*b as i64),
        _ => None,
    })
}
/// 从 JSON 值取 Decimal(容 String / Number)。
pub(crate) fn dv_dec(r: &Value, k: &str) -> Decimal {
    match r.get(k) {
        Some(Value::String(s)) => s.parse().unwrap_or(Decimal::ZERO),
        Some(Value::Number(n)) => n
            .as_f64()
            .and_then(rust_decimal::Decimal::from_f64_retain)
            .unwrap_or(Decimal::ZERO),
        _ => Decimal::ZERO,
    }
}
fn s_body(b: &Value, k: &str) -> String {
    b.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string()
}

// ============================================================================
// 装载
// ============================================================================

/// 资本抵销科目配置(来自方案)。
async fn load_capital_cfg(scheme: &str) -> Result<CapitalCfg> {
    let rows = query_rows(
        "SELECT investment_account, goodwill_account, nci_account, minority_pl_account, capital_reserve_account \
         FROM cg_consol_scheme WHERE scheme_code=$1",
        vec![DataValue::String(scheme.to_string())],
        "consol_scheme",
    )
    .await?;
    let r = rows.first().ok_or_else(|| api_err("合并方案不存在"))?;
    Ok(CapitalCfg {
        investment_account: sv(r, "investment_account").unwrap_or_else(|| "1511".into()),
        goodwill_account: sv(r, "goodwill_account").unwrap_or_else(|| "1801".into()),
        nci_account: sv(r, "nci_account").unwrap_or_else(|| "4400".into()),
        minority_pl_account: sv(r, "minority_pl_account").unwrap_or_else(|| "4900".into()),
        capital_reserve_account: sv(r, "capital_reserve_account").unwrap_or_else(|| "4002".into()),
    })
}

/// 方案的币种/折算配置。
struct SchemeFx {
    group_currency: String,
    cta_account: String,
}
async fn load_scheme_fx(scheme: &str) -> Result<SchemeFx> {
    let rows = query_rows(
        "SELECT group_currency, cta_account FROM cg_consol_scheme WHERE scheme_code=$1",
        vec![DataValue::String(scheme.to_string())],
        "consol_scheme_fx",
    )
    .await?;
    let r = rows.first().cloned().unwrap_or_else(|| json!({}));
    Ok(SchemeFx {
        group_currency: sv(&r, "group_currency").unwrap_or_else(|| "CNY".into()),
        cta_account: sv(&r, "cta_account").unwrap_or_else(|| "4106".into()),
    })
}

/// 汇率表:(from_ccy, rate_type) → rate(默认 to=集团币,故 to 维度略)。
async fn load_fx_rates(scheme: &str, period: &str) -> Result<HashMap<(String, String), Decimal>> {
    let rows = query_rows(
        "SELECT from_ccy, rate_type, rate FROM cg_fx_rate \
         WHERE scheme_code=$1 AND period_code=$2 AND COALESCE(status,1)=1",
        vec![DataValue::String(scheme.to_string()), DataValue::String(period.to_string())],
        "consol_fx_rate",
    )
    .await?;
    Ok(rows
        .iter()
        .filter_map(|r| {
            Some((
                (sv(r, "from_ccy")?, sv(r, "rate_type")?.to_ascii_lowercase()),
                dv_dec(r, "rate"),
            ))
        })
        .collect())
}

/// 合并范围节点(某方案某期);附带每节点的投资额与功能币。
pub(crate) struct ScopeLoaded {
    pub(crate) nodes: Vec<ScopeNode>,
    investment: HashMap<String, Decimal>,
    /// org_code → 功能币种(空=随集团报告币,不折算)。
    currency: HashMap<String, String>,
    /// 同一控制下企业合并的子节点集(L1:资本抵销走权益结合法,差额入资本公积不确认商誉)。
    common_control: HashSet<String>,
}

pub(crate) async fn load_scope(scheme: &str, period: &str) -> Result<ScopeLoaded> {
    let rows = query_rows(
        "SELECT org_code, org_name, parent_code, consol_method, ownership_pct, is_leaf, \
                level_no, investment_amount, currency, under_common_control \
         FROM cg_scope WHERE scheme_code=$1 AND period_code=$2 AND COALESCE(status,1)=1 \
         ORDER BY level_no, org_code",
        vec![DataValue::String(scheme.to_string()), DataValue::String(period.to_string())],
        "consol_scope",
    )
    .await?;
    let mut nodes = Vec::new();
    let mut investment = HashMap::new();
    let mut currency = HashMap::new();
    let mut common_control = HashSet::new();
    for r in &rows {
        let code = sv(r, "org_code").unwrap_or_default();
        investment.insert(code.clone(), dv_dec(r, "investment_amount"));
        if let Some(c) = sv(r, "currency").filter(|s| !s.is_empty()) {
            currency.insert(code.clone(), c);
        }
        if iv(r, "under_common_control").unwrap_or(0) == 1 {
            common_control.insert(code.clone());
        }
        nodes.push(ScopeNode {
            code: code.clone(),
            name: sv(r, "org_name").unwrap_or_default(),
            parent: sv(r, "parent_code").filter(|s| !s.is_empty()),
            method: ConsolMethod::parse(&sv(r, "consol_method").unwrap_or_default()),
            ownership: {
                let o = dv_dec(r, "ownership_pct");
                if o == Decimal::ZERO { Decimal::ONE } else { o }
            },
            is_leaf: iv(r, "is_leaf").unwrap_or(0) == 1,
            level: iv(r, "level_no").unwrap_or(1) as i32,
        });
    }
    Ok(ScopeLoaded { nodes, investment, currency, common_control })
}

/// CoA 科目映射(C1):(主体, 本地科目) → (集团科目, 符号)。entity_code 为空=通配所有主体。
/// 无映射的主体/科目在装载时直通(本地科目即集团科目,符号 1)——故未配置映射零影响。
async fn load_coa_mapping(scheme: &str) -> Result<HashMap<(String, String), (String, Decimal)>> {
    let rows = query_rows(
        "SELECT entity_code, local_account, group_account, sign FROM cg_coa_mapping \
         WHERE scheme_code=$1 AND COALESCE(status,1)=1",
        vec![DataValue::String(scheme.to_string())],
        "consol_coa_mapping",
    )
    .await?;
    let mut out = HashMap::new();
    for r in &rows {
        let ent = sv(r, "entity_code").unwrap_or_default();
        let local = match sv(r, "local_account").filter(|s| !s.is_empty()) {
            Some(s) => s,
            None => continue,
        };
        let group = match sv(r, "group_account").filter(|s| !s.is_empty()) {
            Some(s) => s,
            None => continue,
        };
        let sign = {
            let s = dv_dec(r, "sign");
            if s == Decimal::ZERO { Decimal::ONE } else { s }
        };
        out.insert((ent, local), (group, sign));
    }
    Ok(out)
}

/// 个别试算表:entity → (集团科目 → 借方正金额)。经 CoA 映射把本地科目归一到集团科目。
async fn load_entity_balances(
    scheme: &str,
    period: &str,
    coa: &HashMap<(String, String), (String, Decimal)>,
) -> Result<HashMap<String, BTreeMap<String, Decimal>>> {
    let rows = query_rows(
        "SELECT entity_code, account_code, amount FROM cg_entity_balance \
         WHERE scheme_code=$1 AND period_code=$2 AND COALESCE(status,1)=1",
        vec![DataValue::String(scheme.to_string()), DataValue::String(period.to_string())],
        "consol_entity_balance",
    )
    .await?;
    let mut out: HashMap<String, BTreeMap<String, Decimal>> = HashMap::new();
    for r in &rows {
        let ent = sv(r, "entity_code").unwrap_or_default();
        let local = sv(r, "account_code").unwrap_or_default();
        let amt = dv_dec(r, "amount");
        // 映射优先级:(主体,本地科目) → (通配主体"",本地科目) → 直通(本地即集团,符号1)。
        let (group, sign) = coa
            .get(&(ent.clone(), local.clone()))
            .or_else(|| coa.get(&(String::new(), local.clone())))
            .cloned()
            .unwrap_or((local.clone(), Decimal::ONE));
        *out.entry(ent).or_default().entry(group).or_insert(Decimal::ZERO) += amt * sign;
    }
    Ok(out)
}

async fn load_account_types(scheme: &str) -> Result<HashMap<String, String>> {
    let rows = query_rows(
        "SELECT account_code, account_type FROM cg_group_account \
         WHERE scheme_code=$1 AND COALESCE(status,1)=1",
        vec![DataValue::String(scheme.to_string())],
        "consol_group_account",
    )
    .await?;
    Ok(rows
        .iter()
        .filter_map(|r| Some((sv(r, "account_code")?, sv(r, "account_type")?.to_ascii_lowercase())))
        .collect())
}

/// 集团科目性质(pub(crate) 包装,供 cashflow L5 工作底稿法分类账户用)。
pub(crate) async fn load_account_types_pub(scheme: &str) -> Result<HashMap<String, String>> {
    load_account_types(scheme).await
}

/// 抵销规则:elim_type → (dr_account, cr_account, rule_code)。取每类第一条启用规则。
async fn load_elim_rules(scheme: &str) -> Result<HashMap<String, (String, String, String)>> {
    let rows = query_rows(
        "SELECT rule_code, elim_type, dr_account, cr_account FROM cg_elim_rule \
         WHERE scheme_code=$1 AND COALESCE(enabled,1)=1 AND COALESCE(status,1)=1 \
         ORDER BY elim_type, sort_no, rule_code",
        vec![DataValue::String(scheme.to_string())],
        "consol_elim_rule",
    )
    .await?;
    let mut out = HashMap::new();
    for r in &rows {
        let et = sv(r, "elim_type").unwrap_or_default();
        out.entry(et).or_insert_with(|| {
            (
                sv(r, "dr_account").unwrap_or_default(),
                sv(r, "cr_account").unwrap_or_default(),
                sv(r, "rule_code").unwrap_or_default(),
            )
        });
    }
    Ok(out)
}

/// 内部往来匹配。
async fn load_ic_matches(scheme: &str, period: &str) -> Result<Vec<(String, IcMatch)>> {
    let rows = query_rows(
        "SELECT entity_a, entity_b, ic_type, amount FROM cg_ic_match \
         WHERE scheme_code=$1 AND period_code=$2 AND COALESCE(status,1)=1",
        vec![DataValue::String(scheme.to_string()), DataValue::String(period.to_string())],
        "consol_ic_match",
    )
    .await?;
    Ok(rows
        .iter()
        .map(|r| {
            (
                sv(r, "ic_type").unwrap_or_default(),
                IcMatch {
                    entity_a: sv(r, "entity_a").unwrap_or_default(),
                    entity_b: sv(r, "entity_b").unwrap_or_default(),
                    amount: dv_dec(r, "amount"),
                },
            )
        })
        .collect())
}

/// 存货未实现内部利润(期初/期末)——C6 抵销+期初结转输入。
async fn load_interim_profit(scheme: &str, period: &str) -> Result<Vec<InventoryProfit>> {
    let rows = query_rows(
        "SELECT seller, buyer, opening_profit, ending_profit FROM cg_interim_profit \
         WHERE scheme_code=$1 AND period_code=$2 AND COALESCE(status,1)=1",
        vec![DataValue::String(scheme.to_string()), DataValue::String(period.to_string())],
        "consol_interim_profit",
    )
    .await?;
    Ok(rows
        .iter()
        .map(|r| InventoryProfit {
            seller: sv(r, "seller").unwrap_or_default(),
            buyer: sv(r, "buyer").unwrap_or_default(),
            opening: dv_dec(r, "opening_profit"),
            ending: dv_dec(r, "ending_profit"),
        })
        .collect())
}

/// 商誉减值输入(C6):合并节点 → 本期减值额(自然口径,正)。
async fn load_goodwill_impair(scheme: &str, period: &str) -> Result<HashMap<String, Decimal>> {
    let rows = query_rows(
        "SELECT node_code, amount FROM cg_goodwill_impair \
         WHERE scheme_code=$1 AND period_code=$2 AND COALESCE(status,1)=1",
        vec![DataValue::String(scheme.to_string()), DataValue::String(period.to_string())],
        "consol_goodwill_impair",
    )
    .await?;
    let mut out: HashMap<String, Decimal> = HashMap::new();
    for r in &rows {
        if let Some(node) = sv(r, "node_code").filter(|s| !s.is_empty()) {
            *out.entry(node).or_insert(Decimal::ZERO) += dv_dec(r, "amount");
        }
    }
    Ok(out)
}

/// L2 分步取得/处置交易(某方案某期);按合并节点归组。
struct StepTxn {
    node: String,
    txn_type: String,
    loses_control: bool,
    prev_carrying: Decimal,
    prev_fair_value: Decimal,
    proceeds: Decimal,
    disposed_share: Decimal,
    retained_fair_value: Decimal,
    net_assets_share: Decimal,
}

async fn load_step_txns(scheme: &str, period: &str) -> Result<Vec<StepTxn>> {
    let rows = query_rows(
        "SELECT node_code, txn_type, loses_control, prev_carrying, prev_fair_value, proceeds, \
                disposed_share, retained_fair_value, net_assets_share FROM cg_step_txn \
         WHERE scheme_code=$1 AND period_code=$2 AND COALESCE(status,1)=1",
        vec![DataValue::String(scheme.to_string()), DataValue::String(period.to_string())],
        "consol_step_txn",
    )
    .await?;
    Ok(rows
        .iter()
        .filter_map(|r| {
            let node = sv(r, "node_code").filter(|s| !s.is_empty())?;
            Some(StepTxn {
                node,
                txn_type: sv(r, "txn_type").unwrap_or_default(),
                loses_control: iv(r, "loses_control").unwrap_or(0) == 1,
                prev_carrying: dv_dec(r, "prev_carrying"),
                prev_fair_value: dv_dec(r, "prev_fair_value"),
                proceeds: dv_dec(r, "proceeds"),
                disposed_share: dv_dec(r, "disposed_share"),
                retained_fair_value: dv_dec(r, "retained_fair_value"),
                net_assets_share: dv_dec(r, "net_assets_share"),
            })
        })
        .collect())
}

/// L3 固定资产内部交易未实现利润(某方案某期);LCA 抵销输入。
async fn load_fa_profit(scheme: &str, period: &str) -> Result<Vec<FixedAssetProfit>> {
    let rows = query_rows(
        "SELECT seller, buyer, unrealized, dep_years, elapsed_years FROM cg_fa_profit \
         WHERE scheme_code=$1 AND period_code=$2 AND COALESCE(status,1)=1",
        vec![DataValue::String(scheme.to_string()), DataValue::String(period.to_string())],
        "consol_fa_profit",
    )
    .await?;
    Ok(rows
        .iter()
        .map(|r| FixedAssetProfit {
            seller: sv(r, "seller").unwrap_or_default(),
            buyer: sv(r, "buyer").unwrap_or_default(),
            unrealized: dv_dec(r, "unrealized"),
            dep_years: dv_dec(r, "dep_years"),
            elapsed_years: dv_dec(r, "elapsed_years"),
        })
        .collect())
}

// ============================================================================
// C4 内部往来对账引擎:申报 → 双边配对 → 差异检测 → matched 回填抵销输入
// ============================================================================

/// 各主体申报的内部往来两侧头寸。
async fn load_ic_declarations(scheme: &str, period: &str) -> Result<Vec<IcDeclaration>> {
    let rows = query_rows(
        "SELECT entity_code, partner_code, ic_type, direction, amount FROM cg_ic_declaration \
         WHERE scheme_code=$1 AND period_code=$2 AND COALESCE(status,1)=1",
        vec![DataValue::String(scheme.to_string()), DataValue::String(period.to_string())],
        "consol_ic_declaration",
    )
    .await?;
    Ok(rows
        .iter()
        .map(|r| IcDeclaration {
            entity: sv(r, "entity_code").unwrap_or_default(),
            partner: sv(r, "partner_code").unwrap_or_default(),
            ic_type: sv(r, "ic_type").unwrap_or_default(),
            direction: sv(r, "direction").unwrap_or_default(),
            amount: dv_dec(r, "amount"),
        })
        .collect())
}

/// 运行内部往来对账(C4):申报 → 双边配对 → 写 cg_ic_recon(差异工作台);
/// matched(min A/B)回填 cg_ic_match 供抵销引擎消费(差异保留至查明,不硬抵销)。
pub async fn run_ic_reconciliation(scheme: &str, period: &str) -> Result<Value> {
    let decls = load_ic_declarations(scheme, period).await?;
    let results = reconcile(&decls);
    // 清本期对账结果(幂等重算)。
    execute(
        "DELETE FROM cg_ic_recon WHERE scheme_code=$1 AND period_code=$2",
        vec![DataValue::String(scheme.to_string()), DataValue::String(period.to_string())],
    )
    .await?;
    let (mut matched_n, mut diff_n, mut one_n) = (0usize, 0usize, 0usize);
    for r in &results {
        match r.status.as_str() {
            "matched" => matched_n += 1,
            "diff" => diff_n += 1,
            _ => one_n += 1,
        }
        execute(
            "INSERT INTO cg_ic_recon (id, code, name, scheme_code, period_code, entity_a, entity_b, \
                ic_type, a_amount, b_amount, matched, diff, recon_status, sort_no, status, create_time, update_time) \
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,0,1,CURRENT_TIMESTAMP,CURRENT_TIMESTAMP)",
            vec![
                DataValue::Int(cmx_utils::next_pk_id()),
                DataValue::String(format!("{scheme}|{period}|{}|{}|{}", r.entity_a, r.entity_b, r.ic_type)),
                DataValue::NullTyped(SqlTypeMarker::Text),
                DataValue::String(scheme.to_string()),
                DataValue::String(period.to_string()),
                DataValue::String(r.entity_a.clone()),
                DataValue::String(r.entity_b.clone()),
                DataValue::String(r.ic_type.clone()),
                DataValue::Decimal(r.a_amount),
                DataValue::Decimal(r.b_amount),
                DataValue::Decimal(r.matched),
                DataValue::Decimal(r.diff),
                DataValue::String(r.status.clone()),
            ],
        )
        .await?;
        // matched 回填 cg_ic_match(matched>0),供抵销引擎消费。
        if r.matched != Decimal::ZERO {
            execute(
                "INSERT INTO cg_ic_match (id, code, name, scheme_code, period_code, entity_a, entity_b, ic_type, amount, status, sort_no, create_time, update_time) \
                 VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,1,0,CURRENT_TIMESTAMP,CURRENT_TIMESTAMP) \
                 ON CONFLICT (scheme_code, period_code, entity_a, entity_b, ic_type) DO UPDATE SET amount=EXCLUDED.amount, update_time=CURRENT_TIMESTAMP",
                vec![
                    DataValue::Int(cmx_utils::next_pk_id()),
                    DataValue::String(format!("{scheme}|{period}|{}|{}|{}", r.entity_a, r.entity_b, r.ic_type)),
                    DataValue::NullTyped(SqlTypeMarker::Text),
                    DataValue::String(scheme.to_string()),
                    DataValue::String(period.to_string()),
                    DataValue::String(r.entity_a.clone()),
                    DataValue::String(r.entity_b.clone()),
                    DataValue::String(r.ic_type.clone()),
                    DataValue::Decimal(r.matched),
                ],
            )
            .await?;
        }
    }
    Ok(json!({
        "ok": true, "scheme": scheme, "period": period,
        "pairs": results.len(), "matched": matched_n, "diff": diff_n, "one_sided": one_n,
        "message": format!("对账完成:{}对({}平/{}差异/{}单边)", results.len(), matched_n, diff_n, one_n),
    }))
}

/// 查内部往来对账结果(差异工作台)。
pub async fn get_ic_recon(scheme: &str, period: &str) -> Result<Value> {
    let rows = query_rows(
        "SELECT entity_a, entity_b, ic_type, a_amount, b_amount, matched, diff, recon_status \
         FROM cg_ic_recon WHERE scheme_code=$1 AND period_code=$2 \
         ORDER BY entity_a, entity_b, ic_type",
        vec![DataValue::String(scheme.to_string()), DataValue::String(period.to_string())],
        "consol_ic_recon",
    )
    .await?;
    Ok(json!({ "scheme": scheme, "period": period, "count": rows.len(), "rows": rows }))
}

// ============================================================================
// 合并引擎编排:自底向上逐级合并
// ============================================================================

/// 运行一次合并。清该 scheme+period 的派生数据,逐级重算,落 cg_consol_data + cg_elim_journal。
/// 返回 { ok, nodes, entries, message }。
pub async fn run_consolidation(scheme: &str, period: &str) -> Result<Value> {
    let cfg = load_capital_cfg(scheme).await?;
    let scope = load_scope(scheme, period).await?;
    if scope.nodes.is_empty() {
        return Err(api_err("该方案该期间未配置合并范围"));
    }
    let coa = load_coa_mapping(scheme).await?;
    let ent_bal = load_entity_balances(scheme, period, &coa).await?;
    let acc_type = load_account_types(scheme).await?;
    let rules = load_elim_rules(scheme).await?;
    let ic = load_ic_matches(scheme, period).await?;
    let interim = load_interim_profit(scheme, period).await?;
    let goodwill_impair = load_goodwill_impair(scheme, period).await?;
    let step_txns = load_step_txns(scheme, period).await?;
    let fa_profit = load_fa_profit(scheme, period).await?;
    // 外币折算配置(C5):集团报告币 + CTA 科目 + 本期汇率。
    let fx = load_scheme_fx(scheme).await?;
    let fx_rates = load_fx_rates(scheme, period).await?;

    // 建父→子索引 + 各节点子树成员(含自身)。
    let by_code: HashMap<String, &ScopeNode> = scope.nodes.iter().map(|n| (n.code.clone(), n)).collect();
    let mut children: HashMap<String, Vec<String>> = HashMap::new();
    for n in &scope.nodes {
        if let Some(p) = &n.parent {
            children.entry(p.clone()).or_default().push(n.code.clone());
        }
    }
    let subtree = compute_subtrees(&scope.nodes, &children);

    // 处理顺序:层级深→浅(叶子先,根最后)。
    let mut order: Vec<&ScopeNode> = scope.nodes.iter().collect();
    order.sort_by(|a, b| b.level.cmp(&a.level));

    // 每节点算出的"合并数"(account → 借方正),供上级聚合。
    let mut node_consolidated: HashMap<String, BTreeMap<String, Decimal>> = HashMap::new();

    // 清派生。
    execute(
        "DELETE FROM cg_consol_data WHERE scheme_code=$1 AND period_code=$2",
        vec![DataValue::String(scheme.to_string()), DataValue::String(period.to_string())],
        )
        .await?;
    execute(
        "DELETE FROM cg_elim_journal WHERE scheme_code=$1 AND period_code=$2",
        vec![DataValue::String(scheme.to_string()), DataValue::String(period.to_string())],
    )
    .await?;

    let mut total_entries = 0usize;
    let mut doc_seq = 0i64;

    for node in order {
        if node.is_leaf {
            // 叶子:合并数 = 其个别数(直接);功能币 ≠ 集团报告币 → 先折算(C5)。
            let mut bal = ent_bal.get(&node.code).cloned().unwrap_or_default();
            let ccy = scope.currency.get(&node.code).cloned().unwrap_or_default();
            if !ccy.is_empty() && ccy != fx.group_currency {
                let rates = match (
                    fx_rates.get(&(ccy.clone(), "closing".to_string())),
                    fx_rates.get(&(ccy.clone(), "average".to_string())),
                    fx_rates.get(&(ccy.clone(), "historical".to_string())),
                ) {
                    (Some(cl), Some(av), Some(hi)) => FxRates { closing: *cl, average: *av, historical: *hi },
                    _ => {
                        return Err(api_err(&format!(
                            "主体 {} 功能币 {} 缺 closing/average/historical 汇率(cg_fx_rate)",
                            node.code, ccy
                        )));
                    }
                };
                let src: Vec<(String, Decimal)> = bal.iter().map(|(a, v)| (a.clone(), *v)).collect();
                let translated = translate_entity(
                    &src,
                    |a: &str| acc_type.get(a).and_then(|t| AccountType::parse(t)).unwrap_or(AccountType::Asset),
                    rates,
                    &fx.cta_account,
                );
                bal = translated.into_iter().collect();
            }
            node_consolidated.insert(node.code.clone(), bal.clone());
            write_consol_data(scheme, period, &node.code, &bal, &bal, &[], &[]).await?;
            continue;
        }

        // 合并节点:聚合直接下级。
        let kids = children.get(&node.code).cloned().unwrap_or_default();
        let contributions: Vec<Contribution> = kids
            .iter()
            .filter_map(|k| {
                let kn = by_code.get(k)?;
                let balances = node_consolidated
                    .get(k)
                    .map(|m| m.iter().map(|(a, v)| (a.clone(), *v)).collect())
                    .unwrap_or_default();
                Some(Contribution {
                    entity: k.clone(),
                    method: kn.method,
                    ownership: kn.ownership,
                    balances,
                })
            })
            .collect();
        let individual = aggregate(&contributions);

        // —— 本级抵销 ——
        let mut elims: Vec<ElimEntry> = Vec::new();
        // —— 本级调整(权益法权益确认、商誉减值等;进工作底稿"调整"栏) ——
        let mut adjusts: Vec<ElimEntry> = Vec::new();

        // ① 资本抵销 + 少数股东损益:对每个全额/比例合并的下级子公司。
        for k in &kids {
            let kn = match by_code.get(k) {
                Some(x) => x,
                None => continue,
            };
            if !matches!(kn.method, ConsolMethod::Full | ConsolMethod::Proportional) {
                continue;
            }
            let child_bal = node_consolidated.get(k).cloned().unwrap_or_default();
            let inv = scope.investment.get(k).copied().unwrap_or(Decimal::ZERO);
            // —— 资本抵销:仅当有投资额(母对子长投)时 ——
            if inv != Decimal::ZERO {
                // 子公司权益科目(用于资本抵销)。★排除 CTA 折算差额:CTA 是折算后
                // 产生的权益(其他综合收益),属集团/少数股东,不参与"投资 vs 取得时权益"的抵销,
                // 否则会把折算储备一并消掉。
                let sub_equity: Vec<(String, Decimal)> = child_bal
                    .iter()
                    .filter(|(a, _)| {
                        a.as_str() != fx.cta_account
                            && matches!(acc_type.get(*a).map(String::as_str), Some("equity") | Some("权益"))
                    })
                    .map(|(a, v)| (a.clone(), *v))
                    .collect();
                // L1:同一控制下企业合并(under_common_control)→ 权益结合法(差额入资本公积,不确认商誉);
                // 否则常规资本抵销(差额→商誉)。
                if scope.common_control.contains(k) {
                    let rc = rules.get("capital_common_control").map(|r| r.2.clone())
                        .or_else(|| rules.get("capital").map(|r| r.2.clone()))
                        .unwrap_or_else(|| "R_CC".into());
                    elims.push(common_control_elimination(&sub_equity, inv, kn.ownership, &cfg, &rc));
                } else {
                    let rc = rules.get("capital").map(|r| r.2.clone()).unwrap_or_else(|| "R_CAPITAL".into());
                    elims.push(capital_elimination(&sub_equity, inv, kn.ownership, &cfg, &rc));
                }
            }
            // —— 少数股东损益:凡持股 <100% 的全额合并子公司都要分摊(与是否资本抵销解耦) ——
            // 少数股东损益 = (1−p) × 子净利润;净利润(自然) = −Σ(损益科目 借方正)。
            let pl_sum: Decimal = child_bal
                .iter()
                .filter(|(a, _)| {
                    matches!(
                        acc_type.get(*a).map(String::as_str),
                        Some("income") | Some("expense") | Some("收入") | Some("费用")
                    )
                })
                .map(|(_, v)| *v)
                .sum();
            let net_profit = -pl_sum;
            let rc2 = rules.get("nci").map(|r| r.2.clone()).unwrap_or_else(|| "R_NCI".into());
            if let Some(e) = minority_pl(net_profit, kn.ownership, &cfg, &rc2) {
                elims.push(e);
            }
        }

        // ② 内部往来抵销:两端都在本节点子树内、且非在更低层已抵销(即本节点是最低公共祖先)。
        let members: &HashSet<String> = &subtree[&node.code];
        for (ic_type, m) in &ic {
            if !members.contains(&m.entity_a) || !members.contains(&m.entity_b) {
                continue;
            }
            // 最低公共祖先判定:两端不在同一个"直接下级子树"内(否则应在更低层抵销)。
            if same_child_subtree(&kids, &subtree, &m.entity_a, &m.entity_b) {
                continue;
            }
            match ic_type.as_str() {
                "debt" => {
                    if let Some((dr, cr, rc)) = rules.get("debt") {
                        elims.extend(debt_elimination(std::slice::from_ref(m), dr, cr, rc));
                    }
                }
                "sales" => {
                    if let Some((dr, cr, rc)) = rules.get("sales") {
                        elims.extend(sales_elimination(std::slice::from_ref(m), dr, cr, rc));
                    }
                }
                // L4 内部股利抵销:母(entity_a)自子(entity_b)确认的股利收益冲回 + 还原留存。
                "dividend" => {
                    let (inc_acc, re_acc, rc) = rules.get("dividend")
                        .map(|r| (r.0.clone(), r.1.clone(), r.2.clone()))
                        .unwrap_or_else(|| ("6111".into(), "4104".into(), "R_DIV".into()));
                    elims.extend(dividend_elimination(std::slice::from_ref(m), &inc_acc, &re_acc, &rc));
                }
                _ => {}
            }
        }

        // ③ 存货未实现内部利润抵销 + 期初结转(C6):卖方/买方均在本节点子树、且为最低公共祖先。
        if let Some((cost_acc, inv_acc, inv_rc)) = rules.get("inventory").cloned() {
            let (open_re_acc, _cr, open_rc) = rules
                .get("inventory_opening")
                .cloned()
                .unwrap_or_else(|| ("4104".into(), String::new(), "R_INV_OPEN".into()));
            for p in &interim {
                if !members.contains(&p.seller) || !members.contains(&p.buyer) {
                    continue;
                }
                if same_child_subtree(&kids, &subtree, &p.seller, &p.buyer) {
                    continue;
                }
                elims.extend(inventory_profit_elimination(
                    p, &cost_acc, &inv_acc, &open_re_acc, &inv_rc, &open_rc,
                ));
            }
        }

        // ③' L3 固定资产内部交易未实现利润 + 逐期折旧转回:卖买双方在本节点子树且为 LCA。
        {
            let gain = rules.get("fixed_asset_profit").map(|r| r.0.clone()).filter(|s| !s.is_empty()).unwrap_or_else(|| "6301".into());
            let asset = rules.get("fixed_asset_profit").map(|r| r.1.clone()).filter(|s| !s.is_empty()).unwrap_or_else(|| "1601".into());
            let fa_rc = rules.get("fixed_asset_profit").map(|r| r.2.clone()).unwrap_or_else(|| "R_FA".into());
            let (accum_dep, expense, dep_rc) = rules.get("fixed_asset_depreciation")
                .map(|r| (r.0.clone(), r.1.clone(), r.2.clone()))
                .unwrap_or_else(|| ("1602".into(), "6602".into(), "R_FA_DEP".into()));
            for p in &fa_profit {
                if !members.contains(&p.seller) || !members.contains(&p.buyer) {
                    continue;
                }
                if same_child_subtree(&kids, &subtree, &p.seller, &p.buyer) {
                    continue;
                }
                elims.extend(fixed_asset_profit_elimination(
                    p, &gain, &asset, &accum_dep, &expense, &fa_rc, &dep_rc,
                ));
            }
        }

        // ④ 权益法权益确认(C6):对联营/合营下级,按份额确认投资收益 + 调增长投(不逐行并入)。
        {
            let (inv_acc, inc_acc, em_rc) = rules
                .get("equity_method")
                .cloned()
                .unwrap_or_else(|| (cfg.investment_account.clone(), "6111".to_string(), "R_EQUITY".to_string()));
            for k in &kids {
                let kn = match by_code.get(k) {
                    Some(x) => x,
                    None => continue,
                };
                if kn.method != ConsolMethod::Equity {
                    continue;
                }
                let child_bal = node_consolidated.get(k).cloned().unwrap_or_default();
                let pl_sum: Decimal = child_bal
                    .iter()
                    .filter(|(a, _)| {
                        matches!(
                            acc_type.get(*a).map(String::as_str),
                            Some("income") | Some("expense") | Some("收入") | Some("费用")
                        )
                    })
                    .map(|(_, v)| *v)
                    .sum();
                let net_profit = -pl_sum;
                if let Some(e) = equity_pickup(net_profit, kn.ownership, &inv_acc, &inc_acc, &em_rc) {
                    adjusts.push(e);
                }
            }
        }

        // ⑤ 商誉减值(C6):本节点资本抵销形成的商誉计提减值(冲商誉、确认减值损失)。
        if let Some(amt) = goodwill_impair.get(&node.code).copied() {
            let (imp_acc, _cr, gw_rc) = rules
                .get("goodwill_impair")
                .cloned()
                .unwrap_or_else(|| ("6701".to_string(), String::new(), "R_GOODWILL".to_string()));
            if let Some(e) = goodwill_impairment(amt, &cfg.goodwill_account, &imp_acc, &gw_rc) {
                adjusts.push(e);
            }
        }

        // ⑥ L2 分步取得/处置(本节点):原持股公允重估 / 处置权益交易或损益。进"调整"栏。
        for t in step_txns.iter().filter(|t| t.node == node.code) {
            let gain_acc = rules.get("step_acquisition").map(|r| r.0.clone())
                .filter(|s| !s.is_empty()).unwrap_or_else(|| "6111".to_string());
            match t.txn_type.as_str() {
                "step_acq" => {
                    let rc = rules.get("step_acquisition").map(|r| r.2.clone()).unwrap_or_else(|| "R_STEP".into());
                    if let Some(e) = step_acquisition(t.prev_carrying, t.prev_fair_value, &cfg.investment_account, &gain_acc, &rc) {
                        adjusts.push(e);
                    }
                }
                "disposal" => {
                    let rc = rules.get("disposal").map(|r| r.2.clone()).unwrap_or_else(|| "R_DISP".into());
                    if let Some(e) = disposal(
                        t.loses_control, t.proceeds, t.disposed_share, t.retained_fair_value,
                        t.net_assets_share, &cfg.nci_account, &cfg.capital_reserve_account, &gain_acc, &rc,
                    ) {
                        adjusts.push(e);
                    }
                }
                _ => {}
            }
        }

        // —— 工作底稿 + 落库 ——
        let cells = worksheet(&node.code, &individual, &adjusts, &elims);
        let consolidated: BTreeMap<String, Decimal> =
            cells.iter().map(|c| (c.account.clone(), c.consolidated)).collect();
        node_consolidated.insert(node.code.clone(), consolidated.clone());
        write_consol_data(scheme, period, &node.code, &individual, &consolidated, &adjusts, &elims).await?;
        // 调整 + 抵销凭证落合并分类账(调整在前)。
        for e in adjusts.iter().chain(elims.iter()) {
            doc_seq += 1;
            write_elim_journal(scheme, period, &node.code, doc_seq, e).await?;
        }
        total_entries += adjusts.len() + elims.len();
    }

    Ok(json!({
        "ok": true,
        "scheme": scheme,
        "period": period,
        "nodes": scope.nodes.len(),
        "entries": total_entries,
        "message": "合并完成",
    }))
}

/// 计算每节点子树成员集(含自身)。
pub(crate) fn compute_subtrees(
    nodes: &[ScopeNode],
    children: &HashMap<String, Vec<String>>,
) -> HashMap<String, HashSet<String>> {
    fn collect(code: &str, children: &HashMap<String, Vec<String>>, out: &mut HashSet<String>) {
        out.insert(code.to_string());
        if let Some(ks) = children.get(code) {
            for k in ks {
                collect(k, children, out);
            }
        }
    }
    nodes
        .iter()
        .map(|n| {
            let mut s = HashSet::new();
            collect(&n.code, children, &mut s);
            (n.code.clone(), s)
        })
        .collect()
}

/// 装载范围 + 直接算出每节点子树成员集(供 CF/EQC 聚合复用)。返回 (节点, 子树映射)。
pub(crate) async fn load_scope_subtrees(
    scheme: &str,
    period: &str,
) -> Result<(Vec<ScopeNode>, HashMap<String, HashSet<String>>)> {
    let scope = load_scope(scheme, period).await?;
    let mut children: HashMap<String, Vec<String>> = HashMap::new();
    for n in &scope.nodes {
        if let Some(p) = &n.parent {
            children.entry(p.clone()).or_default().push(n.code.clone());
        }
    }
    let subtree = compute_subtrees(&scope.nodes, &children);
    Ok((scope.nodes, subtree))
}

/// 两个主体是否落在同一个直接下级的子树内(→ 应在更低层抵销,本层跳过)。
fn same_child_subtree(
    kids: &[String],
    subtree: &HashMap<String, HashSet<String>>,
    a: &str,
    b: &str,
) -> bool {
    kids.iter().any(|k| {
        subtree
            .get(k)
            .map(|s| s.contains(a) && s.contains(b))
            .unwrap_or(false)
    })
}

// ============================================================================
// 落库
// ============================================================================

/// 落合并结果(四栏)到 cg_consol_data。
async fn write_consol_data(
    scheme: &str,
    period: &str,
    node: &str,
    individual: &BTreeMap<String, Decimal>,
    consolidated: &BTreeMap<String, Decimal>,
    adjust: &[ElimEntry],
    elims: &[ElimEntry],
) -> Result<()> {
    // 全科目并集(个别 + 合并 + 调整/抵销涉及)。
    let mut accs: BTreeMap<String, ()> = BTreeMap::new();
    for a in individual.keys().chain(consolidated.keys()) {
        accs.insert(a.clone(), ());
    }
    for e in adjust.iter().chain(elims.iter()) {
        for l in &e.lines {
            accs.insert(l.account.clone(), ());
        }
    }
    for acc in accs.keys() {
        let ind = individual.get(acc).copied().unwrap_or(Decimal::ZERO);
        let adj: Decimal = adjust.iter().map(|e| e.net_for(acc)).sum();
        let elm: Decimal = elims.iter().map(|e| e.net_for(acc)).sum();
        let con = consolidated.get(acc).copied().unwrap_or(ind + adj + elm);
        execute(
            "INSERT INTO cg_consol_data (id, code, scheme_code, period_code, node_code, account_code, \
                individual, adjust, elim, consolidated, sort_no, status, create_time, update_time) \
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,0,1,CURRENT_TIMESTAMP,CURRENT_TIMESTAMP) \
             ON CONFLICT (scheme_code, period_code, node_code, account_code) \
             DO UPDATE SET individual=EXCLUDED.individual, adjust=EXCLUDED.adjust, \
                elim=EXCLUDED.elim, consolidated=EXCLUDED.consolidated, update_time=CURRENT_TIMESTAMP",
            vec![
                DataValue::Int(cmx_utils::next_pk_id()),
                DataValue::String(format!("{scheme}|{period}|{node}|{acc}")),
                DataValue::String(scheme.to_string()),
                DataValue::String(period.to_string()),
                DataValue::String(node.to_string()),
                DataValue::String(acc.clone()),
                DataValue::Decimal(ind),
                DataValue::Decimal(adj),
                DataValue::Decimal(elm),
                DataValue::Decimal(con),
            ],
        )
        .await?;
    }
    Ok(())
}

/// 落抵销凭证到合并分类账 cg_elim_journal。
async fn write_elim_journal(
    scheme: &str,
    period: &str,
    node: &str,
    doc_seq: i64,
    entry: &ElimEntry,
) -> Result<()> {
    let doc_no = format!("CJ-{doc_seq:06}");
    for (i, l) in entry.lines.iter().enumerate() {
        execute(
            "INSERT INTO cg_elim_journal (id, code, scheme_code, period_code, node_code, doc_no, \
                line_no, elim_type, account_code, dr, cr, partner, is_opening, is_manual, source_rule, \
                sort_no, status, create_time, update_time) \
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,0,$14,0,1,CURRENT_TIMESTAMP,CURRENT_TIMESTAMP)",
            vec![
                DataValue::Int(cmx_utils::next_pk_id()),
                DataValue::String(format!("{doc_no}-{}", i + 1)),
                DataValue::String(scheme.to_string()),
                DataValue::String(period.to_string()),
                DataValue::String(node.to_string()),
                DataValue::String(doc_no.clone()),
                DataValue::Int((i + 1) as i64),
                DataValue::String(entry.elim_type.clone()),
                DataValue::String(l.account.clone()),
                DataValue::Decimal(l.dr),
                DataValue::Decimal(l.cr),
                l.partner
                    .clone()
                    .map(DataValue::String)
                    .unwrap_or(DataValue::NullTyped(SqlTypeMarker::Text)),
                DataValue::Int(entry.is_opening as i64),
                DataValue::String(entry.source_rule.clone()),
            ],
        )
        .await?;
    }
    Ok(())
}

// ============================================================================
// 查询
// ============================================================================

/// 查合并工作底稿(某方案某期某节点的四栏)。
pub async fn get_worksheet(scheme: &str, period: &str, node: &str) -> Result<Value> {
    let rows = query_rows(
        "SELECT account_code, individual, adjust, elim, consolidated FROM cg_consol_data \
         WHERE scheme_code=$1 AND period_code=$2 AND node_code=$3 ORDER BY account_code",
        vec![
            DataValue::String(scheme.to_string()),
            DataValue::String(period.to_string()),
            DataValue::String(node.to_string()),
        ],
        "consol_worksheet",
    )
    .await?;
    Ok(json!({ "scheme": scheme, "period": period, "node": node, "count": rows.len(), "rows": rows }))
}

/// 查合并分类账(抵销凭证)。
pub async fn get_elim_journal(scheme: &str, period: &str, node: &str) -> Result<Value> {
    let rows = query_rows(
        "SELECT doc_no, line_no, elim_type, account_code, dr, cr, partner, is_opening, source_rule \
         FROM cg_elim_journal WHERE scheme_code=$1 AND period_code=$2 AND node_code=$3 \
         ORDER BY doc_no, line_no",
        vec![
            DataValue::String(scheme.to_string()),
            DataValue::String(period.to_string()),
            DataValue::String(node.to_string()),
        ],
        "consol_journal",
    )
    .await?;
    Ok(json!({ "scheme": scheme, "period": period, "node": node, "count": rows.len(), "entries": rows }))
}

/// 合并取数(供报表 CG/IND/ELIM 函数):某科目在某合并节点的四栏值。
pub async fn consol_value(scheme: &str, period: &str, node: &str, account: &str) -> Result<Value> {
    let rows = query_rows(
        "SELECT individual, adjust, elim, consolidated FROM cg_consol_data \
         WHERE scheme_code=$1 AND period_code=$2 AND node_code=$3 AND account_code=$4",
        vec![
            DataValue::String(scheme.to_string()),
            DataValue::String(period.to_string()),
            DataValue::String(node.to_string()),
            DataValue::String(account.to_string()),
        ],
        "consol_value",
    )
    .await?;
    let r = rows.first().cloned().unwrap_or_else(|| json!({}));
    Ok(json!({
        "individual": dv_dec(&r, "individual").to_string(),
        "adjust": dv_dec(&r, "adjust").to_string(),
        "elim": dv_dec(&r, "elim").to_string(),
        "consolidated": dv_dec(&r, "consolidated").to_string(),
    }))
}

/// ConsolMethod → 存库字符串(与 parse 对齐)。
fn method_str(m: ConsolMethod) -> &'static str {
    match m {
        ConsolMethod::Full => "full",
        ConsolMethod::Equity => "equity",
        ConsolMethod::Proportional => "proportional",
        ConsolMethod::Cost => "cost",
    }
}

/// C7 范围变动:对比本期 vs 上期合并范围,落 cg_scope_change(CAS33/IFRS10 附注)。
/// prev_period 为空时自动取"同方案早于本期的最近一期"。返回 { ok, prev, changes:[...] }。
pub async fn run_scope_change(scheme: &str, period: &str, prev_period: Option<&str>) -> Result<Value> {
    // 解析上期:显式给定优先;否则取同方案 < period 的最近一期。
    let prev = match prev_period.filter(|s| !s.is_empty()) {
        Some(p) => p.to_string(),
        None => {
            let rows = query_rows(
                "SELECT DISTINCT period_code FROM cg_scope \
                 WHERE scheme_code=$1 AND period_code < $2 AND COALESCE(status,1)=1 \
                 ORDER BY period_code DESC LIMIT 1",
                vec![DataValue::String(scheme.to_string()), DataValue::String(period.to_string())],
                "consol_prev_period",
            )
            .await?;
            match rows.first().and_then(|r| sv(r, "period_code")) {
                Some(p) => p,
                None => return Err(api_err("未找到上期合并范围(无更早期间),无法对比范围变动")),
            }
        }
    };

    let curr_nodes = load_scope(scheme, period).await?.nodes;
    let prev_nodes = load_scope(scheme, &prev).await?.nodes;
    if curr_nodes.is_empty() {
        return Err(api_err("本期未配置合并范围"));
    }
    let changes: Vec<ScopeChange> = diff_scope(&prev_nodes, &curr_nodes, false);

    // 清本期变动 → 重写(幂等)。
    execute(
        "DELETE FROM cg_scope_change WHERE scheme_code=$1 AND period_code=$2",
        vec![DataValue::String(scheme.to_string()), DataValue::String(period.to_string())],
    )
    .await?;
    for (i, c) in changes.iter().enumerate() {
        execute(
            "INSERT INTO cg_scope_change (id, code, name, scheme_code, period_code, prev_period, \
                org_code, org_name, change_type, curr_method, prev_method, curr_ownership, prev_ownership, \
                sort_no, status, create_time, update_time) \
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,1,CURRENT_TIMESTAMP,CURRENT_TIMESTAMP)",
            vec![
                DataValue::Int(cmx_utils::next_pk_id()),
                DataValue::String(format!("{scheme}|{period}|{}", c.org_code)),
                DataValue::String(c.org_name.clone()),
                DataValue::String(scheme.to_string()),
                DataValue::String(period.to_string()),
                DataValue::String(prev.clone()),
                DataValue::String(c.org_code.clone()),
                DataValue::String(c.org_name.clone()),
                DataValue::String(c.change_type.clone()),
                c.curr_method.map(|m| DataValue::String(method_str(m).into()))
                    .unwrap_or(DataValue::NullTyped(SqlTypeMarker::Text)),
                c.prev_method.map(|m| DataValue::String(method_str(m).into()))
                    .unwrap_or(DataValue::NullTyped(SqlTypeMarker::Text)),
                DataValue::Decimal(c.curr_ownership),
                DataValue::Decimal(c.prev_ownership),
                DataValue::Int(i as i64),
            ],
        )
        .await?;
    }

    // 统计各类型计数。
    let mut counts: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    for c in &changes {
        *counts.entry(c.change_type.clone()).or_insert(0) += 1;
    }
    Ok(json!({
        "ok": true, "scheme": scheme, "period": period, "prev": prev,
        "count": changes.len(),
        "counts": counts,
        "message": format!("范围变动对比完成(本期 {period} vs 上期 {prev}):{} 项变动", changes.len()),
    }))
}

/// 查合并范围变动清单(某方案某期)。
pub async fn get_scope_change(scheme: &str, period: &str) -> Result<Value> {
    let rows = query_rows(
        "SELECT org_code, org_name, change_type, curr_method, prev_method, curr_ownership, prev_ownership, prev_period \
         FROM cg_scope_change WHERE scheme_code=$1 AND period_code=$2 ORDER BY sort_no, org_code",
        vec![DataValue::String(scheme.to_string()), DataValue::String(period.to_string())],
        "consol_scope_change",
    )
    .await?;
    Ok(json!({ "scheme": scheme, "period": period, "count": rows.len(), "rows": rows }))
}

/// 列合并方案。
pub async fn list_schemes() -> Result<Value> {
    let rows = query_rows(
        "SELECT scheme_code, name, standard, group_currency FROM cg_consol_scheme \
         WHERE COALESCE(status,1)=1 ORDER BY scheme_code",
        vec![],
        "consol_schemes",
    )
    .await?;
    Ok(json!({ "count": rows.len(), "schemes": rows }))
}

/// 某方案的会计期间(cg_scope 去重),供工作台期间下拉。
pub async fn list_periods(scheme: &str) -> Result<Value> {
    let rows = query_rows(
        "SELECT DISTINCT period_code FROM cg_scope \
         WHERE scheme_code=$1 AND COALESCE(status,1)=1 ORDER BY period_code DESC",
        vec![DataValue::String(scheme.to_string())],
        "consol_periods",
    )
    .await?;
    Ok(json!({ "scheme": scheme, "periods": rows }))
}

/// 某方案某期的合并范围节点(供工作台节点树)。
pub async fn list_nodes(scheme: &str, period: &str) -> Result<Value> {
    let rows = query_rows(
        "SELECT org_code, org_name, parent_code, consol_method, ownership_pct, is_leaf, \
                level_no, currency, investment_amount \
         FROM cg_scope WHERE scheme_code=$1 AND period_code=$2 AND COALESCE(status,1)=1 \
         ORDER BY level_no, org_code",
        vec![DataValue::String(scheme.to_string()), DataValue::String(period.to_string())],
        "consol_nodes",
    )
    .await?;
    Ok(json!({ "scheme": scheme, "period": period, "count": rows.len(), "nodes": rows }))
}

/// 某方案的集团科目表(code→name/type),供工作台工作底稿显示科目名。
pub async fn list_accounts(scheme: &str) -> Result<Value> {
    let rows = query_rows(
        "SELECT account_code, name, account_type FROM cg_group_account \
         WHERE scheme_code=$1 AND COALESCE(status,1)=1 ORDER BY account_code",
        vec![DataValue::String(scheme.to_string())],
        "consol_accounts",
    )
    .await?;
    Ok(json!({ "scheme": scheme, "count": rows.len(), "accounts": rows }))
}

pub(crate) fn body_s(b: &Value, k: &str) -> String {
    s_body(b, k)
}
