# M2 执行控制面基线

## 已交付范围

M2 将任务看板连接到可追踪的执行控制面：Runner 周期扫描 Todo 生成方案作业，方案完成后进入 `plan_review`；方案确认评论生成执行作业，受控作业完成后写入执行事件、任务时间线和运行输出，并将任务推进到 `acceptance`；验收通过评论进入 `done`，返工评论回到 `in_progress` 并重新执行。阶段转换与作业入队在同一个 PostgreSQL 事务内完成，单容器内的 Runner 通过数据库行锁领取作业并建立 30 秒租约。

## 数据模型

- `execution_jobs`：作业类型、状态、关联任务、去重键、尝试次数和可执行时间。
- `execution_attempts`：Runner 身份、租约、心跳、完成状态和失败原因。
- `execution_events`：领取、开始、输出和完成/失败等运行事实。
- `run_outputs`：任务运行输出索引，当前保存受控执行摘要。

迁移版本为 `0003_m2_execution_control`，迁移程序保持幂等，不删除已有任务或运行数据。

## API 与状态流转

阶段转换 `POST /api/tasks/:task_id/transitions` 进入 `in_progress` 或评论意图确认方案/返工时：

1. 任务设置为 `execution_status = queued`。
2. 创建 `kind = execute_task` 的作业，并按任务 revision 生成去重键。
3. 写入 `execution.job_queued` 任务事件。

Runner 领取后将执行状态设为 `running`。方案作业成功时将任务设置为 `board_stage = plan_review`；执行作业成功时将任务设置为 `board_stage = acceptance`、`execution_status = succeeded`、`progress_percent = 100`。验收通过评论将任务设置为 `done`，返工评论会创建新的执行作业。运行记录通过 `GET /api/tasks/:task_id/execution` 查询。

## Codex 运行适配

Runner 已接入镜像内的 Codex CLI 适配器。`controlled` 模式仍是默认值，只写入可验证的本地输出；`real` 模式才启动 `codex exec --json`，方案作业使用 `read-only`，执行作业使用 `workspace-write`，并设置非交互审批、超时、敏感环境排除和项目工作区路径约束。CLI 版本固定为 `0.147.0`，`CODEX_HOME` 使用 Docker 命名 Volume 保存登录状态。`GET /api/runtime/codex` 只返回安装、版本、模式和认证状态。

## 当前限制

真实模式需要用户在容器内完成 Codex 登录；本轮验证环境未将宿主机登录凭据复制到容器，也未发起真实模型调用。项目代码工作区目前按项目 UUID 创建，尚未接入项目源码目录绑定和 GitTree 管理。评论中的 `intent` 仍是显式适配入口，不代表已具备自然语言语义理解。未知作业类型会记录失败尝试；租约过期或外部调用失败的作业按 1 分钟、5 分钟退避重试，达到三次总尝试后失败。

## 验证证据

- `docker compose -f compose.yaml build`：Server、Runner、Migrate 和 Web 构建通过。
- `docker compose -f compose.yaml up -d --remove-orphans`：仅运行 `xiexu-app` 与 `xiexu-postgres`，应用健康。
- `GET /readyz`：返回数据库和迁移均 ready。
- `GET /api/runtime/codex`：返回 `codex-cli 0.147.0`、`controlled` 和未认证状态。
- API 烟测：默认方案确认任务走 `backlog -> todo -> plan_review -> in_progress -> acceptance -> done`，返工任务从 `acceptance` 回到 `in_progress` 后再次进入 `acceptance`，每轮均产生可查询的执行尝试和输出。
