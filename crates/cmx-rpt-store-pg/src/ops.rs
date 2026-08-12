//! ops —— 报表协同编辑 B 档：操作日志应用器（方案 docs/报表协同编辑方案-B档.html §6/§7/§9）。
//!
//! `apply_ops`：一批语义操作（setCellFormula/bindElement/insertRow…）在**咨询锁串行化**下
//! 逐条应用——幂等去重(client_op_id) → 对象级冲突判定(baseSeq vs 目标 lastSeq) → 应用到投影
//! (cr_cell_element_map 按业务键增量 UPSERT，告别先删后插) → append cr_report_op_log。
//! `list_ops`：按 seq 增量拉取（打开重放 + 编辑中追平）。
//!
//! 冲突三态（§9）：不同对象都接受；同格值操作 last-writer + conflict_flag + prev_value；
//! 结构操作(insertRow 等平移地址)从严——要求 baseSeq 追平当前 seq，否则拒绝请客户端 rebase。
//! seq 串行化：pg_advisory_xact_lock(hashtext(report|version)) + MAX(seq)+1
//! （母版 create_version 的 MAX+1，锁内读写故无竞态；咨询锁跨实例生效）。

use serde_json::{Value, json};
use tracing::debug;

use cmx_core::dv;
use cmx_core::model::cell::{DataValue, SqlTypeMarker};
use cmx_database_pg::get_default_pg_db_manager;

use cmx_rpt_model::{DEFAULT_REGION, RPT_DB_ID};

use crate::{Result, api_err, exec_dv};

/// 结构操作类型：平移其它对象地址，并发从严（拒绝-rebase）。
const STRUCTURAL_OPS: &[&str] = &[
    "insertRow", "deleteRow", "insertCol", "deleteCol",
    "addRegion", "delRegion", "addSheet", "renameSheet",
];

/// 结构操作中「插/删行列」子集：平移 cr_cell_element_map 的 cell_ref（有存储副作用）。
/// 其余结构操作（addRegion/addSheet 等）仅 log-only，由客户端重放 + 快照物化收敛。
const ROWCOL_STRUCTURAL_OPS: &[&str] = &["insertRow", "deleteRow", "insertCol", "deleteCol"];

/// 写投影的操作类型：直接落 cr_cell_element_map（计算态消费的公式/绑定）。
const PROJECTION_OPS: &[&str] = &[
    "setCellFormula", "setCheckFormula", "bindElement", "unbindElement",
];

fn is_structural(op_type: &str) -> bool {
    STRUCTURAL_OPS.contains(&op_type)
}

/// 事务内 DataValue 查询（应用器的读全部在咨询锁内，须带 txn_id——crate 级 query_rows 不带）。
async fn query_rows_txn(
    txn_id: &str,
    sql: &str,
    params: Vec<DataValue>,
    label: &str,
) -> Result<Vec<Value>> {
    let mm = get_default_pg_db_manager();
    let ds = mm
        .query_sql_with_datavalues(RPT_DB_ID, Some(txn_id), sql, params, label)
        .await
        .map_err(|e| api_err(&format!("操作日志查询失败: {e}")))?;
    let v = serde_json::to_value(&ds).map_err(|e| api_err(&format!("查询结果序列化失败: {e}")))?;
    Ok(v.get("rows")
        .and_then(|r| r.as_array())
        .cloned()
        .unwrap_or_default())
}

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

/// 目标对象规范地址：值/公式类 `sheet!cell`；结构类 `row:{sheet}:{at}` 等（Op 自带则用之）。
fn canonical_target(op: &Value) -> String {
    if let Some(t) = op.get("target") {
        if let Some(s) = t.as_str() {
            return s.to_string();
        }
        // 对象形态 {sheet, cell} / {sheet, at}
        let sheet = jstr(t, "sheet").unwrap_or_default();
        if let Some(cell) = jstr(t, "cell") {
            return format!("{sheet}!{}", cell.to_ascii_uppercase());
        }
        if let Some(at) = jint(t, "at") {
            let kind = jstr(op, "type").unwrap_or_default();
            let axis = if kind.contains("Row") { "row" } else { "col" };
            return format!("{axis}:{sheet}:{at}");
        }
        if let Some(region) = jstr(t, "region") {
            return format!("region:{region}");
        }
    }
    String::new()
}

