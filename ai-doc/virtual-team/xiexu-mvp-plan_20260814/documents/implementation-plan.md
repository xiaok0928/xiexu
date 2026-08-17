# 协序 MVP 实施计划

## 1. 计划边界

本计划用于把已确认的协序产品、UI 和架构方案转成工程实施顺序。当前只做计划，不实施源码。

进入源码开发前仍必须遵循全局 `AGENTS.md` 的代码门禁：先输出并确认 `【修改范围】`、`【接口设计】`、`【边界评估】`，再输出并确认 `【逻辑流程复述】`。任何实际源码、测试、构建脚本、migration 或配置修改都必须等这些门禁完成。

本计划不做日期或工期承诺，只按里程碑和并行波次组织。

## 2. MVP 总目标

协序 MVP 要证明以下闭环成立：

用户在本地 Docker 中启动 Web 应用后，可以创建项目、记录 Backlog 想法、把想法拖入 Todo，由系统周期扫描并交给协调 Agent 生成方案；Human 确认方案后，Runner 在容器内调用 Codex 执行任务，任务进入验收；Human 通过评论表达验收通过或返工，系统根据语义推动状态流转；项目、任务、Agent、评论、执行事件、输出、工作流运行记录都能被持久化和回看。

## 3. 核心状态和硬依赖

任务看板主阶段固定为：

```text
backlog -> todo -> plan_review -> in_progress -> acceptance -> done
```

补充终态和异常态：

```text
cancelled
```

执行态独立于看板列：

```text
queued / running / blocked / failed / succeeded / cancelled
```

硬依赖顺序：

1. 数据模型和状态机先于 UI 看板。
2. 执行 job、attempt、event、runner heartbeat 先于真实 Codex 执行。
3. 任务闭环先于工作流闭环。
4. 工作流版本化先于运行实例。
5. API 契约先于 FE 联调。
6. UI 设计包经 PM 负责人批准，且 UI/FE 对齐后，FE 才能进入页面实现。
7. Docker 启动门禁和健康检查先于端到端验收。

## 4. 里程碑与交付物

### G0：UI 设计包门禁

目标：在真正实现页面前，把已确认产品基线转成可实施 UI 设计包，避免 FE 在交互状态未收口前固化页面。

Ownership：

- UI：输出任务看板、项目空间、Agent 管理、新对话、项目群聊、工作流画布、运行记录的页面结构、关键状态、空态、错误态和交互说明。
- PM：按已确认产品基线校验 UI 是否覆盖 MVP 功能，不重开产品决策。
- FE：与 UI 对齐组件拆分、页面路由、前端状态来源、事件更新方式和不可由前端自造的业务状态。
- TPM：检查 UI 设计包、PM 批准记录和 UI/FE 对齐是否齐备。

交付物：

- UI 页面清单。
- 关键页面线框或原型。
- 状态与交互说明。
- FE 组件拆分建议。
- PM 校验结论。
- PM 负责人批准记录。

退出标准：

- UI 设计包经 PM 负责人批准。
- UI/FE 已确认页面实现边界。
- BE/FE 集成仍需等待 API 契约确认；G0 不替代接口门禁。

### M0：仓库与运行骨架

目标：创建可运行但业务能力最小的 xiexu 工程基座。

Ownership：

- SA：确认 monorepo 模块边界和核心 crate 分层。
- BE：创建 Rust workspace、`apps/server`、`apps/runner`、`apps/migrate`、domain/application/infrastructure 基础结构。
- FE：创建 `apps/web` React + TypeScript + Vite 基座。
- SRE：创建 Docker Compose、Dockerfile、环境变量、health/readiness、volume/mount 基线。
- QA：定义 smoke test 和最小启动检查。

交付物：

- `postgres/server/runner/migrate` 可通过 Docker Compose 启动。
- `migrate` 可创建基础 schema。
- `server` 提供 `/healthz`、`/readyz`。
- `runner` 可注册 heartbeat，但不执行真实任务。
- Web Shell 可打开。

退出标准：

- `docker compose config` 成功。
- `migrate` 成功退出。
- `server /readyz` ready。
- `runner_instances` 出现活跃 heartbeat。
- FE 构建通过。

### M1：任务与项目基础模型

目标：建立项目、任务、父子任务、评论、附件/输出元数据的事实源。

Ownership：

- SA：确认 Project、Task、TaskRelation、TaskComment、RunOutput 的模型和状态迁移规则。
- BE：实现 migration、repository、service、API。
- FE：实现项目列表、项目空间入口、任务看板基础列。
- UI：确认 taskboard 风格布局、父子任务折叠、卡片字段、右下角“需要确认方案”控件。
- QA：覆盖状态迁移和父子任务聚合。

