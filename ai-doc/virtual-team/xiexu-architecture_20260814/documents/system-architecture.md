# 协序 xiexu MVP 系统架构

日期：2026-08-14
角色：SA
状态：已确认
依据：`architecture-baseline.md` 已确认 Rust、PostgreSQL、无 Redis、容器内 Codex Runner、托管工作区 + 可选 bind mount。

## 0. 范围与原则

本文件只定义 MVP 范围内的系统边界，不展开 SRE 部署细节。

MVP 长期服务最小集合：

- `postgres`：唯一事实源。
- `server`：Rust HTTP/API/Web control plane，同时持有 scheduler loop。
- `runner`：Rust 后台执行器，容器内调用 Codex Runner adapter。
- `migrate`：一次性迁移进程，不是长期服务。

首版不引入：

- Redis。
- 独立 scheduler 服务。
- 独立 artifact 服务。
- 独立 realtime 服务。
- 独立 web/nginx 服务，Web 静态资源由 `server` 托管。

关键边界：

- Scheduler 属于 `server`，不属于 `runner`。
- `server` 负责决定“什么时候创建 execution job”。
- `runner` 只负责领取、执行和回写 execution job。
- 看板阶段与执行状态分离，执行异常不新增看板列。

## 1. Rust Workspace 与模块边界

建议 workspace：

```text
xiexu/
  apps/
    server/
    runner/
    migrate/
    mcp-server/
    web/
  crates/
    domain/
    application/
    infrastructure/
    mcp/
    api/
    shared/
```

`apps/server`

- 提供 HTTP API、WebSocket/SSE、静态 Web 资源。
- 处理用户操作：项目、任务、评论、workflow、审批、运行记录、Agent、记忆、artifact 查询。
- 持有 scheduler loop，扫描 due schedule 并创建 `execution_jobs`。
- 处理错过的定时触发：不自动补跑，server 重启后跳过已过期触发并推进 `next_fire_at` 到下一次未来时间。
- 只写控制状态，不执行 Codex，不运行项目命令。
- 不挂载用户项目目录，最多访问协序数据目录中的附件和 artifact。

`apps/runner`

- 后台常驻进程。
- 注册 `runner_instances`，从 PostgreSQL 领取 `execution_jobs`，创建 `execution_attempts`，续租，执行，写事件。
- 容器内调用 Codex Runner adapter。
- 挂载托管工作区、Codex profile、artifact volume、可选 bind mount 根目录。
- 负责暂停、终止、审批等待、失败重试、lease 续期。
- 不负责 schedule 扫描和 job 创建。

`apps/migrate`

- 执行数据库迁移。
- Compose 启动阶段运行一次，完成后退出。
- 不处理业务请求。

`apps/mcp-server`

- 给 Codex 使用的 MCP stdio binary。
- 不作为 Compose 长期服务。
- Runner 可把它写入 Codex profile 或由 Relay app-server `--stdio` 启动。

`apps/web`

- React/TypeScript 前端。
- 构建产物由 `server` 托管。
- 借鉴 Dashi Taskboard 的看板密度、评论活动、工作流画布体验。

`crates/domain`

- 只放领域对象、值对象、状态枚举、状态机规则、不可变业务约束。
- 不依赖 HTTP、PostgreSQL、文件系统、Codex CLI。
- 不绑定 `codex exec --json`；domain 只认识中性的执行事件和输出引用。

`crates/application`

- 放用例服务和端口 trait。
- 负责状态流转编排、事务边界定义、权限预留点、业务校验。
- 典型服务：`TaskService`、`WorkflowService`、`CommentService`、`ApprovalService`、`SchedulerService`、`ExecutionJobService`、`AgentService`、`MemoryService`、`ArtifactService`、`WorkspaceService`。

`crates/infrastructure`

- 放 PostgreSQL repository、execution queue、`LISTEN/NOTIFY`、文件 artifact store、Codex adapter、workspace manager、schedule scanner 实现。
- 实现 application 中的端口 trait。
- 所有外部副作用在此落地。

`crates/mcp`

- 定义 MCP tools schema 和 handler。
- 工具能力保持 MVP 最小：任务读取/更新、评论、记忆、artifact、项目文档、审批回填。

