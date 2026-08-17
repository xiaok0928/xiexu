# 协序 xiexu 首版架构基线

日期：2026-08-14
角色：SA
状态：已确认
范围：六项技术选择复核。只读参考本地 `relay` 与 `dashi-taskboard`，未联网，未修改源码。

## 0. 复核依据

本次只读核对：

- `/Volumes/Tools/workspace/task-relay/relay`
- `/Volumes/Tools/workspace/task-relay/dashi-taskboard`

仓库与索引：

- `relay` Git 根目录为 `/Volumes/Tools/workspace/task-relay/relay`。
- `dashi-taskboard` Git 根目录为 `/Volumes/Tools/workspace/task-relay/dashi-taskboard`。
- 两个仓库均有 CodeGraph CLI 索引，`codegraph status` 显示 up to date，但提示索引由旧版本引擎构建；本次未重建索引。

关键事实：

- Relay 是 Rust workspace，包含 `apps/server`、`apps/agent-trigger`、`apps/mcp-server`、`crates/domain`、`crates/application`、`crates/infrastructure`、`crates/mcp`，后端依赖 `axum`、`tokio-postgres`、`r2d2_postgres`、`rmcp`。
- Relay `deploy/README.md` 明确：生产环境唯一应用数据库为 PostgreSQL；部署栈包含数据库迁移、Rust Server、Web；`Agent Trigger` 默认运行在宿主机，以便访问宿主机 Codex 登录态、本地项目与 Git worktree。
- Relay `docker-compose.yml` 中 Redis 和 MinIO 标记为 `future` profile，不是当前核心依赖。
- Dashi Taskboard 是 Node.js 22 + React/Vite + 本地 HTTP API + SQLite；它的任务看板、CLI、Codex Skill、`codex exec --json -C <workspace>` 事件流值得借鉴，但其 SQLite/单机任务板架构不适合作为协序多用户多 Agent 后端基线。

## 1. 后端语言：Rust

结论：协序 MVP 后端建议使用 Rust，并优先复用 Relay 的既有多用户、Agent、记忆、触发器和 MCP 能力，而不是用 TypeScript 重写核心后端。

理由：

- 协序的核心不是普通 CRUD，而是多 Agent 身份、任务触发、长期/短期记忆、运行记录、审批、项目工作区、Codex profile、MCP 工具契约和后台触发器。Relay 已经在 Rust 中实现了这些平台能力。
- Relay 的 Rust 后端边界清晰：`server` 负责 API/control plane，`agent-trigger` 负责触发和 Codex 运行，domain/application/infrastructure 分层已有可复用基础。
- 如果用 TypeScript 重写，需要重新实现 Agent identity、memory、trigger lease、run activity、approval、MCP tool、workspace 准备和安全边界，风险高于收益。
- Taskboard 的价值主要在 UI/看板交互、任务状态体验、CLI/Skill 使用方式和 Codex event 归一化，不应反向决定后端语言。

主要风险：

- Rust 迭代速度低于 TypeScript，需要避免过早抽象和过度平台化。
- 复用 Relay 时必须做产品裁剪，不能把 Relay 的 company 概念、权限模型、Harness/Git 强绑定完整搬入 MVP。
- 需要明确哪些 Relay module 直接复用、哪些只借鉴设计，否则容易变成大规模迁移工程。

建议边界：

- 后端控制面：Rust。
- Agent Trigger/Runner：Rust，复用 Relay 的触发控制、租约和运行记录模型，但部署为容器内 Runner。
- Web 前端：React/TypeScript，参考 Taskboard 的看板体验。
- Taskboard 的 Node 服务端不作为协序核心后端，只作为交互和 Codex CLI event 处理参考。

## 2. 数据库：PostgreSQL

结论：使用 PostgreSQL 16+。

理由：

- Relay 生产部署已经采用 PostgreSQL，且有迁移、实时事件和多 Agent 状态存储经验。
- 协序需要强事务和可审计状态：任务父子关系、评论驱动流转、workflow run、人工确认、Agent memory、Codex run、运行事件、交付物版本都不适合只用 SQLite。
- PostgreSQL 可承担 MVP 的队列、租约、定时扫描、状态机一致性、事件 cursor 和 JSONB 扩展。

主要风险：

- 本地 Docker 安装需要数据库备份和恢复机制，不能只靠容器存在。
- 任务状态和 workflow 状态必须用显式状态机和版本号，不能只靠字符串字段随意更新。
- 大日志和大附件不要直接塞进普通业务表，应使用文件存储 + metadata。

