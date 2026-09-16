//! report 路由模块适配器——`ModuleRoutes<S>` 契约接入（bin 组合根消费）。
//!
//! [`crate::report_routes`] / [`crate::consol_routes`] 自由函数仍是路由真源（平台壳
//! `cmx-rpt-api` 以 `S = CmxAppState` 直调），本模块仅以 unit struct 适配为契约模块，
//! 供独立壳 bin 以 [`crate::ModuleSet`] 组合（去重守卫 + 统一 fold）。

use axum::Router;
use axum::routing::get;

use cmx_engine_kit::routes::ModuleRoutes;

use crate::dashboard;
use crate::{consol_routes, report_routes};

/// 报表设计/应用工作台 + 计算（`/report-design/*`、`/report-source-bindings*`、`/rpt/compute`）。
pub struct ReportCoreModule;

impl<S: Clone + Send + Sync + 'static> ModuleRoutes<S> for ReportCoreModule {
    fn routes(&self) -> Router<S> {
        report_routes::<S>()
    }

    fn prefix(&self) -> &'static str {
        "/report-design*|/report-source-bindings*|/rpt/compute"
    }

    fn module_name(&self) -> &'static str {
        "rpt.core"
    }
}

/// 合并报表域（`/consol/*`：方案/范围/个别数/规则/往来录入 + 运行合并 + 工作底稿/合并分类账查询）。
pub struct ConsolModule;

impl<S: Clone + Send + Sync + 'static> ModuleRoutes<S> for ConsolModule {
    fn routes(&self) -> Router<S> {
        consol_routes::<S>()
    }

    fn prefix(&self) -> &'static str {
        "/consol/*"
    }

    fn module_name(&self) -> &'static str {
        "rpt.consol"
    }
}

/// 大盘数据源（`/rpt/stats`）——现状挂 open 切片免认证（根大盘轮询，7 引擎唯一免认证 stats）。
pub struct RptStatsModule;

impl<S: Clone + Send + Sync + 'static> ModuleRoutes<S> for RptStatsModule {
    fn routes(&self) -> Router<S> {
        Router::new().route("/rpt/stats", get(dashboard::rpt_stats))
    }

    fn prefix(&self) -> &'static str {
        "/rpt/stats"
    }

    fn module_name(&self) -> &'static str {
        "rpt.stats"
    }
}
