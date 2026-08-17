# 协序 UI/FE 实现契约草案

日期：2026-08-16  
依据：`product-decision-baseline.md`、`mvp-scope.md`、`ui-delivery-plan.md`、`versions/v0.1/UI/*`、`ui-approval.md`

## 1. 结论摘要

当前 UI 门禁已由 PM 放行，G0 方向成立。这个契约只约束 UI/FE 的页面结构、字段展示、交互流转和数据依赖，不包含后端实现，也不扩展权限模块、Git 工作树、桌面端能力或超出 MVP 的复杂规则引擎。

## 2. 路由与页面入口

### 2.1 入口布局

| 路由/入口 | 页面 | 说明 | 是否默认 |
| --- | --- | --- | --- |
| `/` | 任务面板 | 进入后默认展示当前项目的任务看板 | 是 |
| `/project/:projectId` | 项目空间 | 项目视角总览、项目群聊、文档、Agent、交付物 | 否 |
| `/workflow/:workflowId` | 工作流 | 可视化节点编排、运行配置、最近运行 | 否 |
| `/chat/new` | 新对话 | Human 与单个 Agent 的一对一对话入口 | 否 |
| `/agents` | Agent | 默认角色、固定项目 Agent、自定义 Agent 管理 | 否 |
| `/runs` | 运行记录 | 按任务、工作流、状态查看执行轨迹 | 否 |
| `/settings` | 设置 | 系统默认项、扫描周期、运行环境、工作区映射 | 否 |

### 2.2 页面切换规则

- 左侧导航负责模块级切换。
- 顶部工具栏的“自动化”按钮直接跳转工作流页。
- 同一项目内切换“任务面板”和“项目空间”时，不丢失当前项目上下文。
- 移动端优先保留模块切换，不强制保留桌面侧栏。

## 3. 组件边界

### 3.1 任务面板

- 左侧/主区域：四到五列看板，包含 Backlog、Todo、方案待确认、处理中、等待验收。
- 右侧：任务详情抽屉，展示事实源、评论、执行标记和关联信息。
- 卡片必须同时承载任务阶段和执行状态，但二者仍是两个字段。

### 3.2 项目空间

- 主区：项目总览、项目群聊、项目任务、项目文档、项目 Agent、交付物。
- 侧区：文档版本、刷新记录、文档变更摘要。

### 3.3 工作流

- 左侧：工作流列表。
- 中间：节点画布，只保留开始/结束、执行、判断、人工确认、连线五类元素。
- 右侧：运行配置、定时规则预览、最近运行。

### 3.4 新对话

- 仅保留 Human 与单个 Agent 的直接会话。
- 如果需要多人协作，应在项目空间内创建临时群聊，而不是把“新对话”做成群聊总入口。

### 3.5 Agent

- 仅展示角色、职责、是否固定、记忆摘要、可补充职责。
- 不在 UI 层引入公司概念，也不先做权限矩阵页。

## 4. Task.task_stage + ExecutionJob.status 映射

### 4.1 任务阶段

| `Task.task_stage` | 类型 | 必填 | 只读 | 含义 | UI 显示 |
| --- | --- | --- | --- | --- | --- |
| `backlog` | enum | 是 | 否 | 仅记录想法，不执行 | Backlog |
| `todo` | enum | 是 | 否 | 想法已确认，待协调 Agent 认领 | Todo |
| `review` | enum | 是 | 否 | 方案待确认 | 方案待确认 |
| `progress` | enum | 是 | 否 | 正在执行 | 处理中 |
| `acceptance` | enum | 是 | 否 | 等待 Human 验收 | 等待验收 |
| `done` | enum | 是 | 否 | 已完成归档 | 归档 |
| `cancelled` | enum | 是 | 否 | 取消或放弃 | 取消 |

### 4.2 执行状态

| `ExecutionJob.status` | 类型 | 必填 | 只读 | 含义 | UI 显示 |
| --- | --- | --- | --- | --- | --- |
| `queued` | enum | 是 | 是 | 已进入执行队列，未开始 | queued |
| `running` | enum | 是 | 是 | 正在执行 | running |
| `blocked` | enum | 是 | 是 | 被评论、依赖或验收回退阻塞 | blocked |
| `failed` | enum | 是 | 是 | 执行失败 | failed |
| `succeeded` | enum | 是 | 是 | 执行成功 | succeeded |
| `cancelled` | enum | 是 | 是 | 被终止 | cancelled |

### 4.3 展示优先级

1. 看板列位置由 `Task.task_stage` 决定。
2. 卡片右下角的状态徽标由 `ExecutionJob.status` 决定。
3. 抽屉顶部 `stage/status` 文案以 `stage / status` 组合展示。
4. 当两者冲突时，阶段优先，执行状态只做补充说明。

## 5. 父子任务、验收和返工

### 5.1 父子关系

| 字段 | 类型 | 必填 | 只读 | 说明 |
| --- | --- | --- | --- | --- |
| `Task.parent_id` | string \| null | 否 | 否 | 空表示父任务；非空表示子任务 |
| `Task.children` | Task[] | 否 | 是 | 父任务下的直接子任务集合 |
| `Task.child_progress` | number | 否 | 是 | 父任务聚合进度，前端只读展示 |

