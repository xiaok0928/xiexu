use crate::{connect, ApiError, AppState};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::{delete, get},
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    path::{Path as FilePath, PathBuf},
    time::Duration,
};
use tokio::process::Command;
use tokio_postgres::Row;
use uuid::Uuid;

/// 容器内允许启用 Git 控制面的项目工作区根目录。
const PROJECTS_ROOT: &str = "/workspace/projects";
/// 每个项目内由协序独占管理的 worktree 容器目录。
const WORKTREE_DIRECTORY: &str = ".xiexu-worktrees";
/// Git 子进程的单次最长执行时间，避免控制面请求无限占用服务资源。
const GIT_COMMAND_TIMEOUT: Duration = Duration::from_secs(20);

/// 项目 Git 开关更新请求，仅允许显式启用或关闭可选能力。
#[derive(Deserialize)]
struct UpdateProjectGitRequest {
    /// 是否允许该项目调用只读 Git 探测和 worktree 生命周期命令。
    enabled: bool,
}

/// 创建 Git worktree 会话请求，可选绑定到项目内的单个任务。
#[derive(Deserialize)]
struct CreateWorktreeSessionRequest {
    /// 需要关联的项目任务，缺省时创建独立的项目级会话。
    task_id: Option<String>,
}

/// 已通过项目根目录和 Git 顶层目录校验的仓库信息。
struct RepositoryProbe {
    /// 项目仓库的规范化绝对路径。
    repository_path: PathBuf,
    /// 当前提交，用于调用方识别 detached worktree 的基线。
    current_head: String,
    /// 只读 Git status 的紧凑摘要。
    status_summary: String,
}

/// Git 子进程的受控输出，错误信息会在 API 层截断后返回。
struct GitCommandResult {
    /// 子进程是否以成功状态退出。
    success: bool,
    /// 标准输出的 UTF-8 损失转换结果。
    stdout: String,
    /// 标准错误的 UTF-8 损失转换结果。
    stderr: String,
}

/// 注册项目 Git 设置和受管 worktree 会话接口。
pub(crate) fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/projects/:project_id/git", get(get_project_git).patch(update_project_git))
        .route("/api/projects/:project_id/git/sessions", get(list_worktree_sessions).post(create_worktree_session))
        .route("/api/git-worktree-sessions/:session_id", delete(delete_worktree_session))
}

/// 返回项目的 Git 开关、仓库可用性和仅在启用后执行的只读仓库状态。
async fn get_project_git(State(state): State<AppState>, Path(project_id): Path<String>) -> Result<Json<Value>, ApiError> {
    // 先读取持久化开关并验证项目存在，关闭时不得产生 Git 子进程。
    let client = connect(&state).await?;
    let enabled = project_git_enabled(&client, &project_id).await?;

    if !enabled {
        return Ok(Json(git_config_json(false, None)));
    }

    // 已启用项目才探测仓库；仓库在运行期被移动时以不可用状态返回，便于前端恢复配置。
    match inspect_repository(&project_id).await {
        Ok(repository) => Ok(Json(git_config_json(true, Some(&repository)))),
        Err(_) => Ok(Json(git_config_json(true, None))),
    }
}

/// 显式启用或关闭项目 Git 控制面；启用前必须确认项目路径就是现有 Git 仓库根目录。
async fn update_project_git(
    State(state): State<AppState>,
    Path(project_id): Path<String>,
    Json(body): Json<UpdateProjectGitRequest>,
) -> Result<Json<Value>, ApiError> {
    // 先确认项目存在，避免为不存在的项目创建孤立配置。
    let client = connect(&state).await?;
    let exists = client
        .query_opt("SELECT 1 FROM projects WHERE id = $1", &[&project_id])
        .await
        .map_err(ApiError::database)?
        .is_some();
    if !exists {
        return Err(ApiError::not_found("project not found"));
    }

    // 默认关闭或主动关闭不调用 Git；开启时必须验证容器内的绑定目录。
    let repository = if body.enabled { Some(inspect_repository(&project_id).await?) } else { None };

    // 配置以项目为唯一键保存，重复开关操作保持幂等。
    client
        .execute(
            "INSERT INTO project_git_settings (project_id, enabled) VALUES ($1, $2) \
             ON CONFLICT (project_id) DO UPDATE SET enabled = EXCLUDED.enabled, updated_at = now()",
            &[&project_id, &body.enabled],
        )
        .await
        .map_err(ApiError::database)?;

    Ok(Json(git_config_json(body.enabled, repository.as_ref())))
}