## 3. MVP 是否引入 Redis：不引入

结论：MVP 不引入 Redis。

理由：

- Relay 已把 Redis 标为 future profile，说明当前平台能力不依赖 Redis。
- 本地单机 Docker 场景下，PostgreSQL 已能承担队列、lease、定时扫描和基础 realtime。
- Redis 会增加部署、备份、故障恢复和一致性复杂度，但不能替代 PostgreSQL 事实源。

MVP 替代：

- 队列：PostgreSQL job 表 + `FOR UPDATE SKIP LOCKED` 或显式 lease。
- 定时：PostgreSQL schedule 表 + Agent Trigger/worker 周期扫描。
- 实时：PostgreSQL `LISTEN/NOTIFY` 或事件 cursor + WebSocket/SSE。
- 锁：PostgreSQL advisory lock 或行级租约。

后续引入条件：

- 多 server/worker 横向扩展。
- WebSocket fanout 明显增大。
- presence、typing、短 TTL 协作状态成为热点。
- PostgreSQL queue 经过压测确认成为瓶颈。

## 4. 交付物管理：文件存储 + PostgreSQL 元数据 + 版本

结论：不把 Git 作为 MVP 交付物系统的必选依赖。采用文件存储保存内容，PostgreSQL 保存元数据、版本、来源 run 和关联任务。

设计：

- 文件落在协序数据目录，例如 `/var/lib/xiexu/artifacts` 或宿主机 `.xiexu/artifacts`。
- PostgreSQL 保存 `artifact`、`artifact_version`、`run_output`、`attachment`、`project_document_version`。
- 每次 Agent 输出形成不可变版本，记录 hash、大小、mime、来源 run、来源任务、创建者 Agent、摘要。
- 任务卡片展示摘要和进度，详情页查看完整输出、子任务输出、运行记录和附件。

Git 边界：

- Git 不是协序核心功能的运行前提，没有 Git 的项目可以正常使用任务、Agent、工作流和交付物功能。
- 协序默认不执行 `git init`，不自动创建分支、提交、合并或推送。
- 公司项目默认不由协序自动创建大量分支。
- 单人项目或可控仓库保留 Git/worktree 能力，可借鉴 Relay 的隔离 worktree。
- 对已有 Git 项目，协序可以读取仓库状态、生成 patch/diff 或执行用户明确授权的 Git 操作；是否提交公司 Git 由 Human 或外部流程决定。
- Runner 镜像仍预装 Git，用于 Clone、读取已有仓库和可选的 Git/worktree 模式。

主要风险：

- 文件存储会膨胀，需要按项目、run、artifact 类型统计容量，并提供保留策略。
- 无 Git 模式下多个 Agent 同时改同一文件有覆盖风险，需要协调 Agent + path lease + 冲突检测。
- Artifact 与运行记录必须可追溯，否则后续验收和返工难以定位来源。

## 5. Codex 部署：MVP 采用容器内 Agent Runner

结论：MVP 使用单一 Docker Compose 交付包，Codex CLI 安装在独立的 `xiexu-runner` 容器内。用户无需安装宿主机 Runner、Codex CLI 或桌面客户端。

理由：

- 协序明确为纯 Web 应用，目标是用户只安装 Docker，完成一次部署后通过浏览器使用全部能力。
- 独立 runner 容器可以统一 Codex 版本、运行环境、日志、并发限制、进程终止和升级方式。
- `server` 不直接运行 Codex，也不直接读写项目源码；所有 Agent 执行都由 runner 承担。
- Relay 的 Agent 身份、Profile、Trigger、Memory 和运行记录设计继续复用，但不沿用其必须安装宿主机 Trigger 的部署方式。

MVP 形态：

- Docker Compose 内：PostgreSQL、Rust Server、Web、migration、`xiexu-runner`。
- `xiexu-runner` 预装 Codex CLI、Git 和首版支持范围内的常用跨平台工具链。
- 每个 Agent 使用独立 Codex profile/home，隔离凭证、Skill、MCP 配置和运行上下文。
- Codex 登录或 API 配置通过 Web 完成，并保存在持久化 Docker Volume 中。

支持边界：

- 协序可部署在安装了 Docker 的 macOS、Windows 和 Linux 主机上。
- 首版主要支持 Web、后端、脚本、CLI 和其他跨平台项目。
- 依赖 Xcode、Windows SDK、宿主机 GUI 或特殊硬件环境的原生项目不在首版范围内。

## 6. Docker 项目目录挂载