/// 应用一批操作。body: { version, ops: [ {type,target,payload,baseSeq,clientOpId} ] }。
/// actor 来自 AuthContext（api 层传入）。返回 { curSeq, results: [...] }。
pub async fn apply_ops(
    code: &str,
    body: &Value,
    actor_id: &str,
    actor_name: &str,
) -> Result<Value> {
    let version = jstr(body, "version").unwrap_or_default(); // ''=默认版本，与 layout 端点一致
    let ops = body
        .get("ops")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    if ops.is_empty() {
        return Err(api_err("ops 不能为空"));
    }
    debug!(
        "{:<12} - apply_ops {code}/{version} n={} actor={actor_id}",
        "RPT-OPS",
        ops.len()
    );

    let mm = get_default_pg_db_manager();
    let tx = mm.get_transaction_context();
    let txn_id = tx
        .begin(RPT_DB_ID)
        .await
        .map_err(|e| api_err(&format!("开启事务失败: {e}")))?;

    let result = apply_ops_in_txn(&txn_id, code, &version, &ops, actor_id, actor_name).await;
    match result {
        Ok(v) => {
            tx.commit(&txn_id)
                .await
                .map_err(|e| api_err(&format!("提交事务失败: {e}")))?;
            Ok(v)
        }
        Err(e) => {
            let _ = tx.rollback(&txn_id).await;
            Err(e)
        }
    }
}