/// 列出项目的 Git worktree 会话，不创建或调整任何本地 Git 状态。
async fn list_worktree_sessions(
    State(state): State<AppState>,
    Path(project_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    // 使用项目存在性检查保持空会话列表和不存在项目两种状态可区分。
    let client = connect(&state).await?;
    project_git_enabled(&client, &project_id).await?;

    // 会话按创建时间倒序返回，任务关联使用 nullable 字段表达项目级会话。
    let rows = client
        .query(
            "SELECT id, project_id, task_id, worktree_path, status, \
             to_char(created_at, 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"'), \
             to_char(cleaned_at, 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"') \
             FROM git_worktree_sessions WHERE project_id = $1 ORDER BY created_at DESC",
            &[&project_id],
        )
        .await
        .map_err(ApiError::database)?;

    Ok(Json(json!({ "items": rows.iter().map(worktree_session_json).collect::<Vec<_>>() })))
}

/// 为显式启用且可用的项目仓库创建 detached HEAD worktree，并可绑定任务。
async fn create_worktree_session(
    State(state): State<AppState>,
    Path(project_id): Path<String>,
    Json(body): Json<CreateWorktreeSessionRequest>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    // 数据库开关是 Git 命令的第一道门槛，未启用项目不得进入仓库探测。
    let mut client = connect(&state).await?;
    if !project_git_enabled(&client, &project_id).await? {
        return Err(ApiError::conflict("git workspace is disabled for project"));
    }

    // 任务存在时必须属于同一项目，避免一个 worktree 被跨项目错误引用。
    if let Some(task_id) = body.task_id.as_deref() {
        let task_project_id = client
            .query_opt("SELECT project_id FROM tasks WHERE id = $1", &[&task_id])
            .await
            .map_err(ApiError::database)?
            .ok_or_else(|| ApiError::not_found("task not found"))?
            .get::<_, String>(0);
        if task_project_id != project_id {
            return Err(ApiError::invalid("task does not belong to project"));
        }
    }

    // 仅在已经通过项目目录边界校验的仓库内创建受管会话。
    let repository = inspect_repository(&project_id).await?;
    let session_id = Uuid::new_v4().to_string();
    let worktree_path = managed_worktree_path(&repository.repository_path, &session_id);
    let worktree_parent = worktree_path
        .parent()
        .ok_or_else(|| ApiError::invalid("managed worktree path has no parent"))?
        .to_path_buf();
    tokio::fs::create_dir_all(&worktree_parent)
        .await
        .map_err(|_| ApiError::invalid("cannot create managed worktree directory"))?;
    let canonical_parent = tokio::fs::canonicalize(&worktree_parent)
        .await
        .map_err(|_| ApiError::invalid("cannot resolve managed worktree directory"))?;
    if !canonical_parent.starts_with(&repository.repository_path) {
        return Err(ApiError::invalid("managed worktree directory escapes repository"));
    }

    // detached HEAD 固定会话基线，且不创建分支、提交、合并或推送。
    let worktree_path_text = worktree_path.to_string_lossy().to_string();
    let add_result = run_git(
        &repository.repository_path,
        &["worktree", "add", "--detach", worktree_path_text.as_str(), repository.current_head.as_str()],
    )
    .await?;
    if !add_result.success {
        return Err(git_command_error("cannot create git worktree", &add_result));
    }

    // 创建成功后再次解析路径，拒绝符号链接或 Git 输出导致的边界逃逸。
    let canonical_worktree = tokio::fs::canonicalize(&worktree_path)
        .await
        .map_err(|_| ApiError::conflict("git worktree was not created"))?;
    if !canonical_worktree.starts_with(&canonical_parent) || canonical_worktree == repository.repository_path {
        return Err(ApiError::conflict("git worktree path validation failed"));
    }

    // Git 副作用完成后以事务记录会话和任务绑定，保证任务只指向已存在 worktree。
    let transaction = client.transaction().await.map_err(ApiError::database)?;
    transaction
        .execute(
            "INSERT INTO git_worktree_sessions (id, project_id, task_id, worktree_path, status) VALUES ($1, $2, $3, $4, 'active')",
            &[&session_id, &project_id, &body.task_id, &worktree_path_text],
        )
        .await
        .map_err(ApiError::database)?;
    if let Some(task_id) = body.task_id.as_deref() {
        transaction
            .execute("UPDATE tasks SET workspace_session_id = $2, updated_at = now() WHERE id = $1", &[&task_id, &session_id])
            .await
            .map_err(ApiError::database)?;
    }
    transaction.commit().await.map_err(ApiError::database)?;

    Ok((
        StatusCode::CREATED,
        Json(json!({
            "id": session_id,
            "project_id": project_id,
            "task_id": body.task_id,
            "worktree_path": worktree_path_text,
            "status": "active",
            "current_head": repository.current_head,
        })),
    ))
}

