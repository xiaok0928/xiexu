use crate::{connect, required_text, ApiError, AppState};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::{get, patch, post},
    Json, Router,
};
use serde_json::{json, Value};
use tokio_postgres::{Row, Transaction};
use uuid::Uuid;

/// 工作流节点允许的最小类型集合，对应画布中的开始、结束、执行、判断和人工确认。
const NODE_TYPES: [&str; 5] = ["start", "end", "execute", "condition", "human_confirm"];

/// 工作流模块路由，覆盖画布保存、定义查询、手动运行和运行控制。
pub(crate) fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/api/projects/:project_id/workflows",
            get(list_workflows).post(create_workflow),
        )
        .route(
            "/api/workflows/:workflow_id",
            get(get_workflow).patch(update_workflow),
        )
        .route("/api/workflows/:workflow_id/pause", post(pause_workflow))
        .route("/api/workflows/:workflow_id/resume", post(resume_workflow))
        .route(
            "/api/workflows/:workflow_id/terminate",
            post(terminate_workflow),
        )
        .route(
            "/api/workflows/:workflow_id/schedules",
            get(list_schedules).post(create_schedule),
        )
        .route(
            "/api/workflows/:workflow_id/runs",
            get(list_workflow_runs).post(create_workflow_run),
        )
        .route("/api/workflow-runs/:run_id", get(get_workflow_run))
        .route("/api/workflow-runs/:run_id/nodes", get(list_node_runs))
        .route("/api/workflow-runs/:run_id/approvals", get(list_approvals))
        .route("/api/workflow-runs/:run_id/outputs", get(list_run_outputs))
        .route("/api/workflow-runs/:run_id/pause", post(pause_workflow_run))
        .route(
            "/api/workflow-runs/:run_id/resume",
            post(resume_workflow_run),
        )
        .route(
            "/api/workflow-runs/:run_id/terminate",
            post(terminate_workflow_run),
        )
        .route(
            "/api/workflow-schedules/:schedule_id",
            patch(update_schedule).delete(delete_schedule),
        )
        .route(
            "/api/workflow-schedules/:schedule_id/enable",
            post(enable_schedule),
        )
        .route(
            "/api/workflow-schedules/:schedule_id/disable",
            post(disable_schedule),
        )
        .route(
            "/api/approval-requests/:approval_id/resolve",
            post(resolve_approval),
        )
}