交付物：

- 项目 CRUD。
- Backlog 想法创建。
- Backlog 拖入 Todo。
- Todo 卡片默认 `requires_plan_review=true`，卡片级可取消。
- 父子任务可创建、折叠、展开，父任务展示聚合进度，子任务展示实时进度。
- 评论可写入并展示。

退出标准：

- Backlog 不触发执行。
- Todo 才允许被扫描。
- 父任务验收可带动子任务全部验收。
- 子任务验收后父任务可显示部分验收。
- 看板列不出现 `running/failed/blocked` 这类执行态。

### M2：执行底座与最早端到端切片

目标：先打通一条最早端到端链路，不等待完整工作流画布。

最早端到端切片：

```text
创建项目 -> 创建 Backlog 想法 -> 拖入 Todo -> 周期扫描生成 prepare_task_plan job -> runner 领取 job -> 写 execution_events -> 任务进入 plan_review -> Human 评论确认方案 -> 创建 execute_task job -> runner 写 run_outputs -> 任务进入 acceptance -> Human 评论验收通过 -> done
```

Ownership：

- SA：确认 `execution_jobs`、`execution_attempts`、`execution_events`、`runner_instances`、lease、幂等键和状态映射。
- BE：实现 job 创建、领取、lease、事件追加、状态推进、Runner 执行适配层。
- FE：接入任务详情、执行事件流、输出展示、确认/返工评论。
- QA：验证 happy path、重复提交、runner 崩溃恢复、失败状态可见。
- SRE：验证容器内 workspace、`CODEX_HOME`、allowed roots、runner concurrency。

交付物：

- 周期扫描 Todo。
- 协调 Agent 生成方案 job。
- 方案确认评论触发执行 job。
- Runner 写入事件和输出。
- 验收通过评论触发 done。
- 验收失败评论触发返工，返工回到 `in_progress` 或按状态机回到可执行阶段，不默认回 Backlog。
- 技术错误默认重试 2 次，重试间隔为 1 分钟和 5 分钟。
- 任务执行 30 分钟无 heartbeat 时，由协调 Agent 检查并决定继续等待、重试、重指派或通知 Human。
- 同一节点或同一任务重指派 2 次仍失败时，进入 `blocked` 执行态并通知 Human。

退出标准：

- 同一任务重复触发不会产生不可解释的重复执行。
- runner 执行中崩溃后 lease 到期可恢复。
- execution failed 不改变业务主阶段为错误列，只在当前任务上显示失败标记和事件。
- run output 可从任务详情查看。

### M3：Agent、聊天与记忆

目标：把 Relay 风格 Agent 能力迁入协序，但职责文本按协序重新编写。

Ownership：

- PM：按已确认基线校验默认 Agent 身份清单和职责边界，不重开产品决策。
- SA：确认 Agent、AgentProfile、AgentMemory、Conversation、Message 的模型边界。
- BE：实现 Agent 身份、项目固定 Agent、任务动态参与 Agent、私有记忆读写、1:1 新对话、项目临时群聊。
- FE：实现 Agent 管理、项目 Agent 展示、聊天界面。
- UI：确认聊天与任务评论的视觉区分。
- QA：验证 Agent 固定身份、动态加入退出、记忆归属和消息追溯。

交付物：

- 默认 Agent 角色导入为协序自有职责文本。
- Human 可创建 Agent 身份，AI 辅助优化职责。
- 可为特定 Agent 补充职责。
- 项目至少有一个协调 Agent。
- 新对话为 Human 与 Agent 的 1:1 聊天。
- 项目群聊可产生任务卡片。
- 临时群聊可关联到一个或多个任务，群聊消息可作为任务上下文被执行 Agent 读取。
- 临时群聊归档时生成总结，并分别沉淀到任务上下文、项目记忆和项目文档候选更新。
- Agent 私有记忆记录执行经验，不等同于用户私有数据权限。

退出标准：

- 项目固定 Agent 与任务动态参与 Agent 不冲突。
- 任务分派由协调 Agent 负责，不要求用户直接指定执行 Agent。
- 权限管理不进入 MVP，所有数据对登录用户开放。

### M4：项目文档与跨任务协作

目标：让项目开发空间有可持续上下文，并支持跨子任务依赖协作。

Ownership：

