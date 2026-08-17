# 协序 MVP BE 初始对齐

日期：2026-08-14

角色：BE

依据：

- `product-baseline_20260814/documents/product-decision-baseline.md`
- `xiexu-architecture_20260814/documents/architecture-baseline.md`
- `xiexu-architecture_20260814/documents/system-architecture.md`
- `xiexu-mvp-plan_20260814/documents/mvp-scope.md`
- `xiexu-mvp-plan_20260814/PM/initial-alignment.md`
- `xiexu-mvp-plan_20260814/SA/initial-alignment.md`
- `xiexu-mvp-plan_20260814/SRE/initial-alignment.md`
- 本地参考项目：`/Volumes/Tools/workspace/task-relay/relay`

## 1. BE 结论

MVP 后端不要从完整项目管理或完整工作流编排开始。应先固化一个 PostgreSQL 驱动的执行协议和任务状态机，再把 Runner、Workflow、Agent 记忆、交付物与项目文档逐步接入同一条执行链。

后端首轮目标是证明：

1. `server` 可以管理项目、任务、评论、状态流转、工作流定义和运行记录。
2. `runner` 可以通过 PostgreSQL job/lease 领取任务、续租、写事件、写输出，并把结果反馈给任务/工作流。
3. 不依赖 Redis、不依赖宿主机额外安装、不要求 Git，即可在 Docker Compose 内完成最小闭环。

## 2. 后端实施切片

### M0：Rust workspace 与基础运行骨架

范围：

- 建立 `apps/server`、`apps/runner`、`apps/migrate`、`crates/domain`、`crates/application`、`crates/infrastructure`、`crates/shared`。
- `server` 提供 HTTP API、health/readiness、SSE 或 WebSocket 事件出口。
- `migrate` 作为 one-shot migration 服务。
- `runner` 作为常驻进程，只通过 PostgreSQL 与 `server` 协作。

BE 关注点：

- 复用 Relay 的 workspace 分层方向：domain/application/infrastructure/shared。
- 依赖选择可参考 Relay：`axum`、`tokio`、`serde`、`uuid`、`chrono`、`tokio-postgres` 或等价 PostgreSQL client。
- 不复用 Relay 的 company tenancy 语义。协序 MVP 已暂缓权限系统，不引入公司概念。

验收：

- `docker compose up` 后 `postgres`、`migrate`、`server`、`runner` 启动顺序清晰。
- `server /readyz` 能确认数据库迁移已完成。
- `runner` 能注册自身并维持 heartbeat。

### M1：数据库迁移与核心实体

第一批表建议：

- `projects`
- `project_documents`
- `project_document_versions`
- `agents`
- `agent_profiles`
- `agent_memories`
- `tasks`
- `task_relations`
- `task_comments`
- `task_events`
- `execution_jobs`
- `execution_attempts`
- `execution_events`
- `runner_instances`
- `run_outputs`
- `artifacts`
- `artifact_versions`
- `attachments`

第二批表建议：

- `workflow_definitions`
- `workflow_versions`
- `workflow_runs`
- `workflow_node_runs`
- `workflow_edges`
- `approval_requests`
- `scheduler_triggers`

实体边界：

- `tasks` 负责看板可见状态、父子关系、需求/子任务承载、方案确认标记、验收状态。
- `execution_jobs` 负责后端执行队列，不直接代表用户看板状态。
- `workflow_runs` 负责一次工作流运行记录，不直接吞掉任务实体。
- `run_outputs` 和 `artifacts` 负责不可变输出，不把大内容塞进任务表。

验收：

- migration 可重复检测已执行版本。
- 核心实体支持软状态流转，不靠删除表达业务结束。
- 常用查询有索引：项目任务列表、任务父子树、job 领取、attempt lease、workflow run 明细、任务评论时间线。

### M2：任务看板状态机与评论语义入口

MVP 任务主状态建议按已确认口径实现：

