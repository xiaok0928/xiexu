# 协序 MVP 前端实施初始对齐

日期：2026-08-14
角色：FE
范围：只对齐 Web 前端实施计划，不修改源码，不新增产品决策。

## 1. FE 结论

协序前端首版应采用“Relay 的 Web 工程形态 + Dashi Taskboard 的任务看板交互骨架”。

具体判断：

- 工程基座跟随 Relay：React + TypeScript + Vite，作为 `apps/web` 交付，由 Rust `server` 承载静态资源、HTTP API、SSE/WebSocket。
- 看板体验参考 Dashi：左侧导航、项目/任务入口、横向列式任务面板、任务卡、任务详情、评论、自动化入口、列表/甘特等视图能力可以借鉴。
- 不迁移 Dashi 的 Tauri、host runtime、桌面托盘、窗口控制、macOS/Windows 适配逻辑。
- 不迁移 Dashi 的“自动认领待办”独立菜单语义，新项目中它属于设置项；右上角“自动化”应进入工作流画布。
- FE 第一条端到端链路不应从工作流画布开始。应先打通“项目 -> 看板 -> Todo 任务 -> 方案确认 -> 执行中 -> 验收 -> 评论驱动返工/完成”，再接入工作流。

## 2. 前端实施切片

### Slice 0：Web Shell 与基础工程

目标：让 `apps/web` 可以在 Docker 内被 `server` 承载，并具备稳定页面壳。

页面与组件：

- AppShell：左侧导航、顶部项目切换、全局搜索入口、用户入口。
- Router：任务面板、项目空间、项目群聊、工作流、运行记录、设置。
- ApiClient：统一请求封装、错误模型、AbortSignal、认证头预留。
- RealtimeClient：SSE/WebSocket 事件接收，先支持任务和运行事件。

依赖后端：

- `GET /api/meta`
- `GET /api/me`
- `GET /api/projects`
- `GET /api/events/stream` 或 WebSocket 等价通道

### Slice 1：项目与任务看板主链路

目标：复刻 Dashi 看板核心体验，并落到协序已确认状态机。

页面与组件：

- BoardPage：项目维度任务面板。
- BoardColumn：`backlog`、`todo`、`plan_review`、`in_progress`、`acceptance`。
- TaskCard：父子任务、聚合进度、Agent/用户头像、标签、来源项目/工作流、是否需要方案确认勾选。
- TaskDetailDrawer：任务描述、状态、子任务、评论、附件、执行记录入口。
- TaskComposer：手动创建想法或任务，支持后续 AI 总结入口。

关键行为：

- Backlog 仅记录想法，不执行。
- 用户手动拖入 Todo 后，进入可扫描状态。
- Todo 卡片右下角默认勾选“需要方案确认”，可单项取消。
- 父任务可展开/收起；父卡显示聚合进度，子卡显示实时进度。
- 评论区支持语义流转入口，但 FE 只负责提交评论与展示系统判定结果，不在前端判断业务意图。

依赖后端：

- `GET /api/projects/:projectId/tasks`
- `POST /api/projects/:projectId/tasks`
- `PATCH /api/tasks/:taskId`
- `POST /api/tasks/:taskId/move`
- `POST /api/tasks/:taskId/comments`
- `POST /api/tasks/:taskId/acceptance`
- `GET /api/tasks/:taskId/events`

### Slice 2：项目群聊与新对话

目标：把任务、项目群聊、新对话串起来，但避免做复杂 IM。

页面与组件：

- ProjectChatPage：项目群聊，支持需求消息同步生成任务卡片。
- DirectChatPage：一对一新对话，类似当前用户与 Codex 的对话形态。
- MentionComposer：评论和聊天中的 `@Agent`、`@任务`、`@文档`。
- MessageTimeline：用户、Agent、系统事件统一展示。

关键行为：

- 项目群聊发布需求时，可以生成关联任务。
- 任务评论中的 `@` 触发协助感知，由后端/Agent 处理，FE 展示协助状态与回复。
- 新对话无第三方参与；临时群聊属于项目内能力，后续切片再扩展。

依赖后端：

- `GET /api/projects/:projectId/messages`
- `POST /api/projects/:projectId/messages`
- `GET /api/chats`
- `POST /api/chats`
- `GET /api/chats/:chatId/messages`
- `POST /api/chats/:chatId/messages`
- `GET /api/mention-suggestions`

### Slice 3：工作流列表、画布与运行记录

目标：实现独立工作流模块，并让任务面板展示工作流运行状态。

