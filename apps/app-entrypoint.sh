#!/bin/sh
set -eu

# 先完成数据库迁移，迁移失败时不启动依赖数据库结构的应用进程。
/usr/local/bin/xiexu-migrate

# Runner 与 Server 共享同一个应用容器，但仍保持各自独立的进程职责。
/usr/local/bin/xiexu-runner &
runner_pid=$!
/usr/local/bin/xiexu-server &
server_pid=$!

# 任一应用进程退出时，终止同容器内的另一个进程，交给 Compose 统一重启。
cleanup() {
  kill "$server_pid" "$runner_pid" 2>/dev/null || true
}
trap cleanup INT TERM EXIT

# Server 是对外服务主进程，等待它结束并返回其退出码。
wait "$server_pid"
