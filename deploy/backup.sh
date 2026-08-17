#!/bin/sh
set -eu

# 协序一致性备份入口：导出 PostgreSQL，并归档 workspace、artifacts 与 CODEX_HOME 命名卷。
repository_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
backup_target=${1:-}

# 备份目标必须由操作者显式指定，且不得覆盖已有目录或写入文件系统根目录。
if [ -z "$backup_target" ]; then
  echo "Usage: $0 <new-backup-directory>" >&2
  exit 64
fi
case "$backup_target" in
  /|.|..) echo "Refusing unsafe backup directory: $backup_target" >&2; exit 64 ;;
esac
if [ -e "$backup_target" ]; then
  echo "Backup directory already exists: $backup_target" >&2
  exit 73
fi
mkdir -p "$backup_target"
backup_dir=$(CDPATH= cd -- "$backup_target" && pwd)

# COMPOSE_FILE 可由 bind mount 部署覆盖；未设置时只使用仓库基础配置。
export COMPOSE_FILE=${COMPOSE_FILE:-$repository_root/compose.yaml}
export COMPOSE_PROJECT_NAME=${COMPOSE_PROJECT_NAME:-xiexu}

# 先停止会持续写入数据库和工作区的应用容器，保证四类数据来自同一静止窗口。
docker compose --project-directory "$repository_root" stop app >/dev/null
restart_required=1
cleanup() {
  if [ "${restart_required:-0}" = 1 ]; then
    docker compose --project-directory "$repository_root" start app >/dev/null 2>&1 || true
  fi
}
trap cleanup EXIT INT TERM

# 数据库使用 custom format，恢复时可安全执行 clean/if-exists；凭据只在容器内部展开。
docker compose --project-directory "$repository_root" exec -T postgres sh -eu -c \
  'pg_dump --format=custom --no-owner --no-privileges --username="$POSTGRES_USER" --dbname="$POSTGRES_DB"' > "$backup_dir/postgres.dump"

# 通过应用镜像读取同一组挂载，归档保留相对路径且不把宿主机路径写入备份。
docker compose --project-directory "$repository_root" run --rm --no-deps -T --entrypoint sh app -eu -c \
  'tar -C /workspace -czf - .' > "$backup_dir/workspace.tar.gz"
docker compose --project-directory "$repository_root" run --rm --no-deps -T --entrypoint sh app -eu -c \
  'tar -C /var/lib/xiexu/artifacts -czf - .' > "$backup_dir/artifacts.tar.gz"
docker compose --project-directory "$repository_root" run --rm --no-deps -T --entrypoint sh app -eu -c \
  'tar -C /var/lib/xiexu/codex-home -czf - .' > "$backup_dir/codex-home.tar.gz"

# 清单不记录令牌或数据库密码，只声明可验证的备份组成和敏感性。
cat > "$backup_dir/manifest.txt" <<'EOF'
format=xiexu-backup-v1
contains=postgres,workspace,artifacts,codex-home
sensitive=true
EOF
chmod 600 "$backup_dir"/*

# 完成后恢复应用运行，EXIT trap 仅作为异常兜底。
docker compose --project-directory "$repository_root" start app >/dev/null
restart_required=0
trap - EXIT INT TERM
echo "Backup created: $backup_dir"
