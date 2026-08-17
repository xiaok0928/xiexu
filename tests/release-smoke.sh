#!/bin/sh
set -eu

# M6 发布候选烟测：使用独立 Compose project，绝不操作默认 xiexu 服务、卷或宿主机项目目录。
repository_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
temporary_root=$(mktemp -d)
release_project="xiexu-release-smoke-$$"
release_port=${XIEXU_RELEASE_SMOKE_PORT:-}

# 未指定端口时由本机 Node 申请一个可用临时端口，避免与已有协序实例或其他开发服务冲突。
if [ -z "$release_port" ]; then
  release_port=$(node -e "const server = require('net').createServer(); server.listen(0, '127.0.0.1', () => { console.log(server.address().port); server.close(); });")
fi
export COMPOSE_PROJECT_NAME="$release_project"
export XIEXU_PORT="$release_port"
export COMPOSE_FILE="$repository_root/compose.yaml"

# 测试结束时只销毁本脚本自己创建的 project、卷、网络和临时备份目录。
cleanup() {
  docker compose --project-directory "$repository_root" down --volumes --remove-orphans >/dev/null 2>&1 || true
  rm -rf -- "$temporary_root"
}
trap cleanup EXIT INT TERM

# 等待发布容器完成迁移和 readiness，失败时输出诊断日志便于定位。
wait_ready() {
  attempt=0
  while [ "$attempt" -lt 60 ]; do
    if curl -fsS "http://127.0.0.1:$release_port/readyz" | grep -q '"status":"ready"'; then
      return 0
    fi
    attempt=$((attempt + 1))
    sleep 2
  done
  docker compose --project-directory "$repository_root" logs >&2 || true
  return 1
}

# 首次启动验证新卷迁移、应用存活和默认受管工作区。
docker compose --project-directory "$repository_root" up --build -d
wait_ready
docker compose --project-directory "$repository_root" exec -T app sh -eu -c 'test -d /workspace && test -d /var/lib/xiexu/artifacts && test -d /var/lib/xiexu/codex-home'
curl -fsS -X POST "http://127.0.0.1:$release_port/api/projects" -H 'content-type: application/json' \
  --data '{"name":"M6 恢复基线","description":"发布候选烟测"}' >/dev/null
docker compose --project-directory "$repository_root" exec -T app sh -eu -c \
  'printf original > /workspace/release-smoke.txt; printf artifact > /var/lib/xiexu/artifacts/release-smoke.txt'

# 备份后修改数据，再恢复并确认数据库和文件均回到备份时状态。
"$repository_root/deploy/backup.sh" "$temporary_root/backup"
curl -fsS -X POST "http://127.0.0.1:$release_port/api/projects" -H 'content-type: application/json' \
  --data '{"name":"M6 恢复后变更","description":"应被恢复移除"}' >/dev/null
docker compose --project-directory "$repository_root" exec -T app sh -eu -c \
  'printf changed > /workspace/release-smoke.txt; printf changed > /var/lib/xiexu/artifacts/release-smoke.txt'
"$repository_root/deploy/restore.sh" --confirm-restore "$temporary_root/backup"
wait_ready
docker compose --project-directory "$repository_root" exec -T app sh -eu -c \
  'test "$(cat /workspace/release-smoke.txt)" = original && test "$(cat /var/lib/xiexu/artifacts/release-smoke.txt)" = artifact'
curl -fsS "http://127.0.0.1:$release_port/api/projects" | grep -q 'M6 恢复基线'
if curl -fsS "http://127.0.0.1:$release_port/api/projects" | grep -q 'M6 恢复后变更'; then
  echo 'Restore retained data written after backup' >&2
  exit 1
fi

# 重建独立实例后启用 bind override，确认外部项目目录只挂到 projects 子目录。
docker compose --project-directory "$repository_root" down --volumes --remove-orphans
mkdir -p "$temporary_root/host-projects"
printf bind > "$temporary_root/host-projects/bind-marker.txt"
export XIEXU_PROJECTS_DIR="$temporary_root/host-projects"
export COMPOSE_FILE="$repository_root/compose.yaml:$repository_root/deploy/compose.bind.yaml"
docker compose --project-directory "$repository_root" up --build -d
wait_ready
docker compose --project-directory "$repository_root" exec -T app sh -eu -c 'test "$(cat /workspace/projects/bind-marker.txt)" = bind'

# Runner 路径逃逸保护由同一发布构建中的 Rust 单元测试覆盖。
docker build --target rust-build -t "$release_project-rust" "$repository_root" >/dev/null
docker run --rm "$release_project-rust" cargo test -p xiexu-runner
echo 'M6 release smoke passed'