页面与组件：

- WorkflowListPage：工作流定义列表、状态、最近运行结果。
- WorkflowCanvasPage：开始/结束、执行、判断、人工确认、连接线。
- NodeEditor：自然语言节点内容编辑，节点类型、描述、输入输出摘要。
- ScheduleEditor：周期性重复、预定时间、AI 解析规则；不规则预定时间以结构化结果展示。
- RunHistoryPage：运行实例、节点运行、输出、子任务输出。

关键行为：

- 工作流保存后形成版本，运行中的版本不可修改。
- 判断节点的连线标记“是/否”，不做复杂规则编辑器。
- 人工确认节点通过评论语义决定继续路径，不设置固定拒绝节点。
- 暂停工作流时，暂停该工作流下正在运行的实例；终止代表放弃。
- 工作流产生的任务卡显示工作流名称，来源自动关联。

依赖后端：

- `GET /api/workflows`
- `POST /api/workflows`
- `PATCH /api/workflows/:workflowId`
- `POST /api/workflows/:workflowId/versions`
- `POST /api/workflow-runs`
- `POST /api/workflow-runs/:runId/pause`
- `POST /api/workflow-runs/:runId/resume`
- `POST /api/workflow-runs/:runId/terminate`
- `GET /api/workflow-runs/:runId`
- `GET /api/workflow-runs/:runId/outputs`

### Slice 4：Agent 身份、记忆与项目文档

目标：让用户能看到参与“人”的构成、项目文档和 Agent 记忆结果，但不把复杂权限放入 MVP。

页面与组件：

- AgentsPage：Relay 预置 Agent 角色、用户自定义 Agent、职责补充口。
- AgentProfileEditor：名称、职责、模型配置、私有记忆摘要。
- ProjectDocsPage：项目文档、版本、刷新记录、父任务完成触发刷新状态。
- MemoryView：项目共享记忆与 Agent 私有记忆摘要，优先只读展示。

依赖后端：

- `GET /api/agents`
- `POST /api/agents`
- `PATCH /api/agents/:agentId`
- `GET /api/agents/:agentId/memories`
- `GET /api/projects/:projectId/documents`
- `POST /api/projects/:projectId/documents/refresh`

## 3. 状态管理建议

服务端状态：

- 使用 TanStack Query 或等价轻量 query cache 管理项目、任务、评论、工作流、运行记录。
- 所有可被 Agent、Runner、Scheduler 改变的数据都视为服务端状态，不放入全局 store 当真相源。
- SSE/WebSocket 事件只做 query invalidation 或局部 patch，避免前端复制状态机。

页面状态：

- 看板拖拽中的 hover、drop preview、列折叠、父任务展开、当前打开 drawer 属于页面状态。
- 工作流画布中的临时节点坐标、选中节点、框选、拖动中连线属于页面状态。

表单状态：

- 任务创建、评论、Agent 配置、工作流节点编辑、调度规则编辑使用局部 form state。
- 所有提交按钮必须处理提交中、失败重试、重复提交禁用。

全局状态：

- 当前用户、当前项目、主题/语言、左侧导航折叠可放全局。
- 不在 MVP 实现细粒度权限 store。用户已确认权限模块后置，前端只保留字段兼容空间。

## 4. Dashi 可借鉴范围

可借鉴：

- `BoardColumn` 的列内拖拽、drop preview、任务重排体验。
- `TaskCard` 的卡片密度、标签、优先级、参与人、评论入口、进度展示。
- `TaskDetail`/评论相关组件的任务详情组织方式。
- `AiChat` 的线程式对话体验，可作为新对话和项目群聊的 UI 参考。
- `ProjectAutomationMenu` 的浮层、开关、调度字段交互，可拆解到设置页和工作流调度编辑器。
- `DashboardView`、`IssueListView`、`GanttView` 的多视图信息组织方式，MVP 可先保留看板和列表，甘特延后。

不建议借鉴：

- Tauri 桌面窗口、托盘、host runtime 发布能力。
- Dashi 当前任务状态命名：协序应使用已确认的 `backlog/todo/plan_review/in_progress/acceptance/done/cancelled`。
- Dashi 的本地 API 路径与 `X-Taskboard-*` 语义。
- Dashi 的自动认领按钮位置与含义。协序的右上角“自动化”是工作流入口，自动认领是设置项。

## 5. API 契约依赖与前后端对齐点

FE 需要后端尽早冻结这些 DTO：

