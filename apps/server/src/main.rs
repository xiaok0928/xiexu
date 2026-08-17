mod collaboration;

use axum::{extract::{Path, Query, State}, http::StatusCode, response::IntoResponse, routing::{get, post}, Json, Router};
use serde_json::{json, Value};
use std::{collections::HashMap, env, sync::Arc, time::Duration};
use tokio::process::Command;
use tokio_postgres::{Client, NoTls, Row};
use tower_http::services::{ServeDir, ServeFile};
use uuid::Uuid;

/// 服务端共享配置，统一承载数据库地址和 Codex 运行状态探测边界。
#[derive(Clone)]
pub(crate) struct AppState {
    /// PostgreSQL 连接地址，只在服务端内部使用。
    pub(crate) database_url: Arc<String>,
    /// Codex CLI 可执行文件路径。
    codex_bin: Arc<String>,
    /// Runner 使用的 Codex 执行模式。
    codex_mode: Arc<String>,
}

/// 健康检查响应，供容器编排判断进程是否存活。
#[derive(serde::Serialize)]
struct HealthResponse { status: &'static str }

/// 就绪检查响应，明确数据库和迁移状态。
#[derive(serde::Serialize)]
struct ReadyResponse { status: &'static str, database: &'static str, migration: &'static str }

/// Codex 运行时状态响应，只暴露安装、版本、模式和认证布尔值。
#[derive(serde::Serialize)]
struct CodexRuntimeResponse {
    /// 容器中是否可以执行 Codex CLI。
    installed: bool,
    /// 已安装的 CLI 版本；探测失败时为空。
    version: Option<String>,
    /// 当前执行模式，默认为 controlled。
    mode: String,
    /// 当前 CODEX_HOME 是否已完成认证，不返回账号或令牌内容。
    authenticated: bool,
}

/// 进程入口：组装 M0 健康检查、M1 领域 API、M2 执行查询和静态 Web 路由。
#[tokio::main]
async fn main() {
    // 读取运行时配置，保持服务监听地址和数据库依赖可由 Compose 注入。
    let database_url = env::var("DATABASE_URL").expect("DATABASE_URL must be configured");
    let bind_addr = env::var("SERVER_BIND").unwrap_or_else(|_| "0.0.0.0:8080".to_owned());
    let codex_bin = env::var("CODEX_BIN").unwrap_or_else(|_| "/usr/local/bin/codex".to_owned());
    let codex_mode = env::var("CODEX_EXECUTION_MODE").unwrap_or_else(|_| "controlled".to_owned());
    let state = AppState { database_url: Arc::new(database_url), codex_bin: Arc::new(codex_bin), codex_mode: Arc::new(codex_mode) };
    // 组装 API 路由和静态 Web 回退，深链访问始终回到前端入口。
    let app = Router::new()
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .route("/api/runtime/codex", get(codex_runtime))
        .route("/api/projects", get(list_projects).post(create_project))
        .route("/api/projects/:project_id", get(get_project).patch(update_project))
        .route("/api/projects/:project_id/tasks", get(list_tasks).post(create_task))
        .route("/api/tasks/:task_id", get(get_task).patch(update_task))
        .route("/api/tasks/:task_id/transitions", post(transition_task))
        .route("/api/tasks/:task_id/comments", get(list_comments).post(create_comment))
        .route("/api/tasks/:task_id/events", get(list_events))
        .route("/api/tasks/:task_id/execution", get(list_execution))
        .merge(collaboration::routes())
        .nest_service("/", ServeDir::new("/app/web").append_index_html_on_directories(true).fallback(ServeFile::new("/app/web/index.html")))
        .with_state(state);
    // 绑定监听端口并启动 HTTP 服务，启动失败直接让容器退出以便编排发现。
    let listener = tokio::net::TcpListener::bind(&bind_addr).await.expect("bind server");
    println!("xiexu server listening on {bind_addr}");
    axum::serve(listener, app).await.expect("serve server");
}

/// 返回进程存活状态，不访问数据库，区分进程问题与依赖问题。
async fn healthz() -> impl IntoResponse { Json(HealthResponse { status: "ok" }) }

/// 检查数据库连接和迁移表，只有迁移就绪后才返回成功。
async fn readyz(State(state): State<AppState>) -> impl IntoResponse {
    let connection = tokio_postgres::connect(state.database_url.as_str(), NoTls).await;
    let (client, connection_task) = match connection {
        Ok(value) => value,
        Err(error) => {
            eprintln!("database readiness check failed: {error}");
            return (StatusCode::SERVICE_UNAVAILABLE, Json(ReadyResponse { status: "not_ready", database: "down", migration: "unknown" }));
        }
    };
    tokio::spawn(async move { if let Err(error) = connection_task.await { eprintln!("database readiness connection ended: {error}"); } });
    let migration_exists = client.query_one("SELECT EXISTS (SELECT 1 FROM information_schema.tables WHERE table_schema = 'public' AND table_name = 'schema_migrations')", &[]).await.ok().and_then(|row| row.try_get::<_, bool>(0).ok()).unwrap_or(false);
    if migration_exists { (StatusCode::OK, Json(ReadyResponse { status: "ready", database: "up", migration: "ready" })) } else { (StatusCode::SERVICE_UNAVAILABLE, Json(ReadyResponse { status: "not_ready", database: "up", migration: "pending" })) }
}

/// 探测容器内 Codex CLI 和认证状态，不把命令输出中的账户信息返回给调用方。
async fn codex_runtime(State(state): State<AppState>) -> Json<CodexRuntimeResponse> {
    // 版本探测限定为五秒，CLI 异常不能阻塞服务线程。
    let version_output = tokio::time::timeout(Duration::from_secs(5), Command::new(state.codex_bin.as_str()).arg("--version").output()).await;
    let version = version_output.ok().and_then(Result::ok).filter(|output| output.status.success()).and_then(|output| String::from_utf8(output.stdout).ok()).map(|value| value.trim().chars().take(200).collect::<String>()).filter(|value| !value.is_empty());
    let installed = version.is_some();

    // 登录探测只读取退出状态，避免响应或日志包含账户、令牌等认证细节。
    let authenticated = if installed {
        tokio::time::timeout(Duration::from_secs(5), Command::new(state.codex_bin.as_str()).args(["login", "status"]).output()).await.ok().and_then(Result::ok).is_some_and(|output| output.status.success())
    } else {
        false
    };
    Json(CodexRuntimeResponse { installed, version, mode: state.codex_mode.as_str().to_owned(), authenticated })
}

/// 统一 API 错误，避免把数据库内部错误直接泄漏给前端。
pub(crate) struct ApiError(StatusCode, String);

impl ApiError {
    /// 构造数据库故障响应。
    pub(crate) fn database(error: tokio_postgres::Error) -> Self { eprintln!("database error: {error}"); Self(StatusCode::INTERNAL_SERVER_ERROR, "database error".to_owned()) }
    /// 构造资源不存在响应。
    pub(crate) fn not_found(message: impl Into<String>) -> Self { Self(StatusCode::NOT_FOUND, message.into()) }
    /// 构造请求校验失败响应。
    pub(crate) fn invalid(message: impl Into<String>) -> Self { Self(StatusCode::UNPROCESSABLE_ENTITY, message.into()) }
    /// 构造状态冲突响应。
    pub(crate) fn conflict(message: impl Into<String>) -> Self { Self(StatusCode::CONFLICT, message.into()) }
}

impl IntoResponse for ApiError {
    /// 将内部错误转换为稳定 JSON 错误结构。
    fn into_response(self) -> axum::response::Response { (self.0, Json(json!({ "error": self.1 }))).into_response() }
}

/// 建立一个短生命周期数据库连接，事务边界在处理函数内显式可见。
pub(crate) async fn connect(state: &AppState) -> Result<Client, ApiError> {
    let (client, connection) = tokio_postgres::connect(state.database_url.as_str(), NoTls).await.map_err(ApiError::database)?;
    tokio::spawn(async move { if let Err(error) = connection.await { eprintln!("database connection ended: {error}"); } });
    Ok(client)
}

/// 读取 JSON 字段并执行非空校验。
pub(crate) fn required_text(body: &Value, key: &str) -> Result<String, ApiError> {
    body.get(key).and_then(Value::as_str).map(str::trim).filter(|value| !value.is_empty()).map(ToOwned::to_owned).ok_or_else(|| ApiError::invalid(format!("{key} is required")))
}

/// 将数据库行映射为项目响应。
fn project_json(row: &Row) -> Value { json!({ "id": row.get::<_, String>(0), "name": row.get::<_, String>(1), "description": row.get::<_, String>(2), "status": row.get::<_, String>(3), "created_at": row.get::<_, String>(4), "updated_at": row.get::<_, String>(5) }) }

/// 将数据库行映射为任务卡片响应，子任务数量由数据库事实源计算。
fn task_json(row: &Row) -> Value {
    json!({ "id": row.get::<_, String>(0), "project_id": row.get::<_, String>(1), "parent_task_id": row.get::<_, Option<String>>(2), "title": row.get::<_, String>(3), "description": row.get::<_, String>(4), "board_stage": row.get::<_, String>(5), "plan_status": row.get::<_, String>(6), "execution_status": row.get::<_, String>(7), "acceptance_status": row.get::<_, String>(8), "progress_percent": row.get::<_, i16>(9), "requires_plan_confirmation": row.get::<_, bool>(10), "children_count": row.get::<_, i64>(11), "revision": row.get::<_, i64>(12), "created_at": row.get::<_, String>(13), "updated_at": row.get::<_, String>(14) })
}

/// 查询项目列表，默认按最近更新时间倒序返回。
async fn list_projects(State(state): State<AppState>) -> Result<Json<Value>, ApiError> {
    let client = connect(&state).await?;
    let rows = client.query("SELECT id, name, description, status, to_char(created_at, 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"'), to_char(updated_at, 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"') FROM projects WHERE status = 'active' ORDER BY updated_at DESC", &[]).await.map_err(ApiError::database)?;
    Ok(Json(json!({ "items": rows.iter().map(project_json).collect::<Vec<_>>(), "next_cursor": null })))
}

/// 创建项目，并在同一事务初始化项目文档、协调 Agent 和长期主群聊。
async fn create_project(State(state): State<AppState>, Json(body): Json<Value>) -> Result<(StatusCode, Json<Value>), ApiError> {
    // 校验用户输入并准备共享主键，所有初始化事实必须同时成功或同时回滚。
    let name = required_text(&body, "name")?;
    if name.len() > 200 { return Err(ApiError::invalid("name is too long")); }
    let description = body.get("description").and_then(Value::as_str).unwrap_or("");
    let id = Uuid::new_v4().to_string();
    let coordinator_id = Uuid::new_v4().to_string();
    let conversation_id = Uuid::new_v4().to_string();
    let mut client = connect(&state).await?;
    let transaction = client.transaction().await.map_err(ApiError::database)?;

    // 建立项目和项目概览事实，新建项目不会依赖异步任务才能进入可用状态。
    let row = transaction.query_one("INSERT INTO projects (id, name, description) VALUES ($1, $2, $3) RETURNING id, name, description, status, to_char(created_at, 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"'), to_char(updated_at, 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"')", &[&id, &name, &description]).await.map_err(ApiError::database)?;
    transaction.execute("INSERT INTO project_documents (id, project_id, doc_type, title) VALUES ($1, $2, 'overview', '项目概览')", &[&Uuid::new_v4().to_string(), &id]).await.map_err(ApiError::database)?;

    // 为项目创建独立协调 Agent，使其职责补充和私有记忆不会与其他项目混用。
    let coordinator_name = format!("{name} 协调 Agent");
    transaction.execute("INSERT INTO agents (id, template_code, name, description, instructions, created_by) VALUES ($1, 'project_manager', $2, '负责该项目的任务拆分、指派、依赖协调和结果汇总。', '以项目目标为边界协调固定与动态 Agent；Human 负责确认关键方案和验收最终结果。', 'system')", &[&coordinator_id, &coordinator_name]).await.map_err(ApiError::database)?;
    transaction.execute("INSERT INTO project_agents (project_id, agent_id, assignment_type) VALUES ($1, $2, 'coordinator')", &[&id, &coordinator_id]).await.map_err(ApiError::database)?;

    // 创建长期项目主群聊并加入 Human 与协调 Agent，后续固定 Agent 加入项目时会同步加入该群聊。
    let conversation_title = format!("{name} 项目群");
    transaction.execute("INSERT INTO conversations (id, conversation_type, project_id, title, created_by) VALUES ($1, 'project_main', $2, $3, 'system')", &[&conversation_id, &id, &conversation_title]).await.map_err(ApiError::database)?;
    transaction.execute("INSERT INTO conversation_participants (conversation_id, actor_type, actor_id) VALUES ($1, 'human', 'human'), ($1, 'agent', $2)", &[&conversation_id, &coordinator_id]).await.map_err(ApiError::database)?;

    // 提交完整初始化事务后再向调用方暴露项目。
    transaction.commit().await.map_err(ApiError::database)?;
    Ok((StatusCode::CREATED, Json(project_json(&row))))
}

/// 查询单个项目，归档项目仍允许通过明确 ID 查看历史事实。
async fn get_project(State(state): State<AppState>, Path(project_id): Path<String>) -> Result<Json<Value>, ApiError> {
    let client = connect(&state).await?;
    let row = client.query_opt("SELECT id, name, description, status, to_char(created_at, 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"'), to_char(updated_at, 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"') FROM projects WHERE id = $1", &[&project_id]).await.map_err(ApiError::database)?.ok_or_else(|| ApiError::not_found("project not found"))?;
    Ok(Json(project_json(&row)))
}

/// 更新项目名称和说明，不提供物理删除接口。
async fn update_project(State(state): State<AppState>, Path(project_id): Path<String>, Json(body): Json<Value>) -> Result<Json<Value>, ApiError> {
    let name = body.get("name").and_then(Value::as_str).map(str::trim);
    let description = body.get("description").and_then(Value::as_str);
    if name.is_none() && description.is_none() { return Err(ApiError::invalid("name or description is required")); }
    let client = connect(&state).await?;
    let row = client.query_opt("UPDATE projects SET name = COALESCE($2, name), description = COALESCE($3, description), updated_at = now() WHERE id = $1 RETURNING id, name, description, status, to_char(created_at, 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"'), to_char(updated_at, 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"')", &[&project_id, &name, &description]).await.map_err(ApiError::database)?.ok_or_else(|| ApiError::not_found("project not found"))?;
    Ok(Json(project_json(&row)))
}

/// 查询项目任务，支持按看板阶段和父任务过滤。
async fn list_tasks(State(state): State<AppState>, Path(project_id): Path<String>, Query(query): Query<HashMap<String, String>>) -> Result<Json<Value>, ApiError> {
    let client = connect(&state).await?;
    let stage = query.get("board_stage");
    let parent_id = query.get("parent_id");
    let rows = client.query("SELECT t.id, t.project_id, t.parent_task_id, t.title, t.description, t.board_stage, t.plan_status, t.execution_status, t.acceptance_status, t.progress_percent, t.requires_plan_confirmation, (SELECT count(*) FROM tasks child WHERE child.parent_task_id = t.id), t.revision, to_char(t.created_at, 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"'), to_char(t.updated_at, 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"') FROM tasks t WHERE t.project_id = $1 AND ($2::text IS NULL OR t.board_stage = $2) AND ($3::text IS NULL OR t.parent_task_id = $3) ORDER BY t.created_at ASC", &[&project_id, &stage, &parent_id]).await.map_err(ApiError::database)?;
    Ok(Json(json!({ "items": rows.iter().map(task_json).collect::<Vec<_>>(), "next_cursor": null })))
}

/// 创建任务，M1 新建任务统一从 Backlog 开始，避免伪造执行。
async fn create_task(State(state): State<AppState>, Path(project_id): Path<String>, Json(body): Json<Value>) -> Result<(StatusCode, Json<Value>), ApiError> {
    let title = required_text(&body, "title")?;
    if title.len() > 500 { return Err(ApiError::invalid("title is too long")); }
    let description = body.get("description").and_then(Value::as_str).unwrap_or("");
    let parent_id = body.get("parent_task_id").and_then(Value::as_str);
    let requires = body.get("requires_plan_confirmation").and_then(Value::as_bool).unwrap_or(true);
    let mut client = connect(&state).await?;
    let transaction = client.transaction().await.map_err(ApiError::database)?;
    if transaction.query_opt("SELECT 1 FROM projects WHERE id = $1 AND status = 'active'", &[&project_id]).await.map_err(ApiError::database)?.is_none() { return Err(ApiError::not_found("project not found")); }
    if let Some(parent) = parent_id { if transaction.query_opt("SELECT 1 FROM tasks WHERE id = $1 AND project_id = $2", &[&parent, &project_id]).await.map_err(ApiError::database)?.is_none() { return Err(ApiError::invalid("parent task must belong to the same project")); } }
    let id = Uuid::new_v4().to_string();
    let row = transaction.query_one("INSERT INTO tasks (id, project_id, parent_task_id, title, description, board_stage, plan_status, execution_status, requires_plan_confirmation) VALUES ($1, $2, $3, $4, $5, 'backlog', CASE WHEN $6 THEN 'pending_generation' ELSE 'not_required' END, 'idle', $6) RETURNING id, project_id, parent_task_id, title, description, board_stage, plan_status, execution_status, acceptance_status, progress_percent, requires_plan_confirmation, (SELECT count(*) FROM tasks child WHERE child.parent_task_id = tasks.id), revision, to_char(created_at, 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"'), to_char(updated_at, 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"')", &[&id, &project_id, &parent_id, &title, &description, &requires]).await.map_err(ApiError::database)?;
    transaction.execute("INSERT INTO task_events (task_id, event_type, actor_type, actor_id, after_data) VALUES ($1, 'task.created', 'human', 'human', $2)", &[&id, &json!({ "board_stage": "backlog" })]).await.map_err(ApiError::database)?;
    transaction.commit().await.map_err(ApiError::database)?;
    Ok((StatusCode::CREATED, Json(task_json(&row))))
}

/// 查询单个任务及其当前聚合字段。
async fn get_task(State(state): State<AppState>, Path(task_id): Path<String>) -> Result<Json<Value>, ApiError> {
    let client = connect(&state).await?;
    let row = client.query_opt("SELECT t.id, t.project_id, t.parent_task_id, t.title, t.description, t.board_stage, t.plan_status, t.execution_status, t.acceptance_status, t.progress_percent, t.requires_plan_confirmation, (SELECT count(*) FROM tasks child WHERE child.parent_task_id = t.id), t.revision, to_char(t.created_at, 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"'), to_char(t.updated_at, 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"') FROM tasks t WHERE t.id = $1", &[&task_id]).await.map_err(ApiError::database)?.ok_or_else(|| ApiError::not_found("task not found"))?;
    Ok(Json(task_json(&row)))
}

/// 更新任务文本、方案确认开关、进度和父任务，不直接接受任意看板阶段。
async fn update_task(State(state): State<AppState>, Path(task_id): Path<String>, Json(body): Json<Value>) -> Result<Json<Value>, ApiError> {
    let title = body.get("title").and_then(Value::as_str).map(str::trim);
    let description = body.get("description").and_then(Value::as_str);
    let requires = body.get("requires_plan_confirmation").and_then(Value::as_bool);
    let progress = body.get("progress_percent").and_then(Value::as_i64);
    if title.is_none() && description.is_none() && requires.is_none() && progress.is_none() { return Err(ApiError::invalid("no mutable fields")); }
    if title.is_some_and(str::is_empty) { return Err(ApiError::invalid("title cannot be empty")); }
    if progress.is_some_and(|value| !(0..=100).contains(&value)) { return Err(ApiError::invalid("progress_percent must be between 0 and 100")); }
    let mut client = connect(&state).await?;
    let transaction = client.transaction().await.map_err(ApiError::database)?;
    let row = transaction.query_opt("UPDATE tasks SET title = COALESCE($2, title), description = COALESCE($3, description), requires_plan_confirmation = COALESCE($4, requires_plan_confirmation), progress_percent = COALESCE($5, progress_percent), revision = revision + 1, updated_at = now() WHERE id = $1 RETURNING id, project_id, parent_task_id, title, description, board_stage, plan_status, execution_status, acceptance_status, progress_percent, requires_plan_confirmation, (SELECT count(*) FROM tasks WHERE parent_task_id = tasks.id), revision, to_char(created_at, 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"'), to_char(updated_at, 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"')", &[&task_id, &title, &description, &requires, &progress]).await.map_err(ApiError::database)?.ok_or_else(|| ApiError::not_found("task not found"))?;
    transaction.execute("INSERT INTO task_events (task_id, event_type, actor_type, actor_id, event_data) VALUES ($1, 'task.updated', 'human', 'human', $2)", &[&task_id, &json!({ "updated": true })]).await.map_err(ApiError::database)?;
    transaction.commit().await.map_err(ApiError::database)?;
    Ok(Json(task_json(&row)))
}

/// 执行显式看板阶段转换，状态机校验由服务端负责；进入处理中时同事务创建执行作业。
async fn transition_task(State(state): State<AppState>, Path(task_id): Path<String>, Json(body): Json<Value>) -> Result<Json<Value>, ApiError> {
    let target = required_text(&body, "target_stage")?;
    let target_stage = xiexu_domain::BoardStage::parse(&target).ok_or_else(|| ApiError::invalid("invalid target_stage"))?;
    let reason = body.get("reason").and_then(Value::as_str).unwrap_or("");
    let mut client = connect(&state).await?;
    let transaction = client.transaction().await.map_err(ApiError::database)?;
    let current = transaction.query_opt("SELECT board_stage, requires_plan_confirmation, revision FROM tasks WHERE id = $1 FOR UPDATE", &[&task_id]).await.map_err(ApiError::database)?.ok_or_else(|| ApiError::not_found("task not found"))?;
    let from = current.get::<_, String>(0);
    let from_stage = xiexu_domain::BoardStage::parse(&from).ok_or_else(|| ApiError::invalid("stored task stage is invalid"))?;
    if !xiexu_domain::is_valid_transition(from_stage, target_stage) { return Err(ApiError::conflict(format!("invalid transition: {from} -> {target}"))); }
    let requires = current.get::<_, bool>(1);
    let plan_status = if target_stage == xiexu_domain::BoardStage::PlanReview { "reviewing" } else if target_stage == xiexu_domain::BoardStage::Todo && requires { "pending_generation" } else if target_stage == xiexu_domain::BoardStage::InProgress { "approved" } else { "not_required" };
    let execution_status = if target_stage == xiexu_domain::BoardStage::InProgress { "queued" } else { "idle" };
    transaction.execute("UPDATE tasks SET board_stage = $2, plan_status = $3, execution_status = $4, revision = revision + 1, updated_at = now() WHERE id = $1", &[&task_id, &target, &plan_status, &execution_status]).await.map_err(ApiError::database)?;
    transaction.execute("INSERT INTO task_transitions (id, task_id, from_stage, to_stage, reason) VALUES ($1, $2, $3, $4, $5)", &[&Uuid::new_v4().to_string(), &task_id, &from, &target, &reason]).await.map_err(ApiError::database)?;
    transaction.execute("INSERT INTO task_events (task_id, event_type, actor_type, actor_id, before_data, after_data, event_data) VALUES ($1, 'task.stage_changed', 'human', 'human', $2, $3, $4)", &[&task_id, &json!({ "board_stage": from }), &json!({ "board_stage": target }), &json!({ "reason": reason })]).await.map_err(ApiError::database)?;
    if target_stage == xiexu_domain::BoardStage::InProgress {
        // 使用阶段转换前的 revision 构造去重键，避免同一轮执行重复入队。
        let next_revision = current.get::<_, i64>(2) + 1;
        let dedupe_key = format!("task:{task_id}:execute:{next_revision}");
        transaction.execute("INSERT INTO execution_jobs (id, kind, status, task_id, payload, dedupe_key) VALUES ($1, 'execute_task', 'queued', $2, $3, $4) ON CONFLICT (dedupe_key) DO NOTHING", &[&Uuid::new_v4().to_string(), &task_id, &json!({ "task_id": task_id.clone(), "reason": reason, "revision": next_revision }), &dedupe_key]).await.map_err(ApiError::database)?;
        transaction.execute("INSERT INTO task_events (task_id, event_type, actor_type, actor_id, event_data) VALUES ($1, 'execution.job_queued', 'system', 'control-plane', $2)", &[&task_id, &json!({ "kind": "execute_task", "dedupe_key": dedupe_key })]).await.map_err(ApiError::database)?;
    }
    if target_stage == xiexu_domain::BoardStage::Done { transaction.execute("UPDATE tasks SET board_stage = 'done', acceptance_status = 'passed', progress_percent = 100, revision = revision + 1, updated_at = now() WHERE parent_task_id = $1 AND board_stage <> 'cancelled'", &[&task_id]).await.map_err(ApiError::database)?; }
    transaction.commit().await.map_err(ApiError::database)?;
    get_task(State(state), Path(task_id)).await
}

/// 查询任务评论，评论按追加时间顺序返回。
async fn list_comments(State(state): State<AppState>, Path(task_id): Path<String>) -> Result<Json<Value>, ApiError> {
    let client = connect(&state).await?;
    let rows = client.query("SELECT id, author_type, author_name, content, intent, transition_applied, to_char(created_at, 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"') FROM task_comments WHERE task_id = $1 ORDER BY created_at ASC", &[&task_id]).await.map_err(ApiError::database)?;
    Ok(Json(json!({ "items": rows.iter().map(|row| json!({ "id": row.get::<_, String>(0), "author_type": row.get::<_, String>(1), "author_name": row.get::<_, String>(2), "content": row.get::<_, String>(3), "intent": row.get::<_, String>(4), "transition_applied": row.get::<_, bool>(5), "created_at": row.get::<_, String>(6) })).collect::<Vec<_>>() })))
}

/// 保存评论事实，并将显式意图提示交给状态机尝试应用；自然语言识别仍留给后续 Agent。
async fn create_comment(State(state): State<AppState>, Path(task_id): Path<String>, Json(body): Json<Value>) -> Result<(StatusCode, Json<Value>), ApiError> {
    let content = required_text(&body, "content")?;
    let intent = body.get("intent").and_then(Value::as_str).unwrap_or("note");
    let allowed = ["note", "approve_plan", "reject_plan", "accept", "rework", "mention"];
    if !allowed.contains(&intent) { return Err(ApiError::invalid("unsupported intent hint")); }
    let mut client = connect(&state).await?;
    let transaction = client.transaction().await.map_err(ApiError::database)?;
    let current = transaction.query_opt("SELECT board_stage, revision FROM tasks WHERE id = $1 FOR UPDATE", &[&task_id]).await.map_err(ApiError::database)?.ok_or_else(|| ApiError::not_found("task not found"))?;
    let current_stage = current.get::<_, String>(0);
    let current_revision = current.get::<_, i64>(1);
    let mut applied = false;
    let comment_id = Uuid::new_v4().to_string();
    let current_stage_value = xiexu_domain::BoardStage::parse(&current_stage).ok_or_else(|| ApiError::invalid("stored task stage is invalid"))?;
    let mut next_stage = None;
    if intent == "approve_plan" && current_stage_value == xiexu_domain::BoardStage::PlanReview {
        next_stage = Some(xiexu_domain::BoardStage::InProgress);
    } else if intent == "accept" && current_stage_value == xiexu_domain::BoardStage::Acceptance {
        next_stage = Some(xiexu_domain::BoardStage::Done);
    } else if intent == "rework" && current_stage_value == xiexu_domain::BoardStage::Acceptance {
        next_stage = Some(xiexu_domain::BoardStage::InProgress);
    }
    if let Some(target_stage) = next_stage {
        let next_revision = current_revision + 1;
        let next_plan_status = if target_stage == xiexu_domain::BoardStage::InProgress { "approved" } else { "not_required" };
        let next_execution_status = if target_stage == xiexu_domain::BoardStage::InProgress { "queued" } else { "idle" };
        let next_acceptance_status = if target_stage == xiexu_domain::BoardStage::Done { "passed" } else { "not_started" };
        transaction.execute("UPDATE tasks SET board_stage = $2, plan_status = $3, execution_status = $4, acceptance_status = $5, progress_percent = CASE WHEN $2 = 'done' THEN 100 ELSE progress_percent END, revision = revision + 1, updated_at = now() WHERE id = $1", &[&task_id, &target_stage.as_str(), &next_plan_status, &next_execution_status, &next_acceptance_status]).await.map_err(ApiError::database)?;
        transaction.execute("INSERT INTO task_transitions (id, task_id, from_stage, to_stage, reason) VALUES ($1, $2, $3, $4, $5)", &[&Uuid::new_v4().to_string(), &task_id, &current_stage, &target_stage.as_str(), &format!("comment:{intent}")]).await.map_err(ApiError::database)?;
        transaction.execute("INSERT INTO task_events (task_id, event_type, actor_type, actor_id, before_data, after_data, event_data) VALUES ($1, 'task.stage_changed', 'human', 'human', $2, $3, $4)", &[&task_id, &json!({ "board_stage": current_stage.clone() }), &json!({ "board_stage": target_stage.as_str() }), &json!({ "reason": format!("comment:{intent}"), "comment_id": comment_id.clone() })]).await.map_err(ApiError::database)?;
        if target_stage == xiexu_domain::BoardStage::Done {
            transaction.execute("UPDATE tasks SET board_stage = 'done', acceptance_status = 'passed', progress_percent = 100, revision = revision + 1, updated_at = now() WHERE parent_task_id = $1 AND board_stage <> 'cancelled'", &[&task_id]).await.map_err(ApiError::database)?;
        } else {
            let dedupe_key = format!("task:{task_id}:execute:{next_revision}");
            transaction.execute("INSERT INTO execution_jobs (id, kind, status, task_id, payload, dedupe_key) VALUES ($1, 'execute_task', 'queued', $2, $3, $4) ON CONFLICT (dedupe_key) DO NOTHING", &[&Uuid::new_v4().to_string(), &task_id, &json!({ "task_id": task_id.clone(), "reason": format!("comment:{intent}"), "revision": next_revision, "comment_id": comment_id.clone() }), &dedupe_key]).await.map_err(ApiError::database)?;
            transaction.execute("INSERT INTO task_events (task_id, event_type, actor_type, actor_id, event_data) VALUES ($1, 'execution.job_queued', 'system', 'control-plane', $2)", &[&task_id, &json!({ "kind": "execute_task", "dedupe_key": dedupe_key })]).await.map_err(ApiError::database)?;
        }
        applied = true;
    }
    transaction.execute("INSERT INTO task_comments (id, task_id, author_type, author_name, content, intent, transition_applied) VALUES ($1, $2, 'human', 'Human', $3, $4, $5)", &[&comment_id, &task_id, &content, &intent, &applied]).await.map_err(ApiError::database)?;
    transaction.execute("INSERT INTO task_events (task_id, event_type, actor_type, actor_id, event_data) VALUES ($1, 'task.comment_added', 'human', 'human', $2)", &[&task_id, &json!({ "comment_id": comment_id, "intent": intent })]).await.map_err(ApiError::database)?;
    transaction.commit().await.map_err(ApiError::database)?;
    Ok((StatusCode::CREATED, Json(json!({ "id": comment_id, "task_id": task_id, "intent": intent, "transition_applied": applied }))))
}

/// 查询任务时间线事件，供后续运行记录和 Agent 上下文复用。
async fn list_events(State(state): State<AppState>, Path(task_id): Path<String>) -> Result<Json<Value>, ApiError> {
    let client = connect(&state).await?;
    let rows = client.query("SELECT id, event_type, actor_type, actor_id, before_data, after_data, event_data, to_char(created_at, 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"') FROM task_events WHERE task_id = $1 ORDER BY id ASC", &[&task_id]).await.map_err(ApiError::database)?;
    Ok(Json(json!({ "items": rows.iter().map(|row| json!({ "id": row.get::<_, i64>(0), "event_type": row.get::<_, String>(1), "actor_type": row.get::<_, String>(2), "actor_id": row.get::<_, String>(3), "before_data": row.get::<_, Option<Value>>(4), "after_data": row.get::<_, Option<Value>>(5), "event_data": row.get::<_, Value>(6), "created_at": row.get::<_, String>(7) })).collect::<Vec<_>>() })))
}

/// 查询任务关联的执行作业、尝试、事件和输出，供运行记录页面复用。
async fn list_execution(State(state): State<AppState>, Path(task_id): Path<String>) -> Result<Json<Value>, ApiError> {
    let client = connect(&state).await?;
    if client.query_opt("SELECT id FROM tasks WHERE id = $1", &[&task_id]).await.map_err(ApiError::database)?.is_none() { return Err(ApiError::not_found("task not found")); }
    let jobs = client.query("SELECT id, kind, status, task_id, payload, attempt_count, max_attempts, to_char(available_at, 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"'), to_char(created_at, 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"'), to_char(updated_at, 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"') FROM execution_jobs WHERE task_id = $1 ORDER BY created_at ASC", &[&task_id]).await.map_err(ApiError::database)?;
    let attempts = client.query("SELECT a.id, a.job_id, a.runner_instance_id, a.status, to_char(a.lease_expires_at, 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"'), to_char(a.started_at, 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"'), to_char(a.heartbeat_at, 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"'), to_char(a.finished_at, 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"'), a.failure_message, a.codex_thread_id FROM execution_attempts a JOIN execution_jobs j ON j.id = a.job_id WHERE j.task_id = $1 ORDER BY a.started_at ASC", &[&task_id]).await.map_err(ApiError::database)?;
    let events = client.query("SELECT id, job_id, attempt_id, task_id, event_type, payload, to_char(created_at, 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"') FROM execution_events WHERE task_id = $1 ORDER BY id ASC", &[&task_id]).await.map_err(ApiError::database)?;
    let outputs = client.query("SELECT id, job_id, task_id, output_type, content, to_char(created_at, 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"') FROM run_outputs WHERE task_id = $1 ORDER BY created_at ASC", &[&task_id]).await.map_err(ApiError::database)?;
    Ok(Json(json!({
        "jobs": jobs.iter().map(|row| json!({ "id": row.get::<_, String>(0), "kind": row.get::<_, String>(1), "status": row.get::<_, String>(2), "task_id": row.get::<_, Option<String>>(3), "payload": row.get::<_, Value>(4), "attempt_count": row.get::<_, i32>(5), "max_attempts": row.get::<_, i32>(6), "available_at": row.get::<_, String>(7), "created_at": row.get::<_, String>(8), "updated_at": row.get::<_, String>(9) })).collect::<Vec<_>>(),
        "attempts": attempts.iter().map(|row| json!({ "id": row.get::<_, String>(0), "job_id": row.get::<_, String>(1), "runner_instance_id": row.get::<_, String>(2), "status": row.get::<_, String>(3), "lease_expires_at": row.get::<_, String>(4), "started_at": row.get::<_, String>(5), "heartbeat_at": row.get::<_, String>(6), "finished_at": row.get::<_, Option<String>>(7), "failure_message": row.get::<_, Option<String>>(8), "codex_thread_id": row.get::<_, Option<String>>(9) })).collect::<Vec<_>>(),
        "events": events.iter().map(|row| json!({ "id": row.get::<_, i64>(0), "job_id": row.get::<_, String>(1), "attempt_id": row.get::<_, Option<String>>(2), "task_id": row.get::<_, Option<String>>(3), "event_type": row.get::<_, String>(4), "payload": row.get::<_, Value>(5), "created_at": row.get::<_, String>(6) })).collect::<Vec<_>>(),
        "outputs": outputs.iter().map(|row| json!({ "id": row.get::<_, String>(0), "job_id": row.get::<_, String>(1), "task_id": row.get::<_, Option<String>>(2), "output_type": row.get::<_, String>(3), "content": row.get::<_, String>(4), "created_at": row.get::<_, String>(5) })).collect::<Vec<_>>()
    })))
}