结论：容器内 runner 支持“托管工作区”和“本地工作区”两种项目来源。托管工作区使用 Docker Volume；本地工作区通过安装时声明的允许根目录进行 bind mount。

托管工作区：

- 新建、上传、压缩包导入或 Git Clone 的项目保存到协序管理的工作区 Volume。
- 项目不依赖固定宿主机路径，适合迁移和备份，是新项目的默认方式。

本地工作区：

- 已有公司项目通过 bind mount 映射到 runner 容器。
- 安装时声明一个或多个允许项目根目录，UI 只能添加这些根目录下的项目。
- 新增允许根目录需要修改部署配置并重新创建 runner 容器，不需要重新安装协序。

```yaml
services:
  worker:
    image: xiexu/worker:latest
    environment:
      XIEXU_PROJECT_ROOTS: /workspace/projects
      CODEX_HOME: /home/xiexu/.codex
    volumes:
      - type: bind
        source: ${XIEXU_PROJECTS_ROOT}
        target: /workspace/projects
      - xiexu_data:/var/lib/xiexu
      - xiexu_codex_home:/home/xiexu/.codex
```

`.env` 示例：

```env
XIEXU_PROJECTS_ROOT=/Volumes/Tools/workspace
```

映射示例：

- 宿主机路径：`/Volumes/Tools/workspace/task-relay/relay`
- 容器路径：`/workspace/projects/task-relay/relay`

主要风险：

- 用户新增 `/Users/me/project-a` 但 compose 只挂载了 `/Volumes/Tools/workspace` 时，runner 无法访问该项目。
- macOS Docker Desktop 还需要在设置中允许共享对应宿主机目录。
- 挂载 Docker socket 虽可动态创建任意挂载容器，但权限风险过高，不建议 MVP 使用。

## 7. 最终建议

明确建议：

1. Rust 后端，优先复用 Relay 多用户、Agent、记忆、触发器、MCP 和运行记录能力。
2. PostgreSQL 作为唯一事实源。
3. MVP 不引入 Redis。
4. 交付物采用文件存储 + PostgreSQL 元数据 + 不可变版本，Git/worktree 只作为可选源码协作能力。
5. MVP 使用单一 Docker Compose 交付包，Codex CLI 在独立的 `xiexu-runner` 容器内运行，不安装宿主机 Runner 或桌面客户端。
6. 新项目默认使用 Docker 托管工作区；已有项目通过静态允许根目录 bind mount 接入，并接受不能任意访问未挂载宿主机路径的限制。

这条路线在复用 Relay 多 Agent 平台底座的同时，满足协序纯 Web、单一 Docker 交付和无需宿主机附加安装的产品要求；Taskboard 主要作为任务看板和 Codex 交互体验参考。

## 8. Docker 服务划分

状态：已确认。

首版使用 3 个长期服务和 1 个一次性初始化进程：

- `postgres`：系统唯一事实源，同时承担任务队列、运行租约、状态一致性和事件通知。
- `server`：Rust HTTP API，托管 Web 静态资源，处理任务、项目、工作流、聊天、评论、审批和定时调度。
- `runner`：领取执行任务，运行 Codex 和工作流节点，访问项目目录，写入运行事件并生成交付物。
- `migrate`：启动时执行数据库迁移，成功后退出；`server` 和 `runner` 在迁移成功后启动。

首版不单独拆分 `web`、`scheduler`、`artifact-service` 或 `realtime-service`。`server` 与 `runner` 必须保持独立，避免项目目录、Codex 凭证、外部命令执行和资源故障进入面向用户的 API 安全边界。

`server` 与 `runner` 通过 PostgreSQL 控制表协作：`server` 在事务中创建 job 和 run，`runner` 使用 lease 与 `FOR UPDATE SKIP LOCKED` 领取任务并续租，运行事件写回 PostgreSQL，再由 `server` 推送到 Web。

定时调度循环首版内置于 `server`。调度器负责准时创建持久化运行任务，具体执行由 `runner` 完成；Runner 暂时不可用时，已创建的任务保留等待执行。后续横向扩展时通过 PostgreSQL advisory lock 或 schedule lease 保证单次触发。

## 9. 详细设计索引

- [MVP 系统架构](./system-architecture.md)：Rust 模块边界、领域对象、状态机、Server/Runner 协议和参考项目复用清单。
- [部署与运行设计](../SRE/deployment-runtime.md)：Compose、网络、Volume、可选本地挂载、备份恢复、升级回滚和运行安全。
