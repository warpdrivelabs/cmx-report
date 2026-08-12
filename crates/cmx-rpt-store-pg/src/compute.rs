//! compute —— 报表计算态服务（方案 §7/§8 落地）：装载 → 递归求值 → 落库。
//!
//! `compute_report_service(code, body)` 是 `POST /report-design/reports/{code}/compute` 的服务体：
//!   1. 装载主报表快照（cr_cell_element_map 单元格级公式 + cr_data_element 元素级继承 §6
//!      + cr_cell_data 已存值/手工标记）→ `ReportSnapshot`；
//!   2. 装载相对期间序列（cr_acct_calendar 叶子期间升序）+ 组织父级（cr_consol_org）；
//!   3. `cmx_rpt_formula::compute_report` 递归求值（两遍法取数 + REF 跨表 + 三色环检测），
//!      REF 他表经 `PgProvider::load_report` 懒装载同一装载器；
//!   4. 事务内 UPSERT 算好的格回 cr_cell_data（data_status=computed/error，is_manual=0）。
//!
//! 取数叶子（QM/QC/FS/JE）当前走 `PgProvider::batch_balance` 的**占位实现**（方案 §7
//! DataProvider 先占位）——P4 换 GlBalanceProvider 查 cv_gl_balance，本服务零改。

use std::collections::HashMap;

use async_trait::async_trait;
use rust_decimal::Decimal;
use serde_json::{Value, json};
use tracing::debug;

use cmx_core::dv;
use cmx_core::model::cell::{DataValue, SqlTypeMarker};
use cmx_database_pg::get_default_pg_db_manager;

use cmx_rpt_formula::{
    BalanceKey, CellInput, ComputeOutcome, DataProvider, FValue, ReportSnapshot, compute_report,
};
use cmx_rpt_model::{DEFAULT_REGION, RPT_DB_ID};

use crate::{Result, api_err, exec_dv, query_rows};

/// 单元格的持久化坐标（cr_cell_data 唯一键需要，从 cr_cell_element_map 带出）。
#[derive(Clone)]
struct CellCoord {
    sheet_code: String,
    region_code: String,
    row_id: i64,
    col_id: i64,
    element_code: Option<String>,
    value_type: Option<String>,
}

/// 装载一张报表某 org+period 的快照（主报表 + REF 他表共用）。
///
/// 返回 (snapshot, cell_ref → 持久化坐标)；坐标供落库用，快照供引擎用。
async fn load_snapshot(
    report: &str,
    version: &str,
    org: &str,
    period: &str,
) -> Result<Option<(ReportSnapshot, HashMap<String, CellCoord>)>> {
    // —— 单元格级：cr_cell_element_map（公式 + 元素绑定 + 坐标） ——
    let map_rows = query_rows(
        r#"SELECT sheet_code, region_code, row_id, col_id, cell_ref, element_code,
                  value_type, calc_formula
           FROM cr_cell_element_map
           WHERE report_code=$1 AND version_code=$2
           ORDER BY sheet_code, region_code, row_id, col_id"#,
        dv![report, version],
        "compute_cell_map",
    )
    .await?;
    if map_rows.is_empty() {
        return Ok(None);
    }

    // —— 元素级：cr_data_element（§6 继承源） ——
    let elem_rows = query_rows(
        r#"SELECT code, calc_formula FROM cr_data_element
           WHERE COALESCE(status,1)=1 AND calc_formula IS NOT NULL AND calc_formula <> ''"#,
        dv![],
        "compute_data_elements",
    )
    .await?;
    let mut element_formulas: HashMap<String, String> = HashMap::new();
    for e in &elem_rows {
        if let (Some(c), Some(f)) = (jstr(e, "code"), jstr(e, "calc_formula")) {
            element_formulas.insert(c, f);
        }
    }

    // —— 已存值 + 手工标记：cr_cell_data（按 org+period），键 cell_ref ——
    let data_rows = query_rows(
        r#"SELECT cell_ref, row_id, col_id, value_type, text_value, num_value,
                  data_status, is_manual
           FROM cr_cell_data
           WHERE report_code=$1 AND version_code=$2 AND org_code=$3 AND period_code=$4"#,
        dv![report, version, org, period],
        "compute_existing_data",
    )
    .await?;
    // 键：优先 cell_ref；缺失退化为 row|col（与 map 对齐）
    let mut stored: HashMap<String, (Option<FValue>, bool)> = HashMap::new();
    for d in &data_rows {
        let key = jstr(d, "cell_ref").unwrap_or_else(|| {
            format!(
                "{}|{}",
                jint(d, "row_id").unwrap_or(0),
                jint(d, "col_id").unwrap_or(0)
            )
        });
        let is_manual = jint(d, "is_manual").unwrap_or(0) == 1;
        let val = json_cell_value(d);
        stored.insert(key, (val, is_manual));
    }

    // —— 组装 CellInput + 坐标 ——
    let mut cells: Vec<CellInput> = Vec::with_capacity(map_rows.len());
    let mut coords: HashMap<String, CellCoord> = HashMap::new();
    for m in &map_rows {
        let row_id = jint(m, "row_id").unwrap_or(0);
        let col_id = jint(m, "col_id").unwrap_or(0);
        // cell_ref 是公式寻址键；缺失则用 row|col 兜底（不可被 A1 引用，但仍可自算）
        let cell_ref = jstr(m, "cell_ref")
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| format!("{row_id}|{col_id}"));
        let coord_key = cell_ref.clone();
        let (stored_val, is_manual) = stored.get(&coord_key).cloned().unwrap_or((None, false));
        cells.push(CellInput {
            cell_ref: cell_ref.clone(),
            element_code: jstr(m, "element_code").filter(|s| !s.is_empty()),
            cell_formula: jstr(m, "calc_formula").filter(|s| !s.trim().is_empty()),
            stored: stored_val,
            is_manual,
        });
        coords.insert(
            cell_ref,
            CellCoord {
                sheet_code: jstr(m, "sheet_code").unwrap_or_default(),
                region_code: jstr(m, "region_code")
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| DEFAULT_REGION.to_string()),
                row_id,
                col_id,
                element_code: jstr(m, "element_code").filter(|s| !s.is_empty()),
                value_type: jstr(m, "value_type").filter(|s| !s.is_empty()),
            },
        );
    }

    let snap = ReportSnapshot {
        report: report.to_string(),
        version: version.to_string(),
        org: org.to_string(),
        period: period.to_string(),
        cells,
        element_formulas,
    };
    Ok(Some((snap, coords)))
}

