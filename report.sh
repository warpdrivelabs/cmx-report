#!/usr/bin/env bash
#
# 启动报表平台微服务（MEGA Report · :8092）。
#
# 统一启动契约（门户/流程/报表/主数据各服务同一套）：
#   1) cd 到本 workspace 根（.env / *-server.toml 的相对路径基准）
#   2) cargo run 对应 bin（bin 自动读 .env → 配置生效，无需手动 source）
#
# 用法：
#   ./report.sh                # 开发模式（debug，增量编译，改代码自动重编）
#   ./report.sh --release      # 发布模式（透传给 cargo run）
#
# 依赖：PostgreSQL（fico 库，含 cr_* 报表数据字典表）。访问 http://127.0.0.1:8092/
set -euo pipefail
cd "$(dirname "$0")"
exec cargo run -p cmx-rpt-server "$@"
