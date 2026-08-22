//! close —— C7-Next N3:合并关账编排(采集→对账→合并→复核→出表)。
//!
//! 关账把「个别数就位 → 内部往来对账 → 逐级合并 → 人工复核门 → 出表」串成一条可追溯、
//! 可回退的状态机,落 `cg_close_run`(运行头)+ `cg_close_step`(每步审计)。复用既有服务
//! `run_ic_reconciliation` / `run_consolidation` / `run_cashflow` / `run_equity_change` /
//! `seed_consol_statements`,不重复实现算法。
//!
//! ★ flow 对接(env-gated):`FLOW_BASE_URL` 配了 → start 时经 [`crate::flow_client`] 起一个真
//!   cmx-flow 流程实例(采集/对账/合并/复核四节点,复核是人工 userTask 进门户待办中心),
//!   实例 id 回填 `cg_close_run.flow_instance_id`;未配 → 纯服务内顺序编排(不起实例)。
//!   无论是否对接 flow,服务内状态机都是关账的真相源(flow 只做人工审批与可视化编排)。
//!
//! 步骤序:collect → reconcile → consolidate → review(人工门) → statements → closed。
//! `advance_close` 每调一次推进到「下一步」;review 步需显式 `POST advance {step:"review", approve:true}`。

use serde_json::{Value, json};

use cmx_core::model::cell::{DataValue, SqlTypeMarker};

use crate::flow_client::FlowClient;
use crate::{Result, api_err, execute, query_rows, sv};

fn pk() -> DataValue {
    DataValue::Int(cmx_utils::next_pk_id())
}

/// 关账步骤序(review 是人工门)。
const STEPS: &[&str] = &["collect", "reconcile", "consolidate", "review", "statements"];

/// run_code 规则:{scheme}-{period}(每方案每期一次关账,幂等)。
fn run_code_of(scheme: &str, period: &str) -> String {
    format!("{scheme}-{period}")
}

/// 发起关账:建 cg_close_run(幂等 UPSERT,状态 collecting),可选起 flow 实例。
pub async fn start_close(scheme: &str, period: &str) -> Result<Value> {
    if scheme.is_empty() || period.is_empty() {
        return Err(api_err("scheme/period 不能为空"));
    }
    // 校验合并范围在位(采集前置)。
    let scope = query_rows(
        "SELECT org_code FROM cg_scope WHERE scheme_code=$1 AND period_code=$2 AND COALESCE(status,1)=1 LIMIT 1",
        vec![DataValue::String(scheme.to_string()), DataValue::String(period.to_string())],
        "close_scope_check",
    )
    .await?;
    if scope.is_empty() {
        return Err(api_err("该方案该期间未配置合并范围,无法发起关账"));
    }

    let run_code = run_code_of(scheme, period);
    // 起 flow 实例(env-gated;失败不阻断关账,仅记 warn)。
    let fc = FlowClient::from_env();
    let mut flow_instance_id: Option<String> = None;
    if fc.enabled() {
        match fc.start_close_instance(&run_code, scheme, period).await {
            Ok(id) => flow_instance_id = Some(id),
            Err(e) => tracing::warn!(target: "consol::close", run_code=%run_code, err=%e, "起 flow 关账实例失败,退化为服务内编排"),
        }
    }

    execute(
        "INSERT INTO cg_close_run (id, code, name, scheme_code, period_code, run_code, run_status, current_step, \
            flow_instance_id, started_at, sort_no, status, create_time, update_time) \
         VALUES ($1,$2,$3,$4,$5,$6,'collecting','collect',$7,CURRENT_TIMESTAMP,0,1,CURRENT_TIMESTAMP,CURRENT_TIMESTAMP) \
         ON CONFLICT (scheme_code, period_code) DO UPDATE SET run_status='collecting', current_step='collect', \
            flow_instance_id=COALESCE(EXCLUDED.flow_instance_id, cg_close_run.flow_instance_id), \
            started_at=CURRENT_TIMESTAMP, closed_at=NULL, update_time=CURRENT_TIMESTAMP",
        vec![
            pk(),
            DataValue::String(run_code.clone()),
            DataValue::String(format!("{scheme} {period} 关账")),
            DataValue::String(scheme.to_string()),
            DataValue::String(period.to_string()),
            DataValue::String(run_code.clone()),
            flow_instance_id.clone().map(DataValue::String).unwrap_or(DataValue::NullTyped(SqlTypeMarker::Text)),
        ],
    )
    .await?;
    // 清旧步骤审计(幂等重跑)。
    execute(
        "DELETE FROM cg_close_step WHERE run_code=$1",
        vec![DataValue::String(run_code.clone())],
    )
    .await?;
    record_step(&run_code, scheme, period, "collect", "pending", &json!({"note":"关账已发起,等待推进"})).await?;

    Ok(json!({
        "ok": true, "scheme": scheme, "period": period, "runCode": run_code,
        "flowEnabled": fc.enabled(), "flowInstanceId": flow_instance_id,
        "runStatus": "collecting", "currentStep": "collect",
        "message": format!("关账已发起{}", if flow_instance_id.is_some() { "(flow 实例已起)" } else { "(服务内编排)" }),
    }))
}

