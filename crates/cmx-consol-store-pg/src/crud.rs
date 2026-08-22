//! crud —— cg_* 主数据/输入的批量 UPSERT(供 API 录入方案/范围/个别数/规则/往来)。
//!
//! 每个 upsert 取 body.items[](或单对象),按各表唯一键 ON CONFLICT 幂等落库。
//! NOT NULL 无默认列(sort_no/status/create_time)显式给值(元数据表无 DEFAULT 子句)。

use rust_decimal::Decimal;
use serde_json::{Value, json};

use cmx_core::model::cell::{DataValue, SqlTypeMarker};

use crate::{Result, execute};

fn arr(b: &Value) -> Vec<Value> {
    b.get("items").and_then(|v| v.as_array()).cloned().unwrap_or_else(|| vec![b.clone()])
}
fn s(it: &Value, k: &str) -> Option<String> {
    it.get(k).and_then(|v| v.as_str()).map(str::to_owned)
}
fn dvs(it: &Value, k: &str) -> DataValue {
    match s(it, k) {
        Some(v) if !v.is_empty() => DataValue::String(v),
        _ => DataValue::NullTyped(SqlTypeMarker::Text),
    }
}
fn dvs_req(it: &Value, k: &str) -> DataValue {
    DataValue::String(s(it, k).unwrap_or_default())
}
fn dvi(it: &Value, k: &str, def: i64) -> DataValue {
    DataValue::Int(it.get(k).and_then(|v| match v {
        Value::Number(n) => n.as_i64(),
        Value::Bool(b) => Some(*b as i64),
        Value::String(s) => s.parse().ok(),
        _ => None,
    }).unwrap_or(def))
}
fn dvdec(it: &Value, k: &str) -> DataValue {
    let d = match it.get(k) {
        Some(Value::String(s)) => s.parse().unwrap_or(Decimal::ZERO),
        Some(Value::Number(n)) => n.as_f64().and_then(Decimal::from_f64_retain).unwrap_or(Decimal::ZERO),
        _ => Decimal::ZERO,
    };
    DataValue::Decimal(d)
}
fn pk() -> DataValue {
    DataValue::Int(cmx_utils::next_pk_id())
}

/// 合并方案(单条)。
pub async fn upsert_scheme(b: &Value) -> Result<Value> {
    execute(
        "INSERT INTO cg_consol_scheme (id, code, name, scheme_code, standard, group_currency, ledger, \
            investment_account, goodwill_account, nci_account, minority_pl_account, cta_account, capital_reserve_account, sort_no, status, create_time, update_time) \
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,0,1,CURRENT_TIMESTAMP,CURRENT_TIMESTAMP) \
         ON CONFLICT (scheme_code) DO UPDATE SET name=EXCLUDED.name, standard=EXCLUDED.standard, \
            group_currency=EXCLUDED.group_currency, ledger=EXCLUDED.ledger, \
            investment_account=EXCLUDED.investment_account, goodwill_account=EXCLUDED.goodwill_account, \
            nci_account=EXCLUDED.nci_account, minority_pl_account=EXCLUDED.minority_pl_account, \
            cta_account=EXCLUDED.cta_account, capital_reserve_account=EXCLUDED.capital_reserve_account, update_time=CURRENT_TIMESTAMP",
        vec![
            pk(), dvs_req(b, "schemeCode"), dvs(b, "name"), dvs_req(b, "schemeCode"),
            dvs(b, "standard"), dvs(b, "groupCurrency"), dvs(b, "ledger"),
            dvs(b, "investmentAccount"), dvs(b, "goodwillAccount"), dvs(b, "nciAccount"), dvs(b, "minorityPlAccount"),
            dvs(b, "ctaAccount"), dvs(b, "capitalReserveAccount"),
        ],
    ).await?;
    Ok(json!({ "ok": true }))
}

