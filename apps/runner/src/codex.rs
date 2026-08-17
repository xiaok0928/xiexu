use serde_json::Value;
use std::{env, path::{Path, PathBuf}, process::Stdio, time::Duration};
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
        let executable =
            env::var("CODEX_BIN").unwrap_or_else(|_| "/usr/local/bin/codex".to_owned());
        let mode = match env::var("CODEX_EXECUTION_MODE")
            .unwrap_or_else(|_| "controlled".to_owned())
            .as_str()
        {
            "controlled" => ExecutionMode::Controlled,
            "real" => ExecutionMode::Real,
            value => return Err(format!("unsupported CODEX_EXECUTION_MODE: {value}")),
        };
        let workspace_root = env::var("XIEXU_WORKSPACE_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("/workspace"));
        if !workspace_root.is_absolute() {
            return Err("XIEXU_WORKSPACE_ROOT must be absolute".to_owned());
        }
        let max_run_seconds = env::var("CODEX_MAX_RUN_SECONDS")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(1_800)
            .clamp(30, 3_600);
        Ok(Self {
            executable: PathBuf::from(executable),
            mode,
            workspace_root,
            max_run_seconds,
        })
    }

    /// 返回当前是否启用真实 CLI 执行。
    pub fn is_real(&self) -> bool {
        self.mode == ExecutionMode::Real
    }

    /// 返回可用于日志和状态接口的稳定模式名称。
    pub fn mode_name(&self) -> &'static str {
        if self.is_real() {
            "real"
        } else {
            "controlled"
        }
    }

    /// 返回覆盖最长运行时间的数据库租约秒数。
    pub fn lease_seconds(&self) -> i32 {
        (self.max_run_seconds + 60) as i32
    }

    /// 在受管项目工作区内运行 Codex，并解析 JSONL 最终结果。
    pub async fn run(
        &self,
        kind: &str,
        context: TaskPromptContext<'_>,
    ) -> Result<CodexRunOutput, String> {
        // 未绑定 worktree 的作业保持使用项目默认目录，兼容既有任务和非研发类作业。
        self.run_in_workspace(kind, context, None).await
    }

    /// 在已验证的项目默认目录或 worktree 会话目录中执行 Codex。
    pub async fn run_in_workspace(
        &self,
        kind: &str,
        context: TaskPromptContext<'_>,
        workspace_override: Option<&Path>,
    ) -> Result<CodexRunOutput, String> {
        // 绑定 worktree 时重新完成真实路径校验，避免数据库读取和外部进程启动之间发生符号链接替换。
        let workspace = match workspace_override {
            Some(path) => self.prepare_worktree_workspace(context.project_id, path).await?,
            None => self.prepare_workspace(context.project_id).await?,
        };
        let prompt = build_prompt(kind, &context)?;
        let sandbox = if kind == "execute_task" {
            "workspace-write"
        } else {
            "read-only"
        };

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
        let output = timeout(Duration::from_secs(self.max_run_seconds), command.output())
            .await
            .map_err(|_| format!("Codex run exceeded {} seconds", self.max_run_seconds))?
            .map_err(|error| format!("failed to start Codex: {error}"))?;
        parse_output(output.status.success(), &output.stdout, &output.stderr)
    }

    /// 校验指定 worktree 属于项目受管目录，拒绝相对路径、符号链接逃逸和跨项目目录。
    pub async fn prepare_worktree_workspace(&self, project_id: &str, session_path: &Path) -> Result<PathBuf, String> {
        // 先复用默认项目目录的 UUID 与符号链接防护，确保受管 worktree 根目录建立在可信项目目录下。
        let project_workspace = self.prepare_workspace(project_id).await?;
        if !session_path.is_absolute() {
            return Err("worktree session path must be absolute".to_owned());
        }

        // 会话目录必须已经由控制面创建，Runner 绝不自行创建 worktree、分支或 Git 仓库。
        let sessions_dir = project_workspace.join(".xiexu-worktrees");
        let canonical_sessions = tokio::fs::canonicalize(&sessions_dir)
            .await
            .map_err(|error| format!("cannot resolve managed worktree directory: {error}"))?;
        if !canonical_sessions.starts_with(&project_workspace) {
            return Err("managed worktree directory escapes project workspace".to_owned());
        }
        let canonical_session = tokio::fs::canonicalize(session_path)
            .await
            .map_err(|error| format!("cannot resolve worktree session path: {error}"))?;
        if !canonical_session.starts_with(&canonical_sessions) {
            return Err("worktree session path escapes managed worktree directory".to_owned());
        }
        Ok(canonical_session)
    }

    /// 创建并校验项目工作区，防止符号链接或异常 ID 逃逸允许根目录。
    async fn prepare_workspace(&self, project_id: &str) -> Result<PathBuf, String> {
        // 项目主键必须是 UUID，避免路径分隔符、上级目录和平台特殊路径进入拼接阶段。
        Uuid::parse_str(project_id).map_err(|_| "project_id must be a UUID".to_owned())?;
        tokio::fs::create_dir_all(&self.workspace_root)
            .await
            .map_err(|error| format!("cannot create workspace root: {error}"))?;
        let canonical_root = tokio::fs::canonicalize(&self.workspace_root)
            .await
            .map_err(|error| format!("cannot resolve workspace root: {error}"))?;

        // 先校验 projects 容器目录，避免符号链接在拒绝请求前向允许根之外创建项目目录。
        let projects_dir = canonical_root.join("projects");
        tokio::fs::create_dir_all(&projects_dir)
            .await
            .map_err(|error| format!("cannot create projects directory: {error}"))?;
        let canonical_projects = tokio::fs::canonicalize(&projects_dir)
            .await
            .map_err(|error| format!("cannot resolve projects directory: {error}"))?;
        if !canonical_projects.starts_with(&canonical_root) {
            return Err("projects directory escapes XIEXU_WORKSPACE_ROOT".to_owned());
        }

        // 项目目录创建后再次解析真实路径，拒绝预先存在且指向允许根之外的项目级符号链接。
        let project_dir = canonical_projects.join(project_id);
        tokio::fs::create_dir_all(&project_dir)
            .await
            .map_err(|error| format!("cannot create project workspace: {error}"))?;
        let canonical_project = tokio::fs::canonicalize(&project_dir)
            .await
            .map_err(|error| format!("cannot resolve project workspace: {error}"))?;
        if !canonical_project.starts_with(&canonical_projects) {
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
        context.project_name,
        context.title,
        context.description,
        context.agent_name,
        context.agent_instructions,
        context.memories
    );
    match kind {
        "prepare_task_plan" => Ok(format!("你是协序的项目协调 Agent。请只分析当前任务并输出可供 Human 审核的实施方案，不要修改工作区文件。方案应包含目标、步骤、边界、风险和验收方式。\n\n{task}")),
        "execute_task" => Ok(format!("你是协序的执行 Agent。请在当前工作区完成下面任务，先检查现有内容，再进行必要修改和验证。不得访问当前工作区之外的文件。最后输出完成摘要、验证结果和仍存在的限制。\n\n{task}")),
        "optimize_agent_profile" => Ok(format!("你是协序的 Agent 身份设计助手。请根据用户输入生成职责草案，包含角色定位、核心职责、工作边界、协作方式和结果要求。只输出草案，不修改任何文件或现有 Agent 配置。\n\n{task}")),
        "summarize_conversation" => Ok(format!("你是协序的协作记录整理 Agent。请将对话归纳为目标、关键决定、已完成事项、未完成事项、依赖和后续动作。不要创造对话中不存在的结论。\n\n{task}")),
        "refresh_project_document" => Ok(format!(
            concat!(
                "你是协序的项目协调 Agent。请根据目标章节当前内容、项目其他章节和任务事实，输出目标章节的完整替换候选。",
                "只输出候选正文，不要添加标题、解释、Markdown 代码块或数据库操作；不得修改工作区文件，",
                "不得创造输入中不存在的进展或结论。\n\n{task}"
            ),
            task = task
        )),
        "evaluate_workflow_condition" => Ok(format!(
            concat!(
                "你是协序工作流的判断 Agent。只依据给定规则、运行输入和已完成节点输出判断条件是否成立。",
                "最终消息只能是 yes 或 no，不要解释原因，不要修改工作区文件。\n\n{task}"
            ),
            task = task
        )),
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
            Some("thread.started") => {
                thread_id = event
                    .get("thread_id")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned)
            }
            Some("item.completed")
                if event.pointer("/item/type").and_then(Value::as_str) == Some("agent_message") =>
            {
                final_message = event
                    .pointer("/item/text")
                    .and_then(Value::as_str)
                    .map(|value| truncate(value, 65_536));
            }
            Some("turn.failed") | Some("error") => {
                event_error = event
                    .get("message")
                    .and_then(Value::as_str)
                    .map(|value| truncate(value, 2_000))
            }
            _ => {}
        }
    }
    if !success {
        let stderr = truncate(String::from_utf8_lossy(stderr).trim(), 2_000);
        return Err(event_error
            .or_else(|| (!stderr.is_empty()).then_some(stderr))
            .unwrap_or_else(|| "Codex run failed".to_owned()));
    }
    let content =
        final_message.ok_or_else(|| "Codex completed without a final agent message".to_owned())?;
    Ok(CodexRunOutput { content, thread_id })
}

