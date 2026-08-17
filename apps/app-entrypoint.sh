#!/bin/sh
set -eu

# 先完成数据库迁移，迁移失败时不启动依赖数据库结构的应用进程。
/usr/local/bin/xiexu-migrate

# Runner 与 Server 共享同一个应用容器，但仍保持各自独立的进程职责。
/usr/local/bin/xiexu-runner &
runner_pid=$!
/usr/local/bin/xiexu-server &
server_pid=$!

# 任一应用进程退出时，终止同容器内的另一个进程，避免 Server 健康但 Runner 已停止的半失效状态。
cleanup() {
  kill "$server_pid" "$runner_pid" 2>/dev/null || true
}
trap cleanup INT TERM EXIT

# POSIX sh 没有可移植的 wait -n，因此用存活探测等待首个子进程结束。
while kill -0 "$server_pid" 2>/dev/null && kill -0 "$runner_pid" 2>/dev/null; do
  sleep 1
done

# 首个子进程退出后统一清理；即使正常退出也返回失败，让 Compose 恢复完整应用。
cleanup
wait "$server_pid" 2>/dev/null || true
wait "$runner_pid" 2>/dev/null || true
exit 1