async fn apply_ops_in_txn(
    txn_id: &str,
    code: &str,
    version: &str,
    ops: &[Value],
    actor_id: &str,
    actor_name: &str,
) -> Result<Value> {
    // ── 串行化闸门：同报表版本跨实例互斥（事务结束自动释放）。
    //    pg_advisory_xact_lock 返回 void，executor 不识别 void 列——包一层子查询返常量 1。
    query_rows_txn(
        txn_id,
        "SELECT 1 AS locked FROM (SELECT pg_advisory_xact_lock(hashtext($1))) t",
        dv![format!("rpt-ops|{code}|{version}")],
        "rpt_ops_lock",
    )
    .await?;

    // 当前 seq（锁内读，MAX+1 无竞态；母版 create_version）
    let mut cur_seq = query_rows_txn(
        txn_id,
        "SELECT COALESCE(MAX(seq),0) AS max_seq FROM cr_report_op_log WHERE report_code=$1 AND version_code=$2",
        dv![code, version],
        "rpt_ops_seq",
    )
    .await?
    .first()
    .and_then(|r| jint(r, "max_seq"))
    .unwrap_or(0);

    let mut results: Vec<Value> = Vec::with_capacity(ops.len());
    for op in ops {
        let op_type = jstr(op, "type").unwrap_or_default();
        if op_type.is_empty() {
            results.push(json!({ "applied": false, "rejected": true, "reason": "缺少 type" }));
            continue;
        }
        let client_op_id = jstr(op, "clientOpId").unwrap_or_default();
        let base_seq = jint(op, "baseSeq").unwrap_or(0);
        let target = canonical_target(op);
        let payload = op.get("payload").cloned().unwrap_or(Value::Null);

        // ── 幂等：client_op_id 已应用过 → 返回既有 seq，不重复应用 ──
        if !client_op_id.is_empty() {
            let dup = query_rows_txn(
                txn_id,
                "SELECT seq FROM cr_report_op_log WHERE report_code=$1 AND version_code=$2 AND client_op_id=$3",
                dv![code, version, client_op_id.clone()],
                "rpt_ops_dup",
            )
            .await?;
            if let Some(seq) = dup.first().and_then(|r| jint(r, "seq")) {
                results.push(json!({
                    "clientOpId": client_op_id, "seq": seq,
                    "applied": false, "duplicate": true,
                }));
                continue;
            }
        }

        // ── 冲突判定（§9 三态） ──
        let mut conflict_flag = 0i64;
        let mut prev_value: Option<String> = None;
        if is_structural(&op_type) {
            // 结构操作从严：必须基于最新 seq 提交，否则拒绝请客户端追平后重发
            if base_seq < cur_seq {
                results.push(json!({
                    "clientOpId": client_op_id, "applied": false, "rejected": true,
                    "reason": format!("结构操作需基于最新状态：baseSeq={base_seq} < curSeq={cur_seq}，请追平(GET ops?since={base_seq})后重发"),
                    "rebaseFrom": base_seq, "curSeq": cur_seq,
                }));
                continue;
            }
        } else if !target.is_empty() {
            // 值/公式操作：只与同目标的后发操作冲突（last-writer + 标记）
            let last = query_rows_txn(
                txn_id,
                r#"SELECT seq, payload, actor_name FROM cr_report_op_log
                   WHERE report_code=$1 AND version_code=$2 AND target=$3
                   ORDER BY seq DESC LIMIT 1"#,
                dv![code, version, target.clone()],
                "rpt_ops_last_target",
            )
            .await?;
            if let Some(row) = last.first()
                && let Some(last_seq) = jint(row, "seq")
                && last_seq > base_seq
            {
                conflict_flag = 1;
                let who = jstr(row, "actor_name").unwrap_or_default();
                prev_value = jstr(row, "payload").map(|p| {
                    json!({ "seq": last_seq, "actor": who, "payload": p }).to_string()
                });
            }
        }

        // ── 应用到投影（公式/绑定类；其余 log-only 由客户端重放 + 快照物化收敛） ──
        if PROJECTION_OPS.contains(&op_type.as_str()) {
            apply_projection_op(txn_id, code, version, &op_type, op, &payload).await?;
        } else if ROWCOL_STRUCTURAL_OPS.contains(&op_type.as_str()) {
            // 插/删行列：平移 cr_cell_element_map 的 cell_ref（被删条目坍缩删除）
            apply_structural_op(txn_id, code, version, &op_type, op).await?;
        }

        // ── append 日志 ──
        cur_seq += 1;
        let summary = format!("{op_type} {target}");
        exec_dv(
            txn_id,
            r#"INSERT INTO cr_report_op_log
               (id, name, report_code, version_code, seq, op_type, target, payload,
                base_seq, client_op_id, conflict_flag, prev_value, actor_id, actor_name,
                sort_no, status, create_time)
               VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,0,1,CURRENT_TIMESTAMP)"#,
            vec![
                DataValue::Int(cmx_utils::next_pk_id()),
                DataValue::String(summary.chars().take(120).collect()),
                DataValue::String(code.to_string()),
                DataValue::String(version.to_string()),
                DataValue::Int(cur_seq),
                DataValue::String(op_type.clone()),
                DataValue::String(target.clone()),
                DataValue::String(payload.to_string()),
                DataValue::Int(base_seq),
                match client_op_id.as_str() {
                    "" => DataValue::NullTyped(SqlTypeMarker::Text),
                    x => DataValue::String(x.to_string()),
                },
                DataValue::Int(conflict_flag),
                match &prev_value {
                    Some(p) => DataValue::String(p.clone()),
                    None => DataValue::NullTyped(SqlTypeMarker::Text),
                },
                DataValue::String(actor_id.to_string()),
                DataValue::String(actor_name.to_string()),
            ],
        )
        .await?;

        results.push(json!({
            "clientOpId": client_op_id, "seq": cur_seq, "applied": true,
            "conflict": conflict_flag == 1,
            "conflictPrev": prev_value,
        }));
    }

    Ok(json!({
        "reportCode": code, "version": version,
        "curSeq": cur_seq, "results": results,
    }))
}

