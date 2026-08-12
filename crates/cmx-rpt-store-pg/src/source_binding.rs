//! cmx-rpt-store-pg::source_binding —— 报表取数路由绑定（治理面）的持久化/服务层。
//!
//! 表 `cr_report_source_binding`（由 data/meta 定义部署建表，本层不建表）承载
//! 「单据类型(source_key) + 组织(org_id，NULL=默认兜底) → 物理目标(target_kind + target_ref)」
//! 的绑定条目。本模块只做**注册（CRUD）**：列出某类型的全部绑定、upsert 一条、按 id 删除。
//! 运行时三层继承解析（精确→沿组织路径继承→兜底）与 scatter-gather 取数后续接入。
//!
//! 落地范式对齐 cmx-flow 的子流程组织路由（`cmx_flow_subflow_binding` + `PgSubflowBindingStore`），
//! 但改写为 rpt store 的无状态自由函数风格：读走 `query_rows`（参数化），写走事务内 `exec_dv`
//! 强类型 DataValue 绑定。upsert 语义 = 同 `(source_key, org_id)` 视为一条（先删后插，避免同组织多条）。

use serde_json::{Value, json};

use cmx_core::dv;
use cmx_core::model::cell::{DataValue, SqlTypeMarker};
use cmx_database_pg::get_default_pg_db_manager;

use cmx_rpt_model::RPT_DB_ID;

use crate::{Result, api_err, exec_dv, query_rows};

/// 列某单据类型逻辑名（source_key）的全部绑定，兜底绑定（org_id IS NULL）排最后。
///
/// 返回投影字段：id / sourceKey / orgId / targetKind / targetRef / transport /
/// priority / enabled / remark / isDefault（org_id 为空即默认兜底）。
pub async fn list_source_bindings(source_key: &str) -> Result<Value> {
    let sql = "SELECT id, source_key, org_id, target_kind, target_ref, transport, \
                      priority, enabled, remark \
               FROM cr_report_source_binding \
               WHERE source_key = $1 \
               ORDER BY (org_id IS NULL), priority DESC, org_id";
    let rows = query_rows(sql, dv![source_key], "rpt_source_binding_list").await?;
    let items: Vec<Value> = rows.iter().map(project_binding).collect();
    Ok(json!({ "sourceKey": source_key, "bindings": items }))
}

