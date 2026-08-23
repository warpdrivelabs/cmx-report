# cmx-report 合并报表平台 · 全面功能测试报告

| 项 | 值 |
|---|---|
| 测试日期 | 2026-08-23 11:20 |
| 分支 | main · 工具链 cargo 1.97.1 |
| 环境 | report-server :8092 · model-center :8099 · PostgreSQL fico-db |
| 测试类型 | **功能测试**(端点全覆盖 + CRUD 往返 + 关账全生命周期 + 负路径/校验 + 幂等) |
| 代码状态 | **未提交** |

## 一、执行摘要

本轮为**功能测试**,聚焦上一轮 happy-path E2E 未覆盖的面:39 个后端端点的完整覆盖、CRUD 往返、**关账编排全生命周期状态机**、负路径与校验、幂等语义。

**结论:全部通过。新增功能套件 27/27,叠加回归 49/49,合计 76/76,0 失败。**

| 套件 | 类型 | 结果 |
|---|---|---|
| **e2e-consol-functional.sh(新增)** | 功能(F1–F6) | ✅ **27 / 27** |
| cmx-consol-model | 单元回归 | ✅ 24 / 24 |
| e2e-consol-later.sh | 后端回归 | ✅ 9 / 9 |
| e2e-consol-orthogonal.sh | 后端回归 | ✅ 12 / 12 |
| e2e-consol-statements.sh | 后端回归 | ✅ 4 / 4 |
| **本轮合计** | | ✅ **76 / 76** |

**端点覆盖:39 / 39**(含前端 CDP 触发的 ic-recon GET)。

## 二、新增功能套件详情(e2e-consol-functional.sh,27/27)

新建独立 `FUNC_LIFE` / `FUNC_EMPTY` 方案,零污染既有数据。

### F1 查询端点(5 ✅)—— 补全从未被测的读侧
| 断言 | 结果 |
|---|---|
| `periods` 返回≥1 期 | ✅ np=1 |
| `nodes`=4(GRP/P/S1/S2) | ✅ |
| `accounts`=9 | ✅ |
| `value`(GRP/1001)=500(300+50+150 无抵销) | ✅ |
| `value` 不存在账户=0(不报错) | ✅ 容错 |

### F2 主数据 CRUD 往返(4 ✅)—— 补全未测的 upsert
| 断言 | 结果 |
|---|---|
| `coa-mapping` upsert saved=1 | ✅ |
| `interim-profit` upsert saved=1 | ✅ |
| `goodwill-impair` upsert saved=1 | ✅ |
| `interim-profit` 同键 upsert 覆盖(无唯一冲突) | ✅ upsert 语义 |

### F3 关账编排全生命周期(9 ✅)—— **本轮核心补强**
完整走通状态机 `collect → reconcile → consolidate → review(人工门) → statements → closed`,再 reopen:

| 断言 | 结果 |
|---|---|
| `close/start` 后 status.exists=true | ✅ |
| collect 完成→进 reconcile | ✅ |
| reconcile 完成→进 consolidate | ✅ |
| consolidate 完成→进 review | ✅ |
| **review 未 approve→停复核门(pendingApproval)** | ✅ 人工门生效 |
| **review approve=true→放行进 statements** | ✅ |
| statements 完成→关账 closed | ✅ |
| status.steps≥5(审计留痕) | ✅ nst=5 |
| reopen→回 collecting | ✅ 可回退 |

### F4 负路径 / 校验(4 ✅)—— 系统健壮性
| 断言 | 结果 |
|---|---|
| 空范围方案 run→报错 | ✅ code=1「未配置合并范围」 |
| net-investment-hedge 缺必填科目→列校验报错 | ✅ code=400 |
| **closed 后再 advance→被拒** | ✅ code=1 |
| 未知方案查询→空集(容错不崩) | ✅ code=0 空 |

### F5 幂等(3 ✅)
| 断言 | 结果 |
|---|---|
| scheme 重复 upsert 幂等(count 17=17) | ✅ |
| run 重复幂等(账户数 11=11,DELETE+重建) | ✅ |
| 重复 run 后 BS 恒等=0 | ✅ |

### F6 CF/EQC run 聚合(2 ✅)
| 断言 | 结果 |
|---|---|
| `cashflow/run` 聚合成功 | ✅ code=0 |
| `equity/run` 聚合成功 | ✅ code=0 |

