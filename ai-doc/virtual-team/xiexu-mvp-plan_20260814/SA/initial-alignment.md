# 协序 MVP 架构实施初始对齐

日期：2026-08-14
角色：SA
用途：给 TPM/BE/FE/QA/SRE 的实施计划输入，不重写架构。

## 0. 输入基线

已读取并按以下文档对齐：

- `/Volumes/Tools/workspace/task-relay/xiexu/ai-doc/virtual-team/product-baseline_20260814/documents/product-decision-baseline.md`
- `/Volumes/Tools/workspace/task-relay/xiexu/ai-doc/virtual-team/xiexu-architecture_20260814/documents/architecture-baseline.md`
- `/Volumes/Tools/workspace/task-relay/xiexu/ai-doc/virtual-team/xiexu-architecture_20260814/documents/system-architecture.md`
- `/Volumes/Tools/workspace/task-relay/xiexu/ai-doc/virtual-team/xiexu-architecture_20260814/SRE/deployment-runtime.md`

当前确认约束：

- Rust 后端。
- PostgreSQL，MVP 无 Redis。
- 长期服务最小集合为 `postgres`、`server`、`runner`，`migrate` 为一次性进程。
- Scheduler 在 `server`，不在 `runner`。
- Runner 在容器内运行 Codex adapter，默认托管工作区，可选 bind mount。
- Server 与 Runner 通过 PostgreSQL `execution_jobs`、`execution_attempts`、`execution_events`、`runner_instances`、`run_outputs` 协作。
- 看板阶段与执行状态分离。

## 1. 架构实施里程碑

### M0：仓库骨架与基础运行面

目标：让 Rust workspace、Docker Compose、PostgreSQL、migration、server/runner 基础进程跑起来。

范围：

- 建立 `apps/server`、`apps/runner`、`apps/migrate`、`apps/mcp-server`、`crates/domain`、`crates/application`、`crates/infrastructure`、`crates/mcp`、`crates/api`、`crates/shared`。
- 建立 PostgreSQL migration 机制。
- 建立 `/healthz`、`/readyz`、runner heartbeat。
- 建立基础配置：`DATABASE_URL`、`XIEXU_RUNNER_ALLOWED_ROOTS`、`CODEX_HOME`、runner concurrency。

完成标准：

- `docker compose up` 能启动 `postgres/server/runner`。
- `migrate` 可重复执行且幂等。
- server 可读写数据库，runner 可注册 `runner_instances`。

### M1：领域模型与执行协议

目标：先锁定系统骨架和跨进程协议，避免后续看板/工作流返工。

范围：

- 实现核心表：project、task、comment、agent、agent_memory、workflow definition/version/run/node run。
- 实现 `execution_jobs`、`execution_attempts`、`execution_events`、`runner_instances`、`run_outputs`。
- 实现 job claim、attempt lease、heartbeat、event append、失败重试、dead job。
- 实现 server scheduler 的最小扫描能力，错过定时不自动补跑。

完成标准：

- server 能创建 execution job。
- runner 能领取 job、创建 attempt、续租、写 event、结束 job。
- runner 崩溃或 lease 过期后，job 可按规则恢复或进入 dead。

### M2：任务看板闭环

目标：先让用户任务从想法到验收闭环成立。

范围：

- 实现看板阶段：`backlog/todo/plan_review/in_progress/acceptance/done/cancelled`。
- 实现 `plan_status` 与 `execution_status` 子状态，不新增看板列。
- 实现父子任务、聚合进度、展开/收缩数据结构。
- 实现评论 append-only 与 `comment_interpretation` job。
- 实现方案确认、验收通过、验收失败、返工流转。

完成标准：

- backlog 拖入 todo 后可触发 Agent 处理。
- 默认需要方案确认；取消方案确认后可直接执行。
- 验收父任务可完成全部子任务，验收子任务可形成父任务部分验收展示。

### M3：Workflow MVP

目标：让工作流能保存、调度、执行，并在任务看板中体现任务。

范围：