`crates/api`

- HTTP DTO、分页、错误响应、SSE/WebSocket event view。
- 与 domain 分离，避免 API 字段污染领域模型。

`crates/shared`

- 通用错误、时间、ID、配置、脱敏、JSON helper。
- 只放跨 crate 需要的低层公共能力。

## 2. 核心领域对象及关系

用户与 Agent：

- `User`：Human 账号。MVP 暂不实现复杂权限，但保留 `created_by_user_id`、`updated_by_user_id`、审计字段。
- `Agent`：固定 Agent 身份。
- `AgentRoleProfile`：Agent 身份定义，参考 Relay profession/role catalog，可由 Human 后续自定义。
- `AgentMemory`：固定 Agent 私有记忆，区分 `short_term`、`long_term`，可关联项目、任务、run、artifact。
- `AgentCodexProfile`：Codex 运行 profile，包括 `CODEX_HOME`、model、approval policy、MCP 配置、运行环境。

项目与工作区：

- `Project`：协序中的项目空间，不等同于 Git 仓库。
- `ProjectWorkspace`：项目执行工作区配置。
- `ManagedWorkspace`：协序托管工作区，位于 runner volume。
- `BindMountRoot`：可选宿主机 bind mount 根目录映射，保存 host hint 和 container root。
- `ProjectDocument` / `ProjectDocumentVersion`：项目文档及版本，由父任务完成刷新和定时兜底刷新产生。

任务与评论：

- `Task`：看板卡片，可代表需求、研发子任务、运营任务、workflow 产生的任务。
- `Task.board_stage`：用户可见看板阶段，只允许 `backlog`、`todo`、`plan_review`、`in_progress`、`acceptance`、`done`、`cancelled`。
- `Task.plan_status`：方案子状态，例如 `not_required`、`pending_generation`、`generating`、`generated`、`reviewing`、`approved`、`change_requested`、`failed`。
- `Task.execution_status`：执行子状态，例如 `idle`、`queued`、`running`、`blocked`、`failed`、`succeeded`、`cancelled`。
- `TaskRelation`：父子关系、阻塞关系、依赖关系。父任务展示聚合进度，子任务展示实时进度。
- `TaskComment`：用户或 Agent 评论，append-only。
- `CommentIntent`：评论语义解析结果，例如确认方案、要求修改、验收通过、验收失败、补充信息、@协助。
- `TaskActivity`：任务活动事件，用于看板和详情页展示。

工作流：

- `WorkflowDefinition`：工作流逻辑定义。
- `WorkflowVersion`：保存后的不可变版本；运行时引用 version，不直接引用可编辑草稿。
- `WorkflowNode`：开始、结束、执行、判断、人工确认节点。
- `WorkflowEdge`：连接线，判断节点出边必须标记 `yes` / `no`。
- `WorkflowSchedule`：一次执行、预定时间、周期重复、AI 解析后的结构化时间规则。
- `WorkflowRun`：一次运行实例。
- `WorkflowNodeRun`：节点运行实例。

执行与输出：

- `ExecutionJob`：server/runner 的工作协议对象，对应表 `execution_jobs`。
- `ExecutionAttempt`：runner 对 job 的一次执行尝试，对应表 `execution_attempts`。
- `ExecutionEvent`：append-only 执行事件，对应表 `execution_events`。
- `RunnerInstance`：runner 注册与 run lease，对应表 `runner_instances`。
- `RunOutput`：运行输出索引，对应表 `run_outputs`，可引用 artifact。
- `Artifact` / `ArtifactVersion`：运行输出、附件、文档、报告、补丁、截图等文件元数据与版本。
- `ApprovalRequest`：人工确认或 Codex tool approval。

核心关系简化：