/// A1 引用 → (row_id, col_id) 稳定网格锚点（行号 1 基、列号 1 基）。
/// 唯一键含 row_id/col_id——若都填 0 第二个新格子即撞键；用画布网格位置做锚点
/// （与 report-applier 存数的 rowId=r+1/colId=c+1 同一约定），layout 保存时会被真行列 id 覆盖。
fn cell_ref_anchor(cell: &str) -> (i64, i64) {
    let bytes = cell.as_bytes();
    let mut i = 0;
    let mut col: i64 = 0;
    while i < bytes.len() && bytes[i].is_ascii_alphabetic() {
        col = col * 26 + ((bytes[i].to_ascii_uppercase() - b'A') as i64 + 1);
        i += 1;
    }
    let row: i64 = cell[i..].parse().unwrap_or(0);
    (row, col)
}

/// A1 → (row0, col0) 0 基索引；非法返回 None。
fn parse_a1(cell: &str) -> Option<(i64, i64)> {
    let bytes = cell.as_bytes();
    let mut i = 0;
    let mut col: i64 = 0;
    while i < bytes.len() && bytes[i].is_ascii_alphabetic() {
        col = col * 26 + ((bytes[i].to_ascii_uppercase() - b'A') as i64 + 1);
        i += 1;
    }
    if i == 0 {
        return None;
    }
    let row: i64 = cell[i..].parse().ok()?;
    if row < 1 {
        return None;
    }
    Some((row - 1, col - 1))
}

/// 0 基列索引 → A1 列字母。
fn col_to_letters(col0: i64) -> String {
    let mut n = col0 + 1;
    let mut s = String::new();
    while n > 0 {
        let r = ((n - 1) % 26) as u8;
        s.insert(0, (b'A' + r) as char);
        n = (n - 1) / 26;
    }
    if s.is_empty() {
        s.push('A');
    }
    s
}

/// 单个 0 基索引在插入/删除后的新值；被删区间内返回 None（映射坍缩）。
/// 与前端 designer.js `structShiftIndex` 及引擎 refTransform 同语义。
fn struct_shift_index(pos: i64, index: i64, count: i64, is_insert: bool) -> Option<i64> {
    if is_insert {
        return Some(if pos >= index { pos + count } else { pos });
    }
    // delete
    if pos >= index && pos < index + count {
        return None;
    }
    if pos >= index + count {
        return Some(pos - count);
    }
    Some(pos)
}

/// A1 地址按结构编辑移位；被删返回 None。axis='row' 移行、否则移列。
fn shift_cell_ref(cell: &str, axis_is_row: bool, index: i64, count: i64, is_insert: bool) -> Option<String> {
    let (row0, col0) = parse_a1(cell)?;
    if axis_is_row {
        let nr = struct_shift_index(row0, index, count, is_insert)?;
        Some(format!("{}{}", col_to_letters(col0), nr + 1))
    } else {
        let nc = struct_shift_index(col0, index, count, is_insert)?;
        Some(format!("{}{}", col_to_letters(nc), row0 + 1))
    }
}

