use serde_json::Value;
use std::{env, path::PathBuf, process::Stdio, time::Duration};
use tokio::{process::Command, time::timeout};
use uuid::Uuid;

/// Codex 执行模式，受控模式用于未登录环境，真实模式才启动 CLI。
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ExecutionMode {
    /// 只验证执行控制面，不启动外部进程。
    Controlled,
    /// 在容器内启动 Codex CLI。
    Real,
}

/// Codex 运行配置，集中维护可执行文件、工作区根目录和超时边界。
#[derive(Clone)]
pub struct CodexConfig {
    /// Codex CLI 绝对路径或 PATH 中的命令名。
    executable: PathBuf,
    /// 当前运行模式。
    mode: ExecutionMode,
    /// 所有项目工作区必须位于该目录内。
    workspace_root: PathBuf,
    /// 单次 Codex 运行最大秒数。
    max_run_seconds: u64,
}

/// 传递给 Codex 的任务上下文，不包含数据库凭据或其他敏感配置。
pub struct TaskPromptContext<'a> {
    /// 项目主键，用于创建受管工作区目录。
    pub project_id: &'a str,
    /// 项目名称，仅作为提示上下文。
    pub project_name: &'a str,
    /// 任务标题。
    pub title: &'a str,
    /// 任务说明。
    pub description: &'a str,
    /// 当前执行 Agent 的显示名称；系统作业使用稳定的系统角色名称。
    pub agent_name: &'a str,
    /// 合并模板职责、实例指令和项目职责补充后的执行约束。
    pub agent_instructions: &'a str,
    /// 仅包含当前 Agent 且符合项目、任务范围的相关记忆。
    pub memories: &'a str,
}

/// Codex 成功结果，仅保留最终消息和可选 thread ID。
pub struct CodexRunOutput {
    /// Agent 最终消息，作为任务运行输出持久化。
    pub content: String,
    /// CLI JSONL 中的 thread ID，供后续会话续接使用。
    pub thread_id: Option<String>,
}

impl CodexConfig {
    /// 从环境变量加载配置，并拒绝不支持的执行模式。
    pub fn from_env() -> Result<Self, String> {
        let executable = env::var("CODEX_BIN").unwrap_or_else(|_| "/usr/local/bin/codex".to_owned());
        let mode = match env::var("CODEX_EXECUTION_MODE").unwrap_or_else(|_| "controlled".to_owned()).as_str() {
            "controlled" => ExecutionMode::Controlled,
            "real" => ExecutionMode::Real,
            value => return Err(format!("unsupported CODEX_EXECUTION_MODE: {value}")),
        };
        let workspace_root = env::var("XIEXU_WORKSPACE_ROOT").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from("/workspace"));
        if !workspace_root.is_absolute() {
            return Err("XIEXU_WORKSPACE_ROOT must be absolute".to_owned());
        }
        let max_run_seconds = env::var("CODEX_MAX_RUN_SECONDS").ok().and_then(|value| value.parse::<u64>().ok()).unwrap_or(1_800).clamp(30, 3_600);
        Ok(Self { executable: PathBuf::from(executable), mode, workspace_root, max_run_seconds })
    }

    /// 返回当前是否启用真实 CLI 执行。
    pub fn is_real(&self) -> bool { self.mode == ExecutionMode::Real }

    /// 返回可用于日志和状态接口的稳定模式名称。
    pub fn mode_name(&self) -> &'static str { if self.is_real() { "real" } else { "controlled" } }

    /// 返回覆盖最长运行时间的数据库租约秒数。
    pub fn lease_seconds(&self) -> i32 { (self.max_run_seconds + 60) as i32 }

    /// 在受管项目工作区内运行 Codex，并解析 JSONL 最终结果。
    pub async fn run(&self, kind: &str, context: TaskPromptContext<'_>) -> Result<CodexRunOutput, String> {
        let workspace = self.prepare_workspace(context.project_id).await?;
        let prompt = build_prompt(kind, &context)?;
        let sandbox = if kind == "execute_task" { "workspace-write" } else { "read-only" };

        // 以非交互模式启动 Codex，排除敏感环境进入模型生成的 Shell 子进程。
        let mut command = Command::new(&self.executable);
        command
            .arg("--ask-for-approval")
            .arg("never")
            .arg("exec")
            .arg("--skip-git-repo-check")
            .arg("--json")
            .arg("--color")
            .arg("never")
            .arg("--sandbox")
            .arg(sandbox)
            .arg("--config")
            .arg("shell_environment_policy.exclude=[\"OPENAI_API_KEY\",\"CODEX_API_KEY\",\"DATABASE_URL\"]")
            .arg(prompt)
            .current_dir(workspace)
            .env("GIT_TERMINAL_PROMPT", "0")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let output = timeout(Duration::from_secs(self.max_run_seconds), command.output()).await.map_err(|_| format!("Codex run exceeded {} seconds", self.max_run_seconds))?.map_err(|error| format!("failed to start Codex: {error}"))?;
        parse_output(output.status.success(), &output.stdout, &output.stderr)
    }

    /// 创建并校验项目工作区，防止符号链接或异常 ID 逃逸允许根目录。
    async fn prepare_workspace(&self, project_id: &str) -> Result<PathBuf, String> {
        Uuid::parse_str(project_id).map_err(|_| "project_id must be a UUID".to_owned())?;
        tokio::fs::create_dir_all(&self.workspace_root).await.map_err(|error| format!("cannot create workspace root: {error}"))?;
        let canonical_root = tokio::fs::canonicalize(&self.workspace_root).await.map_err(|error| format!("cannot resolve workspace root: {error}"))?;
        let project_dir = canonical_root.join("projects").join(project_id);
        tokio::fs::create_dir_all(&project_dir).await.map_err(|error| format!("cannot create project workspace: {error}"))?;
        let canonical_project = tokio::fs::canonicalize(&project_dir).await.map_err(|error| format!("cannot resolve project workspace: {error}"))?;
        if !canonical_project.starts_with(&canonical_root) {
            return Err("project workspace escapes XIEXU_WORKSPACE_ROOT".to_owned());
        }
        Ok(canonical_project)
    }
}

