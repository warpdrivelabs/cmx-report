//! cashflow —— C7-Next N2:现金流量项目流水 / 权益变动流水的逐级聚合落库(供 CCF/CSE 出表)。
//!
//! 原始流水由主体上报(node_code=''),`run_cashflow`/`run_equity_change` 按合并范围子树聚合 +
//! 内部现金流抵销(借方正,纯算法在 cmx-consol-model),把每合并节点的聚合结果写回同表
//! (node_code=合并节点、entity_code='')。之后 CCF/CSE 报表经 `CF(期间,@节点,'项目码')`/
//! `EQC(期间,@节点,'列码')` 取数函数读聚合行出表——完整复用 RPT 计算态,不新增计算路径。
//!
//! 幂等:每次 run 先删该 scheme+period 的聚合行(node_code<>''),再逐节点重算重写。

use serde_json::{Value, json};

use cmx_core::model::cell::DataValue;

use cmx_consol_model::{
    CashFlowRow, CfActivity, EquityChangeRow, aggregate_cash_flow, aggregate_equity_change,
    derive_cash_flow_worksheet,
};

use crate::{Result, dv_dec, execute, iv, load_scope_subtrees, query_rows, sv};

fn pk() -> DataValue {
    DataValue::Int(cmx_utils::next_pk_id())
}

/// 装载某方案某期的**主体原始现金流量流水**(node_code=''),供聚合。
async fn load_cash_flow_rows(scheme: &str, period: &str) -> Result<Vec<CashFlowRow>> {
    let rows = query_rows(
        "SELECT entity_code, activity, item_code, amount, is_intercompany FROM cg_cash_flow_item \
         WHERE scheme_code=$1 AND period_code=$2 AND COALESCE(node_code,'')='' AND COALESCE(status,1)=1",
        vec![DataValue::String(scheme.to_string()), DataValue::String(period.to_string())],
        "consol_cashflow_raw",
    )
    .await?;
    Ok(rows
        .iter()
        .map(|r| CashFlowRow {
            entity: sv(r, "entity_code").unwrap_or_default(),
            activity: sv(r, "activity").unwrap_or_default(),
            item_code: sv(r, "item_code").unwrap_or_default(),
            amount: dv_dec(r, "amount"),
            is_intercompany: iv(r, "is_intercompany").unwrap_or(0) == 1,
        })
        .collect())
}

/// 装载某方案某期的**主体原始权益变动流水**(node_code=''),供聚合。
async fn load_equity_rows(scheme: &str, period: &str) -> Result<Vec<EquityChangeRow>> {
    let rows = query_rows(
        "SELECT entity_code, column_code, equity_item, change_type, amount FROM cg_equity_change \
         WHERE scheme_code=$1 AND period_code=$2 AND COALESCE(node_code,'')='' AND COALESCE(status,1)=1",
        vec![DataValue::String(scheme.to_string()), DataValue::String(period.to_string())],
        "consol_equity_raw",
    )
    .await?;
    Ok(rows
        .iter()
        .map(|r| EquityChangeRow {
            entity: sv(r, "entity_code").unwrap_or_default(),
            column_code: sv(r, "column_code").unwrap_or_default(),
            equity_item: sv(r, "equity_item").unwrap_or_default(),
            change_type: sv(r, "change_type").unwrap_or_default(),
            amount: dv_dec(r, "amount"),
        })
        .collect())
}

/// N2 现金流量逐级聚合:按合并范围子树聚合 + 内部现金流抵销,落每节点聚合行(node_code=N)。幂等。
pub async fn run_cashflow(scheme: &str, period: &str) -> Result<Value> {
    let (nodes, subtree) = load_scope_subtrees(scheme, period).await?;
    if nodes.is_empty() {
        return Err(crate::api_err("该方案该期间未配置合并范围"));
    }
    let rows = load_cash_flow_rows(scheme, period).await?;

    // 清聚合行(保留主体原始行 node_code='')。
    execute(
        "DELETE FROM cg_cash_flow_item WHERE scheme_code=$1 AND period_code=$2 AND COALESCE(node_code,'')<>''",
        vec![DataValue::String(scheme.to_string()), DataValue::String(period.to_string())],
    )
    .await?;

    let mut written = 0usize;
    for n in &nodes {
        let members = match subtree.get(&n.code) {
            Some(m) => m,
            None => continue,
        };
        let agg = aggregate_cash_flow(&rows, members);
        for (item_code, amount) in &agg {
            write_cash_flow_agg(scheme, period, &n.code, item_code, *amount).await?;
            written += 1;
        }
    }
    Ok(json!({
        "ok": true, "scheme": scheme, "period": period,
        "nodes": nodes.len(), "rows": written,
        "message": format!("现金流量聚合完成({} 节点,{} 聚合行)", nodes.len(), written),
    }))
}