/// 推进关账一步。`step` 指定要执行的步骤(须等于当前步);review 步需 `approve=true` 放行。
/// 返回推进后的状态。
pub async fn advance_close(scheme: &str, period: &str, step: Option<&str>, approve: bool) -> Result<Value> {
    let run_code = run_code_of(scheme, period);
    let run = load_run(&run_code).await?;
    let cur = sv(&run, "current_step").unwrap_or_else(|| "collect".into());
    let run_status = sv(&run, "run_status").unwrap_or_default();
    if run_status == "closed" {
        return Err(api_err("该关账已完成(closed),如需重跑请先 reopen"));
    }
    // 目标步:显式给定须与当前步一致;缺省=当前步。
    let target = step.unwrap_or(&cur);
    if target != cur {
        return Err(api_err(&format!("当前步为 {cur},不能直接执行 {target}")));
    }

    let result: Value = match cur.as_str() {
        "collect" => {
            // 采集:校验个别数在位(cg_entity_balance 有行)。
            let rows = query_rows(
                "SELECT 1 FROM cg_entity_balance WHERE scheme_code=$1 AND period_code=$2 AND COALESCE(status,1)=1 LIMIT 1",
                vec![DataValue::String(scheme.to_string()), DataValue::String(period.to_string())],
                "close_collect_check",
            )
            .await?;
            if rows.is_empty() {
                record_step(&run_code, scheme, period, "collect", "failed", &json!({"error":"个别数未就位(cg_entity_balance 空)"})).await?;
                return Err(api_err("采集失败:个别数未就位(cg_entity_balance 空)"));
            }
            json!({"ok": true, "note": "个别数已就位"})
        }
        "reconcile" => crate::run_ic_reconciliation(scheme, period).await?,
        "consolidate" => {
            let c = crate::run_consolidation(scheme, period).await?;
            // 顺带跑 CF/EQC 聚合(有原始流水才有效,无则 0 行,不报错)。
            let cf = crate::run_cashflow(scheme, period).await.unwrap_or_else(|_| json!({"rows":0}));
            let eq = crate::run_equity_change(scheme, period).await.unwrap_or_else(|_| json!({"rows":0}));
            json!({"consolidation": c, "cashflow": cf, "equity": eq})
        }
        "review" => {
            if !approve {
                record_step(&run_code, scheme, period, "review", "pending", &json!({"note":"等待人工复核放行"})).await?;
                set_run(&run_code, "reviewing", "review", None).await?;
                return Ok(json!({
                    "ok": true, "runCode": run_code, "runStatus": "reviewing", "currentStep": "review",
                    "pendingApproval": true, "message": "已到复核门,请人工复核后 approve=true 放行",
                }));
            }
            json!({"ok": true, "note": "人工复核已放行"})
        }
        "statements" => crate::seed_consol_statements().await?,
        other => return Err(api_err(&format!("未知步骤 {other}"))),
    };

    record_step(&run_code, scheme, period, &cur, "done", &result).await?;

    // 计算下一步。
    let idx = STEPS.iter().position(|s| *s == cur).unwrap_or(0);
    let (next_step, next_status, done) = if idx + 1 < STEPS.len() {
        let ns = STEPS[idx + 1];
        let status = match ns {
            "reconcile" => "reconciling",
            "consolidate" => "consolidating",
            "review" => "reviewing",
            "statements" => "consolidating",
            _ => "collecting",
        };
        (ns.to_string(), status.to_string(), false)
    } else {
        ("closed".to_string(), "closed".to_string(), true)
    };
    let closed_at = done;
    set_run(&run_code, &next_status, &next_step, if closed_at { Some(()) } else { None }).await?;
    if !done {
        record_step(&run_code, scheme, period, &next_step, "pending", &json!({"note":"等待推进"})).await?;
    }

    Ok(json!({
        "ok": true, "scheme": scheme, "period": period, "runCode": run_code,
        "completedStep": cur, "runStatus": next_status, "currentStep": next_step,
        "closed": done, "result": result,
        "message": if done { format!("关账完成:{run_code} 已 closed") } else { format!("步骤 {cur} 完成,进入 {next_step}") },
    }))
}

