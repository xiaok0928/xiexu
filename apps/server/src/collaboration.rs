use crate::{connect, required_text, ApiError, AppState};
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    routing::{get, patch, post},
    Json, Router,
};
use serde_json::{json, Value};
use std::collections::HashMap;
use tokio_postgres::Row;
use uuid::Uuid;

/// 组装 M3 Agent、记忆、指派和对话路由，保持与核心任务路由共享同一服务状态。
pub(crate) fn routes() -> Router<AppState> {
    // 路由只暴露已进入 M3 范围的能力，工作流和权限边界继续由后续里程碑处理。
    Router::new()
        .route("/api/agent-templates", get(list_agent_templates))
        .route("/api/agents", get(list_agents).post(create_agent))
        .route("/api/agents/responsibility-drafts", post(create_responsibility_draft))
        .route("/api/agents/:agent_id", get(get_agent).patch(update_agent))
        .route("/api/agents/:agent_id/memories", get(list_agent_memories).post(create_agent_memory))
        .route("/api/projects/:project_id/agents", get(list_project_agents).post(assign_project_agent))
        .route("/api/projects/:project_id/agents/:agent_id", patch(update_project_agent))
        .route("/api/tasks/:task_id/agents", get(list_task_agents).post(assign_task_agent))
        .route("/api/conversations", get(list_conversations).post(create_conversation))
        .route("/api/conversations/:conversation_id", get(get_conversation).patch(update_conversation))
        .route("/api/conversations/:conversation_id/messages", get(list_messages).post(create_message))
        .route("/api/conversations/:conversation_id/task-links", post(link_conversation_task))
        .route("/api/conversations/:conversation_id/tasks", post(create_task_from_conversation))
        .route("/api/conversations/:conversation_id/archive", post(archive_conversation))
        .route("/api/execution-jobs/:job_id", get(get_execution_job))
}

/// 将 Agent 行映射为公开响应，职责补充与基础指令分开返回便于界面编辑。
fn agent_json(row: &Row) -> Value {
    json!({
        "id": row.get::<_, String>(0), "template_code": row.get::<_, Option<String>>(1), "name": row.get::<_, String>(2),
        "description": row.get::<_, String>(3), "instructions": row.get::<_, String>(4), "responsibility_supplement": row.get::<_, String>(5),
        "status": row.get::<_, String>(6), "created_by": row.get::<_, String>(7), "created_at": row.get::<_, String>(8), "updated_at": row.get::<_, String>(9)
    })
}

/// 将对话行映射为公开响应，并携带参与者和任务数量的聚合结果。
fn conversation_json(row: &Row) -> Value {
    json!({
        "id": row.get::<_, String>(0), "conversation_type": row.get::<_, String>(1), "project_id": row.get::<_, Option<String>>(2),
        "title": row.get::<_, String>(3), "status": row.get::<_, String>(4), "created_by": row.get::<_, String>(5),
        "participant_count": row.get::<_, i64>(6), "task_count": row.get::<_, i64>(7), "created_at": row.get::<_, String>(8),
        "updated_at": row.get::<_, String>(9), "archived_at": row.get::<_, Option<String>>(10)
    })
}

/// 返回全部启用的内置 Agent 角色模板，按职业族和名称稳定排序。
async fn list_agent_templates(State(state): State<AppState>) -> Result<Json<Value>, ApiError> {
    // 模板是只读目录，直接一次性返回，避免创建 Agent 时产生重复查询。
    let client = connect(&state).await?;
    let rows = client.query("SELECT code, name, category, description, default_instructions, builtin FROM agent_role_templates WHERE active = TRUE ORDER BY category, name", &[]).await.map_err(ApiError::database)?;
    let items = rows.iter().map(|row| json!({ "code": row.get::<_, String>(0), "name": row.get::<_, String>(1), "category": row.get::<_, String>(2), "description": row.get::<_, String>(3), "default_instructions": row.get::<_, String>(4), "builtin": row.get::<_, bool>(5) })).collect::<Vec<_>>();
    Ok(Json(json!({ "items": items })))
}

/// 查询 Agent 实例，可按状态和角色模板筛选。
async fn list_agents(State(state): State<AppState>, Query(query): Query<HashMap<String, String>>) -> Result<Json<Value>, ApiError> {
    // 空筛选返回全部历史 Agent，界面可以明确展示停用状态。
    let client = connect(&state).await?;
    let status = query.get("status");
    let template_code = query.get("template_code");
    let rows = client.query("SELECT id, template_code, name, description, instructions, responsibility_supplement, status, created_by, to_char(created_at, 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"'), to_char(updated_at, 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"') FROM agents WHERE ($1::text IS NULL OR status = $1) AND ($2::text IS NULL OR template_code = $2) ORDER BY updated_at DESC", &[&status, &template_code]).await.map_err(ApiError::database)?;
    Ok(Json(json!({ "items": rows.iter().map(agent_json).collect::<Vec<_>>() })))
}

