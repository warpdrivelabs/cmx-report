//! notes —— L7 合并财务报表附注自动生成(从既有 cg_* 派生,零新表)。
//!
//! 从合并结果 cg_consol_data / cg_elim_journal / cg_scope_change / cg_goodwill_impair 派生
//! 结构化附注 JSON:①少数股东权益与损益明细 ②商誉变动(资本抵销形成 − 减值)③合并范围变动。
//! 纯读派生,前端「附注」tab 直接渲染;不落库、不改核心循环。

use serde_json::{Value, json};

use cmx_core::model::cell::DataValue;

use crate::{Result, dv_dec, query_rows, sv};

/// 生成某方案某期的合并附注(以根节点为集团口径)。
pub async fn generate_notes(scheme: &str, period: &str, node: Option<&str>) -> Result<Value> {
    // 解析集团根节点(未指定 → 取无 parent 的顶层)。
    let node = match node.filter(|s| !s.is_empty()) {
        Some(n) => n.to_string(),
        None => {
            let rows = query_rows(
                "SELECT org_code FROM cg_scope WHERE scheme_code=$1 AND period_code=$2 \
                 AND COALESCE(parent_code,'')='' AND COALESCE(status,1)=1 ORDER BY level_no, org_code LIMIT 1",
                vec![DataValue::String(scheme.to_string()), DataValue::String(period.to_string())],
                "notes_root",
            )
            .await?;
            rows.first().and_then(|r| sv(r, "org_code")).unwrap_or_default()
        }
    };

    let nci = note_nci(scheme, period, &node).await?;
    let goodwill = note_goodwill(scheme, period, &node).await?;
    let scope_change = note_scope_change(scheme, period).await?;

    Ok(json!({
        "ok": true, "scheme": scheme, "period": period, "node": node,
        "notes": {
            "nci": nci,
            "goodwill": goodwill,
            "scopeChange": scope_change,
        },
        "message": "合并附注已生成",
    }))
}

/// ①少数股东权益 + 少数股东损益明细(从 cg_consol_data 取 nci/minority_pl 科目合并数)。
async fn note_nci(scheme: &str, period: &str, node: &str) -> Result<Value> {
    // 少数股东权益 = account_type=nci 的合并数;少数股东损益 = minority_pl 科目(从方案配置)。
    let rows = query_rows(
        "SELECT d.account_code, d.consolidated, COALESCE(a.account_type,'') AS account_type, COALESCE(a.name,'') AS acc_name \
         FROM cg_consol_data d LEFT JOIN cg_group_account a \
           ON a.scheme_code=d.scheme_code AND a.account_code=d.account_code \
         WHERE d.scheme_code=$1 AND d.period_code=$2 AND d.node_code=$3",
        vec![
            DataValue::String(scheme.to_string()),
            DataValue::String(period.to_string()),
            DataValue::String(node.to_string()),
        ],
        "notes_nci",
    )
    .await?;
    let mut nci_equity = rust_decimal::Decimal::ZERO;
    let mut items = Vec::new();
    for r in &rows {
        let ty = sv(r, "account_type").unwrap_or_default().to_ascii_lowercase();
        if ty == "nci" || ty == "少数股东权益" {
            let v = dv_dec(r, "consolidated");
            nci_equity += v;
            items.push(json!({
                "account": sv(r, "account_code"),
                "name": sv(r, "acc_name"),
                "consolidated": v.to_string(),
                // 展示口径:权益为贷方(借方正为负),翻正。
                "presented": (-v).to_string(),
            }));
        }
    }
    Ok(json!({
        "title": "少数股东权益",
        "totalConsolidated": nci_equity.to_string(),
        "totalPresented": (-nci_equity).to_string(),
        "items": items,
    }))
}