/// N2 权益变动逐级聚合:按列聚合,落每节点聚合行(node_code=N)。幂等。
pub async fn run_equity_change(scheme: &str, period: &str) -> Result<Value> {
    let (nodes, subtree) = load_scope_subtrees(scheme, period).await?;
    if nodes.is_empty() {
        return Err(crate::api_err("该方案该期间未配置合并范围"));
    }
    let rows = load_equity_rows(scheme, period).await?;

    execute(
        "DELETE FROM cg_equity_change WHERE scheme_code=$1 AND period_code=$2 AND COALESCE(node_code,'')<>''",
        vec![DataValue::String(scheme.to_string()), DataValue::String(period.to_string())],
    )
    .await?;

    let mut written = 0usize;
    for n in &nodes {
        let members = match subtree.get(&n.code) {
            Some(m) => m,
            None => continue,
        };
        let agg = aggregate_equity_change(&rows, members);
        for (column_code, amount) in &agg {
            write_equity_agg(scheme, period, &n.code, column_code, *amount).await?;
            written += 1;
        }
    }
    Ok(json!({
        "ok": true, "scheme": scheme, "period": period,
        "nodes": nodes.len(), "rows": written,
        "message": format!("权益变动聚合完成({} 节点,{} 聚合行)", nodes.len(), written),
    }))
}

/// 落现金流量聚合行(node_code=N,entity_code='')。唯一键 (scheme,period,node,entity,item)。
async fn write_cash_flow_agg(scheme: &str, period: &str, node: &str, item: &str, amount: rust_decimal::Decimal) -> Result<()> {
    execute(
        "INSERT INTO cg_cash_flow_item (id, code, scheme_code, period_code, node_code, entity_code, \
            activity, item_code, amount, is_intercompany, sort_no, status, create_time, update_time) \
         VALUES ($1,$2,$3,$4,$5,'','', $6,$7,0,0,1,CURRENT_TIMESTAMP,CURRENT_TIMESTAMP) \
         ON CONFLICT (scheme_code, period_code, node_code, entity_code, item_code) \
         DO UPDATE SET amount=EXCLUDED.amount, update_time=CURRENT_TIMESTAMP",
        vec![
            pk(),
            DataValue::String(format!("{scheme}|{period}|{node}||{item}")),
            DataValue::String(scheme.to_string()),
            DataValue::String(period.to_string()),
            DataValue::String(node.to_string()),
            DataValue::String(item.to_string()),
            DataValue::Decimal(amount),
        ],
    )
    .await
}

/// 落权益变动聚合行(node_code=N,entity_code='')。唯一键 (scheme,period,node,entity,equity_item,change_type)。
/// 聚合行以 column_code 归集,equity_item 存 '__agg__'、change_type 存列码保证唯一。
async fn write_equity_agg(scheme: &str, period: &str, node: &str, column: &str, amount: rust_decimal::Decimal) -> Result<()> {
    execute(
        "INSERT INTO cg_equity_change (id, code, scheme_code, period_code, node_code, entity_code, \
            equity_item, change_type, column_code, amount, sort_no, status, create_time, update_time) \
         VALUES ($1,$2,$3,$4,$5,'','__agg__', $6,$7,$8,0,1,CURRENT_TIMESTAMP,CURRENT_TIMESTAMP) \
         ON CONFLICT (scheme_code, period_code, node_code, entity_code, equity_item, change_type) \
         DO UPDATE SET column_code=EXCLUDED.column_code, amount=EXCLUDED.amount, update_time=CURRENT_TIMESTAMP",
        vec![
            pk(),
            DataValue::String(format!("{scheme}|{period}|{node}|agg|{column}")),
            DataValue::String(scheme.to_string()),
            DataValue::String(period.to_string()),
            DataValue::String(node.to_string()),
            // change_type = 列码(保证唯一键区分不同列的聚合行)。
            DataValue::String(column.to_string()),
            DataValue::String(column.to_string()),
            DataValue::Decimal(amount),
        ],
    )
    .await
}

