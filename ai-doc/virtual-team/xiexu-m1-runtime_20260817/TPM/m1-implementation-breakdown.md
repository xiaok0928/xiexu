# 协序 M1 技术项目拆解

> 边界修订：经 SA 复核，M1 最终只交付项目/任务事实源、父子任务、依赖、评论、事件时间线和项目文档基础存储；`execution_jobs`、`execution_attempts`、Runner 业务作业领取、`noop/record_transition` 和评论驱动状态动作移入 M2/M3。本文件后续“执行控制面”描述仅作为原始拆解记录，不代表 M1 已交付能力。

日期：2026-08-17  
任务根目录：`xiexu-m1-runtime_20260817`  
目标交付目录：`/Volumes/Tools/workspace/task-relay/xiexu`

## 1. 里程碑定义

M1 目标是把 M0 的运行骨架推进为“任务域与执行控制面基础版”。完成后，用户能够创建项目和任务，在任务面板中查看父子任务、拖动或执行状态转换、发表评论，并由系统为后续执行创建可追踪的作业记录；Runner 能够以租约方式领取并完成通用作业，但暂不执行真实 Codex。

M1 的验收主链路：

```text
创建项目
-> 创建 Backlog 想法
-> 用户将想法移入 Todo
-> 系统创建任务计划作业
-> Runner 领取作业并记录事件
-> 任务进入 plan_review 或 in_progress
-> 任务评论被持久化并可追踪
-> 任务进入 acceptance
-> 用户通过评论表达验收结果
-> 任务进入 done 或回退 Todo/In Progress
```

本阶段不把评论自然语言解释为 AI 能力；M1 先保存评论、提供显式可验证的意图入口，并保留后续接入 AI 解释器的边界。

## 2. 发现方式与仓库边界

源码目录为 `/Volumes/Tools/workspace/task-relay/xiexu`。执行：

```text
git -C /Volumes/Tools/workspace/task-relay/xiexu rev-parse --show-toplevel
```

结果为 `not a git repository`。该目录没有可解析的 Git 仓库，也没有仓库级 `.codegraph` 索引，因此本拆解采用 `bounded fallback after repository-scoped CodeGraph readiness checks`：基于现有源码、M0 文档、架构基线和构建结果进行边界确认。M1 不初始化 Git，不运行 `codegraph init`，不假定用户公司的 Git 项目可以承载协序内部任务状态。

## 3. 【修改范围】

### 3.1 目标模块与责任角色

| 编号 | 模块/文件范围 | 责任角色 | 交付内容 |
|---|---|---|---|
| M1-SA | `crates/domain`、迁移模型、状态契约 | SA | 固化实体、状态机、幂等键和租约规则 |
| M1-BE | `apps/server`、`crates/application`、`crates/infrastructure`、`apps/migrate` | BE | 项目/任务/评论/作业 API、事务和持久化实现 |
| M1-RUNNER | `apps/runner` | BE + SRE | 通用作业领取、租约续期、成功/失败/取消回写 |
| M1-FE | `apps/web/src` | FE | 看板真实数据接入、父子任务折叠、评论与状态操作 |
| M1-QA | 相关测试目录与 Compose 验证脚本 | QA | API、状态机、租约、前端联调和回归验证 |
| M1-SRE | `compose.yaml`、Dockerfile、运行文档 | SRE | 迁移顺序、服务健康检查、数据卷和故障恢复验证 |

### 3.2 预计新增或修改的核心对象

- `projects`
- `tasks`
- `task_comments`
- `task_transitions`
- `execution_jobs`
- `execution_attempts`
- `execution_events`
- 已有的 `runner_instances`
- `schema_migrations` 的后续版本

### 3.3 明确排除

- 真实 Codex 调用、`CODEX_HOME` 管理和 MCP Server。
- 工作流画布、工作流版本、节点运行和定时触发。
- Agent 默认角色、项目固定 Agent、长期记忆和短期记忆。
- 评论自然语言的 AI 意图判断。
- 用户权限和数据权限。
- 外部项目目录动态挂载、Git 操作、worktree、分支合并。
- Redis、消息队列和批量流程暂停/终止。
- 任务自动认领策略的完整设置页。

## 4. 【接口设计】

