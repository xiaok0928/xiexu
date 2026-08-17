use crate::{connect, required_text, ApiError, AppState};
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};
use tokio_postgres::Transaction;
use uuid::Uuid;

/// 文档版本差异查询参数，版本号必须显式指定以保证结果可复现。
#[derive(Deserialize)]
struct DiffQuery {
    /// 作为差异基线的旧版本号。
    from: i32,
    /// 作为差异目标的新版本号。
    to: i32,
}

/// 评论中的显式提及目标，服务端不依赖自然语言猜测实体身份。
#[derive(Deserialize)]
struct MentionInput {
    /// 提及目标类型，仅允许 task 或 agent。
    target_type: String,
    /// 目标任务或 Agent 的稳定主键。
    target_id: String,
}

/// 项目上下文路由，集中提供文档生命周期和增强评论协作接口。
pub(crate) fn routes() -> Router<AppState> {
    // 评论路由由本模块接管，确保父评论、提及、依赖状态与原有状态机在同一事务提交。
    Router::new()
        .route(
            "/api/projects/:project_id/documents",
            get(list_project_documents),
        )
        .route("/api/documents/:document_id", get(get_document))
        .route(
            "/api/documents/:document_id/candidates",
            get(list_document_candidates),
        )
        .route(
            "/api/document-candidates/:candidate_id/resolve",
            post(resolve_document_candidate),
        )
        .route(
            "/api/documents/:document_id/versions",
            get(list_document_versions),
        )
        .route(
            "/api/documents/:document_id/diff",
            get(diff_document_versions),
        )
        .route(
            "/api/documents/:document_id/rollback",
            post(rollback_document),
        )
        .route(
            "/api/documents/:document_id/sections/:section_key",
            axum::routing::patch(update_document_section),
        )
        .route(
            "/api/documents/:document_id/refresh",
            post(refresh_document),
        )
        .route(
            "/api/tasks/:task_id/comments",
            get(list_comments).post(create_comment),
        )
        .route("/api/tasks/:task_id/mentions", get(list_task_mentions))
}

/// 在项目创建事务内生成项目概览、默认章节和首个不可变版本。
pub(crate) async fn initialize_project_document(
    transaction: &Transaction<'_>,
    project_id: &str,
    project_name: &str,
    project_description: &str,
) -> Result<(), ApiError> {
    // 创建唯一概览文档，项目创建失败时文档事实会随同一事务回滚。
    let document_id = Uuid::new_v4().to_string();
    transaction
        .execute(
            "INSERT INTO project_documents (id, project_id, doc_type, title) VALUES ($1, $2, 'overview', '项目概览')",
            &[&document_id, &project_id],
        )
        .await
        .map_err(ApiError::database)?;

    // 默认章节提供可持续更新的稳定边界，项目说明只进入目标章节，避免复制到全部内容。
    let goal = if project_description.trim().is_empty() {
        format!("{project_name} 的项目目标待 Human 补充。")
    } else {
        project_description.to_owned()
    };
    let sections = [
        ("goal", "项目目标", goal.as_str(), 10),
        (
            "scope",
            "范围与边界",
            "记录当前确认的范围、明确排除项与兼容边界。",
            20,
        ),
        (
            "progress",
            "交付进展",
            "项目已创建，任务进展将在文档刷新后汇总。",
            30,
        ),
        ("decisions", "关键决策", "尚无已确认决策。", 40),
        ("risks", "风险与待办", "尚无已识别风险。", 50),
    ];
    for (section_key, title, content, sort_order) in sections {
        transaction
            .execute(
                "INSERT INTO project_document_sections (document_id, section_key, title, content, sort_order) VALUES ($1, $2, $3, $4, $5)",
                &[&document_id, &section_key, &title, &content, &sort_order],
            )
            .await
            .map_err(ApiError::database)?;
    }

    // 首版快照与可编辑章节同时落库，使新项目立即具备版本、diff 和回退能力。
    write_document_version(
        transaction,
        &document_id,
        "initial_generation",
        "system",
        None,
    )
    .await?;
    Ok(())
}

/// 在父任务完成事务内幂等创建文档刷新作业，普通子任务完成时不产生额外作业。
pub(crate) async fn enqueue_parent_document_refresh(
    transaction: &Transaction<'_>,
    task_id: &str,
    project_id: &str,
    task_revision: i64,
) -> Result<(), ApiError> {
    // 只有确实包含子任务的父任务触发刷新，并绑定项目协调 Agent 作为执行身份。
    let document = transaction
        .query_opt(
            concat!(
                "SELECT pd.id, pd.current_version_no, pa.agent_id FROM project_documents pd JOIN project_agents pa ",
                "ON pa.project_id = pd.project_id AND pa.assignment_type = 'coordinator' AND pa.status = 'active' ",
                "WHERE pd.project_id = $1 AND pd.doc_type = 'overview' AND pd.status = 'active' ",
                "AND EXISTS (SELECT 1 FROM tasks child WHERE child.parent_task_id = $2) FOR UPDATE OF pd"
            ),
            &[&project_id, &task_id],
        )
        .await
        .map_err(ApiError::database)?;
    let Some(document) = document else {
        return Ok(());
    };
    let document_id = document.get::<_, String>(0);
    let base_version_no = document.get::<_, i32>(1);
    let agent_id = document.get::<_, String>(2);
    let refresh_job_id = Uuid::new_v4().to_string();
    let dedupe_key =
        format!("document:{document_id}:refresh:parent:{task_id}:revision:{task_revision}");

    // 作业、刷新水位和任务事件与父任务完成原子提交，但实际文档生成保持异步。
    transaction
        .execute(
            concat!(
                "INSERT INTO execution_jobs (id, kind, status, task_id, project_id, agent_id, payload, dedupe_key) ",
                "VALUES ($1, 'refresh_project_document', 'queued', $2, $3, $4, $5, $6) ",
                "ON CONFLICT (dedupe_key) DO NOTHING"
            ),
            &[
                &refresh_job_id,
                &task_id,
                &project_id,
                &agent_id,
                &json!({
                    "document_id": document_id, "project_id": project_id, "source_task_id": task_id,
                    "base_version_no": base_version_no, "trigger_type": "parent_task_completed"
                }),
                &dedupe_key,
            ],
        )
        .await
        .map_err(ApiError::database)?;
    transaction
        .execute(
            "UPDATE projects SET document_refresh_requested_at = now() WHERE id = $1",
            &[&project_id],
        )
        .await
        .map_err(ApiError::database)?;
    transaction
        .execute(
            concat!(
                "INSERT INTO task_events (task_id, event_type, actor_type, actor_id, event_data) ",
                "VALUES ($1, 'document.refresh_queued', 'system', 'control-plane', $2)"
            ),
            &[&task_id, &json!({
                "job_id": refresh_job_id, "document_id": document_id, "trigger_type": "parent_task_completed"
            })],
        )
        .await
        .map_err(ApiError::database)?;
    Ok(())
}