/// 查某方案某期某节点的现金流量(聚合行 node_code=N)。供工作台/审计。
pub async fn get_cash_flow(scheme: &str, period: &str, node: &str) -> Result<Value> {
    let rows = query_rows(
        "SELECT item_code, item_name, activity, amount FROM cg_cash_flow_item \
         WHERE scheme_code=$1 AND period_code=$2 AND node_code=$3 AND COALESCE(status,1)=1 \
         ORDER BY activity, item_code",
        vec![
            DataValue::String(scheme.to_string()),
            DataValue::String(period.to_string()),
            DataValue::String(node.to_string()),
        ],
        "consol_cashflow_get",
    )
    .await?;
    Ok(json!({ "scheme": scheme, "period": period, "node": node, "count": rows.len(), "rows": rows }))
}

/// 查某方案某期某节点的权益变动(聚合行 node_code=N)。
pub async fn get_equity_change(scheme: &str, period: &str, node: &str) -> Result<Value> {
    let rows = query_rows(
        "SELECT column_code, amount FROM cg_equity_change \
         WHERE scheme_code=$1 AND period_code=$2 AND node_code=$3 AND COALESCE(status,1)=1 \
         ORDER BY column_code",
        vec![
            DataValue::String(scheme.to_string()),
            DataValue::String(period.to_string()),
            DataValue::String(node.to_string()),
        ],
        "consol_equity_get",
    )
    .await?;
    Ok(json!({ "scheme": scheme, "period": period, "node": node, "count": rows.len(), "rows": rows }))
}

// ============================================================================
// L5 现金流量表·工作底稿法(间接法):从两期合并数差额推导,不吃录入流水
// ============================================================================

/// 账户分类:按 account_type + code 前缀归入现金/经营/投资/筹资。
/// 现金 = 1001/1002/货币资金类;投资 = 长期资产(15xx/16xx/17xx/18xx 长投/固资/无形/商誉);
/// 筹资 = 权益(4xxx)+ 长期借款(2501/2701);其余资产负债 + 损益 = 经营。
fn classify_account(code: &str, acc_type: &str) -> CfActivity {
    if code.starts_with("1001") || code.starts_with("1002") || code.starts_with("1012") {
        return CfActivity::Cash;
    }
    let t = acc_type.to_ascii_lowercase();
    if matches!(t.as_str(), "equity" | "nci" | "权益" | "少数股东权益") {
        return CfActivity::Financing;
    }
    if code.starts_with("2501") || code.starts_with("2701") {
        return CfActivity::Financing;
    }
    if code.starts_with("15") || code.starts_with("16") || code.starts_with("17") || code.starts_with("18") {
        return CfActivity::Investing;
    }
    CfActivity::Operating
}

/// 读某方案某期某节点的合并数(account → 借方正 consolidated)。
async fn load_node_consolidated(scheme: &str, period: &str, node: &str) -> Result<std::collections::BTreeMap<String, rust_decimal::Decimal>> {
    let rows = query_rows(
        "SELECT account_code, consolidated FROM cg_consol_data \
         WHERE scheme_code=$1 AND period_code=$2 AND node_code=$3",
        vec![
            DataValue::String(scheme.to_string()),
            DataValue::String(period.to_string()),
            DataValue::String(node.to_string()),
        ],
        "consol_node_data",
    )
    .await?;
    let mut m = std::collections::BTreeMap::new();
    for r in &rows {
        if let Some(a) = sv(r, "account_code") {
            m.insert(a, dv_dec(r, "consolidated"));
        }
    }
    Ok(m)
}