/// 集团科目表(批量)。
pub async fn upsert_group_accounts(b: &Value) -> Result<Value> {
    let items = arr(b);
    for (i, it) in items.iter().enumerate() {
        execute(
            "INSERT INTO cg_group_account (id, code, name, scheme_code, account_code, account_type, parent_code, is_equity, sort_no, status, create_time, update_time) \
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,1,CURRENT_TIMESTAMP,CURRENT_TIMESTAMP) \
             ON CONFLICT (scheme_code, account_code) DO UPDATE SET name=EXCLUDED.name, account_type=EXCLUDED.account_type, \
                parent_code=EXCLUDED.parent_code, is_equity=EXCLUDED.is_equity, update_time=CURRENT_TIMESTAMP",
            vec![
                pk(), dvs_req(it, "accountCode"), dvs(it, "name"), dvs_req(it, "schemeCode"),
                dvs_req(it, "accountCode"), dvs_req(it, "accountType"), dvs(it, "parentCode"),
                dvi(it, "isEquity", 0), dvi(it, "sortNo", i as i64),
            ],
        ).await?;
    }
    Ok(json!({ "ok": true, "saved": items.len() }))
}

/// CoA 科目映射(批量;C1)。唯一键 (scheme,entity_code,local_account)。entity_code 空=通配。
pub async fn upsert_coa_mapping(b: &Value) -> Result<Value> {
    let items = arr(b);
    for (i, it) in items.iter().enumerate() {
        execute(
            "INSERT INTO cg_coa_mapping (id, code, name, scheme_code, entity_code, local_account, group_account, sign, sort_no, status, create_time, update_time) \
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,1,CURRENT_TIMESTAMP,CURRENT_TIMESTAMP) \
             ON CONFLICT (scheme_code, entity_code, local_account) DO UPDATE SET group_account=EXCLUDED.group_account, sign=EXCLUDED.sign, update_time=CURRENT_TIMESTAMP",
            vec![
                pk(),
                DataValue::String(format!("{}|{}|{}", s(it,"schemeCode").unwrap_or_default(), s(it,"entityCode").unwrap_or_default(), s(it,"localAccount").unwrap_or_default())),
                dvs(it, "name"), dvs_req(it, "schemeCode"),
                // entity_code 允许空串(通配),故用 dvs_req 保留空串而非 NULL(唯一键含它)。
                DataValue::String(s(it, "entityCode").unwrap_or_default()),
                dvs_req(it, "localAccount"), dvs_req(it, "groupAccount"), dvi(it, "sign", 1), dvi(it, "sortNo", i as i64),
            ],
        ).await?;
    }
    Ok(json!({ "ok": true, "saved": items.len() }))
}

/// 合并范围(批量)。
pub async fn upsert_scope(b: &Value) -> Result<Value> {
    let items = arr(b);
    for (i, it) in items.iter().enumerate() {
        execute(
            "INSERT INTO cg_scope (id, code, name, scheme_code, period_code, org_code, org_name, parent_code, \
                consol_method, ownership_pct, is_leaf, level_no, investment_amount, currency, first_time, disposal, under_common_control, sort_no, status, create_time, update_time) \
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,1,CURRENT_TIMESTAMP,CURRENT_TIMESTAMP) \
             ON CONFLICT (scheme_code, period_code, org_code) DO UPDATE SET org_name=EXCLUDED.org_name, parent_code=EXCLUDED.parent_code, \
                consol_method=EXCLUDED.consol_method, ownership_pct=EXCLUDED.ownership_pct, is_leaf=EXCLUDED.is_leaf, \
                level_no=EXCLUDED.level_no, investment_amount=EXCLUDED.investment_amount, currency=EXCLUDED.currency, \
                under_common_control=EXCLUDED.under_common_control, update_time=CURRENT_TIMESTAMP",
            vec![
                pk(), dvs_req(it, "orgCode"), dvs(it, "orgName"), dvs_req(it, "schemeCode"), dvs_req(it, "periodCode"),
                dvs_req(it, "orgCode"), dvs(it, "orgName"), dvs(it, "parentCode"),
                dvs_req(it, "consolMethod"), dvdec(it, "ownershipPct"), dvi(it, "isLeaf", 0),
                dvi(it, "levelNo", 1), dvdec(it, "investmentAmount"), dvs(it, "currency"), dvi(it, "firstTime", 0), dvi(it, "disposal", 0),
                dvi(it, "underCommonControl", 0),
                dvi(it, "sortNo", i as i64),
            ],
        ).await?;
    }
    Ok(json!({ "ok": true, "saved": items.len() }))
}

