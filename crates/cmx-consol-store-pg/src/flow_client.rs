//! flow_client —— 关账编排对接独立 cmx-flow-server 的**极薄 HTTP 客户端**(env-gated)。
//!
//! ★ 零编译期 flow 依赖:本 crate 不引 cmx-flow-*,只用 reqwest 打 flow 的公开 HTTP API
//!   (POST /api/instances 起实例、GET /api/instances/{id} 查状态)。对齐 cmx-flowengine 自身
//!   S1/S6 的「运行时注入、编译期解耦」姿态——报表微服务与流程微服务各自独立部署。
//!
//! ★ 触发闸门:仅当环境变量 `FLOW_BASE_URL` 非空时才对接 flow;未配则 `enabled()==false`,
//!   关账退化为纯服务内顺序编排(不起流程实例)。鉴权头 `X-API-Key`(FLOW_API_KEY)、
//!   `X-User`(FLOW_USER,缺省 consol-close)、`X-Tenant`(FLOW_TENANT,缺省 default)。

use serde_json::{Value, json};

/// flow 对接配置(从环境变量读)。`base` 为空 → 关账不对接 flow。
pub struct FlowClient {
    base: String,
    /// 起实例的完整路径(独立 flow-server 为 /api/flow/instances;demo 为 /api/instances)。
    instances_path: String,
    api_key: Option<String>,
    user: String,
    tenant: String,
    definition_key: String,
}

impl FlowClient {
    /// 从环境变量构造。`FLOW_BASE_URL` 空 → enabled()==false。
    pub fn from_env() -> Self {
        FlowClient {
            base: std::env::var("FLOW_BASE_URL").unwrap_or_default().trim_end_matches('/').to_string(),
            instances_path: std::env::var("FLOW_INSTANCES_PATH").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| "/api/flow/instances".into()),
            api_key: std::env::var("FLOW_API_KEY").ok().filter(|s| !s.is_empty()),
            user: std::env::var("FLOW_USER").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| "consol-close".into()),
            tenant: std::env::var("FLOW_TENANT").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| "default".into()),
            definition_key: std::env::var("FLOW_CLOSE_DEF_KEY").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| "consol_close".into()),
        }
    }

    /// 是否对接 flow(FLOW_BASE_URL 已配)。
    pub fn enabled(&self) -> bool {
        !self.base.is_empty()
    }

    fn apply_headers(&self, rb: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        let mut rb = rb.header("X-User", &self.user).header("X-Tenant", &self.tenant);
        if let Some(k) = &self.api_key {
            rb = rb.header("X-API-Key", k);
        }
        rb
    }

    /// 起一个关账流程实例,绑定 cg_close_run 单据(bizLink)。返回 flow 实例 id(失败返 Err)。
    /// definitionKey=consol_close(可 FLOW_CLOSE_DEF_KEY 覆盖);businessKey=run_code。
    pub async fn start_close_instance(&self, run_code: &str, scheme: &str, period: &str) -> Result<String, String> {
        if !self.enabled() {
            return Err("flow 未启用(FLOW_BASE_URL 未配)".into());
        }
        let url = format!("{}{}", self.base, self.instances_path);
        let body = json!({
            "definitionKey": self.definition_key,
            "businessKey": run_code,
            "variables": { "scheme": scheme, "period": period, "initiator": self.user },
            "bizLink": { "bizTable": "cg_close_run", "bizId": run_code, "bizKey": run_code, "role": "close_run" }
        });
        let client = reqwest::Client::new();
        let resp = self
            .apply_headers(client.post(&url).json(&body))
            .send()
            .await
            .map_err(|e| format!("flow 起实例请求失败: {e}"))?;
        let status = resp.status();
        let v: Value = resp.json().await.map_err(|e| format!("flow 响应解析失败: {e}"))?;
        if !status.is_success() || v.get("code").and_then(|c| c.as_i64()) == Some(1) {
            return Err(format!("flow 起实例被拒: {}", v.get("msg").and_then(|m| m.as_str()).unwrap_or("unknown")));
        }
        // 响应信封 { code, msg, data:{ instanceId | instance_id | id } }。
        let data = v.get("data").cloned().unwrap_or(v);
        let id = data
            .get("instanceId")
            .or_else(|| data.get("instance_id"))
            .or_else(|| data.get("id"))
            .and_then(|x| x.as_str().map(str::to_owned).or_else(|| x.as_i64().map(|n| n.to_string())))
            .ok_or_else(|| "flow 响应缺 instanceId".to_string())?;
        Ok(id)
    }
}
