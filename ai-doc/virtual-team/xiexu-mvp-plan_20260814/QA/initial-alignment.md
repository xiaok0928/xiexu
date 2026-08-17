# 协序 MVP QA 初始对齐

日期：2026-08-14
角色：QA
范围：协序 MVP 实施计划阶段的测试策略、门禁和不可放行条件。

## 1. 基线与判断

QA 以以下文件作为当前测试基线：

- `/Volumes/Tools/workspace/task-relay/xiexu/ai-doc/virtual-team/product-baseline_20260814/documents/product-decision-baseline.md`
- `/Volumes/Tools/workspace/task-relay/xiexu/ai-doc/virtual-team/xiexu-architecture_20260814/documents/architecture-baseline.md`
- `/Volumes/Tools/workspace/task-relay/xiexu/ai-doc/virtual-team/xiexu-architecture_20260814/documents/system-architecture.md`
- `/Volumes/Tools/workspace/task-relay/xiexu/ai-doc/virtual-team/xiexu-architecture_20260814/SRE/deployment-runtime.md`
- `/Volumes/Tools/workspace/task-relay/xiexu/ai-doc/virtual-team/xiexu-mvp-plan_20260814/documents/mvp-scope.md`
- `/Volumes/Tools/workspace/task-relay/xiexu/ai-doc/virtual-team/xiexu-mvp-plan_20260814/PM/initial-alignment.md`
- `/Volumes/Tools/workspace/task-relay/xiexu/ai-doc/virtual-team/xiexu-mvp-plan_20260814/SA/initial-alignment.md`
- `/Volumes/Tools/workspace/task-relay/xiexu/ai-doc/virtual-team/xiexu-mvp-plan_20260814/SRE/initial-alignment.md`
- `/Volumes/Tools/workspace/task-relay/xiexu/ai-doc/virtual-team/xiexu-mvp-plan_20260814/BE/initial-alignment.md`
- `/Volumes/Tools/workspace/task-relay/xiexu/ai-doc/virtual-team/xiexu-mvp-plan_20260814/FE/initial-alignment.md`
- `/Volumes/Tools/workspace/task-relay/xiexu/ai-doc/virtual-team/xiexu-mvp-plan_20260814/UI/initial-alignment.md`
- `/Volumes/Tools/workspace/task-relay/xiexu/ai-doc/virtual-team/xiexu-mvp-plan_20260814/UI/ui-delivery-plan.md`

当前不应把以下已确认事项当作阻塞：MVP 暂不做权限模块、MVP 不引入 Redis、Git 不是核心依赖、工作流任务默认不需要最终验收、Backlog 只记录想法且不被 Agent 扫描、任务看板与项目开发视图分离但共用同一批底层任务和执行者。

## 2. 按里程碑测试策略

### M0：Docker 运行底座与数据持久化

测试目标：

- `postgres`、`migrate`、`server`、`runner` 能通过 Docker Compose 完整启动。
- `migrate` 只在 `postgres` healthy 后运行，且失败时 `server` 和 `runner` 不进入可用状态。
- 新项目默认进入 Docker managed volume，已有项目可通过允许的 bind mount 根目录访问。
- 重启容器后用户、项目、任务、运行记录、输出、产物版本仍可查询。

核心用例：

- 首次启动、重复启动、迁移失败、数据库未就绪、runner 异常退出、server 重启、volume 删除前后的行为。
- 未配置外部项目目录时，系统仍可创建托管项目并运行基础任务。
- 配置外部项目目录时，只有 allowed roots 内路径可被 runner 访问。

门禁：

- `docker compose config` 通过。
- 健康检查顺序可观测。
- 容器重启后关键数据不丢失。

### M1：Execution 协议与 runner lease

测试目标：

- `execution_jobs`、`execution_attempts`、`execution_events`、`runner_instances`、`run_outputs` 形成唯一执行事实源。
- runner 通过 lease 领取任务，续租、超时、重试、失败和恢复可追踪。
- 并发 runner 不会重复执行同一个 job。

核心用例：

- 单 runner 正常领取并完成 job。
- 两个 runner 同时抢占同一 job，仅一个成功。
- runner 执行中崩溃，lease 过期后可重新分配。
- 重复提交同一触发请求不会产生不可解释的重复执行。
- 执行事件按时间和 attempt 关联可还原。

门禁：

- 任意 job 必须能追踪到创建原因、当前状态、最后 attempt、事件流水和输出。
- 失败不能直接新增看板列，只能作为执行状态展示在任务卡片或详情中。