/// 主体个别试算表(批量)。
pub async fn upsert_entity_balances(b: &Value) -> Result<Value> {
    let items = arr(b);
    for (i, it) in items.iter().enumerate() {
        execute(
            "INSERT INTO cg_entity_balance (id, code, name, scheme_code, period_code, entity_code, account_code, amount, sort_no, status, create_time, update_time) \
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,1,CURRENT_TIMESTAMP,CURRENT_TIMESTAMP) \
             ON CONFLICT (scheme_code, period_code, entity_code, account_code) DO UPDATE SET amount=EXCLUDED.amount, update_time=CURRENT_TIMESTAMP",
            vec![
                pk(),
                DataValue::String(format!("{}|{}|{}|{}", s(it,"schemeCode").unwrap_or_default(), s(it,"periodCode").unwrap_or_default(), s(it,"entityCode").unwrap_or_default(), s(it,"accountCode").unwrap_or_default())),
                dvs(it, "name"), dvs_req(it, "schemeCode"), dvs_req(it, "periodCode"),
                dvs_req(it, "entityCode"), dvs_req(it, "accountCode"), dvdec(it, "amount"), dvi(it, "sortNo", i as i64),
            ],
        ).await?;
    }
    Ok(json!({ "ok": true, "saved": items.len() }))
}

/// 抵销规则(批量)。
pub async fn upsert_elim_rules(b: &Value) -> Result<Value> {
    let items = arr(b);
    for (i, it) in items.iter().enumerate() {
        execute(
            "INSERT INTO cg_elim_rule (id, code, name, scheme_code, rule_code, elim_type, dr_account, cr_account, enabled, remark, sort_no, status, create_time, update_time) \
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,1,CURRENT_TIMESTAMP,CURRENT_TIMESTAMP) \
             ON CONFLICT (scheme_code, rule_code) DO UPDATE SET elim_type=EXCLUDED.elim_type, dr_account=EXCLUDED.dr_account, \
                cr_account=EXCLUDED.cr_account, enabled=EXCLUDED.enabled, update_time=CURRENT_TIMESTAMP",
            vec![
                pk(), dvs_req(it, "ruleCode"), dvs(it, "name"), dvs_req(it, "schemeCode"), dvs_req(it, "ruleCode"),
                dvs_req(it, "elimType"), dvs(it, "drAccount"), dvs(it, "crAccount"), dvi(it, "enabled", 1), dvs(it, "remark"), dvi(it, "sortNo", i as i64),
            ],
        ).await?;
    }
    Ok(json!({ "ok": true, "saved": items.len() }))
}

/// 内部往来匹配(批量)。
pub async fn upsert_ic_matches(b: &Value) -> Result<Value> {
    let items = arr(b);
    for (i, it) in items.iter().enumerate() {
        execute(
            "INSERT INTO cg_ic_match (id, code, name, scheme_code, period_code, entity_a, entity_b, ic_type, amount, status, sort_no, create_time, update_time) \
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,1,$10,CURRENT_TIMESTAMP,CURRENT_TIMESTAMP) \
             ON CONFLICT (scheme_code, period_code, entity_a, entity_b, ic_type) DO UPDATE SET amount=EXCLUDED.amount, update_time=CURRENT_TIMESTAMP",
            vec![
                pk(),
                DataValue::String(format!("{}|{}|{}|{}|{}", s(it,"schemeCode").unwrap_or_default(), s(it,"periodCode").unwrap_or_default(), s(it,"entityA").unwrap_or_default(), s(it,"entityB").unwrap_or_default(), s(it,"icType").unwrap_or_default())),
                dvs(it, "name"), dvs_req(it, "schemeCode"), dvs_req(it, "periodCode"),
                dvs_req(it, "entityA"), dvs_req(it, "entityB"), dvs_req(it, "icType"), dvdec(it, "amount"), dvi(it, "sortNo", i as i64),
            ],
        ).await?;
    }
    Ok(json!({ "ok": true, "saved": items.len() }))
}