/// 升序叶子期间序列（相对期间偏移用）：cr_acct_calendar 叶子按 fiscal_year+period_no 升序。
async fn load_periods() -> Result<Vec<String>> {
    let rows = query_rows(
        r#"SELECT code FROM cr_acct_calendar
           WHERE COALESCE(status,1)=1 AND COALESCE(is_leaf,1)=1
           ORDER BY fiscal_year, period_no"#,
        dv![],
        "compute_periods",
    )
    .await?;
    Ok(rows.iter().filter_map(|r| jstr(r, "code")).collect())
}

/// 组织父级映射 code → parentCode（@parent 解析用），经 id/parent_id 组合。
async fn load_org_parent() -> Result<HashMap<String, String>> {
    let rows = query_rows(
        r#"SELECT id, code, parent_id FROM cr_consol_org WHERE COALESCE(status,1)=1"#,
        dv![],
        "compute_org_parent",
    )
    .await?;
    let mut id_to_code: HashMap<i64, String> = HashMap::new();
    for r in &rows {
        if let (Some(id), Some(code)) = (jint(r, "id"), jstr(r, "code")) {
            id_to_code.insert(id, code);
        }
    }
    let mut out: HashMap<String, String> = HashMap::new();
    for r in &rows {
        if let (Some(code), Some(pid)) = (jstr(r, "code"), jint(r, "parent_id"))
            && let Some(pcode) = id_to_code.get(&pid)
        {
            out.insert(code, pcode.clone());
        }
    }
    Ok(out)
}

/// PG 取数实现：REF 他表经 load_snapshot 懒装载（真跨表递归）；QM/QC/FS/JE 占位（P4 接 GL）。
struct PgProvider;

#[async_trait]
impl DataProvider for PgProvider {
    async fn batch_balance(
        &self,
        keys: &[BalanceKey],
    ) -> std::result::Result<HashMap<BalanceKey, Decimal>, String> {
        // 占位（方案 §7 DataProvider 先占位）：按对象码派生一个稳定演示值，
        // 让计算态整链路（装载→算→落库→取数）可端到端验证；P4 换 cv_gl_balance 真源。
        let mut out = HashMap::new();
        for k in keys {
            out.insert(k.clone(), placeholder_balance(k));
        }
        Ok(out)
    }

    async fn load_report(
        &self,
        report: &str,
        version: &str,
        org: &str,
        period: &str,
    ) -> std::result::Result<Option<ReportSnapshot>, String> {
        match load_snapshot(report, version, org, period).await {
            Ok(Some((snap, _))) => Ok(Some(snap)),
            Ok(None) => Ok(None),
            Err(e) => Err(format!("{e}")),
        }
    }
}

