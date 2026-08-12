# cmx-report

> 独立报表平台微服务 —— 企业报表设计/应用/计算的后端能力中心。
>
> **MEGA Report · 报表平台**（:8092）

`cmx-report` 是一个独立的 Cargo workspace，承载报表微服务。它把原本内嵌在 `cmx-container` 里的报表后端剥离成可独立部署的服务：报表既能像今天一样内嵌在平台 web-server（经 keep-wired `cmx-rpt-api` 薄壳），又能作为 `cmx-rpt-server` 独立进程跑（:8092）。这与流程引擎 `cmx-flowengine`、门户 `cmx-portalservice` 是同一模式。

---

## 定位

平台后端的微服务化拆分，让每个能力中心成为独立进程：

| 能力中心 | workspace | bin | 端口 |
|---|---|---|---|
| 门户 | `cmx-portalservice` | `cmx-portal-server` | 8080 |
| 流程引擎 | `cmx-flowengine` | `cmx-flow-server` | 8091 |
| **报表**（本仓） | `cmx-report` | `cmx-rpt-server` | 8092 |
| 主数据 | （规划中） | mdm | — |

各微服务 **同一套装配核**（chassis `run`）、**同一套配置制度**（`CONFIG_FILE` → ConfigManager）、**同一套启动脚本契约**，只是服务身份（banner / 业务路由）不同。

---

## 架构：一芯多壳（对标 cmx-flow-app）

```
cmx-report/                                （独立 workspace）
├── crates/
│   ├── cmx-rpt-model/       中立模型（常量 RPT_DB_ID + 请求 DTO，无 DB/HTTP）
│   ├── cmx-rpt-formula/     报表函数引擎（DSL 词法/语法/求值 + 函数注册 + REF 依赖解析）
│   ├── cmx-rpt-store-pg/    PG 持久化/服务层（cr_* 表读写 + ZmcDataSet 零拷贝）
│   ├── cmx-rpt-app/         ★ 中立核：全部 handler + report_routes::<S>() + 监控大盘
│   └── cmx-rpt-server/      ★ 独立 chassis bin：banner + 大盘 + 数据源钩子
│
└──（跨 workspace path 引用，仍是 cmx-container 成员）
     cmx-database-pg / cmx-core / cmx-utils / cmx-biz / cmx-api-types /
     cmx-job-core / cmx-traits / cmx-web-chassis / cmx-web-monitor / cmx-service-base

cmx-container/                             （保留）
└── crates/libs/cmx-rpt/cmx-rpt-api/       keep-wired 薄壳：ReportModule → report_routes::<CmxAppState>()
```

**中立核 `cmx-rpt-app`** —— 全部 axum handler + 路由表 `report_routes::<S>()`（对任意 axum state 泛型 `S` 成立：handler 只带 Path/Query/Json 提取器，DB 走 store 层全局 pg 管理器 + `RPT_DB_ID`）。信封类型**直用 `cmx-api-types`**——比 flow 更省：report store 本就返回 `cmx_api_types::Result`，无需自建 resp.rs / 转换桥。不依赖 `cmx-api`。

**平台壳 `cmx-rpt-api`（留 cmx-container，keep-wired）** —— `ReportModule::routes()` 调 `cmx_rpt_app::report_routes::<CmxAppState>()`，由 web-server 合并；经跨 workspace path **反向引用**本库的 `cmx-rpt-app`。这与流程「引擎核在独立 ws、`cmx-flow-api` 壳留 container」是同一模式。

**独立壳 `cmx-rpt-server`（本 workspace）** —— chassis bin，`merge(report_routes::<()>())` + 根路径监控大盘 + 单数据源注册钩子。

**两壳零 handler 漂移** —— handler 丢弃原来绑定不用的 `State(_s)`/`CmxSvrContext(_ctx)` 两提取器 → 与 AppState 类型无关，故路由对任意 `S` 成立。唯一身份 handler `apply_ops` 改走 `cmx-traits::auth::context_scope::current_auth()`（与平台 `mw_auth` 注入的 `SVRContext.auth_context` 并行同源，嵌入平台时取到同一登录用户 → 零回归；独立 server 无认证时回退空串＝迁移前 `None` 分支行为）。

**依赖策略** —— 域内 rpt crate 纯 path；基础设施仍是 `cmx-container` 成员，经跨 ws path 引用；外部 crate 走 aliyun 镜像，版本与 `cmx-container` 根对齐。因此**构建本仓需 `../cmx-container/` 并排存在**。

---

## 业务域

