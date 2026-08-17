# 协序 MVP 部署与运行设计

日期：2026-08-14
角色：SRE
状态：已确认
范围：单机 Docker Compose 交付、运行可靠性、数据持久化、升级回滚、跨宿主边界。

## 1. 结论

协序 MVP 采用单一 Docker Compose 交付包。当前对用户暴露两个服务：`postgres` 和 `app`。`app` 容器内部按顺序运行一次性迁移，然后同时托管 `server` 与 `runner`；`server` 负责 Web/API，`runner` 负责后台执行。这样用户只需管理一个应用容器，数据库仍保持独立以便备份、升级和故障隔离。

后续如果 Runner 需要独立扩容或资源隔离，可以再拆分为多个 Runner 容器；这不改变对用户隐藏 Compose 细节的交付目标。

MVP 不引入 Redis、MinIO、宿主机 Runner、桌面客户端、Docker socket 或 Kubernetes。所有持久数据使用 PostgreSQL 与 Docker Volume。已有宿主机项目的 bind mount 是可选能力，不能作为基础 Compose 的必填项；纯托管工作区必须在不配置外部项目目录时也能启动。

## 2. 参考来源

本设计只读参考本地项目：

- `/Volumes/Tools/workspace/task-relay/relay`
- `/Volumes/Tools/workspace/task-relay/dashi-taskboard`

参考方式为 bounded fallback 文件检查，主要查看 Relay 的 `docker-compose.yml`、`docker-compose.prod.yml`、`Dockerfile.server`、`apps/agent-trigger` 相关运行逻辑和已确认的 `architecture-baseline.md`。未联网，未修改参考项目源码。

Relay 可借鉴点：

- `postgres + migrate + server` 的 Compose 结构。
- 使用 PostgreSQL 16。
- Codex 运行环境、profile、CLI home 与执行过程应受管。
- 对外部能力、项目根目录和审批策略进行显式配置。

Relay 不应直接照搬点：

- Redis、MinIO、Harness 在协序 MVP 不进入核心依赖。
- Relay 的宿主机 Codex/agent-trigger 模式需要改成容器内 `runner`。
- 协序不在 MVP 暴露 Docker socket，因此不支持通过服务端动态创建带任意宿主机挂载的新容器。

## 3. Compose 服务边界

当前实现的 Compose 服务边界以本节为准：

`app`

- 启动时先运行 `xiexu-migrate`，迁移失败则不启动业务进程。
- 在同一容器内启动 `xiexu-runner` 和 `xiexu-server`。
- 对外只发布 Web/API 端口；Runner 不单独发布端口。
- 使用 `xiexu_workspace` 和 `xiexu_artifacts` 卷。

`postgres`

- 独立运行 PostgreSQL，并持久化到 `xiexu_pgdata`。

未来执行能力完整实现后，下面的 `migrate`、`server`、`runner` 职责仍然成立，但默认可由 `app` 容器内部编排；只有需要独立扩缩容时才恢复为多个 Compose 服务。

`postgres`

- 运行 PostgreSQL 16+。
- 仅在内部 Docker network 暴露 5432。
- 持久化到 `xiexu_pgdata` volume。
- 通过 `pg_isready` 提供 healthcheck。

`migrate`

- 使用与 `server` 相同版本的 migration artifact。
- 依赖 `postgres: service_healthy`。
- 一次性执行 schema migration，成功后退出 0。
- `server` 和 `runner` 必须等待 migration 完成后启动。
- migration 必须幂等；失败时不启动业务服务。

`server`

- 暴露 Web/API 端口，默认宿主机 `127.0.0.1:8080`。
- 内置 Web 静态资源、HTTP API、任务状态机、工作流调度器、评论语义处理入口、运行事件聚合。
- 不直接运行 Codex，不直接读写项目源码。
- 可以读写 PostgreSQL、`xiexu_data` 中的附件和静态产物元数据缓存。

`runner`

- 内置 Codex CLI、Git、基础构建工具和必要系统依赖。
- 通过 PostgreSQL 拉取待执行 job，回写心跳、日志、状态、产物和错误。
- 基础模式只挂载受管 workspace volume。
- 外部已有项目通过可选 override 显式 bind mount 到 `/workspaces/external/<alias>`。
- 持久化 `CODEX_HOME` 到 `xiexu_codex_home`，用于保存 Codex profile、MCP 配置、插件和认证状态。
- 不暴露公网端口，默认只与 `server`、`postgres` 在内部网络通信。