```text
Project 1 - n Task
Task 1 - n TaskComment
Task 1 - n TaskActivity
Task 1 - n TaskRelation
WorkflowDefinition 1 - n WorkflowVersion
WorkflowVersion 1 - n WorkflowRun
WorkflowRun 1 - n WorkflowNodeRun
WorkflowRun 1 - n Task
WorkflowRun 1 - n ExecutionEvent
WorkflowRun 1 - n RunOutput
Task n - 1 WorkflowRun via source_workflow_run_id
Task n - 1 WorkflowNodeRun via source_node_run_id
Agent 1 - n AgentMemory
Agent 1 - n ExecutionJob
ExecutionJob 1 - n ExecutionAttempt
ExecutionAttempt 1 - n ExecutionEvent
ApprovalRequest n - 1 Task | WorkflowRun | WorkflowNodeRun | ExecutionJob
```

边界修正：

- 避免把 Task 建模为 WorkflowRun 的父级。
- 工作流产生任务时，任务通过 `source_workflow_run_id` 和 `source_node_run_id` 回指来源。
- 手工创建任务的 source 字段为空。

## 3. 状态机边界

### 3.1 看板阶段与执行子状态

用户可见看板阶段只允许：

- `backlog`：想法记录，只保存，不执行。
- `todo`：已确认要做，等待协调 Agent 扫描/认领/指派。
- `plan_review`：等待 Human 确认方案。
- `in_progress`：执行中或执行编排中。
- `acceptance`：等待 Human 验收。
- `done`：验收完成。
- `cancelled`：Human 取消。

不作为看板列：

- `planning`
- `queued`
- `running`
- `blocked`
- `failed`

这些状态落在 `plan_status` 或 `execution_status`。

### 3.2 方案子状态

`plan_status`：

- `not_required`
- `pending_generation`
- `generating`
- `generated`
- `reviewing`
- `approved`
- `change_requested`
- `failed`

主要规则：

- `todo` 中默认 `plan_status = pending_generation`。
- 用户取消方案确认时，`plan_status = not_required`，任务可进入执行排队。
- 方案生成中，`board_stage` 仍是 `todo`。
- 方案生成完成后，`board_stage = plan_review`。
- Human 确认方案后，`plan_status = approved`，`board_stage = in_progress`。
- Human 要求改方案时，`plan_status = change_requested`，`board_stage` 仍为 `plan_review`，server 创建新的方案生成 job。

### 3.3 执行子状态

`execution_status`：

- `idle`
- `queued`
- `running`
- `blocked`
- `failed`
- `succeeded`
- `cancelled`

主要规则：

- `execution_status = queued/running/blocked/failed` 不新增看板列。
- 执行排队或运行时，`board_stage = in_progress`。
- 执行阻塞时，`board_stage = in_progress`，卡片展示 blocked 标记和阻塞原因。
- 执行失败时，`board_stage = in_progress`，卡片展示 failed 标记；Human 评论决定返工、取消或重新规划。
- 执行成功后，普通任务进入 `board_stage = acceptance`；工作流内部任务可按节点策略直接完成或生成验收任务。

### 3.4 看板主流转

```text
backlog -> todo
todo -> plan_review
todo -> in_progress
plan_review -> in_progress
in_progress -> acceptance
acceptance -> done
acceptance -> todo
acceptance -> in_progress
任何非终态 -> cancelled
```

边界规则：

- `backlog` 不触发 Agent 执行。
- `todo` 默认需要方案确认；任务卡片可取消“需要方案确认”。
- 方案生成、执行排队、执行运行、阻塞、失败都是子状态，不改变看板列集合。
- `acceptance` 中验收父任务等于验收全部子任务；验收单个子任务会让父任务展示部分验收聚合态。
- 验收失败通过评论表达，评论语义决定回到 `todo` 重新规划，或回到 `in_progress` 直接返工。

### 3.5 评论状态边界

评论本身不直接改任务状态。

流程：

```text
TaskComment created
-> comment_interpretation execution_job
-> CommentIntent generated
-> server/application 校验当前 board_stage、plan_status、execution_status 和 actor
-> 产生 TaskTransition / ApprovalDecision / MentionEvent
```

MVP `CommentIntent`：

- `approve_plan`
- `request_plan_changes`
- `accept_task`
- `reject_task`
- `clarify_requirement`
- `mention_agent`
- `no_state_change`

边界规则：

- 评论 append-only，不覆盖历史。
- AI 解析只能生成意图，最终状态流转必须走 application 状态机。
- 状态不匹配时意图作废，例如 `accept_task` 只能作用于 `acceptance`。