### M2：任务看板状态机

测试目标：

- 用户可见阶段只允许 `backlog`、`todo`、`plan_review`、`in_progress`、`acceptance`、`done`、`cancelled`。
- 执行状态 `queued/running/blocked/failed` 与看板阶段分离。
- Backlog 不执行；Todo 才进入系统周期扫描和协调 Agent 指派。
- Todo 卡片默认需要方案确认，用户可在卡片右下角取消该项。

核心用例：

- 新对话或手动录入创建 Backlog 想法，不产生 execution job。
- 用户拖入 Todo 后，被周期扫描发现并由协调 Agent 指派。
- 勾选需要方案确认：Todo -> plan_review -> Human 评论确认 -> in_progress。
- 取消需要方案确认：Todo -> in_progress，不进入 plan_review。
- execution failed 时保持 `in_progress` 或当前业务阶段，并显示失败标记。
- 用户取消任务进入 `cancelled`，不得继续被扫描执行。

门禁：

- 所有状态变更必须经过后端状态机校验。
- FE 只能渲染后端返回状态，不能复制或绕过状态机。

### M3：评论语义、验收与父子任务

测试目标：

- 评论可表达方案通过、方案调整、验收通过、验收失败、补充说明、提及协助等意图。
- 评论语义只能作为状态机输入，不能直接改数据库状态。
- 父任务和子任务在看板中均可见，父任务可展开/收起子任务并展示聚合进度。

核心用例：

- plan_review 下评论“确认方案”进入执行。
- plan_review 下评论“需要调整方案”保持或回到可重新出方案状态。
- acceptance 下评论“验收通过”进入 `done` 并从活跃看板移入历史。
- acceptance 下评论“这里不对，重新设计方案”回到 `todo`。
- acceptance 下评论“按当前方案返工修一下”回到 `in_progress`。
- 验收父任务时，全部子任务随父任务验收通过。
- 只验收一个子任务时，父任务显示部分验收。
- 子任务返工时，父任务聚合状态和进度同步回退。
- 子任务 A 在评论中 `@` 子任务 B 或 Agent，请求接口、方法或协助；被提及方感知、回复、完成后 A 能继续确认。

门禁：

- 语义识别低置信度时必须保留为普通评论或等待 Human 明确，不得自动推进关键状态。
- Agent 不得自己验收自己完成的普通任务。
- 父子任务的聚合状态必须可重复计算，不能依赖前端临时状态。

### M4：工作流定义、运行与人工确认

测试目标：

- 工作流作为独立模块存在，任务看板只展示工作流运行产生的上层任务、子任务、运行状态和输出入口。
- 节点支持开始/结束、执行、判断、人工确认、连接线。
- 判断节点通过连线上的 `yes/no` 分支表达结果。
- 运行期不可修改正在运行的版本，非运行状态可编辑并保存新版本。
- 运行记录可查看整体输出和子任务输出。

核心用例：

- 新建画布、保存、再次编辑、运行一次、配置周期性重复、配置预定时间、通过 AI 解析重复规则。
- 不规则预定时间以结构化数据保存，不只保存自然语言文本。
- 人工确认节点暂停当前 run，Human 评论或确认后继续。
- 暂停工作流定义时，关联运行实例挂起；恢复后继续。
- 终止工作流定义或单次 run 后，运行被标记放弃且不会继续执行。
- 工作流运行产生的任务卡片自动显示工作流名称。
- 工作流任务默认不进入最终验收，人工确认节点除外。

门禁：

- 同一工作流运行必须绑定确定的 workflow version。
- 暂停、恢复、终止必须幂等。
- 没有“错过后补跑一次”的自动补跑逻辑；需要补跑时由用户手动触发。

### M5：产物、记忆与项目文档

测试目标：

- 交付物采用 Docker volume 文件存储和 PostgreSQL metadata/versioning。
- 输出形成不可变版本，记录 hash、大小、mime、来源 run、来源任务、创建者 Agent 和摘要。
- Agent 私有记忆绑定具体 `agent_id`，保存执行经验、失败教训、协作习惯和问题解决方法。
- 项目文档在父任务完成后刷新，并通过定时任务兜底发现遗漏变更。

核心用例：

- 同一任务多次输出形成多个 artifact version，不覆盖历史版本。
- 删除或失效用户账号时，已完成数据不删除；正在运行的个人相关执行按后续账号策略停止或阻塞。
- Agent 写入私有记忆后，同一 Agent 后续执行可检索；其他同角色 Agent 不自动共享。
- 父任务完成触发项目文档刷新；定时兜底能补齐遗漏变更。

