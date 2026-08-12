//! float_crud —— 浮动行/列存储表的增删改查服务（方案 F2）。
//!
//! 对 `cr_report_float_row` / `cr_report_float_col` 两表做 list/create/update/delete/batch。
//! 读走 ZmcDataSet 零拷贝，写走强类型 `exec_dv`（cells JSONB 用 `DataValue::Json` + `$N::jsonb`，
//! 镜像 cmx-job-store-pg）。按 org+period 隔离，唯一键 (8元+dim_key) 保证 CRUD/seed 幂等。
//!
//! 语义中立于「行 vs 列」：两表结构同构，用 `FloatKind` 分派表名与少数字段名（row_type/col_type）。

use serde_json::{Value, json};

use cmx_core::model::cell::DataValue;
use cmx_database_pg::{ZmcRowSource, get_default_pg_db_manager};
use cmx_rpt_model::{DEFAULT_REGION, RPT_DB_ID};

use crate::float_ddl::ensure_float_schema;
use crate::{Result, api_err, exec_dv, s};

/// 浮动方向：行或列。决定物理表名与类型字段名。
#[derive(Clone, Copy, PartialEq)]
pub enum FloatKind {
    Row,
    Col,
}

impl FloatKind {
    fn table(self) -> &'static str {
        match self {
            FloatKind::Row => "cr_report_float_row",
            FloatKind::Col => "cr_report_float_col",
        }
    }
    /// 类型字段名（row_type / col_type）与默认值。
    fn type_field(self) -> (&'static str, &'static str) {
        match self {
            FloatKind::Row => ("row_type", "detail"),
            FloatKind::Col => ("col_type", "data"),
        }
    }
}

/// 从请求体取定位 6 元（report 由 path 传入，其余从 body）。缺省安全值。
pub(crate) struct Locator {
    pub(crate) version: String,
    pub(crate) sheet: String,
    pub(crate) region: String,
    pub(crate) org: String,
    pub(crate) period: String,
}

pub(crate) fn make_locator(body: &Value) -> Locator {
    Locator {
        version: s(body, "version").unwrap_or_default(),
        sheet: s(body, "sheetCode").unwrap_or_default(),
        region: s(body, "regionCode")
            .filter(|c| !c.is_empty())
            .unwrap_or_else(|| DEFAULT_REGION.to_string()),
        org: s(body, "orgCode").unwrap_or_default(),
        period: s(body, "periodCode").unwrap_or_default(),
    }
}

/// 列出某 (报表+版本+sheet+区域+组织+期间) 的浮动行/列，按 seq 升序。
pub async fn list_float(code: &str, kind: FloatKind, body: &Value) -> Result<Value> {
    ensure_float_schema().await?;
    let loc = make_locator(body);
    let (type_field, _) = kind.type_field();
    let mm = get_default_pg_db_manager();
    let sql = format!(
        "SELECT id, dim_key, label, parent_dim_key, {type_field} AS kind_type, \
                level_no, seq, cells::text AS cells_text, is_manual, source_tag \
         FROM {} \
         WHERE report_code=$1 AND version_code=$2 AND sheet_code=$3 AND region_code=$4 \
           AND org_code=$5 AND period_code=$6 AND COALESCE(status,1)=1 \
         ORDER BY seq, id",
        kind.table()
    );
    let ds = mm
        .query_sql_zmc_with_datavalues(
            RPT_DB_ID,
            &sql,
            vec![
                DataValue::String(code.to_string()),
                DataValue::String(loc.version.clone()),
                DataValue::String(loc.sheet.clone()),
                DataValue::String(loc.region.clone()),
                DataValue::String(loc.org.clone()),
                DataValue::String(loc.period.clone()),
            ],
            "rpt_float_list",
        )
        .await
        .map_err(|e| api_err(&format!("读取浮动数据失败: {e}")))?;

    let sc = &ds.schema;
    let c_id = sc.col_index("id");
    let c_dim = sc.col_index("dim_key");
    let c_label = sc.col_index("label");
    let c_parent = sc.col_index("parent_dim_key");
    let c_type = sc.col_index("kind_type");
    let c_level = sc.col_index("level_no");
    let c_seq = sc.col_index("seq");
    let c_cells = sc.col_index("cells_text");
    let c_manual = sc.col_index("is_manual");
    let c_src = sc.col_index("source_tag");

    let mut items = Vec::with_capacity(ds.row_count());
    for row in &ds.rows {
        let gs = |c: Option<usize>| c.and_then(|i| row.get_str(i)).map(str::to_owned);
        let gi = |c: Option<usize>| c.and_then(|i| row.get_i64(i));
        // 整数列宽度自适应：元数据部署把 INT/SMALLINT 统一建成 BIGINT，但为兼容旧库
        // （ensure_schema 曾建 SMALLINT/INT），按 i64→i32→i16 依次尝试。
        let gint = |c: Option<usize>| {
            c.and_then(|i| {
                row.get_i64(i)
                    .or_else(|| row.get_i32(i).map(|v| v as i64))
                    .or_else(|| row.get_i16(i).map(|v| v as i64))
            })
        };
        // cells 存 JSONB，取文本后解析回对象（失败给空对象）。
        let cells: Value = gs(c_cells)
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_else(|| json!({}));
        items.push(json!({
            "id": gi(c_id),
            "dimKey": gs(c_dim),
            "label": gs(c_label),
            "parentDimKey": gs(c_parent),
            "type": gs(c_type),
            "levelNo": gint(c_level),
            "seq": gint(c_seq),
            "cells": cells,
            "isManual": gint(c_manual),
            "sourceTag": gs(c_src),
        }));
    }

    Ok(json!({
        "reportCode": code,
        "kind": if kind == FloatKind::Row { "row" } else { "col" },
        "count": items.len(),
        "items": items,
    }))
}

