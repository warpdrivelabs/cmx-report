/*
 * cmx-rpt 独立报表平台微服务 HTTP 服务器（对标 cmx-flow-server）。
 *
 * 采用通用骨架 cmx-web-chassis：与 flow-server / mdm-server 同一套启动/日志/中间件/优雅关闭/
 * banner。main 只填 ServiceSpec——report 路由 + 监控大盘 + 一个启动钩子（注册报表数据源）+
 * report 专属 banner/配色，交 chassis::run 装配。零 cmx-api 依赖。
 *
 * 配置（report-server.toml，路径由 CONFIG_FILE 指定；[server] 框架键 env 覆盖 SERVER__*，与 ConfigManager `__` 约定同名）：
 *   [server] host/port/log_dir/log_level/graceful_timeout_secs（默认 0.0.0.0:8092）
 *   [[databases]] 标准数据源段（db_id 固定 = cmx_rpt_model::RPT_DB_ID = "fico-db"，default=true；
 *   缺段启动失败，无内置 URL 兜底）
 *
 * 用法：
 *   cargo run -p cmx-rpt-server   # 读 cwd 的 report-server.toml（或 CONFIG_FILE 指定）
 *   curl http://127.0.0.1:8092/api/report-design/reports
 */

use axum::Router;
use axum::routing::get;
use cmx_form::serve::{FormPagesModule, PageServeConfig};
use cmx_rpt_app::dashboard;
use cmx_rpt_app::{ConsolModule, ModuleSet, ReportCoreModule, RptStatsModule};
use cmx_rpt_model::RPT_DB_ID;
use cmx_web_chassis::{BannerSpec, ChassisConfig, ServiceSpec, run};

/// report 专属字符画（MEGA REPORT，区别于平台/流程/默认 banner）。
const REPORT_ART: &str = r#"
███╗   ███╗███████╗ ██████╗  █████╗     ██████╗ ███████╗██████╗  ██████╗ ██████╗ ████████╗
████╗ ████║██╔════╝██╔════╝ ██╔══██╗    ██╔══██╗██╔════╝██╔══██╗██╔═══██╗██╔══██╗╚══██╔══╝
██╔████╔██║█████╗  ██║  ███╗███████║    ██████╔╝█████╗  ██████╔╝██║   ██║██████╔╝   ██║
██║╚██╔╝██║██╔══╝  ██║   ██║██╔══██║    ██╔══██╗██╔══╝  ██╔═══╝ ██║   ██║██╔══██╗   ██║
██║ ╚═╝ ██║███████╗╚██████╔╝██║  ██║    ██║  ██║███████╗██║     ╚██████╔╝██║  ██║   ██║
╚═╝     ╚═╝╚══════╝ ╚═════╝ ╚═╝  ╚═╝    ╚═╝  ╚═╝╚══════╝╚═╝      ╚═════╝ ╚═╝  ╚═╝   ╚═╝
"#;