### 3.6 审批状态机

MVP 审批状态：

- `pending`
- `approved`
- `rejected`
- `cancelled`
- `expired`

适用场景：

- 任务方案确认。
- workflow 人工确认节点。
- Codex tool approval。

边界规则：

- 人工确认节点不需要单独“拒绝节点”。Human 评论由语义解析决定继续、补充、转向或终止。
- 审批只能被绑定对象消费一次。
- 审批完成必须写 `execution_events` 和业务 transition。

### 3.7 工作流状态机

Workflow 定义状态：

- `draft`
- `active`
- `paused`
- `archived`

Workflow run 状态：

- `queued`
- `running`
- `waiting_approval`
- `paused`
- `succeeded`
- `failed`
- `terminated`
- `cancelled`

Node run 状态：

- `queued`
- `running`
- `waiting_approval`
- `skipped`
- `succeeded`
- `failed`
- `cancelled`

边界规则：

- 运行中的 workflow version 不可修改。
- 修改 workflow 产生新 version，不影响已启动 run。
- 暂停 workflow 定义时，同时挂起其未完成 run。
- 终止 workflow 定义或 run 表示放弃，不能恢复，只能重新触发新 run。
- 判断节点只接受 `yes` / `no` 出边，复杂条件写在节点内容中，由 Agent 执行时输出布尔结果。

## 4. Server-Runner PostgreSQL 协议

### 4.1 表分组

与 SRE 统一的核心协议表：

- `execution_jobs`
- `execution_attempts`
- `execution_events`
- `runner_instances`
- `run_outputs`
- `workflow_schedules`
- `approval_requests`

`execution_jobs` MVP 字段：

- `id`
- `kind`
- `status`
- `priority`
- `available_at`
- `payload jsonb`
- `dedupe_key`
- `attempt_count`
- `max_attempts`
- `created_by`
- `created_at`
- `updated_at`

`execution_attempts` MVP 字段：

- `id`
- `job_id`
- `runner_instance_id`
- `status`
- `started_at`
- `heartbeat_at`
- `lease_expires_at`
- `finished_at`
- `failure_code`
- `failure_message`

`runner_instances` MVP 字段：

- `id`
- `runner_name`
- `status`
- `started_at`
- `heartbeat_at`
- `lease_expires_at`
- `capabilities jsonb`

Job kind：

- `scan_todo_tasks`
- `interpret_comment`
- `prepare_task_plan`
- `execute_task`
- `execute_workflow_run`
- `execute_workflow_node`
- `refresh_project_document`
- `run_codex_turn`

Job status：

- `queued`
- `running`
- `waiting_approval`
- `succeeded`
- `failed`
- `cancelled`
- `dead`

Attempt status：

- `leased`
- `running`
- `waiting_approval`
- `succeeded`
- `failed`
- `cancelled`
- `lease_lost`

### 4.2 创建任务

只有 `server` 创建 `execution_jobs`：

- 用户操作创建：手动触发 workflow、确认方案、要求返工、保存评论后需要解释。
- Scheduler 创建：到期 schedule 触发 workflow run。
- 状态机创建：任务进入可执行状态后创建执行 job。

Runner 不创建 schedule，不扫描 due schedule。

### 4.3 Runner 注册与领取任务

Runner 启动后先注册或刷新 `runner_instances`。

Runner 使用 PostgreSQL 行锁领取：

```sql
WITH next_job AS (
  SELECT id
  FROM execution_jobs
  WHERE status = 'queued'
    AND available_at <= now()
  ORDER BY priority DESC, available_at ASC, created_at ASC
  FOR UPDATE SKIP LOCKED
  LIMIT 1
)
UPDATE execution_jobs
SET status = 'running',
    attempt_count = attempt_count + 1,
    updated_at = now()
WHERE id IN (SELECT id FROM next_job)
RETURNING *;
```

同一事务内创建 `execution_attempts`：

```text
execution_attempts.status = leased
execution_attempts.runner_instance_id = current runner
execution_attempts.lease_expires_at = now + lease ttl
```

执行边界：