/// 重开关账(把 closed 重置为 collecting,清步骤审计;派生数据由后续重跑覆盖)。
pub async fn reopen_close(scheme: &str, period: &str) -> Result<Value> {
    let run_code = run_code_of(scheme, period);
    let _ = load_run(&run_code).await?;
    set_run(&run_code, "reopened", "collect", None).await?;
    execute(
        "UPDATE cg_close_run SET run_status='collecting', current_step='collect', closed_at=NULL, update_time=CURRENT_TIMESTAMP WHERE run_code=$1",
        vec![DataValue::String(run_code.clone())],
    )
    .await?;
    execute(
        "DELETE FROM cg_close_step WHERE run_code=$1",
        vec![DataValue::String(run_code.clone())],
    )
    .await?;
    record_step(&run_code, scheme, period, "collect", "pending", &json!({"note":"已重开关账"})).await?;
    Ok(json!({ "ok": true, "runCode": run_code, "runStatus": "collecting", "currentStep": "collect", "message": "关账已重开" }))
}

/// 查关账状态(运行头 + 步骤审计明细)。
pub async fn get_close_status(scheme: &str, period: &str) -> Result<Value> {
    let run_code = run_code_of(scheme, period);
    let runs = query_rows(
        "SELECT scheme_code, period_code, run_code, run_status, current_step, flow_instance_id, started_at, closed_at \
         FROM cg_close_run WHERE run_code=$1",
        vec![DataValue::String(run_code.clone())],
        "close_status_run",
    )
    .await?;
    let steps = query_rows(
        "SELECT step_code, step_status, result_json, operator, ts FROM cg_close_step \
         WHERE run_code=$1 ORDER BY create_time, step_code",
        vec![DataValue::String(run_code.clone())],
        "close_status_steps",
    )
    .await?;
    Ok(json!({
        "scheme": scheme, "period": period, "runCode": run_code,
        "run": runs.first().cloned().unwrap_or(json!(null)),
        "steps": steps,
        "exists": !runs.is_empty(),
    }))
}

// —— 内部 ——

async fn load_run(run_code: &str) -> Result<Value> {
    let rows = query_rows(
        "SELECT scheme_code, period_code, run_code, run_status, current_step, flow_instance_id FROM cg_close_run WHERE run_code=$1",
        vec![DataValue::String(run_code.to_string())],
        "close_load_run",
    )
    .await?;
    rows.into_iter().next().ok_or_else(|| api_err("关账运行不存在,请先 start"))
}

/// 更新运行头状态/当前步(closed 时打 closed_at)。
async fn set_run(run_code: &str, status: &str, step: &str, close: Option<()>) -> Result<()> {
    let closed_sql = if close.is_some() { ", closed_at=CURRENT_TIMESTAMP" } else { "" };
    execute(
        &format!(
            "UPDATE cg_close_run SET run_status=$2, current_step=$3{closed_sql}, update_time=CURRENT_TIMESTAMP WHERE run_code=$1"
        ),
        vec![
            DataValue::String(run_code.to_string()),
            DataValue::String(status.to_string()),
            DataValue::String(step.to_string()),
        ],
    )
    .await
}

/// 记一步审计(UPSERT:同 run+step 覆盖)。
async fn record_step(run_code: &str, scheme: &str, period: &str, step: &str, step_status: &str, result: &Value) -> Result<()> {
    execute(
        "INSERT INTO cg_close_step (id, code, name, scheme_code, period_code, run_code, step_code, step_status, \
            result_json, operator, ts, sort_no, status, create_time, update_time) \
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,CURRENT_TIMESTAMP,0,1,CURRENT_TIMESTAMP,CURRENT_TIMESTAMP) \
         ON CONFLICT (run_code, step_code) DO UPDATE SET step_status=EXCLUDED.step_status, result_json=EXCLUDED.result_json, \
            ts=CURRENT_TIMESTAMP, update_time=CURRENT_TIMESTAMP",
        vec![
            pk(),
            DataValue::String(format!("{run_code}|{step}")),
            DataValue::String(format!("{step} 步")),
            DataValue::String(scheme.to_string()),
            DataValue::String(period.to_string()),
            DataValue::String(run_code.to_string()),
            DataValue::String(step.to_string()),
            DataValue::String(step_status.to_string()),
            DataValue::String(serde_json::to_string(result).unwrap_or_default()),
            DataValue::String("consol-close".to_string()),
        ],
    )
    .await
}