#[tokio::main]
async fn main() -> cmx_web_chassis::Result<()> {
    // 统一启动契约（与门户/flow 一致）：自动读 cwd 的 .env（CONFIG_FILE 等）。
    // 必须在 ChassisConfig::load / init_infra（都读 env）之前，故置于 main 首行。
    dotenvy::dotenv().ok();

    // 基础设施装配（与门户 run_platform 同一制度）：本地 toml ← Nacos 远程配置中心 ← env
    // 三源 ConfigManager + 注册中心客户端（自注册 + 实例缓存 + 30s 服务列表同步）。开关默认
    // 全关（未开 NACOS_ENABLED 时走 Mock，纯本地 toml+env，行为与接入前一致）；开启后
    // create 阶段强依赖 Nacos 可达，失败即中止启动（register 阶段失败仅 warn）。
    cmx_service_base::init_infra()
        .await
        .map_err(|e| cmx_web_chassis::ChassisError::Config(format!("基础设施初始化失败: {e}")))?;

    // 框架级配置：[server] 段 + SERVER__* env 覆盖（与 ConfigManager `__` 约定同名）+ 可选 report-server.toml，默认端口 8092。
    let mut cfg = ChassisConfig::load("report", "report-server.toml");
    if std::env::var("SERVER__PORT").is_err() && cfg.port == 8080 {
        cfg.port = 8092; // 未显式配端口时用 report 默认（避开平台 8080 / demo 8090 / flow 8091）。
    }

    // report 专属 banner：靛蓝 → 品红 渐变。
    let banner = BannerSpec::defaults("report")
        .art(REPORT_ART)
        .tagline("  MEGA Report · 报表平台微服务 · cmx-web-chassis ")
        .stops(vec![(60, 110, 255), (150, 80, 255), (255, 80, 180)]);

    // 路由：
    //   - 根路径 /  → 报表监控大盘 HTML（自包含单页，轮询 /api/rpt/stats）。
    //   - /api/report-design/*、/api/report-source-bindings*、/api/rpt/compute（URL 与迁移前一致）。
    //   - /api/rpt/stats（大盘数据源）。
    //
    // chassis 默认把 router nest 到 /api 下；这里改用 nest_api(false) 自己 nest，好让根大盘 `/` 逃出 /api。
    // 模块化装配（authed / open 双切片，open 现状带 observe 层）见 [`build_app_router`]，与契约测试共用。
    let app_router = build_app_router();

    // 通用技术监控：/_mon 技术页 + 系统采样器由 chassis 自动挂。这里设服务名 + 声明拓扑
    // （独立 report-server 自身即报表平台，能力为「进程内内嵌」，无下游反代）。
    cmx_web_monitor::set_service_name("cmx-rpt 报表平台");
    cmx_web_monitor::set_topology_provider(|| {
        vec![cmx_web_monitor::ServiceDep {
            key: "report".into(),
            label: "报表平台".into(),
            mode: "embedded".into(),
            target: None,
            proxiable: false,
        }]
    });

    let spec = ServiceSpec::<()>::new("report", cfg)
        .banner(banner)
        .nest_api(false) // 已自行 nest /api，避免 chassis 再包一层。
        .router(app_router)
        .state(())
        // 钩子：注册报表数据源——平台封装：BaseConfig（标准 [[databases]] 段，ConfigManager 三源
        // 合并）+ 共享注册原语 register_pg_datasources。要求 db_id = RPT_DB_ID = "fico-db"
        //（store 全局查询按该 db_id 寻址）；缺段 / 缺 db_id 启动失败（无内置 URL 兜底）。
        .init("datasources", |_meta| {
            Box::pin(async {
                let base = cmx_service_base::BaseConfig::from_config_manager()
                    .map_err(|e| anyhow::anyhow!("读取 [[databases]] 配置失败: {e}"))?;
                cmx_service_base::validate_databases(
                    &base.databases,
                    &cmx_service_base::DatasourceRules {
                        required_db_ids: &[RPT_DB_ID],
                        ..Default::default()
                    },
                )
                .map_err(|e| anyhow::anyhow!("数据源校验失败（需 db_id=\"{RPT_DB_ID}\"，报表 store 全局查询按该 db_id 寻址）: {e}"))?;
                let ids: Vec<&str> = base.databases.iter().map(|d| d.db_id.as_str()).collect();
                cmx_service_base::register_pg_datasources(&base.databases)
                    .await
                    .map_err(|e| anyhow::anyhow!("注册数据源失败: {e}"))?;
                tracing::info!(databases = ?ids, "✅ 报表 tokio-pg 数据源已注册（[[databases]] 配置驱动）");
                Ok(())
            })
        })
        // 认证预热（fail-fast）：ConfigManager 就绪后校验 [auth].mode——缺失/非法启动即 panic 终止，
        // 而非等首个 authed 请求才在中间件里 panic（表现为连接重置 000，极难定位）。与 flow 同款。
        .init("auth", |_meta| {
            Box::pin(async {
                cmx_rpt_app::auth_config_warmup();
                Ok(())
            })
        });

    let result = run(spec).await;
    // serve 结束（收到关闭信号或自然退出）：注销注册中心实例后再返回——不用 `?` 提前返回，
    // 否则 Err 路径会跳过注销（实例要等 Nacos 心跳超时才摘除）。
    cmx_service_base::shutdown_infra().await;
    result
}

// ============================================================================
// bin 组合根装配（模块化）
// ============================================================================

/// authed 切片：报表业务路由（设计/应用工作台 + 计算 + 合并报表域）。
///
/// 返回**未加层**的路由器——main 按现状序「observe（内）→ auth（外）」加层；路由契约
/// 测试直接探测本函数（auth 中间件对无凭证请求统一 401，会掩盖 405/404 区分）。
fn build_authed_router() -> Router {
    ModuleSet::<()>::new(vec![])
        .with(Box::new(ReportCoreModule))
        .with(Box::new(ConsolModule))
        .fold()
}