门禁：

- 文件存在但 metadata 缺失、metadata 存在但文件缺失，都必须可检测并报告。
- 产物读取不得越过项目 allowed roots 或 volume 边界。

## 3. 契约测试门禁

BE、FE、runner、scheduler 之间至少需要以下契约：

- Task API：创建、移动、展开/收起父任务、修改 `need_plan_review`、取消、历史查询。
- Comment API：写入评论、返回语义识别结果、状态机校验结果、低置信度处理。
- Execution API 或内部协议：创建 job、runner 领取、attempt 写入、lease 续租、事件追加、输出回写。
- Workflow API：定义保存、版本化、运行、暂停、恢复、终止、人工确认、运行记录查询。
- Artifact API：输出列表、版本详情、下载或预览、hash 和来源追踪。
- Realtime 契约：SSE/WebSocket 只做 query invalidation 或局部 patch，最终状态以 server 查询为准。

契约测试必须覆盖：

- 正常返回结构、空列表、分页、非法状态迁移、重复请求、并发请求、过期版本、权限暂不启用下的数据可见性。
- 后端字段枚举与前端列定义一致。
- 错误码能区分用户输入错误、状态冲突、执行失败、系统不可用和外部项目路径不可访问。

## 4. E2E 验收链路

首条端到端链路不从工作流画布开始，应先验证任务主链路：

```text
创建项目
-> 创建 Backlog 想法
-> 用户拖入 Todo
-> 周期扫描生成 prepare_task_plan execution_job
-> runner 领取并写 execution_events
-> 任务进入 plan_review
-> Human 评论确认方案
-> server 创建 execute_task execution_job
-> runner 写 run_outputs 和 artifact metadata
-> 任务进入 acceptance
-> Human 评论验收通过进入 done
```

第二条链路验证返工：

```text
acceptance
-> Human 评论表达验收失败
-> 语义解析给出返工意图
-> 状态机决定回到 todo 或 in_progress
-> runner 重新执行
-> 再次进入 acceptance
```

第三条链路验证工作流：

```text
保存工作流版本
-> 手动运行
-> 执行节点创建任务或 job
-> 判断节点按 yes/no 分支流转
-> 人工确认节点挂起
-> Human 确认后继续
-> 运行结束并展示输出
```

## 5. Docker 跨平台验证边界

MVP 交付是 Web + Docker，不绑定 macOS 桌面能力。QA 的跨平台边界如下：

- 必测：macOS Docker Desktop，Linux Docker Engine。
- 建议测：Windows + WSL2 Docker Desktop 的基础启动、浏览器访问、volume 持久化。
- 不承诺：依赖 Xcode、Windows SDK、宿主机 GUI、特殊硬件、宿主机桌面自动化的项目执行。
- 不测为 MVP 放行门禁：原生 Windows 容器、Docker socket 动态挂载任意宿主目录、远程分布式 runner。

跨平台验证只覆盖协序容器自身能否运行、项目目录能否按配置挂载、runner 能否在容器内执行受支持命令。被管理项目自身的 OS 专属构建失败，应记录为项目环境限制，不作为协序核心功能失败。

## 6. 不可放行条件

以下任一项未满足，QA 不应给出放行结论：

- Docker Compose 无法从空环境启动到 Web 可访问。
- migration 失败后系统仍继续接受任务执行。
- Backlog 被 Agent 自动扫描或执行。
- 看板列出现未确认阶段，例如 `running`、`failed`、`blocked` 独立列。
- 评论语义绕过状态机直接改任务状态。
- Todo 默认不是“需要方案确认”。
- 用户取消方案确认后仍强制进入 plan_review。
- 普通任务可以被执行 Agent 自己验收通过。
- execution job 在并发 runner 下重复执行且没有幂等保护。
- runner 崩溃后 job 永久卡死且没有可观测恢复路径。
- workflow run 未绑定稳定版本，导致运行中定义变化影响已开始的 run。
- 暂停或终止后仍继续创建新的执行 job。
- artifact 文件与 metadata 不一致且系统不可检测。
- FE 前端状态与后端事实源冲突时以前端状态为准。

## 7. QA 初始结论

当前状态为“测试计划已对齐，未执行验证”。下一阶段 QA 需要 BE、FE、SRE 提供可运行的最小垂直切片后，按 M0 到 M2 先建立自动化 API 和 E2E 门禁，再扩展到工作流、产物、记忆和项目文档。

当前不输出 `LGTM`。