- `Task`：父子关系、状态、聚合进度、来源类型、来源工作流名称、需要方案确认、负责人 Agent、参与者、标签、更新时间。
- `TaskComment`：作者、作者类型、正文、附件、提及对象、语义判定结果、触发的状态流转、创建时间。
- `WorkflowDefinition`：名称、状态、当前版本、调度摘要、创建者、最近运行。
- `WorkflowVersion`：节点、连接线、不可变快照、版本号。
- `WorkflowRun`：状态、开始/结束时间、暂停/终止原因、节点运行摘要、输出入口。
- `AgentProfile`：角色、职责、模型配置摘要、是否预置、是否固定项目 Agent。
- `ExecutionEvent`：任务/工作流/节点来源、事件类型、可读消息、进度、时间。

FE 不应在前端实现这些判断：

- 评论是验收通过、返工、补充意见还是普通评论。
- Todo 由哪个 Agent 认领。
- 工作流人工确认节点下一步走哪条线。
- 项目文档是否需要刷新。
- Agent 私有记忆是否写入。

## 6. 复杂交互风险

- 看板父子任务拖拽：父任务和子任务跨列移动会影响状态机、聚合进度和验收语义，MVP 建议先限制“父任务整组移动”和“子任务单独移动”两种清晰行为。
- 评论语义流转：用户期望通过评论决定验收成功或失败，FE 必须清楚展示“系统已识别的意图”和“即将流转的目标状态”，否则用户会觉得状态变化不可控。
- 工作流画布：自然语言节点能被 Agent 识别，不等于用户能读懂流程。画布卡片必须显示节点类型、短标题、关键输入输出、执行主体和状态。
- 实时状态：Runner、Agent、用户都可能更新任务。FE 必须有事件版本号或 `updatedAt` 冲突提示，避免拖拽后被实时事件覆盖导致错觉。
- 调度规则：AI 解析自然语言后必须给用户结构化结果确认或展示，尤其是不规则预定时间，不能只保存原文。
- 输出查看：运行输出和子任务输出需要统一入口，否则用户会在任务详情、运行记录、项目文档之间迷失。

## 7. 建议最早端到端切片

最早 E2E 不做完整工作流画布，先做“任务执行闭环”：

1. 用户进入项目任务面板。
2. 用户在 Backlog 创建想法。
3. 用户拖到 Todo，默认保留“需要方案确认”。
4. 系统扫描后由协调 Agent 生成方案，任务进入 `plan_review`。
5. 用户在任务详情评论确认方案，任务进入 `in_progress`。
6. Runner/Agent 写入执行事件，任务卡显示实时进度。
7. 执行完成进入 `acceptance`。
8. 用户通过评论验收通过，任务进入 `done` 并从活跃看板消失；或评论返工，任务回到 `todo`。

这个切片能同时验证：

- Dashi 风格任务看板是否适合协序。
- 评论语义流转是否可被用户理解。
- Agent 协调、Runner 事件、任务状态机、前端实时刷新是否能连成闭环。
- 后续工作流运行生成任务时，可以复用同一套卡片、详情、评论和事件展示。

## 8. FE 排期建议

第一阶段：

- Web Shell、API client、Realtime client。
- Project list、BoardPage、BoardColumn、TaskCard、TaskDetailDrawer。
- 任务创建、拖拽状态流转、评论、实时事件展示。

第二阶段：

- ProjectChatPage、DirectChatPage、MentionComposer。
- 项目群聊生成任务卡片。
- 任务详情中展示关联聊天、Agent 协助回复。

第三阶段：

- WorkflowListPage、WorkflowCanvasPage、RunHistoryPage。
- 保存版本、手动运行、暂停/恢复/终止、查看输出。

第四阶段：

- AgentsPage、ProjectDocsPage、MemoryView。
- Agent 创建与职责补充、项目文档刷新记录、记忆摘要展示。

## 9. FE 需要主协调确认或推动的事项

这些不是新产品决策，但需要 BE/SA/TPM 在实施前固定：

- 任务状态 DTO 与状态流转 API 是命令式接口，还是通过通用 comment/transition endpoint 实现。
- 看板拖拽是否允许父子任务跨层级移动，还是仅允许同层排序与跨列移动。
- 实时事件采用 SSE 还是 WebSocket；FE 两者都能做，但 DTO 结构要统一。
- 工作流画布是否采用现成库，如 React Flow。建议采用成熟库，避免自研节点连线基础能力。
- Dashi 的视觉资产是否允许直接复用。若许可证或品牌边界不清，建议只复用交互结构，不复用图标与插画文件。