/// 结构操作（插/删行列）→ 平移 cr_cell_element_map 的 cell_ref/row_id/col_id，
/// 被删行列上的条目删除（映射坍缩）。target={sheet, at}，payload={count}。
///
/// 只碰 cr_cell_element_map（设计态取数/校验公式、元素绑定投影）；cr_report_row/col 的
/// row_no/col_no 及 SSJSON 版式由客户端下次 save_layout 全量重建收敛（对齐「log 同步语义层 +
/// 快照物化版式」约定），故本函数不碰它们。cr_cell_data 是运行态(org+period)数据，另属。
async fn apply_structural_op(
    txn_id: &str,
    code: &str,
    version: &str,
    op_type: &str,
    op: &Value,
) -> Result<()> {
    let t = op.get("target").cloned().unwrap_or(Value::Null);
    let sheet = jstr(&t, "sheet")
        .or_else(|| {
            // 串形态 `row:{sheet}:{at}`
            jstr(op, "target").and_then(|s| s.split(':').nth(1).map(|x| x.to_string()))
        })
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "Sheet1".to_string());
    let at = jint(&t, "at")
        .or_else(|| jstr(op, "target").and_then(|s| s.split(':').nth(2).and_then(|x| x.parse().ok())))
        .ok_or_else(|| api_err(&format!("{op_type} 缺少目标索引 at")))?;
    let payload = op.get("payload").cloned().unwrap_or(Value::Null);
    let count = jint(&payload, "count").unwrap_or(1).max(1);
    let axis_is_row = op_type.contains("Row");
    let is_insert = op_type.starts_with("insert");

    // 读该 sheet 所有映射条目（含 cell_ref）
    let rows = query_rows_txn(
        txn_id,
        r#"SELECT id, cell_ref FROM cr_cell_element_map
           WHERE report_code=$1 AND version_code=$2 AND sheet_code=$3"#,
        dv![code, version, sheet.clone()],
        "rpt_struct_map_scan",
    )
    .await?;

    for r in &rows {
        let id = match jint(r, "id") {
            Some(v) => v,
            None => continue,
        };
        let cell = match jstr(r, "cell_ref") {
            Some(c) => c,
            None => continue,
        };
        match shift_cell_ref(&cell, axis_is_row, at, count, is_insert) {
            None => {
                // 落被删区间 → 删除该映射条目
                exec_dv(
                    txn_id,
                    "DELETE FROM cr_cell_element_map WHERE id=$1",
                    vec![DataValue::Int(id)],
                )
                .await?;
            }
            Some(new_ref) if new_ref != cell => {
                let (arow, acol) = cell_ref_anchor(&new_ref);
                exec_dv(
                    txn_id,
                    r#"UPDATE cr_cell_element_map
                       SET cell_ref=$1, row_id=$2, col_id=$3, update_time=CURRENT_TIMESTAMP
                       WHERE id=$4"#,
                    vec![
                        DataValue::String(new_ref),
                        DataValue::Int(arow),
                        DataValue::Int(acol),
                        DataValue::Int(id),
                    ],
                )
                .await?;
            }
            Some(_) => { /* 未移动（在插删点之前）→ 不动 */ }
        }
    }
    Ok(())
}