/// 把当前章节序列化为不可变文档版本，并原子推进文档版本号。
async fn write_document_version(
    transaction: &Transaction<'_>,
    document_id: &str,
    source_type: &str,
    actor_id: &str,
    rollback_from_version_no: Option<i32>,
) -> Result<i32, ApiError> {
    // 按稳定顺序读取章节，保证相同内容生成相同快照与 hash。
    let rows = transaction
        .query(
            concat!(
                "SELECT section_key, title, content, sort_order, locked_by_human, revision ",
                "FROM project_document_sections WHERE document_id = $1 ORDER BY sort_order, section_key"
            ),
            &[&document_id],
        )
        .await
        .map_err(ApiError::database)?;
    let sections = rows
        .iter()
        .map(|row| {
            json!({
                "section_key": row.get::<_, String>(0),
                "title": row.get::<_, String>(1),
                "content": row.get::<_, String>(2),
                "sort_order": row.get::<_, i32>(3),
                "locked_by_human": row.get::<_, bool>(4),
                "revision": row.get::<_, i64>(5)
            })
        })
        .collect::<Vec<_>>();
    let content = serde_json::to_string(&json!({ "sections": sections }))
        .map_err(|_| ApiError::invalid("document snapshot cannot be serialized"))?;
    let content_hash = transaction
        .query_one("SELECT md5($1)", &[&content])
        .await
        .map_err(ApiError::database)?
        .get::<_, String>(0);

    // 锁定文档主记录并推进单调版本号，并发更新不会生成重复 version_no。
    let version_no = transaction
        .query_one(
            concat!(
                "UPDATE project_documents SET current_version_no = current_version_no + 1, revision = revision + 1, ",
                "last_refreshed_at = now(), updated_at = now() WHERE id = $1 RETURNING current_version_no"
            ),
            &[&document_id],
        )
        .await
        .map_err(ApiError::database)?
        .get::<_, i32>(0);
    transaction
        .execute(
            concat!(
                "INSERT INTO project_document_versions (id, document_id, version_no, content, content_hash, source_type, ",
                "created_by_actor_id, rollback_from_version_no) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)"
            ),
            &[&Uuid::new_v4().to_string(), &document_id, &version_no, &content, &content_hash, &source_type, &actor_id, &rollback_from_version_no],
        )
        .await
        .map_err(ApiError::database)?;
    Ok(version_no)
}

