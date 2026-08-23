# cmx-report 合并报表平台 · 完整测试报告

| 项 | 值 |
|---|---|
| 测试日期 | 2026-08-23 10:53 |
| 分支 | main |
| 工具链 | cargo 1.97.1 |
| 环境 | report-server :8092(在跑)· model-center :8099(在跑)· PostgreSQL fico-db |
| 被测范围 | 合并引擎(C1–C7)+ Next-5 + Later-7(L1–L7)+ 正交项(O1–O7)全链 |
| 代码状态 | **未提交**(8 改 + 6 新增,见附录 D) |

## 一、执行摘要

**结论:全部通过。** 五个层次共 **129 项断言 / 检验组合全绿,0 失败**。

| 层次 | 套件 | 结果 |
|---|---|---|
| 构建完整性 | `cargo build`(全 workspace) | ✅ 0 error(2 warning:未用私有函数) |
| 引擎单元测试 | `cmx-consol-model` | ✅ **24 / 24** |
| 后端 E2E | 5 套件(Later-7 / 四表 / 范围变动 / O1–O6 / O7 一键部署) | ✅ **63 / 63** |
| 前端 E2E(CDP) | 工作台 + 四表出表 | ✅ **28 / 28** |
| 数据完整性 | 全方案 BS 恒等性扫描 | ✅ **14 / 14** 组合平衡 |
| **合计** | | ✅ **129 / 129** |

## 二、测试层次详情

### 2.1 构建完整性

```
cargo build（全 workspace） → 0 errors, 2 warnings
```
两条 warning 均为 `cmx-consol-store-pg` 内未使用的私有辅助函数(`s_body` / `body_s`),无功能影响。

### 2.2 引擎单元测试(cmx-consol-model,24/24)

纯引擎无 DB/HTTP 依赖,覆盖全部合并算法:

| # | 用例 | 验证点 |
|---|---|---|
| 1 | aggregate_full_and_proportional | 全额/比例合并聚合 |
| 2 | capital_elimination_balanced_with_goodwill_and_nci | 资本抵销(商誉+少数股东)平衡 |
| 3 | common_control_uses_capital_reserve_no_goodwill | 同一控制→资本公积不确认商誉 |
| 4 | step_acquisition_remeasures_prev_holding | 分步取得原持股重估 |
| 5 | disposal_equity_txn_vs_loss_of_control | 处置(权益交易 vs 丧失控制) |
| 6 | minority_pl_splits_profit | 少数股东损益分摊 |
| 7 | debt_and_sales_elimination_balanced | 债务/购销抵销平衡 |
| 8 | dividend_elimination_removes_intragroup_income | 内部股利抵销 |
| 9 | worksheet_consolidated_equals_ind_plus_adjust_plus_elim | 工作底稿恒等式 |
| 10 | full_worked_example_balance_sheet_ties | 完整算例 BS 平衡 |
| 11 | fx_translate_balances_with_cta | 外币折算 + CTA |
| 12 | inventory_unrealized_profit_two_periods | 存货未实现利润(两期) |
| 13 | fixed_asset_profit_with_depreciation_reversal | 固定资产利润 + 折旧转回 |
| 14 | ic_reconcile_matched_diff_onesided | 内部往来对账三态 |
| 15 | equity_pickup_and_loss | 权益法确认 |
| 16 | goodwill_impairment_entry | 商誉减值分录 |
| 17 | diff_scope_classifies_changes | 范围变动分类 |
| 18 | aggregate_cash_flow_skips_intercompany_and_nonmembers | 现金流量聚合(剔除内部) |
| 19 | aggregate_equity_change_by_column | 权益变动按列聚合 |
| 20 | cash_flow_worksheet_ties_to_cash_delta | 现金流量工作底稿勾稽 |
| 21 | effective_ownership_cross_holding_converges | 交叉持股有效持股收敛 |
| 22 | effective_ownership_no_cross_equals_direct | 无交叉→等于直接持股 |
| 23 | net_investment_hedge_reclassifies_to_cta | **O1** 净投资套期重分类 |
| 24 | goodwill_impairment_test_capped_at_goodwill | **O2** 商誉减值封顶 |

### 2.3 后端 E2E(真机 curl 往返,63/63)

