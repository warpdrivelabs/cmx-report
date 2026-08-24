//! cmx-rpt-app —— 报表模块的**平台中立应用层**（对标 cmx-flow-app）。
//!
//! 暴露 [`report_routes`]`::<S>()`：一张对任意 axum state 泛型 `S` 成立的报表路由表
//! （全部 handler 只带 Path/Query/Json 提取器，不取 state；DB 走 store 层全局 pg 管理器）。
//! 两壳复用同一核，零 handler 漂移：
//!   - 平台壳 `cmx-rpt-api`（留 cmx-container）：`ReportModule::routes()` 调 `report_routes::<CmxAppState>()`。
//!   - 独立壳 `cmx-rpt-server`（本 workspace）：main 里 `merge(report_routes::<()>())` + 大盘 + 数据源钩子。
//!
//! 端点路径与迁移前完全一致（`/report-design/*`、`/report-source-bindings*`、`/rpt/compute`；
//! `/api` 前缀由宿主 nest 加）。信封类型来自 cmx-api-types（store 本就返回 `cmx_api_types::Result`）。

pub mod dashboard;
pub mod handlers;

use axum::Router;
use axum::routing::{get, post};

/// 报表路由表（对任意 state 泛型 `S` 成立）。宿主 `merge` 或 `nest("/api", …)` 之。
pub mod consol;
pub use consol::consol_routes;

pub fn report_routes<S>() -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    Router::new()
        // 报表设计工作台：全部数据来自 fico-db 物理表。
        .route("/report-design/overview", get(handlers::report_design_overview))
        .route(
            "/report-design/reports",
            get(handlers::report_design_reports).post(handlers::report_design_create_report),
        )
        .route("/report-design/elements", get(handlers::report_design_elements))
        // 报表应用工作台：会计日历（期间）+ 合并组织架构（组织树）读服务，来自 fico-db。
        .route("/report-design/calendar", get(handlers::report_design_calendar))
        .route("/report-design/consol-org", get(handlers::report_design_consol_org))
        .route(
            "/report-design/reports/{code}",
            get(handlers::report_design_report_detail)
                .delete(handlers::report_design_delete_report),
        )
        .route(
            "/report-design/reports/{code}/versions",
            post(handlers::report_design_create_version),
        )
        .route(
            "/report-design/reports/{code}/versions/{version}/default",
            post(handlers::report_design_set_default_version),
        )
        // 报表两模式加载存储（ReportModel 单一事实源）：
        //   layout = 设计版式（cr_report_fmt BLOB + 关系投影 sheet/region/row/col）
        //   data   = 报表数据（cr_cell_data 按 org+period，读走 ZmcDataSet 零拷贝）
        .route(
            "/report-design/reports/{code}/layout",
            get(handlers::report_design_load_layout).post(handlers::report_design_save_layout),
        )
        .route(
            "/report-design/reports/{code}/data/query",
            post(handlers::report_design_query_data),
        )
        // 打开报表：一次调用取全集（版式+cellMap+元素+函数[+数据]），替代前端顺序多调。
        .route(
            "/report-design/reports/{code}/open",
            post(handlers::report_design_open_report),
        )
        // 打开并展开：浮动行列（is_repeatable=1 区域的模板行）按数据源展开成 N 条实例行。
        .route(
            "/report-design/reports/{code}/expand",
            post(handlers::report_design_expand_report),
        )
        // 浮动行/列存储态 CRUD（F2/F3）：cr_report_float_row/col 的增删改查 + 取数初始化。
        .route(
            "/report-design/reports/{code}/float/rows/query",
            post(handlers::report_float_rows_query),
        )
        .route(
            "/report-design/reports/{code}/float/rows",
            post(handlers::report_float_rows_save),
        )
        .route(
            "/report-design/reports/{code}/float/rows/{id}",
            axum::routing::delete(handlers::report_float_rows_delete),
        )
        .route(
            "/report-design/reports/{code}/float/cols/query",
            post(handlers::report_float_cols_query),
        )
        .route(
            "/report-design/reports/{code}/float/cols",
            post(handlers::report_float_cols_save),
        )
        .route(
            "/report-design/reports/{code}/float/cols/{id}",
            axum::routing::delete(handlers::report_float_cols_delete),
        )
        .route(
            "/report-design/reports/{code}/float/seed",
            post(handlers::report_float_seed),
        )
        .route(
            "/report-design/reports/{code}/data",
            post(handlers::report_design_save_data),
        )
        // 计算态：真算（装载公式→递归求值→落 cr_cell_data）+ 函数目录（设计器向导）。
        .route(
            "/report-design/reports/{code}/compute",
            post(handlers::report_design_compute),
        )
        .route("/report-design/functions", get(handlers::report_design_functions))
        // 协同编辑 B 档：语义操作提交（POST）+ 增量拉取追平（GET ?version=&since=）。
        .route(
            "/report-design/reports/{code}/ops",
            get(handlers::report_design_list_ops).post(handlers::report_design_apply_ops),
        )
        // 计算路由设置：报表取数路由绑定（cr_report_source_binding）注册 CRUD。
        //   list 用 {key}，delete 用独立 /id/{id} 子路径避免与 {key} 路由歧义。
        .route(
            "/report-source-bindings",
            post(handlers::upsert_report_source_binding),
        )
        .route(
            "/report-source-bindings/{key}",
            get(handlers::list_report_source_bindings),
        )
        .route(
            "/report-source-bindings/id/{id}",
            axum::routing::delete(handlers::delete_report_source_binding),
        )
        // 旧 html 报表设计器预览兼容接口；新工作台不使用。
        .route("/rpt/compute", post(handlers::rpt_compute))
}