/// 查询项目全部有效文档及版本、章节和待处理候选摘要。
async fn list_project_documents(
    State(state): State<AppState>,
    Path(project_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    // 先确认项目存在，避免把不存在项目与空文档列表混为一谈。
    let client = connect(&state).await?;
    if client
        .query_opt("SELECT 1 FROM projects WHERE id = $1", &[&project_id])
        .await
        .map_err(ApiError::database)?
        .is_none()
    {
        return Err(ApiError::not_found("project not found"));
    }

    // 聚合计数由数据库批量计算，避免按文档逐条查询章节和候选。
    let rows = client
        .query(
            "SELECT pd.id, pd.project_id, pd.doc_type, pd.title, pd.revision, pd.current_version_no, pd.status,
               (SELECT count(*) FROM project_document_sections s WHERE s.document_id = pd.id),
               (SELECT count(*) FROM project_document_update_candidates c WHERE c.document_id = pd.id AND c.status IN ('pending', 'conflict')),
               to_char(pd.last_refreshed_at, 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"'), to_char(pd.updated_at, 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"')
             FROM project_documents pd WHERE pd.project_id = $1 AND pd.status = 'active' ORDER BY pd.created_at, pd.id",
            &[&project_id],
        )
        .await
        .map_err(ApiError::database)?;
    let items = rows
        .iter()
        .map(|row| {
            json!({
                "id": row.get::<_, String>(0), "project_id": row.get::<_, String>(1), "doc_type": row.get::<_, String>(2), "title": row.get::<_, String>(3),
                "revision": row.get::<_, i64>(4), "current_version_no": row.get::<_, i32>(5), "status": row.get::<_, String>(6),
                "section_count": row.get::<_, i64>(7), "pending_candidate_count": row.get::<_, i64>(8),
                "last_refreshed_at": row.get::<_, Option<String>>(9), "updated_at": row.get::<_, String>(10)
            })
        })
        .collect::<Vec<_>>();
    Ok(Json(json!({ "items": items })))
}

/// 查询单个文档的当前章节和候选更新，候选不会被静默应用。
async fn get_document(
    State(state): State<AppState>,
    Path(document_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    // 读取文档元数据并明确区分资源不存在。
    let client = connect(&state).await?;
    let document = client
        .query_opt(
            concat!(
                "SELECT id, project_id, doc_type, title, revision, current_version_no, status, ",
                "to_char(last_refreshed_at, 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"'), ",
                "to_char(updated_at, 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"') FROM project_documents WHERE id = $1"
            ),
            &[&document_id],
        )
        .await
        .map_err(ApiError::database)?
        .ok_or_else(|| ApiError::not_found("document not found"))?;

    // 章节和候选分别批量读取，锁定章节的候选仍完整返回供 Human 判断。
    let section_rows = client
        .query(
            concat!(
                "SELECT section_key, title, content, sort_order, locked_by_human, revision, ",
                "to_char(updated_at, 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"') FROM project_document_sections ",
                "WHERE document_id = $1 ORDER BY sort_order, section_key"
            ),
            &[&document_id],
        )
        .await
        .map_err(ApiError::database)?;
    let candidate_rows = client
        .query(
            concat!(
                "SELECT id, section_key, proposed_content, source_type, source_id, base_section_revision, status, conflict_reason, ",
                "to_char(created_at, 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"'), ",
                "to_char(resolved_at, 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"') FROM project_document_update_candidates ",
                "WHERE document_id = $1 ORDER BY created_at DESC LIMIT 200"
            ),
            &[&document_id],
        )
        .await
        .map_err(ApiError::database)?;
    let sections = section_rows
        .iter()
        .map(|row| json!({
            "section_key": row.get::<_, String>(0), "title": row.get::<_, String>(1), "content": row.get::<_, String>(2),
            "sort_order": row.get::<_, i32>(3), "locked_by_human": row.get::<_, bool>(4), "revision": row.get::<_, i64>(5),
            "updated_at": row.get::<_, String>(6)
        }))
        .collect::<Vec<_>>();
    let candidates = candidate_rows
        .iter()
        .map(|row| json!({
            "id": row.get::<_, String>(0), "section_key": row.get::<_, String>(1), "proposed_content": row.get::<_, String>(2),
            "source_type": row.get::<_, String>(3), "source_id": row.get::<_, Option<String>>(4),
            "base_section_revision": row.get::<_, i64>(5), "status": row.get::<_, String>(6),
            "conflict_reason": row.get::<_, Option<String>>(7), "created_at": row.get::<_, String>(8),
            "resolved_at": row.get::<_, Option<String>>(9)
        }))
        .collect::<Vec<_>>();
    Ok(Json(json!({
        "id": document.get::<_, String>(0), "project_id": document.get::<_, String>(1), "doc_type": document.get::<_, String>(2), "title": document.get::<_, String>(3),
        "revision": document.get::<_, i64>(4), "current_version_no": document.get::<_, i32>(5), "status": document.get::<_, String>(6),
        "last_refreshed_at": document.get::<_, Option<String>>(7), "updated_at": document.get::<_, String>(8), "sections": sections, "candidates": candidates
    })))
}

/// 查询文档候选更新及冲突状态，供 Human 独立刷新候选列表。
async fn list_document_candidates(
    State(state): State<AppState>,
    Path(document_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    // 文档存在性与候选空列表分开表达，避免错误 ID 被误判为暂无候选。
    let client = connect(&state).await?;
    if client
        .query_opt(
            "SELECT 1 FROM project_documents WHERE id = $1",
            &[&document_id],
        )
        .await
        .map_err(ApiError::database)?
        .is_none()
    {
        return Err(ApiError::not_found("document not found"));
    }

    // 返回所有状态便于审计，调用方可在本地筛选 pending 或 conflict。
    let rows = client
        .query(
            concat!(
                "SELECT id, section_key, proposed_content, source_type, source_id, base_section_revision, status, conflict_reason, ",
                "to_char(created_at, 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"'), ",
                "to_char(resolved_at, 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"') FROM project_document_update_candidates ",
                "WHERE document_id = $1 ORDER BY created_at DESC LIMIT 200"
            ),
            &[&document_id],
        )
        .await
        .map_err(ApiError::database)?;
    let items = rows
        .iter()
        .map(|row| json!({
            "id": row.get::<_, String>(0), "section_key": row.get::<_, String>(1), "proposed_content": row.get::<_, String>(2),
            "source_type": row.get::<_, String>(3), "source_id": row.get::<_, Option<String>>(4),
            "base_section_revision": row.get::<_, i64>(5), "status": row.get::<_, String>(6),
            "conflict_reason": row.get::<_, Option<String>>(7), "created_at": row.get::<_, String>(8),
            "resolved_at": row.get::<_, Option<String>>(9)
        }))
        .collect::<Vec<_>>();
    Ok(Json(json!({ "items": items })))
}

/// 接受或拒绝一个候选更新，接受时通过 section revision 避免静默覆盖。
async fn resolve_document_candidate(
    State(state): State<AppState>,
    Path(candidate_id): Path<String>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    // action 使用显式枚举，避免把任意状态直接写入候选事实。
    let action = required_text(&body, "action")?;
    if action != "accept" && action != "reject" {
        return Err(ApiError::invalid("action must be accept or reject"));
    }
    let mut client = connect(&state).await?;
    let transaction = client.transaction().await.map_err(ApiError::database)?;

    // 先锁定候选并读取其文档归属，候选 ID 是 resolve API 的唯一资源标识。
    let candidate = transaction
        .query_opt(
            "SELECT document_id, section_key, proposed_content, base_section_revision, status FROM project_document_update_candidates WHERE id = $1 FOR UPDATE",
            &[&candidate_id],
        )
        .await
        .map_err(ApiError::database)?
        .ok_or_else(|| ApiError::not_found("document candidate not found"))?;
    let document_id = candidate.get::<_, String>(0);
    let current_status = candidate.get::<_, String>(4);
    if current_status != "pending" && current_status != "conflict" {
        return Err(ApiError::conflict("document candidate is already resolved"));
    }

    // 文档锁与候选锁顺序固定为候选后文档，所有候选 resolve 请求遵守同一路径以避免互锁。
    if transaction
        .query_opt(
            "SELECT 1 FROM project_documents WHERE id = $1 FOR UPDATE",
            &[&document_id],
        )
        .await
        .map_err(ApiError::database)?
        .is_none()
    {
        return Err(ApiError::not_found("document not found"));
    }

    // 拒绝只关闭候选，不改变当前文档，因此无需制造内容版本。
    if action == "reject" {
        transaction
            .execute("UPDATE project_document_update_candidates SET status = 'rejected', resolved_at = now() WHERE id = $1", &[&candidate_id])
            .await
            .map_err(ApiError::database)?;
        transaction.commit().await.map_err(ApiError::database)?;
        return Ok(Json(
            json!({ "id": candidate_id, "document_id": document_id, "status": "rejected" }),
        ));
    }

    // 接受前再次核对候选基线；章节锁只限制 Agent 自动覆盖，不限制 Human 明确采用候选。
    let section_key = candidate.get::<_, String>(1);
    let proposed_content = candidate.get::<_, String>(2);
    let base_section_revision = candidate.get::<_, i64>(3);
    let section = transaction
        .query_opt("SELECT revision FROM project_document_sections WHERE document_id = $1 AND section_key = $2 FOR UPDATE", &[&document_id, &section_key])
        .await
        .map_err(ApiError::database)?
        .ok_or_else(|| ApiError::not_found("document section not found"))?;
    if section.get::<_, i64>(0) != base_section_revision {
        let reason = "section_revision_changed";
        transaction
            .execute(
                "UPDATE project_document_update_candidates SET status = 'conflict', conflict_reason = $2 WHERE id = $1",
                &[&candidate_id, &reason],
            )
            .await
            .map_err(ApiError::database)?;
        transaction.commit().await.map_err(ApiError::database)?;
        return Err(ApiError::conflict(reason));
    }

    // 无冲突时应用候选、关闭候选并追加不可变版本，三个事实原子提交。
    transaction
        .execute(
            "UPDATE project_document_sections SET content = $3, revision = revision + 1, updated_at = now() WHERE document_id = $1 AND section_key = $2",
            &[&document_id, &section_key, &proposed_content],
        )
        .await
        .map_err(ApiError::database)?;
    transaction
        .execute(
            "UPDATE project_document_update_candidates SET status = 'accepted', conflict_reason = NULL, resolved_at = now() WHERE id = $1",
            &[&candidate_id],
        )
        .await
        .map_err(ApiError::database)?;
    let version_no = write_document_version(
        &transaction,
        &document_id,
        "candidate_accepted",
        "human",
        None,
    )
    .await?;
    transaction.commit().await.map_err(ApiError::database)?;
    Ok(Json(
        json!({ "id": candidate_id, "document_id": document_id, "section_key": section_key, "status": "accepted", "version_no": version_no }),
    ))
}

/// 查询文档不可变版本历史，正文由详情和 diff 接口按需读取。
async fn list_document_versions(
    State(state): State<AppState>,
    Path(document_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    // 资源存在性单独校验，使空历史只代表异常旧数据而非错误 ID。
    let client = connect(&state).await?;
    if client
        .query_opt(
            "SELECT 1 FROM project_documents WHERE id = $1",
            &[&document_id],
        )
        .await
        .map_err(ApiError::database)?
        .is_none()
    {
        return Err(ApiError::not_found("document not found"));
    }

    // 版本按新到旧返回，hash 和来源信息支持调用方审计。
    let rows = client
        .query(
            concat!(
                "SELECT id, version_no, content_hash, source_type, created_by_actor_id, source_task_id, rollback_from_version_no, metadata, ",
                "to_char(created_at, 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"') FROM project_document_versions ",
                "WHERE document_id = $1 ORDER BY version_no DESC"
            ),
            &[&document_id],
        )
        .await
        .map_err(ApiError::database)?;
    let items = rows
        .iter()
        .map(|row| json!({
            "id": row.get::<_, String>(0), "version_no": row.get::<_, i32>(1), "content_hash": row.get::<_, String>(2),
            "source_type": row.get::<_, String>(3), "created_by_actor_id": row.get::<_, String>(4),
            "source_task_id": row.get::<_, Option<String>>(5), "rollback_from_version_no": row.get::<_, Option<i32>>(6),
            "metadata": row.get::<_, Value>(7), "created_at": row.get::<_, String>(8)
        }))
        .collect::<Vec<_>>();
    Ok(Json(json!({ "items": items })))
}

/// 将版本快照解析为按章节键索引的结构，兼容检查集中在版本操作入口。
fn snapshot_sections(content: &str) -> Result<BTreeMap<String, Value>, ApiError> {
    // 旧版纯文本内容无法可靠回退为章节，明确报冲突而不是猜测拆分规则。
    let snapshot: Value = serde_json::from_str(content)
        .map_err(|_| ApiError::conflict("document version is not a structured section snapshot"))?;
    let sections = snapshot
        .get("sections")
        .and_then(Value::as_array)
        .ok_or_else(|| ApiError::conflict("document version has no sections"))?;
    let mut mapped = BTreeMap::new();
    for section in sections {
        let section_key = section
            .get("section_key")
            .and_then(Value::as_str)
            .ok_or_else(|| ApiError::conflict("document version contains an invalid section"))?;
        mapped.insert(section_key.to_owned(), section.clone());
    }
    Ok(mapped)
}

/// 比较两个不可变版本并返回章节级 added、removed、modified 差异。
async fn diff_document_versions(
    State(state): State<AppState>,
    Path(document_id): Path<String>,
    Query(query): Query<DiffQuery>,
) -> Result<Json<Value>, ApiError> {
    if query.from == query.to {
        return Err(ApiError::invalid("from and to must be different versions"));
    }

    // 一次查询同时获取两个版本，缺少任一版本都按资源不存在处理。
    let client = connect(&state).await?;
    let rows = client
        .query(
            "SELECT version_no, content FROM project_document_versions WHERE document_id = $1 AND version_no = ANY($2) ORDER BY version_no",
            &[&document_id, &vec![query.from, query.to]],
        )
        .await
        .map_err(ApiError::database)?;
    if rows.len() != 2 {
        return Err(ApiError::not_found("document version not found"));
    }
    let contents = rows
        .iter()
        .map(|row| (row.get::<_, i32>(0), row.get::<_, String>(1)))
        .collect::<HashMap<_, _>>();
    let before = snapshot_sections(contents.get(&query.from).expect("validated from version"))?;
    let after = snapshot_sections(contents.get(&query.to).expect("validated to version"))?;

    // 合并章节键后只返回实际变化，调用方无需过滤未变化章节。
    let mut keys = before
        .keys()
        .chain(after.keys())
        .cloned()
        .collect::<Vec<_>>();
    keys.sort_unstable();
    keys.dedup();
    let changes = keys
        .into_iter()
        .filter_map(|section_key| {
            let old = before.get(&section_key);
            let new = after.get(&section_key);
            if old == new { return None; }
            let change_type = if old.is_none() { "added" } else if new.is_none() { "removed" } else { "modified" };
            Some(json!({ "section_key": section_key, "change_type": change_type, "before": old, "after": new }))
        })
        .collect::<Vec<_>>();
    Ok(Json(
        json!({ "document_id": document_id, "from": query.from, "to": query.to, "changes": changes }),
    ))
}

/// 将历史版本恢复为当前章节，并把回退结果追加为新版本。
async fn rollback_document(
    State(state): State<AppState>,
    Path(document_id): Path<String>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let version_no = body
        .get("version_no")
        .and_then(Value::as_i64)
        .and_then(|value| i32::try_from(value).ok())
        .ok_or_else(|| ApiError::invalid("version_no is required"))?;
    let mut client = connect(&state).await?;
    let transaction = client.transaction().await.map_err(ApiError::database)?;

    // 锁定文档并读取目标快照，禁止并发 section 更新与回退互相覆盖。
    let current_version = transaction
        .query_opt(
            "SELECT current_version_no FROM project_documents WHERE id = $1 FOR UPDATE",
            &[&document_id],
        )
        .await
        .map_err(ApiError::database)?
        .ok_or_else(|| ApiError::not_found("document not found"))?
        .get::<_, i32>(0);
    if current_version == version_no {
        return Err(ApiError::conflict("requested version is already current"));
    }
    let content = transaction
        .query_opt("SELECT content FROM project_document_versions WHERE document_id = $1 AND version_no = $2", &[&document_id, &version_no])
        .await
        .map_err(ApiError::database)?
        .ok_or_else(|| ApiError::not_found("document version not found"))?
        .get::<_, String>(0);
    let sections = snapshot_sections(&content)?;

    // 保存当前单调 revision，回退后仍要让所有旧候选基线失效，不能恢复历史 revision 数值。
    let current_revision_rows = transaction
        .query(
            "SELECT section_key, revision FROM project_document_sections WHERE document_id = $1",
            &[&document_id],
        )
        .await
        .map_err(ApiError::database)?;
    let current_revisions = current_revision_rows
        .iter()
        .map(|row| (row.get::<_, String>(0), row.get::<_, i64>(1)))
        .collect::<HashMap<_, _>>();

    // 历史快照是完整状态，先清空当前章节再按快照恢复，避免残留后续新增章节。
    transaction
        .execute(
            "DELETE FROM project_document_sections WHERE document_id = $1",
            &[&document_id],
        )
        .await
        .map_err(ApiError::database)?;
    for (section_key, section) in sections {
        let title = section
            .get("title")
            .and_then(Value::as_str)
            .ok_or_else(|| ApiError::conflict("document version contains an invalid title"))?;
        let section_content = section
            .get("content")
            .and_then(Value::as_str)
            .ok_or_else(|| ApiError::conflict("document version contains invalid content"))?;
        let sort_order = section
            .get("sort_order")
            .and_then(Value::as_i64)
            .and_then(|value| i32::try_from(value).ok())
            .unwrap_or(0);
        let locked_by_human = section
            .get("locked_by_human")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let historical_revision = section.get("revision").and_then(Value::as_i64).unwrap_or(0);
        let revision = current_revisions
            .get(&section_key)
            .copied()
            .unwrap_or(historical_revision)
            .max(historical_revision)
            + 1;
        transaction
            .execute(
                concat!(
                    "INSERT INTO project_document_sections ",
                    "(document_id, section_key, title, content, sort_order, locked_by_human, revision) ",
                    "VALUES ($1, $2, $3, $4, $5, $6, $7)"
                ),
                &[&document_id, &section_key, &title, &section_content, &sort_order, &locked_by_human, &revision],
            )
            .await
            .map_err(ApiError::database)?;
    }

    // 回退不删除后续历史，而是生成标明来源版本的新版本。
    let new_version_no = write_document_version(
        &transaction,
        &document_id,
        "rollback",
        "human",
        Some(version_no),
    )
    .await?;
    transaction.commit().await.map_err(ApiError::database)?;
    Ok(Json(
        json!({ "document_id": document_id, "version_no": new_version_no, "rollback_from_version_no": version_no }),
    ))
}

/// 更新 Human 编辑内容或章节锁定状态，并生成可审计的新版本。
async fn update_document_section(
    State(state): State<AppState>,
    Path((document_id, section_key)): Path<(String, String)>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    // PATCH 至少包含一个可变字段，空请求不制造无意义版本。
    let content = body.get("content").and_then(Value::as_str);
    let locked_by_human = body.get("locked_by_human").and_then(Value::as_bool);
    if content.is_none() && locked_by_human.is_none() {
        return Err(ApiError::invalid("content or locked_by_human is required"));
    }
    let mut client = connect(&state).await?;
    let transaction = client.transaction().await.map_err(ApiError::database)?;

    // 锁定文档主记录串行化版本号，并由 Human 显式编辑覆盖当前章节。
    if transaction
        .query_opt(
            "SELECT 1 FROM project_documents WHERE id = $1 FOR UPDATE",
            &[&document_id],
        )
        .await
        .map_err(ApiError::database)?
        .is_none()
    {
        return Err(ApiError::not_found("document not found"));
    }
    let row = transaction
        .query_opt(
            concat!(
                "UPDATE project_document_sections SET content = COALESCE($3, content), ",
                "locked_by_human = COALESCE($4, locked_by_human), revision = revision + 1, updated_at = now() ",
                "WHERE document_id = $1 AND section_key = $2 RETURNING title, content, sort_order, locked_by_human, revision, ",
                "to_char(updated_at, 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"')"
            ),
            &[&document_id, &section_key, &content, &locked_by_human],
        )
        .await
        .map_err(ApiError::database)?
        .ok_or_else(|| ApiError::not_found("document section not found"))?;

    // 章节事实更新和版本追加保持原子，客户端永远不会看到缺少历史的当前内容。
    let version_no =
        write_document_version(&transaction, &document_id, "human_edit", "human", None).await?;
    transaction.commit().await.map_err(ApiError::database)?;
    Ok(Json(json!({
        "document_id": document_id, "section_key": section_key, "title": row.get::<_, String>(0), "content": row.get::<_, String>(1),
        "sort_order": row.get::<_, i32>(2), "locked_by_human": row.get::<_, bool>(3), "revision": row.get::<_, i64>(4),
        "updated_at": row.get::<_, String>(5), "document_version_no": version_no
    })))
}

/// 手动请求异步刷新文档，只负责可靠入队，不阻塞任务主流程。
async fn refresh_document(
    State(state): State<AppState>,
    Path(document_id): Path<String>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let mut client = connect(&state).await?;
    let transaction = client.transaction().await.map_err(ApiError::database)?;

    // 锁定文档并获取项目、当前版本和协调 Agent，作业从入队起就具备完整执行归属。
    let row = transaction
        .query_opt(
            concat!(
                "SELECT pd.project_id, pd.current_version_no, pa.agent_id FROM project_documents pd ",
                "JOIN project_agents pa ON pa.project_id = pd.project_id AND pa.assignment_type = 'coordinator' AND pa.status = 'active' ",
                "WHERE pd.id = $1 AND pd.status = 'active' FOR UPDATE OF pd"
            ),
            &[&document_id],
        )
        .await
        .map_err(ApiError::database)?
        .ok_or_else(|| ApiError::not_found("document not found"))?;
    let project_id = row.get::<_, String>(0);
    let base_version_no = row.get::<_, i32>(1);
    let agent_id = row.get::<_, String>(2);
    let job_id = Uuid::new_v4().to_string();

    // 手动刷新每次都创建独立作业，多个来源最终由 section revision 和候选冲突规则保护。
    transaction
        .execute(
            "INSERT INTO execution_jobs (id, kind, status, project_id, agent_id, payload) VALUES ($1, 'refresh_project_document', 'queued', $2, $3, $4)",
            &[
                &job_id,
                &project_id,
                &agent_id,
                &json!({
                    "document_id": document_id.clone(), "project_id": project_id.clone(),
                    "base_version_no": base_version_no, "trigger_type": "manual"
                }),
            ],
        )
        .await
        .map_err(ApiError::database)?;
    transaction
        .execute(
            "UPDATE projects SET document_refresh_requested_at = now() WHERE id = $1",
            &[&project_id],
        )
        .await
        .map_err(ApiError::database)?;
    transaction.commit().await.map_err(ApiError::database)?;
    Ok((
        StatusCode::ACCEPTED,
        Json(
            json!({ "job_id": job_id, "document_id": document_id, "status": "queued", "base_version_no": base_version_no }),
        ),
    ))
}

/// 查询任务评论、父评论关系和显式提及，按追加顺序返回完整协作链。
async fn list_comments(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let client = connect(&state).await?;

    // 先确认任务存在，再一次性读取全部评论，空数组只表示当前尚无评论。
    if client
        .query_opt("SELECT 1 FROM tasks WHERE id = $1", &[&task_id])
        .await
        .map_err(ApiError::database)?
        .is_none()
    {
        return Err(ApiError::not_found("task not found"));
    }
    let rows = client
        .query(
            concat!(
                "SELECT id, parent_comment_id, author_type, author_name, content, intent, transition_applied, ",
                "to_char(created_at, 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"') FROM task_comments ",
                "WHERE task_id = $1 ORDER BY created_at ASC, id ASC"
            ),
            &[&task_id],
        )
        .await
        .map_err(ApiError::database)?;

    // 批量读取评论提及并按 comment_id 分组，避免评论列表产生 N+1 查询。
    let mention_rows = client
        .query(
            "SELECT m.comment_id, m.id, m.target_type, m.target_id, m.status, m.resolved_by_comment_id,
               CASE WHEN m.target_type = 'task' THEN (SELECT title FROM tasks WHERE id = m.target_id)
                    WHEN m.target_type = 'agent' THEN (SELECT name FROM agents WHERE id = m.target_id) END,
               to_char(m.created_at, 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"'), to_char(m.resolved_at, 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"')
             FROM task_mentions m WHERE m.source_task_id = $1 ORDER BY m.created_at, m.id",
            &[&task_id],
        )
        .await
        .map_err(ApiError::database)?;
    let mut mentions_by_comment = HashMap::<String, Vec<Value>>::new();
    for row in mention_rows {
        mentions_by_comment.entry(row.get::<_, String>(0)).or_default().push(json!({
            "id": row.get::<_, String>(1), "target_type": row.get::<_, String>(2), "target_id": row.get::<_, String>(3),
            "status": row.get::<_, String>(4), "resolved_by_comment_id": row.get::<_, Option<String>>(5), "target_name": row.get::<_, Option<String>>(6),
            "created_at": row.get::<_, String>(7), "resolved_at": row.get::<_, Option<String>>(8)
        }));
    }

    // 评论响应内联 mentions，前端可直接还原回复树和依赖状态。
    let items = rows
        .iter()
        .map(|row| {
            let comment_id = row.get::<_, String>(0);
            json!({
                "id": comment_id, "parent_comment_id": row.get::<_, Option<String>>(1), "author_type": row.get::<_, String>(2),
                "author_name": row.get::<_, String>(3), "content": row.get::<_, String>(4), "intent": row.get::<_, String>(5),
                "transition_applied": row.get::<_, bool>(6), "created_at": row.get::<_, String>(7),
                "mentions": mentions_by_comment.remove(&comment_id).unwrap_or_default()
            })
        })
        .collect::<Vec<_>>();
    Ok(Json(json!({ "items": items })))
}

/// 校验评论提及目标与来源任务属于同一项目，防止跨项目隐式建立依赖。
async fn validate_mention_target(
    transaction: &Transaction<'_>,
    project_id: &str,
    source_task_id: &str,
    mention: &MentionInput,
) -> Result<(), ApiError> {
    // 任务提及必须指向同项目的其他任务，避免自依赖和跨项目数据泄漏。
    if mention.target_type == "task" {
        if mention.target_id == source_task_id {
            return Err(ApiError::invalid(
                "task cannot mention itself as a dependency",
            ));
        }
        let target_project = transaction
            .query_opt(
                "SELECT project_id FROM tasks WHERE id = $1",
                &[&mention.target_id],
            )
            .await
            .map_err(ApiError::database)?
            .ok_or_else(|| {
                ApiError::invalid(format!("mentioned task not found: {}", mention.target_id))
            })?
            .get::<_, String>(0);
        if target_project != project_id {
            return Err(ApiError::invalid(
                "mentioned task must belong to the same project",
            ));
        }
        return Ok(());
    }

    // Agent 提及只允许项目固定成员或该项目任务的动态参与者，避免无上下文通知全局 Agent。
    if mention.target_type == "agent" {
        let exists = transaction
            .query_opt(
                "SELECT 1 FROM agents a WHERE a.id = $1 AND a.status = 'active' AND
                   (EXISTS (SELECT 1 FROM project_agents pa WHERE pa.project_id = $2 AND pa.agent_id = a.id AND pa.status = 'active') OR
                    EXISTS (SELECT 1 FROM task_agents ta JOIN tasks t ON t.id = ta.task_id WHERE t.project_id = $2 AND ta.agent_id = a.id AND ta.status = 'active'))",
                &[&mention.target_id, &project_id],
            )
            .await
            .map_err(ApiError::database)?
            .is_some();
        if !exists {
            return Err(ApiError::invalid(format!(
                "mentioned project agent not found: {}",
                mention.target_id
            )));
        }
        return Ok(());
    }
    Err(ApiError::invalid(
        "mention target_type must be task or agent",
    ))
}

/// 保存增强评论，并原子处理状态机、提及依赖、等待与解除。
async fn create_comment(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
    Json(body): Json<Value>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    // 校验评论基础字段和显式协作载荷，未知意图仍拒绝进入状态机。
    let content = required_text(&body, "content")?;
    let intent = body.get("intent").and_then(Value::as_str).unwrap_or("note");
    let allowed = [
        "note",
        "approve_plan",
        "reject_plan",
        "accept",
        "rework",
        "mention",
    ];
    if !allowed.contains(&intent) {
        return Err(ApiError::invalid("unsupported intent hint"));
    }
    let parent_comment_id = body
        .get("parent_comment_id")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    let resolves_mention_id = body
        .get("resolves_mention_id")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    let mentions = body
        .get("mentions")
        .cloned()
        .map(serde_json::from_value::<Vec<MentionInput>>)
        .transpose()
        .map_err(|_| ApiError::invalid("mentions must contain target_type and target_id"))?
        .unwrap_or_default();
    if mentions
        .iter()
        .any(|mention| mention.target_id.trim().is_empty())
    {
        return Err(ApiError::invalid("mention target_id cannot be empty"));
    }

    // 锁定任务当前状态，评论转换、等待状态和作业创建必须在同一事务内完成。
    let mut client = connect(&state).await?;
    let transaction = client.transaction().await.map_err(ApiError::database)?;
    let current = transaction
        .query_opt(
            "SELECT board_stage, revision, project_id FROM tasks WHERE id = $1 FOR UPDATE",
            &[&task_id],
        )
        .await
        .map_err(ApiError::database)?
        .ok_or_else(|| ApiError::not_found("task not found"))?;
    let current_stage = current.get::<_, String>(0);
    let current_revision = current.get::<_, i64>(1);
    let project_id = current.get::<_, String>(2);
    if let Some(parent) = &parent_comment_id {
        if transaction
            .query_opt(
                "SELECT 1 FROM task_comments WHERE id = $1 AND task_id = $2",
                &[&parent, &task_id],
            )
            .await
            .map_err(ApiError::database)?
            .is_none()
        {
            return Err(ApiError::invalid(
                "parent_comment_id must belong to the same task",
            ));
        }
    }
    for mention in &mentions {
        validate_mention_target(&transaction, &project_id, &task_id, mention).await?;
    }

    // 沿用既有显式意图状态机，评论不能绕过当前看板阶段推进关键状态。
    let comment_id = Uuid::new_v4().to_string();
    let current_stage_value = xiexu_domain::BoardStage::parse(&current_stage)
        .ok_or_else(|| ApiError::invalid("stored task stage is invalid"))?;
    let mut next_stage = None;
    if intent == "approve_plan" && current_stage_value == xiexu_domain::BoardStage::PlanReview {
        next_stage = Some(xiexu_domain::BoardStage::InProgress);
    } else if intent == "accept" && current_stage_value == xiexu_domain::BoardStage::Acceptance {
        next_stage = Some(xiexu_domain::BoardStage::Done);
    } else if intent == "rework" && current_stage_value == xiexu_domain::BoardStage::Acceptance {
        next_stage = Some(xiexu_domain::BoardStage::InProgress);
    }
    let mut transition_applied = false;
    if let Some(target_stage) = next_stage {
        let next_revision = current_revision + 1;
        let next_plan_status = if target_stage == xiexu_domain::BoardStage::InProgress {
            "approved"
        } else {
            "not_required"
        };
        let next_execution_status = if target_stage == xiexu_domain::BoardStage::InProgress {
            "queued"
        } else {
            "idle"
        };
        let next_acceptance_status = if target_stage == xiexu_domain::BoardStage::Done {
            "passed"
        } else {
            "not_started"
        };
        transaction
            .execute(
                concat!(
                    "UPDATE tasks SET board_stage = $2, plan_status = $3, execution_status = $4, acceptance_status = $5, ",
                    "progress_percent = CASE WHEN $2 = 'done' THEN 100 ELSE progress_percent END, revision = revision + 1, updated_at = now() ",
                    "WHERE id = $1"
                ),
                &[&task_id, &target_stage.as_str(), &next_plan_status, &next_execution_status, &next_acceptance_status],
            )
            .await
            .map_err(ApiError::database)?;
        transaction
            .execute(
                "INSERT INTO task_transitions (id, task_id, from_stage, to_stage, reason) VALUES ($1, $2, $3, $4, $5)",
                &[&Uuid::new_v4().to_string(), &task_id, &current_stage, &target_stage.as_str(), &format!("comment:{intent}")],
            )
            .await
            .map_err(ApiError::database)?;
        transaction
            .execute(
                concat!(
                    "INSERT INTO task_events (task_id, event_type, actor_type, actor_id, before_data, after_data, event_data) ",
                    "VALUES ($1, 'task.stage_changed', 'human', 'human', $2, $3, $4)"
                ),
                &[
                    &task_id,
                    &json!({ "board_stage": current_stage.clone() }),
                    &json!({ "board_stage": target_stage.as_str() }),
                    &json!({ "reason": format!("comment:{intent}"), "comment_id": comment_id.clone() }),
                ],
            )
            .await
            .map_err(ApiError::database)?;

        // 进入执行阶段时创建去重作业；完成父任务同步验收子任务并异步刷新项目文档。
        if target_stage == xiexu_domain::BoardStage::Done {
            transaction
                .execute(
                    concat!(
                        "UPDATE tasks SET board_stage = 'done', acceptance_status = 'passed', progress_percent = 100, ",
                        "revision = revision + 1, updated_at = now() WHERE parent_task_id = $1 AND board_stage <> 'cancelled'"
                    ),
                    &[&task_id],
                )
                .await
                .map_err(ApiError::database)?;

            // 父任务完成刷新使用共享事务阶段，确保评论和通用转换入口遵循同一规则。
            enqueue_parent_document_refresh(&transaction, &task_id, &project_id, next_revision)
                .await?;
        } else {
            let dedupe_key = format!("task:{task_id}:execute:{next_revision}");
            transaction
                .execute(
                    concat!(
                        "INSERT INTO execution_jobs (id, kind, status, task_id, payload, dedupe_key) ",
                        "VALUES ($1, 'execute_task', 'queued', $2, $3, $4) ON CONFLICT (dedupe_key) DO NOTHING"
                    ),
                    &[
                        &Uuid::new_v4().to_string(), &task_id,
                        &json!({
                            "task_id": task_id.clone(), "reason": format!("comment:{intent}"),
                            "revision": next_revision, "comment_id": comment_id.clone()
                        }),
                        &dedupe_key,
                    ],
                )
                .await
                .map_err(ApiError::database)?;
            transaction
                .execute(
                    concat!(
                        "INSERT INTO task_events (task_id, event_type, actor_type, actor_id, event_data) ",
                        "VALUES ($1, 'execution.job_queued', 'system', 'control-plane', $2)"
                    ),
                    &[&task_id, &json!({ "kind": "execute_task", "dedupe_key": dedupe_key })],
                )
                .await
                .map_err(ApiError::database)?;
        }
        transition_applied = true;
    }

    // 评论先落库，后续 mention 和解除记录均通过外键关联这条可追溯回复。
    transaction
        .execute(
            concat!(
                "INSERT INTO task_comments ",
                "(id, task_id, parent_comment_id, author_type, author_name, content, intent, transition_applied) ",
                "VALUES ($1, $2, $3, 'human', 'Human', $4, $5, $6)"
            ),
            &[&comment_id, &task_id, &parent_comment_id, &content, &intent, &transition_applied],
        )
        .await
        .map_err(ApiError::database)?;

    // 新提及建立通知事实；任务目标额外建立 active 依赖，并让来源任务进入可解释等待态。
    let mut created_mentions = Vec::new();
    for mention in mentions {
        let mention_id = Uuid::new_v4().to_string();
        transaction
            .execute(
                "INSERT INTO task_mentions (id, comment_id, source_task_id, target_type, target_id) VALUES ($1, $2, $3, $4, $5)",
                &[&mention_id, &comment_id, &task_id, &mention.target_type, &mention.target_id],
            )
            .await
            .map_err(ApiError::database)?;
        if mention.target_type == "task" {
            transaction
                .execute(
                    concat!(
                        "INSERT INTO task_relations (id, project_id, from_task_id, to_task_id, relation_type, status, source_comment_id) ",
                        "VALUES ($1, $2, $3, $4, 'depends_on', 'active', $5) ",
                        "ON CONFLICT (from_task_id, to_task_id, relation_type) DO UPDATE SET status = 'active', ",
                        "source_comment_id = EXCLUDED.source_comment_id, resolved_comment_id = NULL, resolved_at = NULL"
                    ),
                    &[&Uuid::new_v4().to_string(), &project_id, &task_id, &mention.target_id, &comment_id],
                )
                .await
                .map_err(ApiError::database)?;
            transaction
                .execute(
                    concat!(
                        "INSERT INTO task_events (task_id, event_type, actor_type, actor_id, event_data) ",
                        "VALUES ($1, 'task.dependency_requested', 'human', 'human', $2)"
                    ),
                    &[&mention.target_id, &json!({
                        "source_task_id": task_id.clone(), "source_comment_id": comment_id.clone(), "mention_id": mention_id.clone()
                    })],
                )
                .await
                .map_err(ApiError::database)?;
        }
        created_mentions.push(json!({ "id": mention_id, "target_type": mention.target_type, "target_id": mention.target_id, "status": "pending" }));
    }
    if !created_mentions.is_empty() {
        transaction.execute("UPDATE tasks SET collaboration_status = 'waiting', updated_at = now() WHERE id = $1", &[&task_id]).await.map_err(ApiError::database)?;
    }

    // 显式 resolves_mention_id 只可解除当前任务发出的未完成提及，并同步关闭对应任务依赖。
    let mut resolved_mention = None;
    if let Some(mention_id) = resolves_mention_id {
        let mention = transaction
            .query_opt("SELECT comment_id, target_type, target_id, status FROM task_mentions WHERE id = $1 AND source_task_id = $2 FOR UPDATE", &[&mention_id, &task_id])
            .await
            .map_err(ApiError::database)?
            .ok_or_else(|| ApiError::invalid("resolves_mention_id must belong to the same task"))?;
        if mention.get::<_, String>(3) != "pending" {
            return Err(ApiError::conflict("mention is already resolved"));
        }
        let source_comment_id = mention.get::<_, String>(0);
        let target_type = mention.get::<_, String>(1);
        let target_id = mention.get::<_, String>(2);
        transaction
            .execute(
                "UPDATE task_mentions SET status = 'resolved', resolved_by_comment_id = $2, resolved_at = now() WHERE id = $1",
                &[&mention_id, &comment_id],
            )
            .await
            .map_err(ApiError::database)?;
        if target_type == "task" {
            transaction
                .execute(
                    concat!(
                        "UPDATE task_relations SET status = 'resolved', resolved_comment_id = $2, resolved_at = now() ",
                        "WHERE from_task_id = $1 AND source_comment_id = $3 AND relation_type = 'depends_on'"
                    ),
                    &[&task_id, &comment_id, &source_comment_id],
                )
                .await
                .map_err(ApiError::database)?;
        }
        resolved_mention = Some(
            json!({ "id": mention_id, "target_type": target_type, "target_id": target_id, "status": "resolved" }),
        );
    }

    // 仅当不存在其他 pending 提及时解除等待，多个并行依赖不会被单个回复过早放行。
    let pending_count = transaction
        .query_one(
            "SELECT count(*) FROM task_mentions WHERE source_task_id = $1 AND status = 'pending'",
            &[&task_id],
        )
        .await
        .map_err(ApiError::database)?
        .get::<_, i64>(0);
    let collaboration_status = if pending_count == 0 {
        "ready"
    } else {
        "waiting"
    };
    transaction
        .execute(
            "UPDATE tasks SET collaboration_status = $2, updated_at = now() WHERE id = $1",
            &[&task_id, &collaboration_status],
        )
        .await
        .map_err(ApiError::database)?;
    transaction
        .execute(
            concat!(
                "INSERT INTO task_events (task_id, event_type, actor_type, actor_id, event_data) ",
                "VALUES ($1, 'task.comment_added', 'human', 'human', $2)"
            ),
            &[&task_id, &json!({
                "comment_id": comment_id.clone(), "intent": intent, "parent_comment_id": parent_comment_id,
                "created_mentions": created_mentions, "resolved_mention": resolved_mention,
                "collaboration_status": collaboration_status
            })],
        )
        .await
        .map_err(ApiError::database)?;
    transaction.commit().await.map_err(ApiError::database)?;
    Ok((
        StatusCode::CREATED,
        Json(json!({
            "id": comment_id, "task_id": task_id, "parent_comment_id": parent_comment_id, "intent": intent, "transition_applied": transition_applied,
            "mentions": created_mentions, "resolved_mention": resolved_mention, "collaboration_status": collaboration_status
        })),
    ))
}

/// 查询任务发出或收到的提及及其依赖解除状态。
async fn list_task_mentions(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    // 查询任务归属用于约束 Agent 提及范围，并明确不存在错误。
    let client = connect(&state).await?;
    let project_id = client
        .query_opt("SELECT project_id FROM tasks WHERE id = $1", &[&task_id])
        .await
        .map_err(ApiError::database)?
        .ok_or_else(|| ApiError::not_found("task not found"))?
        .get::<_, String>(0);

    // outgoing 包含当前任务发出的全部提及，incoming 包含对当前任务及其参与 Agent 的请求。
    let rows = client
        .query(
            "SELECT m.id, m.comment_id, m.source_task_id, source.title, m.target_type, m.target_id,
               CASE WHEN m.target_type = 'task' THEN (SELECT title FROM tasks WHERE id = m.target_id)
                    WHEN m.target_type = 'agent' THEN (SELECT name FROM agents WHERE id = m.target_id) END,
               m.status, m.resolved_by_comment_id, to_char(m.created_at, 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"'), to_char(m.resolved_at, 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"')
             FROM task_mentions m JOIN tasks source ON source.id = m.source_task_id
             WHERE m.source_task_id = $1 OR (m.target_type = 'task' AND m.target_id = $1) OR
               (m.target_type = 'agent' AND m.target_id IN (SELECT agent_id FROM task_agents WHERE task_id = $1 AND status = 'active'))
             ORDER BY m.created_at, m.id",
            &[&task_id],
        )
        .await
        .map_err(ApiError::database)?;
    let items = rows
        .iter()
        .map(|row| {
            let source_task_id = row.get::<_, String>(2);
            let direction = if source_task_id == task_id { "outgoing" } else { "incoming" };
            json!({
                "id": row.get::<_, String>(0), "comment_id": row.get::<_, String>(1), "source_task_id": source_task_id,
                "source_task_title": row.get::<_, String>(3), "target_type": row.get::<_, String>(4), "target_id": row.get::<_, String>(5),
                "target_name": row.get::<_, Option<String>>(6), "status": row.get::<_, String>(7), "resolved_by_comment_id": row.get::<_, Option<String>>(8),
                "created_at": row.get::<_, String>(9), "resolved_at": row.get::<_, Option<String>>(10), "direction": direction
            })
        })
        .collect::<Vec<_>>();
    Ok(Json(json!({ "project_id": project_id, "items": items })))
}
