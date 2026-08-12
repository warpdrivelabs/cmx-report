//! cmx-rpt-store-pg —— 报表模块的 PostgreSQL 持久化/服务层。
//!
//! 承载全部 cr_* 报表数据字典物理表（fico-db）的读写：报表主档/版本/类别/元素/会计日历/
//! 合并组织的查询，版式 BLOB(cr_report_fmt) + 关系投影(sheet/region/row/col/cell_element_map)
//! 的事务重建，单元格数据(cr_cell_data) 按 org+period 的批量 UPSERT。读走 ZmcDataSet 零拷贝
//! 省内存，写走强类型 DataValue 绑定。表由 data/meta 定义建，本层不建表（无 DDL）。
//!
//! 服务函数返回 `serde_json::Value` 载荷或语义结果（如 `SaveLayoutOutcome`），由 cmx-rpt-api
//! 的薄 handler 包装成 HTTP 响应。错误统一走 `cmx_api_types::Error`（经 `api_err`/BizError 桥接）。

use std::collections::HashMap;

use serde_json::{Value, json};
use tracing::{debug, error};

use cmx_core::dv;
use cmx_core::model::cell::{DataValue, SqlTypeMarker};
use cmx_database_pg::{ZmcRowSource, get_default_pg_db_manager};

use cmx_rpt_model::{CreateVersionBody, DEFAULT_REGION, LayoutQuery, RPT_DB_ID, ReportListQuery};

pub use cmx_api_types::{Error, Result};

pub mod compute;
pub use compute::compute_report_service;
pub mod expand;
pub mod float_ddl;
pub mod float_crud;
pub mod ops;
pub use ops::{apply_ops, list_ops};
pub mod source_binding;
pub use source_binding::{
    binding_id, delete_source_binding, list_source_bindings, upsert_source_binding,
};
pub mod rpt_job;
pub use rpt_job::{KIND_RPT_COMPUTE, KIND_RPT_VERIFY, RptComputeJob, RptVerifyJob};

// 公共错误助手重导出（api_err 对外暴露，向后兼容本 crate 调用点零改动）。
pub use cmx_biz::api_err;

/// 把 DB 执行错误翻译成优雅业务错误（PG 明细 → `CmxErrCode` 中文 + 稳定码），
/// 绝不把 PG 英文原文/SQL 暴露给前端。
fn db_err(e: cmx_database_pg::Error) -> Error {
    cmx_biz::BizError::from_db_error(&cmx_database_pg::pg_detail(&e)).into()
}

// ============================================================================
// DB 门面 helper
// ============================================================================

/// DataValue 参数查询 → 行数组（报表读取多为小体量关系投影）。
pub(crate) async fn query_rows(sql: &str, params: Vec<DataValue>, label: &str) -> Result<Vec<Value>> {
    let mm = get_default_pg_db_manager();
    let ds = mm
        .query_sql_with_datavalues(RPT_DB_ID, None, sql, params, label)
        .await
        .map_err(|e| {
            // 日志侧：结构化记录完整 PG 明细（SQL / label / SQLSTATE 文案 / DETAIL / 约束名），
            // 解决 tokio-postgres 顶层 Display 恒为无信息 "db error" 导致的排障盲区。
            // 响应侧：走 `db_err` 翻译为稳定中文（对齐 DOC saver / `execute` 约定，不暴露 PG 原文）。
            error!(
                target: "rpt::store::query",
                rpt_db_id = RPT_DB_ID,
                query_label = label,
                query_sql = sql,
                pg_detail = %cmx_database_pg::pg_detail(&e),
                "报表设计数据查询失败"
            );
            db_err(e)
        })?;
    let v = serde_json::to_value(&ds).map_err(|e| api_err(&format!("查询结果序列化失败: {e}")))?;
    Ok(v.get("rows")
        .and_then(|r| r.as_array())
        .cloned()
        .unwrap_or_default())
}

/// DataValue 参数执行（非事务，单语句）。
async fn execute(sql: &str, params: Vec<DataValue>) -> Result<()> {
    let mm = get_default_pg_db_manager();
    mm.execute_sql_with_datavalues(RPT_DB_ID, None, sql, params)
        .await
        // 落库失败：翻译成优雅提示 + 稳定错误码，不暴露 PG 英文原文（对齐 DOC saver）。
        .map_err(db_err)?;
    Ok(())
}

/// 强类型执行（事务内），参数按 DataValue 变体精确绑定（String→text, Binary→bytea, Null→NULL…）。
pub(crate) async fn exec_dv(txn_id: &str, sql: &str, params: Vec<DataValue>) -> Result<()> {
    let mm = get_default_pg_db_manager();
    mm.execute_sql_with_datavalues(RPT_DB_ID, Some(txn_id), sql, params)
        .await
        // 落库失败：把 PG 原始错误翻译成优雅提示 + 稳定错误码（唯一键/外键/非空等），
        // 不再暴露英文原文 + SQL（对齐 DOC saver 机制）。
        .map_err(db_err)?;
    Ok(())
}

// ============================================================================
// JSON 字段/DataValue 绑定 helper
// ============================================================================

fn str_field(body: &Value, name: &str) -> Option<String> {
    body.get(name)
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned)
}

fn int_field(body: &Value, name: &str) -> Option<i64> {
    body.get(name).and_then(|v| v.as_i64())
}

fn bool_int_field(body: &Value, name: &str) -> Option<i64> {
    body.get(name).and_then(|v| match v {
        Value::Bool(b) => Some(if *b { 1 } else { 0 }),
        Value::Number(n) => n.as_i64(),
        Value::String(s) => match s.as_str() {
            "1" | "true" | "TRUE" | "yes" | "YES" => Some(1),
            "0" | "false" | "FALSE" | "no" | "NO" => Some(0),
            _ => None,
        },
        _ => None,
    })
}

fn dv_str(s: Option<&str>) -> DataValue {
    match s {
        Some(v) if !v.is_empty() => DataValue::String(v.to_string()),
        // 强类型 Text NULL：tokio-postgres 按列 OID 校验 NULL 类型（裸 Null 会当 text 绑到
        // 非 text 列报错；这里列本就是 varchar 故 Text NULL 正确）。
        _ => DataValue::NullTyped(SqlTypeMarker::Text),
    }
}

/// NOT NULL 文本列：缺失时用给定默认值（避免 null 约束冲突）。
fn dv_str_def(s: Option<&str>, def: &str) -> DataValue {
    match s {
        Some(v) if !v.is_empty() => DataValue::String(v.to_string()),
        _ => DataValue::String(def.to_string()),
    }
}

fn dv_i64(v: Option<i64>) -> DataValue {
    // 强类型 Int NULL：宽度自适应 INT2/4/8，绑到 bigint 列的 NULL 正确（裸 Null=text 会报错）。
    v.map(DataValue::Int)
        .unwrap_or(DataValue::NullTyped(SqlTypeMarker::Int))
}

/// NOT NULL 整数列：缺失时用给定默认值。
fn dv_i64_def(v: Option<i64>, def: i64) -> DataValue {
    DataValue::Int(v.unwrap_or(def))
}

fn s(body: &Value, k: &str) -> Option<String> {
    body.get(k).and_then(|v| v.as_str()).map(str::to_owned)
}

fn i(body: &Value, k: &str) -> Option<i64> {
    body.get(k).and_then(|v| match v {
        Value::Number(n) => n.as_i64(),
        Value::Bool(b) => Some(if *b { 1 } else { 0 }),
        _ => None,
    })
}

fn arr<'a>(body: &'a Value, k: &str) -> &'a [Value] {
    body.get(k)
        .and_then(|v| v.as_array())
        .map(|a| a.as_slice())
        .unwrap_or(&[])
}

/// 0基列号 → 字母（0→A, 26→AA），列 code 缺失时的兜底。
fn col_letter_of(idx: usize) -> String {
    let mut n = idx + 1;
    let mut s = String::new();
    while n > 0 {
        let r = (n - 1) % 26;
        s.insert(0, (b'A' + r as u8) as char);
        n = (n - 1) / 26;
    }
    if s.is_empty() { "A".to_string() } else { s }
}

/// B1 稳定 id：解析行列/单元格 id，按业务键复用既有真号，避免先删后插每次重铸切断外部引用。
///
/// 优先级：① 前端回传真实数字 id → 原样用；② 业务键命中既有(preload) → 复用旧 id
/// （前端总回传临时 `t:...` 串，故此路是常态）；③ 全新对象 → 铸新 next_pk_id。
fn resolve_or_reuse(v: Option<&Value>, reuse: &HashMap<String, i64>, bkey: &str) -> i64 {
    match v {
        Some(Value::Number(n)) => {
            if let Some(x) = n.as_i64() {
                return x;
            }
        }
        Some(Value::String(s)) if !s.is_empty() && s.chars().all(|c| c.is_ascii_digit()) => {
            if let Ok(x) = s.parse::<i64>() {
                return x;
            }
        }
        _ => {}
    }
    if !bkey.is_empty()
        && let Some(&id) = reuse.get(bkey)
    {
        return id;
    }
    cmx_utils::next_pk_id()
}

/// 预载既有「业务键 → 真实 id」映射（B1）：先删后插前读出旧 id，重插时复用。
/// 业务键 = `sheet_code|region_code|{key_col}`（key_col 对行=code、列=code、单元格=cell_ref）。
/// 在事务 DELETE 之前调用（走独立连接读已提交的旧态，不受本事务未提交删除影响）。
async fn preload_id_map(
    code: &str,
    version: &str,
    table: &str,
    key_col: &str,
) -> Result<HashMap<String, i64>> {
    let sql = format!(
        "SELECT sheet_code, region_code, {key_col} AS bkey, id \
         FROM {table} WHERE report_code=$1 AND version_code=$2"
    );
    let rows = query_rows(&sql, dv![code, version], "rpt_preload_ids").await?;
    let mut m = HashMap::new();
    for r in &rows {
        let sheet = r.get("sheet_code").and_then(|v| v.as_str()).unwrap_or("");
        let region = r.get("region_code").and_then(|v| v.as_str()).unwrap_or("");
        let bkey = r.get("bkey").and_then(|v| v.as_str()).unwrap_or("");
        if bkey.is_empty() {
            continue;
        }
        if let Some(id) = r.get("id").and_then(|v| v.as_i64()) {
            m.insert(format!("{sheet}|{region}|{bkey}"), id);
        }
    }
    Ok(m)
}

// ============================================================================
// 报表主档 / 版本 / 类别 / 元素 / 日历 / 组织：读服务
// ============================================================================