## 三、测试过程中发现并处置的问题

### 3.1 测试数据缺陷(已修正,非系统缺陷)
初版 F5 的 S2 个别数试算不平(150 缺失、营收 50 无对应资产,合计 −20),导致 BS 恒等断言失败(bs=−20)。

**定性:这恰好验证了引擎的诚实性** —— 合并引擎忠实聚合输入的个别数,不会"擅自平账"。个别数不平,合并数就不平。修正测试种子(补 S2 货币资金 150 使个别数平衡)后 BS=0。

**该现象反而是一条正向证据**:引擎不掩盖上游数据质量问题,BS 恒等可作为个别数完整性的探针。

### 3.2 端点覆盖缺口(已补全)
上一轮报告后复核发现 15 个端点从未被任何套件触及(全部查询端点、关账全部四个端点、value、部分 upsert)。本轮功能套件专门补齐,端点覆盖从 24/39 提升至 **39/39**。

## 四、端点覆盖矩阵(39/39)

| 分类 | 端点 | 覆盖来源 |
|---|---|---|
| 查询(5) | schemes/periods/nodes/accounts/value | F1 + 既有 |
| 主数据(11) | group-accounts/coa-mapping/scope/entity-balances/elim-rules/ic-matches/ic-declarations/fx-rates/interim-profit/goodwill-impair/shareholding | F2 + later/orthogonal |
| 对账(2) | ic-reconcile / ic-recon | orthogonal + workbench CDP |
| 抵销输入(5) | step-txn/fa-profit/net-investment-hedge/goodwill-cgu/effective-ownership | orthogonal |
| 关账(4) | close/start·advance·reopen·status | **F3(本轮补全)** |
| 现金流/权益(5) | cash-flow/equity-change/cashflow/run·worksheet/equity/run | F6 + later |
| 出表/合并(5) | run/seed-statements/scope-change/worksheet/journal | later/statements/scope-change |
| 附注/建议(2) | notes / ic-adjustment-suggestions | orthogonal |

## 五、风险与观察

1. **代码未提交** —— 结果绑定当前工作区。
2. **BS 恒等是数据质量探针,非仅算法校验** —— 上游个别数不平会传导至合并数;生产环境应在采集(collect)步增加个别数试算平衡校验(目前 collect 只校验 cg_entity_balance 非空,不校验借贷平衡)。**建议**:collect 步补一条 Σ(个别数)=0 的入口断言。
3. **关账 flow 对接为 env-gated** —— 本轮在未配 `FLOW_BASE_URL` 下测试(纯服务内编排,状态机是真相源)。**接真 cmx-flow 实例的路径(flow_instance_id 回填、门户待办复核)未在本轮覆盖**,需专项环境验证。
4. **store 层仍无独立单测** —— DB 编排靠 E2E 覆盖。
5. **测试产生残留 FUNC_* / 部分历史方案** —— 累积至 17 个方案;建议加测试后清理钩子,或用事务回滚隔离。

## 附录

### A. 运行命令
```bash
bash scripts/e2e-consol-functional.sh          # 新增功能套件 F1-F6
cargo test -p cmx-consol-model                 # 单元回归
bash scripts/e2e-consol-later.sh               # 后端回归
bash scripts/e2e-consol-orthogonal.sh          # 后端回归
bash scripts/e2e-consol-statements.sh          # 后端回归
```

### B. 结果矩阵
| 套件 | 断言 | 通过 | 失败 |
|---|---|---|---|
| e2e-consol-functional.sh | 27 | 27 | 0 |
| cmx-consol-model | 24 | 24 | 0 |
| e2e-consol-later.sh | 9 | 9 | 0 |
| e2e-consol-orthogonal.sh | 12 | 12 | 0 |
| e2e-consol-statements.sh | 4 | 4 | 0 |
| **合计** | **76** | **76** | **0** |

### C. 与上一轮(完整测试报告)的关系
上一轮验证"能力齐全 + happy-path 正确"(129 项)。本轮补"接口全覆盖 + 边界/负路径/生命周期健壮"(功能维度),两轮合计构成对合并报表平台的完整测试基线。新增资产:`scripts/e2e-consol-functional.sh`。