/// 从请求体读取画布数组并校验节点、连线和分支条件的基本完整性。
fn parse_canvas(body: &Value) -> Result<(Vec<Value>, Vec<Value>), ApiError> {
    let nodes = body
        .get("nodes")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let edges = body
        .get("edges")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if nodes.is_empty() {
        return Err(ApiError::invalid("workflow requires at least one node"));
    }
    let mut keys = std::collections::HashSet::new();
    let mut starts = 0;
    let mut ends = 0;
    for node in &nodes {
        let key = node
            .get("id")
            .or_else(|| node.get("key"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| ApiError::invalid("workflow node id is required"))?;
        if !keys.insert(key.to_owned()) {
            return Err(ApiError::invalid("workflow node ids must be unique"));
        }
        let node_type = node
            .get("node_type")
            .or_else(|| node.get("type"))
            .and_then(Value::as_str)
            .unwrap_or("execute");
        if !NODE_TYPES.contains(&node_type) {
            return Err(ApiError::invalid(format!(
                "unsupported workflow node type: {node_type}"
            )));
        }
        starts += i32::from(node_type == "start");
        ends += i32::from(node_type == "end");
        if node
            .get("label")
            .and_then(Value::as_str)
            .map(str::trim)
            .is_none_or(str::is_empty)
        {
            return Err(ApiError::invalid("workflow node label is required"));
        }
    }
    for edge in &edges {
        let source = edge
            .get("source")
            .or_else(|| edge.get("source_node_key"))
            .and_then(Value::as_str)
            .unwrap_or("");
        let target = edge
            .get("target")
            .or_else(|| edge.get("target_node_key"))
            .and_then(Value::as_str)
            .unwrap_or("");
        if !keys.contains(source) || !keys.contains(target) {
            return Err(ApiError::invalid(
                "workflow edge references an unknown node",
            ));
        }
        if edge
            .get("id")
            .or_else(|| edge.get("key"))
            .and_then(Value::as_str)
            .map(str::trim)
            .is_none_or(str::is_empty)
        {
            return Err(ApiError::invalid("workflow edge id is required"));
        }
    }
    if starts != 1 || ends < 1 {
        return Err(ApiError::invalid(
            "workflow requires exactly one start node and at least one end node",
        ));
    }
    for node in nodes.iter().filter(|node| {
        node.get("node_type")
            .or_else(|| node.get("type"))
            .and_then(Value::as_str)
            == Some("condition")
    }) {
        let key = node
            .get("id")
            .or_else(|| node.get("key"))
            .and_then(Value::as_str)
            .unwrap_or("");
        let labels = edges
            .iter()
            .filter(|edge| {
                edge.get("source")
                    .or_else(|| edge.get("source_node_key"))
                    .and_then(Value::as_str)
                    == Some(key)
            })
            .filter_map(|edge| edge.get("label").and_then(Value::as_str))
            .collect::<std::collections::HashSet<_>>();
        if !labels.contains("是") && !labels.contains("yes") {
            return Err(ApiError::invalid(format!(
                "condition node {key} requires a yes edge"
            )));
        }
        if !labels.contains("否") && !labels.contains("no") {
            return Err(ApiError::invalid(format!(
                "condition node {key} requires a no edge"
            )));
        }
    }
    Ok((nodes, edges))
}

/// 从画布节点中读取统一字段，允许前端使用 id/key、type/node_type 两种兼容命名。
fn node_fields(node: &Value) -> Result<(String, String, String, Value, f64, f64), ApiError> {
    let key = node
        .get("id")
        .or_else(|| node.get("key"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_owned();
    let node_type = node
        .get("node_type")
        .or_else(|| node.get("type"))
        .and_then(Value::as_str)
        .unwrap_or("execute")
        .to_owned();
    let label = node
        .get("label")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_owned();
    let config = node.get("config").cloned().unwrap_or_else(|| json!({}));
    let x = node
        .get("position_x")
        .or_else(|| node.get("x"))
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    let y = node
        .get("position_y")
        .or_else(|| node.get("y"))
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    if key.is_empty() || label.is_empty() {
        return Err(ApiError::invalid("workflow node id and label are required"));
    }
    Ok((key, node_type, label, config, x, y))
}

/// 读取统一连线字段，分支条件作为 JSON 保留以兼容后续自然语言解析结果。
fn edge_fields(edge: &Value) -> Result<(String, String, String, String, Value), ApiError> {
    let key = edge
        .get("id")
        .or_else(|| edge.get("key"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_owned();
    let source = edge
        .get("source")
        .or_else(|| edge.get("source_node_key"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_owned();
    let target = edge
        .get("target")
        .or_else(|| edge.get("target_node_key"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_owned();
    let label = edge
        .get("label")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    let condition = edge.get("condition").cloned().unwrap_or_else(|| json!({}));
    if key.is_empty() || source.is_empty() || target.is_empty() {
        return Err(ApiError::invalid(
            "workflow edge id, source and target are required",
        ));
    }
    Ok((key, source, target, label, condition))
}

/// 在事务内创建工作流版本，并将节点、连线拆成可查询的结构化事实。
async fn insert_version(
    transaction: &Transaction<'_>,
    workflow_id: &str,
    version_no: i32,
    body: &Value,
    created_by: &str,
) -> Result<String, ApiError> {
    let (nodes, edges) = parse_canvas(body)?;
    let version_id = Uuid::new_v4().to_string();
    let definition = json!({ "nodes": nodes, "edges": edges });
    transaction
        .execute(
            "INSERT INTO workflow_versions (id, workflow_id, version_no, status, definition, created_by) VALUES ($1, $2, $3, 'saved', $4, $5)",
            &[&version_id, &workflow_id, &version_no, &definition, &created_by],
        )
        .await
        .map_err(ApiError::database)?;
    for node in body
        .get("nodes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let (key, node_type, label, config, x, y) = node_fields(node)?;
        transaction
            .execute(
                "INSERT INTO workflow_nodes (id, version_id, node_key, node_type, label, config, position_x, position_y) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
                &[&Uuid::new_v4().to_string(), &version_id, &key, &node_type, &label, &config, &x, &y],
            )
            .await
            .map_err(ApiError::database)?;
    }
    for edge in body
        .get("edges")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let (key, source, target, label, condition) = edge_fields(edge)?;
        transaction
            .execute(
                "INSERT INTO workflow_edges (id, version_id, edge_key, source_node_key, target_node_key, label, condition) VALUES ($1, $2, $3, $4, $5, $6, $7)",
                &[&Uuid::new_v4().to_string(), &version_id, &key, &source, &target, &label, &condition],
            )
            .await
            .map_err(ApiError::database)?;
    }
    Ok(version_id)
}

/// 将工作流列表行映射为稳定的摘要响应。
fn workflow_summary(row: &Row) -> Value {
    json!({ "id": row.get::<_, String>(0), "project_id": row.get::<_, String>(1), "name": row.get::<_, String>(2), "description": row.get::<_, String>(3), "status": row.get::<_, String>(4), "current_version_no": row.get::<_, i32>(5), "created_by": row.get::<_, String>(6), "created_at": row.get::<_, String>(7), "updated_at": row.get::<_, String>(8) })
}

/// 查询项目下的工作流定义摘要，画布内容通过单个工作流查询按需加载。
async fn list_workflows(
    State(state): State<AppState>,
    Path(project_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let client = connect(&state).await?;
    let rows = client.query("SELECT id, project_id, name, description, status, current_version_no, created_by, to_char(created_at, 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"'), to_char(updated_at, 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"') FROM workflows WHERE project_id = $1 ORDER BY updated_at DESC", &[&project_id]).await.map_err(ApiError::database)?;
    Ok(Json(
        json!({ "items": rows.iter().map(workflow_summary).collect::<Vec<_>>() }),
    ))
}

/// 创建工作流并保存首个画布版本，未传画布时使用一个开始到结束的可运行骨架。
async fn create_workflow(
    State(state): State<AppState>,
    Path(project_id): Path<String>,
    Json(body): Json<Value>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let name = required_text(&body, "name")?;
    let description = body
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or("");
    let default_canvas = json!({ "nodes": [{ "id": "start", "type": "start", "label": "开始" }, { "id": "end", "type": "end", "label": "结束" }], "edges": [{ "id": "start-end", "source": "start", "target": "end", "label": "" }] });
    let canvas = if body.get("nodes").is_some() {
        &body
    } else {
        &default_canvas
    };
    let workflow_id = Uuid::new_v4().to_string();
    let mut client = connect(&state).await?;
    let transaction = client.transaction().await.map_err(ApiError::database)?;
    if transaction
        .query_opt("SELECT 1 FROM projects WHERE id = $1", &[&project_id])
        .await
        .map_err(ApiError::database)?
        .is_none()
    {
        return Err(ApiError::not_found("project not found"));
    }
    transaction
        .execute(
            "INSERT INTO workflows (id, project_id, name, description, current_version_no) VALUES ($1, $2, $3, $4, 1)",
            &[&workflow_id, &project_id, &name, &description],
        )
        .await
        .map_err(ApiError::database)?;
    let version_id = insert_version(&transaction, &workflow_id, 1, canvas, "human").await?;
    transaction.commit().await.map_err(ApiError::database)?;
    Ok((
        StatusCode::CREATED,
        Json(
            json!({ "id": workflow_id, "project_id": project_id, "name": name, "description": description, "status": "active", "current_version_no": 1, "version_id": version_id }),
        ),
    ))
}

/// 查询工作流、当前版本和画布节点连线，保证保存后可完整恢复画布。
async fn get_workflow(
    State(state): State<AppState>,
    Path(workflow_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let client = connect(&state).await?;
    let row = client.query_opt("SELECT id, project_id, name, description, status, current_version_no, created_by, to_char(created_at, 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"'), to_char(updated_at, 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"') FROM workflows WHERE id = $1", &[&workflow_id]).await.map_err(ApiError::database)?.ok_or_else(|| ApiError::not_found("workflow not found"))?;
    let version = client.query_opt("SELECT id, version_no, status, definition, created_by, to_char(created_at, 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"') FROM workflow_versions WHERE workflow_id = $1 AND version_no = $2", &[&workflow_id, &row.get::<_, i32>(5)]).await.map_err(ApiError::database)?.ok_or_else(|| ApiError::not_found("workflow version not found"))?;
    let nodes = client
        .query(
            "SELECT node_key, node_type, label, config, position_x, position_y FROM workflow_nodes WHERE version_id = $1 ORDER BY node_key",
            &[&version.get::<_, String>(0)],
        )
        .await
        .map_err(ApiError::database)?;
    let edges = client
        .query(
            "SELECT edge_key, source_node_key, target_node_key, label, condition FROM workflow_edges WHERE version_id = $1 ORDER BY edge_key",
            &[&version.get::<_, String>(0)],
        )
        .await
        .map_err(ApiError::database)?;
    let mut response = workflow_summary(&row);
    response["version"] = json!({ "id": version.get::<_, String>(0), "version_no": version.get::<_, i32>(1), "status": version.get::<_, String>(2), "definition": version.get::<_, Value>(3), "created_by": version.get::<_, String>(4), "created_at": version.get::<_, String>(5), "nodes": nodes.iter().map(|item| json!({ "id": item.get::<_, String>(0), "type": item.get::<_, String>(1), "label": item.get::<_, String>(2), "config": item.get::<_, Value>(3), "x": item.get::<_, f64>(4), "y": item.get::<_, f64>(5) })).collect::<Vec<_>>(), "edges": edges.iter().map(|item| json!({ "id": item.get::<_, String>(0), "source": item.get::<_, String>(1), "target": item.get::<_, String>(2), "label": item.get::<_, String>(3), "condition": item.get::<_, Value>(4) })).collect::<Vec<_>>() });
    Ok(Json(response))
}

/// 保存工作流名称和新画布版本；每次保存追加不可变版本，旧版本仍可用于历史运行记录。
async fn update_workflow(
    State(state): State<AppState>,
    Path(workflow_id): Path<String>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let mut client = connect(&state).await?;
    let transaction = client.transaction().await.map_err(ApiError::database)?;
    let current = transaction
        .query_opt(
            "SELECT project_id, current_version_no, status FROM workflows WHERE id = $1 FOR UPDATE",
            &[&workflow_id],
        )
        .await
        .map_err(ApiError::database)?
        .ok_or_else(|| ApiError::not_found("workflow not found"))?;
    if current.get::<_, String>(2) == "terminated" {
        return Err(ApiError::conflict("terminated workflow cannot be modified"));
    }
    let name = body.get("name").and_then(Value::as_str).map(str::trim);
    let description = body.get("description").and_then(Value::as_str);
    if name.is_some_and(str::is_empty) {
        return Err(ApiError::invalid("workflow name cannot be empty"));
    }
    if name.is_some() || description.is_some() {
        transaction
            .execute(
                "UPDATE workflows SET name = COALESCE($2, name), description = COALESCE($3, description), updated_at = now() WHERE id = $1",
                &[&workflow_id, &name, &description],
            )
            .await
            .map_err(ApiError::database)?;
    }
    let version_id = if body.get("nodes").is_some() || body.get("edges").is_some() {
        let next = current.get::<_, i32>(1) + 1;
        let version_id = insert_version(&transaction, &workflow_id, next, &body, "human").await?;
        transaction
            .execute(
                "UPDATE workflows SET current_version_no = $2, updated_at = now() WHERE id = $1",
                &[&workflow_id, &next],
            )
            .await
            .map_err(ApiError::database)?;
        Some((next, version_id))
    } else {
        None
    };
    transaction.commit().await.map_err(ApiError::database)?;
    let mut result = get_workflow(State(state), Path(workflow_id)).await?.0;
    if let Some((version_no, id)) = version_id {
        result["saved_version"] = json!({ "version_no": version_no, "id": id });
    }
    Ok(Json(result))
}

/// 控制自动化定义状态，只影响未来触发，不改变已经创建的运行实例。
async fn control_workflow(
    State(state): State<AppState>,
    workflow_id: String,
    action: &str,
) -> Result<Json<Value>, ApiError> {
    let client = connect(&state).await?;
    let row = client
        .query_opt(
            "SELECT status FROM workflows WHERE id = $1",
            &[&workflow_id],
        )
        .await
        .map_err(ApiError::database)?
        .ok_or_else(|| ApiError::not_found("workflow not found"))?;
    let current = row.get::<_, String>(0);
    let next = match (action, current.as_str()) {
        ("pause", "active") => "paused",
        ("resume", "paused") => "active",
        ("terminate", "active" | "paused") => "terminated",
        (_, "paused") if action == "pause" => "paused",
        (_, "terminated") if action == "terminate" => "terminated",
        _ => {
            return Err(ApiError::conflict(format!(
                "cannot {action} workflow from {current}"
            )))
        }
    };

    // 调度记录保持原配置，调度器通过工作流状态阻止暂停或终止定义产生新运行。
    client
        .execute(
            "UPDATE workflows SET status = $2, updated_at = now() WHERE id = $1",
            &[&workflow_id, &next],
        )
        .await
        .map_err(ApiError::database)?;
    get_workflow(State(state), Path(workflow_id)).await
}

/// 暂停自动化定义，已经产生的运行实例继续执行。
async fn pause_workflow(
    State(state): State<AppState>,
    Path(workflow_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    control_workflow(State(state), workflow_id, "pause").await
}

/// 恢复暂停的自动化定义，原有启用调度重新具备触发资格。
async fn resume_workflow(
    State(state): State<AppState>,
    Path(workflow_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    control_workflow(State(state), workflow_id, "resume").await
}

/// 终止自动化定义，终止不可恢复且不会终止既有运行实例。
async fn terminate_workflow(
    State(state): State<AppState>,
    Path(workflow_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    control_workflow(State(state), workflow_id, "terminate").await
}

/// 将调度记录映射为结构化响应，自然语言规则和解析结果同时保留。
fn schedule_json(row: &Row) -> Value {
    json!({ "id": row.get::<_, String>(0), "workflow_id": row.get::<_, String>(1), "schedule_type": row.get::<_, String>(2), "schedule_expression": row.get::<_, String>(3), "parsed_rule": row.get::<_, Value>(4), "timezone": row.get::<_, String>(5), "enabled": row.get::<_, bool>(6), "next_run_at": row.get::<_, Option<String>>(7), "created_at": row.get::<_, String>(8), "updated_at": row.get::<_, String>(9) })
}

/// 查询工作流的全部调度配置，禁用记录也返回以便用户再次启用。
async fn list_schedules(
    State(state): State<AppState>,
    Path(workflow_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let client = connect(&state).await?;
    let rows = client.query("SELECT id, workflow_id, schedule_type, schedule_expression, parsed_rule, timezone, enabled, to_char(next_run_at, 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"'), to_char(created_at, 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"'), to_char(updated_at, 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"') FROM workflow_schedules WHERE workflow_id = $1 ORDER BY created_at", &[&workflow_id]).await.map_err(ApiError::database)?;
    Ok(Json(
        json!({ "items": rows.iter().map(schedule_json).collect::<Vec<_>>() }),
    ))
}

/// 校验调度类型及 AI 解析规则，AI 调度必须保存非空结构化结果。
fn schedule_fields(
    body: &Value,
) -> Result<(String, String, Value, String, bool, Option<String>), ApiError> {
    let schedule_type = required_text(body, "schedule_type")?;
    if !["periodic", "scheduled", "ai_parsed"].contains(&schedule_type.as_str()) {
        return Err(ApiError::invalid("invalid schedule_type"));
    }
    let expression = required_text(body, "schedule_expression")?;
    let parsed_rule = body
        .get("parsed_rule")
        .cloned()
        .unwrap_or_else(|| json!({}));
    if schedule_type == "ai_parsed"
        && parsed_rule
            .as_object()
            .is_none_or(serde_json::Map::is_empty)
    {
        return Err(ApiError::invalid(
            "ai_parsed schedule requires a structured parsed_rule",
        ));
    }
    let timezone = body
        .get("timezone")
        .and_then(Value::as_str)
        .unwrap_or("Asia/Shanghai")
        .to_owned();
    let enabled = body
        .get("enabled")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let next_run_at = body
        .get("next_run_at")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    Ok((
        schedule_type,
        expression,
        parsed_rule,
        timezone,
        enabled,
        next_run_at,
    ))
}

/// 创建周期、预定时间或 AI 解析调度；只保存结构化规则，不实现错过后自动补跑。
async fn create_schedule(
    State(state): State<AppState>,
    Path(workflow_id): Path<String>,
    Json(body): Json<Value>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let (schedule_type, expression, parsed_rule, timezone, enabled, next_run_at) =
        schedule_fields(&body)?;
    let client = connect(&state).await?;
    let workflow = client
        .query_opt(
            "SELECT status FROM workflows WHERE id = $1",
            &[&workflow_id],
        )
        .await
        .map_err(ApiError::database)?
        .ok_or_else(|| ApiError::not_found("workflow not found"))?;
    if workflow.get::<_, String>(0) == "terminated" {
        return Err(ApiError::conflict(
            "terminated workflow cannot add schedules",
        ));
    }
    let schedule_id = Uuid::new_v4().to_string();
    let row = client.query_one("INSERT INTO workflow_schedules (id, workflow_id, schedule_type, schedule_expression, parsed_rule, timezone, enabled, next_run_at) VALUES ($1, $2, $3, $4, $5, $6, $7, $8::text::timestamptz) RETURNING id, workflow_id, schedule_type, schedule_expression, parsed_rule, timezone, enabled, to_char(next_run_at, 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"'), to_char(created_at, 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"'), to_char(updated_at, 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"')", &[&schedule_id, &workflow_id, &schedule_type, &expression, &parsed_rule, &timezone, &enabled, &next_run_at]).await.map_err(ApiError::database)?;
    Ok((StatusCode::CREATED, Json(schedule_json(&row))))
}

/// 更新调度表达式、结构化规则和下次执行时间，未提供字段保持原值。
async fn update_schedule(
    State(state): State<AppState>,
    Path(schedule_id): Path<String>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let schedule_type = body.get("schedule_type").and_then(Value::as_str);
    if schedule_type.is_some_and(|value| !["periodic", "scheduled", "ai_parsed"].contains(&value)) {
        return Err(ApiError::invalid("invalid schedule_type"));
    }
    let expression = body.get("schedule_expression").and_then(Value::as_str);
    let parsed_rule = body.get("parsed_rule").cloned();
    if schedule_type == Some("ai_parsed")
        && parsed_rule
            .as_ref()
            .and_then(Value::as_object)
            .is_none_or(serde_json::Map::is_empty)
    {
        return Err(ApiError::invalid(
            "ai_parsed schedule requires a structured parsed_rule",
        ));
    }
    let timezone = body.get("timezone").and_then(Value::as_str);
    let enabled = body.get("enabled").and_then(Value::as_bool);
    let next_run_at = body.get("next_run_at").and_then(Value::as_str);
    let client = connect(&state).await?;
    let row = client.query_opt("UPDATE workflow_schedules SET schedule_type = COALESCE($2, schedule_type), schedule_expression = COALESCE($3, schedule_expression), parsed_rule = COALESCE($4, parsed_rule), timezone = COALESCE($5, timezone), enabled = COALESCE($6, enabled), next_run_at = COALESCE($7::text::timestamptz, next_run_at), updated_at = now() WHERE id = $1 RETURNING id, workflow_id, schedule_type, schedule_expression, parsed_rule, timezone, enabled, to_char(next_run_at, 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"'), to_char(created_at, 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"'), to_char(updated_at, 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"')", &[&schedule_id, &schedule_type, &expression, &parsed_rule, &timezone, &enabled, &next_run_at]).await.map_err(ApiError::database)?.ok_or_else(|| ApiError::not_found("workflow schedule not found"))?;
    Ok(Json(schedule_json(&row)))
}

/// 删除尚未需要保留历史的调度配置，不删除已产生运行记录。
async fn delete_schedule(
    State(state): State<AppState>,
    Path(schedule_id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let client = connect(&state).await?;
    if client
        .execute(
            "DELETE FROM workflow_schedules WHERE id = $1",
            &[&schedule_id],
        )
        .await
        .map_err(ApiError::database)?
        == 0
    {
        return Err(ApiError::not_found("workflow schedule not found"));
    }
    Ok(StatusCode::NO_CONTENT)
}

/// 显式启用或停用调度，不修改规则和下次执行时间。
async fn set_schedule_enabled(
    State(state): State<AppState>,
    schedule_id: String,
    enabled: bool,
) -> Result<Json<Value>, ApiError> {
    let client = connect(&state).await?;
    let row = client.query_opt("UPDATE workflow_schedules SET enabled = $2, updated_at = now() WHERE id = $1 RETURNING id, workflow_id, schedule_type, schedule_expression, parsed_rule, timezone, enabled, to_char(next_run_at, 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"'), to_char(created_at, 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"'), to_char(updated_at, 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"')", &[&schedule_id, &enabled]).await.map_err(ApiError::database)?.ok_or_else(|| ApiError::not_found("workflow schedule not found"))?;
    Ok(Json(schedule_json(&row)))
}

/// 启用单条工作流调度。
async fn enable_schedule(
    State(state): State<AppState>,
    Path(schedule_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    set_schedule_enabled(State(state), schedule_id, true).await
}

/// 停用单条工作流调度。
async fn disable_schedule(
    State(state): State<AppState>,
    Path(schedule_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    set_schedule_enabled(State(state), schedule_id, false).await
}

/// 将运行记录行映射为稳定响应，状态变化由控制接口显式推进。
fn run_json(row: &Row) -> Value {
    json!({ "id": row.get::<_, String>(0), "workflow_id": row.get::<_, String>(1), "version_id": row.get::<_, String>(2), "project_id": row.get::<_, String>(3), "status": row.get::<_, String>(4), "trigger_type": row.get::<_, String>(5), "input": row.get::<_, Value>(6), "output": row.get::<_, Value>(7), "error_message": row.get::<_, Option<String>>(8), "started_at": row.get::<_, Option<String>>(9), "finished_at": row.get::<_, Option<String>>(10), "created_at": row.get::<_, String>(11), "updated_at": row.get::<_, String>(12), "parent_task_id": row.get::<_, Option<String>>(13) })
}

/// 查询某个工作流的运行历史，最新运行优先，保留终止和失败记录。
async fn list_workflow_runs(
    State(state): State<AppState>,
    Path(workflow_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let client = connect(&state).await?;
    let rows = client.query("SELECT id, workflow_id, version_id, project_id, status, trigger_type, input, output, error_message, to_char(started_at, 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"'), to_char(finished_at, 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"'), to_char(created_at, 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"'), to_char(updated_at, 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"'), parent_task_id FROM workflow_runs WHERE workflow_id = $1 ORDER BY created_at DESC", &[&workflow_id]).await.map_err(ApiError::database)?;
    Ok(Json(
        json!({ "items": rows.iter().map(run_json).collect::<Vec<_>>() }),
    ))
}

/// 手动创建一次工作流运行，固定当前版本并在同一事务投递 Runner 作业。
async fn create_workflow_run(
    State(state): State<AppState>,
    Path(workflow_id): Path<String>,
    Json(body): Json<Value>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let input = body.get("input").cloned().unwrap_or_else(|| json!({}));
    let run_id = Uuid::new_v4().to_string();
    let mut client = connect(&state).await?;
    let transaction = client.transaction().await.map_err(ApiError::database)?;
    let workflow = transaction
        .query_opt(
            "SELECT project_id, current_version_no, status, name FROM workflows WHERE id = $1 FOR UPDATE",
            &[&workflow_id],
        )
        .await
        .map_err(ApiError::database)?
        .ok_or_else(|| ApiError::not_found("workflow not found"))?;
    if workflow.get::<_, String>(2) != "active" {
        return Err(ApiError::conflict("workflow is not active"));
    }
    let version_id = transaction
        .query_one(
            "SELECT id FROM workflow_versions WHERE workflow_id = $1 AND version_no = $2",
            &[&workflow_id, &workflow.get::<_, i32>(1)],
        )
        .await
        .map_err(ApiError::database)?
        .get::<_, String>(0);
    let project_id = workflow.get::<_, String>(0);
    let workflow_name = workflow.get::<_, String>(3);
    let job_id = Uuid::new_v4().to_string();
    let parent_task_id = Uuid::new_v4().to_string();
    transaction
        .execute(
            "INSERT INTO workflow_runs (id, workflow_id, version_id, project_id, status, trigger_type, input) VALUES ($1, $2, $3, $4, 'queued', 'manual', $5)",
            &[&run_id, &workflow_id, &version_id, &project_id, &input],
        )
        .await
        .map_err(ApiError::database)?;
    transaction.execute("INSERT INTO tasks (id, project_id, title, description, board_stage, plan_status, execution_status, acceptance_status, requires_plan_confirmation, source_type, source_workflow_run_id, workflow_name) VALUES ($1, $2, $3, '工作流运行汇总任务', 'in_progress', 'not_required', 'queued', 'not_started', FALSE, 'workflow_run', $4, $3)", &[&parent_task_id, &project_id, &workflow_name, &run_id]).await.map_err(ApiError::database)?;
    transaction
        .execute(
            "UPDATE workflow_runs SET parent_task_id = $2 WHERE id = $1",
            &[&run_id, &parent_task_id],
        )
        .await
        .map_err(ApiError::database)?;
    transaction.execute("INSERT INTO task_events (task_id, event_type, actor_type, actor_id, after_data, event_data) VALUES ($1, 'workflow.run_created', 'system', 'workflow-control', $2, $3)", &[&parent_task_id, &json!({ "board_stage": "in_progress" }), &json!({ "workflow_id": workflow_id, "workflow_run_id": run_id, "workflow_name": workflow_name })]).await.map_err(ApiError::database)?;
    transaction.execute("INSERT INTO workflow_node_runs (id, run_id, node_key, node_type) SELECT $1 || ':' || node_key, $1, node_key, node_type FROM workflow_nodes WHERE version_id = $2 ON CONFLICT (run_id, node_key) DO NOTHING", &[&run_id, &version_id]).await.map_err(ApiError::database)?;
    transaction
        .execute(
            "INSERT INTO execution_jobs (id, kind, status, project_id, payload) VALUES ($1, 'run_workflow', 'queued', $2, $3)",
            &[&job_id, &project_id, &json!({ "run_id": run_id })],
        )
        .await
        .map_err(ApiError::database)?;
    transaction
        .execute(
            "INSERT INTO workflow_run_events (run_id, event_type, payload) VALUES ($1, 'run.queued', $2)",
            &[&run_id, &json!({ "trigger_type": "manual" })],
        )
        .await
        .map_err(ApiError::database)?;
    transaction.commit().await.map_err(ApiError::database)?;
    Ok((
        StatusCode::ACCEPTED,
        Json(
            json!({ "id": run_id, "workflow_id": workflow_id, "version_id": version_id, "project_id": project_id, "parent_task_id": parent_task_id, "workflow_name": workflow_name, "execution_job_id": job_id, "status": "queued", "trigger_type": "manual", "input": input }),
        ),
    ))
}

/// 查询单次运行当前状态和事件时间线，供运行记录页面展示节点执行过程。
async fn get_workflow_run(
    State(state): State<AppState>,
    Path(run_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let client = connect(&state).await?;
    let row = client.query_opt("SELECT id, workflow_id, version_id, project_id, status, trigger_type, input, output, error_message, to_char(started_at, 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"'), to_char(finished_at, 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"'), to_char(created_at, 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"'), to_char(updated_at, 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"'), parent_task_id FROM workflow_runs WHERE id = $1", &[&run_id]).await.map_err(ApiError::database)?.ok_or_else(|| ApiError::not_found("workflow run not found"))?;
    let events = client.query("SELECT id, event_type, node_key, payload, to_char(created_at, 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"') FROM workflow_run_events WHERE run_id = $1 ORDER BY created_at, id", &[&run_id]).await.map_err(ApiError::database)?;
    let mut response = run_json(&row);
    response["events"] = json!(events.iter().map(|event| json!({ "id": event.get::<_, i64>(0), "event_type": event.get::<_, String>(1), "node_key": event.get::<_, Option<String>>(2), "payload": event.get::<_, Value>(3), "created_at": event.get::<_, String>(4) })).collect::<Vec<_>>());
    Ok(Json(response))
}

/// 查询运行内的节点实例，执行节点通过 task_id 跳转到统一任务详情。
async fn list_node_runs(
    State(state): State<AppState>,
    Path(run_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let client = connect(&state).await?;
    let rows = client.query("SELECT id, run_id, node_key, node_type, status, task_id, attempt_count, input, output, error_message, to_char(started_at, 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"'), to_char(finished_at, 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"'), to_char(created_at, 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"'), to_char(updated_at, 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"') FROM workflow_node_runs WHERE run_id = $1 ORDER BY created_at, node_key", &[&run_id]).await.map_err(ApiError::database)?;
    Ok(Json(
        json!({ "items": rows.iter().map(|row| json!({ "id": row.get::<_, String>(0), "run_id": row.get::<_, String>(1), "node_key": row.get::<_, String>(2), "node_type": row.get::<_, String>(3), "status": row.get::<_, String>(4), "task_id": row.get::<_, Option<String>>(5), "attempt_count": row.get::<_, i32>(6), "input": row.get::<_, Value>(7), "output": row.get::<_, Value>(8), "error_message": row.get::<_, Option<String>>(9), "started_at": row.get::<_, Option<String>>(10), "finished_at": row.get::<_, Option<String>>(11), "created_at": row.get::<_, String>(12), "updated_at": row.get::<_, String>(13) })).collect::<Vec<_>>() }),
    ))
}

/// 查询运行的人工确认请求，包含待处理与已处理记录以支撑完整时间线。
async fn list_approvals(
    State(state): State<AppState>,
    Path(run_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let client = connect(&state).await?;
    let rows = client.query("SELECT id, request_type, workflow_run_id, node_run_id, task_id, execution_job_id, status, prompt, response_data, to_char(requested_at, 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"'), to_char(resolved_at, 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"'), resolved_by FROM approval_requests WHERE workflow_run_id = $1 ORDER BY requested_at", &[&run_id]).await.map_err(ApiError::database)?;
    Ok(Json(
        json!({ "items": rows.iter().map(|row| json!({ "id": row.get::<_, String>(0), "request_type": row.get::<_, String>(1), "workflow_run_id": row.get::<_, Option<String>>(2), "node_run_id": row.get::<_, Option<String>>(3), "task_id": row.get::<_, Option<String>>(4), "execution_job_id": row.get::<_, Option<String>>(5), "status": row.get::<_, String>(6), "prompt": row.get::<_, String>(7), "response_data": row.get::<_, Value>(8), "requested_at": row.get::<_, String>(9), "resolved_at": row.get::<_, Option<String>>(10), "resolved_by": row.get::<_, Option<String>>(11) })).collect::<Vec<_>>() }),
    ))
}

/// 聚合工作流调度作业和执行子任务产生的输出，运行记录可直接查看节点与任务结果。
async fn list_run_outputs(
    State(state): State<AppState>,
    Path(run_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let client = connect(&state).await?;
    if client
        .query_opt("SELECT 1 FROM workflow_runs WHERE id = $1", &[&run_id])
        .await
        .map_err(ApiError::database)?
        .is_none()
    {
        return Err(ApiError::not_found("workflow run not found"));
    }
    let rows = client.query("SELECT ro.id, ro.job_id, ro.task_id, ro.output_type, ro.content, ro.node_run_id, to_char(ro.created_at, 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"') FROM run_outputs ro LEFT JOIN execution_jobs job ON job.id = ro.job_id WHERE ro.workflow_run_id = $1 OR job.payload ->> 'run_id' = $1 OR ro.task_id IN (SELECT id FROM tasks WHERE source_workflow_run_id = $1) ORDER BY ro.created_at", &[&run_id]).await.map_err(ApiError::database)?;
    Ok(Json(
        json!({ "items": rows.iter().map(|row| json!({ "id": row.get::<_, String>(0), "job_id": row.get::<_, String>(1), "task_id": row.get::<_, Option<String>>(2), "output_type": row.get::<_, String>(3), "content": row.get::<_, String>(4), "node_run_id": row.get::<_, Option<String>>(5), "created_at": row.get::<_, String>(6) })).collect::<Vec<_>>() }),
    ))
}

/// 处理人工确认评论，将布尔决定写入运行快照并重新投递工作流作业。
async fn resolve_approval(
    State(state): State<AppState>,
    Path(approval_id): Path<String>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let decision = body
        .get("decision")
        .and_then(Value::as_bool)
        .ok_or_else(|| ApiError::invalid("decision boolean is required"))?;
    let comment = body.get("comment").and_then(Value::as_str).unwrap_or("");
    let mut client = connect(&state).await?;
    let transaction = client.transaction().await.map_err(ApiError::database)?;
    let approval = transaction
        .query_opt(
            "SELECT workflow_run_id, node_run_id, task_id, status FROM approval_requests WHERE id = $1 FOR UPDATE",
            &[&approval_id],
        )
        .await
        .map_err(ApiError::database)?
        .ok_or_else(|| ApiError::not_found("approval request not found"))?;
    if approval.get::<_, String>(3) != "pending" {
        return Err(ApiError::conflict("approval request is already resolved"));
    }
    let run_id = approval
        .get::<_, Option<String>>(0)
        .ok_or_else(|| ApiError::invalid("approval request has no workflow run"))?;
    let node_run_id = approval.get::<_, Option<String>>(1);
    let task_id = approval.get::<_, Option<String>>(2);
    let run = transaction
        .query_opt(
            "SELECT project_id, status FROM workflow_runs WHERE id = $1 FOR UPDATE",
            &[&run_id],
        )
        .await
        .map_err(ApiError::database)?
        .ok_or_else(|| ApiError::not_found("workflow run not found"))?;
    if !["waiting_human", "waiting_approval"].contains(&run.get::<_, String>(1).as_str()) {
        return Err(ApiError::conflict(
            "workflow run is not waiting for approval",
        ));
    }
    let response = json!({ "decision": decision, "comment": comment, "resolved_by": "human" });

    // 审批只能消费一次；运行快照保存决定后重新排队，由 Runner 依据固定版本继续。
    transaction
        .execute(
            "UPDATE approval_requests SET status = 'resolved', response_data = $2, resolved_at = now(), resolved_by = 'human' WHERE id = $1",
            &[&approval_id, &response],
        )
        .await
        .map_err(ApiError::database)?;
    if let Some(node_run_id) = node_run_id.as_ref() {
        transaction
            .execute(
                "UPDATE workflow_node_runs SET status = 'succeeded', output = $2, finished_at = now(), updated_at = now() WHERE id = $1",
                &[node_run_id, &response],
            )
            .await
            .map_err(ApiError::database)?;
    }
    if let Some(task_id) = task_id.as_ref() {
        transaction.execute("UPDATE tasks SET board_stage = 'done', execution_status = 'succeeded', acceptance_status = 'passed', progress_percent = 100, updated_at = now() WHERE id = $1", &[task_id]).await.map_err(ApiError::database)?;
    }
    transaction
        .execute(
            "UPDATE workflow_runs SET status = 'queued', output = jsonb_set(output, ARRAY['approval_results', $2], $3, TRUE), updated_at = now() WHERE id = $1",
            &[&run_id, &approval_id, &response],
        )
        .await
        .map_err(ApiError::database)?;
    let job_id = Uuid::new_v4().to_string();
    transaction
        .execute(
            "INSERT INTO execution_jobs (id, kind, status, project_id, payload) VALUES ($1, 'run_workflow', 'queued', $2, $3)",
            &[&job_id, &run.get::<_, String>(0), &json!({ "run_id": run_id })],
        )
        .await
        .map_err(ApiError::database)?;
    transaction
        .execute(
            "INSERT INTO workflow_run_events (run_id, event_type, payload) VALUES ($1, 'approval.resolved', $2)",
            &[
                &run_id,
                &json!({ "approval_id": approval_id, "decision": decision, "comment": comment, "execution_job_id": job_id }),
            ],
        )
        .await
        .map_err(ApiError::database)?;
    transaction.commit().await.map_err(ApiError::database)?;
    Ok(Json(
        json!({ "id": approval_id, "workflow_run_id": run_id, "status": "resolved", "decision": decision, "comment": comment, "execution_job_id": job_id }),
    ))
}

/// 将运行状态推进到暂停、恢复或终止，并同步控制关联的 Runner 作业。
async fn control_run(
    State(state): State<AppState>,
    run_id: String,
    action: &str,
) -> Result<Json<Value>, ApiError> {
    let mut client = connect(&state).await?;
    let transaction = client.transaction().await.map_err(ApiError::database)?;
    let row = transaction
        .query_opt(
            "SELECT status, project_id FROM workflow_runs WHERE id = $1 FOR UPDATE",
            &[&run_id],
        )
        .await
        .map_err(ApiError::database)?
        .ok_or_else(|| ApiError::not_found("workflow run not found"))?;
    let current = row.get::<_, String>(0);
    let next = match (action, current.as_str()) {
        (
            "pause",
            "queued" | "running" | "waiting_child" | "waiting_human" | "waiting_approval",
        ) => "paused",
        ("resume", "paused") => "queued",
        (
            "terminate",
            "queued" | "running" | "waiting_child" | "waiting_human" | "waiting_approval"
            | "paused",
        ) => "terminated",
        (_, "paused") if action == "pause" => "paused",
        (_, "terminated") if action == "terminate" => "terminated",
        _ => {
            return Err(ApiError::conflict(format!(
                "cannot {action} workflow run from {current}"
            )))
        }
    };
    transaction.execute("UPDATE workflow_runs SET status = $2, started_at = CASE WHEN $2 = 'running' AND started_at IS NULL THEN now() ELSE started_at END, finished_at = CASE WHEN $2 = 'terminated' THEN now() ELSE finished_at END, updated_at = now() WHERE id = $1", &[&run_id, &next]).await.map_err(ApiError::database)?;

    // 队列控制与运行状态处于同一事务，定义级控制不进入此分支，因此只影响当前运行及其派生任务。
    if action == "pause" {
        transaction
            .execute(
                concat!(
                    "UPDATE execution_jobs SET status = 'paused', updated_at = now() WHERE status IN ('queued', 'running') AND ",
                    "((kind = 'run_workflow' AND payload ->> 'run_id' = $1) OR (kind = 'execute_task' AND task_id IN ",
                    "(SELECT id FROM tasks WHERE source_workflow_run_id = $1)))"
                ),
                &[&run_id],
            )
            .await
            .map_err(ApiError::database)?;
        transaction
            .execute(
                concat!(
                    "UPDATE tasks SET execution_status = 'paused', updated_at = now() WHERE source_workflow_run_id = $1 ",
                    "AND board_stage NOT IN ('done', 'cancelled')"
                ),
                &[&run_id],
            )
            .await
            .map_err(ApiError::database)?;
    } else if action == "terminate" {
        transaction
            .execute(
                concat!(
                    "UPDATE execution_jobs SET status = 'cancelled', updated_at = now() WHERE status IN ('queued', 'running', 'paused') AND ",
                    "((kind = 'run_workflow' AND payload ->> 'run_id' = $1) OR (kind = 'execute_task' AND task_id IN ",
                    "(SELECT id FROM tasks WHERE source_workflow_run_id = $1)))"
                ),
                &[&run_id],
            )
            .await
            .map_err(ApiError::database)?;
        transaction
            .execute(
                concat!(
                    "UPDATE tasks SET board_stage = 'cancelled', execution_status = 'cancelled', updated_at = now() ",
                    "WHERE source_workflow_run_id = $1 AND board_stage NOT IN ('done', 'cancelled')"
                ),
                &[&run_id],
            )
            .await
            .map_err(ApiError::database)?;
        transaction
            .execute(
                concat!(
                    "UPDATE workflow_node_runs SET status = 'cancelled', finished_at = now(), updated_at = now() WHERE run_id = $1 ",
                    "AND status NOT IN ('succeeded', 'failed', 'cancelled')"
                ),
                &[&run_id],
            )
            .await
            .map_err(ApiError::database)?;
        transaction
            .execute(
                concat!(
                    "UPDATE approval_requests SET status = 'cancelled', resolved_at = now(), resolved_by = 'system' ",
                    "WHERE workflow_run_id = $1 AND status = 'pending'"
                ),
                &[&run_id],
            )
            .await
            .map_err(ApiError::database)?;
    } else {
        let resumed = transaction
            .execute(
                concat!(
                    "UPDATE execution_jobs SET status = 'queued', available_at = now(), updated_at = now() ",
                    "WHERE kind = 'run_workflow' AND payload ->> 'run_id' = $1 AND status = 'paused'"
                ),
                &[&run_id],
            )
            .await
            .map_err(ApiError::database)?;
        if resumed == 0 {
            transaction
                .execute(
                    "INSERT INTO execution_jobs (id, kind, status, project_id, payload) VALUES ($1, 'run_workflow', 'queued', $2, $3)",
                    &[&Uuid::new_v4().to_string(), &row.get::<_, String>(1), &json!({ "run_id": run_id })],
                )
                .await
                .map_err(ApiError::database)?;
        }
        transaction
            .execute(
                concat!(
                    "UPDATE execution_jobs SET status = 'queued', available_at = now(), updated_at = now() WHERE kind = 'execute_task' ",
                    "AND status = 'paused' AND task_id IN (SELECT id FROM tasks WHERE source_workflow_run_id = $1)"
                ),
                &[&run_id],
            )
            .await
            .map_err(ApiError::database)?;
        transaction
            .execute(
                concat!(
                    "UPDATE tasks SET execution_status = 'queued', updated_at = now() WHERE source_workflow_run_id = $1 ",
                    "AND execution_status = 'paused' AND board_stage NOT IN ('done', 'cancelled')"
                ),
                &[&run_id],
            )
            .await
            .map_err(ApiError::database)?;
    }
    transaction
        .execute(
            "INSERT INTO workflow_run_events (run_id, event_type, payload) VALUES ($1, $2, $3)",
            &[
                &run_id,
                &format!("run.{action}"),
                &json!({ "from": current, "to": next }),
            ],
        )
        .await
        .map_err(ApiError::database)?;
    transaction.commit().await.map_err(ApiError::database)?;
    get_workflow_run(State(state), Path(run_id)).await
}

/// 暂停排队或执行中的工作流运行。
async fn pause_workflow_run(
    State(state): State<AppState>,
    Path(run_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    control_run(State(state), run_id, "pause").await
}

/// 恢复已暂停的工作流运行，恢复后回到排队状态等待后续 Runner 接管。
async fn resume_workflow_run(
    State(state): State<AppState>,
    Path(run_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    control_run(State(state), run_id, "resume").await
}

/// 终止工作流运行，终止是最终状态，不删除运行记录和画布版本。
async fn terminate_workflow_run(
    State(state): State<AppState>,
    Path(run_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    control_run(State(state), run_id, "terminate").await
}