/// UPSERT 一条浮动行/列（按唯一键 8元+dim_key 幂等）。用于 create、update、batch、seed。
/// `is_manual`：1=手工、0=取数种子。返回该记录 id。
async fn upsert_one(
    txn_id: &str,
    code: &str,
    kind: FloatKind,
    loc: &Locator,
    item: &Value,
    is_manual: i64,
    source_tag: Option<&str>,
) -> Result<i64> {
    let (type_field, type_def) = kind.type_field();
    let id = item
        .get("id")
        .and_then(|v| v.as_i64())
        .filter(|n| *n > 0)
        .unwrap_or_else(cmx_utils::next_pk_id);
    let dim_key = s(item, "dimKey").unwrap_or_default();
    if dim_key.is_empty() {
        return Err(api_err("浮动记录缺少维度键 dimKey"));
    }
    let label = s(item, "label");
    let parent = s(item, "parentDimKey");
    let kind_type = s(item, "type").unwrap_or_else(|| type_def.to_string());
    let level_no = item.get("levelNo").and_then(|v| v.as_i64()).unwrap_or(1);
    let seq = item.get("seq").and_then(|v| v.as_i64()).unwrap_or(0);
    // cells：对象 → 紧凑 JSON 文本 → DataValue::Json（配 $N::jsonb 落 JSONB）。
    let cells_text = item
        .get("cells")
        .map(|v| v.to_string())
        .unwrap_or_else(|| "{}".to_string());

    let sql = format!(
        "INSERT INTO {tbl}
           (id, report_code, version_code, sheet_code, region_code, org_code, period_code,
            dim_key, label, parent_dim_key, {tf}, level_no, seq, cells, is_manual, source_tag,
            sort_no, status, create_time, update_time)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14::jsonb,$15,$16,$17,1,CURRENT_TIMESTAMP,CURRENT_TIMESTAMP)
         ON CONFLICT (report_code, version_code, sheet_code, region_code, org_code, period_code, dim_key)
         DO UPDATE SET label=EXCLUDED.label, parent_dim_key=EXCLUDED.parent_dim_key,
           {tf}=EXCLUDED.{tf}, level_no=EXCLUDED.level_no, seq=EXCLUDED.seq,
           cells=EXCLUDED.cells, is_manual=EXCLUDED.is_manual, source_tag=EXCLUDED.source_tag,
           sort_no=EXCLUDED.sort_no, update_time=CURRENT_TIMESTAMP",
        tbl = kind.table(),
        tf = type_field
    );
    exec_dv(
        txn_id,
        &sql,
        vec![
            DataValue::Int(id),
            DataValue::String(code.to_string()),
            DataValue::String(loc.version.clone()),
            DataValue::String(loc.sheet.clone()),
            DataValue::String(loc.region.clone()),
            DataValue::String(loc.org.clone()),
            DataValue::String(loc.period.clone()),
            DataValue::String(dim_key),
            crate::dv_str(label.as_deref()),
            crate::dv_str(parent.as_deref()),
            DataValue::String(kind_type),
            DataValue::Int(level_no),
            DataValue::Int(seq),
            DataValue::Json(cells_text),
            DataValue::Int(is_manual),
            crate::dv_str(source_tag),
            DataValue::Int(seq), // sort_no = seq
        ],
    )
    .await?;
    Ok(id)
}