/// 显式清理受管 worktree，并同步解除所有引用该会话的任务绑定。
async fn delete_worktree_session(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> Result<StatusCode, ApiError> {
    // 会话记录决定清理目标，调用方无法直接传入文件系统路径。
    let mut client = connect(&state).await?;
    let row = client
        .query_opt(
            "SELECT project_id, worktree_path, status FROM git_worktree_sessions WHERE id = $1",
            &[&session_id],
        )
        .await
        .map_err(ApiError::database)?
        .ok_or_else(|| ApiError::not_found("git worktree session not found"))?;
    let project_id = row.get::<_, String>(0);
    let recorded_path = row.get::<_, String>(1);
    let status = row.get::<_, String>(2);
    if status == "cleaned" {
        return Ok(StatusCode::NO_CONTENT);
    }

    // 根据项目根和会话标识重建唯一允许删除的路径，严禁删除主仓库或任意外部目录。
    let repository = inspect_repository(&project_id).await?;
    let expected_path = managed_worktree_path(&repository.repository_path, &session_id);
    let expected_path_text = expected_path.to_string_lossy().to_string();
    if recorded_path != expected_path_text {
        return Err(ApiError::conflict("stored git worktree path is invalid"));
    }

    // 仅调用 Git 的受管 remove 子命令，不使用文件系统递归删除。
    let remove_result = run_git(
        &repository.repository_path,
        &["worktree", "remove", "--force", expected_path_text.as_str()],
    )
    .await?;
    if !remove_result.success {
        return Err(git_command_error("cannot remove git worktree", &remove_result));
    }

    // 成功清理后解除任务引用并保留会话历史，避免丢失已执行任务的审计线索。
    let transaction = client.transaction().await.map_err(ApiError::database)?;
    transaction
        .execute("UPDATE tasks SET workspace_session_id = NULL, updated_at = now() WHERE workspace_session_id = $1", &[&session_id])
        .await
        .map_err(ApiError::database)?;
    transaction
        .execute(
            "UPDATE git_worktree_sessions SET status = 'cleaned', cleaned_at = now() WHERE id = $1",
            &[&session_id],
        )
        .await
        .map_err(ApiError::database)?;
    transaction.commit().await.map_err(ApiError::database)?;

    Ok(StatusCode::NO_CONTENT)
}

/// 查询项目开关，同时将不存在的项目转换为统一的 404 业务错误。
async fn project_git_enabled(client: &tokio_postgres::Client, project_id: &str) -> Result<bool, ApiError> {
    // 左连接保证尚未保存设置的项目仍以默认关闭状态返回。
    let row = client
        .query_opt(
            "SELECT COALESCE(settings.enabled, FALSE) FROM projects project \
             LEFT JOIN project_git_settings settings ON settings.project_id = project.id WHERE project.id = $1",
            &[&project_id],
        )
        .await
        .map_err(ApiError::database)?
        .ok_or_else(|| ApiError::not_found("project not found"))?;
    Ok(row.get::<_, bool>(0))
}

/// 生成 GET 与 PATCH 共用的项目 Git 配置快照，保证前端无需为开关结果分支解析字段。
fn git_config_json(enabled: bool, repository: Option<&RepositoryProbe>) -> Value {
    // 关闭或探测失败时只返回不可用状态，避免伪造仓库路径、提交或工作区状态。
    let Some(repository) = repository else {
        return json!({
            "enabled": enabled,
            "repository_available": false,
            "repository_path": Value::Null,
            "status_summary": Value::Null,
            "current_head": Value::Null,
        });
    };

    // 已验证仓库只输出规范化路径、只读状态摘要和当前提交，不包含变更文件明细。
    json!({
        "enabled": enabled,
        "repository_available": true,
        "repository_path": repository.repository_path,
        "status_summary": repository.status_summary,
        "current_head": repository.current_head,
    })
}

/// 检查项目 UUID、挂载目录边界和 Git 顶层目录，返回可安全使用的仓库信息。
async fn inspect_repository(project_id: &str) -> Result<RepositoryProbe, ApiError> {
    // 项目标识必须为 UUID，避免路径片段、符号和父级跳转进入文件系统。
    let project_uuid = Uuid::parse_str(project_id).map_err(|_| ApiError::invalid("project_id must be a UUID"))?;
    let projects_root = tokio::fs::canonicalize(PROJECTS_ROOT)
        .await
        .map_err(|_| ApiError::invalid("project workspace root is unavailable"))?;
    let requested_path = projects_root.join(project_uuid.to_string());
    let repository_path = tokio::fs::canonicalize(&requested_path)
        .await
        .map_err(|_| ApiError::invalid("project repository directory is unavailable"))?;
    if !repository_path.starts_with(&projects_root) || !repository_path.is_dir() {
        return Err(ApiError::invalid("project repository directory is outside workspace root"));
    }

    // Git 顶层必须与项目目录完全一致，防止对子目录所属的公司主仓库误建 worktree。
    let root_result = run_git(&repository_path, &["rev-parse", "--show-toplevel"]).await?;
    if !root_result.success {
        return Err(ApiError::invalid("project directory is not a git repository"));
    }
    let reported_root = root_result.stdout.trim();
    let git_root = tokio::fs::canonicalize(reported_root)
        .await
        .map_err(|_| ApiError::invalid("cannot resolve git repository root"))?;
    if git_root != repository_path {
        return Err(ApiError::invalid("project directory must be the git repository root"));
    }

    // 读取 HEAD 与 status 只用于展示和创建 detached worktree 的基线，不修改仓库内容。
    let head_result = run_git(&repository_path, &["rev-parse", "--verify", "HEAD"]).await?;
    if !head_result.success || head_result.stdout.trim().is_empty() {
        return Err(ApiError::conflict("git repository has no commit to create a worktree from"));
    }
    let status_result = run_git(&repository_path, &["status", "--porcelain=v1", "--branch"]).await?;
    if !status_result.success {
        return Err(git_command_error("cannot read git status", &status_result));
    }

    Ok(RepositoryProbe {
        repository_path,
        current_head: head_result.stdout.trim().to_owned(),
        status_summary: status_summary(&status_result.stdout),
    })
}

/// 生成会话唯一的受管路径，路径只可能位于项目仓库的 `.xiexu-worktrees` 下。
fn managed_worktree_path(repository_path: &FilePath, session_id: &str) -> PathBuf {
    repository_path.join(WORKTREE_DIRECTORY).join(session_id)
}

/// 在受控超时内执行 Git 参数数组，不经过 shell，也不允许调用方指定可执行文件。
async fn run_git(repository_path: &FilePath, arguments: &[&str]) -> Result<GitCommandResult, ApiError> {
    // 所有 Git 调用固定使用当前项目仓库作为工作目录，避免外部环境变量改变目标位置。
    let output = tokio::time::timeout(
        GIT_COMMAND_TIMEOUT,
        Command::new("git").args(arguments).current_dir(repository_path).kill_on_drop(true).output(),
    )
    .await
    .map_err(|_| ApiError::conflict("git command timed out"))?
    .map_err(|_| ApiError::conflict("git executable is unavailable"))?;

    Ok(GitCommandResult {
        success: output.status.success(),
        stdout: String::from_utf8_lossy(&output.stdout).trim().to_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
    })
}

/// 将 Git 输出转换为前端可扫描的摘要，避免直接暴露完整工作区文件列表。
fn status_summary(status_output: &str) -> String {
    // porcelain 的第一行携带分支或 detached HEAD 信息，其余每行代表一项工作区变更。
    let mut lines = status_output.lines();
    let head = lines.next().unwrap_or("## unknown").trim_start_matches("## ").trim();
    let change_count = lines.filter(|line| !line.trim().is_empty()).count();
    format!("{head}; {change_count} changed paths")
}

/// 将失败 Git 子进程转换为不泄露长输出的冲突响应。
fn git_command_error(context: &str, result: &GitCommandResult) -> ApiError {
    // stderr 优先保留诊断信息，缺省时使用 stdout，并统一限制返回长度。
    let detail = if result.stderr.is_empty() { result.stdout.as_str() } else { result.stderr.as_str() };
    let detail = detail.chars().take(300).collect::<String>();
    if detail.is_empty() {
        ApiError::conflict(context)
    } else {
        ApiError::conflict(format!("{context}: {detail}"))
    }
}

/// 将数据库会话行映射为稳定的 API 响应结构。
fn worktree_session_json(row: &Row) -> Value {
    // 数据库时间统一转换为 ISO 8601 文本，nullable 任务和清理时间保留 JSON null。
    json!({
        "id": row.get::<_, String>(0),
        "project_id": row.get::<_, String>(1),
        "task_id": row.get::<_, Option<String>>(2),
        "worktree_path": row.get::<_, String>(3),
        "status": row.get::<_, String>(4),
        "created_at": row.get::<_, String>(5),
        "cleaned_at": row.get::<_, Option<String>>(6),
    })
}

#[cfg(test)]
mod tests {
    use super::{managed_worktree_path, status_summary};
    use std::path::Path;

    /// 验证受管路径始终追加到项目根目录内的固定会话目录。
    #[test]
    fn managed_path_is_nested_under_repository() {
        // 路径构造不接受外部目录，仅组合固定目录和会话标识。
        assert_eq!(
            managed_worktree_path(Path::new("/workspace/projects/project"), "session"),
            Path::new("/workspace/projects/project/.xiexu-worktrees/session")
        );
    }

    /// 验证状态摘要不会包含具体的改动文件名。
    #[test]
    fn status_summary_only_reports_branch_and_count() {
        // porcelain 输入的文件列表只参与计数，返回值不泄露文件路径。
        assert_eq!(status_summary("## HEAD (no branch)\n M secret.txt\n?? local.env"), "HEAD (no branch); 2 changed paths");
    }
}
