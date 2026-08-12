//! rpt_job —— 报表计算作业（业务接入异步任务中心的首个样例，方案 §9）。
//!
//! 把「批量报表计算」包装为一个 [`JobHandler`]：对给定 org+period 下的一组报表 code，
//! 逐张调用既有 [`compute_report_service`](crate::compute_report_service)，每张上报明细进度、
//! 在张与张之间埋 [`JobContext::checkpoint`] 响应暂停/停止。业务逻辑复用，零重写（方案 §9 接入清单）。
//!
//! params: `{ orgCode, periodCode, version?, reportCodes?: [".."] }`。
//!   - `reportCodes` 省略时：对该 org+period 下 cr_report_list 的全部启用报表求值。
//! 幂等（compute_report_service 按 org+period 覆盖写 cr_cell_data）→ 支持 Fresh 重启。

use async_trait::async_trait;
use cmx_core::dv;
use serde_json::{Value, json};

use cmx_job_core::{
    JobCaps, JobContext, JobError, JobHandler, JobPlan, RegisteredJob, Restart,
};
use cmx_rpt_model::RPT_DB_ID;

use crate::{compute_report_service, query_rows};

/// 报表计算作业种类标识。
pub const KIND_RPT_COMPUTE: &str = "rpt.compute";

/// 批量报表计算作业。
pub struct RptComputeJob;

/// 从 params 取必填的 org/period（缺失即参数错误）。
fn require_ctx(params: &Value) -> Result<(String, String, String), JobError> {
    let org = params
        .get("orgCode")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| JobError::new(400, "orgCode 不能为空"))?
        .to_string();
    let period = params
        .get("periodCode")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| JobError::new(400, "periodCode 不能为空"))?
        .to_string();
    let version = params
        .get("version")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    Ok((org, period, version))
}

/// 解析 reportCodes（省略/空 → None，表示全量）。
fn parse_codes(params: &Value) -> Option<Vec<String>> {
    let arr = params.get("reportCodes")?.as_array()?;
    let codes: Vec<String> = arr
        .iter()
        .filter_map(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    if codes.is_empty() { None } else { Some(codes) }
}

/// 查该 org+period 适用的全部启用报表 code（reportCodes 省略时的全量兜底）。
///
/// M1 简化：取 cr_report_list 全部启用报表（status=1）。未来可按 org 适用范围/期间类型过滤。
async fn all_report_codes() -> Result<Vec<(String, String)>, JobError> {
    let sql = r#"SELECT code, COALESCE(name, code) AS name
                 FROM cr_report_list
                 WHERE COALESCE(status, 1) = 1
                 ORDER BY COALESCE(sort_no, 999999), code"#;
    let rows = query_rows(sql, dv!(), "rpt_compute_job.all_codes")
        .await
        .map_err(|e| JobError::new(500, format!("装载报表清单失败: {e}")))?;
    Ok(rows
        .iter()
        .filter_map(|r| {
            let code = r.get("code").and_then(|v| v.as_str())?.to_string();
            let name = r
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or(&code)
                .to_string();
            Some((code, name))
        })
        .collect())
}

#[async_trait]
impl JobHandler for RptComputeJob {
    fn kind(&self) -> &'static str {
        KIND_RPT_COMPUTE
    }

    fn capabilities(&self) -> JobCaps {
        // 幂等（org+period 覆盖写），可暂停（张间 checkpoint），Fresh 重启。
        JobCaps {
            pausable: true,
            restart: Restart::Fresh,
            idempotent: true,
            ..JobCaps::default()
        }
    }

    fn plan(&self, params: &Value) -> Result<JobPlan, JobError> {
        let (org, period, _) = require_ctx(params)?;
        // total 优先用显式 reportCodes 数；全量时留 0，run 里装载后 set_total 修正。
        let total = parse_codes(params).map(|c| c.len() as u64).unwrap_or(0);
        Ok(JobPlan {
            total,
            title: Some(format!("报表计算 · {org} · {period}")),
        })
    }

    async fn run(&self, ctx: &JobContext, params: Value) -> Result<Value, JobError> {
        let (org, period, version) = require_ctx(&params)?;

        // 阶段 1：确定报表清单（显式 or 全量）。
        ctx.set_phase(1, 2, "装载报表清单");
        let targets: Vec<(String, String)> = match parse_codes(&params) {
            Some(codes) => codes.into_iter().map(|c| (c.clone(), c)).collect(),
            None => all_report_codes().await?,
        };
        if targets.is_empty() {
            return Err(JobError::new(422, "无可计算的报表（清单为空）"));
        }
        ctx.set_total(targets.len() as u64);
        ctx.info(format!(
            "共 {} 张报表待计算（org={org} period={period}）",
            targets.len()
        ));
        // 预登记明细行（前端一次性看到全清单，逐张变色）。
        for (code, name) in &targets {
            ctx.add_item(code, name);
        }

        // 阶段 2：逐张求值（复用 compute_report_service，张间 checkpoint）。
        ctx.set_phase(2, 2, "求值");
        let mut ok_count = 0u64;
        let mut fail_count = 0u64;
        let mut cell_total = 0u64;
        let mut errors: Vec<Value> = Vec::new();

        for (code, _name) in &targets {
            ctx.checkpoint().await?; // 暂停/停止响应点
            ctx.item_running(code, "求值中");
            let started = std::time::Instant::now();

            let body = json!({
                "orgCode": org,
                "periodCode": period,
                "version": version,
            });
            match compute_report_service(code, &body).await {
                Ok(out) => {
                    let elapsed = started.elapsed().as_millis() as u64;
                    let computed = out.get("computed").and_then(|v| v.as_u64()).unwrap_or(0);
                    let err_cnt = out.get("errorCount").and_then(|v| v.as_u64()).unwrap_or(0);
                    cell_total += computed;
                    if err_cnt > 0 {
                        // 单张内有单元格错误：算失败行但不终止整批（续算策略）。
                        fail_count += 1;
                        ctx.item_fail(code, format!("{err_cnt} 个单元格错误"));
                        ctx.warn(format!("报表 {code} 有 {err_cnt} 个单元格计算错误"));
                        errors.push(json!({ "code": code, "errorCount": err_cnt }));
                    } else {
                        ok_count += 1;
                        ctx.item_ok(code, elapsed);
                    }
                }
                Err(e) => {
                    // 单张整体失败：记录并续算下一张（不 ? 传播，避免一张挂掉整批）。
                    fail_count += 1;
                    ctx.item_fail(code, e.to_string());
                    ctx.warn(format!("报表 {code} 计算失败: {e}"));
                    errors.push(json!({ "code": code, "error": e.to_string() }));
                }
            }
            ctx.progress_inc(1);
        }

        ctx.info(format!(
            "计算完成：成功 {ok_count} 张，失败 {fail_count} 张，落库 {cell_total} 个单元格"
        ));

        Ok(json!({
            "dbId": RPT_DB_ID,
            "orgCode": org,
            "periodCode": period,
            "version": version,
            "total": targets.len(),
            "ok": ok_count,
            "failed": fail_count,
            "cells": cell_total,
            "errors": errors,
        }))
    }
}