M1 API 使用 JSON over HTTP，保持 Server 现有 Axum 路由风格。所有 ID 使用服务端生成的 UUID；时间字段使用 UTC ISO-8601。当前没有对外稳定客户端，因此允许增加新路由，不修改 M0 的 `/healthz` 与 `/readyz` 语义。

### 4.1 项目

```text
GET    /api/projects
POST   /api/projects
GET    /api/projects/{project_id}
PATCH  /api/projects/{project_id}
```

`POST /api/projects` 输入：

```json
{
  "name": "协序",
  "description": "项目说明"
}
```

输出包含 `id`、`name`、`description`、`created_at`、`updated_at`。删除项目不在 M1 暴露，避免级联删除造成不可逆数据损失。

### 4.2 任务与看板

```text
GET    /api/projects/{project_id}/tasks?board_stage=&parent_id=
POST   /api/projects/{project_id}/tasks
GET    /api/tasks/{task_id}
PATCH  /api/tasks/{task_id}
POST   /api/tasks/{task_id}/transitions
```

任务创建输入至少包括：

```json
{
  "title": "记录一个待验证想法",
  "description": "用户原始描述",
  "board_stage": "backlog",
  "parent_task_id": null,
  "requires_plan_confirmation": true
}
```

状态转换输入：

```json
{
  "target_stage": "todo",
  "reason": "Human 确认进入待处理"
}
```

服务端必须校验状态机，不接受前端直接修改任意状态。父任务进入验收通过时，后端以事务方式将其未取消子任务同步为验收通过；单个子任务通过时，父任务计算为 `partially_accepted`；子任务返工时，父任务保留验收态但记录聚合状态为 `rework_required`，具体回退目标由显式转换请求指定。

### 4.3 评论

```text
GET  /api/tasks/{task_id}/comments
POST /api/tasks/{task_id}/comments
```

输入：

```json
{
  "content": "方案可以，继续执行",
  "intent": "approve_plan"
}
```

`intent` 在 M1 允许显式值：`note`、`approve_plan`、`reject_plan`、`accept`、`rework`、`mention`。缺省时按 `note` 保存。M1 不声称可以可靠解释任意自然语言；后续 AI 解释器可以将自然语言转换为同一组意图，再复用状态机校验。

评论创建只负责记录事实和生成可追踪的 `task_transitions`/`execution_jobs` 请求，不允许绕过当前任务阶段直接完成任务。

### 4.4 执行作业与 Runner

```text
GET  /api/execution-jobs?status=
POST /api/execution-jobs/{job_id}/cancel
```

Runner 内部使用 PostgreSQL 事务领取：

```text
claim_job(worker_id) -> execution_attempt
heartbeat_attempt(attempt_id, lease_until)
append_event(attempt_id, event)
finish_attempt(attempt_id, result)
```

领取规则：

- 只领取 `queued` 且未过期的作业。
- 使用 `SELECT ... FOR UPDATE SKIP LOCKED`，避免多个 Runner 重复领取。
- 领取时创建唯一 `execution_attempts`，写入 `claimed_by` 与 `lease_until`。
- 租约过期后作业可重试，但同一幂等键不能生成并行有效尝试。
- M1 的 Runner 只执行受控的 `noop`/`record_transition` 作业，不调用外部模型或宿主机命令。

## 5. 【边界评估】

### 5.1 正常路径

- 新建项目后可创建 Backlog 任务。
- Backlog -> Todo 只由显式用户操作触发，不自动执行 Agent。
- Todo 任务按 `requires_plan_confirmation` 分流：需要确认则生成计划作业并进入 `plan_review`；不需要确认则直接排队执行作业。
- 评论写入后保留作者、时间、正文和意图，状态转换产生审计记录。
- 父子任务在 API 中同时返回，前端负责折叠/展开；聚合字段由后端计算，避免不同客户端计算不一致。

### 5.2 异常与边界

- 不存在的项目、任务、父任务返回 `404`。
- 父任务指向自身、形成环、跨项目挂载返回 `422`。
- 非法状态转换返回 `409`，不写入部分状态。
- 重复提交相同 `Idempotency-Key` 返回原结果，不重复创建作业。
- 评论意图与当前阶段不匹配时，评论仍可保存，但不触发状态变化，并返回 `transition_applied=false`。
- Runner 进程崩溃时，租约到期后由其他 Runner 重试；已追加事件不可删除。
- 数据库迁移失败时，Compose 不启动 Server/Runner；M0 的 `/readyz` 保持未就绪。
- 前端刷新或深链访问继续由 Server 回退到 `index.html`。