| 域 | 端点 | 职责 |
|---|---|---|
| 报表设计工作台 | `/api/report-design/*` | overview / reports / elements / calendar / consol-org / 版本 / 版式 layout / 数据 data |
| 打开与展开 | `/api/report-design/reports/{code}/open`、`/expand` | 一次取全集（版式+cellMap+元素+函数[+数据]）；浮动行列按数据源展开 |
| 浮动行/列 CRUD | `/api/report-design/reports/{code}/float/*` | cr_report_float_row/col 增删改查 + 取数初始化 |
| 计算态 | `/api/report-design/reports/{code}/compute`、`/functions` | 装载公式→递归求值→落 cr_cell_data；函数目录供设计器向导 |
| 协同编辑 B 档 | `/api/report-design/reports/{code}/ops` | 语义操作提交 + 增量拉取追平 |
| 计算路由设置 | `/api/report-source-bindings*` | 报表取数路由绑定注册 CRUD |
| 兼容 | `/api/rpt/compute` | 旧 html 报表设计器预览兼容接口 |
| 监控大盘 | `/`、`/api/rpt/stats`、`/_mon` | 报表业务大盘 + 技术监控页 |

### ⚠️ 部署假设：单实例

报表数据字典物理表 `cr_*`（版式 BLOB / 单元格 / 浮动 / 来源绑定）全在 **fico-db**；服务层经全局 pg 管理器 + 固定 `RPT_DB_ID = "fico-db"` 读写。协同编辑 B 档用 PG 咨询锁串行化写操作。多实例可共享同一 fico-db（读多写少的报表场景一般可承受），但协同编辑高并发下建议前置单写协调。

---

## 快速开始

### 依赖
- **Rust**（见 `rust-toolchain.toml`，Edition 2024）
- **PostgreSQL**（`fico` 库，含 `cr_*` 报表数据字典表——由平台 `data/meta` 定义部署建表）
- **`../cmx-container/` 并排存在**（跨 workspace path 依赖）

### 启动

```bash
./report.sh            # 开发模式（debug，增量编译）
./report.sh --release  # 发布模式
```

`report.sh` 遵循统一启动契约：`cd` 到 workspace 根（`.env` / `*-server.toml` 相对路径基准）→ `cargo run -p cmx-rpt-server`（bin 自动读 `.env`，无需手动 source）。

启动后访问 **http://127.0.0.1:8092/**（监控大盘）、`/api/report-design/reports`（报表列表）、`/_mon`（技术监控）。

---

## 配置

三层来源，优先级 **环境变量 > `CONFIG_FILE` 指定的 toml > 内置默认**：

| 文件 | 作用 |
|---|---|
| `.env` | 环境变量。`dotenvy` 在启动最前读 cwd 的 `.env`。关键项：`CONFIG_FILE="./report-server.toml"`、`RPT_PORT`、`RPT_PG_URL` |
| `report-server.toml` | 主配置（`CONFIG_FILE` 指向）：`host`/`port`/`log_dir`/`log_level`（chassis 框架级）+ `[datasource] rpt_pg_url`（→ `RPT_PG_URL`） |

配置装配统一走 `ConfigManager`（与 flow/portal/mdm 同一制度：`CONFIG_FILE` toml + env → 全局 ConfigManager）。

---

## 目录结构

```
cmx-report/
├── crates/
│   ├── cmx-rpt-model/            # 中立模型（RPT_DB_ID + DTO）
│   ├── cmx-rpt-formula/          # 报表函数引擎
│   ├── cmx-rpt-store-pg/         # PG 持久化/服务层
│   ├── cmx-rpt-app/              # 中立核（handler + report_routes::<S> + 大盘）
│   └── cmx-rpt-server/           # 薄 bin：report banner + 大盘 + 数据源钩子
├── report.sh                     # 统一启动脚本
├── report-server.toml            # 主配置（CONFIG_FILE 指向）
├── .env                          # 环境变量（CONFIG_FILE / RPT_PORT / RPT_PG_URL）
├── .cargo/config.toml            # aliyun 镜像 + nora registry 定义（跨 ws 解析所需）
├── Cargo.toml                    # workspace 定义（5 成员 + 跨 ws path 依赖）
└── Cargo.lock                    # 锁定依赖（可复现构建）
```

> `target/`、`logs/`、`.env` 经 `.gitignore` 排除。`.cargo/config.toml` 与 `Cargo.lock` 刻意保留（前者跨 workspace 解析必需，后者保可复现构建）。

---

## 许可

[Apache-2.0](LICENSE)。
