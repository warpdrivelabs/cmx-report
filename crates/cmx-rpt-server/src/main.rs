/*
 * cmx-rpt 独立报表平台微服务 HTTP 服务器（对标 cmx-flow-server）。
 *
 * 采用通用骨架 cmx-web-chassis：与 flow-server / mdm-server 同一套启动/日志/中间件/优雅关闭/
 * banner。main 只填 ServiceSpec——report 路由 + 监控大盘 + 一个启动钩子（注册报表数据源）+
 * report 专属 banner/配色，交 chassis::run 装配。零 cmx-api 依赖。
 *
 * 配置（chassis 框架级用 RPT_ 前缀；数据源用 RPT_PG_URL）：
 *   RPT_HOST / RPT_PORT（默认 0.0.0.0:8092）/ RPT_LOG_DIR / RPT_LOG_LEVEL / RPT_CONFIG(toml)
 *   RPT_PG_URL（报表数据源，指向 fico-db；db_id 固定 = cmx_rpt_model::RPT_DB_ID）
 *
 * 用法：
 *   RPT_PG_URL=postgres://postgres:postgres@127.0.0.1:5432/fico cargo run -p cmx-rpt-server
 *   curl http://127.0.0.1:8092/api/report-design/reports
 */

use axum::Router;
use axum::routing::get;
use cmx_database_pg::{DbConfig, DbType};
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

/// report-server.toml 里的数据源段（可选；缺省即不注入，回退 RPT_PG_URL/内置默认）。
///
/// 背景：数据源 URL 经 RPT_PG_URL 环境变量读取，chassis 的 toml 只解析框架级字段（端口/日志），
/// 不碰数据源。故这里补一层：把 toml 的 `[datasource]` 段读出，在起服前 `set_var` 注入
/// RPT_PG_URL——**仅当该 env 未由外部显式设置时**，保持既有「环境变量覆盖 toml」优先级。
#[derive(serde::Deserialize, Default)]
struct ReportFileConfig {
    #[serde(default)]
    datasource: DatasourceSection,
}

#[derive(serde::Deserialize, Default)]
struct DatasourceSection {
    rpt_pg_url: Option<String>, // → RPT_PG_URL
}

/// 读 report-server.toml 的 `[datasource]` 段，注入 RPT_PG_URL。
/// 路径来源与 chassis 统一：`CONFIG_FILE`（与门户/flow 一致）→ 回退 `RPT_CONFIG` → 默认 `report-server.toml`。
/// env 已显式设置的键不覆盖（env 优先）。
fn apply_toml_env() {
    let path = std::env::var("CONFIG_FILE")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| std::env::var("RPT_CONFIG").ok().filter(|s| !s.trim().is_empty()))
        .unwrap_or_else(|| "report-server.toml".to_string());
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(_) => return, // 文件不存在：跳过，回退纯环境变量/默认
    };
    let file: ReportFileConfig = match toml::from_str(&text) {
        Ok(f) => f,
        Err(e) => {
            tracing::warn!(path = %path, error = %e, "report-server.toml 解析失败，数据源段忽略，回退环境变量");
            return;
        }
    };
    // env 未设时才注入（保持「环境变量覆盖 toml」优先级）。
    if let Some(v) = file.datasource.rpt_pg_url {
        if !v.trim().is_empty() && std::env::var("RPT_PG_URL").is_err() {
            // SAFETY: 启动早期、单线程、任何请求/数据源初始化之前设置进程环境变量。
            unsafe { std::env::set_var("RPT_PG_URL", v) }
        }
    }
}

#[tokio::main]
async fn main() -> cmx_web_chassis::Result<()> {
    // 统一启动契约（与门户/flow 一致）：自动读 cwd 的 .env → RPT_* 环境变量。
    // 必须在 ChassisConfig::load / apply_toml_env（都读 env）之前，故置于 main 首行。
    dotenvy::dotenv().ok();

    // 全局 ConfigManager 装配（**所有能力中心共用的唯一那段代码**，在 cmx-service-base）：
    // CONFIG_FILE 指定的 toml + env → ConfigManager::global()。report/portal/flow/mdm 同一制度，
    // 不再各写一套。非致命：配置源缺失只 warn（各处仍有 env/默认兜底），不阻塞启动。
    if let Err(e) = cmx_service_base::init_config_manager() {
        tracing::warn!(error = %e, "全局 ConfigManager 初始化失败，回退 env/默认兜底");
    }

    // 框架级配置：RPT_ 前缀环境变量 + 可选 report-server.toml，默认端口 8092。
    let mut cfg = ChassisConfig::load("report", "RPT", "report-server.toml");
    // 数据源：读同一 toml 的 [datasource] 段 → set_var 注入 RPT_PG_URL（env 未设时）。
    apply_toml_env();
    if std::env::var("RPT_PORT").is_err() && cfg.port == 8080 {
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
        .merge(cmx_rpt_app::native_pages::frontend_pages_routes::<()>())
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
        // 钩子：注册报表数据源（db_id 固定 = RPT_DB_ID = "fico-db"，否则 store 全局查询找不到源）。
        // 经 cmx-service-base 共享注册原语（与 flow/portal 同一 register_pg_datasources）。
        // URL 从 RPT_PG_URL env 读（带默认兜底）。
        .init("datasources", |_meta| {
            Box::pin(async {
                let rpt_url = std::env::var("RPT_PG_URL").unwrap_or_else(|_| {
                    "postgres://postgres:postgres@127.0.0.1:5432/fico".to_string()
                });
                let configs = vec![rpt_db_config(RPT_DB_ID, &rpt_url, true)];
                cmx_service_base::register_pg_datasources(&configs)
                    .await
                    .map_err(|e| anyhow::anyhow!("注册数据源失败: {e}"))?;
                tracing::info!(rpt_db = RPT_DB_ID, "✅ 报表数据源已注册");
                Ok(())
            })
        });

    run(spec).await
}

/// 构造报表 PG 数据源配置（db_id 固定 RPT_DB_ID，url 从 env 来）。注册交 cmx-service-base 的共享
/// `register_pg_datasources` 原语。
fn rpt_db_config(db_id: &str, url: &str, default: bool) -> DbConfig {
    DbConfig {
        db_type: DbType::Postgres,
        db_url: url.to_string(),
        db_id: db_id.to_string(),
        db_name: None,
        db_schema: Some("public".to_string()),
        default,
        pool_config: Default::default(),
        health_check_interval: 60,
        health_check_timeout: 5,
        domain_code: None,
        application_code: None,
        module_code: None,
        source_type: Some("default".to_string()),
    }
}