/// ②商誉变动:期末商誉合并数 + 本期减值(cg_goodwill_impair);资本抵销形成的商誉由合并数体现。
async fn note_goodwill(scheme: &str, period: &str, node: &str) -> Result<Value> {
    // 期末商誉合并数(account_type=asset 且 code 命中方案 goodwill_account)。
    let gw_acc = {
        let rows = query_rows(
            "SELECT goodwill_account FROM cg_consol_scheme WHERE scheme_code=$1",
            vec![DataValue::String(scheme.to_string())],
            "notes_gw_acc",
        )
        .await?;
        rows.first().and_then(|r| sv(r, "goodwill_account")).unwrap_or_else(|| "1801".into())
    };
    let closing = {
        let rows = query_rows(
            "SELECT consolidated FROM cg_consol_data WHERE scheme_code=$1 AND period_code=$2 AND node_code=$3 AND account_code=$4",
            vec![
                DataValue::String(scheme.to_string()),
                DataValue::String(period.to_string()),
                DataValue::String(node.to_string()),
                DataValue::String(gw_acc.clone()),
            ],
            "notes_gw_closing",
        )
        .await?;
        rows.first().map(|r| dv_dec(r, "consolidated")).unwrap_or_default()
    };
    // 本期减值合计。
    let impair = {
        let rows = query_rows(
            "SELECT COALESCE(SUM(amount),0) AS s FROM cg_goodwill_impair WHERE scheme_code=$1 AND period_code=$2 AND COALESCE(status,1)=1",
            vec![DataValue::String(scheme.to_string()), DataValue::String(period.to_string())],
            "notes_gw_impair",
        )
        .await?;
        rows.first().map(|r| dv_dec(r, "s")).unwrap_or_default()
    };
    Ok(json!({
        "title": "商誉",
        "account": gw_acc,
        "closingBalance": closing.to_string(),
        "impairmentThisPeriod": impair.to_string(),
        "note": "期末商誉为资本抵销形成的合并价差扣减累计减值后的净额",
    }))
}

/// ③合并范围变动(直接引用 cg_scope_change)。
async fn note_scope_change(scheme: &str, period: &str) -> Result<Value> {
    let rows = query_rows(
        "SELECT org_code, org_name, change_type, curr_method, prev_method, curr_ownership, prev_ownership, prev_period \
         FROM cg_scope_change WHERE scheme_code=$1 AND period_code=$2 ORDER BY sort_no, org_code",
        vec![DataValue::String(scheme.to_string()), DataValue::String(period.to_string())],
        "notes_scope_change",
    )
    .await?;
    Ok(json!({ "title": "合并范围变动", "count": rows.len(), "items": rows }))
}

/// O3 自动内部往来调整建议:对 cg_ic_recon 的 diff 行(A≠B),生成建议调整分录方向与金额。
/// 纯分析(读对账结果,不落账):diff = a_amount − b_amount;建议把少报方调整到匹配对方。
/// 返回 { count, suggestions:[{entity_a,entity_b,ic_type,a_amount,b_amount,diff,suggestion,adjust_entity,adjust_amount}] }。
pub async fn ic_adjustment_suggestions(scheme: &str, period: &str) -> Result<Value> {
    let rows = query_rows(
        "SELECT entity_a, entity_b, ic_type, a_amount, b_amount, diff, recon_status \
         FROM cg_ic_recon WHERE scheme_code=$1 AND period_code=$2 AND recon_status='diff' \
         ORDER BY entity_a, entity_b, ic_type",
        vec![DataValue::String(scheme.to_string()), DataValue::String(period.to_string())],
        "ic_adj_suggest",
    )
    .await?;
    let mut out = Vec::new();
    for r in &rows {
        let a = dv_dec(r, "a_amount");
        let b = dv_dec(r, "b_amount");
        let diff = dv_dec(r, "diff"); // a − b
        let ea = sv(r, "entity_a").unwrap_or_default();
        let eb = sv(r, "entity_b").unwrap_or_default();
        // diff>0:A 侧(债权/收入)报多 → 建议调减 A 或调增 B;取「把少报方 B 调增 |diff|」。
        let (adjust_entity, adjust_amount, suggestion) = if diff > rust_decimal::Decimal::ZERO {
            (eb.clone(), diff, format!("{eb} 少报 {diff},建议调增 {eb} 至与 {ea} 匹配"))
        } else {
            (ea.clone(), -diff, format!("{ea} 少报 {},建议调增 {ea} 至与 {eb} 匹配", -diff))
        };
        out.push(json!({
            "entity_a": ea, "entity_b": eb, "ic_type": sv(r, "ic_type"),
            "a_amount": a.to_string(), "b_amount": b.to_string(), "diff": diff.to_string(),
            "adjust_entity": adjust_entity, "adjust_amount": adjust_amount.to_string(), "suggestion": suggestion,
        }));
    }
    Ok(json!({ "ok": true, "scheme": scheme, "period": period, "count": out.len(), "suggestions": out }))
}