/// 占位取数值：对象码尾部数字 + 种类偏移，稳定可复现（非随机，便于断言/演示）。
fn placeholder_balance(k: &BalanceKey) -> Decimal {
    use cmx_rpt_formula::FetchKind::*;
    let digits: i64 = k
        .object
        .chars()
        .filter(|c| c.is_ascii_digit())
        .collect::<String>()
        .parse()
        .unwrap_or(0);
    let base = 1000 + digits % 9000;
    let bump = match k.kind {
        EndBalance => 0,
        BeginBalance => -100,
        DebitAmount => 10,
        CreditAmount => 20,
        NetAmount => 30,
    };
    Decimal::from(base + bump)
}

/// 计算态服务：装载 → 递归求值 → 落库。返回 { ok, computed, errorCount, errors, cells }。
pub async fn compute_report_service(code: &str, body: &Value) -> Result<Value> {
    let version = resolve_version(code, jstr(body, "version")).await?;
    let org = jstr(body, "orgCode")
        .filter(|s| !s.is_empty())
        .ok_or_else(|| api_err("orgCode 不能为空"))?;
    let period = jstr(body, "periodCode")
        .filter(|s| !s.is_empty())
        .ok_or_else(|| api_err("periodCode 不能为空"))?;

    debug!(
        "{:<12} - compute {code}/{version} org={org} period={period}",
        "RPT-COMPUTE"
    );

    // 1) 装载主报表快照 + 坐标
    let (snapshot, coords) = load_snapshot(code, &version, &org, &period)
        .await?
        .ok_or_else(|| api_err("报表无单元格映射（cr_cell_element_map 为空），无法计算"))?;

    // 2) 相对期间序列 + 组织父级
    let periods = load_periods().await?;
    let org_parent = load_org_parent().await?;

    // 3) 递归求值（两遍法取数 + REF 跨表 + 三色环检测）
    let outcome: ComputeOutcome = compute_report(snapshot, &PgProvider, periods, org_parent).await;

    // 4) 落库：事务内 UPSERT 算好的格
    persist_outcome(code, &version, &org, &period, &outcome, &coords).await?;

    let cells_json: Vec<Value> = outcome
        .cells
        .iter()
        .map(|c| {
            json!({
                "cellRef": c.cell_ref,
                "value": fvalue_json(&c.value),
                "status": c.status,
            })
        })
        .collect();

    Ok(json!({
        "dbId": RPT_DB_ID,
        "reportCode": code,
        "version": version,
        "orgCode": org,
        "periodCode": period,
        "ok": outcome.error_count == 0,
        "computed": outcome.computed,
        "errorCount": outcome.error_count,
        "errors": outcome.errors,
        "cells": cells_json,
    }))
}

/// 落库算好的单元格（事务，UPSERT cr_cell_data 8 元键，data_status=computed/error，is_manual=0）。
async fn persist_outcome(
    code: &str,
    version: &str,
    org: &str,
    period: &str,
    outcome: &ComputeOutcome,
    coords: &HashMap<String, CellCoord>,
) -> Result<()> {
    if outcome.cells.is_empty() {
        return Ok(());
    }
    let mm = get_default_pg_db_manager();
    let tx = mm.get_transaction_context();
    let txn_id = tx
        .begin(RPT_DB_ID)
        .await
        .map_err(|e| api_err(&format!("开启事务失败: {e}")))?;

    let result = persist_apply(&txn_id, code, version, org, period, outcome, coords).await;
    match result {
        Ok(()) => {
            tx.commit(&txn_id)
                .await
                .map_err(|e| api_err(&format!("提交事务失败: {e}")))?;
            Ok(())
        }
        Err(e) => {
            let _ = tx.rollback(&txn_id).await;
            Err(e)
        }
    }
}