- 领取成功后 runner 将 attempt 进入 `running` 并定期续租。
- runner 崩溃后，attempt lease 过期，server 或 runner reclaim 流程把 job 放回 `queued` 或标记 `dead`。
- 接管前必须检查幂等键、attempt 状态和目标 run 当前状态，避免重复执行外部副作用。

### 4.4 事件协议

所有运行过程写 `execution_events`：

- `job.queued`
- `attempt.leased`
- `attempt.started`
- `codex.session_started`
- `codex.event`
- `node.started`
- `approval.requested`
- `approval.resolved`
- `output.created`
- `artifact.created`
- `task.transitioned`
- `attempt.succeeded`
- `attempt.failed`
- `attempt.cancelled`
- `attempt.lease_lost`

规则：

- `execution_events` append-only。
- 每个 run 或 attempt 内有递增 `sequence`。
- UI 使用 cursor 拉取，也可由 `server` 通过 PostgreSQL `LISTEN/NOTIFY` 推送。
- 事件中不直接存大文本，大文本落 `run_outputs` 或 artifact，事件只存摘要和引用。

`run_outputs`：

- 保存结构化输出引用。
- 可关联 `workflow_run_id`、`node_run_id`、`task_id`、`execution_attempt_id`、`artifact_version_id`。
- 用于从运行记录提取任务输出、子任务输出和工作流节点输出。

### 4.5 控制指令

Server 不直接调用 runner HTTP。

控制方式：

- 暂停：server 更新 run/job/control 状态为 `pause_requested`。
- 恢复：server 更新状态并创建恢复 job。
- 终止：server 写 `terminate_requested`，runner 读取后终止 Codex 进程组。
- 审批：server 写 `approval_requests.status`，runner 监听或轮询后继续。

Runner 循环：

```text
register runner_instance
-> claim execution_job
-> create execution_attempt
-> mark attempt running
-> execute phase
-> append execution_event
-> renew attempt lease and runner_instance lease
-> check control command
-> if approval needed, mark job/attempt waiting_approval and suspend execution
-> if finished, mark attempt and job succeeded/failed/cancelled
```

### 4.6 Scheduler

MVP 不拆独立 scheduler 服务，但 scheduler loop 放在 `server`。

实现方式：

- `server` 扫描 `workflow_schedules where next_fire_at <= now`。
- 使用行锁抢占 schedule。
- 创建 `workflow_run` 和 `execution_jobs`。
- 更新 `next_fire_at`。

错过定时规则：

- 不自动补跑。
- server 停机期间错过的触发不补偿创建历史 run。
- server 重启后，扫描 schedule 时如果 `next_fire_at` 已经过期，只推进到下一次未来时间。
- 用户需要补跑时，通过手动触发 workflow 创建新的 run。

原因：

- 调度是控制面决策，应由 `server` 负责。
- `runner` 只负责执行已创建的 job，避免 runner 因执行扩容而重复承担调度语义。
- 没有 Redis 时，PostgreSQL 行锁足够保证单 server 或多 server 后续扩展下的 schedule 抢占。

后续拆分条件：

- 多 server 后调度负载明显增加。
- schedule 计算复杂度明显上升。
- 调度 SLA 需要独立监控和扩缩容。

## 5. 托管工作区与 Codex Adapter

Runner 工作区类型：

- `managed_workspace`：默认模式，位于 Docker volume，例如 `/var/lib/xiexu/workspaces/<project_id>/<run_id>`。
- `bind_mount_workspace`：可选模式，映射宿主机项目根目录到容器内，例如 `/workspace/binds/root-1/project-a`。

执行策略：

- 任务执行默认在托管工作区。
- 需要访问已有本地项目时，用户必须在 Docker Compose 中声明 bind mount root。
- 数据库保存 host path hint、container path、root id 和项目相对路径。
- Runner 只使用 container path 执行。

Codex adapter 边界：

- Domain 不绑定 `codex exec --json`。
- Application 只依赖 `CodexSessionDriver` trait，例如 `start_turn`、`resume_turn`、`cancel_turn`、`stream_events`。
- Infrastructure 优先复用 Relay app-server `--stdio` 模式，保持与 Relay 既有 Codex 控制能力兼容。
- 允许提供批处理适配器，例如基于 `codex exec --json` 的 batch adapter，用于简单执行或降级场景。
- Adapter 输出统一归一化为 `execution_events` 和 `run_outputs`，不把 CLI 原始事件泄漏到领域层。