- SA：确认 ProjectDocument、DocumentVersion、Mention、DependencySignal 模型。
- BE：实现项目文档生成、父任务完成刷新、定时兜底刷新、文档版本、diff、回退、Human 锁定章节、冲突保护、@mention 通知与评论联动。
- FE：实现项目文档查看、版本历史、diff、回退、章节锁定提示、任务评论 @、跨任务引用展示。
- QA：验证文档刷新时机、版本差异、回退、锁定冲突、@Agent/@任务协作链、评论感知和恢复执行。

交付物：

- 添加项目后生成项目文档。
- 父任务完成后刷新项目文档。
- 定时任务兜底检查遗漏变更。
- 项目文档支持版本历史、版本 diff 和回退。
- Human 可锁定项目文档章节；锁定章节不被 Agent 自动改写，只能生成候选变更。
- 多来源更新同一章节时进入冲突保护，不静默覆盖。
- 子任务 A 可在评论中 @ 子任务 B 或 B 的 Agent 请求接口/方法协助。
- B 完成后在 A 评论下回复，A 感知后继续执行。

退出标准：

- 文档刷新不阻塞任务主流程。
- @ 协作有明确可追溯评论。
- 依赖方未回复时，请求方任务进入可解释的等待/blocked 执行态，而不是改变看板主列。

### M5：工作流定义、运行与人工确认

目标：实现独立工作流模块，并让运行结果能在任务看板体现。

Ownership：

- PM：按已确认基线校验工作流 MVP 行为口径，不重开产品决策。
- UI：确认流程画布节点、连线 yes/no 标记、保存/运行/暂停/终止入口。
- SA：确认 WorkflowDefinition、WorkflowVersion、WorkflowRun、WorkflowNodeRun、ApprovalRequest、RunOutput 模型。
- BE：实现工作流保存、版本化、运行、节点执行、判断分支、人工确认、自动化暂停/终止、单次 run 暂停/恢复/终止。
- FE：实现工作流列表、画布、运行记录、节点输出、人工确认入口。
- QA：验证工作流 happy path、判断分支、人工节点挂起、暂停/恢复/终止。
- SRE：验证 scheduler 和 runner 对 workflow job 的一致处理。

交付物：

- 节点类型：开始/结束、执行、判断、人工确认、连接线。
- 判断节点通过 yes/no 连线流转。
- 用户写自然语言节点说明，Agent 可识别并执行。
- 工作流保存后可手动运行。
- 工作流可配置周期重复、预定时间、AI 解析重复规则。
- 不规则预定时间结构化存储。
- 自动化定义的暂停/终止只影响未来触发，已产生的 WorkflowRun 继续执行。
- 暂停单次 WorkflowRun 才挂起该实例，暂停可由 Human 手动恢复。
- 终止单次 WorkflowRun 才放弃该实例，终止不可恢复。
- WorkflowRun 在任务看板生成父任务，执行节点生成子任务，父任务展示聚合进度，子任务展示节点实时进度。
- 人工确认节点生成任务看板可见的确认项，并在项目群聊/运行记录中同步展示。
- 工作流运行记录可查看任务输出和子任务输出。
- 工作流产生的任务卡片显示工作流名称。

退出标准：

- 运行期工作流版本不可修改；非运行期定义可编辑并形成新版本。
- 工作流除人工确认节点外不进入普通验收阶段。
- 用户评论会被解析为确认、拒绝补充、验收通过、验收失败等意图，并驱动对应状态流转。
- 不支持“错过后补跑一次”，需要补跑由用户手动触发。

### M6：Docker 发布、备份恢复与发布候选

目标：形成可交付的本地 Docker MVP。

Ownership：

- SRE：发布配置、备份恢复、升级回滚、敏感目录说明。
- BE：readiness、migration、数据兼容和版本绑定。
- FE：生产构建与静态资源承载。
- QA：发布候选验收。
- TPM：汇总 release checklist 和剩余风险。

交付物：

- Docker Compose 默认托管 workspace volume。
- 可选 bind mount 外部项目目录。
- Runner 执行前 canonical path 校验。
- PostgreSQL、workspace、data、`CODEX_HOME` 备份恢复说明。
- migration 可重复执行。
- workflow run 绑定 version，升级不改变已启动运行定义。

退出标准：

- macOS Docker Desktop 必测通过。
- Linux Docker Engine 必测通过。
- Windows Docker Desktop + WSL2 必测通过。
- Windows/Linux/macOS 声明支持范围内的 Docker Desktop 或 Docker Engine 场景均需完成发布验收。
- 未配置外部项目 mount 时，不影响默认托管 workspace 运行。
- `CODEX_HOME` 敏感性在部署文档中明确。

### M7：可选 Git/worktree 后段能力