/// L5 工作底稿法现金流量:从本期 vs 上期合并数差额推导三活动净额,写回 cg_cash_flow_item
/// (item_code=CF_OP/CF_INV/CF_FIN/CF_NET,activity=worksheet 标法,与 N2 录入法并存可区分)。
/// prev_period 为空 → 自动取同方案早于本期的最近一期。幂等(先删本法聚合行再写)。
pub async fn run_cashflow_worksheet(scheme: &str, period: &str, prev_period: Option<&str>) -> Result<Value> {
    let (nodes, _subtree) = load_scope_subtrees(scheme, period).await?;
    if nodes.is_empty() {
        return Err(crate::api_err("该方案该期间未配置合并范围"));
    }
    let prev = match prev_period.filter(|s| !s.is_empty()) {
        Some(p) => p.to_string(),
        None => {
            let rows = query_rows(
                "SELECT DISTINCT period_code FROM cg_consol_data \
                 WHERE scheme_code=$1 AND period_code < $2 ORDER BY period_code DESC LIMIT 1",
                vec![DataValue::String(scheme.to_string()), DataValue::String(period.to_string())],
                "consol_cf_prev",
            )
            .await?;
            match rows.first().and_then(|r| sv(r, "period_code")) {
                Some(p) => p,
                None => return Err(crate::api_err("未找到上期合并数(无更早期间),工作底稿法需两期")),
            }
        }
    };

    let acc_type = crate::load_account_types_pub(scheme).await?;

    execute(
        "DELETE FROM cg_cash_flow_item WHERE scheme_code=$1 AND period_code=$2 \
         AND COALESCE(node_code,'')<>'' AND item_code IN ('CF_OP','CF_INV','CF_FIN','CF_NET')",
        vec![DataValue::String(scheme.to_string()), DataValue::String(period.to_string())],
    )
    .await?;

    let mut written = 0usize;
    for n in &nodes {
        let opening = load_node_consolidated(scheme, &prev, &n.code).await?;
        let closing = load_node_consolidated(scheme, period, &n.code).await?;
        if opening.is_empty() && closing.is_empty() {
            continue;
        }
        let lines = derive_cash_flow_worksheet(&opening, &closing, |a| {
            classify_account(a, acc_type.get(a).map(String::as_str).unwrap_or(""))
        });
        for l in &lines {
            let (item, name) = match l.activity {
                CfActivity::Operating => ("CF_OP", "经营活动现金流量净额(工作底稿法)"),
                CfActivity::Investing => ("CF_INV", "投资活动现金流量净额(工作底稿法)"),
                CfActivity::Financing => ("CF_FIN", "筹资活动现金流量净额(工作底稿法)"),
                CfActivity::Cash => ("CF_NET", "现金及现金等价物净增加额(工作底稿法)"),
            };
            write_cf_worksheet_row(scheme, period, &n.code, item, name, l.amount).await?;
            written += 1;
        }
    }
    Ok(json!({
        "ok": true, "scheme": scheme, "period": period, "prev": prev,
        "nodes": nodes.len(), "rows": written,
        "message": format!("工作底稿法现金流量完成(本期 {period} vs 上期 {prev}):{} 行", written),
    }))
}

async fn write_cf_worksheet_row(scheme: &str, period: &str, node: &str, item: &str, name: &str, amount: rust_decimal::Decimal) -> Result<()> {
    execute(
        "INSERT INTO cg_cash_flow_item (id, code, scheme_code, period_code, node_code, entity_code, \
            activity, item_code, item_name, amount, is_intercompany, sort_no, status, create_time, update_time) \
         VALUES ($1,$2,$3,$4,$5,'','worksheet',$6,$7,$8,0,0,1,CURRENT_TIMESTAMP,CURRENT_TIMESTAMP) \
         ON CONFLICT (scheme_code, period_code, node_code, entity_code, item_code) \
         DO UPDATE SET amount=EXCLUDED.amount, item_name=EXCLUDED.item_name, update_time=CURRENT_TIMESTAMP",
        vec![
            pk(),
            DataValue::String(format!("{scheme}|{period}|{node}||{item}")),
            DataValue::String(scheme.to_string()),
            DataValue::String(period.to_string()),
            DataValue::String(node.to_string()),
            DataValue::String(item.to_string()),
            DataValue::String(name.to_string()),
            DataValue::Decimal(amount),
        ],
    )
    .await
}