// inventory 注册（方案 §7.5）：web-server 链接本 crate 即自动收录本 handler。
inventory::submit! { RegisteredJob { make: || Box::new(RptComputeJob) } }

// ═══════════════════════════════════════════════════════════════════════════
// 第二业务样例：报表校验 rpt.verify（证明多业务接入，方案 §9.1）。
// ═══════════════════════════════════════════════════════════════════════════

/// 报表校验作业种类标识。
pub const KIND_RPT_VERIFY: &str = "rpt.verify";

/// 批量报表校验作业：对给定 org+period 下一组报表，逐张扫 cr_cell_data 里 data_status='error'
/// 的单元格，汇总各报表的错误格数。只读（不落库），幂等，Fresh 重启。
pub struct RptVerifyJob;

#[async_trait]
impl JobHandler for RptVerifyJob {
    fn kind(&self) -> &'static str {
        KIND_RPT_VERIFY
    }

    fn capabilities(&self) -> JobCaps {
        JobCaps {
            pausable: true,
            restart: Restart::Fresh,
            idempotent: true,
            ..JobCaps::default()
        }
    }

    fn plan(&self, params: &Value) -> Result<JobPlan, JobError> {
        let (org, period, _) = require_ctx(params)?;
        let total = parse_codes(params).map(|c| c.len() as u64).unwrap_or(0);
        Ok(JobPlan {
            total,
            title: Some(format!("报表校验 · {org} · {period}")),
        })
    }

    async fn run(&self, ctx: &JobContext, params: Value) -> Result<Value, JobError> {
        let (org, period, _version) = require_ctx(&params)?;

        ctx.set_phase(1, 2, "装载报表清单");
        let targets: Vec<(String, String)> = match parse_codes(&params) {
            Some(codes) => codes.into_iter().map(|c| (c.clone(), c)).collect(),
            None => all_report_codes().await?,
        };
        if targets.is_empty() {
            return Err(JobError::new(422, "无可校验的报表（清单为空）"));
        }
        ctx.set_total(targets.len() as u64);
        ctx.info(format!("共 {} 张报表待校验（org={org} period={period}）", targets.len()));
        for (code, name) in &targets {
            ctx.add_item(code, name);
        }

        ctx.set_phase(2, 2, "校验");
        let mut clean = 0u64;
        let mut dirty = 0u64;
        let mut total_errors = 0u64;
        let mut findings: Vec<Value> = Vec::new();

        for (code, _name) in &targets {
            ctx.checkpoint().await?;
            ctx.item_running(code, "校验中");
            // 扫本报表 org+period 下 data_status='error' 的单元格数。
            let sql = r#"SELECT COUNT(*) AS n FROM cr_cell_data
                         WHERE report_code = $1 AND org_code = $2 AND period_code = $3
                           AND data_status = 'error'"#;
            let rows = query_rows(sql, dv![code.as_str(), org.as_str(), period.as_str()], "rpt_verify_job")
                .await
                .unwrap_or_default();
            let err_cells = rows
                .first()
                .and_then(|r| r.get("n"))
                .and_then(|v| v.as_u64().or_else(|| v.as_str().and_then(|s| s.parse().ok())))
                .unwrap_or(0);
            if err_cells > 0 {
                dirty += 1;
                total_errors += err_cells;
                ctx.item_fail(code, format!("{err_cells} 个错误单元格"));
                ctx.warn(format!("报表 {code} 有 {err_cells} 个错误单元格"));
                findings.push(json!({ "code": code, "errorCells": err_cells }));
            } else {
                clean += 1;
                ctx.item_ok(code, 0);
            }
            ctx.progress_inc(1);
        }

        ctx.info(format!("校验完成：通过 {clean} 张，异常 {dirty} 张，共 {total_errors} 个错误单元格"));
        Ok(json!({
            "dbId": RPT_DB_ID,
            "orgCode": org,
            "periodCode": period,
            "total": targets.len(),
            "clean": clean,
            "dirty": dirty,
            "errorCells": total_errors,
            "findings": findings,
        }))
    }
}

inventory::submit! { RegisteredJob { make: || Box::new(RptVerifyJob) } }