/// 新增/批量保存浮动行/列（body.items[] 逐条 UPSERT，事务）。手工数据 is_manual=1。
pub async fn save_float(code: &str, kind: FloatKind, body: &Value) -> Result<Value> {
    ensure_float_schema().await?;
    let loc = make_locator(body);
    let items = body
        .get("items")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    if items.is_empty() {
        return Ok(json!({ "ok": true, "saved": 0, "ids": [] }));
    }

    let mm = get_default_pg_db_manager();
    let tx = mm.get_transaction_context();
    let txn_id = tx
        .begin(RPT_DB_ID)
        .await
        .map_err(|e| api_err(&format!("开启事务失败: {e}")))?;

    let mut ids = Vec::new();
    for item in &items {
        match upsert_one(&txn_id, code, kind, &loc, item, 1, s(item, "sourceTag").as_deref()).await
        {
            Ok(id) => ids.push(id),
            Err(e) => {
                let _ = tx.rollback(&txn_id).await;
                return Err(e);
            }
        }
    }
    tx.commit(&txn_id)
        .await
        .map_err(|e| api_err(&format!("提交事务失败: {e}")))?;
    Ok(json!({ "ok": true, "saved": ids.len(), "ids": ids }))
}

/// 删除一条浮动行/列（按 id）。返回删除条数。
pub async fn delete_float(code: &str, kind: FloatKind, id: i64) -> Result<Value> {
    ensure_float_schema().await?;
    let mm = get_default_pg_db_manager();
    let tx = mm.get_transaction_context();
    let txn_id = tx
        .begin(RPT_DB_ID)
        .await
        .map_err(|e| api_err(&format!("开启事务失败: {e}")))?;
    let sql = format!("DELETE FROM {} WHERE id=$1 AND report_code=$2", kind.table());
    let r = exec_dv(
        &txn_id,
        &sql,
        vec![DataValue::Int(id), DataValue::String(code.to_string())],
    )
    .await;
    match r {
        Ok(_) => {
            tx.commit(&txn_id)
                .await
                .map_err(|e| api_err(&format!("提交事务失败: {e}")))?;
            Ok(json!({ "ok": true, "deleted": 1, "id": id }))
        }
        Err(e) => {
            let _ = tx.rollback(&txn_id).await;
            Err(e)
        }
    }
}

/// 批量 UPSERT 种子记录（供 seed 用；is_manual=0）。默认**不覆盖已有手工行**（保护用户编辑）。
/// `overwrite_manual=true` 时才连手工行一起重置。返回写入条数。
pub(crate) async fn seed_upsert(
    code: &str,
    kind: FloatKind,
    loc: &Locator,
    items: &[Value],
    source_tag: &str,
    overwrite_manual: bool,
) -> Result<usize> {
    let mm = get_default_pg_db_manager();
    let tx = mm.get_transaction_context();
    let txn_id = tx
        .begin(RPT_DB_ID)
        .await
        .map_err(|e| api_err(&format!("开启事务失败: {e}")))?;

    // 保护手工行：预取本 8元键下 is_manual=1 的 dim_key 集合，seed 跳过它们（除非 overwrite）。
    let mut manual_keys = std::collections::HashSet::new();
    if !overwrite_manual {
        let q = format!(
            "SELECT dim_key FROM {} WHERE report_code=$1 AND version_code=$2 AND sheet_code=$3 \
               AND region_code=$4 AND org_code=$5 AND period_code=$6 AND is_manual=1",
            kind.table()
        );
        if let Ok(ds) = mm
            .query_sql_zmc_with_datavalues(
                RPT_DB_ID,
                &q,
                vec![
                    DataValue::String(code.to_string()),
                    DataValue::String(loc.version.clone()),
                    DataValue::String(loc.sheet.clone()),
                    DataValue::String(loc.region.clone()),
                    DataValue::String(loc.org.clone()),
                    DataValue::String(loc.period.clone()),
                ],
                "rpt_float_manual_keys",
            )
            .await
        {
            let ci = ds.schema.col_index("dim_key");
            for row in &ds.rows {
                if let Some(k) = ci.and_then(|i| row.get_str(i)) {
                    manual_keys.insert(k.to_string());
                }
            }
        }
    }

    let mut n = 0;
    for item in items {
        let dk = s(item, "dimKey").unwrap_or_default();
        if !overwrite_manual && manual_keys.contains(&dk) {
            continue; // 保护手工行
        }
        match upsert_one(&txn_id, code, kind, loc, item, 0, Some(source_tag)).await {
            Ok(_) => n += 1,
            Err(e) => {
                let _ = tx.rollback(&txn_id).await;
                return Err(e);
            }
        }
    }
    tx.commit(&txn_id)
        .await
        .map_err(|e| api_err(&format!("提交事务失败: {e}")))?;
    Ok(n)
}