- `backlog`：仅记录想法，不自动执行。
- `todo`：用户确认要做，等待协调 Agent 扫描和认领。
- `plan_review`：Agent 输出方案，等待 Human 评论确认。
- `processing`：执行中。
- `acceptance`：等待 Human 验收。
- `done`：验收完成后从活跃看板消失，仍保留历史。

关键规则：

- 任务从 `backlog` 进入 `todo` 只能由用户手动触发。
- `todo` 卡片默认 `requires_plan_review = true`，卡片右下角可取消。
- 方案确认、验收成功、验收失败、补充意见都通过评论进入，由后端记录原始评论和 AI 解析后的意图。
- 父任务可展开/收缩，父任务展示聚合进度，子任务展示实时子任务进度。
- 父任务验收通过时，未单独返工的子任务全部通过；单个子任务返工时，父任务进入部分验收或重新聚合。
- 返工后回到 `processing`，不是 `todo`，因为问题已经进入实现闭环。

BE API 前置项：

- 创建/更新任务。
- 拖拽变更任务主状态。
- 创建评论。
- 查询任务树与聚合进度。
- 查询任务事件时间线。
- 评论语义解析先保留接口边界，MVP 可由 Agent job 写入解析结果。

### M3：Execution Job 协议

Job 类型建议先收敛到以下最小集合：

- `scan_todo_tasks`
- `prepare_task_plan`
- `execute_task`
- `process_acceptance_comment`
- `run_workflow`
- `run_workflow_node`
- `refresh_project_document`
- `memory_extract`

核心字段建议：

- `id`
- `job_type`
- `scope_type`
- `scope_id`
- `project_id`
- `task_id`
- `workflow_run_id`
- `workflow_node_run_id`
- `agent_id`
- `status`
- `priority`
- `payload`
- `not_before`
- `lease_owner`
- `lease_expires_at`
- `attempt_count`
- `max_attempts`
- `created_at`
- `updated_at`

领取语义：

- `runner` 使用数据库事务领取 `queued` 且到期的 job。
- 领取时写入 `execution_attempts`，并设置 `lease_owner`、`lease_expires_at`。
- 长任务必须 heartbeat 续租。
- lease 到期后允许其他 runner 重新领取，但必须通过幂等键避免重复落同一份业务结果。
- job 失败写结构化错误，是否重试由 `max_attempts` 和错误类型决定。

验收：

- 多 runner 并发时同一个 job 只能有一个有效 attempt。
- server 重启不丢 job。
- runner 崩溃后 lease 过期可恢复。
- job 事件可实时推送给前端看板和运行详情。

### M4：Runner 与 Codex 调用

Runner MVP 不需要先做复杂调度。它只要能：

- 注册 `runner_instances`。
- 领取 `execution_jobs`。
- 根据 job 类型构造 Codex prompt 和上下文。
- 在容器内执行 Codex CLI。
- 把 stdout/stderr、结构化结果、事件、产物引用写回 PostgreSQL/文件存储。
- 根据后端状态机规则请求 server 或 application service 推进任务状态。

可参考 Relay：

- `apps/agent-trigger` 的后台触发进程组织方式。
- `crates/infrastructure/src/codex_control*` 的 Codex 调用、请求存储、运行探测和校验思路。
- `crates/infrastructure/src/codex_trigger*` 的 agent trigger 与 app server 交互边界。

协序必须新建：

- 基于 `execution_jobs` 的通用 job/attempt/lease 协议。
- 面向任务、工作流、项目文档、记忆提取的统一 Runner dispatcher。
- Docker 内 workspace path canonical 校验。
- `run_outputs` 与 artifact metadata 的不可变输出协议。

### M5：工作流定义、版本与运行

MVP 支持的流程元素：

- 开始节点。
- 结束节点。
- 执行节点。
- 判断节点。
- 人工确认节点。
- 连接线。

规则：

- 工作流定义保存后形成可运行版本。
- 运行中的 workflow run 不允许被已编辑定义影响。
- 判断节点只需要 `是/否` 两类连接线，不引入复杂规则引擎。
- 人工确认节点创建 `approval_requests`，等待有数据权限的用户评论或确认后继续。
- 工作流可以一次执行、预定时间执行、周期性重复执行，也可以通过 AI 解析重复规则后结构化存储。
- 暂停自动化流程时，后续触发暂停，正在运行的实例也挂起；终止代表放弃，不自动恢复。