/// 报表设计工作台总览：类别 + 期间类型 + 全量报表。
pub async fn overview() -> Result<Value> {
    debug!("{:<12} - overview db={}", "RPT-STORE", RPT_DB_ID);
    let categories = query_rows(
        r#"SELECT code, name, sort_no, status, remark
           FROM cr_report_category
           WHERE COALESCE(status, 1) = 1
           ORDER BY COALESCE(sort_no, 999999), code"#,
        dv![],
        "report_design_categories",
    )
    .await?;
    let periods = query_rows(
        r#"SELECT code, name, sort_no, status, remark
           FROM cr_period_type
           WHERE COALESCE(status, 1) = 1
           ORDER BY COALESCE(sort_no, 999999), code"#,
        dv![],
        "report_design_periods",
    )
    .await?;
    let reports = report_rows(&ReportListQuery::default()).await?;
    Ok(json!({
        "dbId": RPT_DB_ID,
        "categories": categories,
        "periods": periods,
        "reports": reports,
    }))
}

/// 过滤报表列表（供 overview + /reports 复用）。
pub async fn report_rows(q: &ReportListQuery) -> Result<Vec<Value>> {
    let mut wheres = Vec::new();
    let mut params: Vec<DataValue> = Vec::new();
    let mut n = 0usize;
    if let Some(category) = q
        .category
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        n += 1;
        wheres.push(format!("rl.report_category = ${n}"));
        params.push(DataValue::String(category.to_string()));
    }
    if let Some(period_type) = q
        .period_type
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        n += 1;
        wheres.push(format!("rl.period_type = ${n}"));
        params.push(DataValue::String(period_type.to_string()));
    }
    if let Some(kw) = q.q.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        n += 1;
        wheres.push(format!(
            "(rl.code ILIKE ${n} OR rl.name ILIKE ${n} OR COALESCE(rl.remark, '') ILIKE ${n})"
        ));
        params.push(DataValue::String(format!("%{kw}%")));
    }
    let where_sql = if wheres.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", wheres.join(" AND "))
    };
    let sql = format!(
        r#"SELECT
             rl.code, rl.name, rl.report_type, rl.report_category, rl.format_code,
             rl.period_type, rl.currency_code, rl.amount_unit, rl.entity_scope,
             rl.template_version, rl.data_source, rl.is_statutory, rl.remark,
             rl.sort_no, rl.status, rl.create_time, rl.update_time,
             COALESCE((SELECT COUNT(*) FROM cr_report_version rv WHERE rv.report_code = rl.code), 0) AS version_count,
             (SELECT rv.code FROM cr_report_version rv WHERE rv.report_code = rl.code AND COALESCE(rv.is_current, 0) = 1 ORDER BY rv.version_no DESC, rv.code DESC LIMIT 1) AS current_version_code,
             COALESCE((
               SELECT jsonb_agg(jsonb_build_object(
                 'code', rv.code,
                 'name', rv.name,
                 'version_no', rv.version_no,
                 'version_status', rv.version_status,
                 'is_current', rv.is_current,
                 'base_version_code', rv.base_version_code,
                 'effective_from', rv.effective_from,
                 'effective_to', rv.effective_to,
                 'change_summary', rv.change_summary,
                 'publish_time', rv.publish_time,
                 'remark', rv.remark
               ) ORDER BY rv.version_no DESC, rv.code DESC)
               FROM cr_report_version rv
               WHERE rv.report_code = rl.code
             ), '[]'::jsonb)::text AS versions
           FROM cr_report_list rl
           {where_sql}
           ORDER BY COALESCE(rl.sort_no, 999999), rl.code"#
    );
    query_rows(&sql, params, "report_design_reports").await
}

/// 报表列表（含 dbId 信封）。
pub async fn reports(q: &ReportListQuery) -> Result<Value> {
    let reports = report_rows(q).await?;
    Ok(json!({ "dbId": RPT_DB_ID, "items": reports }))
}

/// 数据元素：元素分类 + 数据元素。
pub async fn elements() -> Result<Value> {
    debug!("{:<12} - elements db={}", "RPT-STORE", RPT_DB_ID);
    let categories = query_rows(
        r#"SELECT code, name, sort_no, status, remark
           FROM cr_element_category
           WHERE COALESCE(status, 1) = 1
           ORDER BY COALESCE(sort_no, 999999), code"#,
        dv![],
        "report_design_element_categories",
    )
    .await?;
    let elements = query_rows(
        r#"SELECT code, name, category_code, data_type, unit, decimals,
                  value_source, formula_type, calc_formula, check_formula,
                  sort_no, status, remark
           FROM cr_data_element
           WHERE COALESCE(status, 1) = 1
           ORDER BY COALESCE(sort_no, 999999), code"#,
        dv![],
        "report_design_data_elements",
    )
    .await?;
    Ok(json!({
        "dbId": RPT_DB_ID,
        "categories": categories,
        "elements": elements,
    }))
}

/// 会计日历字典（cr_acct_calendar 年度→月度自分级）。
pub async fn calendar() -> Result<Value> {
    debug!("{:<12} - calendar db={}", "RPT-STORE", RPT_DB_ID);
    let periods = query_rows(
        r#"SELECT code, name, calendar_type, fiscal_year, period_no, parent_code,
                  full_path, level_no, is_leaf, period_type, start_date, end_date,
                  quarter, period_status, is_year_end, days, sort_no, status
           FROM cr_acct_calendar
           WHERE COALESCE(status, 1) = 1
           ORDER BY COALESCE(sort_no, 999999), code"#,
        dv![],
        "report_design_calendar",
    )
    .await?;
    Ok(json!({ "dbId": RPT_DB_ID, "periods": periods }))
}

/// 合并组织架构（cr_consol_org parent_id 自分级树）。
pub async fn consol_org() -> Result<Value> {
    debug!("{:<12} - consol_org db={}", "RPT-STORE", RPT_DB_ID);
    let orgs = query_rows(
        r#"SELECT id, code, name, consol_scheme, org_type, parent_id, full_path,
                  level_no, is_leaf, entity_code, consol_method, ownership_pct, voting_pct,
                  consol_currency, is_parent, offset_flag, remark, sort_no, status
           FROM cr_consol_org
           WHERE COALESCE(status, 1) = 1
           ORDER BY COALESCE(sort_no, 999999), id"#,
        dv![],
        "report_design_consol_org",
    )
    .await?;
    Ok(json!({ "dbId": RPT_DB_ID, "orgs": orgs }))
}

/// 报表详情：主档 + 版本 + 选中版本统计。
pub async fn report_detail(code: &str, version: Option<String>) -> Result<Value> {
    let report = query_rows(
        r#"SELECT code, name, report_type, report_category, format_code, period_type,
                  currency_code, amount_unit, entity_scope, template_version, data_source,
                  is_statutory, remark, sort_no, status, create_time, update_time
           FROM cr_report_list
           WHERE code = $1"#,
        dv![code],
        "report_design_report",
    )
    .await?
    .into_iter()
    .next()
    .ok_or_else(|| api_err("报表不存在"))?;

    let code = report
        .get("code")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let versions = query_rows(
        r#"SELECT code, name, report_code, version_no, version_status, is_current,
                  base_version_code, effective_from, effective_to, change_summary,
                  publish_time, publish_by, remark, sort_no, status, create_time, update_time
           FROM cr_report_version
           WHERE report_code = $1
           ORDER BY version_no DESC, code DESC"#,
        dv![code.clone()],
        "report_design_versions",
    )
    .await?;
    let selected_version = version
        .filter(|v| !v.trim().is_empty())
        .or_else(|| {
            versions
                .iter()
                .find(|v| v.get("is_current").and_then(|x| x.as_i64()).unwrap_or(0) == 1)
                .and_then(|v| {
                    v.get("code")
                        .and_then(|x| x.as_str())
                        .map(ToOwned::to_owned)
                })
        })
        .or_else(|| {
            versions.first().and_then(|v| {
                v.get("code")
                    .and_then(|x| x.as_str())
                    .map(ToOwned::to_owned)
            })
        });

    let stats = if let Some(ver) = selected_version.as_deref() {
        query_rows(
            r#"SELECT
                 (SELECT COUNT(*) FROM cr_report_sheet WHERE report_code = $1 AND version_code = $2) AS sheet_count,
                 (SELECT COUNT(*) FROM cr_report_region WHERE report_code = $1 AND version_code = $2) AS region_count,
                 (SELECT COUNT(*) FROM cr_report_row WHERE report_code = $1 AND version_code = $2) AS row_count,
                 (SELECT COUNT(*) FROM cr_report_col WHERE report_code = $1 AND version_code = $2) AS col_count,
                 (SELECT COUNT(*) FROM cr_report_fmt WHERE report_code = $1 AND version_code = $2) AS format_count"#,
            dv![code, ver],
            "report_design_detail_stats",
        )
        .await?
        .into_iter()
        .next()
        .unwrap_or_else(|| json!({}))
    } else {
        json!({})
    };

    Ok(json!({
        "dbId": RPT_DB_ID,
        "report": report,
        "versions": versions,
        "selectedVersion": selected_version,
        "stats": stats,
    }))
}

// ============================================================================
// 报表主档 / 版本：写服务
// ============================================================================

/// 新建报表（含初始版本）。
pub async fn create_report(body: &Value) -> Result<Value> {
    let code = str_field(body, "code").ok_or_else(|| api_err("报表编码不能为空"))?;
    let name = str_field(body, "name").ok_or_else(|| api_err("报表名称不能为空"))?;
    let report_type = str_field(body, "report_type").unwrap_or_else(|| "CUSTOM".to_string());
    let report_category =
        str_field(body, "report_category").unwrap_or_else(|| "management".to_string());
    let period_type = str_field(body, "period_type").unwrap_or_else(|| "month".to_string());
    let sort_no = int_field(body, "sort_no").unwrap_or(100);
    let status = bool_int_field(body, "status").unwrap_or(1);
    let is_statutory = bool_int_field(body, "is_statutory").unwrap_or(0);

    execute(
        r#"INSERT INTO cr_report_list
           (code, name, report_type, report_category, format_code, period_type,
            currency_code, amount_unit, entity_scope, template_version, data_source,
            is_statutory, remark, sort_no, status, create_time, update_time)
           VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,CURRENT_TIMESTAMP,CURRENT_TIMESTAMP)"#,
        dv![
            code.clone(),
            name.clone(),
            report_type,
            report_category,
            str_field(body, "format_code"),
            period_type,
            str_field(body, "currency_code").unwrap_or_else(|| "CNY".to_string()),
            str_field(body, "amount_unit").unwrap_or_else(|| "yuan".to_string()),
            str_field(body, "entity_scope").unwrap_or_else(|| "single".to_string()),
            str_field(body, "template_version").unwrap_or_else(|| "V1".to_string()),
            str_field(body, "data_source"),
            is_statutory,
            str_field(body, "remark"),
            sort_no,
            status
        ],
    )
    .await?;

    let version_code = str_field(body, "version_code").unwrap_or_else(|| "V1".to_string());
    create_version_row(
        &code,
        &version_code,
        &str_field(body, "version_name").unwrap_or_else(|| "初始版本".to_string()),
        1,
        None,
        "draft",
        1,
        str_field(body, "change_summary").as_deref(),
    )
    .await?;

    Ok(json!({ "code": code, "name": name, "version": version_code }))
}

