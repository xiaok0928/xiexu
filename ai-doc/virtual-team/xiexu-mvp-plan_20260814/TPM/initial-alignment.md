# 协序 MVP TPM 初始对齐

## 1. 实施目标理解

协序 MVP 的目标不是先做一个普通任务看板，也不是先做一个纯 Agent 聊天工具，而是打通“任务可视化管理 + 多 Agent 协作执行 + 工作流自动化 + 本地 Docker 运行”的最小闭环。

MVP 必须覆盖已确认方向：

- Web 应用，Docker Compose 本地部署。
- 后端 Rust，前端 React + TypeScript，数据库 PostgreSQL，MVP 不引入 Redis。
- `server` 作为 HTTP/API/Web control plane，`runner` 作为容器内 Codex 执行进程，`migrate` 负责 schema 迁移。
- 任务看板与项目模块分开，但底层任务、执行、评论、输出和状态流转保持同源。
- Backlog 只记录想法，不执行；Todo 是确认要做且允许 Agent 扫描的入口。
- Todo 卡片默认需要方案确认，用户可在卡片级取消。
- 看板主阶段与执行状态分离：主阶段表达业务流转，`queued/running/blocked/failed` 表达执行态。
- 工作流是独立模块，可保存、配置定时/手动运行，并在任务看板展示运行产生的任务和状态。
- Agent 身份、项目固定 Agent、动态任务参与 Agent、Agent 私有记忆都进入 MVP 设计边界。
- 权限管理暂不做，功能权限和数据权限在 MVP 对外全部开放。

## 2. 当前依赖链判断

实施顺序的硬依赖应按数据与状态机优先，而不是按页面优先：

1. 项目骨架、Docker Compose、PostgreSQL migration、基础配置先落地。
2. 领域模型和状态机先于前端看板，避免 UI 把执行态误建成业务列。
3. `execution_jobs`、`execution_attempts`、`execution_events`、`runner_instances` 先于真实 Codex 执行。
4. 任务评论语义解析、方案确认、验收/返工先于完整多 Agent 协作体验。
5. 项目、Agent、记忆、项目文档在任务闭环稳定后接入。
6. 工作流 definition/version/run/node-run 在任务和执行底座稳定后接入。
7. 定时调度、暂停/终止、人工确认节点、输出沉淀在工作流基础模型后接入。
8. QA 端到端链路、SRE 发布门禁、备份恢复和升级回滚检查作为每个可运行切片的退出条件。

## 3. 角色边界

- PM：维护 MVP 范围、用户故事、验收口径、确认哪些能力属于首版必达。
- UI：确认任务看板、项目空间、工作流画布、聊天、运行记录等页面信息结构与交互，不直接决定领域状态。
- SA：维护核心模型、状态机、模块边界、API 风格、执行一致性和数据持久化方案。
- BE：实现 Rust server、domain/application/infrastructure、migration、任务/工作流/Agent/执行 API。
- FE：实现 React Web，按后端契约消费 API 和事件，不复制业务状态机。
- QA：定义端到端链路、状态机测试、异常恢复测试、不可放行条件。
- SRE：维护 Docker、配置、健康检查、备份恢复、Runner 隔离、发布门禁。
- TPM：负责拆分里程碑、识别并行边界、设置接口和确认门禁、整合各角色交付物。

## 4. 初始计划风险

- 任务主阶段和执行态容易混用，这是最高优先级的产品/技术一致性风险。
- 工作流和任务看板之间必须明确“同一底层执行事实源”，否则会出现两个系统各自记账。
- Runner 在容器内执行 Codex，需要路径白名单、workspace canonical 校验、attempt lease 和可恢复事件流。
- Relay 的 Agent 角色可迁移结构，但职责文本应按协序自行编写，避免直接复制带来授权风险。
- UI 已确认方向，但进入源码实现前仍需要把核心页面交互转成可实施页面清单和 API 契约。
- 目标源码根目录已切换为 `/Volumes/Tools/workspace/task-relay/xiexu`；后续不能从默认交付 workspace 推断源码仓库。

## 5. 下一步

等待 PM 修订版范围和 QA 测试门禁完成后，TPM 将综合全部角色输入，输出 `documents/implementation-plan.md`。该计划只作为实施拆解，不授权直接修改源码；进入代码开发前仍须按 `AGENTS.md` 完成 `【修改范围】`、`【接口设计】`、`【边界评估】` 和 `【逻辑流程复述】` 的确认门禁。