目标：在默认无 Git 的前提下，为单人新项目保留 Git/worktree 协作能力。

Ownership：

- SA：确认 GitProjectSettings、WorkspaceSnapshot、WorktreeSession 的边界，确保默认无 Git 不受影响。
- BE：实现 Git 能力探测、可选 worktree 创建、隔离工作区、状态只读展示和显式清理。
- FE：实现项目设置中的 Git/worktree 可选入口和风险提示。
- QA：验证默认无 Git、Git 仓库只读展示、worktree 创建与清理、禁用自动提交链路。
- SRE：验证 bind mount 下 Git/worktree 对 volume、权限和磁盘占用的影响。

交付物：

- 默认不要求 Git。
- 不自动 `git init`。
- 不自动创建分支。
- 不自动 commit、merge、push。
- 用户显式开启后，才允许为单人新项目创建 worktree 隔离区。
- 公司既有 Git 项目默认按无 Git 模式处理，只把文件系统作为执行空间。

退出标准：

- 未开启 Git/worktree 时，任务和工作流闭环完全可用。
- Git/worktree 能力只影响被显式开启的项目。
- 自动化和 Agent 不得绕过设置执行分支、提交、合并或推送。

## 5. BE/FE API 契约门禁

在任何 BE/FE 联调实现前，必须由 SA + BE + FE 共同确认 API 契约，至少包含：

- Project API：项目创建、列表、详情、项目文档入口。
- Task API：创建、更新阶段、父子关系、聚合进度、卡片设置、任务详情。
- Comment API：评论创建、语义意图、@mention、验收/返工触发。
- Execution API：job、attempt、event、runner heartbeat、run output。
- Agent API：Agent 身份、职责、项目 Agent、动态参与、记忆。
- Conversation API：1:1 对话、项目群聊、消息到任务卡片的转化。
- Workflow API：definition、version、run、node run、schedule、自动化定义 pause/terminate、单次 run pause/resume/terminate、approval request。
- Event API：SSE/WebSocket 或轮询策略、事件 payload、前端 invalidation 规则。
- Git API：默认关闭，显式开启后提供能力探测、worktree session 创建、状态查看和清理；禁止自动 init、branch、commit、merge、push。

门禁输出应包括：

- OpenAPI 或等价 API 文档。
- TypeScript DTO 或 schema 生成策略。
- 错误码和错误 payload。
- 幂等键规则。
- 技术错误重试规则：默认 2 次，间隔 1 分钟和 5 分钟。
- 无 heartbeat 检查规则：30 分钟无 heartbeat 由协调 Agent 检查。
- 重指派失败规则：同节点重指派 2 次仍失败进入 `blocked` 并通知 Human。
- 时间、枚举、状态字段的兼容策略。

## 6. 负责人批准门禁

进入真正源码实现前，UI 和 PM 至少要完成以下校验：

- 任务看板页面：列、卡片、父子任务折叠、方案确认勾选、执行态标记、评论入口。
- 项目空间页面：项目入口、项目文档、项目群聊、项目任务联动。
- Agent 页面：默认 Agent、创建 Agent、职责补充、项目固定 Agent。
- 新对话页面：Human 与 Agent 的 1:1 对话。
- 工作流页面：列表、画布、节点属性、保存、运行、暂停、终止、运行记录。
- 运行记录页面：任务输出、子任务输出、节点输出、错误事件。
- Git/worktree 设置入口：默认关闭，仅作为后段可选能力展示。

负责人批准规则：

- PM 负责产品范围和 UI 设计包批准。
- SA 负责架构边界和接口约束批准。
- TPM 负责实施范围、依赖顺序和角色交付批准。
- QA 与 SRE 共同负责测试结果和发布候选批准。
- UI 具体高保真视觉可分阶段由 PM 批准，但核心信息架构和交互状态必须先收口。
- 用户已明确后续团队检查点由负责人把控，不再逐项等待用户批准；该委托不替代全局 `AGENTS.md` 对项目源码修改规定的强制门禁。

## 7. 并行波次

可并行：

- M0 中 BE server 骨架、FE Web Shell、SRE Compose、QA smoke checklist 可以并行，但 migration schema 名称需先由 SA 对齐。
- M1 中 FE 静态看板组件可与 BE 模型开发并行，但不得固化未确认状态机。
- M3 Agent 页面可与 M4 项目文档模型并行，前提是 Agent/Profile/Memory 基础契约已确认。
- M5 工作流画布 UI 可与 BE workflow model 并行，前提是节点类型、连线规则、版本化规则已确认。

不可并行或需要前置：