/// upsert 一条绑定：同 `(source_key, org_id)` 视为一条（改目标/传输/优先级/启用/备注）。
/// `org_id` 为 None 表示该类型的默认兜底绑定。事务内「先删后插」保证同组织仅一条。返回绑定 id。
#[allow(clippy::too_many_arguments)]
pub async fn upsert_source_binding(
    id: &str,
    source_key: &str,
    org_id: Option<&str>,
    target_kind: &str,
    target_ref: &str,
    transport: Option<&str>,
    priority: i64,
    enabled: bool,
    remark: Option<&str>,
) -> Result<Value> {
    let mm = get_default_pg_db_manager();
    let tx = mm.get_transaction_context();
    let txn_id = tx
        .begin(RPT_DB_ID)
        .await
        .map_err(|e| api_err(&format!("开启事务失败: {e}")))?;

    let result = upsert_in_txn(
        &txn_id,
        id,
        source_key,
        org_id,
        target_kind,
        target_ref,
        transport,
        priority,
        enabled,
        remark,
    )
    .await;
    match result {
        Ok(()) => {
            tx.commit(&txn_id)
                .await
                .map_err(|e| api_err(&format!("提交事务失败: {e}")))?;
            Ok(json!({ "id": id }))
        }
        Err(e) => {
            let _ = tx.rollback(&txn_id).await;
            Err(e)
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn upsert_in_txn(
    txn_id: &str,
    id: &str,
    source_key: &str,
    org_id: Option<&str>,
    target_kind: &str,
    target_ref: &str,
    transport: Option<&str>,
    priority: i64,
    enabled: bool,
    remark: Option<&str>,
) -> Result<()> {
    // ① 先删同 (source_key, org_id) 旧绑定：org_id 为 NULL 要特判（= 兜底条目）。
    match org_id {
        Some(o) => {
            exec_dv(
                txn_id,
                "DELETE FROM cr_report_source_binding WHERE source_key = $1 AND org_id = $2",
                vec![
                    DataValue::String(source_key.to_string()),
                    DataValue::String(o.to_string()),
                ],
            )
            .await?;
        }
        None => {
            exec_dv(
                txn_id,
                "DELETE FROM cr_report_source_binding WHERE source_key = $1 AND org_id IS NULL",
                vec![DataValue::String(source_key.to_string())],
            )
            .await?;
        }
    }

    // ② 插入新绑定。可空文本列用 NullTyped(Text)（裸 Null 会被当 text 绑错列类型报错，
    //    这里列本就是 varchar 故 Text NULL 正确，与 store 的 dv_str 一致）。
    exec_dv(
        txn_id,
        "INSERT INTO cr_report_source_binding \
            (id, source_key, org_id, target_kind, target_ref, transport, priority, enabled, remark, create_time, update_time) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, now(), now())",
        vec![
            DataValue::String(id.to_string()),
            DataValue::String(source_key.to_string()),
            opt_text(org_id),
            DataValue::String(target_kind.to_string()),
            DataValue::String(target_ref.to_string()),
            opt_text(transport),
            DataValue::Int(priority),
            DataValue::Int(if enabled { 1 } else { 0 }),
            opt_text(remark),
        ],
    )
    .await?;
    Ok(())
}

/// 按 id 删除一条绑定。
pub async fn delete_source_binding(id: &str) -> Result<Value> {
    let mm = get_default_pg_db_manager();
    let tx = mm.get_transaction_context();
    let txn_id = tx
        .begin(RPT_DB_ID)
        .await
        .map_err(|e| api_err(&format!("开启事务失败: {e}")))?;

    let r = exec_dv(
        &txn_id,
        "DELETE FROM cr_report_source_binding WHERE id = $1",
        vec![DataValue::String(id.to_string())],
    )
    .await;
    match r {
        Ok(()) => {
            tx.commit(&txn_id)
                .await
                .map_err(|e| api_err(&format!("提交事务失败: {e}")))?;
            Ok(json!({ "deleted": id }))
        }
        Err(e) => {
            let _ = tx.rollback(&txn_id).await;
            Err(e)
        }
    }
}

/// 可空文本列的 DataValue：空/None → 强类型 Text NULL（对齐 store 的 `dv_str`）。
fn opt_text(s: Option<&str>) -> DataValue {
    match s {
        Some(v) if !v.trim().is_empty() => DataValue::String(v.trim().to_string()),
        _ => DataValue::NullTyped(SqlTypeMarker::Text),
    }
}

/// 把一行查询结果投影成前端友好的驼峰 JSON。`enabled` 归一为布尔，`priority` 归一为整数。
fn project_binding(r: &Value) -> Value {
    let get_str = |k: &str| r.get(k).and_then(|v| v.as_str()).map(str::to_owned);
    let org_id = get_str("org_id");
    // enabled 列是 TINYINT，query_rows 经 ZmcDataSet/JSON 可能呈现为数字或字符串，两种都归一。
    let enabled = match r.get("enabled") {
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_i64().unwrap_or(0) != 0,
        Some(Value::String(s)) => matches!(s.as_str(), "1" | "true" | "TRUE"),
        _ => false,
    };
    let priority = match r.get("priority") {
        Some(Value::Number(n)) => n.as_i64().unwrap_or(0),
        Some(Value::String(s)) => s.parse::<i64>().unwrap_or(0),
        _ => 0,
    };
    json!({
        "id": get_str("id"),
        "sourceKey": get_str("source_key"),
        "orgId": org_id,
        "targetKind": get_str("target_kind"),
        "targetRef": get_str("target_ref"),
        "transport": get_str("transport"),
        "priority": priority,
        "enabled": enabled,
        "remark": get_str("remark"),
        "isDefault": org_id.is_none(),
    })
}

/// 从 source_key + org 派生稳定绑定 id（非加密，仅去重定位用；同库同 key 碰撞面可忽略）。
/// 与 flow 的 `binding_id` 同款 FNV-1a，避免引 uuid/sha 依赖。
pub fn binding_id(source_key: &str, org: Option<&str>) -> String {
    let raw = format!("{source_key}|{}", org.unwrap_or("__default__"));
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in raw.as_bytes() {
        h ^= u64::from(*byte);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("rsb_{h:016x}")
}