/// 创建具体 Agent 身份；选择模板时继承模板职责，但后续补充职责仍只影响该实例。
async fn create_agent(State(state): State<AppState>, Json(body): Json<Value>) -> Result<(StatusCode, Json<Value>), ApiError> {
    // 校验显示名称和可选模板，防止创建无法解释来源的半成品 Agent。
    let name = required_text(&body, "name")?;
    if name.len() > 200 { return Err(ApiError::invalid("name is too long")); }
    let template_code = body.get("template_code").and_then(Value::as_str).map(str::trim).filter(|value| !value.is_empty());
    let description_input = body.get("description").and_then(Value::as_str).unwrap_or("");
    let instructions_input = body.get("instructions").and_then(Value::as_str).unwrap_or("");
    let supplement = body.get("responsibility_supplement").and_then(Value::as_str).unwrap_or("");
    let client = connect(&state).await?;

    // 模板存在时用其文本补齐空白字段，自定义输入始终优先。
    let template = if let Some(code) = template_code {
        Some(client.query_opt("SELECT description, default_instructions FROM agent_role_templates WHERE code = $1 AND active = TRUE", &[&code]).await.map_err(ApiError::database)?.ok_or_else(|| ApiError::invalid("agent template not found"))?)
    } else {
        None
    };
    let description = if description_input.trim().is_empty() { template.as_ref().map(|row| row.get::<_, String>(0)).unwrap_or_default() } else { description_input.to_owned() };
    let instructions = if instructions_input.trim().is_empty() { template.as_ref().map(|row| row.get::<_, String>(1)).unwrap_or_default() } else { instructions_input.to_owned() };
    if description.trim().is_empty() || instructions.trim().is_empty() { return Err(ApiError::invalid("description and instructions are required for a custom agent")); }

    // 保存具体 Agent，模板后续更新不会覆盖实例级职责。
    let id = Uuid::new_v4().to_string();
    let row = client.query_one("INSERT INTO agents (id, template_code, name, description, instructions, responsibility_supplement) VALUES ($1, $2, $3, $4, $5, $6) RETURNING id, template_code, name, description, instructions, responsibility_supplement, status, created_by, to_char(created_at, 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"'), to_char(updated_at, 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"')", &[&id, &template_code, &name, &description, &instructions, &supplement]).await.map_err(ApiError::database)?;
    Ok((StatusCode::CREATED, Json(agent_json(&row))))
}

/// 查询单个 Agent 及其完整职责配置。
async fn get_agent(State(state): State<AppState>, Path(agent_id): Path<String>) -> Result<Json<Value>, ApiError> {
    // 明确 ID 查询包含停用实例，保证历史任务和记忆可解释。
    let client = connect(&state).await?;
    let row = client.query_opt("SELECT id, template_code, name, description, instructions, responsibility_supplement, status, created_by, to_char(created_at, 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"'), to_char(updated_at, 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"') FROM agents WHERE id = $1", &[&agent_id]).await.map_err(ApiError::database)?.ok_or_else(|| ApiError::not_found("agent not found"))?;
    Ok(Json(agent_json(&row)))
}

/// 更新 Agent 的展示信息、基础指令、特定职责补充或可用状态。
async fn update_agent(State(state): State<AppState>, Path(agent_id): Path<String>, Json(body): Json<Value>) -> Result<Json<Value>, ApiError> {
    // 只接受公开可编辑字段，并验证状态枚举。
    let name = body.get("name").and_then(Value::as_str).map(str::trim);
    let description = body.get("description").and_then(Value::as_str);
    let instructions = body.get("instructions").and_then(Value::as_str);
    let supplement = body.get("responsibility_supplement").and_then(Value::as_str);
    let status = body.get("status").and_then(Value::as_str);
    if name.is_none() && description.is_none() && instructions.is_none() && supplement.is_none() && status.is_none() { return Err(ApiError::invalid("no mutable fields")); }
    if name.is_some_and(str::is_empty) { return Err(ApiError::invalid("name cannot be empty")); }
    if status.is_some_and(|value| xiexu_domain::AgentStatus::parse(value).is_none()) { return Err(ApiError::invalid("invalid agent status")); }
    let client = connect(&state).await?;

    // 项目必须始终保留可工作的协调 Agent，因此禁止直接停用仍在岗的协调者。
    if status == Some("inactive") {
        let coordinates = client.query_one("SELECT EXISTS (SELECT 1 FROM project_agents WHERE agent_id = $1 AND assignment_type = 'coordinator' AND status = 'active')", &[&agent_id]).await.map_err(ApiError::database)?.get::<_, bool>(0);
        if coordinates { return Err(ApiError::conflict("replace the coordinator before deactivating this agent")); }
    }

    // 更新实例配置，不反向修改角色模板。
    let row = client.query_opt("UPDATE agents SET name = COALESCE($2, name), description = COALESCE($3, description), instructions = COALESCE($4, instructions), responsibility_supplement = COALESCE($5, responsibility_supplement), status = COALESCE($6, status), updated_at = now() WHERE id = $1 RETURNING id, template_code, name, description, instructions, responsibility_supplement, status, created_by, to_char(created_at, 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"'), to_char(updated_at, 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"')", &[&agent_id, &name, &description, &instructions, &supplement, &status]).await.map_err(ApiError::database)?.ok_or_else(|| ApiError::not_found("agent not found"))?;
    Ok(Json(agent_json(&row)))
}

/// 创建 Agent 职责优化作业，AI 输出作为草案保存，不自动覆盖现有身份。
async fn create_responsibility_draft(State(state): State<AppState>, Json(body): Json<Value>) -> Result<(StatusCode, Json<Value>), ApiError> {
    // 输入保留用户原始意图，并允许为已有 Agent 生成新的职责草案。
    let name = required_text(&body, "name")?;
    let description = required_text(&body, "description")?;
    let agent_id = body.get("agent_id").and_then(Value::as_str);
    let supplement = body.get("responsibility_supplement").and_then(Value::as_str).unwrap_or("");
    let client = connect(&state).await?;
    if let Some(id) = agent_id {
        if client.query_opt("SELECT 1 FROM agents WHERE id = $1", &[&id]).await.map_err(ApiError::database)?.is_none() { return Err(ApiError::not_found("agent not found")); }
    }

    // 使用独立作业保存草案生成过程，受控模式和真实 Codex 模式共享同一追踪路径。
    let job_id = Uuid::new_v4().to_string();
    client.execute("INSERT INTO execution_jobs (id, kind, status, agent_id, payload, dedupe_key) VALUES ($1, 'optimize_agent_profile', 'queued', $2, $3, $4)", &[&job_id, &agent_id, &json!({ "name": name, "description": description, "responsibility_supplement": supplement }), &format!("agent-profile:{job_id}")]).await.map_err(ApiError::database)?;
    Ok((StatusCode::ACCEPTED, Json(json!({ "job_id": job_id, "status": "queued" }))))
}