## 4. 网络与安全边界

Compose 建议定义一个内部网络 `xiexu_internal`，`postgres` 和 `runner` 不发布宿主机端口。只有 `server` 发布 Web/API 端口。

默认安全策略：

- 容器统一启用 `security_opt: ["no-new-privileges:true"]`。
- 不挂载 `/var/run/docker.sock`。
- 不使用 privileged 容器。
- 基础 Compose 不挂载宿主机项目目录。
- 外部项目 override 不允许把宿主机根目录、用户 home、`/Volumes` 整体或公司 Git 根目录整体作为默认挂载。
- 所有密钥通过 `.env`、Docker secret-compatible 文件或安装向导写入受管 volume，不写入镜像。
- `runner` 应支持并发上限，默认 `XIEXU_RUNNER_CONCURRENCY=1`，后续再按机器资源调大。

## 5. Volume 与目录模型

推荐 volume：

- `xiexu_pgdata`：PostgreSQL 数据。
- `xiexu_data`：上传附件、任务产物、运行输出索引缓存、导出包。
- `xiexu_workspaces`：协序创建的新项目工作区。
- `xiexu_codex_home`：容器内 Codex profile、MCP 配置、插件和认证状态，属于敏感数据 volume。
- `xiexu_runner_cache`：依赖缓存、临时构建缓存，可删除重建。

推荐容器路径：

- `/var/lib/xiexu/data`
- `/workspaces/managed`
- `/workspaces/external`
- `/home/xiexu/.codex`
- `/var/cache/xiexu-runner`

新增项目时分两类：

- 协序创建的新项目：落到 `xiexu_workspaces:/workspaces/managed`，用户不需要关心宿主机路径。
- 已有宿主机项目：作为可选能力启用。必须通过额外 Compose override 把某个父目录显式 bind mount 到 `/workspaces/external/<alias>`，之后项目路径只允许选择该挂载树下的子目录。

基础 `.env` 示例：

```env
XIEXU_HTTP_BIND=127.0.0.1
XIEXU_HTTP_PORT=8080
POSTGRES_DB=xiexu
POSTGRES_USER=xiexu
POSTGRES_PASSWORD=change-me
XIEXU_RUNNER_CONCURRENCY=1
XIEXU_RUNNER_ALLOWED_ROOTS=/workspaces/managed
CODEX_HOME=/home/xiexu/.codex
```

外部项目 override `.env` 示例：

```env
XIEXU_EXTERNAL_PROJECTS_ROOT=/Volumes/Tools/workspace
XIEXU_RUNNER_ALLOWED_ROOTS=/workspaces/managed,/workspaces/external/default
```

外部项目示例 mount 映射：

```text
宿主机：/Volumes/Tools/workspace
容器内：/workspaces/external/default

宿主机项目：/Volumes/Tools/workspace/task-relay/relay
容器内项目：/workspaces/external/default/task-relay/relay
```

## 6. Server 与 Runner 协议

MVP 建议使用 PostgreSQL 作为唯一协调面，不引入 Redis。

核心表语义与 SA 文档保持一致：

- `execution_jobs`：待执行任务，包含任务类型、来源对象、目标 workspace、Agent 身份、输入快照、期望状态、优先级和幂等 key。
- `execution_attempts`：每次执行尝试，记录 runner、开始/结束时间、退出码、错误摘要和资源用量。
- `execution_events`：追加式运行事件，包括日志片段、阶段进度、Agent 消息、产物引用和状态变化。
- `runner_instances`：Runner 心跳、能力声明、租约和当前负载。
- `run_outputs`：结构化输出，供任务卡片、工作流运行记录和项目文档刷新提取。

调度方式：

- `server` 创建 job。
- `runner` 使用 `SELECT ... FOR UPDATE SKIP LOCKED` 或等价 lease 机制领取 job。
- `runner` 定期回写 heartbeat 到 `runner_instances`。
- `server` 通过 job 的 `desired_state` 下发 pause、resume、terminate。
- `runner` 轮询或使用 PostgreSQL `LISTEN/NOTIFY` 加速感知；`LISTEN/NOTIFY` 只作为唤醒信号，最终状态以数据库为准。

一致性规则：