/// 删除报表（级联删 7 张表）。
pub async fn delete_report(code: &str) -> Result<Value> {
    debug!("{:<12} - delete_report {}", "RPT-STORE", code);
    for sql in [
        "DELETE FROM cr_report_fmt WHERE report_code = $1",
        "DELETE FROM cr_report_col WHERE report_code = $1",
        "DELETE FROM cr_report_row WHERE report_code = $1",
        "DELETE FROM cr_report_region WHERE report_code = $1",
        "DELETE FROM cr_report_sheet WHERE report_code = $1",
        "DELETE FROM cr_report_version WHERE report_code = $1",
        "DELETE FROM cr_report_list WHERE code = $1",
    ] {
        execute(sql, dv![code]).await?;
    }
    Ok(json!({ "code": code }))
}

/// 建版本。
pub async fn create_version(report_code: &str, body: &CreateVersionBody) -> Result<Value> {
    let rows = query_rows(
        "SELECT COALESCE(MAX(version_no), 0) AS max_no FROM cr_report_version WHERE report_code = $1",
        dv![report_code],
        "report_design_version_no",
    )
    .await?;
    let next_no = rows
        .first()
        .and_then(|r| r.get("max_no"))
        .and_then(|v| v.as_i64())
        .unwrap_or(0)
        + 1;
    let version_code = body
        .code
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| format!("V{next_no}"));
    let version_name = body
        .name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| format!("版本 {next_no}"));
    let is_current = if body.is_current.unwrap_or(false) {
        1
    } else {
        0
    };
    if is_current == 1 {
        execute(
            "UPDATE cr_report_version SET is_current = 0, update_time = CURRENT_TIMESTAMP WHERE report_code = $1",
            dv![report_code],
        )
        .await?;
    }
    create_version_row(
        report_code,
        &version_code,
        &version_name,
        next_no,
        body.base_version_code.as_deref(),
        "draft",
        is_current,
        body.change_summary.as_deref(),
    )
    .await?;
    Ok(json!({
        "reportCode": report_code,
        "version": version_code,
        "versionNo": next_no
    }))
}

/// 设置默认（当前生效）版本。
pub async fn set_default_version(report_code: &str, version: &str) -> Result<Value> {
    let exists = query_rows(
        "SELECT code FROM cr_report_version WHERE report_code = $1 AND code = $2",
        dv![report_code, version],
        "report_design_default_version_check",
    )
    .await?;
    if exists.is_empty() {
        return Err(api_err("版本不存在"));
    }
    execute(
        "UPDATE cr_report_version SET is_current = CASE WHEN code = $2 THEN 1 ELSE 0 END, update_time = CURRENT_TIMESTAMP WHERE report_code = $1",
        dv![report_code, version],
    )
    .await?;
    Ok(json!({ "reportCode": report_code, "version": version }))
}

#[allow(clippy::too_many_arguments)]
async fn create_version_row(
    report_code: &str,
    version_code: &str,
    version_name: &str,
    version_no: i64,
    base_version_code: Option<&str>,
    version_status: &str,
    is_current: i64,
    change_summary: Option<&str>,
) -> Result<()> {
    execute(
        r#"INSERT INTO cr_report_version
           (code, name, report_code, version_no, version_status, is_current,
            base_version_code, change_summary, remark, sort_no, status, create_time, update_time)
           VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,1,CURRENT_TIMESTAMP,CURRENT_TIMESTAMP)"#,
        dv![
            version_code,
            version_name,
            report_code,
            version_no,
            version_status,
            is_current,
            base_version_code,
            change_summary,
            change_summary,
            version_no
        ],
    )
    .await
}

// ============================================================================
// 模式一 · 版式加载/存储
// ============================================================================

/// 读版式：cr_report_fmt(BLOB, ZmcDataSet 零拷贝→base64) + 关系投影。
pub async fn load_layout(code: &str, q: &LayoutQuery) -> Result<Value> {
    let version = q
        .version
        .as_deref()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_default()
        .to_string();

    // —— 版式 BLOB：ZmcDataSet 零拷贝，直接从行借出 bytes 编 base64 ——
    let mm = get_default_pg_db_manager();
    let fmt = {
        let ds = mm
            .query_sql_zmc_with_datavalues(
                RPT_DB_ID,
                r#"SELECT doc_content, doc_format, mime_type, file_size, content_hash, storage_type, external_uri
                   FROM cr_report_fmt WHERE report_code = $1 AND version_code = $2"#,
                vec![
                    DataValue::String(code.to_string()),
                    DataValue::String(version.clone()),
                ],
                "rpt_fmt",
            )
            .await
            .map_err(|e| api_err(&format!("读取报表格式失败: {e}")))?;
        if ds.row_count() == 0 {
            Value::Null
        } else {
            use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
            let sc = &ds.schema;
            let row = &ds.rows[0];
            let idx = |n: &str| sc.col_index(n);
            let content_b64 = idx("doc_content")
                .and_then(|c| row.get_bytes(c))
                .map(|b| BASE64.encode(b));
            let get_s = |n: &str| idx(n).and_then(|c| row.get_str(c)).map(str::to_owned);
            let get_i = |n: &str| idx(n).and_then(|c| row.get_i64(c));
            json!({
                "docContent": content_b64,
                "docFormat": get_s("doc_format"),
                "mimeType": get_s("mime_type"),
                "fileSize": get_i("file_size"),
                "contentHash": get_s("content_hash"),
                "storageType": get_s("storage_type"),
                "externalUri": get_s("external_uri"),
            })
        }
    };

    // —— 关系投影：小体量，直接 query_rows(JSON) 即可 ——
    let p = dv![code, version.clone()];
    let sheets = query_rows(
        r#"SELECT report_code, version_code, sheet_index, name, sheet_type, tab_color,
                  row_count, col_count, header_rows, fixed_rows, fixed_cols, paper_size,
                  orientation, font_family, font_size, show_gridline, is_hidden,
                  title_style, header_style, sort_no, status
           FROM cr_report_sheet WHERE report_code=$1 AND version_code=$2
           ORDER BY sheet_index"#,
        p.clone(),
        "rpt_sheets",
    )
    .await?;
    let regions = query_rows(
        r#"SELECT report_code, version_code, sheet_code, region_code, region_name, region_type,
                  start_row, start_col, end_row, end_col, start_cell, end_cell, row_span, col_span,
                  direction, is_repeatable, data_source, is_merged, freeze_flag, bg_color,
                  border_style, style, sort_no, status
           FROM cr_report_region WHERE report_code=$1 AND version_code=$2
           ORDER BY sheet_code, region_code"#,
        p.clone(),
        "rpt_regions",
    )
    .await?;
    let rows = query_rows(
        r#"SELECT id, code, name, report_code, version_code, sheet_code, region_code, row_no,
                  line_no, row_type, parent_id, full_path, level_no, is_leaf, formula,
                  account_range, balance_dir, indent, is_bold, sort_no, status
           FROM cr_report_row WHERE report_code=$1 AND version_code=$2
           ORDER BY sheet_code, region_code, row_no"#,
        p.clone(),
        "rpt_rows",
    )
    .await?;
    let cols = query_rows(
        r#"SELECT id, code, name, report_code, version_code, sheet_code, region_code, col_no,
                  col_letter, col_type, parent_id, full_path, level_no, is_leaf, value_type,
                  period_offset, width, decimals, align, formula, is_hidden, sort_no, status
           FROM cr_report_col WHERE report_code=$1 AND version_code=$2
           ORDER BY sheet_code, region_code, col_no"#,
        p.clone(),
        "rpt_cols",
    )
    .await?;
    let cell_map = query_rows(
        r#"SELECT id, code, name, report_code, version_code, sheet_code, region_code, row_id,
                  col_id, cell_ref, element_code, value_type, data_source, calc_formula,
                  check_formula, is_editable, number_format, sort_no, status
           FROM cr_cell_element_map WHERE report_code=$1 AND version_code=$2
           ORDER BY sheet_code, region_code"#,
        p,
        "rpt_cell_map",
    )
    .await?;

    Ok(json!({
        "dbId": RPT_DB_ID,
        "reportCode": code,
        "version": version,
        "fmt": fmt,
        "sheets": sheets,
        "regions": regions,
        "rows": rows,
        "cols": cols,
        "cellMap": cell_map,
    }))
}

/// 存版式的语义结果：冲突（乐观锁）或成功（含返回载荷）。api 层据此映射 409 / 200。
pub enum SaveLayoutOutcome {
    /// content_hash 与库内不一致（他人已更新）。
    Conflict,
    /// 成功，携带 { ok, contentHash, fileSize, idMap } 载荷。
    Ok(Value),
}

