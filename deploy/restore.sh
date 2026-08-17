#!/bin/sh
set -eu

# 协序显式恢复入口：仅接受本项目 v1 清单，并在应用停止期间整体替换持久化数据。
repository_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
confirmation=${1:-}
backup_source=${2:-}

# 恢复具有固定参数和已存在的备份目录，避免误把任意目录当作可恢复快照。
if [ "$confirmation" != "--confirm-restore" ] || [ -z "$backup_source" ]; then
  echo "Usage: $0 --confirm-restore <backup-directory>" >&2
  exit 64
fi
backup_dir=$(CDPATH= cd -- "$backup_source" 2>/dev/null && pwd) || { echo "Backup directory not found: $backup_source" >&2; exit 66; }
for required_file in manifest.txt postgres.dump workspace.tar.gz artifacts.tar.gz codex-home.tar.gz; do
  if [ ! -f "$backup_dir/$required_file" ]; then
    echo "Incomplete backup, missing: $required_file" >&2
    exit 65
  fi
done
if ! grep -qx 'format=xiexu-backup-v1' "$backup_dir/manifest.txt"; then
  echo "Unsupported backup manifest" >&2
  exit 65
fi

# COMPOSE_FILE 与备份时保持同一部署拓扑，可覆盖为基础配置加 bind override。
export COMPOSE_FILE=${COMPOSE_FILE:-$repository_root/compose.yaml}
export COMPOSE_PROJECT_NAME=${COMPOSE_PROJECT_NAME:-xiexu}
docker compose --project-directory "$repository_root" stop app >/dev/null
restart_required=1
cleanup() {
  if [ "${restart_required:-0}" = 1 ]; then
    docker compose --project-directory "$repository_root" start app >/dev/null 2>&1 || true
  fi
}
trap cleanup EXIT INT TERM

# 数据库在应用停写后清理并恢复，所有连接参数仅由 PostgreSQL 容器环境提供。
docker compose --project-directory "$repository_root" exec -T postgres sh -eu -c \
  'pg_restore --clean --if-exists --no-owner --no-privileges --username="$POSTGRES_USER" --dbname="$POSTGRES_DB"' < "$backup_dir/postgres.dump"

# 每个数据根先清空再解包，确保恢复结果不混入备份之后产生的残留文件。
docker compose --project-directory "$repository_root" run --rm --no-deps -T --entrypoint sh app -eu -c \
  'find /workspace -mindepth 1 -maxdepth 1 -exec rm -rf -- {} +; tar -C /workspace -xzf -' < "$backup_dir/workspace.tar.gz"
docker compose --project-directory "$repository_root" run --rm --no-deps -T --entrypoint sh app -eu -c \
  'find /var/lib/xiexu/artifacts -mindepth 1 -maxdepth 1 -exec rm -rf -- {} +; tar -C /var/lib/xiexu/artifacts -xzf -' < "$backup_dir/artifacts.tar.gz"
docker compose --project-directory "$repository_root" run --rm --no-deps -T --entrypoint sh app -eu -c \
  'find /var/lib/xiexu/codex-home -mindepth 1 -maxdepth 1 -exec rm -rf -- {} +; tar -C /var/lib/xiexu/codex-home -xzf -' < "$backup_dir/codex-home.tar.gz"

# 恢复后再次执行幂等迁移，允许旧备份升级到当前镜像要求的数据库结构。
docker compose --project-directory "$repository_root" run --rm --no-deps -T --entrypoint /usr/local/bin/xiexu-migrate app
docker compose --project-directory "$repository_root" start app >/dev/null
restart_required=0
trap - EXIT INT TERM
echo "Restore completed from: $backup_dir"