/// 内部往来申报(批量;C4)。唯一键 (scheme,period,entity_code,partner_code,ic_type,direction)。
pub async fn upsert_ic_declarations(b: &Value) -> Result<Value> {
    let items = arr(b);
    for (i, it) in items.iter().enumerate() {
        execute(
            "INSERT INTO cg_ic_declaration (id, code, name, scheme_code, period_code, entity_code, partner_code, ic_type, direction, amount, sort_no, status, create_time, update_time) \
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,1,CURRENT_TIMESTAMP,CURRENT_TIMESTAMP) \
             ON CONFLICT (scheme_code, period_code, entity_code, partner_code, ic_type, direction) DO UPDATE SET amount=EXCLUDED.amount, update_time=CURRENT_TIMESTAMP",
            vec![
                pk(),
                DataValue::String(format!("{}|{}|{}|{}|{}|{}", s(it,"schemeCode").unwrap_or_default(), s(it,"periodCode").unwrap_or_default(), s(it,"entityCode").unwrap_or_default(), s(it,"partnerCode").unwrap_or_default(), s(it,"icType").unwrap_or_default(), s(it,"direction").unwrap_or_default())),
                dvs(it, "name"), dvs_req(it, "schemeCode"), dvs_req(it, "periodCode"),
                dvs_req(it, "entityCode"), dvs_req(it, "partnerCode"), dvs_req(it, "icType"), dvs_req(it, "direction"), dvdec(it, "amount"), dvi(it, "sortNo", i as i64),
            ],
        ).await?;
    }
    Ok(json!({ "ok": true, "saved": items.len() }))
}

/// 汇率(批量;外币折算 C5)。唯一键 (scheme,period,from_ccy,to_ccy,rate_type)。
pub async fn upsert_fx_rates(b: &Value) -> Result<Value> {
    let items = arr(b);
    for (i, it) in items.iter().enumerate() {
        let to_ccy = s(it, "toCcy").filter(|s| !s.is_empty()).unwrap_or_else(|| "CNY".into());
        execute(
            "INSERT INTO cg_fx_rate (id, code, name, scheme_code, period_code, from_ccy, to_ccy, rate_type, rate, sort_no, status, create_time, update_time) \
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,1,CURRENT_TIMESTAMP,CURRENT_TIMESTAMP) \
             ON CONFLICT (scheme_code, period_code, from_ccy, to_ccy, rate_type) DO UPDATE SET rate=EXCLUDED.rate, update_time=CURRENT_TIMESTAMP",
            vec![
                pk(),
                DataValue::String(format!("{}|{}|{}|{}|{}", s(it,"schemeCode").unwrap_or_default(), s(it,"periodCode").unwrap_or_default(), s(it,"fromCcy").unwrap_or_default(), to_ccy, s(it,"rateType").unwrap_or_default())),
                dvs(it, "name"), dvs_req(it, "schemeCode"), dvs_req(it, "periodCode"),
                dvs_req(it, "fromCcy"), DataValue::String(to_ccy), dvs_req(it, "rateType"), dvdec(it, "rate"), dvi(it, "sortNo", i as i64),
            ],
        ).await?;
    }
    Ok(json!({ "ok": true, "saved": items.len() }))
}

/// 存货未实现内部利润(批量;C6)。唯一键 (scheme,period,seller,buyer)。
pub async fn upsert_interim_profit(b: &Value) -> Result<Value> {
    let items = arr(b);
    for (i, it) in items.iter().enumerate() {
        execute(
            "INSERT INTO cg_interim_profit (id, code, name, scheme_code, period_code, seller, buyer, opening_profit, ending_profit, sort_no, status, create_time, update_time) \
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,1,CURRENT_TIMESTAMP,CURRENT_TIMESTAMP) \
             ON CONFLICT (scheme_code, period_code, seller, buyer) DO UPDATE SET opening_profit=EXCLUDED.opening_profit, ending_profit=EXCLUDED.ending_profit, update_time=CURRENT_TIMESTAMP",
            vec![
                pk(),
                DataValue::String(format!("{}|{}|{}|{}", s(it,"schemeCode").unwrap_or_default(), s(it,"periodCode").unwrap_or_default(), s(it,"seller").unwrap_or_default(), s(it,"buyer").unwrap_or_default())),
                dvs(it, "name"), dvs_req(it, "schemeCode"), dvs_req(it, "periodCode"),
                dvs_req(it, "seller"), dvs_req(it, "buyer"), dvdec(it, "openingProfit"), dvdec(it, "endingProfit"), dvi(it, "sortNo", i as i64),
            ],
        ).await?;
    }
    Ok(json!({ "ok": true, "saved": items.len() }))
}