/// 按字符边界截断外部输出，防止单次运行无限放大数据库记录。
fn truncate(value: &str, max_chars: usize) -> String {
    value.chars().take(max_chars).collect()
}

#[cfg(test)]
mod tests {
    use super::{CodexConfig, ExecutionMode};
    use std::path::PathBuf;
    use uuid::Uuid;

    /// 为单个测试创建不会与并行用例冲突的临时根目录。
    fn temporary_root(case_name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("xiexu-{case_name}-{}", Uuid::new_v4()))
    }

    /// 构造只用于路径校验的受控模式配置，不启动外部 Codex 进程。
    fn test_config(workspace_root: PathBuf) -> CodexConfig {
        CodexConfig {
            executable: PathBuf::from("codex"),
            mode: ExecutionMode::Controlled,
            workspace_root,
            max_run_seconds: 30,
        }
    }

    /// 合法 UUID 应在受管 projects 目录内创建并返回真实路径。
    #[tokio::test]
    async fn prepares_workspace_inside_managed_root() {
        // 准备独立临时目录并执行正常路径。
        let root = temporary_root("workspace");
        let project_id = Uuid::new_v4().to_string();
        let workspace = test_config(root.clone())
            .prepare_workspace(&project_id)
            .await
            .expect("prepare managed workspace");

        // 返回路径必须位于真实 projects 目录，验证后清理测试资产。
        let expected_root = std::fs::canonicalize(root.join("projects"))
            .expect("canonical projects root");
        assert!(workspace.starts_with(expected_root));
        std::fs::remove_dir_all(root).expect("remove test workspace");
    }

    /// 非 UUID 输入必须在任何项目目录创建前被拒绝。
    #[tokio::test]
    async fn rejects_non_uuid_project_id_without_side_effects() {
        // 使用路径穿越形态输入，确认校验先于文件系统写入。
        let root = temporary_root("invalid-id");
        let error = test_config(root.clone())
            .prepare_workspace("../outside")
            .await
            .expect_err("reject invalid project id");

        // 根目录也不应被创建，避免非法请求留下路径副作用。
        assert_eq!(error, "project_id must be a UUID");
        assert!(!root.exists());
    }

    /// Unix 环境下 projects 符号链接不得把项目创建操作引导到允许根之外。
    #[cfg(unix)]
    #[tokio::test]
    async fn rejects_projects_symlink_escape_before_project_creation() {
        use std::os::unix::fs::symlink;

        // 预置指向外部目录的 projects 链接，模拟被篡改的挂载或工作区。
        let root = temporary_root("projects-link");
        let outside = temporary_root("outside");
        std::fs::create_dir_all(&root).expect("create workspace root");
        std::fs::create_dir_all(&outside).expect("create outside root");
        symlink(&outside, root.join("projects")).expect("create projects symlink");
        let project_id = Uuid::new_v4().to_string();

        // 校验应在项目目录创建前失败，外部目录保持为空。
        let error = test_config(root.clone())
            .prepare_workspace(&project_id)
            .await
            .expect_err("reject projects symlink escape");
        assert_eq!(error, "projects directory escapes XIEXU_WORKSPACE_ROOT");
        assert!(!outside.join(project_id).exists());
        std::fs::remove_dir_all(root).expect("remove workspace root");
        std::fs::remove_dir_all(outside).expect("remove outside root");
    }

    /// 已存在的会话目录必须解析到项目受管 .xiexu-worktrees 根目录之下。
    #[tokio::test]
    async fn accepts_worktree_session_inside_managed_project_directory() {
        // 创建项目与会话目录，模拟控制面已经完成 Git worktree 创建后的文件布局。
        let root = temporary_root("managed-worktree");
        let project_id = Uuid::new_v4().to_string();
        let session = root.join("projects").join(&project_id).join(".xiexu-worktrees").join("session-a");
        std::fs::create_dir_all(&session).expect("create managed session directory");

        // 返回路径必须是会话目录的真实路径，供外部 Codex 进程作为 current_dir 使用。
        let workspace = test_config(root.clone())
            .prepare_worktree_workspace(&project_id, &session)
            .await
            .expect("accept managed worktree session");
        assert_eq!(workspace, std::fs::canonicalize(&session).expect("canonical session"));
        std::fs::remove_dir_all(root).expect("remove test workspace");
    }

    /// 项目工作区内的普通目录不得伪装为 worktree 会话目录。
    #[tokio::test]
    async fn rejects_worktree_session_outside_managed_directory() {
        // 创建看似属于项目但不在 .xiexu-worktrees 下的目录，验证目录层级是强制边界。
        let root = temporary_root("unmanaged-worktree");
        let project_id = Uuid::new_v4().to_string();
        let project = root.join("projects").join(&project_id);
        let session = project.join("unmanaged-session");
        std::fs::create_dir_all(project.join(".xiexu-worktrees")).expect("create managed root");
        std::fs::create_dir_all(&session).expect("create unmanaged session");

        // 普通目录不能被用于执行研发任务，避免任务跳出控制面创建的 worktree 集合。
        let error = test_config(root.clone())
            .prepare_worktree_workspace(&project_id, &session)
            .await
            .expect_err("reject unmanaged worktree session");
        assert_eq!(error, "worktree session path escapes managed worktree directory");
        std::fs::remove_dir_all(root).expect("remove test workspace");
    }

    /// Unix 环境下受管目录中的符号链接也不得把 Codex 工作目录带到项目边界之外。
    #[cfg(unix)]
    #[tokio::test]
    async fn rejects_worktree_session_symlink_escape() {
        use std::os::unix::fs::symlink;

        // 预置受管会话根和一个指向外部的会话符号链接，模拟挂载被替换的攻击路径。
        let root = temporary_root("worktree-link");
        let outside = temporary_root("worktree-outside");
        let project_id = Uuid::new_v4().to_string();
        let sessions = root.join("projects").join(&project_id).join(".xiexu-worktrees");
        std::fs::create_dir_all(&sessions).expect("create managed sessions root");
        std::fs::create_dir_all(&outside).expect("create outside directory");
        let session = sessions.join("session-link");
        symlink(&outside, &session).expect("create escaping worktree symlink");

        // 真实路径在允许根之外时必须拒绝，不能仅依赖未解析的字符串前缀。
        let error = test_config(root.clone())
            .prepare_worktree_workspace(&project_id, &session)
            .await
            .expect_err("reject escaping worktree symlink");
        assert_eq!(error, "worktree session path escapes managed worktree directory");
        std::fs::remove_dir_all(root).expect("remove test workspace");
        std::fs::remove_dir_all(outside).expect("remove outside directory");
    }
}