**Later-7 能力(`e2e-consol-later.sh`,9 ✅)**
- L1 同一控制企业合并:商誉=0 + BS 恒等=0
- L2 分步取得:投资收益重估 adjust=−30
- L3 固定资产未实现利润:固资 elim=−100 + 折旧转回 +40
- L4 内部股利抵销:凭证 2 行
- L5 现金流量表·工作底稿法:三活动净额=现金实际变动
- L6 交叉持股·有效持股:A≈0.8511(迭代收敛)
- L7 附注自动生成:商誉期末=15

**C7 合并四表出表(`e2e-consol-statements.sh`,13 ✅)**
- 四表定义 seed(CBS/CIS/CCF/CSE)
- CBS(CSCEC/2026-06):资产合计=权益合计=负债和权益合计=360、A1 标签渲染、零计算错误
- CIS(IS_TEST/G):营收 350 / 利润总额 130 / 少数股东损益 10 / 归母 120
- CCF / CSE 模板壳零计算错误

**C7 范围变动(`e2e-consol-scope-change.sh`,10 ✅)**
- CAS_LEGAL:处置 3 项 + 新纳入 4 项 + 自动识别上期 2026-06
- SC_TEST:四类变动齐全(增持/方法变更/处置/新纳入)
- 幂等:重跑不产生重复

**正交项 O1–O6(`e2e-consol-orthogonal.sh`,22 ✅)**
- O1 净投资套期:CTA 重分类 adjust=−60 / 套期储备=+60 / BS=0
- O2 商誉减值 CGU:减值后商誉=5(20−15)/ 减值损失=+15 / BS=0
- O3 自动 IC 调整建议:建议条数≥1 / 调整主体=少报方 S / 调整额=20
- O4 完整 CSE:年初 1000 / 年末 1200 / 零错误
- O5 完整 CCF(33 行):经营 60 / 投资 50 / 筹资 30 / 净增加 145 / 期末 345 / 零错误 / 单元格 53
- O6 有效持股接 NCI:有效持股 S=0.9 / NCI 损益用 0.9 得 10(非直接 0.8 得 20)/ BS=0

**O7 一键部署(`bootstrap-consol.sh`,9 ✅)**
- ① model-center 部署 24 张 cg_* 元数据(加法幂等)
- ② seed demo 方案(3 主体 / 11 科目 / 3 抵销规则 / 1 组 IC)
- ③ run 合并 + BS 恒等=0 + 内部应收合并后=0
- ④ 四表 seed + CBS/CIS/CCF/CSE 计算态零错误

### 2.4 前端 E2E(CDP / Playwright,28/28)

**合并工作台(`e2e-consol-workbench.mjs`,18 ✅)**
- 同源页加载 + native page 动态 import + 三区挂载
- explorer 方案下拉(15 方案)+ 运行按钮
- content 六 tab(底稿/对账/分类账/范围变动/合并报表/附注)
- GW_TEST:商誉减值后=15.00 / 资产减值损失=5.00 / 借贷平衡 / 底稿合计平衡
- 分类账含商誉减值(R_GW)+ 资本抵销(R_CAP)凭证
- RECON_TEST 对账表有行 + 差异行标红
- **O3 自动调整建议面板渲染**(panel=true rows=1)
- 范围变动 tab 徽标渲染
- **无页面级 JS 错误**

**四表出表前端(`e2e-consol-statements-frontend.mjs`,10 ✅)**
- 出表按钮 + 六 tab + 合并报表 tab 激活
- 四表 chip 齐全(CBS/CIS/CCF/CSE)
- CBS 内嵌预览 16 行(29 格 · 错误 0)
- CCF 真取数(53 格 · 错误 0)
- 抵销栏下钻:工作底稿 → 分类账过滤高亮 → 回工作底稿(闭环)

> 注:测试期出现一条后台资源 401(Unauthorized),属登录态静态资源加载,非测试断言,不影响结果。

### 2.5 数据完整性 · BS 恒等性全方案扫描(14/14)

对全部 15 个方案的每个(方案 × 期间 × 顶层节点)组合,断言 **Σ(所有合并科目)= 0**(借方正约定下的资产负债表恒等式)。

```
检验(方案,期间,顶层节点)组合: 14
BS 恒等=0 通过: 14
全部平衡 ✅
```