后端实现顺序：

1. `workflow_definitions` 与 `workflow_versions`。
2. `workflow_runs` 与 `workflow_node_runs`。
3. 节点执行 dispatcher。
4. 人工确认挂起与恢复。
5. scheduler trigger。
6. 工作流运行记录、输出提取和任务卡片映射。

### M6：Agent、记忆、项目文档与交付物

Agent：

- 先搬 Relay 默认 profession catalog，去掉公司语义。
- Agent profile 允许用户创建，并由 AI 优化职责。
- 项目固定 Agent 与动态协助 Agent 不冲突：固定 Agent 是项目默认班底，动态 Agent 是任务执行过程中的临时参与者。

记忆：

- Agent 私有记忆是固定 Agent 在执行任务时沉淀的经验、解决方案、偏好和项目上下文，不是用户私密数据。
- 项目记忆和项目文档分开：项目文档面向项目事实与交付状态，Agent 记忆面向执行经验。
- 记忆写入不应被过严限制，否则 Agent 无法持续学习；但必须保留来源任务、来源运行、可信度、更新时间和可追溯证据。

项目文档：

- 创建项目后参考 Relay 生成项目文档。
- 父任务完成后刷新项目文档。
- 定时任务兜底检查遗漏变更。
- 文档支持 `@` 协作线索，任务评论也支持 `@` Agent 或相关任务。

交付物：

- 文件放 Docker Volume，PostgreSQL 保存 metadata/version/hash/mime/size/source。
- 输出从运行记录提取，同时可查看子任务输出。
- 不要求 Git 作为交付物管理前提。

## 3. API 与事件契约前置项

BE 在 FE/UI 开始深度联调前需要先稳定这些契约。

任务 API：

- `POST /api/projects`
- `GET /api/projects`
- `GET /api/projects/{project_id}`
- `POST /api/projects/{project_id}/tasks`
- `GET /api/projects/{project_id}/tasks?view=board`
- `PATCH /api/tasks/{task_id}`
- `POST /api/tasks/{task_id}/move`
- `POST /api/tasks/{task_id}/comments`
- `GET /api/tasks/{task_id}/timeline`
- `POST /api/tasks/{task_id}/acceptance`

执行 API：

- `GET /api/execution/jobs/{job_id}`
- `GET /api/runs/{run_id}`
- `GET /api/runs/{run_id}/outputs`
- `GET /api/projects/{project_id}/events`

工作流 API：

- `POST /api/workflows`
- `GET /api/workflows`
- `GET /api/workflows/{workflow_id}`
- `POST /api/workflows/{workflow_id}/versions`
- `POST /api/workflows/{workflow_id}/runs`
- `POST /api/workflow-runs/{run_id}/pause`
- `POST /api/workflow-runs/{run_id}/resume`
- `POST /api/workflow-runs/{run_id}/terminate`
- `POST /api/approval-requests/{approval_id}/comments`

实时事件建议：

- `task.created`
- `task.updated`
- `task.moved`
- `task.comment.created`
- `task.intent.detected`
- `execution.job.queued`
- `execution.attempt.started`
- `execution.event.appended`
- `execution.output.created`
- `workflow.run.started`
- `workflow.node.started`
- `workflow.node.waiting_human`
- `workflow.node.completed`
- `workflow.run.completed`
- `workflow.run.paused`
- `workflow.run.terminated`

## 4. 与 Relay 的复用边界

建议复用或迁移思想：

- Rust workspace 分层与 crate 依赖方向。
- PostgreSQL repository 组织方式。
- migration 目录结构。
- Agent profession catalog 的默认角色种子。
- Agent memory contracts 的字段思路：scope、tier、type、tags、importance、confidence、source refs、expires_at。
- MCP server 和 tool contract 的组织方式。
- Codex control/trigger 的运行探测、请求校验、外部进程调用经验。
- realtime events 的基础模型。

