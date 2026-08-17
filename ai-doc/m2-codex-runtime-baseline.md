# M2 Codex 运行时基线

## 目标

把 M2 的执行控制面接到单容器内可复用的 Codex CLI，同时保留受控模式，保证未完成登录或尚未准备项目工作区时不会意外启动真实执行。

## 运行模式

- `controlled`：默认模式，不启动外部进程，只验证作业领取、租约、状态、事件和输出链路。
- `real`：启动 `/usr/local/bin/codex exec --json`。`prepare_task_plan` 使用 `read-only`，`execute_task` 使用 `workspace-write`。

真实调用使用非交互审批、JSONL 输出、单次超时和 `GIT_TERMINAL_PROMPT=0`；子进程排除 `OPENAI_API_KEY`、`CODEX_API_KEY`、`DATABASE_URL`。最终 Agent 消息截断到 65536 字符，错误截断到 2000 字符。

## 容器配置

镜像固定安装 `@openai/codex@0.147.0`。Compose 注入：

- `CODEX_BIN=/usr/local/bin/codex`
- `CODEX_EXECUTION_MODE=controlled|real`
- `CODEX_MAX_RUN_SECONDS=1800`，实际限制在 30 至 3600 秒
- `CODEX_HOME=/var/lib/xiexu/codex-home`
- `XIEXU_WORKSPACE_ROOT=/workspace`

`/workspace`、`/var/lib/xiexu/artifacts` 和 `/var/lib/xiexu/codex-home` 是独立 Docker Volume。Runner 为每个项目建立 `/workspace/projects/<project-id>`，并校验 canonical path 不得逃逸工作区根目录。

## 认证与状态

启用真实模式前执行：

```bash
docker compose exec app codex login --device-auth
docker compose exec app codex login status
CODEX_EXECUTION_MODE=real docker compose up -d
```

`GET /api/runtime/codex` 返回 `installed`、`version`、`mode` 和 `authenticated`。认证探测只读取退出状态，不返回账号、令牌或 CLI 原始输出。宿主机 Codex 登录状态不会自动复制进容器。

## 失败与追踪

每次外部执行对应一个 `execution_attempts`，成功时记录最终输出和 `codex_thread_id`，失败时保留错误信息。失败作业第一次延迟 1 分钟、第二次延迟 5 分钟，默认第三次失败后终止自动重试。现阶段 thread ID 只作为运行事实保存，尚未提供续接对话 API。

## 未完成边界

本阶段未绑定项目的宿主机源码目录，真实执行使用容器内项目工作区；GitTree/Copy-on-Write、项目 Agent 配置、自然语言评论理解和工作流节点执行仍属于后续阶段。真实模型调用需要用户在目标容器内完成认证后再验证。
