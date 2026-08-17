# 协序 MVP SRE 初始对齐

日期：2026-08-14
角色：SRE
输入来源：

- `/Volumes/Tools/workspace/task-relay/xiexu/ai-doc/virtual-team/product-baseline_20260814/documents/product-decision-baseline.md`
- `/Volumes/Tools/workspace/task-relay/xiexu/ai-doc/virtual-team/xiexu-architecture_20260814/documents/architecture-baseline.md`
- `/Volumes/Tools/workspace/task-relay/xiexu/ai-doc/virtual-team/xiexu-architecture_20260814/documents/system-architecture.md`
- `/Volumes/Tools/workspace/task-relay/xiexu/ai-doc/virtual-team/xiexu-architecture_20260814/SRE/deployment-runtime.md`

本文只作为 MVP 实施计划输入，不修改源码。

## 1. 第一阶段必须纳入的运行能力

第一阶段不能只做业务功能原型，必须同时做最小可运行交付闭环。原因是协序的核心价值依赖 Agent 执行、运行记录、产物持久化和本地 Docker 部署；如果这些基础设施后补，任务、工作流、项目开发三条主线会反复返工。

必须纳入的能力：

- 基础 `docker-compose.yml`：包含 `postgres`、`migrate`、`server`、`runner`，不依赖外部项目 bind mount。
- 可选 `docker-compose.external.yml`：只在用户接入已有宿主机项目时启用，声明外部项目 bind mount 和 `XIEXU_RUNNER_ALLOWED_ROOTS`。
- 一次性 migration：`migrate` 必须在 `postgres` healthy 后运行，`server` 与 `runner` 只能在 migration 成功后启动。
- 核心 Volume：`xiexu_pgdata`、`xiexu_data`、`xiexu_workspaces`、`xiexu_codex_home`、`xiexu_runner_cache`。
- Runner 内置 Codex CLI：首版不要求用户在宿主机安装 Codex，不引入宿主机 Runner。
- PostgreSQL 协调协议：第一阶段就按 `execution_jobs`、`execution_attempts`、`execution_events`、`runner_instances`、`run_outputs` 落库，不引入临时内存队列。
- 基础健康检查：`postgres` healthcheck、`server /healthz`、`server /readyz`、runner heartbeat、Codex CLI/profile probe。
- 基础日志：容器 stdout/stderr 输出结构化日志，运行级事件写入 `execution_events`。
- 基础备份语义：明确 PostgreSQL、`xiexu_data`、`xiexu_workspaces`、`xiexu_codex_home` 的备份范围。
- 凭证敏感处理：`xiexu_codex_home` 按敏感 volume 处理，备份文件限制权限，恢复后必须校验 Codex 凭证。

## 2. Compose 计划输入

基础 Compose 的第一阶段目标是“纯托管工作区可启动”。因此基础配置不能要求 `XIEXU_EXTERNAL_PROJECTS_ROOT`，否则没有已有项目的用户会在安装阶段被阻塞。

基础 Compose 要求：

- `postgres` 不发布宿主机端口，只在内部网络可见。
- `server` 默认发布到 `127.0.0.1:${XIEXU_HTTP_PORT:-8080}`。
- `runner` 不发布端口，只通过 PostgreSQL 与 server 协调。
- `runner` 默认只允许访问 `/workspaces/managed`。
- 所有服务启用 `no-new-privileges:true`。
- 不挂载 Docker socket，不使用 privileged 容器。

外部项目 override 要求：

- 文件独立为 `docker-compose.external.yml`。
- `XIEXU_EXTERNAL_PROJECTS_ROOT` 只在启用 override 时必填。
- bind mount 到 `/workspaces/external/default` 或带 alias 的子路径。
- `XIEXU_RUNNER_ALLOWED_ROOTS` 必须显式追加外部容器路径。
- 安装向导需要提示用户选择尽量窄的父目录，不建议挂载用户 home、磁盘根目录或公司 Git 总目录。

## 3. 迁移计划输入

第一阶段 migration 不能只是开发脚本，必须作为交付包的一部分进入 Compose。

必须实现：

- `migrate` 容器独立命令，例如 `xiexu-migrate`。
- migration 版本与 server 镜像版本成套发布。
- migration 成功退出 0，失败退出非 0。
- `server`、`runner` 使用 `depends_on: service_completed_successfully` 或等价启动顺序。
- `server /readyz` 校验当前数据库 schema version 与应用期望版本匹配。

计划边界：

- 第一阶段只要求向前迁移。
- 不要求自动 downgrade。
- 遇到不可逆迁移时，发布前必须先完成备份。

## 4. Volume 与数据计划输入

第一阶段必须把数据写入位置固定下来，避免后续功能把产物散落到容器临时目录。

必须固定：

- PostgreSQL 保存项目、任务、评论、工作流、运行状态、Agent 记忆、产物元数据。
- `xiexu_data` 保存附件、运行产物和导出包。
- `xiexu_workspaces` 保存协序创建的新项目源码。
- `xiexu_codex_home` 保存 Codex profile、MCP 配置、插件和认证状态。
- `xiexu_runner_cache` 只作为可删除缓存，不进入必须备份范围。

第一阶段验收时必须证明：

- 重启 `server`/`runner` 不丢任务状态。
- 重建 `runner` 容器不丢 Codex profile。
- 删除 `xiexu_runner_cache` 不影响系统恢复。
- 产物有不可变版本记录，能从任务或 workflow run 回溯。

## 5. 凭证计划输入

`CODEX_HOME` 不是普通缓存，必须从第一阶段按敏感目录处理。

必须实现：

