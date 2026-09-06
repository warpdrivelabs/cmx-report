//! JWT 认证中间件——收编至 `cmx-engine-kit::auth::jwt`（唯一真源，与 flow / onto 同款语义）。
//!
//! report 无 SSE 一次性票据端点，`JwtSpec` 传空白名单、无票据消费回调。模式（off/jwt）、密钥、
//! claim 宽容解析、服务间 API-Key 委托桥等行为契约见真源 `cmx-engine-kit/src/auth/jwt.rs`。
//! report-server / 平台壳各自 `.layer(from_fn(auth))` 挂载。

use axum::extract::Request;
use axum::middleware::Next;
use axum::response::Response;

use cmx_engine_kit::auth::jwt::{self, JwtSpec};

/// 启动期预热认证配置（fail-fast）：`auth.mode` 缺失/非法时启动即 panic 终止，而非等首个
/// authed 请求才在中间件里 panic（表现为连接重置 000，极难定位）。server bin 的 init 钩子调用。
pub use jwt::auth_config_warmup;

/// report 专属 JWT 参数：无 SSE 票据白名单、无票据消费回调。
static SPEC: JwtSpec = JwtSpec::new("report", &[], None);

/// JWT 认证中间件（解析身份 → 建租户 scope → 放行；no-key/坏 key 在 jwt 模式下 401）。
pub async fn auth(req: Request, next: Next) -> Response {
    jwt::auth_mw(req, next, &SPEC).await
}