/// 存版式：乐观锁校验 → 事务内 UPSERT BLOB + 重建关系投影。自管事务。
pub async fn save_layout(code: &str, body: &Value) -> Result<SaveLayoutOutcome> {
    use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
    use sha2::{Digest, Sha256};

    let version = s(body, "version").unwrap_or_default();
    let fmt = body.get("fmt").cloned().unwrap_or(Value::Null);
    let doc_b64 = fmt.get("docContent").and_then(|v| v.as_str()).unwrap_or("");
    let doc_bytes = BASE64
        .decode(doc_b64)
        .map_err(|e| api_err(&format!("docContent 非法 base64: {e}")))?;
    let doc_format = fmt
        .get("docFormat")
        .and_then(|v| v.as_str())
        .unwrap_or("ssjson")
        .to_string();
    let mime_type = fmt
        .get("mimeType")
        .and_then(|v| v.as_str())
        .unwrap_or("application/json")
        .to_string();
    let client_hash = fmt
        .get("contentHash")
        .and_then(|v| v.as_str())
        .map(str::to_owned);
    let new_hash = format!("{:x}", Sha256::digest(&doc_bytes));
    let file_size = doc_bytes.len() as i64;

    let mm = get_default_pg_db_manager();

    // —— 乐观锁：比对 DB 现存 content_hash 与前端携带的 hash ——
    let cur = query_rows(
        "SELECT content_hash FROM cr_report_fmt WHERE report_code=$1 AND version_code=$2",
        dv![code, version.clone()],
        "rpt_fmt_hash",
    )
    .await?;
    if let Some(row) = cur.first() {
        let db_hash = row
            .get("content_hash")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        if !db_hash.is_empty()
            && client_hash
                .as_deref()
                .map(|h| h != db_hash)
                .unwrap_or(false)
        {
            return Ok(SaveLayoutOutcome::Conflict);
        }
    }

    let tx = mm.get_transaction_context();
    let txn_id = tx
        .begin(RPT_DB_ID)
        .await
        .map_err(|e| api_err(&format!("开启事务失败: {e}")))?;

    let result = save_layout_apply(
        &txn_id,
        code,
        &version,
        doc_bytes,
        &doc_format,
        &mime_type,
        file_size,
        &new_hash,
        body,
    )
    .await;
    match result {
        Ok(id_map) => {
            tx.commit(&txn_id)
                .await
                .map_err(|e| api_err(&format!("提交事务失败: {e}")))?;
            Ok(SaveLayoutOutcome::Ok(json!({
                "ok": true,
                "contentHash": new_hash,
                "fileSize": file_size,
                "idMap": id_map,
            })))
        }
        Err(e) => {
            let _ = tx.rollback(&txn_id).await;
            Err(e)
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn save_layout_apply(
    txn_id: &str,
    code: &str,
    version: &str,
    doc_bytes: Vec<u8>,
    doc_format: &str,
    mime_type: &str,
    file_size: i64,
    new_hash: &str,
    body: &Value,
) -> Result<Value> {
    // 1) UPSERT 版式 BLOB（DataValue::Binary → bytea）
    exec_dv(
        txn_id,
        r#"INSERT INTO cr_report_fmt
           (report_code, version_code, doc_content, doc_format, mime_type, file_size,
            content_hash, storage_type, sort_no, status, create_time, update_time)
           VALUES ($1,$2,$3,$4,$5,$6,$7,'inline',0,1,CURRENT_TIMESTAMP,CURRENT_TIMESTAMP)
           ON CONFLICT (report_code, version_code) DO UPDATE SET
             doc_content=EXCLUDED.doc_content, doc_format=EXCLUDED.doc_format,
             mime_type=EXCLUDED.mime_type, file_size=EXCLUDED.file_size,
             content_hash=EXCLUDED.content_hash, update_time=CURRENT_TIMESTAMP"#,
        vec![
            DataValue::String(code.to_string()),
            DataValue::String(version.to_string()),
            DataValue::Binary(doc_bytes),
            DataValue::String(doc_format.to_string()),
            DataValue::String(mime_type.to_string()),
            DataValue::Int(file_size),
            DataValue::String(new_hash.to_string()),
        ],
    )
    .await?;

    // 2) 重建关系投影：先删本 report+version 全部，再批量插入（幂等）。
    //    B1 稳定 id：删除前先预载既有「业务键→真实 id」映射，重插时复用旧 id——
    //    避免行/列/单元格 id 每次保存重铸而切断 cr_cell_data 等外部引用（协同的前置地基）。
    let row_reuse = preload_id_map(code, version, "cr_report_row", "code").await?;
    let col_reuse = preload_id_map(code, version, "cr_report_col", "code").await?;
    let cell_reuse = preload_id_map(code, version, "cr_cell_element_map", "cell_ref").await?;

    for tbl in [
        "cr_report_sheet",
        "cr_report_region",
        "cr_report_row",
        "cr_report_col",
        "cr_cell_element_map",
    ] {
        exec_dv(
            txn_id,
            &format!("DELETE FROM {tbl} WHERE report_code=$1 AND version_code=$2"),
            vec![
                DataValue::String(code.to_string()),
                DataValue::String(version.to_string()),
            ],
        )
        .await?;
    }

    // 2a) sheets
    for (idx, sh) in arr(body, "sheets").iter().enumerate() {
        exec_dv(
            txn_id,
            r#"INSERT INTO cr_report_sheet
               (report_code, version_code, sheet_index, name, sheet_type, tab_color, row_count,
                col_count, header_rows, fixed_rows, fixed_cols, font_family, font_size,
                show_gridline, is_hidden, title_style, header_style, sort_no, status,
                create_time, update_time)
               VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,1,CURRENT_TIMESTAMP,CURRENT_TIMESTAMP)"#,
            vec![
                DataValue::String(code.to_string()),
                DataValue::String(version.to_string()),
                dv_i64(i(sh, "sheetIndex").or(Some(idx as i64))),
                dv_str_def(s(sh, "name").as_deref(), &format!("Sheet{}", idx + 1)),
                dv_str_def(s(sh, "sheetType").as_deref(), "data"),
                dv_str(s(sh, "tabColor").as_deref()),
                dv_i64_def(i(sh, "rowCount"), 60),
                dv_i64_def(i(sh, "colCount"), 18),
                dv_i64(i(sh, "headerRows")),
                dv_i64(i(sh, "fixedRows")),
                dv_i64(i(sh, "fixedCols")),
                dv_str(s(sh, "fontFamily").as_deref()),
                dv_i64(i(sh, "fontSize")),
                dv_i64_def(i(sh, "showGridline"), 1),
                dv_i64_def(i(sh, "isHidden"), 0),
                dv_str(s(sh, "titleStyle").as_deref()),
                dv_str(s(sh, "headerStyle").as_deref()),
                dv_i64(i(sh, "sortNo").or(Some(idx as i64))),
            ],
        )
        .await?;
    }

    // 2b) regions（含默认区域）
    for (idx, rg) in arr(body, "regions").iter().enumerate() {
        let region_code = s(rg, "code")
            .filter(|c| !c.is_empty())
            .unwrap_or_else(|| DEFAULT_REGION.to_string());
        exec_dv(
            txn_id,
            r#"INSERT INTO cr_report_region
               (report_code, version_code, sheet_code, region_code, region_name, region_type,
                start_row, start_col, end_row, end_col, start_cell, end_cell, row_span, col_span,
                direction, is_repeatable, data_source, is_merged, freeze_flag, bg_color,
                border_style, style, sort_no, status, create_time, update_time)
               VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19,$20,$21,$22,$23,1,CURRENT_TIMESTAMP,CURRENT_TIMESTAMP)"#,
            vec![
                DataValue::String(code.to_string()),
                DataValue::String(version.to_string()),
                dv_str(s(rg, "sheetCode").as_deref()),
                DataValue::String(region_code),
                dv_str_def(s(rg, "name").as_deref(), "默认区域"),
                dv_str_def(s(rg, "type").as_deref(), "data"),
                dv_i64(i(rg, "startRow")),
                dv_i64(i(rg, "startCol")),
                dv_i64(i(rg, "endRow")),
                dv_i64(i(rg, "endCol")),
                dv_str(s(rg, "startCell").as_deref()),
                dv_str(s(rg, "endCell").as_deref()),
                dv_i64(i(rg, "rowSpan")),
                dv_i64(i(rg, "colSpan")),
                dv_str(s(rg, "direction").as_deref()),
                dv_i64_def(i(rg, "isRepeatable"), 0),
                dv_str(s(rg, "dataSource").as_deref()),
                dv_i64_def(i(rg, "isMerged"), 0),
                dv_i64_def(i(rg, "freezeFlag"), 0),
                dv_str(s(rg, "bgColor").as_deref()),
                dv_str(s(rg, "borderStyle").as_deref()),
                dv_str(s(rg, "style").as_deref()),
                dv_i64(i(rg, "sortNo").or(Some(idx as i64))),
            ],
        )
        .await?;
    }

    // 2c) rows —— id 铸真号 + 临时id映射
    let mut id_map = serde_json::Map::new();
    let mut temp_id_map: HashMap<String, i64> = HashMap::new();
    for (idx, rw) in arr(body, "rows").iter().enumerate() {
        let sheet_code = s(rw, "sheetCode").unwrap_or_default();
        let region_code = s(rw, "regionCode")
            .filter(|c| !c.is_empty())
            .unwrap_or_else(|| DEFAULT_REGION.to_string());
        let row_code = s(rw, "code").unwrap_or_default();
        let real_id = resolve_or_reuse(
            rw.get("id"),
            &row_reuse,
            &format!("{sheet_code}|{region_code}|{row_code}"),
        );
        id_map.insert(
            format!("row|{sheet_code}|{region_code}|{row_code}"),
            json!(real_id),
        );
        if let Some(tid) = rw.get("id").and_then(|v| v.as_str()) {
            temp_id_map.insert(tid.to_string(), real_id);
        }
        exec_dv(
            txn_id,
            r#"INSERT INTO cr_report_row
               (id, code, name, report_code, version_code, sheet_code, region_code, row_no,
                line_no, row_type, parent_id, full_path, level_no, is_leaf, formula, account_range,
                balance_dir, indent, is_bold, sort_no, status, create_time, update_time)
               VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19,$20,1,CURRENT_TIMESTAMP,CURRENT_TIMESTAMP)"#,
            vec![
                DataValue::Int(real_id),
                dv_str_def(Some(&row_code), &format!("R{}", idx + 1)),
                dv_str_def(s(rw, "name").as_deref(), ""),
                DataValue::String(code.to_string()),
                DataValue::String(version.to_string()),
                dv_str(Some(&sheet_code)),
                DataValue::String(region_code),
                dv_i64(i(rw, "rowNo").or(Some(idx as i64))),
                dv_str(s(rw, "lineNo").as_deref()),
                dv_str_def(s(rw, "rowType").as_deref(), "data"),
                dv_i64(i(rw, "parentId")),
                dv_str_def(s(rw, "fullPath").as_deref(), &row_code),
                dv_i64_def(i(rw, "levelNo"), 1),
                dv_i64_def(i(rw, "isLeaf"), 1),
                dv_str(s(rw, "formula").as_deref()),
                dv_str(s(rw, "accountRange").as_deref()),
                dv_str(s(rw, "balanceDir").as_deref()),
                dv_i64(i(rw, "indent")),
                dv_i64_def(i(rw, "isBold"), 0),
                dv_i64(i(rw, "sortNo").or(Some(idx as i64))),
            ],
        )
        .await?;
    }

    // 2d) cols
    for (idx, cl) in arr(body, "cols").iter().enumerate() {
        let sheet_code = s(cl, "sheetCode").unwrap_or_default();
        let region_code = s(cl, "regionCode")
            .filter(|c| !c.is_empty())
            .unwrap_or_else(|| DEFAULT_REGION.to_string());
        let col_code = s(cl, "code").unwrap_or_default();
        let real_id = resolve_or_reuse(
            cl.get("id"),
            &col_reuse,
            &format!("{sheet_code}|{region_code}|{col_code}"),
        );
        id_map.insert(
            format!("col|{sheet_code}|{region_code}|{col_code}"),
            json!(real_id),
        );
        if let Some(tid) = cl.get("id").and_then(|v| v.as_str()) {
            temp_id_map.insert(tid.to_string(), real_id);
        }
        exec_dv(
            txn_id,
            r#"INSERT INTO cr_report_col
               (id, code, name, report_code, version_code, sheet_code, region_code, col_no,
                col_letter, col_type, parent_id, full_path, level_no, is_leaf, value_type, period_offset,
                width, decimals, align, formula, is_hidden, sort_no, status, create_time, update_time)
               VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19,$20,$21,$22,1,CURRENT_TIMESTAMP,CURRENT_TIMESTAMP)"#,
            vec![
                DataValue::Int(real_id),
                dv_str_def(Some(&col_code), &col_letter_of(idx)),
                dv_str_def(s(cl, "name").as_deref(), ""),
                DataValue::String(code.to_string()),
                DataValue::String(version.to_string()),
                dv_str(Some(&sheet_code)),
                DataValue::String(region_code),
                dv_i64(i(cl, "colNo").or(Some(idx as i64))),
                dv_str(s(cl, "colLetter").as_deref()),
                dv_str_def(s(cl, "colType").as_deref(), "data"),
                dv_i64(i(cl, "parentId")),
                dv_str_def(s(cl, "fullPath").as_deref(), &col_code),
                dv_i64_def(i(cl, "levelNo"), 1),
                dv_i64_def(i(cl, "isLeaf"), 1),
                dv_str(s(cl, "valueType").as_deref()),
                dv_i64(i(cl, "periodOffset")),
                dv_i64(i(cl, "width")),
                dv_i64(i(cl, "decimals")),
                dv_str(s(cl, "align").as_deref()),
                dv_str(s(cl, "formula").as_deref()),
                dv_i64_def(i(cl, "isHidden"), 0),
                dv_i64(i(cl, "sortNo").or(Some(idx as i64))),
            ],
        )
        .await?;
    }

    // 2e) cell_element_map —— row_id/col_id 把前端临时id串解引用成真号
    let resolve_ref = |v: Option<&Value>| -> DataValue {
        match v {
            Some(Value::Number(n)) => n.as_i64().map(DataValue::Int).unwrap_or(DataValue::Int(0)),
            Some(Value::String(t)) => temp_id_map
                .get(t)
                .copied()
                .map(DataValue::Int)
                .unwrap_or(DataValue::Int(0)),
            _ => DataValue::Int(0),
        }
    };
    for (idx, cm) in arr(body, "cellMap").iter().enumerate() {
        // B1 稳定 id：单元格映射按 sheet|region|cell_ref 复用既有 id，避免每次保存重铸。
        let cm_sheet = s(cm, "sheetCode").unwrap_or_default();
        let cm_region = s(cm, "regionCode")
            .filter(|c| !c.is_empty())
            .unwrap_or_else(|| DEFAULT_REGION.to_string());
        let cm_ref = s(cm, "cellRef").unwrap_or_default();
        let cm_bkey = if cm_ref.is_empty() {
            String::new()
        } else {
            format!("{cm_sheet}|{cm_region}|{cm_ref}")
        };
        let cm_id = if cm_bkey.is_empty() {
            cmx_utils::next_pk_id()
        } else {
            cell_reuse
                .get(&cm_bkey)
                .copied()
                .unwrap_or_else(cmx_utils::next_pk_id)
        };
        exec_dv(
            txn_id,
            r#"INSERT INTO cr_cell_element_map
               (id, code, report_code, version_code, sheet_code, region_code, row_id, col_id, cell_ref,
                element_code, value_type, data_source, calc_formula, check_formula, is_editable,
                number_format, sort_no, status, create_time, update_time)
               VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,1,CURRENT_TIMESTAMP,CURRENT_TIMESTAMP)"#,
            vec![
                DataValue::Int(cm_id),
                dv_str_def(s(cm, "cellRef").as_deref(), &format!("CM{}", idx + 1)),
                DataValue::String(code.to_string()),
                DataValue::String(version.to_string()),
                dv_str(s(cm, "sheetCode").as_deref()),
                dv_str_def(
                    s(cm, "regionCode").filter(|c| !c.is_empty()).as_deref(),
                    DEFAULT_REGION,
                ),
                resolve_ref(cm.get("rowId")),
                resolve_ref(cm.get("colId")),
                dv_str(s(cm, "cellRef").as_deref()),
                dv_str(s(cm, "elementCode").as_deref()),
                dv_str(s(cm, "valueType").as_deref()),
                dv_str(s(cm, "dataSource").as_deref()),
                dv_str(s(cm, "calcFormula").as_deref()),
                dv_str(s(cm, "checkFormula").as_deref()),
                dv_i64_def(i(cm, "isEditable"), 1),
                dv_str(s(cm, "numberFormat").as_deref()),
                dv_i64(i(cm, "sortNo").or(Some(idx as i64))),
            ],
        )
        .await?;
    }

    Ok(Value::Object(id_map))
}