/// 公式/绑定类操作 → cr_cell_element_map 增量 UPSERT（按 sheet|region|cell_ref 业务键，
/// 复用既有行 id——B1 稳定 id 的增量版；锁内 SELECT→UPDATE/INSERT 无竞态）。
async fn apply_projection_op(
    txn_id: &str,
    code: &str,
    version: &str,
    op_type: &str,
    op: &Value,
    payload: &Value,
) -> Result<()> {
    let t = op.get("target").cloned().unwrap_or(Value::Null);
    let sheet = jstr(&t, "sheet").unwrap_or_else(|| "Sheet1".to_string());
    let region = jstr(&t, "region")
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| DEFAULT_REGION.to_string());
    let cell = jstr(&t, "cell")
        .map(|c| c.to_ascii_uppercase())
        .or_else(|| {
            // target 为串形态 "Sheet1!C5"
            jstr(op, "target").and_then(|s| {
                s.split_once('!').map(|(_, c)| c.to_ascii_uppercase())
            })
        })
        .filter(|s| !s.is_empty())
        .ok_or_else(|| api_err(&format!("{op_type} 缺少目标单元格")))?;

    // 既有行?
    let existing = query_rows_txn(
        txn_id,
        r#"SELECT id FROM cr_cell_element_map
           WHERE report_code=$1 AND version_code=$2 AND sheet_code=$3 AND region_code=$4 AND cell_ref=$5"#,
        dv![code, version, sheet.clone(), region.clone(), cell.clone()],
        "rpt_ops_cell_lookup",
    )
    .await?;
    let existing_id = existing.first().and_then(|r| jint(r, "id"));

    // 列名 + 新值（unbindElement 置 NULL）
    let (col, val): (&str, DataValue) = match op_type {
        "setCellFormula" => (
            "calc_formula",
            match jstr(payload, "formula") {
                Some(f) if !f.trim().is_empty() => DataValue::String(f),
                _ => DataValue::NullTyped(SqlTypeMarker::Text),
            },
        ),
        "setCheckFormula" => (
            "check_formula",
            match jstr(payload, "formula").or_else(|| jstr(payload, "checkFormula")) {
                Some(f) if !f.trim().is_empty() => DataValue::String(f),
                _ => DataValue::NullTyped(SqlTypeMarker::Text),
            },
        ),
        "bindElement" => (
            "element_code",
            match jstr(payload, "elementCode") {
                Some(e) if !e.is_empty() => DataValue::String(e),
                _ => DataValue::NullTyped(SqlTypeMarker::Text),
            },
        ),
        "unbindElement" => ("element_code", DataValue::NullTyped(SqlTypeMarker::Text)),
        _ => return Ok(()),
    };

    match existing_id {
        Some(id) => {
            exec_dv(
                txn_id,
                &format!(
                    "UPDATE cr_cell_element_map SET {col}=$1, update_time=CURRENT_TIMESTAMP WHERE id=$2"
                ),
                vec![val, DataValue::Int(id)],
            )
            .await?;
        }
        None => {
            let (anchor_row, anchor_col) = cell_ref_anchor(&cell);
            exec_dv(
                txn_id,
                &format!(
                    r#"INSERT INTO cr_cell_element_map
                       (id, code, report_code, version_code, sheet_code, region_code, row_id, col_id,
                        cell_ref, {col}, is_editable, sort_no, status, create_time, update_time)
                       VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,1,0,1,CURRENT_TIMESTAMP,CURRENT_TIMESTAMP)"#
                ),
                vec![
                    DataValue::Int(cmx_utils::next_pk_id()),
                    DataValue::String(cell.clone()),
                    DataValue::String(code.to_string()),
                    DataValue::String(version.to_string()),
                    DataValue::String(sheet),
                    DataValue::String(region),
                    DataValue::Int(anchor_row),
                    DataValue::Int(anchor_col),
                    DataValue::String(cell),
                    val,
                ],
            )
            .await?;
        }
    }
    Ok(())
}