async fn persist_apply(
    txn_id: &str,
    code: &str,
    version: &str,
    org: &str,
    period: &str,
    outcome: &ComputeOutcome,
    coords: &HashMap<String, CellCoord>,
) -> Result<()> {
    for c in &outcome.cells {
        let Some(coord) = coords.get(&c.cell_ref) else {
            continue; // 无坐标（理论不会：cells 来自 coords 同源）
        };
        // 数值 / 文本 分流
        let (num_dv, text_dv, value_type) = match &c.value {
            FValue::Num(d) => (
                DataValue::Decimal(*d),
                DataValue::NullTyped(SqlTypeMarker::Text),
                coord.value_type.clone().unwrap_or_else(|| "amount".into()),
            ),
            FValue::Error(e) => (
                DataValue::NullTyped(SqlTypeMarker::Decimal),
                DataValue::String(e.clone()),
                coord.value_type.clone().unwrap_or_else(|| "amount".into()),
            ),
            other => (
                DataValue::NullTyped(SqlTypeMarker::Decimal),
                DataValue::String(other.as_text()),
                "text".to_string(),
            ),
        };
        let cell_ref_dv = if c.cell_ref.contains('|') {
            // 兜底键（无真 cell_ref）：写 NULL
            DataValue::NullTyped(SqlTypeMarker::Text)
        } else {
            DataValue::String(c.cell_ref.clone())
        };

        exec_dv(
            txn_id,
            r#"INSERT INTO cr_cell_data
               (id, org_code, period_code, report_code, version_code, sheet_code, region_code,
                row_id, col_id, cell_ref, element_code, value_type, text_value, num_value,
                currency_code, amount_unit, data_status, is_manual, compute_time, sort_no, status,
                create_time, update_time)
               VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,NULL,NULL,$15,0,CURRENT_TIMESTAMP,0,1,CURRENT_TIMESTAMP,CURRENT_TIMESTAMP)
               ON CONFLICT (org_code, period_code, report_code, version_code, sheet_code, region_code, row_id, col_id)
               DO UPDATE SET cell_ref=EXCLUDED.cell_ref, element_code=EXCLUDED.element_code,
                 value_type=EXCLUDED.value_type, text_value=EXCLUDED.text_value,
                 num_value=EXCLUDED.num_value, data_status=EXCLUDED.data_status,
                 is_manual=0, compute_time=CURRENT_TIMESTAMP, update_time=CURRENT_TIMESTAMP"#,
            vec![
                DataValue::Int(cmx_utils::next_pk_id()),
                DataValue::String(org.to_string()),
                DataValue::String(period.to_string()),
                DataValue::String(code.to_string()),
                DataValue::String(version.to_string()),
                DataValue::String(coord.sheet_code.clone()),
                DataValue::String(coord.region_code.clone()),
                DataValue::Int(coord.row_id),
                DataValue::Int(coord.col_id),
                cell_ref_dv,
                match &coord.element_code {
                    Some(e) => DataValue::String(e.clone()),
                    None => DataValue::NullTyped(SqlTypeMarker::Text),
                },
                DataValue::String(value_type),
                text_dv,
                num_dv,
                DataValue::String(c.status.to_string()),
            ],
        )
        .await?;
    }
    Ok(())
}

/// 解析生效版本：给定非空则用；否则取当前版本（is_current=1），再否则最新。
async fn resolve_version(code: &str, given: Option<String>) -> Result<String> {
    if let Some(v) = given.filter(|s| !s.trim().is_empty()) {
        return Ok(v);
    }
    let rows = query_rows(
        r#"SELECT code FROM cr_report_version
           WHERE report_code=$1
           ORDER BY COALESCE(is_current,0) DESC, version_no DESC, code DESC
           LIMIT 1"#,
        dv![code],
        "compute_resolve_version",
    )
    .await?;
    rows.first()
        .and_then(|r| jstr(r, "code"))
        .ok_or_else(|| api_err("报表无版本，无法计算"))
}

// ─────────────────────── JSON 取值 helper ───────────────────────

fn jstr(v: &Value, k: &str) -> Option<String> {
    v.get(k).and_then(|x| x.as_str()).map(str::to_owned)
}

fn jint(v: &Value, k: &str) -> Option<i64> {
    v.get(k).and_then(|x| match x {
        Value::Number(n) => n.as_i64(),
        Value::String(s) => s.parse().ok(),
        _ => None,
    })
}

/// cr_cell_data 一行 JSON → FValue（数值优先，其次文本）。
fn json_cell_value(row: &Value) -> Option<FValue> {
    if let Some(n) = row.get("num_value") {
        match n {
            Value::Number(x) => {
                if let Some(f) = x.as_f64() {
                    return Decimal::from_f64_retain(f).map(FValue::Num);
                }
            }
            Value::String(s) if !s.is_empty() => {
                if let Ok(d) = s.parse::<Decimal>() {
                    return Some(FValue::Num(d));
                }
            }
            _ => {}
        }
    }
    jstr(row, "text_value")
        .filter(|s| !s.is_empty())
        .map(FValue::Str)
}

fn fvalue_json(v: &FValue) -> Value {
    match v {
        FValue::Num(d) => json!(d.normalize().to_string()),
        FValue::Str(s) => json!(s),
        FValue::Bool(b) => json!(b),
        FValue::Null => Value::Null,
        FValue::Error(e) => json!(e),
    }
}
