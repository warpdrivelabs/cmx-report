//! float_ddl —— 浮动行/列数据表存在性入口（存储态浮动，方案 F1）。
//!
//! 浮动行/列从「运行时派生（每次 expand 即时算）」升级为「可增删改查的存储业务数据」。
//! 两张固定表：`cr_report_float_row`（存浮动行）、`cr_report_float_col`（存浮动列）。
//! 按 org+period 隔离（对齐 cr_cell_data 8 元键），整行/列自包含（cells JSONB 存各列/各行值）。
//!
//! ★ 建表**不在程序中做**：两表已在报表数据字典元数据
//! `data/meta/definitions/fi/cmxfico/report/cmxfico_report_dct_meta_v1.json` 的
//! `dictionaryTables[]` 里声明（dictCode `report_float_row`/`report_float_col`），
//! 由模型中心部署（`POST /api/model/deploy` → `create_or_upgrade_table`，additive-only）
//! 建到 fico-db，与其余 cr_* 表同一套元数据驱动的建表链路。本模块不再执行任何 DDL。
//!
//! 保留 `ensure_float_schema()` 仅作调用点占位（no-op，恒 Ok），使 CRUD/expand 入口无需改动；
//! 表不存在属于「未部署元数据」的部署问题，由部署链路解决，不在运行期悄悄建表。

use crate::Result;

/// no-op：两表由元数据部署创建（见模块文档），程序内不建表。
///
/// 历史上曾用 `CREATE TABLE IF NOT EXISTS` 幂等自建；按「表结构须在元数据中声明、
/// 不能在程序中创建」的要求，改为元数据驱动，本函数不再执行 DDL。
pub async fn ensure_float_schema() -> Result<()> {
    Ok(())
}