不建议直接照搬：

- Relay 的 company tenancy、company project、staffing 权限模型。
- Relay 面向外部 Agent 服务的业务语义。
- 现有 problem workspace 命名和状态结构。
- 与 Git/公司协作绑定较深的项目治理语义。

必须新建：

- 协序任务看板状态机。
- 父子任务聚合进度。
- 评论语义驱动的状态流转。
- 工作流定义/版本/运行/节点运行。
- PostgreSQL job/attempt/lease 队列协议。
- Docker 内 Runner workspace path 安全模型。
- 文件交付物不可变版本与运行输出提取。

## 5. 依赖与风险

主要依赖：

- SA 需要确认最终 module/package 命名和 domain 边界。
- SRE 需要确认 compose 服务名、volume、环境变量和 allowed roots。
- FE/UI 需要尽早拿到任务树、看板列、卡片字段、实时事件 schema。
- PM 需要冻结 MVP 的任务状态文案和“从看板消失但历史可查”的表现口径。

主要风险：

- 如果先做完整工作流画布，后端会提前背负过多规则引擎复杂度。建议先做二分判断和人工确认。
- 如果没有 job/attempt/lease 协议，Runner 并发和崩溃恢复会不可控。
- 如果任务状态和执行状态混用，看板会很快失真。两者必须分表或至少分字段建模。
- 如果输出直接写任务字段，后续运行记录、子任务输出、交付物版本都会难以追溯。
- 如果容器允许访问任意宿主机路径，MVP 安全风险过高。必须通过 allowed roots 和 bind mount 显式暴露。
- 如果 Agent 记忆没有来源和可信度，后续长期记忆会污染任务执行。

工作量高风险区：

- Codex CLI 在容器内的认证、隔离、超时、日志截断和结果结构化。
- 评论自然语言意图解析与状态流转的准确性。
- workflow run 暂停/恢复/终止与正在运行 attempt 的一致性。
- 项目文档自动刷新与任务事件的去重。
- 多 runner 并发领取、重复执行、幂等落库。

## 6. 建议最早端到端切片

最早 E2E 不做完整画布，也不做完整 Agent 角色市场。建议路径：

1. 用户创建项目。
2. 用户创建 `todo` 任务，默认 `requires_plan_review = true`。
3. server 创建 `prepare_task_plan` job。
4. runner 领取 job，写 `execution_events`，生成方案输出。
5. 任务进入 `plan_review`。
6. Human 在任务评论里确认方案。
7. server 创建 `execute_task` job。
8. runner 执行任务，写 `run_outputs` 和 artifact metadata。
9. 任务进入 `acceptance`。
10. Human 评论验收通过。
11. 任务进入 `done`，历史、运行记录和输出仍可查询。

这个切片验证的后端能力：

- migration 与基础实体。
- 任务状态机。
- 评论语义入口。
- job/attempt/lease。
- runner heartbeat 与事件写入。
- run output 与 artifact metadata。
- SSE/WebSocket 实时刷新。
- Human 确认与验收闭环。

## 7. BE 下一步任务清单

1. 与 SA 对齐 `crates/domain` 的实体命名和枚举状态。
2. 输出第一批 migration 设计草案，先覆盖 M1 核心实体。
3. 输出 `execution_jobs` 领取 SQL 和并发语义草案。
4. 输出任务状态机 transition table。
5. 与 FE/UI 对齐 board query response schema。
6. 与 SRE 对齐 runner 环境变量、workspace path canonical 校验和日志截断策略。
7. 与 PM 对齐 MVP 卡片字段与评论意图枚举。

## 8. 暂不进入的事项

- 不实现权限系统。
- 不实现公司/组织概念。
- 不实现 Redis。
- 不实现 Git 分支/worktree 合并能力。
- 不实现复杂规则引擎。
- 不实现批量暂停、批量终止、批量迁移。
- 不把 Backlog 自动转为可执行任务。
- 不把外部宿主机任意路径暴露给 Runner。