(15 方案中部分尚无期间/合并数据,产生 14 个可校验组合,全部平衡。)

## 三、被测系统规模

| 维度 | 规模 |
|---|---|
| 元数据表(cg_*) | 24 张(model-center 部署,加法幂等) |
| 引擎公共函数(cmx-consol-model) | 24 个 |
| 后端端点(consol_routes) | 39 个 |
| 抵销/调整类型(ELIM_LABEL) | 16 类 |
| 出表 | 复用 cmx-rpt-formula 引擎(CBS/CIS 真取数 + CCF 33 行 + CSE 20 列) |
| 前端 | native-page portal.consol.workbench(六 tab,改源即生效) |

## 四、风险与观察

1. **代码未提交** — 本次测试基于工作区未提交改动(8 改 6 新增)。测试结果绑定当前工作区状态,提交前建议复跑。
2. **两条 dead-code warning** — `s_body` / `body_s` 未使用,建议清理(非阻断)。
3. **CCF/CSE 模板壳 vs 完整流水并存** — C7 阶段 seed 的是模板壳(`e2e-consol-statements.sh` 验壳零错误);O4/O5 提供完整 33 行/20 列流水法(`e2e-consol-orthogonal.sh` 验真取数)。两条路径均绿,但生产选型需明确以哪套为准。
4. **前端 401 后台资源** — 不影响断言,但建议排查登录态静态资源路径,避免噪声。
5. **无 store 层独立单测** — 存储层(cmx-consol-store-pg)靠 E2E 覆盖,无 `#[tokio::test]`。DB 编排逻辑的回归目前依赖真机 E2E,建议后续补关键路径的存储层测试。

## 附录

### A. 测试命令清单
```bash
cargo build                                   # 构建
cargo test -p cmx-consol-model                # 引擎单测 24
bash   scripts/e2e-consol-later.sh            # Later-7
bash   scripts/e2e-consol-statements.sh       # 四表出表
bash   scripts/e2e-consol-scope-change.sh     # 范围变动
bash   scripts/e2e-consol-orthogonal.sh       # O1–O6
bash   scripts/bootstrap-consol.sh            # O7 一键部署
node   scripts/e2e-consol-workbench.mjs       # 工作台 CDP
node   scripts/e2e-consol-statements-frontend.mjs  # 四表前端 CDP
```

### B. 前置条件
- report-server 运行于 :8092(数据源 postgres://…/fico)
- model-center 运行于 :8099(承载 `/api/model/deploy`,db_id=fico-db)
- 已部署 24 张 cg_* 元数据(bootstrap 步骤①幂等保证)

### C. 结果矩阵
| 套件 | 类型 | 断言 | 通过 | 失败 |
|---|---|---|---|---|
| cargo build | 构建 | — | ✅ | 0 |
| cmx-consol-model | 单元 | 24 | 24 | 0 |
| e2e-consol-later.sh | 后端 | 9 | 9 | 0 |
| e2e-consol-statements.sh | 后端 | 13 | 13 | 0 |
| e2e-consol-scope-change.sh | 后端 | 10 | 10 | 0 |
| e2e-consol-orthogonal.sh | 后端 | 22 | 22 | 0 |
| bootstrap-consol.sh | 后端 | 9 | 9 | 0 |
| e2e-consol-workbench.mjs | 前端 | 18 | 18 | 0 |
| e2e-consol-statements-frontend.mjs | 前端 | 10 | 10 | 0 |
| BS 恒等性扫描 | 数据 | 14 | 14 | 0 |
| **合计** | | **129** | **129** | **0** |

### D. 未提交改动
改动(8):`crates/cmx-consol-model/src/engine.rs`、`crates/cmx-consol-store-pg/src/{crud,lib,notes,statements}.rs`、`crates/cmx-rpt-app/src/consol.rs`、`scripts/e2e-consol-workbench.mjs`、`web/ui-native/rpt/consol-workbench.js`
新增(6):`scripts/bootstrap-consol.sh`、`scripts/e2e-consol-orthogonal.sh`、`docs/summary/assets/{gen-svgs-v2.mjs,shot-v2.mjs,v2/}`、`docs/summary/cmx-report合并报表平台-阶段性总结-v2.md`