/// 按白名单作业类型生成稳定提示，用户输入只作为任务内容而不是 CLI 参数。
fn build_prompt(kind: &str, context: &TaskPromptContext<'_>) -> Result<String, String> {
    // 身份、职责和记忆作为清晰分区进入提示，避免与用户任务内容互相覆盖。
    let task = format!(
        "项目：{}\n工作标题：{}\n工作说明：{}\n\n当前 Agent：{}\n职责约束：{}\n\n相关记忆：{}",
        context.project_name, context.title, context.description, context.agent_name, context.agent_instructions, context.memories
    );
    match kind {
        "prepare_task_plan" => Ok(format!("你是协序的项目协调 Agent。请只分析当前任务并输出可供 Human 审核的实施方案，不要修改工作区文件。方案应包含目标、步骤、边界、风险和验收方式。\n\n{task}")),
        "execute_task" => Ok(format!("你是协序的执行 Agent。请在当前工作区完成下面任务，先检查现有内容，再进行必要修改和验证。不得访问当前工作区之外的文件。最后输出完成摘要、验证结果和仍存在的限制。\n\n{task}")),
        "optimize_agent_profile" => Ok(format!("你是协序的 Agent 身份设计助手。请根据用户输入生成职责草案，包含角色定位、核心职责、工作边界、协作方式和结果要求。只输出草案，不修改任何文件或现有 Agent 配置。\n\n{task}")),
        "summarize_conversation" => Ok(format!("你是协序的协作记录整理 Agent。请将对话归纳为目标、关键决定、已完成事项、未完成事项、依赖和后续动作。不要创造对话中不存在的结论。\n\n{task}")),
        _ => Err(format!("unsupported Codex job kind: {kind}")),
    }
}

/// 解析 Codex JSONL，只持久化最终 Agent 消息、thread ID 和经过截断的错误。
fn parse_output(success: bool, stdout: &[u8], stderr: &[u8]) -> Result<CodexRunOutput, String> {
    let stdout = String::from_utf8_lossy(stdout);
    let mut thread_id = None;
    let mut final_message = None;
    let mut event_error = None;
    for line in stdout.lines() {
        let Ok(event) = serde_json::from_str::<Value>(line) else { continue; };
        match event.get("type").and_then(Value::as_str) {
            Some("thread.started") => thread_id = event.get("thread_id").and_then(Value::as_str).map(ToOwned::to_owned),
            Some("item.completed") if event.pointer("/item/type").and_then(Value::as_str) == Some("agent_message") => {
                final_message = event.pointer("/item/text").and_then(Value::as_str).map(|value| truncate(value, 65_536));
            }
            Some("turn.failed") | Some("error") => event_error = event.get("message").and_then(Value::as_str).map(|value| truncate(value, 2_000)),
            _ => {}
        }
    }
    if !success {
        let stderr = truncate(String::from_utf8_lossy(stderr).trim(), 2_000);
        return Err(event_error.or_else(|| (!stderr.is_empty()).then_some(stderr)).unwrap_or_else(|| "Codex run failed".to_owned()));
    }
    let content = final_message.ok_or_else(|| "Codex completed without a final agent message".to_owned())?;
    Ok(CodexRunOutput { content, thread_id })
}

/// 按字符边界截断外部输出，防止单次运行无限放大数据库记录。
fn truncate(value: &str, max_chars: usize) -> String { value.chars().take(max_chars).collect() }