/// 查询指定 Agent 的私有记忆，可按项目和任务收窄上下文。
async fn list_agent_memories(State(state): State<AppState>, Path(agent_id): Path<String>, Query(query): Query<HashMap<String, String>>) -> Result<Json<Value>, ApiError> {
    // 过滤条件同时作用于数据库查询，避免先加载其他项目记忆再在应用层丢弃。
    let client = connect(&state).await?;
    let project_id = query.get("project_id");
    let task_id = query.get("task_id");
    let rows = client.query("SELECT id, agent_id, tier, project_id, task_id, job_id, content, source_type, source_id, status, to_char(created_at, 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"'), to_char(updated_at, 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"') FROM agent_memories WHERE agent_id = $1 AND ($2::text IS NULL OR project_id = $2) AND ($3::text IS NULL OR task_id = $3) ORDER BY updated_at DESC", &[&agent_id, &project_id, &task_id]).await.map_err(ApiError::database)?;
    let items = rows.iter().map(|row| json!({ "id": row.get::<_, String>(0), "agent_id": row.get::<_, String>(1), "tier": row.get::<_, String>(2), "project_id": row.get::<_, Option<String>>(3), "task_id": row.get::<_, Option<String>>(4), "job_id": row.get::<_, Option<String>>(5), "content": row.get::<_, String>(6), "source_type": row.get::<_, String>(7), "source_id": row.get::<_, Option<String>>(8), "status": row.get::<_, String>(9), "created_at": row.get::<_, String>(10), "updated_at": row.get::<_, String>(11) })).collect::<Vec<_>>();
    Ok(Json(json!({ "items": items })))
}

/// 为固定 Agent 写入一条可追溯的短期或长期经验记忆。
async fn create_agent_memory(State(state): State<AppState>, Path(agent_id): Path<String>, Json(body): Json<Value>) -> Result<(StatusCode, Json<Value>), ApiError> {
    // 校验记忆内容、层级和可选上下文归属。
    let content = required_text(&body, "content")?;
    let tier = body.get("tier").and_then(Value::as_str).unwrap_or("short_term");
    if xiexu_domain::MemoryTier::parse(tier).is_none() { return Err(ApiError::invalid("invalid memory tier")); }
    let project_id = body.get("project_id").and_then(Value::as_str);
    let task_id = body.get("task_id").and_then(Value::as_str);
    let job_id = body.get("job_id").and_then(Value::as_str);
    let source_type = body.get("source_type").and_then(Value::as_str).unwrap_or("human");
    let source_id = body.get("source_id").and_then(Value::as_str);
    let client = connect(&state).await?;
    if client.query_opt("SELECT 1 FROM agents WHERE id = $1", &[&agent_id]).await.map_err(ApiError::database)?.is_none() { return Err(ApiError::not_found("agent not found")); }

    // 同时提供项目和任务时必须属于同一项目，防止构造跨项目记忆上下文。
    if let Some(task) = task_id {
        let task_project = client.query_opt("SELECT project_id FROM tasks WHERE id = $1", &[&task]).await.map_err(ApiError::database)?.ok_or_else(|| ApiError::not_found("task not found"))?.get::<_, String>(0);
        if project_id.is_some_and(|project| project != task_project) { return Err(ApiError::invalid("task does not belong to project_id")); }
    } else if let Some(project) = project_id {
        if client.query_opt("SELECT 1 FROM projects WHERE id = $1", &[&project]).await.map_err(ApiError::database)?.is_none() { return Err(ApiError::not_found("project not found")); }
    }

    // 记忆固定写入当前 Agent，不提供跨 Agent 复制的隐式行为。
    let id = Uuid::new_v4().to_string();
    client.execute("INSERT INTO agent_memories (id, agent_id, tier, project_id, task_id, job_id, content, source_type, source_id) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)", &[&id, &agent_id, &tier, &project_id, &task_id, &job_id, &content, &source_type, &source_id]).await.map_err(ApiError::database)?;
    Ok((StatusCode::CREATED, Json(json!({ "id": id, "agent_id": agent_id, "tier": tier, "project_id": project_id, "task_id": task_id, "content": content, "status": "active" }))))
}