// ============================================================================
// 模式二 · 数据加载/存储
// ============================================================================

/// 取数：cr_cell_data 按 org+period ZmcDataSet 零拷贝读。
pub async fn query_data(code: &str, body: &Value) -> Result<Value> {
    let version = s(body, "version").unwrap_or_default();
    let org = s(body, "orgCode").unwrap_or_default();
    let period = s(body, "periodCode").unwrap_or_default();

    let mm = get_default_pg_db_manager();
    let ds = mm
        .query_sql_zmc_with_datavalues(
            RPT_DB_ID,
            r#"SELECT sheet_code, region_code, row_id, col_id, cell_ref, element_code,
                      value_type, text_value, num_value, currency_code, amount_unit,
                      data_status, is_manual
               FROM cr_cell_data
               WHERE report_code=$1 AND version_code=$2 AND org_code=$3 AND period_code=$4
               ORDER BY sheet_code, region_code, row_id, col_id"#,
            vec![
                DataValue::String(code.to_string()),
                DataValue::String(version.clone()),
                DataValue::String(org.clone()),
                DataValue::String(period.clone()),
            ],
            "rpt_cell_data",
        )
        .await
        .map_err(|e| api_err(&format!("读取报表数据失败: {e}")))?;

    // 逐行零拷贝取值：只在结果 payload 里生成需要的小对象，峰值内存 O(结果集)。
    let sc = &ds.schema;
    let c_sheet = sc.col_index("sheet_code");
    let c_region = sc.col_index("region_code");
    let c_row = sc.col_index("row_id");
    let c_col = sc.col_index("col_id");
    let c_ref = sc.col_index("cell_ref");
    let c_el = sc.col_index("element_code");
    let c_vt = sc.col_index("value_type");
    let c_txt = sc.col_index("text_value");
    let c_num = sc.col_index("num_value");
    let c_cur = sc.col_index("currency_code");
    let c_unit = sc.col_index("amount_unit");
    let c_ds = sc.col_index("data_status");
    let c_man = sc.col_index("is_manual");

    let mut cells = Vec::with_capacity(ds.row_count());
    for row in &ds.rows {
        let gs = |c: Option<usize>| c.and_then(|i| row.get_str(i)).map(str::to_owned);
        let gi = |c: Option<usize>| c.and_then(|i| row.get_i64(i));
        let num = c_num
            .and_then(|i| row.get_decimal(i))
            .map(|d| d.to_string());
        cells.push(json!({
            "sheetCode": gs(c_sheet),
            "regionCode": gs(c_region),
            "rowId": gi(c_row),
            "colId": gi(c_col),
            "cellRef": gs(c_ref),
            "elementCode": gs(c_el),
            "valueType": gs(c_vt),
            "textValue": gs(c_txt),
            "numValue": num,
            "currencyCode": gs(c_cur),
            "amountUnit": gs(c_unit),
            "dataStatus": gs(c_ds),
            "isManual": gi(c_man),
        }));
    }

    Ok(json!({
        "dbId": RPT_DB_ID,
        "reportCode": code,
        "version": version,
        "orgCode": org,
        "periodCode": period,
        "count": cells.len(),
        "cells": cells,
    }))
}

/// 打开报表（一次后端调用取全集，替代前端顺序多调）：版式 BLOB + 关系投影 + cellMap（元素/公式）
/// + 元素目录 + 函数目录，若 body 带 orgCode+periodCode 再并入数据（cr_cell_data）。
///
/// - 设计器打开：body 仅 {version} → 返回 fmt/sheets/regions/rows/cols/cellMap/categories/elements/functions，cells=[]。
/// - 应用器打开：body {version,orgCode,periodCode} → 额外并入 cells（该 org+period 的已存数据）。
///
/// 各子服务顶层 key 互不冲突，平铺进一个对象合并返回；子服务各自复用（零重复 SQL）。
pub async fn open_report(code: &str, body: &Value) -> Result<Value> {
    let version = s(body, "version").unwrap_or_default();
    let org = s(body, "orgCode").unwrap_or_default();
    let period = s(body, "periodCode").unwrap_or_default();
    let want_data = !org.trim().is_empty() && !period.trim().is_empty();

    // 版式 + 关系投影 + cellMap（复用 load_layout 的整段返回）
    let layout = load_layout(
        code,
        &LayoutQuery {
            version: Some(version.clone()),
        },
    )
    .await?;
    // 元素目录（categories + elements）
    let elements = elements().await?;
    // 函数目录（内置 + 取数函数元数据，供向导/公式栏）
    let functions = cmx_rpt_formula::catalog_json();
    // 数据（仅应用器：org+period 齐备才取；设计器打开返回空 cells）
    let cells = if want_data {
        query_data(code, body).await?
            .get("cells")
            .cloned()
            .unwrap_or_else(|| json!([]))
    } else {
        json!([])
    };

    // 平铺合并：layout 段（fmt/sheets/regions/rows/cols/cellMap）作基座，补 elements/functions/cells。
    let mut out = layout;
    if let Value::Object(map) = &mut out {
        map.insert("dbId".into(), json!(RPT_DB_ID));
        map.insert("reportCode".into(), json!(code));
        map.insert("version".into(), json!(version));
        map.insert(
            "categories".into(),
            elements
                .get("categories")
                .cloned()
                .unwrap_or_else(|| json!([])),
        );
        map.insert(
            "elements".into(),
            elements.get("elements").cloned().unwrap_or_else(|| json!([])),
        );
        map.insert(
            "functions".into(),
            functions
                .get("functions")
                .cloned()
                .unwrap_or_else(|| json!([])),
        );
        map.insert("orgCode".into(), json!(org));
        map.insert("periodCode".into(), json!(period));
        map.insert("hasData".into(), json!(want_data));
        map.insert("cells".into(), cells);
    }
    Ok(out)
}