- job 创建必须带幂等 key，防止定时器重启产生重复运行。
- workflow run 必须绑定 workflow version，运行中不受后续编辑影响。
- `runner_instances` 租约过期后，只有声明为可重试的 job 才能重新入队。
- terminate 是终态，不自动重试。
- pause 是挂起态，保留执行上下文；如果底层 Codex 进程无法原地挂起，则记录 checkpoint 后停止进程，resume 时按可恢复输入重新启动。

## 7. Codex 运行方式

MVP 选择容器内安装 Codex CLI，随 `runner` 镜像一起交付或由镜像构建阶段固定版本安装。这样用户部署 Docker 后不需要额外安装 Codex、Node、Rust 或桌面依赖。

Codex profile 处理：

- `CODEX_HOME=/home/xiexu/.codex` 挂载到 `xiexu_codex_home`。
- `xiexu_codex_home` 可能包含 API key、profile、MCP 配置和插件配置，必须按敏感凭证目录处理。
- 安装向导负责写入默认 profile 和 API key。
- 多 profile 可后续扩展，但 MVP 至少有一个默认 profile。
- profile 不写入 PostgreSQL 明文，只在数据库保存 profile id、显示名、状态和最后探测结果。

执行约束：

- 每次运行创建独立 working directory 或 task sandbox。
- 任务只能访问 `XIEXU_RUNNER_ALLOWED_ROOTS` 下的目录。
- 产物必须复制或登记到 `xiexu_data`，并形成不可变版本记录。
- Runner 不依赖宿主机 GUI；Web UI 验证若需要浏览器，后续可增加受控 browser service，但不进入 MVP 基线。

## 8. 健康检查与可观测性

健康检查：

- `postgres`：`pg_isready`。
- `migrate`：一次性退出码。
- `server /healthz`：进程存活、数据库连接可用。
- `server /readyz`：migration 版本匹配、核心表可读写、调度器 lease 可获得。
- `runner /healthz` 或 DB heartbeat：进程存活、Codex CLI 可执行、`CODEX_HOME` 可读写、allowed roots 存在。

日志：

- 容器 stdout/stderr 输出结构化 JSON 日志。
- 运行级日志同时写入 `execution_events`，便于 UI 展示。
- 长日志按 chunk 存储，避免单行或单字段过大。

指标：

- server 请求延迟、错误率、调度延迟、待执行 job 数。
- runner 心跳延迟、运行中 job 数、失败率、平均执行时间、terminate/pause 成功率。
- PostgreSQL 连接数、磁盘占用、慢查询。

告警建议：

- migration 失败。
- server 不 ready 超过 3 分钟。
- runner heartbeat 断开超过 2 个心跳周期。
- job 堆积超过阈值。
- PostgreSQL volume 剩余空间低于 15%。

## 9. 备份与恢复

必须备份：

- PostgreSQL：项目、任务、评论、工作流定义、运行状态、产物元数据、Agent 记忆。
- `xiexu_data`：附件、产物版本、导出包。
- `xiexu_workspaces`：协序创建的新项目源码。
- `xiexu_codex_home`：Codex profile、MCP 配置和认证状态。该备份属于敏感备份，可能包含 API key 或可用认证材料。

可选备份：

- `xiexu_runner_cache`：不要求备份，可删除重建。

敏感备份要求：

- `xiexu_codex_home` 备份文件必须限制访问权限，建议生成后设置为仅当前管理员可读写。
- 备份文件离开本机或进入共享存储前建议加密。
- 日志、终端输出和交付报告不得打印 profile 内容、API key、token 或完整配置文件。
- 恢复 `xiexu_codex_home` 后必须执行 Codex 凭证探测；失败时让用户重新配置凭证，不应静默继续执行 Agent 任务。

推荐备份命令由后续交付脚本封装，语义如下：

```bash
docker compose exec postgres pg_dump -U "$POSTGRES_USER" "$POSTGRES_DB" > backup/postgres.sql
docker run --rm -v xiexu_data:/src -v "$PWD/backup":/backup alpine tar -czf /backup/xiexu_data.tgz -C /src .
docker run --rm -v xiexu_workspaces:/src -v "$PWD/backup":/backup alpine tar -czf /backup/xiexu_workspaces.tgz -C /src .
docker run --rm -v xiexu_codex_home:/src -v "$PWD/backup":/backup alpine tar -czf /backup/xiexu_codex_home.sensitive.tgz -C /src .
chmod 600 backup/xiexu_codex_home.sensitive.tgz
```