- FE 不应在 API 契约前做联调。
- Runner 不应在 execution lease 和 allowed roots 校验前执行真实 Codex。
- 工作流调度不应先于 WorkflowVersion 和 WorkflowRun。
- 发布候选不应先于 QA 端到端链路和 SRE 门禁。

## 8. QA 测试评审与退出标准

QA 必测链路：

- 任务 happy path。
- 方案确认开关。
- 评论确认、评论返工。
- 父子任务折叠、聚合、部分验收。
- Runner 崩溃、lease 恢复、失败可见。
- 技术错误默认重试 2 次，间隔 1 分钟和 5 分钟。
- 30 分钟无 heartbeat 后协调 Agent 检查并产生可追溯事件。
- 同节点重指派 2 次仍失败后进入 `blocked` 并通知 Human。
- 工作流保存、运行、人工确认、判断 yes/no、暂停、恢复、终止。
- 定时规则：周期重复、预定时间、AI 解析规则、结构化不规则时间。
- 项目文档刷新和 @ 协作。
- 项目文档版本、diff、回退、Human 锁定章节和冲突保护。
- 临时群聊关联任务、归档总结沉淀到任务上下文、项目记忆和项目文档候选更新。
- 可选 Git/worktree 后段能力在默认关闭状态下不影响无 Git 闭环。
- Docker 首次启动、重启、备份恢复、升级 migration。

不可放行条件：

- Backlog 被系统自动执行。
- 看板列出现未确认执行态列。
- 任务完成但输出或事件不可追溯。
- Runner 可访问 allowed roots 之外路径。
- Workflow run 未绑定 version。
- 人工确认节点绕过 Human 继续执行。
- 自动化定义暂停/终止错误影响已产生 WorkflowRun，或单次 run 暂停/终止未按实例维度生效。
- Git/worktree 默认开启，或系统自动 init、branch、commit、merge、push。
- 权限模块半成品进入 MVP 并影响数据访问。

## 9. SRE 发布门禁

发布候选必须满足：

- 默认 Docker Compose 不需要用户额外安装 host runner。
- 不配置外部项目 mount 时也可完整运行。
- 可选 bind mount 必须通过环境变量显式声明 allowed roots。
- `server`、`runner`、`migrate`、`postgres` 生命周期清晰。
- `/healthz`、`/readyz`、runner heartbeat 可观测。
- 数据、workspace、`CODEX_HOME` 的备份恢复说明可执行。
- migration 失败时 server 不进入 ready。
- Runner 停止、暂停、终止不会留下不可解释的运行状态。
- macOS、Linux、Windows 声明支持范围内的 Docker Desktop 或 Docker Engine 场景均通过验收。
- 可选 Git/worktree 在 bind mount 和 managed volume 下都有磁盘占用与清理说明。

## 10. 风险与回退

- 状态模型风险：用数据库状态机和枚举契约控制，FE 只展示状态，不自行推导流转。
- Runner 风险：先用 mock/dry-run adapter 打通事件，再接真实 Codex；真实执行失败不破坏业务阶段。
- 工作流复杂度风险：先做手动运行，再接 schedule，再接自动化定义暂停/终止、单次 run 暂停/恢复/终止和人工节点。
- 工作流实例风险：自动化定义暂停/终止与 WorkflowRun 实例暂停/终止必须分离，避免误停已产生运行实例。
- 路径风险：默认 managed volume，外部 bind mount 显式配置，执行前 canonical path 校验。
- 授权风险：Relay 角色只借鉴结构，协序自写职责文本。
- 范围风险：权限管理不进 MVP，避免半成品权限影响所有模块。
- 数据迁移风险：所有 schema 变更通过 `migrate`，升级前先备份 PostgreSQL 和 volumes。
- Git 风险：默认无 Git，只有用户显式开启后才进入 Git/worktree 流程；任何自动 init、branch、commit、merge、push 都视为越权。

回退策略：

- 功能回退以 feature flag 或配置关闭入口为主。
- 执行失败以 job/attempt/event 追加记录为主，不直接删除事实记录。
- 工作流定义通过 version 保护，已运行实例不随新版本改变。
- Docker 发布失败时保留数据 volume，回滚镜像后重新执行 readiness 检查。

## 11. TPM 结论

当前已经具备进入“源码开发前方案确认”的条件，但还不具备直接编码授权。下一步应由主协调 Agent 基于本计划输出面向用户的源码开发前方案，包含 `【修改范围】`、`【接口设计】`、`【边界评估】`；用户确认后再输出 `【逻辑流程复述】` 并二次确认，随后才能进入实际实现。