// ============================================================================
// 浮动行列展开（P1：行浮动 MVP）
// ============================================================================
//
// 打开应用报表时，把「浮动模板行」按数据源展开成 N 条实例行，随 open bundle 一并返回，
// 前端画布直接渲染最终 N 行（展开发生在后端，前端零展开逻辑）。设计见
// `docs/报表浮动行列(动态明细展开)设计方案.html`。
//
// 浮动区识别：region.is_repeatable=1。模板行识别：该区域内 row_type='float' 的行。
// 模板每列公式来自 cell_element_map.calc_formula（行=模板行 id，列=各列 id）。
// 数据源来自 region.data_source（P1 支持 dict:表名 / sample:内置示例；P4 接 FLIST 真实取数）。

use crate::expand::{
    FloatTemplate, HierLevel, HierTemplate, InstanceRow, SourceRecord, expand_hierarchy,
    expand_template,
};

/// 解析分级维度：data_source 指示分级浮动时返回有序维度列表，否则 None（走扁平）。
/// - `sample-hier` → `["region","cust_code"]`（内置分级示例）。
/// - `<任意源>;hier=dim1,dim2[,...]` → 显式声明层级维度（外→内）。
fn parse_hier_dims(data_source: &str) -> Option<Vec<String>> {
    let spec = data_source.trim();
    if spec.eq_ignore_ascii_case("sample-hier") {
        return Some(vec!["region".to_string(), "cust_code".to_string()]);
    }
    // 显式 ;hier=a,b 标记
    for seg in spec.split(';') {
        let seg = seg.trim();
        if let Some(rest) = seg.strip_prefix("hier=") {
            let dims: Vec<String> = rest
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            if !dims.is_empty() {
                return Some(dims);
            }
        }
    }
    None
}

/// 列浮动检测：data_source 为 `sample-cols` 或带 `;axis=col` → 走列展开（横向铺列）。
fn is_col_float(data_source: &str) -> bool {
    let spec = data_source.trim();
    spec.eq_ignore_ascii_case("sample-cols")
        || spec.split(';').any(|s| s.trim().eq_ignore_ascii_case("axis=col"))
}

/// 列浮动数据源：产出「列集合」（P3 内置 `sample-cols` = 近 6 个月期间；或 `dict:cr_acct_calendar`）。
/// 每条记录一列，dims 含 `period_code`。
async fn resolve_col_source(data_source: &str) -> Result<Vec<SourceRecord>> {
    let spec = data_source.trim();
    // dict:cr_acct_calendar;axis=col → 复用字典源
    if let Some(seg) = spec.split(';').find(|s| s.trim().starts_with("dict:")) {
        return resolve_float_source(seg.trim()).await;
    }
    // sample-cols：内置 6 个月示例。
    let months = [
        "2026-01", "2026-02", "2026-03", "2026-04", "2026-05", "2026-06",
    ];
    Ok(months
        .iter()
        .map(|m| SourceRecord {
            label: (*m).to_string(),
            dims: vec![("period_code".to_string(), (*m).to_string())],
            cells: Vec::new(),
        })
        .collect())
}

/// 构造并展开一个列浮动区：找 `col_type='float'` 模板列 → 其各行公式(cellMap) → expand_columns。
/// 返回 float region JSON（含 `axis:"col"` + `colInstances[]`），无模板列则 None。
async fn expand_col_region(
    code: &str,
    version: &str,
    org: &str,
    period: &str,
    region_code: &str,
    sheet_code: &str,
    data_source: &str,
    start_col: i64,
    cols: &[Value],
    cell_map: &[Value],
) -> Result<Option<Value>> {
    use crate::expand::{ColFloatTemplate, expand_columns};

    // 模板列：同区域 col_type='float'。
    let tpl_col = cols.iter().find(|c| {
        c.get("region_code").and_then(|v| v.as_str()) == Some(region_code)
            && c.get("sheet_code").and_then(|v| v.as_str()) == Some(sheet_code)
            && c.get("col_type").and_then(|v| v.as_str()) == Some("float")
    });
    let Some(tpl_col) = tpl_col else {
        return Ok(None);
    };
    let tpl_col_id = tpl_col.get("id").and_then(|v| v.as_i64()).unwrap_or(0);
    let header_tpl = tpl_col
        .get("name")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("{{label}}")
        .to_string();

    // 该模板列的各行公式：cellMap 中 col_id == 模板列 id 的记录，按 cell_ref 的行号定位。
    let mut row_tpls: Vec<(i64, String)> = Vec::new();
    for m in cell_map {
        if m.get("col_id").and_then(|v| v.as_i64()) != Some(tpl_col_id) {
            continue;
        }
        let f = m.get("calc_formula").and_then(|v| v.as_str()).unwrap_or("");
        if f.is_empty() {
            continue;
        }
        // 行号取自 cell_ref（如 "C5" → 5），退化用 row_id 不适用，故要求 cell_ref。
        let row_no = m
            .get("cell_ref")
            .and_then(|v| v.as_str())
            .and_then(|r| {
                r.chars()
                    .skip_while(|c| c.is_ascii_alphabetic())
                    .collect::<String>()
                    .parse::<i64>()
                    .ok()
            })
            .unwrap_or(0);
        if row_no > 0 {
            row_tpls.push((row_no, f.to_string()));
        }
    }
    row_tpls.sort_by_key(|(r, _)| *r);

    let template = ColFloatTemplate {
        template_col_id: tpl_col_id,
        header_tpl,
        row_tpls,
    };
    // 数据来源优先级（F3）：先读存储态浮动列表（cr_report_float_col）；空则回退实时数据源。
    let stored = read_stored_float_records(
        code, version, sheet_code, region_code, org, period, true,
    )
    .await?;
    let records = if !stored.is_empty() {
        stored
    } else {
        resolve_col_source(data_source).await?
    };
    // 模板列物理列序作为展开起点（0-based）。
    let tpl_ci = tpl_col
        .get("col_no")
        .and_then(|v| v.as_i64())
        .unwrap_or(start_col.max(0));
    let instances = expand_columns(&template, &records, tpl_ci);

    let inst_json: Vec<Value> = instances
        .iter()
        .map(|c| {
            json!({
                "colId": c.col_id,
                "dimKeyPath": c.dim_key_path,
                "header": c.header,
                "colLetter": c.col_letter,
                "colIndex": c.col_index,
                "sortNo": c.sort_no,
                "cells": c.cells.iter().map(|(row, f)| json!({"row": row, "formula": f})).collect::<Vec<_>>(),
            })
        })
        .collect();

    Ok(Some(json!({
        "sheetCode": sheet_code,
        "regionCode": region_code,
        "axis": "col",
        "templateColId": tpl_col_id,
        "dataSource": data_source,
        "startCol": tpl_ci,
        "count": inst_json.len(),
        "colInstances": inst_json,
    })))
}

/// 读存储态浮动记录（cr_report_float_row / _col）→ Vec<SourceRecord>（F3）。
/// 表里有记录时以其为准（用户 CRUD 结果）；空则返回空（调用方回退实时数据源）。
/// `is_col`：true 读浮动列表，false 读浮动行表。dim_key（`k=v;k=v`）解析回 dims 顺序保持。
async fn read_stored_float_records(
    code: &str,
    version: &str,
    sheet: &str,
    region: &str,
    org: &str,
    period: &str,
    is_col: bool,
) -> Result<Vec<SourceRecord>> {
    // 无 org/period（设计器打开）→ 不读存储（存储按 org+period 隔离）。
    if org.trim().is_empty() || period.trim().is_empty() {
        return Ok(Vec::new());
    }
    crate::float_ddl::ensure_float_schema().await?;
    let table = if is_col {
        "cr_report_float_col"
    } else {
        "cr_report_float_row"
    };
    let sql = format!(
        "SELECT dim_key, label, cells::text AS cells_text FROM {table} \
         WHERE report_code=$1 AND version_code=$2 AND sheet_code=$3 AND region_code=$4 \
           AND org_code=$5 AND period_code=$6 AND COALESCE(status,1)=1 \
         ORDER BY seq, id"
    );
    let rows = query_rows(
        &sql,
        dv![code, version, sheet, region, org, period],
        "rpt_float_stored",
    )
    .await?;
    Ok(rows
        .iter()
        .map(|r| {
            let dim_key = r.get("dim_key").and_then(|v| v.as_str()).unwrap_or("");
            let label = r
                .get("label")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            // 存储态显式单元格值（cells JSONB）→ (列标, 值)，覆盖模板公式。
            let cells: Vec<(String, String)> = r
                .get("cells_text")
                .and_then(|v| v.as_str())
                .and_then(|t| serde_json::from_str::<Value>(t).ok())
                .and_then(|v| v.as_object().cloned())
                .map(|obj| {
                    obj.into_iter()
                        .map(|(k, v)| {
                            let s = match v {
                                Value::String(s) => s,
                                other => other.to_string(),
                            };
                            (k, s)
                        })
                        .collect()
                })
                .unwrap_or_default();
            // dim_key `k=v;k=v` → dims（顺序保持，供占位符替换/分级）。
            let dims = dim_key
                .split(';')
                .filter_map(|kv| {
                    let mut it = kv.splitn(2, '=');
                    match (it.next(), it.next()) {
                        (Some(k), Some(v)) if !k.is_empty() => Some((k.to_string(), v.to_string())),
                        _ => None,
                    }
                })
                .collect();
            SourceRecord { label, dims, cells }
        })
        .collect())
}

