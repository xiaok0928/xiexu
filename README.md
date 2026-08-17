# 协序（xiexu）

协序是面向项目管理、多 Agent 协作开发和自定义工作流的 Web 应用。当前目录已完成 M0 至 M5 的任务、协作、项目文档和工作流闭环，并提供 M6 本地 Docker 发布、备份恢复与路径安全基线。

## 启动

```bash
docker compose -f compose.yaml up --build
```

启动后访问 `http://localhost:8080`。健康检查地址为 `/healthz`，就绪检查地址为 `/readyz`。

`xiexu-app` 容器内部按顺序运行数据库迁移，并启动 Server 与 Runner；PostgreSQL 保持独立容器。M2 已提供执行控制面：Todo 任务会被 Runner 周期扫描并生成方案作业，方案完成后进入 `plan_review`；方案确认评论会生成 `execute_task` 作业，Runner 领取并记录租约、尝试、事件和输出，再将任务推进到 `acceptance`。验收通过评论会进入 `done`，返工评论会回到 `in_progress` 并重新执行。默认 `CODEX_EXECUTION_MODE=controlled`，用于验证控制面而不启动外部 CLI；设置为 `real` 后，Runner 才会在 `/workspace/projects/<project-id>` 内调用镜像自带的 Codex CLI。工作区、产物和 Codex 登录状态分别使用 Docker Volume 持久化。

M1 已提供项目、任务、父子任务、看板阶段、评论和事件时间线 API，Web 看板支持真实数据、拖拽移动、Todo 方案确认开关和评论。

M2 新增 `GET /api/tasks/:task_id/execution`，返回该任务的 `jobs`、`attempts`、`events` 和 `outputs`。Todo 扫描、评论驱动状态转换和进入 `in_progress` 的阶段转换均在事务边界内创建作业并写入事件；Runner 使用数据库行锁和 30 秒租约避免重复领取，并在任务状态变化和输出写入时追加任务时间线事件。任务详情已展示运行输出和执行事件数量。

M3 新增 33 个内置 Agent 角色模板、Agent 实例与职责补充、项目固定 Agent、任务动态 Agent、Agent 私有记忆、一对一对话、项目主群聊和项目临时群聊。每个项目都有独立协调 Agent；无主责 Todo 由协调 Agent 保底认领。项目群聊可以通过明确动作创建任务，临时群聊归档后由 Runner 生成总结。Agent 职责优化只生成草案，不自动覆盖配置；普通聊天消息不会隐式创建任务。

Codex 运行状态可通过 `GET /api/runtime/codex` 查看，接口只返回 CLI 是否安装、版本、执行模式和认证布尔值，不返回账号或令牌。首次启用真实模式时，在容器内完成认证并保留命名 Volume：

```bash
docker compose exec app codex login --device-auth
docker compose exec app codex login status
CODEX_EXECUTION_MODE=real docker compose up -d
```

## 部署与恢复

默认部署使用四个 Docker 命名卷：PostgreSQL 数据、受管工作区、交付产物和 `CODEX_HOME`。无需配置宿主机项目目录：

```bash
docker compose -f compose.yaml up --build -d
```

如需让 Runner 访问已有宿主机项目目录，先在 `.env` 中设置绝对路径 `XIEXU_PROJECTS_DIR`，再显式带上 bind override。该模式只覆盖 `/workspace/projects`，其余数据仍使用命名卷：

```bash
docker compose -f compose.yaml -f deploy/compose.bind.yaml up --build -d
```

备份和恢复必须从仓库根目录执行。备份会短暂停止应用容器，依次保存数据库、工作区、产物和 Codex 登录状态；恢复必须显式传入 `--confirm-restore`，会整体替换这些持久化数据并再次执行幂等迁移：

```bash
./deploy/backup.sh ./backups/20260817
./deploy/restore.sh --confirm-restore ./backups/20260817
```

`CODEX_HOME` 可能包含登录令牌，备份目录应存放在受控位置，不应提交到 Git、上传到公开对象存储或发送给不可信人员。脚本支持 macOS Docker Desktop、Linux Docker Engine 和 Windows Docker Desktop + WSL2 的 POSIX shell 环境；Windows 原生 CMD/PowerShell 不直接运行这些脚本。

`/readyz` 除数据库连接外还校验当前镜像要求的最新迁移版本。Runner 只允许在 `/workspace/projects/<UUID>` 中运行，符号链接逃逸、非法项目 ID 和未解析路径会被拒绝。

## 目录

- `apps/server`：Rust HTTP 服务和静态 Web 托管。
- `apps/runner`：Runner 注册、heartbeat、作业领取和受控执行。
- `apps/migrate`：PostgreSQL 启动迁移。
- `apps/web`：React + TypeScript + Vite Web Shell。
- `crates/*`：领域、应用和基础设施层占位。
- `deploy`：可选 bind mount 覆盖、全量备份与显式恢复脚本。
- `ai-doc`：仅本地使用的产品、架构、MVP、UI 与交付文档，已由 `.gitignore` 排除，不进入代码仓库。