- 实现 workflow draft/active/paused/archived。
- 实现 WorkflowVersion，不允许运行中修改。
- 支持开始、结束、执行、判断、人工确认节点。
- 判断节点连接线只接受 `yes/no`。
- 实现 WorkflowRun 产生 Task，Task 通过 `source_workflow_run_id/source_node_run_id` 关联来源。
- 实现定时规则：一次执行、预定时间、周期重复、AI 解析后的结构化规则。

完成标准：

- 用户保存 workflow 后可手动执行或按 schedule 执行。
- server scheduler 创建 workflow run 和 execution job。
- 人工确认节点能通过评论/审批继续。
- 错过定时不自动补跑，重启后推进下一次未来时间。

### M4：容器内 Codex Runner 与工作区

目标：让 runner 能在托管工作区或允许根目录中执行 Codex。

范围：

- 实现 managed workspace。
- 实现 bind mount root 映射和路径校验。
- 实现 Agent Codex profile 隔离。
- 优先实现 Relay app-server `--stdio` adapter。
- 允许保留 `codex exec --json` batch adapter 作为简单执行或降级适配。
- 将 Codex 输出统一归一化为 `execution_events` 和 `run_outputs`。

完成标准：

- runner 能在 managed workspace 中启动一次 Codex 执行并回写事件。
- 未在 `XIEXU_RUNNER_ALLOWED_ROOTS` 内的路径被拒绝。
- run output 可在运行记录和任务详情中查看。

### M5：项目文档、记忆、Artifact

目标：补齐协序可持续协作能力。

范围：

- Artifact 文件存储 + PostgreSQL metadata + immutable version。
- Agent private memory：短期/长期、来源引用、更新/归档。
- 项目文档刷新：父任务完成刷新 + 定时兜底刷新。
- 运行记录提取任务输出、子任务输出、workflow 节点输出。

完成标准：

- Agent 可读取/写入自己的任务经验类私有记忆。
- 项目文档能基于任务完成和兜底调度形成新版本。
- Artifact 可追溯到 task/run/node/agent。

## 2. 硬依赖顺序

必须串行的依赖：

1. Rust workspace 与 migration 基础先于所有业务实现。
2. PostgreSQL schema 与 `execution_*` 协议先于 runner、scheduler、workflow。
3. `runner_instances` 与 attempt lease 先于任何 Codex 执行。
4. 看板主阶段与子状态模型先于任务 UI、评论流转、验收返工。
5. WorkflowVersion 与 WorkflowRun 先于 workflow scheduler 和节点执行。
6. Workspace path model 先于 Codex adapter 执行。
7. `run_outputs` 与 artifact metadata 先于运行记录详情和任务输出提取。
8. Agent profile 与 Codex profile 隔离先于多 Agent 并发执行。

不能倒置的点：

- 不能先做 UI 看板再补状态机，否则会把 `blocked/failed/running` 误做成看板列。
- 不能先接 Codex 再定 execution 协议，否则日志、审批、取消和恢复会返工。
- 不能先做 workflow 画布执行再做 WorkflowVersion，否则运行中修改会污染历史 run。
- 不能先做 bind mount 项目访问再做 allowed roots，否则会形成安全缺口。

## 3. 可并行边界

可并行一：

- BE-A：Rust workspace、migration、PostgreSQL repository。
- SRE：Docker Compose、volume、env、healthcheck、backup/restore。
- FE：看板静态交互原型和 workflow canvas 数据结构 mock。

依赖汇合点：BE-A 输出 schema 和 API DTO 后，FE/SRE 才能接真实接口和运行环境。

可并行二：

- BE-B：execution queue、attempt lease、event append。
- BE-C：Task/Comment/Approval 状态机。
- FE：任务看板、任务详情、评论区。
- QA：状态机用例矩阵。

依赖汇合点：execution 协议稳定后，runner 和 workflow 执行才能进入联调。

可并行三：

- Runner：workspace manager、Codex adapter、runner heartbeat。
- Server：scheduler、workflow CRUD、run 控制。
- FE：workflow 画布保存、运行记录页面。

依赖汇合点：WorkflowRun 和 `execution_jobs` 对齐后，才能做端到端 workflow 执行。

不可并行或需强同步：