/// 增量拉取：seq > since 的操作（打开重放 + 追平轮询）。返回 { curSeq, ops }。
pub async fn list_ops(code: &str, version: &str, since: i64, limit: i64) -> Result<Value> {
    let limit = limit.clamp(1, 1000);
    let rows = crate::query_rows(
        r#"SELECT seq, op_type, target, payload, base_seq, client_op_id, conflict_flag,
                  actor_id, actor_name, create_time
           FROM cr_report_op_log
           WHERE report_code=$1 AND version_code=$2 AND seq > $3
           ORDER BY seq
           LIMIT $4"#,
        dv![code, version, since, limit],
        "rpt_ops_list",
    )
    .await?;
    let cur_seq = crate::query_rows(
        "SELECT COALESCE(MAX(seq),0) AS max_seq FROM cr_report_op_log WHERE report_code=$1 AND version_code=$2",
        dv![code, version],
        "rpt_ops_cur",
    )
    .await?
    .first()
    .and_then(|r| jint(r, "max_seq"))
    .unwrap_or(0);

    let ops: Vec<Value> = rows
        .iter()
        .map(|r| {
            json!({
                "seq": jint(r, "seq"),
                "type": jstr(r, "op_type"),
                "target": jstr(r, "target"),
                "payload": jstr(r, "payload")
                    .and_then(|p| serde_json::from_str::<Value>(&p).ok())
                    .unwrap_or(Value::Null),
                "baseSeq": jint(r, "base_seq"),
                "clientOpId": jstr(r, "client_op_id"),
                "conflict": jint(r, "conflict_flag").unwrap_or(0) == 1,
                "actorId": jstr(r, "actor_id"),
                "actorName": jstr(r, "actor_name"),
                "createdAt": jstr(r, "create_time"),
            })
        })
        .collect();

    Ok(json!({
        "reportCode": code, "version": version,
        "curSeq": cur_seq, "since": since, "count": ops.len(), "ops": ops,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_a1_roundtrip() {
        assert_eq!(parse_a1("A1"), Some((0, 0)));
        assert_eq!(parse_a1("C5"), Some((4, 2)));
        assert_eq!(parse_a1("AA10"), Some((9, 26)));
        assert_eq!(parse_a1("bad"), None);
        assert_eq!(parse_a1("5"), None);
    }

    #[test]
    fn col_letters_roundtrip() {
        assert_eq!(col_to_letters(0), "A");
        assert_eq!(col_to_letters(2), "C");
        assert_eq!(col_to_letters(26), "AA");
        assert_eq!(col_to_letters(27), "AB");
    }

    #[test]
    fn shift_index_insert() {
        // 插入点前不动，插入点及之后 +count
        assert_eq!(struct_shift_index(1, 2, 1, true), Some(1));
        assert_eq!(struct_shift_index(2, 2, 1, true), Some(3));
        assert_eq!(struct_shift_index(4, 2, 2, true), Some(6));
    }

    #[test]
    fn shift_index_delete() {
        // 区间内坍缩 None；之后 -count；之前不动
        assert_eq!(struct_shift_index(1, 2, 1, false), Some(1));
        assert_eq!(struct_shift_index(2, 2, 1, false), None); // 落被删行
        assert_eq!(struct_shift_index(4, 2, 1, false), Some(3));
        assert_eq!(struct_shift_index(3, 2, 2, false), None); // [2,4) 内
        assert_eq!(struct_shift_index(4, 2, 2, false), Some(2));
    }

    #[test]
    fn shift_cell_ref_row() {
        // C5 上方(index=2)插1行 → C6
        assert_eq!(shift_cell_ref("C5", true, 2, 1, true).as_deref(), Some("C6"));
        // C5 下方(index=10)插 → 不动
        assert_eq!(shift_cell_ref("C5", true, 10, 1, true).as_deref(), Some("C5"));
        // 删第3行(index=2) → C5→C4
        assert_eq!(shift_cell_ref("C5", true, 2, 1, false).as_deref(), Some("C4"));
        // 删含 C5 的行(index=4) → 坍缩
        assert_eq!(shift_cell_ref("C5", true, 4, 1, false), None);
    }

    #[test]
    fn shift_cell_ref_col() {
        // C5 左侧(index=1)插1列 → D5
        assert_eq!(shift_cell_ref("C5", false, 1, 1, true).as_deref(), Some("D5"));
        // 删列 C(index=2) → 坍缩
        assert_eq!(shift_cell_ref("C5", false, 2, 1, false), None);
        // 删列 B(index=1) → C5→B5
        assert_eq!(shift_cell_ref("C5", false, 1, 1, false).as_deref(), Some("B5"));
        // 多字母列 AA10 在 index=0 插1列 → AB10
        assert_eq!(shift_cell_ref("AA10", false, 0, 1, true).as_deref(), Some("AB10"));
    }

    #[test]
    fn cell_ref_anchor_matches_new_ref() {
        // 移位后 row_id/col_id 锚点须与新 cell_ref 一致（1 基）
        let (r, c) = cell_ref_anchor("D6");
        assert_eq!((r, c), (6, 4));
    }
}