/// 商誉减值(批量;C6)。唯一键 (scheme,period,node_code)。
pub async fn upsert_goodwill_impair(b: &Value) -> Result<Value> {
    let items = arr(b);
    for (i, it) in items.iter().enumerate() {
        execute(
            "INSERT INTO cg_goodwill_impair (id, code, name, scheme_code, period_code, node_code, amount, sort_no, status, create_time, update_time) \
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,1,CURRENT_TIMESTAMP,CURRENT_TIMESTAMP) \
             ON CONFLICT (scheme_code, period_code, node_code) DO UPDATE SET amount=EXCLUDED.amount, update_time=CURRENT_TIMESTAMP",
            vec![
                pk(),
                DataValue::String(format!("{}|{}|{}", s(it,"schemeCode").unwrap_or_default(), s(it,"periodCode").unwrap_or_default(), s(it,"nodeCode").unwrap_or_default())),
                dvs(it, "name"), dvs_req(it, "schemeCode"), dvs_req(it, "periodCode"),
                dvs_req(it, "nodeCode"), dvdec(it, "amount"), dvi(it, "sortNo", i as i64),
            ],
        ).await?;
    }
    Ok(json!({ "ok": true, "saved": items.len() }))
}

/// 现金流量项目流水(批量;N2 CCF 输入,主体原始行 node_code='')。
/// 唯一键 (scheme,period,node_code,entity_code,item_code)。录入主体行:node_code 传空串。
pub async fn upsert_cash_flow_items(b: &Value) -> Result<Value> {
    let items = arr(b);
    for (i, it) in items.iter().enumerate() {
        let node = s(it, "nodeCode").unwrap_or_default();
        let entity = s(it, "entityCode").unwrap_or_default();
        execute(
            "INSERT INTO cg_cash_flow_item (id, code, name, scheme_code, period_code, node_code, entity_code, \
                activity, item_code, item_name, inflow, outflow, amount, is_intercompany, counterparty, sort_no, status, create_time, update_time) \
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,1,CURRENT_TIMESTAMP,CURRENT_TIMESTAMP) \
             ON CONFLICT (scheme_code, period_code, node_code, entity_code, item_code) DO UPDATE SET \
                activity=EXCLUDED.activity, item_name=EXCLUDED.item_name, inflow=EXCLUDED.inflow, outflow=EXCLUDED.outflow, \
                amount=EXCLUDED.amount, is_intercompany=EXCLUDED.is_intercompany, counterparty=EXCLUDED.counterparty, update_time=CURRENT_TIMESTAMP",
            vec![
                pk(),
                DataValue::String(format!("{}|{}|{}|{}|{}", s(it,"schemeCode").unwrap_or_default(), s(it,"periodCode").unwrap_or_default(), node, entity, s(it,"itemCode").unwrap_or_default())),
                dvs(it, "name"), dvs_req(it, "schemeCode"), dvs_req(it, "periodCode"),
                DataValue::String(node), DataValue::String(entity),
                dvs_req(it, "activity"), dvs_req(it, "itemCode"), dvs(it, "itemName"),
                dvdec(it, "inflow"), dvdec(it, "outflow"), dvdec(it, "amount"),
                dvi(it, "isIntercompany", 0), dvs(it, "counterparty"), dvi(it, "sortNo", i as i64),
            ],
        ).await?;
    }
    Ok(json!({ "ok": true, "saved": items.len() }))
}

/// L3 固定资产内部交易未实现利润(批量)。唯一键 (scheme,period,seller,buyer)。
pub async fn upsert_fa_profit(b: &Value) -> Result<Value> {
    let items = arr(b);
    for (i, it) in items.iter().enumerate() {
        execute(
            "INSERT INTO cg_fa_profit (id, code, name, scheme_code, period_code, seller, buyer, unrealized, dep_years, elapsed_years, sort_no, status, create_time, update_time) \
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,1,CURRENT_TIMESTAMP,CURRENT_TIMESTAMP) \
             ON CONFLICT (scheme_code, period_code, seller, buyer) DO UPDATE SET unrealized=EXCLUDED.unrealized, \
                dep_years=EXCLUDED.dep_years, elapsed_years=EXCLUDED.elapsed_years, update_time=CURRENT_TIMESTAMP",
            vec![
                pk(),
                DataValue::String(format!("{}|{}|{}|{}", s(it,"schemeCode").unwrap_or_default(), s(it,"periodCode").unwrap_or_default(), s(it,"seller").unwrap_or_default(), s(it,"buyer").unwrap_or_default())),
                dvs(it, "name"), dvs_req(it, "schemeCode"), dvs_req(it, "periodCode"),
                dvs_req(it, "seller"), dvs_req(it, "buyer"), dvdec(it, "unrealized"), dvdec(it, "depYears"), dvdec(it, "elapsedYears"),
                dvi(it, "sortNo", i as i64),
            ],
        ).await?;
    }
    Ok(json!({ "ok": true, "saved": items.len() }))
}