/// open 切片：大盘数据源 `/rpt/stats`（根大盘轮询，现状免认证）+ 前端页只读投递。
///
/// 与 flow 同款置于 authed 之外；**现状整片带 observe 遥测层，保持**。前端页错误体经
/// `cmx_api_types::Error` 保持历史 code=404 语义。
fn build_open_router() -> Router {
    ModuleSet::<()>::new(vec![])
        .with(Box::new(RptStatsModule))
        .with(Box::new(FormPagesModule::<cmx_api_types::Error>::new(
            PageServeConfig::from_assets(),
        )))
        .fold()
}

/// 全量装配：根级大盘 + `/api`（authed 切片 + open 切片）。
///
/// 中间件层序保持现状：authed 内 observe（内层，采集身份）→ auth（外层，先跑，认证强制
/// jwt + 服务 APIKey，no-key/坏 key→401）；open 整片 observe。
fn build_app_router() -> Router {
    let authed = build_authed_router()
        .layer(axum::middleware::from_fn(cmx_web_monitor::observe))
        .layer(axum::middleware::from_fn(cmx_rpt_app::auth_middleware));
    let open = build_open_router()
        .layer(axum::middleware::from_fn(cmx_web_monitor::observe));
    let api_router = Router::new().merge(authed).merge(open);
    Router::new()
        // 根路径 → 报表业务监控大盘（报表/分类/期间；免认证，轮询 /api/rpt/stats）。
        .route("/", get(dashboard::dashboard))
        .nest("/api", api_router)
}

// ============================================================================
// 路由契约守护（bin 装配级）
// ============================================================================

#[cfg(test)]
mod route_contract {
    //! 静态清单以改造前 main.rs 逐条抄录（改造后不变即零回归）。
    //!
    //! 探测法：以 **OPTIONS** 探测——命中已有路径返回 405（方法不符），未命中 404；
    //! 不触发任何 handler。authed 切片在加层前探测（原因见 [`super::build_authed_router`]）。

    use super::{build_app_router, build_authed_router, build_open_router};
    use axum::Router;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    /// authed 切片（改造前 main.rs 挂载清单抽样：report-design / report-source-bindings / consol / compute）。
    const AUTHED: &[&str] = &[
        "/report-design/reports",
        "/report-source-bindings",
        "/consol/schemes",
        "/rpt/compute",
    ];

    /// open / 根级（改造前 main.rs 挂载清单：大盘、stats、页面端点——参数化段以具体 id 探测）。
    const OPEN_OR_ROOT: &[&str] = &[
        "/",
        "/api/rpt/stats",
        "/api/native-pages",
        "/api/native-pages/batch",
        "/api/native-pages/probe-id",
        "/api/html-pages",
        "/api/html-pages/probe-id",
    ];

    async fn probe(router: Router, method: &str, path: &str) -> StatusCode {
        let req = Request::builder()
            .method(method)
            .uri(path)
            .body(Body::empty())
            .unwrap();
        router.oneshot(req).await.unwrap().status()
    }

    #[tokio::test]
    async fn authed_paths_mounted() {
        let router = build_authed_router();
        for path in AUTHED {
            let status = probe(router.clone(), "OPTIONS", path).await;
            assert_ne!(status, StatusCode::NOT_FOUND, "authed 路径丢失: {path}");
        }
    }

    #[tokio::test]
    async fn open_and_root_paths_mounted() {
        let router = build_app_router();
        for path in OPEN_OR_ROOT {
            let status = probe(router.clone(), "OPTIONS", path).await;
            assert_ne!(status, StatusCode::NOT_FOUND, "open/根级路径丢失: {path}");
        }
    }

    #[tokio::test]
    async fn open_slice_mounts_without_auth_layers() {
        // 防误把 stats/pages 挂进 authed（免认证面被收窄属行为回归）：open 切片自身
        // （不加层）可直接探测到对应端点。
        let router = build_open_router();
        for path in ["/rpt/stats", "/native-pages", "/html-pages"] {
            let status = probe(router.clone(), "OPTIONS", path).await;
            assert_ne!(status, StatusCode::NOT_FOUND, "open 切片路径丢失: {path}");
        }
    }

    #[tokio::test]
    async fn unknown_path_is_404() {
        let router = build_app_router();
        for path in ["/api/__definitely_absent__", "/__definitely_absent__"] {
            let status = probe(router.clone(), "OPTIONS", path).await;
            assert_eq!(status, StatusCode::NOT_FOUND, "未注册路径竟命中: {path}");
        }
    }
}