/// 查询项目固定 Agent 和唯一协调 Agent。
async fn list_project_agents(State(state): State<AppState>, Path(project_id): Path<String>) -> Result<Json<Value>, ApiError> {
    // 联表一次性返回 Agent 身份和项目级职责，避免前端逐项查询。
    let client = connect(&state).await?;
    let rows = client.query("SELECT a.id, a.name, a.template_code, a.description, a.status, pa.assignment_type, pa.responsibility_override, pa.status, to_char(pa.assigned_at, 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"') FROM project_agents pa JOIN agents a ON a.id = pa.agent_id WHERE pa.project_id = $1 ORDER BY CASE pa.assignment_type WHEN 'coordinator' THEN 0 ELSE 1 END, a.name", &[&project_id]).await.map_err(ApiError::database)?;
    let items = rows.iter().map(|row| json!({ "agent_id": row.get::<_, String>(0), "name": row.get::<_, String>(1), "template_code": row.get::<_, Option<String>>(2), "description": row.get::<_, String>(3), "agent_status": row.get::<_, String>(4), "assignment_type": row.get::<_, String>(5), "responsibility_override": row.get::<_, String>(6), "assignment_status": row.get::<_, String>(7), "assigned_at": row.get::<_, String>(8) })).collect::<Vec<_>>();
    Ok(Json(json!({ "items": items })))
}

/// 将 Agent 设为项目固定成员或替换当前协调 Agent。
async fn assign_project_agent(State(state): State<AppState>, Path(project_id): Path<String>, Json(body): Json<Value>) -> Result<(StatusCode, Json<Value>), ApiError> {
    // 校验职责类型和 Agent 可用状态。
    let agent_id = required_text(&body, "agent_id")?;
    let assignment_type = body.get("assignment_type").and_then(Value::as_str).unwrap_or("fixed");
    if xiexu_domain::ProjectAgentAssignment::parse(assignment_type).is_none() { return Err(ApiError::invalid("invalid project assignment type")); }
    let responsibility_override = body.get("responsibility_override").and_then(Value::as_str).unwrap_or("");
    let mut client = connect(&state).await?;
    let transaction = client.transaction().await.map_err(ApiError::database)?;
    if transaction.query_opt("SELECT 1 FROM projects WHERE id = $1 AND status = 'active'", &[&project_id]).await.map_err(ApiError::database)?.is_none() { return Err(ApiError::not_found("project not found")); }
    if transaction.query_opt("SELECT 1 FROM agents WHERE id = $1 AND status = 'active'", &[&agent_id]).await.map_err(ApiError::database)?.is_none() { return Err(ApiError::invalid("active agent not found")); }

    // 替换协调者时先把原协调者降为固定成员，使项目在事务提交后仍恰好有一个协调 Agent。
    if assignment_type == "coordinator" {
        transaction.execute("UPDATE project_agents SET assignment_type = 'fixed' WHERE project_id = $1 AND assignment_type = 'coordinator' AND status = 'active' AND agent_id <> $2", &[&project_id, &agent_id]).await.map_err(ApiError::database)?;
    }
    transaction.execute("INSERT INTO project_agents (project_id, agent_id, assignment_type, responsibility_override, status) VALUES ($1, $2, $3, $4, 'active') ON CONFLICT (project_id, agent_id) DO UPDATE SET assignment_type = EXCLUDED.assignment_type, responsibility_override = EXCLUDED.responsibility_override, status = 'active', assigned_at = now()", &[&project_id, &agent_id, &assignment_type, &responsibility_override]).await.map_err(ApiError::database)?;

    // 固定成员自动加入项目主群聊，已存在参与记录时恢复其有效成员状态。
    transaction.execute("INSERT INTO conversation_participants (conversation_id, actor_type, actor_id) SELECT id, 'agent', $2 FROM conversations WHERE project_id = $1 AND conversation_type = 'project_main' ON CONFLICT (conversation_id, actor_type, actor_id) DO UPDATE SET left_at = NULL", &[&project_id, &agent_id]).await.map_err(ApiError::database)?;
    transaction.commit().await.map_err(ApiError::database)?;
    Ok((StatusCode::CREATED, Json(json!({ "project_id": project_id, "agent_id": agent_id, "assignment_type": assignment_type, "status": "active" }))))
}

/// 更新项目 Agent 的职责补充或退出状态，最后一个协调 Agent 不允许退出。
async fn update_project_agent(State(state): State<AppState>, Path((project_id, agent_id)): Path<(String, String)>, Json(body): Json<Value>) -> Result<Json<Value>, ApiError> {
    // PATCH 只更新当前项目关系，不物理删除历史指派事实。
    let status = body.get("status").and_then(Value::as_str);
    let responsibility_override = body.get("responsibility_override").and_then(Value::as_str);
    if status.is_none() && responsibility_override.is_none() { return Err(ApiError::invalid("status or responsibility_override is required")); }
    if status.is_some_and(|value| value != "active" && value != "inactive") { return Err(ApiError::invalid("invalid assignment status")); }
    let client = connect(&state).await?;
    let current = client.query_opt("SELECT assignment_type FROM project_agents WHERE project_id = $1 AND agent_id = $2", &[&project_id, &agent_id]).await.map_err(ApiError::database)?.ok_or_else(|| ApiError::not_found("project agent assignment not found"))?;
    if status == Some("inactive") && current.get::<_, String>(0) == "coordinator" { return Err(ApiError::conflict("project coordinator cannot leave before replacement")); }

    // 退出项目时同步标记主群聊离开时间，历史消息仍保留原作者。
    client.execute("UPDATE project_agents SET status = COALESCE($3, status), responsibility_override = COALESCE($4, responsibility_override) WHERE project_id = $1 AND agent_id = $2", &[&project_id, &agent_id, &status, &responsibility_override]).await.map_err(ApiError::database)?;
    if status == Some("inactive") {
        client.execute("UPDATE conversation_participants SET left_at = now() WHERE actor_type = 'agent' AND actor_id = $2 AND conversation_id IN (SELECT id FROM conversations WHERE project_id = $1 AND conversation_type = 'project_main')", &[&project_id, &agent_id]).await.map_err(ApiError::database)?;
    }
    Ok(Json(json!({ "project_id": project_id, "agent_id": agent_id, "status": status.unwrap_or("active"), "responsibility_override": responsibility_override })))
}

/// 查询任务当前和历史 Agent 参与关系。
async fn list_task_agents(State(state): State<AppState>, Path(task_id): Path<String>) -> Result<Json<Value>, ApiError> {
    // 联表返回身份信息，使任务详情无需再次请求 Agent 列表。
    let client = connect(&state).await?;
    let rows = client.query("SELECT a.id, a.name, a.template_code, ta.participation_type, ta.status, to_char(ta.joined_at, 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"'), to_char(ta.left_at, 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"') FROM task_agents ta JOIN agents a ON a.id = ta.agent_id WHERE ta.task_id = $1 ORDER BY CASE ta.participation_type WHEN 'owner' THEN 0 ELSE 1 END, ta.joined_at", &[&task_id]).await.map_err(ApiError::database)?;
    let items = rows.iter().map(|row| json!({ "agent_id": row.get::<_, String>(0), "name": row.get::<_, String>(1), "template_code": row.get::<_, Option<String>>(2), "participation_type": row.get::<_, String>(3), "status": row.get::<_, String>(4), "joined_at": row.get::<_, String>(5), "left_at": row.get::<_, Option<String>>(6) })).collect::<Vec<_>>();
    Ok(Json(json!({ "items": items })))
}

/// 为任务指派主责、参与或协助 Agent，主责切换保持唯一。
async fn assign_task_agent(State(state): State<AppState>, Path(task_id): Path<String>, Json(body): Json<Value>) -> Result<(StatusCode, Json<Value>), ApiError> {
    // 校验任务参与类型和 Agent 可用状态，动态加入不要求预先成为项目固定成员。
    let agent_id = required_text(&body, "agent_id")?;
    let participation_type = body.get("participation_type").and_then(Value::as_str).unwrap_or("participant");
    if xiexu_domain::TaskAgentParticipation::parse(participation_type).is_none() { return Err(ApiError::invalid("invalid task participation type")); }
    let mut client = connect(&state).await?;
    let transaction = client.transaction().await.map_err(ApiError::database)?;
    if transaction.query_opt("SELECT 1 FROM tasks WHERE id = $1", &[&task_id]).await.map_err(ApiError::database)?.is_none() { return Err(ApiError::not_found("task not found")); }
    if transaction.query_opt("SELECT 1 FROM agents WHERE id = $1 AND status = 'active'", &[&agent_id]).await.map_err(ApiError::database)?.is_none() { return Err(ApiError::invalid("active agent not found")); }

    // 新主责接手前结束旧主责参与关系，参与和协助 Agent 可以并存。
    if participation_type == "owner" {
        transaction.execute("UPDATE task_agents SET status = 'inactive', left_at = now() WHERE task_id = $1 AND participation_type = 'owner' AND status = 'active' AND agent_id <> $2", &[&task_id, &agent_id]).await.map_err(ApiError::database)?;
    }
    transaction.execute("INSERT INTO task_agents (task_id, agent_id, participation_type, status, left_at) VALUES ($1, $2, $3, 'active', NULL) ON CONFLICT (task_id, agent_id) DO UPDATE SET participation_type = EXCLUDED.participation_type, status = 'active', joined_at = now(), left_at = NULL", &[&task_id, &agent_id, &participation_type]).await.map_err(ApiError::database)?;
    transaction.execute("INSERT INTO task_events (task_id, event_type, actor_type, actor_id, event_data) VALUES ($1, 'task.agent_assigned', 'agent', $2, $3)", &[&task_id, &agent_id, &json!({ "participation_type": participation_type })]).await.map_err(ApiError::database)?;
    transaction.commit().await.map_err(ApiError::database)?;
    Ok((StatusCode::CREATED, Json(json!({ "task_id": task_id, "agent_id": agent_id, "participation_type": participation_type, "status": "active" }))))
}

/// 查询当前用户可见的对话，可按项目和类型过滤。
async fn list_conversations(State(state): State<AppState>, Query(query): Query<HashMap<String, String>>) -> Result<Json<Value>, ApiError> {
    // MVP 暂不启用数据权限，因此返回满足显式筛选的全部对话。
    let client = connect(&state).await?;
    let project_id = query.get("project_id");
    let conversation_type = query.get("conversation_type");
    let rows = client.query("SELECT c.id, c.conversation_type, c.project_id, c.title, c.status, c.created_by, (SELECT count(*) FROM conversation_participants cp WHERE cp.conversation_id = c.id AND cp.left_at IS NULL), (SELECT count(*) FROM conversation_task_links ctl WHERE ctl.conversation_id = c.id), to_char(c.created_at, 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"'), to_char(c.updated_at, 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"'), to_char(c.archived_at, 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"') FROM conversations c WHERE ($1::text IS NULL OR c.project_id = $1) AND ($2::text IS NULL OR c.conversation_type = $2) ORDER BY c.updated_at DESC", &[&project_id, &conversation_type]).await.map_err(ApiError::database)?;
    Ok(Json(json!({ "items": rows.iter().map(conversation_json).collect::<Vec<_>>() })))
}

/// 创建 Human-Agent 一对一对话或项目临时群聊，项目主群聊只能随项目生成。
async fn create_conversation(State(state): State<AppState>, Json(body): Json<Value>) -> Result<(StatusCode, Json<Value>), ApiError> {
    // 校验对话类型与必要归属字段。
    let conversation_type = required_text(&body, "conversation_type")?;
    let parsed_type = xiexu_domain::ConversationType::parse(&conversation_type).ok_or_else(|| ApiError::invalid("invalid conversation type"))?;
    if parsed_type == xiexu_domain::ConversationType::ProjectMain { return Err(ApiError::invalid("project_main conversation is created with the project")); }
    let title = body.get("title").and_then(Value::as_str).map(str::trim).filter(|value| !value.is_empty()).unwrap_or("新对话");
    let project_id = body.get("project_id").and_then(Value::as_str);
    let direct_agent_id = body.get("agent_id").and_then(Value::as_str);
    let agent_ids = body.get("agent_ids").and_then(Value::as_array).map(|items| items.iter().filter_map(Value::as_str).map(ToOwned::to_owned).collect::<Vec<_>>()).unwrap_or_default();
    if parsed_type == xiexu_domain::ConversationType::Direct && direct_agent_id.is_none() { return Err(ApiError::invalid("direct conversation requires agent_id")); }
    if parsed_type == xiexu_domain::ConversationType::Direct && (project_id.is_some() || !agent_ids.is_empty()) { return Err(ApiError::invalid("direct conversation allows exactly one agent and no project")); }
    if parsed_type == xiexu_domain::ConversationType::ProjectTemporary && project_id.is_none() { return Err(ApiError::invalid("project temporary conversation requires project_id")); }
    let mut client = connect(&state).await?;
    let transaction = client.transaction().await.map_err(ApiError::database)?;
    if let Some(project) = project_id {
        if transaction.query_opt("SELECT 1 FROM projects WHERE id = $1 AND status = 'active'", &[&project]).await.map_err(ApiError::database)?.is_none() { return Err(ApiError::not_found("project not found")); }
    }

    // 建立对话和 Human 参与关系，参与者校验与写入保持同一事务。
    let conversation_id = Uuid::new_v4().to_string();
    transaction.execute("INSERT INTO conversations (id, conversation_type, project_id, title) VALUES ($1, $2, $3, $4)", &[&conversation_id, &conversation_type, &project_id, &title]).await.map_err(ApiError::database)?;
    transaction.execute("INSERT INTO conversation_participants (conversation_id, actor_type, actor_id) VALUES ($1, 'human', 'human')", &[&conversation_id]).await.map_err(ApiError::database)?;
    let mut participants = agent_ids;
    if let Some(agent_id) = direct_agent_id { participants.push(agent_id.to_owned()); }
    if participants.is_empty() && parsed_type == xiexu_domain::ConversationType::ProjectTemporary {
        let coordinator = transaction.query_one("SELECT agent_id FROM project_agents WHERE project_id = $1 AND assignment_type = 'coordinator' AND status = 'active'", &[&project_id]).await.map_err(ApiError::database)?.get::<_, String>(0);
        participants.push(coordinator);
    }
    participants.sort_unstable();
    participants.dedup();
    for agent_id in participants {
        if transaction.query_opt("SELECT 1 FROM agents WHERE id = $1 AND status = 'active'", &[&agent_id]).await.map_err(ApiError::database)?.is_none() { return Err(ApiError::invalid(format!("active agent not found: {agent_id}"))); }
        transaction.execute("INSERT INTO conversation_participants (conversation_id, actor_type, actor_id) VALUES ($1, 'agent', $2)", &[&conversation_id, &agent_id]).await.map_err(ApiError::database)?;
    }
    transaction.commit().await.map_err(ApiError::database)?;
    Ok((StatusCode::CREATED, Json(json!({ "id": conversation_id, "conversation_type": conversation_type, "project_id": project_id, "title": title, "status": "active" }))))
}

/// 查询单个对话、参与者和已关联任务。
async fn get_conversation(State(state): State<AppState>, Path(conversation_id): Path<String>) -> Result<Json<Value>, ApiError> {
    // 对话主体、参与者和任务关联分批查询后一次返回，避免前端产生 N+1 请求。
    let client = connect(&state).await?;
    let row = client.query_opt("SELECT c.id, c.conversation_type, c.project_id, c.title, c.status, c.created_by, (SELECT count(*) FROM conversation_participants cp WHERE cp.conversation_id = c.id AND cp.left_at IS NULL), (SELECT count(*) FROM conversation_task_links ctl WHERE ctl.conversation_id = c.id), to_char(c.created_at, 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"'), to_char(c.updated_at, 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"'), to_char(c.archived_at, 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"') FROM conversations c WHERE c.id = $1", &[&conversation_id]).await.map_err(ApiError::database)?.ok_or_else(|| ApiError::not_found("conversation not found"))?;
    let participants = client.query("SELECT actor_type, actor_id, to_char(joined_at, 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"'), to_char(left_at, 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"') FROM conversation_participants WHERE conversation_id = $1 ORDER BY joined_at", &[&conversation_id]).await.map_err(ApiError::database)?;
    let tasks = client.query("SELECT t.id, t.title, t.board_stage FROM conversation_task_links ctl JOIN tasks t ON t.id = ctl.task_id WHERE ctl.conversation_id = $1 ORDER BY ctl.linked_at", &[&conversation_id]).await.map_err(ApiError::database)?;
    let mut value = conversation_json(&row);
    value["participants"] = json!(participants.iter().map(|item| json!({ "actor_type": item.get::<_, String>(0), "actor_id": item.get::<_, String>(1), "joined_at": item.get::<_, String>(2), "left_at": item.get::<_, Option<String>>(3) })).collect::<Vec<_>>());
    value["tasks"] = json!(tasks.iter().map(|item| json!({ "id": item.get::<_, String>(0), "title": item.get::<_, String>(1), "board_stage": item.get::<_, String>(2) })).collect::<Vec<_>>());
    Ok(Json(value))
}

/// 更新对话标题；一对一对话可以通过该接口直接归档，项目临时群聊使用归档总结动作。
async fn update_conversation(State(state): State<AppState>, Path(conversation_id): Path<String>, Json(body): Json<Value>) -> Result<Json<Value>, ApiError> {
    // 普通更新只允许标题和 direct 对话归档状态。
    let title = body.get("title").and_then(Value::as_str).map(str::trim);
    let status = body.get("status").and_then(Value::as_str);
    if title.is_none() && status.is_none() { return Err(ApiError::invalid("title or status is required")); }
    if title.is_some_and(str::is_empty) { return Err(ApiError::invalid("title cannot be empty")); }
    if status.is_some_and(|value| value != "active" && value != "archived") { return Err(ApiError::invalid("invalid conversation status")); }
    let client = connect(&state).await?;
    let kind = client.query_opt("SELECT conversation_type FROM conversations WHERE id = $1", &[&conversation_id]).await.map_err(ApiError::database)?.ok_or_else(|| ApiError::not_found("conversation not found"))?.get::<_, String>(0);
    if status == Some("archived") && kind != "direct" { return Err(ApiError::invalid("project conversations must use the archive action")); }

    // 归档 direct 对话只停止新消息，不删除参与者和历史内容。
    client.execute("UPDATE conversations SET title = COALESCE($2, title), status = COALESCE($3, status), archived_at = CASE WHEN $3 = 'archived' THEN now() WHEN $3 = 'active' THEN NULL ELSE archived_at END, updated_at = now() WHERE id = $1", &[&conversation_id, &title, &status]).await.map_err(ApiError::database)?;
    get_conversation(State(state), Path(conversation_id)).await
}

/// 按时间顺序返回对话消息，归档对话仍可完整查看。
async fn list_messages(State(state): State<AppState>, Path(conversation_id): Path<String>) -> Result<Json<Value>, ApiError> {
    // 消息是追加事实，查询不根据对话状态过滤。
    let client = connect(&state).await?;
    let rows = client.query("SELECT id, author_type, author_id, content, message_type, task_id, to_char(created_at, 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"') FROM conversation_messages WHERE conversation_id = $1 ORDER BY created_at, id", &[&conversation_id]).await.map_err(ApiError::database)?;
    let items = rows.iter().map(|row| json!({ "id": row.get::<_, String>(0), "author_type": row.get::<_, String>(1), "author_id": row.get::<_, String>(2), "content": row.get::<_, String>(3), "message_type": row.get::<_, String>(4), "task_id": row.get::<_, Option<String>>(5), "created_at": row.get::<_, String>(6) })).collect::<Vec<_>>();
    Ok(Json(json!({ "items": items })))
}

/// 追加一条 Human 或参与 Agent 消息，所有评论内容只记录事实，不隐式创建任务。
async fn create_message(State(state): State<AppState>, Path(conversation_id): Path<String>, Json(body): Json<Value>) -> Result<(StatusCode, Json<Value>), ApiError> {
    // 校验作者和消息正文，Human 使用稳定 actor_id，Agent 必须是当前参与者。
    let content = required_text(&body, "content")?;
    let author_type = body.get("author_type").and_then(Value::as_str).unwrap_or("human");
    let requested_author_id = body.get("author_id").and_then(Value::as_str).unwrap_or("human");
    if author_type != "human" && author_type != "agent" { return Err(ApiError::invalid("invalid author_type")); }
    let author_id = if author_type == "human" { "human" } else { requested_author_id };
    let client = connect(&state).await?;
    let conversation = client.query_opt("SELECT status FROM conversations WHERE id = $1", &[&conversation_id]).await.map_err(ApiError::database)?.ok_or_else(|| ApiError::not_found("conversation not found"))?;
    if conversation.get::<_, String>(0) != "active" { return Err(ApiError::conflict("conversation is not active")); }
    if author_type == "agent" && client.query_opt("SELECT 1 FROM conversation_participants WHERE conversation_id = $1 AND actor_type = 'agent' AND actor_id = $2 AND left_at IS NULL", &[&conversation_id, &author_id]).await.map_err(ApiError::database)?.is_none() { return Err(ApiError::invalid("agent is not an active conversation participant")); }

    // 普通消息不执行意图推断，任务创建必须调用显式任务动作。
    let id = Uuid::new_v4().to_string();
    client.execute("INSERT INTO conversation_messages (id, conversation_id, author_type, author_id, content) VALUES ($1, $2, $3, $4, $5)", &[&id, &conversation_id, &author_type, &author_id, &content]).await.map_err(ApiError::database)?;
    client.execute("UPDATE conversations SET updated_at = now() WHERE id = $1", &[&conversation_id]).await.map_err(ApiError::database)?;
    Ok((StatusCode::CREATED, Json(json!({ "id": id, "conversation_id": conversation_id, "author_type": author_type, "author_id": author_id, "content": content, "message_type": "text" }))))
}

/// 将同一项目中的既有任务关联到项目对话。
async fn link_conversation_task(State(state): State<AppState>, Path(conversation_id): Path<String>, Json(body): Json<Value>) -> Result<(StatusCode, Json<Value>), ApiError> {
    // 关联前校验对话和任务属于同一项目，direct 对话不允许挂载项目任务。
    let task_id = required_text(&body, "task_id")?;
    let client = connect(&state).await?;
    let valid = client.query_opt("SELECT 1 FROM conversations c JOIN tasks t ON t.project_id = c.project_id WHERE c.id = $1 AND t.id = $2 AND c.conversation_type IN ('project_main', 'project_temporary')", &[&conversation_id, &task_id]).await.map_err(ApiError::database)?.is_some();
    if !valid { return Err(ApiError::invalid("conversation and task must belong to the same project")); }

    // 重复关联保持幂等，不生成重复消息。
    client.execute("INSERT INTO conversation_task_links (conversation_id, task_id) VALUES ($1, $2) ON CONFLICT DO NOTHING", &[&conversation_id, &task_id]).await.map_err(ApiError::database)?;
    Ok((StatusCode::CREATED, Json(json!({ "conversation_id": conversation_id, "task_id": task_id }))))
}

/// 从项目群聊中的明确需求动作创建 Backlog 任务，并保留对话来源关联。
async fn create_task_from_conversation(State(state): State<AppState>, Path(conversation_id): Path<String>, Json(body): Json<Value>) -> Result<(StatusCode, Json<Value>), ApiError> {
    // 任务标题是显式输入，普通消息不会触发该函数。
    let title = required_text(&body, "title")?;
    if title.len() > 500 { return Err(ApiError::invalid("title is too long")); }
    let description = body.get("description").and_then(Value::as_str).unwrap_or("");
    let requires = body.get("requires_plan_confirmation").and_then(Value::as_bool).unwrap_or(true);
    let mut client = connect(&state).await?;
    let transaction = client.transaction().await.map_err(ApiError::database)?;
    let conversation = transaction.query_opt("SELECT project_id, status FROM conversations WHERE id = $1 AND conversation_type IN ('project_main', 'project_temporary') FOR UPDATE", &[&conversation_id]).await.map_err(ApiError::database)?.ok_or_else(|| ApiError::invalid("project conversation not found"))?;
    if conversation.get::<_, String>(1) != "active" { return Err(ApiError::conflict("conversation is not active")); }
    let project_id = conversation.get::<_, Option<String>>(0).ok_or_else(|| ApiError::invalid("conversation has no project"))?;

    // 创建 Backlog 任务、对话关联、来源消息和任务事件，四类事实保持原子一致。
    let task_id = Uuid::new_v4().to_string();
    transaction.execute("INSERT INTO tasks (id, project_id, title, description, board_stage, plan_status, execution_status, requires_plan_confirmation) VALUES ($1, $2, $3, $4, 'backlog', CASE WHEN $5 THEN 'pending_generation' ELSE 'not_required' END, 'idle', $5)", &[&task_id, &project_id, &title, &description, &requires]).await.map_err(ApiError::database)?;
    transaction.execute("INSERT INTO conversation_task_links (conversation_id, task_id) VALUES ($1, $2)", &[&conversation_id, &task_id]).await.map_err(ApiError::database)?;
    transaction.execute("INSERT INTO conversation_messages (id, conversation_id, author_type, author_id, content, message_type, task_id) VALUES ($1, $2, 'human', 'human', $3, 'task_request', $4)", &[&Uuid::new_v4().to_string(), &conversation_id, &title, &task_id]).await.map_err(ApiError::database)?;
    transaction.execute("INSERT INTO task_events (task_id, event_type, actor_type, actor_id, after_data, event_data) VALUES ($1, 'task.created', 'human', 'human', $2, $3)", &[&task_id, &json!({ "board_stage": "backlog" }), &json!({ "source": "conversation", "conversation_id": conversation_id })]).await.map_err(ApiError::database)?;
    transaction.execute("UPDATE conversations SET updated_at = now() WHERE id = $1", &[&conversation_id]).await.map_err(ApiError::database)?;
    transaction.commit().await.map_err(ApiError::database)?;
    Ok((StatusCode::CREATED, Json(json!({ "id": task_id, "project_id": project_id, "title": title, "description": description, "board_stage": "backlog", "requires_plan_confirmation": requires, "conversation_id": conversation_id }))))
}

/// 归档项目临时群聊并创建总结作业，原始消息在总结失败时仍保持可读。
async fn archive_conversation(State(state): State<AppState>, Path(conversation_id): Path<String>) -> Result<(StatusCode, Json<Value>), ApiError> {
    // 锁定临时群聊并拒绝重复归档或主群聊归档。
    let mut client = connect(&state).await?;
    let transaction = client.transaction().await.map_err(ApiError::database)?;
    let row = transaction.query_opt("SELECT project_id, status FROM conversations WHERE id = $1 AND conversation_type = 'project_temporary' FOR UPDATE", &[&conversation_id]).await.map_err(ApiError::database)?.ok_or_else(|| ApiError::invalid("temporary project conversation not found"))?;
    if row.get::<_, String>(1) != "active" { return Err(ApiError::conflict("conversation is already archiving or archived")); }
    let project_id = row.get::<_, Option<String>>(0);

    // 先冻结新消息，再入队总结；作业成功后才将状态推进为 archived。
    let job_id = Uuid::new_v4().to_string();
    transaction.execute("UPDATE conversations SET status = 'archiving', updated_at = now() WHERE id = $1", &[&conversation_id]).await.map_err(ApiError::database)?;
    transaction.execute("INSERT INTO execution_jobs (id, kind, status, project_id, conversation_id, payload, dedupe_key) VALUES ($1, 'summarize_conversation', 'queued', $2, $3, $4, $5)", &[&job_id, &project_id, &conversation_id, &json!({ "conversation_id": conversation_id }), &format!("conversation:{conversation_id}:archive")]).await.map_err(ApiError::database)?;
    transaction.commit().await.map_err(ApiError::database)?;
    Ok((StatusCode::ACCEPTED, Json(json!({ "conversation_id": conversation_id, "job_id": job_id, "status": "archiving" }))))
}

/// 查询通用执行作业和输出，供职责草案及对话总结轮询结果。
async fn get_execution_job(State(state): State<AppState>, Path(job_id): Path<String>) -> Result<Json<Value>, ApiError> {
    // 作业和输出分开查询，避免多输出时重复作业字段。
    let client = connect(&state).await?;
    let row = client.query_opt("SELECT id, kind, status, project_id, task_id, agent_id, conversation_id, payload, attempt_count, max_attempts, to_char(created_at, 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"'), to_char(updated_at, 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"') FROM execution_jobs WHERE id = $1", &[&job_id]).await.map_err(ApiError::database)?.ok_or_else(|| ApiError::not_found("execution job not found"))?;
    let outputs = client.query("SELECT id, output_type, content, to_char(created_at, 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"') FROM run_outputs WHERE job_id = $1 ORDER BY created_at", &[&job_id]).await.map_err(ApiError::database)?;
    Ok(Json(json!({ "id": row.get::<_, String>(0), "kind": row.get::<_, String>(1), "status": row.get::<_, String>(2), "project_id": row.get::<_, Option<String>>(3), "task_id": row.get::<_, Option<String>>(4), "agent_id": row.get::<_, Option<String>>(5), "conversation_id": row.get::<_, Option<String>>(6), "payload": row.get::<_, Value>(7), "attempt_count": row.get::<_, i32>(8), "max_attempts": row.get::<_, i32>(9), "created_at": row.get::<_, String>(10), "updated_at": row.get::<_, String>(11), "outputs": outputs.iter().map(|item| json!({ "id": item.get::<_, String>(0), "output_type": item.get::<_, String>(1), "content": item.get::<_, String>(2), "created_at": item.get::<_, String>(3) })).collect::<Vec<_>>() })))
}
