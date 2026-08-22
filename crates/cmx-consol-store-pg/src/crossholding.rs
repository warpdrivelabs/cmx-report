//! crossholding —— L6 交叉持股·有效持股比例(矩阵法/迭代收敛)。
//!
//! 读 `cg_shareholding`(holder→held 直接持股 + is_parent 标记母公司持股),用纯算法
//! `effective_ownership`(cmx-consol-model)求母公司对各主体的有效持股(交叉/环持收敛)。
//! 纯分析端点,不改 run_consolidation 核心循环(有效持股用于 NCI 精算是后续增强)。

use std::collections::HashMap;

use serde_json::{Value, json};

use cmx_core::model::cell::DataValue;

use cmx_consol_model::effective_ownership;

use crate::{Result, api_err, dv_dec, iv, query_rows, sv};

/// 求某方案某期的有效持股。返回 { entities:[...], effective:{code:pct}, direct:{...} }。
pub async fn compute_effective_ownership(scheme: &str, period: &str) -> Result<Value> {
    let rows = query_rows(
        "SELECT holder, held, pct, is_parent FROM cg_shareholding \
         WHERE scheme_code=$1 AND period_code=$2 AND COALESCE(status,1)=1",
        vec![DataValue::String(scheme.to_string()), DataValue::String(period.to_string())],
        "consol_shareholding",
    )
    .await?;
    if rows.is_empty() {
        return Err(api_err("该方案该期间未配置持股关系(cg_shareholding)"));
    }
    // 收集主体全集(holder + held,排除母公司自身用 is_parent 标记的 holder)。
    let mut parent_direct: HashMap<String, rust_decimal::Decimal> = HashMap::new();
    let mut cross: HashMap<(String, String), rust_decimal::Decimal> = HashMap::new();
    let mut entset: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for r in &rows {
        let holder = sv(r, "holder").unwrap_or_default();
        let held = sv(r, "held").unwrap_or_default();
        let pct = dv_dec(r, "pct");
        let is_parent = iv(r, "is_parent").unwrap_or(0) == 1;
        if held.is_empty() { continue; }
        if is_parent {
            *parent_direct.entry(held.clone()).or_default() += pct;
        } else if !holder.is_empty() {
            *cross.entry((holder.clone(), held.clone())).or_default() += pct;
            entset.insert(holder);
        }
        entset.insert(held);
    }
    let entities: Vec<String> = entset.into_iter().collect();
    let eff = effective_ownership(&entities, &parent_direct, &cross);
    let eff_json: serde_json::Map<String, Value> = eff
        .iter()
        .map(|(k, v)| (k.clone(), json!(v.to_string())))
        .collect();
    let direct_json: serde_json::Map<String, Value> = parent_direct
        .iter()
        .map(|(k, v)| (k.clone(), json!(v.to_string())))
        .collect();
    Ok(json!({
        "ok": true, "scheme": scheme, "period": period,
        "entities": entities,
        "effective": eff_json,
        "parentDirect": direct_json,
        "message": format!("有效持股计算完成({} 个主体)", entities.len()),
    }))
}

/// 持股关系批量 UPSERT。唯一键 (scheme,period,holder,held)。
pub async fn upsert_shareholdings(b: &Value) -> Result<Value> {
    let items = b.get("items").and_then(|v| v.as_array()).cloned().unwrap_or_else(|| vec![b.clone()]);
    let s = |it: &Value, k: &str| it.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
    // 数值字段:容 JSON Number / String。
    let dec = |it: &Value, k: &str| -> rust_decimal::Decimal {
        match it.get(k) {
            Some(Value::String(x)) => x.parse().unwrap_or_default(),
            Some(Value::Number(n)) => n.as_f64().and_then(rust_decimal::Decimal::from_f64_retain).unwrap_or_default(),
            _ => rust_decimal::Decimal::default(),
        }
    };
    for (i, it) in items.iter().enumerate() {
        crate::execute(
            "INSERT INTO cg_shareholding (id, code, name, scheme_code, period_code, holder, held, pct, is_parent, sort_no, status, create_time, update_time) \
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,1,CURRENT_TIMESTAMP,CURRENT_TIMESTAMP) \
             ON CONFLICT (scheme_code, period_code, holder, held) DO UPDATE SET pct=EXCLUDED.pct, is_parent=EXCLUDED.is_parent, update_time=CURRENT_TIMESTAMP",
            vec![
                DataValue::Int(cmx_utils::next_pk_id()),
                DataValue::String(format!("{}|{}|{}|{}", s(it,"schemeCode"), s(it,"periodCode"), s(it,"holder"), s(it,"held"))),
                DataValue::NullTyped(cmx_core::model::cell::SqlTypeMarker::Text),
                DataValue::String(s(it,"schemeCode")),
                DataValue::String(s(it,"periodCode")),
                DataValue::String(s(it,"holder")),
                DataValue::String(s(it,"held")),
                DataValue::Decimal(dec(it, "pct")),
                DataValue::Int(it.get("isParent").and_then(|v| v.as_i64().or_else(|| v.as_bool().map(|b| b as i64))).unwrap_or(0)),
                DataValue::Int(i as i64),
            ],
        ).await?;
    }
    Ok(json!({ "ok": true, "saved": items.len() }))
}
