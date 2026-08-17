# 协序（xiexu）

协序是面向项目管理、多 Agent 协作开发和自定义工作流的 Web 应用。当前目录已包含 M0 运行骨架和 M1 任务域基础版。

## 启动

```bash
docker compose -f compose.yaml up --build
```

启动后访问 `http://localhost:8080`。健康检查地址为 `/healthz`，就绪检查地址为 `/readyz`。

`xiexu-app` 容器内部按顺序运行数据库迁移，并启动 Server 与 Runner；PostgreSQL 保持独立容器。M2 已提供执行控制面：Todo 任务会被 Runner 周期扫描并生成方案作业，方案完成后进入 `plan_review`；方案确认评论会生成 `execute_task` 作业，Runner 领取并记录租约、尝试、事件和输出，再将任务推进到 `acceptance`。验收通过评论会进入 `done`，返工评论会回到 `in_progress` 并重新执行。默认 `CODEX_EXECUTION_MODE=controlled`，用于验证控制面而不启动外部 CLI；设置为 `real` 后，Runner 才会在 `/workspace/projects/<project-id>` 内调用镜像自带的 Codex CLI。工作区、产物和 Codex 登录状态分别使用 Docker Volume 持久化。

M1 已提供项目、任务、父子任务、看板阶段、评论和事件时间线 API，Web 看板支持真实数据、拖拽移动、Todo 方案确认开关和评论。M1 不包含 Todo 周期扫描、Agent 认领、AI 评论理解或 Codex 执行。

M2 新增 `GET /api/tasks/:task_id/execution`，返回该任务的 `jobs`、`attempts`、`events` 和 `outputs`。Todo 扫描、评论驱动状态转换和进入 `in_progress` 的阶段转换均在事务边界内创建作业并写入事件；Runner 使用数据库行锁和 30 秒租约避免重复领取，并在任务状态变化和输出写入时追加任务时间线事件。任务详情已展示运行输出和执行事件数量。

Codex 运行状态可通过 `GET /api/runtime/codex` 查看，接口只返回 CLI 是否安装、版本、执行模式和认证布尔值，不返回账号或令牌。首次启用真实模式时，在容器内完成认证并保留命名 Volume：

```bash
docker compose exec app codex login --device-auth
docker compose exec app codex login status
CODEX_EXECUTION_MODE=real docker compose up -d
```

## 目录

- `apps/server`：Rust HTTP 服务和静态 Web 托管。
- `apps/runner`：Runner 注册、heartbeat、作业领取和受控执行。
- `apps/migrate`：PostgreSQL 启动迁移。
- `apps/web`：React + TypeScript + Vite Web Shell。
- `crates/*`：领域、应用和基础设施层占位。
- `ai-doc`：产品、架构、MVP、UI 与交付文档。