恢复顺序：

1. 停止 `server` 与 `runner`。
2. 恢复 PostgreSQL。
3. 恢复 `xiexu_data`、`xiexu_workspaces`、`xiexu_codex_home`。
4. 启动 `migrate` 检查 schema。
5. 启动 `server` 与 `runner`。
6. 检查 `/readyz`、runner heartbeat、最近 workflow/job 状态。
7. 对恢复后的 `CODEX_HOME` 执行凭证探测，确认默认 profile 可用。

## 10. 升级与回滚

升级原则：

- 镜像版本、migration 版本和 Web 静态资源版本必须成套发布。
- `migrate` 先执行，成功后启动新 `server` 和 `runner`。
- running job 在升级前应进入 drain：不再领取新 job，等待短任务完成；超过宽限期的长任务标记为 `interrupted`，由用户或系统策略决定是否重新运行。
- workflow run 绑定版本，因此升级不会改变已启动流程的定义。

回滚原则：

- 无破坏性 schema migration 前，可直接回滚镜像版本。
- 一旦 migration 包含不可逆字段删除或语义迁移，必须先完成备份并提供 forward-fix 优先策略。
- 产物版本不可变，回滚应用不会删除已产生的产物和运行记录。

最低可接受发布检查：

- `docker compose config` 成功。
- `migrate` 退出 0。
- `server /readyz` 返回 ready。
- `runner` 上报 heartbeat。
- 创建一条测试 job，runner 执行并写回 `execution_events`、`run_outputs`。

## 11. 资源限制

MVP 默认单机资源建议：

- `postgres`：512MiB memory limit，保留持久 volume。
- `server`：512MiB memory limit，0.5-1 CPU。
- `runner`：2GiB memory limit，1-2 CPU，默认并发 1。
- `migrate`：256MiB memory limit，只在升级时运行。

当用户需要多个 Agent 并行执行时，优先调高 `XIEXU_RUNNER_CONCURRENCY` 和 `runner` 资源限制；如果单容器隔离不足，再扩展为多个 runner 副本。多 runner 时必须依赖数据库 lease 保证同一 job 只被一个 runner 执行。

## 12. 跨宿主系统边界

Web 应用本身可在 macOS、Windows、Linux 的 Docker Desktop 或 Docker Engine 上运行，但“项目执行能力”受容器和项目技术栈限制。

可支持：

- Web 管理、任务看板、工作流、Agent 调度、项目文档和运行记录。
- 纯 Linux 兼容项目的构建、测试、代码生成和文件产物。
- macOS/Windows/Linux 宿主机上的已有项目，只要目录通过可选 override 显式挂载，且构建不依赖宿主专有 SDK。

不可在 MVP 内保证：

- 依赖 Xcode、iOS Simulator、macOS GUI、Windows SDK、Windows GUI、USB 设备、GPU 驱动或企业内网特殊环境的构建与测试。
- 容器动态读取未挂载的宿主机路径。
- 宿主机原生 IDE、桌面 App 或系统级自动化。

Windows 路径注意：

- 安装向导需要把 Windows 路径转换为 Docker Desktop 可挂载路径。
- 项目内路径在数据库中建议保存容器路径与宿主路径映射，运行时以容器路径为准。
- 避免在任务输入中直接固化 `C:\...` 或 `/Volumes/...`，否则跨宿主迁移会失败。

## 13. 基础 Compose 草案

基础 Compose 只支持受管工作区，不要求外部项目目录变量，因此可以在全新机器上直接启动。