/// 从 open bundle 里，为某浮动区拉取实时数据源记录（未初始化到存储表时的回退/种子来源）。
///
/// - `sample` 或空 → 内置示例记录（无需真实业务库，先跑通展开链路，对齐方案 §8 占位阶梯）。
/// - `dict:<表名>[?parent=<码>]` → 从字典表罗列（如 `dict:cr_consol_org`）。
///
/// 返回有序记录（保证维度键路径确定 → 稳定 id 确定）。
async fn resolve_float_source(data_source: &str) -> Result<Vec<SourceRecord>> {
    let spec = data_source.trim();
    if spec.is_empty() || spec.eq_ignore_ascii_case("sample") {
        // 内置示例：前 5 大待收款客户（方案目标示意）。
        let sample = [
            ("上海A公司", "C001"),
            ("杭州B公司", "C002"),
            ("南京C公司", "C003"),
            ("北京D公司", "C004"),
            ("天津E公司", "C005"),
        ];
        return Ok(sample
            .iter()
            .map(|(label, code)| SourceRecord {
                label: (*label).to_string(),
                dims: vec![("cust_code".to_string(), (*code).to_string())],
                cells: Vec::new(),
            })
            .collect());
    }

    if spec.eq_ignore_ascii_case("sample-hier") {
        // 内置分级示例：按地区▸客户（方案目标示意表）。每条含 region + cust_code 两维。
        let sample = [
            ("华东", "上海A公司", "C001"),
            ("华东", "杭州B公司", "C002"),
            ("华东", "南京C公司", "C003"),
            ("华北", "北京D公司", "C004"),
            ("华北", "天津E公司", "C005"),
        ];
        return Ok(sample
            .iter()
            .map(|(region, label, code)| SourceRecord {
                label: (*label).to_string(),
                dims: vec![
                    ("region".to_string(), (*region).to_string()),
                    ("cust_code".to_string(), (*code).to_string()),
                ],
                cells: Vec::new(),
            })
            .collect());
    }

    if let Some(rest) = spec.strip_prefix("flist:") {
        // flist:<对象>[?top=N]  —— P4 真实取数：按度量降序取前 N（对齐 FLIST 函数语义）。
        // 目前支持 ar_cust（应收客户，从 cv_aux_line 按客户汇总余额 local_dr-local_cr 降序）。
        let obj = rest.split('?').next().unwrap_or("").trim();
        let top: i64 = rest
            .split('?')
            .nth(1)
            .and_then(|q| {
                q.split('&').find_map(|kv| {
                    let mut it = kv.splitn(2, '=');
                    match (it.next(), it.next()) {
                        (Some("top"), Some(v)) => v.trim().parse::<i64>().ok(),
                        _ => None,
                    }
                })
            })
            .unwrap_or(10)
            .clamp(1, 500);
        match obj {
            "ar_cust" => {
                // 客户维度余额 = SUM(local_dr - local_cr)，取正余额（待收）前 N。
                let sql = format!(
                    "SELECT customer_id AS k, \
                            SUM(COALESCE(local_dr,0)-COALESCE(local_cr,0)) AS bal \
                     FROM cv_aux_line WHERE customer_id IS NOT NULL \
                     GROUP BY customer_id \
                     HAVING SUM(COALESCE(local_dr,0)-COALESCE(local_cr,0)) > 0 \
                     ORDER BY bal DESC LIMIT {top}"
                );
                let rows = query_rows(&sql, dv![], "rpt_flist_ar_cust").await?;
                return Ok(rows
                    .iter()
                    .map(|r| {
                        // customer_id 是 bigint；k 可能是数字或字符串，统一成字符串码。
                        let code = r
                            .get("k")
                            .map(|v| match v {
                                Value::Number(n) => n.to_string(),
                                Value::String(s) => s.clone(),
                                _ => String::new(),
                            })
                            .unwrap_or_default();
                        SourceRecord {
                            label: format!("客户 {code}"),
                            dims: vec![("cust_code".to_string(), code)],
                            cells: Vec::new(),
                        }
                    })
                    .collect());
            }
            other => return Err(api_err(&format!("不支持的 FLIST 对象: {other}"))),
        }
    }

    if let Some(rest) = spec.strip_prefix("dict:") {
        // dict:<表名>  —— 从字典表罗列 code/name 作为浮动记录。P1 只支持无过滤的平铺罗列。
        let table = rest.split('?').next().unwrap_or("").trim();
        // 白名单：只允许已知报表维度字典，杜绝任意表名注入。
        let allowed = ["cr_consol_org", "cr_acct_calendar"];
        if !allowed.contains(&table) {
            return Err(api_err(&format!("不支持的浮动数据源字典表: {table}")));
        }
        let (key_field, label_field) = match table {
            "cr_consol_org" => ("code", "name"),
            "cr_acct_calendar" => ("code", "name"),
            _ => ("code", "name"),
        };
        let sql = format!(
            "SELECT {key_field} AS k, {label_field} AS lbl FROM {table} \
             WHERE COALESCE(status,1)=1 ORDER BY sort_no, {key_field} LIMIT 200"
        );
        let rows = query_rows(&sql, dv![], "rpt_float_dict").await?;
        let dim = if table == "cr_consol_org" {
            "org_code"
        } else {
            "period_code"
        };
        return Ok(rows
            .iter()
            .map(|r| {
                let k = r.get("k").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let lbl = r
                    .get("lbl")
                    .and_then(|v| v.as_str())
                    .unwrap_or(&k)
                    .to_string();
                SourceRecord {
                    label: lbl,
                    dims: vec![(dim.to_string(), k)],
                    cells: Vec::new(),
                }
            })
            .collect());
    }

    Err(api_err(&format!("无法解析浮动数据源: {spec}")))
}

/// 从 open bundle 的 rows/cols/cellMap 里，为一条模板行构造 [`FloatTemplate`]。
///
/// 列公式来源：cellMap 中 `row_id==模板行 id` 的记录，按其 col_id 找到 cols 里的列标（col_letter）。
fn build_template(tpl_row: &Value, cols: &[Value], cell_map: &[Value]) -> FloatTemplate {
    let tpl_id = tpl_row.get("id").and_then(|v| v.as_i64()).unwrap_or(0);
    let name_tpl = tpl_row
        .get("name")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("{{label}}")
        .to_string();

    // col_id → col_letter 映射。
    let col_letter = |col_id: i64| -> Option<String> {
        cols.iter()
            .find(|c| c.get("id").and_then(|v| v.as_i64()) == Some(col_id))
            .and_then(|c| c.get("col_letter").and_then(|v| v.as_str()))
            .map(str::to_owned)
    };

    let mut cell_tpls = Vec::new();
    for m in cell_map {
        if m.get("row_id").and_then(|v| v.as_i64()) != Some(tpl_id) {
            continue;
        }
        let f = m
            .get("calc_formula")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if f.is_empty() {
            continue;
        }
        if let Some(cid) = m.get("col_id").and_then(|v| v.as_i64()) {
            if let Some(letter) = col_letter(cid) {
                cell_tpls.push((letter, f.to_string()));
            }
        }
    }

    FloatTemplate {
        template_row_id: tpl_id,
        name_tpl,
        cell_tpls,
    }
}

/// 打开并展开应用报表：在 [`open_report`] 基座上，把浮动区的模板行展开为 N 条实例行。
///
/// 返回 open bundle 追加一个 `float` 段：
/// ```json
/// { ...open bundle..., "float": { "regions": [ {
///     "sheetCode","regionCode","templateRowId","startRow","count",
///     "instances": [ { "rowId","dimKeyPath","name","physRow","sortNo","cells":[{col,formula}] } ]
/// } ] } }
/// ```
/// 前端据此在画布上插入 N 行、逐行 setValue(name)/setFormula(cells)。数据落库沿用既有 save_data
/// （实例行 row_id 即稳定派生 id，8 元键幂等 UPSERT）。
pub async fn expand_report(code: &str, body: &Value) -> Result<Value> {
    let mut bundle = open_report(code, body).await?;

    // 定位上下文（存储态浮动读表按 org+period 隔离；设计器打开时 org/period 空 → 读表返回空、回退实时源）。
    let version = s(body, "version").unwrap_or_default();
    let org = s(body, "orgCode").unwrap_or_default();
    let period = s(body, "periodCode").unwrap_or_default();

    let regions = bundle
        .get("regions")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let rows = bundle
        .get("rows")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let cols = bundle
        .get("cols")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let cell_map = bundle
        .get("cellMap")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    let mut float_regions = Vec::new();
    for region in &regions {
        let is_rep = region
            .get("is_repeatable")
            .and_then(|v| v.as_i64())
            .unwrap_or(0);
        if is_rep != 1 {
            continue;
        }
        let region_code = region
            .get("region_code")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let sheet_code = region
            .get("sheet_code")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let data_source = region
            .get("data_source")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let start_row = region
            .get("start_row")
            .and_then(|v| v.as_i64())
            .unwrap_or(0);
        let start_col = region
            .get("start_col")
            .and_then(|v| v.as_i64())
            .unwrap_or(0);

        // ── 列浮动（P3）：data_source 为 'sample-cols' 或带 ';axis=col' → 模板列 × 数据源 → N 实例列。
        // 与行浮动互斥（一个浮动区一个轴）。命中则处理完 continue，不走行浮动分支。
        if is_col_float(&data_source) {
            if let Some(col_region) = expand_col_region(
                code,
                &version,
                &org,
                &period,
                &region_code,
                &sheet_code,
                &data_source,
                start_col,
                &cols,
                &cell_map,
            )
            .await?
            {
                float_regions.push(col_region);
            }
            continue;
        }

        // 该区域内的浮动模板行（row_type='float'）。P1：每区取第一条模板行。
        let tpl_row = rows.iter().find(|r| {
            r.get("region_code").and_then(|v| v.as_str()) == Some(region_code.as_str())
                && r.get("sheet_code").and_then(|v| v.as_str()) == Some(sheet_code.as_str())
                && r.get("row_type").and_then(|v| v.as_str()) == Some("float")
        });
        let Some(tpl_row) = tpl_row else {
            continue;
        };

        let template = build_template(tpl_row, &cols, &cell_map);
        // 数据来源优先级（F3）：先读存储态浮动表（cr_report_float_row，按 org+period）；
        // 表里有记录 → 以存储为准（用户 CRUD 结果）；表空 → 回退实时数据源（未初始化时仍可预览）。
        let stored = read_stored_float_records(
            code, &version, &sheet_code, &region_code, &org, &period, false,
        )
        .await?;
        let records = if !stored.is_empty() {
            stored
        } else {
            resolve_float_source(&data_source).await?
        };
        // 模板行所在物理行作为展开起点（画布行 1-based）。
        let tpl_phys = tpl_row
            .get("row_no")
            .and_then(|v| v.as_i64())
            .map(|n| n + 1)
            .unwrap_or(start_row.max(1));

        // 分级浮动检测：data_source 为 'sample-hier' 或带 ';hier=<dim1,dim2>' 标记 → 走 expand_hierarchy。
        // 否则走 P1 扁平 expand_template（合计行取同区域已存的 row_type='total' 行做 {{total}} 锚点）。
        let hier_dims = parse_hier_dims(&data_source);
        let (instances, is_hier): (Vec<InstanceRow>, bool) = if let Some(dims) = hier_dims {
            // 分级：合计行占位由引擎生成（顶部）。归集列 = 模板列里以 = 开头/带取数的数值列，
            // P2 简化为「除比率列(公式含 '/')外的模板列」都归集。
            let rollup_cols: Vec<String> = template
                .cell_tpls
                .iter()
                .filter(|(_, f)| !f.contains('/'))
                .map(|(c, _)| c.clone())
                .collect();
            let levels: Vec<HierLevel> = dims
                .iter()
                .enumerate()
                .map(|(i, d)| {
                    let is_leaf = i + 1 == dims.len();
                    HierLevel {
                        dim: d.clone(),
                        rollup: if is_leaf { "none" } else { "subtotal" }.to_string(),
                        subtotal_name_tpl: "{{label}} 小计".to_string(),
                    }
                })
                .collect();
            let hier = HierTemplate {
                leaf: template.clone(),
                levels,
                grand_total: "total".to_string(),
                grand_total_name: "合计".to_string(),
                rollup_cols,
            };
            (expand_hierarchy(&hier, &records, tpl_phys), true)
        } else {
            // 合计行（同区域 row_type='total'）物理行号，供 {{total}} 锚点重定位。
            let total_row = rows
                .iter()
                .find(|r| {
                    r.get("region_code").and_then(|v| v.as_str()) == Some(region_code.as_str())
                        && r.get("sheet_code").and_then(|v| v.as_str())
                            == Some(sheet_code.as_str())
                        && r.get("row_type").and_then(|v| v.as_str()) == Some("total")
                })
                .and_then(|r| r.get("row_no").and_then(|v| v.as_i64()))
                .map(|n| n + 1);
            (expand_template(&template, &records, tpl_phys, total_row), false)
        };

        let inst_json: Vec<Value> = instances
            .iter()
            .map(|r| {
                json!({
                    "rowId": r.row_id,
                    "dimKeyPath": r.dim_key_path,
                    "name": r.name,
                    "physRow": r.phys_row,
                    "sortNo": r.sort_no,
                    "rowType": r.row_type,
                    "levelNo": r.level_no,
                    "parentRow": r.parent_row,
                    "cells": r.cells.iter().map(|(c, f)| json!({"col": c, "formula": f})).collect::<Vec<_>>(),
                })
            })
            .collect();

        float_regions.push(json!({
            "sheetCode": sheet_code,
            "regionCode": region_code,
            "templateRowId": template.template_row_id,
            "dataSource": data_source,
            "hier": is_hier,
            "startRow": tpl_phys,
            "count": inst_json.len(),
            "instances": inst_json,
        }));
    }

    if let Value::Object(map) = &mut bundle {
        map.insert(
            "float".into(),
            json!({ "regions": float_regions, "expanded": true }),
        );
    }
    Ok(bundle)
}

