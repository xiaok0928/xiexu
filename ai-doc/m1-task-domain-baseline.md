# 协序 M1 任务域基础版

## 当前状态

M1 已实现并通过 Docker 验证。源码位于 `/Volumes/Tools/workspace/task-relay/xiexu`；该目录不是 Git 仓库，也没有 `.codegraph` 索引，本轮使用仓库级 CodeGraph readiness 检查后的 bounded fallback。

## 已交付能力

- `projects`：项目创建、列表、详情和名称/说明更新；项目创建时生成项目概览文档占位。
- `tasks`：标题、说明、项目归属、父任务、看板阶段、方案确认开关、计划状态、执行状态、验收状态、进度和 revision。
- 父子任务：父任务和子任务共用同一任务事实源，查询返回 `children_count`，前端支持卡片拖拽和移动端列切换。
- `task_comments`：评论追加保存，记录作者、正文、显式意图提示和是否应用状态转换；M1 不执行 AI 自然语言解释。
- `task_events`：创建、更新、阶段变更和评论事件追加保存，供任务时间线和后续 Agent 上下文使用。
- `task_relations`、`project_documents`、`project_document_versions`：完成后续依赖关系和文档版本能力的基础存储边界，M1 不自动调度或刷新文档。
- 阶段状态机：服务端校验 `backlog -> todo -> plan_review -> in_progress -> acceptance -> done` 及返工/取消分支；前端不能直接写阶段字段。
- Web 看板：接入真实 API，支持项目初始化、Backlog 想法创建、拖拽阶段转换、Todo 方案确认开关、任务详情评论。

## API

```text
GET/POST /api/projects
GET/PATCH /api/projects/:project_id
GET/POST /api/projects/:project_id/tasks
GET/PATCH /api/tasks/:task_id
POST /api/tasks/:task_id/transitions
GET/POST /api/tasks/:task_id/comments
GET /api/tasks/:task_id/events
```

M1 新建任务始终从 `backlog` 开始。评论中的 `intent` 只是非可信提示，服务端保存但不自动完成方案确认、验收或返工；真实语义解释和执行作业属于 M2/M3。

## 运行与迁移

- M0 `runner_instances` heartbeat 保留。
- Compose 对外只保留 `postgres` 和 `app` 两个服务；`app` 容器内部按顺序运行 migrate，并同时托管 server 与 runner。
- `0002_m1_task_domain` 在迁移进程中幂等创建 M1 表，并通过 `ALTER TABLE ... ADD COLUMN IF NOT EXISTS` 兼容已有 M0 数据卷。
- M1 不创建或消费业务 `execution_jobs`，不调用 Codex，不引入 Redis、Git 或 CodeGraph。
- 服务地址：`http://localhost:8080`。

## 验证证据

- `npx tsc --noEmit`：通过。
- `npm run build`：通过。
- `docker compose -f compose.yaml config`：通过。
- `docker compose -f compose.yaml build`：合并后的 app 镜像包含 server、runner、migrate 和 Web，构建通过。
- `docker compose -f compose.yaml up -d`：PostgreSQL、app 正常运行，app healthy；日志确认 server、runner 已在 app 内启动。
- `curl http://localhost:8080/readyz`：返回 `status=ready`。
- API smoke：创建项目、父子任务、Backlog 到 Todo、评论、事件时间线均成功；父子任务返回 `children_count`，评论 `transition_applied=false`。

## 已知限制

M1 只建立事实源和显式状态记录，不声称已具备 Todo 周期扫描、Agent 认领/指派、AI 方案生成、Codex 执行、自动验收、工作流运行或权限控制。上述能力按 M2 及后续里程碑实施。
