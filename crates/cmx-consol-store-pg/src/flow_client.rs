//! flow_client —— 关账编排对接独立 cmx-flow-server 的**契约 SDK 客户端**(目录键 gated)。
//!
//! ★ 调用经 `cmx-flow-sdk`（路径常量 v1 + wire DTO）+ `cmx-service-rpc` 基座
//!   （`[service_rpc.services].flow` 定位 url/服务发现 + 统一鉴权链 + 超时/重试/熔断）——
//!   零编译期 flow 依赖,报表微服务与流程微服务各自独立部署。
//!
//! ★ 触发闸门:仅当服务目录配置了 `flow` 键(`[service_rpc.services.flow]`)才对接 flow;
//!   未配则 `enabled()==false`,关账退化为纯服务内顺序编排(不起流程实例)。
//!
//! ★ 鉴权(行为变化,1b):旧版直设 `X-User`/`X-Tenant` 头——该二头**仅在 flow 的
//!   `auth.mode=off` 时生效**(jwt 模式下被 X-API-Key 服务身份路径忽略,实测为无效头);
//!   现统一走基座鉴权链(`X-API-Key` + 委托令牌 + 请求 ID)。发起人身份经
//!   `variables.initiator` 显式携带(原 `FLOW_USER` 语义,迁 `[consol.flow].initiator`)。
//!
//! ★ 成功判据(行为修正,1b):旧版仅判 `code != 1`(HTTP 200 + code=400 等会被误判成功);
//!   现按标准信封严格判 `code == 0`。

use cmx_flow_sdk::{BizLink, StartInstanceReq};
use cmx_service_rpc::ServiceRpcError;
use cmx_utils::ConfigManager;

/// flow 对接配置。目录未配置 `flow` 键 → `enabled()==false`。
pub struct FlowClient {
    /// 关账流程定义 key(`consol.flow.definition_key`,缺省 consol_close)。
    definition_key: String,
    /// 发起人身份(`consol.flow.initiator`,缺省 consol-close;进 variables.initiator)。
    initiator: String,
}

impl FlowClient {
    /// 从服务配置构造(`[consol.flow]` 段;基座目录读全局单例)。
    pub fn from_config() -> Self {
        let mut cfg = Self {
            definition_key: "consol_close".to_string(),
            initiator: "consol-close".to_string(),
        };
        if let Some(cm) = ConfigManager::try_global() {
            if let Ok(v) = cm.get_string("consol.flow.definition_key")
                && !v.trim().is_empty()
            {
                cfg.definition_key = v.trim().to_string();
            }
            if let Ok(v) = cm.get_string("consol.flow.initiator")
                && !v.trim().is_empty()
            {
                cfg.initiator = v.trim().to_string();
            }
        }
        cfg
    }

    /// 是否对接 flow(服务目录已配置 flow 键)。
    pub fn enabled(&self) -> bool {
        cmx_service_rpc::global().is_some_and(|h| h.directory().contains("flow"))
    }

    /// 起一个关账流程实例,绑定 cg_close_run 单据(bizLink)。返回 flow 实例 id(失败返 Err)。
    /// definitionKey=consol_close(可 `[consol.flow].definition_key` 覆盖);businessKey=run_code。
    pub async fn start_close_instance(
        &self,
        run_code: &str,
        scheme: &str,
        period: &str,
    ) -> Result<String, String> {
        if !self.enabled() {
            return Err("flow 未启用([service_rpc.services.flow] 未配置)".into());
        }
        let client = cmx_flow_sdk::client().map_err(|e| e.to_string())?;
        let resp = client
            .start_instance(
                StartInstanceReq {
                    definition_key: self.definition_key.clone(),
                    business_key: Some(run_code.to_string()),
                    variables: Some(serde_json::json!({
                        "scheme": scheme,
                        "period": period,
                        "initiator": self.initiator,
                    })),
                    biz_link: Some(BizLink {
                        biz_table: "cg_close_run".to_string(),
                        biz_id: run_code.to_string(),
                        biz_key: Some(run_code.to_string()),
                        role: "close_run".to_string(),
                    }),
                    ..Default::default()
                },
                None,
            )
            .await
            .map_err(map_err)?;
        Ok(resp.id)
    }
}

/// 错误文案对齐旧版口径("flow 起实例被拒: {msg}")。
fn map_err(e: ServiceRpcError) -> String {
    format!("flow 起实例被拒: {e}")
}
