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
use cmx_rpt_app::dashboard;
use cmx_rpt_app::report_routes;
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
    let api_router = report_routes::<()>()
        .route("/rpt/stats", get(dashboard::rpt_stats))
        // F2：报表微服务自持前端页只读投递（native + html），字节对齐门户信封，
        // 供门户 F3 反代 report 拥有的页面取页请求；独立运行时也能自投递自己的界面。
        // 资产目录遵循规范 v2（relPath 相对 index.json）；信封直用 cmx-api-types。
        .merge(cmx_form::serve::frontend_pages_routes::<(), cmx_api_types::Error>(
            cmx_form::serve::PageServeConfig::from_assets(),
        ))
        // 合并报表:方案/范围/个别数/规则/往来录入 + 运行合并 + 工作底稿/合并分类账查询。
        .merge(cmx_rpt_app::consol_routes::<()>())
        // 可观测中间件：采集每请求 method/path/协议/状态/耗时，喂 /_mon 请求遥测面板。
        .layer(axum::middleware::from_fn(cmx_web_monitor::observe));
    let app_router = Router::new()
        // 根路径 → 报表业务监控大盘（报表/分类/期间；免认证，轮询 /api/rpt/stats）。
        .route("/", get(dashboard::dashboard))
        .nest("/api", api_router);

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
                if base.databases.is_empty() {
                    return Err(anyhow::anyhow!(
                        "report-server.toml 未配置 [[databases]]（需 db_id=\"{RPT_DB_ID}\" 且 default=true 的库）"
                    ));
                }
                if !base.databases.iter().any(|d| d.db_id == RPT_DB_ID) {
                    return Err(anyhow::anyhow!(
                        "[[databases]] 缺少 db_id=\"{RPT_DB_ID}\"（报表 store 全局查询按该 db_id 寻址）"
                    ));
                }
                let ids: Vec<&str> = base.databases.iter().map(|d| d.db_id.as_str()).collect();
                cmx_service_base::register_pg_datasources(&base.databases)
                    .await
                    .map_err(|e| anyhow::anyhow!("注册数据源失败: {e}"))?;
                tracing::info!(databases = ?ids, "✅ 报表 tokio-pg 数据源已注册（[[databases]] 配置驱动）");
                Ok(())
            })
        });

    let result = run(spec).await;
    // serve 结束（收到关闭信号或自然退出）：注销注册中心实例后再返回——不用 `?` 提前返回，
    // 否则 Err 路径会跳过注销（实例要等 Nacos 心跳超时才摘除）。
    cmx_service_base::shutdown_infra().await;
    result
}