### 5.2 展开与折叠

- 父任务卡片默认显示上层需求和聚合进度。
- 展开后可看到子任务列表和子任务实时进度。
- 子任务卡片保留在任务面板中，不隐藏。

### 5.3 验收规则

- 验收父任务时，父任务及全部子任务一起进入通过态。
- 验收某个子任务时，只影响该子任务，父任务进入部分验收态。
- 某个子任务返工时，前端通过评论意图触发回退，不需要专门的“拒绝节点”。
- 返工后父任务应保留聚合状态，继续显示部分完成或阻塞。

## 6. 评论意图状态

评论不是纯文本展示，前端要把评论内容交给意图识别层，输出流转结果。

| `Comment.intent` | 类型 | 必填 | 只读 | 触发结果 |
| --- | --- | --- | --- | --- |
| `plan_confirm` | enum | 否 | 是 | 方案确认，Review -> Todo/Progress |
| `plan_revision` | enum | 否 | 是 | 继续留在 Review，并追加修改意见 |
| `accept_pass` | enum | 否 | 是 | 验收通过，子任务或父任务进入 Done |
| `accept_rework` | enum | 否 | 是 | 验收失败，回退到 Todo 或 Progress |
| `dependency_request` | enum | 否 | 是 | 依赖方收到协作请求 |
| `dependency_reply` | enum | 否 | 是 | 协作方回复完成或补充接口 |
| `note` | enum | 否 | 是 | 仅记录，不改状态 |
| `question` | enum | 否 | 是 | 仅提问，不改状态 |

### 6.1 意图识别边界

- 识别结果以内容为准，不依赖人工按钮切换。
- 评论发出后可以先进入“待识别”视觉态，再更新为最终意图。
- 无法识别时默认按 `note` 处理，避免误流转。

## 7. Workflow / WorkflowRun / Node / RunOutput

### 7.1 工作流实体

| 字段 | 类型 | 必填 | 只读 | 说明 |
| --- | --- | --- | --- | --- |
| `Workflow.id` | string | 是 | 是 | 工作流主键 |
| `Workflow.name` | string | 是 | 否 | 工作流名称 |
| `Workflow.status` | enum | 是 | 否 | `draft`、`saved`、`running`、`paused`、`terminated`、`archived` |
| `Workflow.mode` | enum | 是 | 否 | `repeat`、`scheduled`、`ai_parsed` |
| `Workflow.schedule_spec` | string | 否 | 否 | 结构化定时规则 |
| `Workflow.schedule_text` | string | 否 | 否 | 自然语言描述 |
| `Workflow.owner_project_id` | string | 是 | 是 | 归属项目 |

### 7.2 工作流运行

| 字段 | 类型 | 必填 | 只读 | 说明 |
| --- | --- | --- | --- | --- |
| `WorkflowRun.id` | string | 是 | 是 | 运行实例主键 |
| `WorkflowRun.workflow_id` | string | 是 | 是 | 所属工作流 |
| `WorkflowRun.status` | enum | 是 | 是 | `queued`、`running`、`paused`、`terminated`、`succeeded`、`failed` |
| `WorkflowRun.trigger_type` | enum | 是 | 是 | `manual`、`scheduled`、`event` |
| `WorkflowRun.started_at` | datetime | 是 | 是 | 开始时间 |
| `WorkflowRun.ended_at` | datetime \| null | 否 | 是 | 结束时间 |

### 7.3 节点

| 字段 | 类型 | 必填 | 只读 | 说明 |
| --- | --- | --- | --- | --- |
| `WorkflowNode.id` | string | 是 | 是 | 节点主键 |
| `WorkflowNode.workflow_id` | string | 是 | 是 | 所属工作流 |
| `WorkflowNode.node_type` | enum | 是 | 否 | `start`、`end`、`action`、`decision`、`manual_confirm` |
| `WorkflowNode.title` | string | 是 | 否 | 节点名称 |
| `WorkflowNode.body` | string | 否 | 否 | 节点描述或提示词 |
| `WorkflowNode.config` | object | 否 | 否 | 节点配置占位 |

### 7.4 运行输出

| 字段 | 类型 | 必填 | 只读 | 说明 |
| --- | --- | --- | --- | --- |
| `RunOutput.id` | string | 是 | 是 | 输出主键 |
| `RunOutput.run_id` | string | 是 | 是 | 所属运行 |
| `RunOutput.node_id` | string \| null | 否 | 是 | 来源节点 |
| `RunOutput.output_type` | enum | 是 | 是 | `log`、`artifact`、`delta`、`comment` |
| `RunOutput.content` | string | 是 | 是 | 输出正文 |
| `RunOutput.linked_task_id` | string \| null | 否 | 是 | 关联任务 |

## 8. 项目群聊和新对话

### 8.1 项目群聊

- 入口在项目空间。
- 群聊里发布的需求可以直接生成任务卡片。
- 项目明确提及时，任务自动关联到对应项目。
- 同一批 Agent 可以在项目空间和任务面板中共享上下文。