- 默认 `CODEX_HOME=/home/xiexu/.codex`。
- `CODEX_HOME` 挂载到 `xiexu_codex_home`。
- 数据库只保存 profile id、显示名、状态、最后探测时间和错误摘要，不保存 API key 明文。
- 安装或首次运行时提供 Codex 凭证配置入口。
- runner 启动时做 Codex CLI 可执行探测与默认 profile 探测。
- 恢复 `xiexu_codex_home` 后必须重新探测凭证。

必须避免：

- 在日志、事件、错误消息、交付文档中输出 API key、token 或完整 profile 内容。
- 把 `xiexu_codex_home` 备份当作普通非敏感附件上传或共享。

## 6. 健康检查计划输入

第一阶段健康检查至少覆盖“服务可启动”和“Agent 能执行”两层。

服务级检查：

- `postgres`：`pg_isready`。
- `migrate`：退出码。
- `server /healthz`：进程存活、数据库连接可用。
- `server /readyz`：schema version 匹配、核心表可读写、调度器可获取 lease。
- `runner`：在 `runner_instances` 写入 heartbeat、能力声明和版本信息。

执行级检查：

- runner 能领取一条测试 `execution_jobs`。
- runner 能创建 `execution_attempts`。
- runner 能写入 `execution_events`。
- runner 能写入 `run_outputs`。
- server 能在 UI/API 层读到运行结果。

## 7. 发布门禁

第一阶段每次可交付版本至少通过以下门禁。

配置门禁：

- `docker compose config` 成功。
- 基础 Compose 不设置 `XIEXU_EXTERNAL_PROJECTS_ROOT` 也能通过配置检查。
- 启用 `docker-compose.external.yml` 后，缺少 `XIEXU_EXTERNAL_PROJECTS_ROOT` 时应明确失败；填写后配置检查通过。
- Compose 文件不包含 Docker socket、privileged 或默认宿主机大目录挂载。

启动门禁：

- `postgres` healthy。
- `migrate` 成功退出。
- `server /readyz` ready。
- `runner_instances` 出现活跃 heartbeat。
- 默认 `CODEX_HOME` 可读写，Codex CLI 探测完成。

功能门禁：

- 创建托管项目工作区成功。
- 创建一条最小测试 job，runner 执行并写回 `execution_events` 与 `run_outputs`。
- 任务或 workflow run 页面能看到执行状态与输出。
- 重启 `server` 和 `runner` 后，未完成 job 不丢失。

数据门禁：

- PostgreSQL 可导出。
- `xiexu_data`、`xiexu_workspaces`、`xiexu_codex_home` 可打包备份。
- `xiexu_codex_home` 备份文件命名或元数据明确标记 sensitive，并限制本地文件权限。
- 恢复后 Codex 凭证探测必须通过，否则系统阻止 Agent 执行并提示重新配置。

## 8. 可以延后的运维能力

以下能力不应压入第一阶段，否则会扩大实现面并影响 MVP 闭环。

- Redis、消息队列、MinIO 或外部对象存储。
- Kubernetes、Helm、Terraform、自动扩缩容。
- Docker socket 动态挂载或动态创建 runner 容器。
- 多机 runner 调度。
- 企业级 Prometheus/Grafana/Loki 打包。
- 自动备份调度与备份保留策略 UI。
- 自动补跑错过的定时任务。
- 完整权限隔离和审计合规报表。
- 浏览器自动化容器、GUI 自动化和宿主机桌面集成。
- 针对 Xcode、Windows SDK、特殊硬件的专用 runner 镜像。

## 9. 关键环境风险

外部项目路径风险：

- Docker 容器不能动态访问未挂载的宿主机路径。
- Windows 与 macOS 路径格式不同，数据库需要保存宿主路径、容器路径和 mount alias。
- 用户选择过大的父目录会扩大 runner 可见范围，安装向导必须限制和提示。

构建环境风险：

- Linux Docker 不能保证构建依赖 Xcode、Windows SDK、宿主机 GUI、USB 设备或特殊硬件的项目。
- 即使 Web 应用跨平台，Agent 对项目的执行能力仍受 runner 镜像和项目依赖限制。

凭证风险：

- `xiexu_codex_home` 泄露等同于 Codex 认证材料泄露风险。
- 备份、恢复、日志采集和问题排查都必须默认脱敏。
- 恢复到新机器后，凭证可能因环境或 profile 状态失效，必须重新探测。

数据增长风险：

- Agent 日志、产物和 workspace 会持续增长。
- 第一阶段至少需要暴露磁盘占用提示和手动清理缓存的路径。
- `xiexu_runner_cache` 可删，`xiexu_data` 与 `xiexu_workspaces` 不可当缓存清理。

升级中断风险：

- 长时间运行的 Agent job 可能在升级或容器重启时中断。
- 第一阶段必须定义 `interrupted` 或等价状态，避免中断 job 被误判成功或重复执行。
- workflow run 需要绑定 workflow version，避免升级后运行定义漂移。

Compose 兼容风险：

- `depends_on: condition: service_completed_successfully` 依赖 Docker Compose 实现版本。
- 如果目标环境不支持，需要提供脚本化启动顺序或 server/runner 启动前自检等待。

## 10. SRE 对第一阶段拆分建议

建议第一阶段按以下顺序交付：

1. 先落 `postgres + migrate + server /healthz /readyz`。
2. 再落基础 volume 与托管 workspace 创建。
3. 再落 `runner_instances` heartbeat 与 Codex profile probe。
4. 再落 `execution_jobs` 到 `execution_events`、`run_outputs` 的最小执行闭环。
5. 最后落备份恢复脚本、external override 和发布门禁脚本。

不建议先做复杂工作流 UI 或多 Agent 并行调度后再补运行底座。原因是任务看板、项目群、工作流运行记录最终都依赖同一套 execution 和 artifact 语义；运行底座晚落会导致前端状态、任务状态和产物模型返工。