安全边界：

- 不挂 Docker socket 作为 MVP 默认能力。
- Server 不挂载项目目录。
- Runner 以非 root 用户运行。
- Runner 限制并发、超时、输出大小、artifact 大小。

## 6. Relay 复用、裁剪、重写清单

### 6.1 Relay 直接复用

- Rust workspace 分层思路：`domain`、`application`、`infrastructure`、`mcp`。
- `axum` server + PostgreSQL 基础。
- PostgreSQL migration 机制。
- PostgreSQL realtime listener 思路。
- Agent identity、Agent profile、profession/role catalog。
- Agent memory 的 scope/tier/status 校验思路。
- Codex profile/control/request/approval 模型。
- Relay app-server `--stdio` 作为 Codex adapter 首选方向。
- Agent Trigger run、activity、lease、cancellation 思路。
- MCP tool 暴露给 Codex 的模式。
- Git worktree 能力作为可选模式参考。

### 6.2 Relay 裁剪

- 移除 company 作为产品概念；MVP 不引入公司/组织模型。
- 暂缓完整权限系统；保留 creator/audit 字段，为后续权限恢复留口子。
- 暂缓外部注册、ownership proof、社交验证。
- 暂缓 Harness/self-hosted Git 平台集成。
- 暂缓复杂 governance policy。
- 暂缓完整 project type catalog，仅保留默认项目类型和 Agent 身份模板。
- Git/worktree 不作为默认执行路径。

### 6.3 需要重写或新建

- 任务看板模型：用户阶段只保留 `backlog/todo/plan_review/in_progress/acceptance/done/cancelled`。
- `plan_status` 和 `execution_status` 子状态模型。
- 父子任务聚合进度与展开/收缩。
- 评论语义驱动状态流转。
- 工作流定义、版本、节点、连接线、人工确认节点。
- WorkflowRun 产生任务的 `source_workflow_run_id` / `source_node_run_id` 关联。
- `execution_jobs`、`execution_attempts`、`execution_events`、`runner_instances`、`run_outputs` 协议表。
- server scheduler 和错过定时跳过策略。
- 容器内 Codex Runner + 托管工作区。
- Artifact 文件存储与版本。
- 项目文档刷新机制。
- 面向无 Git 协作的 path lease 和冲突检测。

## 7. Dashi Taskboard 借鉴清单

直接借鉴：

- 任务看板信息密度和三列/多列状态布局。
- 任务卡片、标签、优先级、评论活动的表现方式。
- 乐观版本控制，更新任务和评论时带 `version`。
- 评论先记录，再由行为/自动化驱动状态变化。
- Codex 事件归一化和运行事件展示思路。
- Workflow control flow 的 `version`、节点、条件分支、画布快照思想。
- 自动化任务对 Codex 的明确操作约束文案。

不借鉴为后端基线：

- SQLite 作为数据库。
- Node 本地 HTTP API 作为核心后端。
- Tauri/macOS launcher。
- CDP 注入 Codex App。
- 本地单机 `.data` 模式。
- 把领域模型直接绑定到 `codex exec --json`。

## 8. MVP 风险清单

- 容器内 runner 无法访问未挂载的宿主机路径，必须通过托管工作区或显式 bind mount 解决。
- 无 Redis 时，PostgreSQL execution queue 必须严格处理 lease 过期、幂等、重试和 dead job。
- Scheduler 在 server 内，必须保证 server 多实例时 schedule 抢占幂等。
- 错过定时不补跑可能导致用户误解，需要在 UI 和运行记录中明确显示 skipped/missed。
- 评论语义解析不能直接改状态，必须经过状态机校验。
- 人工确认节点会让 run 长时间等待，必须支持超时、提醒和终止。
- Artifact 增长需要容量统计和保留策略。
- 多 Agent 同时改同一项目文件时，需要 path lease 和冲突检测，不能只依赖协调 Agent 的自然语言约定。
- Relay 复用必须裁剪 company/权限/Harness，否则 MVP 会过重。
