# 协序 M0 运行基线

更新日期：2026-08-16

## 1. 交付范围

M0 已在 `/Volumes/Tools/workspace/task-relay/xiexu` 建立可通过 Docker Compose 启动的 Web 应用骨架。该目录当前不是 Git 仓库，因此本阶段不依赖 Git，也未初始化 CodeGraph。

## 2. 模块职责

- `apps/server`：Rust HTTP 服务，托管 `apps/web/dist`，提供 `/healthz` 和 `/readyz`。
- `apps/runner`：Rust Runner 进程，注册 `runner_instances` 并每 10 秒续租；M0 不领取或执行业务任务。
- `apps/migrate`：一次性 PostgreSQL 迁移进程，创建 `schema_migrations` 和 `runner_instances`。
- `apps/web`：React + TypeScript + Vite Web Shell，提供任务面板、项目空间、工作流、新对话、Agent、运行记录、设置的模块入口占位。
- `crates/domain`、`crates/application`、`crates/infrastructure`：后续业务模型、用例编排和基础设施适配的 Rust 分层占位。

## 3. 启动与配置

执行：

```bash
docker compose -f compose.yaml up --build
```

Compose 对外只定义 `postgres` 和 `app` 两个服务。PostgreSQL 健康后，`app` 容器内先运行 `migrate`，成功后并行启动 `server` 和 `runner`。默认持久化卷为 `xiexu_pgdata`、`xiexu_workspace`、`xiexu_artifacts`。端口可通过 `XIEXU_PORT` 调整，数据库和 Runner 标识通过环境变量配置。

Server 运行时镜像包含 `wget`，用于 Compose healthcheck；Web 深链接由 Server 回退到 `index.html`，刷新 `/project/...` 或 `/workflow/...` 不会返回 404。

## 4. 稳定接口与数据契约

- `GET /healthz`：返回 `{"status":"ok"}`，只表示 Server 进程存活。
- `GET /readyz`：数据库可连接且 `schema_migrations` 存在时返回 200 和 `status=ready`，否则返回 503。
- `runner_instances` 使用 Runner `id` 主键保证重复注册幂等；状态为 `ready`，并写入心跳和 30 秒租约过期时间。

## 5. 当前限制

- M0 不包含任务、评论、工作流执行、Agent 编排、真实 Codex 调用和权限管理。
- Runner 尚未安装或调用 Codex CLI；真实执行能力属于后续里程碑。
- 宿主机不需要额外安装 Rust、Node 或 Codex，构建和运行均由 Docker 完成。
- M0 只验证 Docker Volume 托管工作区；宿主机 bind mount 的业务接入在后续部署配置中实现。

## 6. 验证证据

- `npx tsc --noEmit`：通过。
- `npm run build`：通过。
- `docker compose -f compose.yaml config`：通过。
- `docker compose -f compose.yaml build`：通过，Rust、Web 和运行时镜像均成功构建。
- `docker compose -f compose.yaml run --rm migrate`：重复执行通过，迁移幂等。
- Compose 运行态：PostgreSQL healthy、App healthy；App 日志确认 migration、Server、Runner 均已启动。
- `/healthz`、`/readyz`：分别返回 `ok`、`ready`。
- PostgreSQL 查询确认 `runner-1|ready` heartbeat 记录存在。
- `/project/xiexu`、`/workflow/demo`：深链接返回 Web Shell（HTTP 200）。