/// L2 分步取得/处置交易(批量)。唯一键 (scheme,period,node_code,txn_type)。
pub async fn upsert_step_txns(b: &Value) -> Result<Value> {
    let items = arr(b);
    for (i, it) in items.iter().enumerate() {
        execute(
            "INSERT INTO cg_step_txn (id, code, name, scheme_code, period_code, node_code, txn_type, loses_control, \
                prev_carrying, prev_fair_value, proceeds, disposed_share, retained_fair_value, net_assets_share, sort_no, status, create_time, update_time) \
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,1,CURRENT_TIMESTAMP,CURRENT_TIMESTAMP) \
             ON CONFLICT (scheme_code, period_code, node_code, txn_type) DO UPDATE SET loses_control=EXCLUDED.loses_control, \
                prev_carrying=EXCLUDED.prev_carrying, prev_fair_value=EXCLUDED.prev_fair_value, proceeds=EXCLUDED.proceeds, \
                disposed_share=EXCLUDED.disposed_share, retained_fair_value=EXCLUDED.retained_fair_value, \
                net_assets_share=EXCLUDED.net_assets_share, update_time=CURRENT_TIMESTAMP",
            vec![
                pk(),
                DataValue::String(format!("{}|{}|{}|{}", s(it,"schemeCode").unwrap_or_default(), s(it,"periodCode").unwrap_or_default(), s(it,"nodeCode").unwrap_or_default(), s(it,"txnType").unwrap_or_default())),
                dvs(it, "name"), dvs_req(it, "schemeCode"), dvs_req(it, "periodCode"),
                dvs_req(it, "nodeCode"), dvs_req(it, "txnType"), dvi(it, "losesControl", 0),
                dvdec(it, "prevCarrying"), dvdec(it, "prevFairValue"), dvdec(it, "proceeds"),
                dvdec(it, "disposedShare"), dvdec(it, "retainedFairValue"), dvdec(it, "netAssetsShare"),
                dvi(it, "sortNo", i as i64),
            ],
        ).await?;
    }
    Ok(json!({ "ok": true, "saved": items.len() }))
}

/// 权益变动流水(批量;N2 CSE 输入,主体原始行 node_code='')。
/// 唯一键 (scheme,period,node_code,entity_code,equity_item,change_type)。
pub async fn upsert_equity_changes(b: &Value) -> Result<Value> {
    let items = arr(b);
    for (i, it) in items.iter().enumerate() {
        let node = s(it, "nodeCode").unwrap_or_default();
        let entity = s(it, "entityCode").unwrap_or_default();
        execute(
            "INSERT INTO cg_equity_change (id, code, name, scheme_code, period_code, node_code, entity_code, \
                equity_item, change_type, column_code, amount, attributable, sort_no, status, create_time, update_time) \
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,1,CURRENT_TIMESTAMP,CURRENT_TIMESTAMP) \
             ON CONFLICT (scheme_code, period_code, node_code, entity_code, equity_item, change_type) DO UPDATE SET \
                column_code=EXCLUDED.column_code, amount=EXCLUDED.amount, attributable=EXCLUDED.attributable, update_time=CURRENT_TIMESTAMP",
            vec![
                pk(),
                DataValue::String(format!("{}|{}|{}|{}|{}|{}", s(it,"schemeCode").unwrap_or_default(), s(it,"periodCode").unwrap_or_default(), node, entity, s(it,"equityItem").unwrap_or_default(), s(it,"changeType").unwrap_or_default())),
                dvs(it, "name"), dvs_req(it, "schemeCode"), dvs_req(it, "periodCode"),
                DataValue::String(node), DataValue::String(entity),
                dvs_req(it, "equityItem"), dvs_req(it, "changeType"), dvs(it, "columnCode"),
                dvdec(it, "amount"), dvs(it, "attributable"), dvi(it, "sortNo", i as i64),
            ],
        ).await?;
    }
    Ok(json!({ "ok": true, "saved": items.len() }))
}