### 8.2 新对话

- 新对话只处理 Human 与一个 Agent 的单轮或多轮澄清。
- 新对话中生成的任务草稿默认进入 Backlog。
- 用户确认后再拖入 Todo。
- 如果需要多人参与，应该切换到项目群聊，而不是在新对话里模拟群组。

## 9. Agent 与记忆字段

### 9.1 Agent 实体

| 字段 | 类型 | 必填 | 只读 | 说明 |
| --- | --- | --- | --- | --- |
| `Agent.id` | string | 是 | 是 | Agent 主键 |
| `Agent.name` | string | 是 | 否 | Agent 名称 |
| `Agent.role_name` | string | 是 | 否 | 角色名，例如 PM、SA、UI、协调 Agent |
| `Agent.is_fixed_project_agent` | boolean | 是 | 否 | 是否为固定项目 Agent |
| `Agent.responsibilities` | string[] | 是 | 否 | 职责列表 |
| `Agent.extra_responsibilities` | string[] | 否 | 否 | 额外补充职责 |
| `Agent.memory_summary` | string | 否 | 是 | 运行经验摘要 |

### 9.2 记忆边界

- `memory_summary` 记录的是 Agent 解决问题和执行任务时沉淀的经验，不是聊天隐私。
- 固定 Agent 与临时 Agent 的记忆隔离展示，但都可以在 Agent 页查看摘要。
- 新建 Agent 时允许 Human 输入职责，由 AI 辅助润色成可执行的角色定义。

## 10. 运行记录与跳转

### 10.1 列表字段

| 字段 | 类型 | 必填 | 只读 | 说明 |
| --- | --- | --- | --- | --- |
| `RunRecord.source_type` | enum | 是 | 是 | `task`、`workflow`、`acceptance` |
| `RunRecord.source_id` | string | 是 | 是 | 来源 ID |
| `RunRecord.status` | enum | 是 | 是 | 与执行态一致 |
| `RunRecord.agent_name` | string | 是 | 是 | 执行 Agent |
| `RunRecord.output_count` | number | 是 | 是 | 输出数量 |
| `RunRecord.created_at` | datetime | 是 | 是 | 创建时间 |

### 10.2 筛选与跳转

- 支持按来源类型、状态、Agent、时间筛选。
- 点击某条记录跳转到对应任务、工作流或验收详情。
- 运行详情页以时间线方式展示领取、扫描、处理、输出等步骤。

## 11. 设置边界

### 11.1 系统设置

- Todo 默认是否需要方案确认。
- Todo 扫描周期。
- 工作流调度方式。
- 容器与宿主机工作区映射。

### 11.2 项目设置

- 当前项目的文档刷新规则。
- 项目 Agent 默认职责。
- 项目群聊是否允许自动生成任务。
- 项目级工作流是否允许定时触发。

### 11.3 明确不做

- 先不做权限管理页。
- 先不做外部注册页。
- 先不做批量暂停和批量终止。

## 12. 移动端交互

- 桌面侧栏在小屏上收起为顶部模块切换。
- 看板列在移动端改为单列选择器，不强行并排挤压。
- 详情抽屉改为下拉或全屏页，不保留桌面右侧固定抽屉。
- 工作流画布在移动端以缩放后的预览为主，编辑仍以桌面优先。

## 13. 加载、空态和错误态

### 13.1 加载态

- 任务列表、项目群聊、运行记录、工作流列表都需要骨架屏或局部加载态。
- 行为提交中必须禁用重复提交。

### 13.2 空态

- Backlog 空时提示“先记录想法”。
- Todo 空时提示“拖入已确认任务”。
- 工作流空时提示“先保存一个流程”。

### 13.3 错误态

- 网络错误展示重试按钮。
- 意图识别失败时保留原评论，并标记为待人工确认。
- 工作流运行失败时保留运行输出和最近步骤。

## 14. API 数据依赖占位

当前只定义前端依赖，不锁定后端实现形式。接口命名可后续与 BE 对齐。

| 资源 | 依赖用途 | 状态 |
| --- | --- | --- |
| `GET /projects` | 项目列表、项目切换 | 占位 |
| `GET /projects/:id/overview` | 项目空间总览 | 占位 |
| `GET /tasks` | 任务面板数据源 | 占位 |
| `GET /tasks/:id` | 任务详情抽屉 | 占位 |
| `POST /tasks/:id/comments` | 评论与意图识别 | 占位 |
| `GET /workflows` | 工作流列表 | 占位 |
| `GET /workflows/:id` | 工作流画布和配置 | 占位 |
| `POST /workflows/:id/runs` | 运行工作流 | 占位 |
| `GET /workflow-runs` | 最近运行记录 | 占位 |
| `GET /agents` | Agent 页面 | 占位 |
| `GET /settings` | 系统设置 | 占位 |

## 15. 交付说明

这个契约对应的 UI 原型已经在 `versions/v0.1/UI` 中落地，PM 已确认方向成立。后续 FE 只需要在这个契约上对齐组件接口、状态机和数据映射，不要再把任务面板与项目空间合并成单一视图，也不要回退到纯聊天式交互。