```yaml
services:
  postgres:
    image: postgres:16
    environment:
      POSTGRES_DB: ${POSTGRES_DB:-xiexu}
      POSTGRES_USER: ${POSTGRES_USER:-xiexu}
      POSTGRES_PASSWORD: ${POSTGRES_PASSWORD:?set POSTGRES_PASSWORD}
    volumes:
      - xiexu_pgdata:/var/lib/postgresql/data
    healthcheck:
      test: ["CMD-SHELL", "pg_isready -U $$POSTGRES_USER -d $$POSTGRES_DB"]
      interval: 5s
      timeout: 5s
      retries: 20
    networks:
      - xiexu_internal
    security_opt:
      - no-new-privileges:true

  migrate:
    image: ${XIEXU_IMAGE:-xiexu/server:latest}
    command: ["xiexu-migrate"]
    environment:
      DATABASE_URL: postgres://${POSTGRES_USER:-xiexu}:${POSTGRES_PASSWORD}@postgres:5432/${POSTGRES_DB:-xiexu}
    depends_on:
      postgres:
        condition: service_healthy
    networks:
      - xiexu_internal
    security_opt:
      - no-new-privileges:true

  server:
    image: ${XIEXU_IMAGE:-xiexu/server:latest}
    command: ["xiexu-server"]
    environment:
      DATABASE_URL: postgres://${POSTGRES_USER:-xiexu}:${POSTGRES_PASSWORD}@postgres:5432/${POSTGRES_DB:-xiexu}
      XIEXU_DATA_DIR: /var/lib/xiexu/data
      XIEXU_HTTP_BIND: 0.0.0.0
      XIEXU_HTTP_PORT: 8080
    ports:
      - "${XIEXU_HTTP_BIND:-127.0.0.1}:${XIEXU_HTTP_PORT:-8080}:8080"
    volumes:
      - xiexu_data:/var/lib/xiexu/data
    depends_on:
      migrate:
        condition: service_completed_successfully
    networks:
      - xiexu_internal
    security_opt:
      - no-new-privileges:true
    restart: unless-stopped

  runner:
    image: ${XIEXU_RUNNER_IMAGE:-xiexu/runner:latest}
    environment:
      DATABASE_URL: postgres://${POSTGRES_USER:-xiexu}:${POSTGRES_PASSWORD}@postgres:5432/${POSTGRES_DB:-xiexu}
      CODEX_HOME: /home/xiexu/.codex
      XIEXU_DATA_DIR: /var/lib/xiexu/data
      XIEXU_MANAGED_WORKSPACE_ROOT: /workspaces/managed
      XIEXU_RUNNER_ALLOWED_ROOTS: ${XIEXU_RUNNER_ALLOWED_ROOTS:-/workspaces/managed}
      XIEXU_RUNNER_CONCURRENCY: ${XIEXU_RUNNER_CONCURRENCY:-1}
    volumes:
      - xiexu_data:/var/lib/xiexu/data
      - xiexu_workspaces:/workspaces/managed
      - xiexu_codex_home:/home/xiexu/.codex
      - xiexu_runner_cache:/var/cache/xiexu-runner
    depends_on:
      migrate:
        condition: service_completed_successfully
    networks:
      - xiexu_internal
    security_opt:
      - no-new-privileges:true
    restart: unless-stopped

volumes:
  xiexu_pgdata:
  xiexu_data:
  xiexu_workspaces:
  xiexu_codex_home:
  xiexu_runner_cache:

networks:
  xiexu_internal:
    driver: bridge
```

## 14. 外部项目 Override 草案

需要接入已有宿主机项目时，使用额外 override 文件，例如 `docker-compose.external.yml`。启动命令示例：`docker compose -f docker-compose.yml -f docker-compose.external.yml up -d`。

```yaml
services:
  runner:
    environment:
      XIEXU_EXTERNAL_WORKSPACE_ROOT: /workspaces/external/default
      XIEXU_RUNNER_ALLOWED_ROOTS: /workspaces/managed,/workspaces/external/default
    volumes:
      - type: bind
        source: ${XIEXU_EXTERNAL_PROJECTS_ROOT:?set XIEXU_EXTERNAL_PROJECTS_ROOT}
        target: /workspaces/external/default
```

外部挂载约束：

- `XIEXU_EXTERNAL_PROJECTS_ROOT` 只在启用 override 时必填。
- 安装向导应提示用户选择尽量窄的父目录。
- 服务端保存项目路径时同时保存宿主路径、容器路径和 mount alias；Runner 执行时只信任容器路径。

## 15. 暂不进入 MVP 的能力

- Redis、消息队列、对象存储服务。
- 动态容器编排、Docker socket、Kubernetes。
- 宿主机 Runner、桌面端、GUI 自动化。
- 多租户权限隔离。
- 企业级集中日志和 Prometheus/Grafana 打包。
- 自动补跑错过的定时任务。

这些能力可以后续作为扩展进入，但不应影响首版单机 Docker 的可安装性和可解释性。