/// 取数初始化种子：把浮动区的数据源（sample/flist/dict）结果**写入** cr_report_float_row/col
/// （方案 F3，`is_manual=0`）。这就是"取数=一键初始化"——之后用户手工 CRUD 以存储表为准，
/// 重取数默认不覆盖手工行（seed_upsert 的手工保护）。
///
/// body: { version, orgCode, periodCode, sheetCode, regionCode, dataSource?, overwriteManual? }
/// dataSource 缺省时从该区域定义（cr_report_region.data_source）读。返回 { ok, kind, seeded }。
pub async fn seed_float(code: &str, body: &Value) -> Result<Value> {
    use crate::float_crud::{FloatKind, make_locator, seed_upsert};

    let loc = make_locator(body);
    if loc.org.trim().is_empty() || loc.period.trim().is_empty() {
        return Err(api_err("取数初始化需要组织与期间上下文"));
    }
    let region_code = s(body, "regionCode")
        .filter(|c| !c.is_empty())
        .unwrap_or_else(|| DEFAULT_REGION.to_string());
    let overwrite = body
        .get("overwriteManual")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    // 数据源：body 显式优先，否则读区域定义。
    let data_source = match s(body, "dataSource").filter(|d| !d.is_empty()) {
        Some(d) => d,
        None => region_data_source(code, &loc.version, &loc.sheet, &region_code).await?,
    };
    if data_source.trim().is_empty() {
        return Err(api_err("该浮动区未配置初始化数据源"));
    }

    // 列浮动 vs 行浮动，走不同源解析 + 落不同表。
    let is_col = is_col_float(&data_source);
    let records = if is_col {
        resolve_col_source(&data_source).await?
    } else {
        resolve_float_source(&data_source).await?
    };

    // 记录 → 浮动表 item：dim_key/label/level/seq + cells（此处只存维度键与标签，
    // cells 留空——展开时由模板公式按 dim_key 替换 {{dim}} 生成，保持存储行与版式解耦）。
    let hier_dims = if is_col { None } else { parse_hier_dims(&data_source) };
    let items: Vec<Value> = records
        .iter()
        .enumerate()
        .map(|(i, rec)| {
            let dim_key = rec
                .dims
                .iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<_>>()
                .join(";");
            // 分级：父维度键 = 除最后一维外的前缀（供展开分组）。
            let parent = if let Some(dims) = &hier_dims {
                if dims.len() > 1 {
                    rec.dims
                        .iter()
                        .filter(|(k, _)| dims.first().map(|d| d == k).unwrap_or(false))
                        .map(|(k, v)| format!("{k}={v}"))
                        .collect::<Vec<_>>()
                        .join(";")
                } else {
                    String::new()
                }
            } else {
                String::new()
            };
            json!({
                "dimKey": dim_key,
                "label": rec.label,
                "parentDimKey": parent,
                "levelNo": 1,
                "seq": i as i64,
                "cells": {},
            })
        })
        .collect();

    let kind = if is_col { FloatKind::Col } else { FloatKind::Row };
    let n = seed_upsert(code, kind, &loc, &items, &data_source, overwrite).await?;
    Ok(json!({
        "ok": true,
        "kind": if is_col { "col" } else { "row" },
        "seeded": n,
        "dataSource": data_source,
    }))
}

/// 读某区域定义的 data_source（cr_report_region）。供 seed 缺省数据源用。
async fn region_data_source(
    code: &str,
    version: &str,
    sheet: &str,
    region: &str,
) -> Result<String> {
    let rows = query_rows(
        "SELECT data_source FROM cr_report_region \
         WHERE report_code=$1 AND version_code=$2 AND sheet_code=$3 AND region_code=$4",
        dv![code, version, sheet, region],
        "rpt_region_src",
    )
    .await?;
    Ok(rows
        .first()
        .and_then(|r| r.get("data_source"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string())
}

/// 存数：批量 UPSERT cr_cell_data（8元唯一键幂等），自管事务。返回 { ok, saved }。
pub async fn save_data(code: &str, body: &Value) -> Result<Value> {
    let version = s(body, "version").unwrap_or_default();
    let org = s(body, "orgCode").unwrap_or_default();
    let period = s(body, "periodCode").unwrap_or_default();
    let cells = arr(body, "cells").to_vec();

    let mm = get_default_pg_db_manager();
    let tx = mm.get_transaction_context();
    let txn_id = tx
        .begin(RPT_DB_ID)
        .await
        .map_err(|e| api_err(&format!("开启事务失败: {e}")))?;

    let result = save_data_apply(&txn_id, code, &version, &org, &period, &cells).await;
    match result {
        Ok(n) => {
            tx.commit(&txn_id)
                .await
                .map_err(|e| api_err(&format!("提交事务失败: {e}")))?;
            Ok(json!({ "ok": true, "saved": n }))
        }
        Err(e) => {
            let _ = tx.rollback(&txn_id).await;
            Err(e)
        }
    }
}

async fn save_data_apply(
    txn_id: &str,
    code: &str,
    version: &str,
    org: &str,
    period: &str,
    cells: &[Value],
) -> Result<usize> {
    for cell in cells {
        let region_code = s(cell, "regionCode")
            .filter(|c| !c.is_empty())
            .unwrap_or_else(|| DEFAULT_REGION.to_string());
        // 数值：DataValue::Decimal（前端传字符串数值）；空→Null。
        let num_val = cell
            .get("numValue")
            .and_then(|v| match v {
                Value::String(x) if !x.is_empty() => x.parse::<rust_decimal::Decimal>().ok(),
                Value::Number(n) => n.as_f64().and_then(rust_decimal::Decimal::from_f64_retain),
                _ => None,
            })
            .map(DataValue::Decimal)
            .unwrap_or(DataValue::NullTyped(SqlTypeMarker::Decimal));

        exec_dv(
            txn_id,
            r#"INSERT INTO cr_cell_data
               (id, org_code, period_code, report_code, version_code, sheet_code, region_code,
                row_id, col_id, cell_ref, element_code, value_type, text_value, num_value,
                currency_code, amount_unit, data_status, is_manual, compute_time, sort_no, status,
                create_time, update_time)
               VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,CURRENT_TIMESTAMP,0,1,CURRENT_TIMESTAMP,CURRENT_TIMESTAMP)
               ON CONFLICT (org_code, period_code, report_code, version_code, sheet_code, region_code, row_id, col_id)
               DO UPDATE SET cell_ref=EXCLUDED.cell_ref, element_code=EXCLUDED.element_code,
                 value_type=EXCLUDED.value_type, text_value=EXCLUDED.text_value,
                 num_value=EXCLUDED.num_value, currency_code=EXCLUDED.currency_code,
                 amount_unit=EXCLUDED.amount_unit, data_status=EXCLUDED.data_status,
                 is_manual=EXCLUDED.is_manual, compute_time=CURRENT_TIMESTAMP,
                 update_time=CURRENT_TIMESTAMP"#,
            vec![
                DataValue::Int(cmx_utils::next_pk_id()),
                DataValue::String(org.to_string()),
                DataValue::String(period.to_string()),
                DataValue::String(code.to_string()),
                DataValue::String(version.to_string()),
                dv_str(s(cell, "sheetCode").as_deref()),
                DataValue::String(region_code),
                dv_i64_def(i(cell, "rowId"), 0),
                dv_i64_def(i(cell, "colId"), 0),
                dv_str(s(cell, "cellRef").as_deref()),
                dv_str(s(cell, "elementCode").as_deref()),
                dv_str(s(cell, "valueType").as_deref()),
                dv_str(s(cell, "textValue").as_deref()),
                num_val,
                dv_str(s(cell, "currencyCode").as_deref()),
                dv_str(s(cell, "amountUnit").as_deref()),
                dv_str_def(s(cell, "dataStatus").as_deref(), "manual"),
                dv_i64_def(i(cell, "isManual"), 1),
            ],
        )
        .await?;
    }
    Ok(cells.len())
}