### 5.3 一致性与并发风险

- 任务状态、评论意图和作业创建必须在同一数据库事务中提交，避免“状态已变但没有作业”。
- 父任务聚合状态更新与子任务转换必须锁定同一父任务，防止并发验收产生错误聚合结果。
- 作业领取采用行锁和租约；事件追加使用单调序号或数据库生成的顺序字段。
- 任务列表分页和父子折叠暂不支持跨页聚合；M1 以单项目中小规模数据为边界。

### 5.4 回滚与兼容

- 新表全部通过版本化 migration 创建，不修改既有 M0 表的语义。
- API 新增路由可独立回滚；旧的 `/healthz`、`/readyz` 必须保持兼容。
- 若 M1 迁移失败，回滚只允许在未产生业务数据时执行；已有数据不做自动 destructive rollback。
- M1 Runner 仅处理白名单作业类型，未知类型进入 `failed` 并记录原因，不执行任意命令。

## 6. 并行边界与依赖

### 可并行

1. `M1-SA` 完成初稿后，`M1-FE` 可基于冻结的 OpenAPI/JSON 示例开发静态数据适配和看板交互。
2. `M1-BE` 开发项目/任务查询接口时，`M1-QA` 可先编写状态机和迁移验收用例。
3. `M1-SRE` 可独立补充 Compose 迁移顺序、卷持久化和健康检查验证。

### 必须串行

1. `M1-SA` 冻结数据模型和状态机 -> `M1-BE` 实现持久化与接口。
2. `M1-BE` 冻结 API 响应结构 -> `M1-FE` 开始真实联调。
3. `M1-BE` 完成作业领取/租约 -> `M1-RUNNER` 接入数据库作业循环。
4. `M1-BE`、`M1-RUNNER` 完成 -> `M1-QA` 执行端到端验收。

## 7. 完成标准

- PostgreSQL 从空数据卷启动，M1 migration 可重复执行且不会重复建表或丢失数据。
- 项目、任务、父子关系和评论可通过 API 创建、查询和更新。
- 非法状态转换、父子环、跨项目父任务和重复幂等请求有明确错误响应。
- 看板可以显示 Backlog/Todo/Plan Review/In Progress/Acceptance/Done，支持父任务折叠和评论入口。
- Todo 任务的计划确认开关可影响作业类型和后续阶段。
- Runner 至少支持两个实例并发领取时不重复执行同一个作业，租约过期后可重试。
- 任务状态变化、评论和执行事件可通过任务详情查询完整追溯。
- `docker compose -f compose.yaml build`、迁移、启动、健康检查、API 集成测试和前端类型/构建检查通过。
- 不引入 Redis、Git、CodeGraph 初始化或真实 Codex 依赖。

## 8. 风险与决策记录

| 风险 | 影响 | 处理 |
|---|---|---|
| M1 过早接入自然语言理解 | 状态流转不可预测，难以验收 | M1 先使用显式 `intent`，保留 AI 适配边界 |
| 父子任务聚合规则与验收规则冲突 | 父任务状态不一致 | 后端事务统一计算，前端不自行推导最终状态 |
| Runner 重试导致重复副作用 | 任务重复执行 | M1 仅允许幂等 noop 作业，并记录 attempt/event |
| 无 Git 仓库导致变更无法通过提交追踪 | 交付审计不足 | 使用 delivery 文档、migration 版本和运行事件追踪；不伪造 Git 流程 |
| 直接暴露任意作业类型 | 容器执行安全风险 | Runner 白名单作业类型，未知类型失败 |

## 9. 交付顺序

1. SA 冻结 schema、状态机、错误码和 JSON 示例。
2. BE 完成 migration、项目/任务/评论 API 和事务状态转换。
3. Runner 完成作业领取、租约、事件和受控作业执行。
4. FE 接入看板数据、父子折叠、评论与状态操作。
5. QA/SRE 完成 Compose、并发租约、端到端和回归验证。
6. TPM 根据实际 diff、构建结果和测试结果进行 `LGTM` 或 `REJECT` 评审。