- `execution_*` 表结构、状态枚举、事件类型必须由 SA/BE/SRE 先锁定。
- 看板阶段和子状态必须由 PM/SA/FE/BE 一次对齐。
- Codex adapter 事件归一化必须与运行记录 UI、QA 断言同步。

## 4. Relay / Dashi 复用优先级

### P0：Relay 优先复用

- Rust workspace 分层。
- PostgreSQL migration 和 repository 组织方式。
- `axum` server 基础。
- Agent identity、Agent profile、profession/role catalog。
- Agent memory 的 scope/tier/status 设计。
- Codex profile/control/request/approval 模型。
- Relay app-server `--stdio` 作为 Codex adapter 首选方向。
- MCP tool 暴露给 Codex 的模式。

### P1：Relay 借鉴但需裁剪

- Agent Trigger run/activity/lease/cancellation。
- PostgreSQL realtime listener。
- Git worktree 能力，仅作为可选模式。
- 项目成员、Agent 分配、任务依赖能力。

裁剪要求：

- 不引入 company 作为产品概念。
- 不引入完整权限系统。
- 不引入外部注册、ownership proof、社交验证。
- 不引入 Harness/self-hosted Git 平台集成。

### P0：Dashi 优先借鉴

- 任务看板信息密度和卡片表现。
- 评论活动流。
- 乐观版本控制。
- Workflow canvas snapshot、节点、条件分支的 UI/数据思想。
- 自动化任务对 Codex 的明确操作约束文案。

### P1：Dashi 只做参考

- Codex 事件归一化和运行展示。
- CLI/Skill 使用方式。

不采用：

- SQLite 后端。
- Node 本地 HTTP API 作为核心后端。
- Tauri/macOS launcher。
- CDP 注入 Codex App。
- 领域模型直接绑定 `codex exec --json`。

## 5. 前 3 个架构风险

### 风险 1：execution 协议设计不稳导致 runner、workflow、UI 全链路返工

影响：

- 运行记录、暂停终止、审批等待、失败重试、输出提取都会受影响。
- FE、SRE、QA 均依赖该协议。

缓解：

- M1 优先锁定 `execution_jobs/execution_attempts/execution_events/runner_instances/run_outputs`。
- 所有执行类功能必须只通过该协议对接。
- QA 先写 lease 过期、重复领取、取消、失败、恢复用例矩阵。

### 风险 2：看板阶段和执行状态混用，破坏用户视角

影响：

- `queued/running/blocked/failed` 如果变成看板列，会偏离已确认产品设计。
- 返工、方案确认、验收聚合会变复杂。

缓解：

- 数据模型强制区分 `board_stage`、`plan_status`、`execution_status`。
- API DTO 和 FE 组件都按主阶段 + 子状态展示。
- 状态机用例先覆盖父子任务、方案确认、验收失败返工。

### 风险 3：容器内 runner 与本地项目路径/工具链不匹配

影响：

- 未挂载路径不可访问。
- 宿主机工具链和容器工具链差异会导致 Codex 执行失败。
- bind mount 过宽会带来安全风险。

缓解：

- MVP 默认 managed workspace。
- 外部项目必须通过 `XIEXU_RUNNER_ALLOWED_ROOTS` 和显式 bind mount 暴露。
- Runner 执行前做 canonical path 校验。
- SRE 在部署文档中明确 Docker Desktop 共享目录、volume、备份和 `CODEX_HOME` 敏感性。

## 6. SA 给实施团队的首轮建议

首轮不要直接从 UI 或 Codex 执行开始。建议按以下顺序拉通：

```text
M0 workspace/runtime
-> M1 execution protocol
-> M2 task board state machine
-> M3 workflow run
-> M4 Codex runner
-> M5 memory/artifact/project docs
```

最早可交付的端到端切片：

```text
创建项目
-> 创建 todo 任务
-> server 创建 prepare_task_plan execution_job
-> runner 领取 job 并写 execution_events
-> 任务进入 plan_review
-> Human 评论确认
-> server 创建 execute_task execution_job
-> runner 写 run_outputs
-> 任务进入 acceptance
-> Human 验收 done
```

这个切片能同时验证：PostgreSQL 协议、server/runner 边界、看板状态机、评论语义入口、运行事件、输出提取和验收闭环。
