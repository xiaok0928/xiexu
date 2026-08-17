mod codex;
mod workflow;

use codex::{CodexConfig, TaskPromptContext};
use serde_json::json;
use std::{env, path::PathBuf, sync::Arc, time::Duration};
use tokio::task::JoinHandle;
use tokio_postgres::{Client, NoTls};
use uuid::Uuid;

/// 自动刷新项目概览时唯一允许改写的章节，其他章节仍由 Human 或后续专用流程维护。
const DOCUMENT_REFRESH_SECTION_KEY: &str = "progress";

/// 已领取的执行作业，携带本次尝试的租约身份。
struct ClaimedJob {
    /// 作业主键。
    job_id: String,
    /// 作业类型，用于选择任务执行、协作整理或项目文档刷新流程。
    kind: String,
    /// 关联任务主键。
    task_id: Option<String>,
    /// 作业所属项目；非项目级职责优化作业可以为空。
    project_id: Option<String>,
    /// 作业指定的 Agent；任务作业缺失时由任务主责或项目协调者补齐。
    agent_id: Option<String>,
    /// 对话总结作业关联的对话主键。
    conversation_id: Option<String>,
    /// 创建作业时保存的结构化输入。
    payload: serde_json::Value,
    /// 本次领取生成的尝试主键。
    attempt_id: String,
}

/// 当前 Runner 正在执行的唯一作业，用于并发限制和 attempt 续租。
struct ActiveExecution {
    /// 需要定时续租的尝试主键。
    attempt_id: String,
    /// 后台执行任务句柄。
    handle: JoinHandle<()>,
}

/// 从数据库读取的统一执行上下文，用于任务、职责草案和对话总结作业。
struct ExecutionContext {
    /// 项目主键。
    project_id: String,
    /// 项目名称。
    project_name: String,
    /// 任务标题。
    title: String,
    /// 任务说明。
    description: String,
    /// 当前执行 Agent 主键；系统职责草案可以为空。
    agent_id: Option<String>,
    /// 当前执行 Agent 名称。
    agent_name: String,
    /// 合并实例、项目补充后的职责约束。
    agent_instructions: String,
    /// 已按 Agent 和业务范围过滤的记忆文本。
    memories: String,
}

/// 项目文档刷新上下文，同时保存提示输入和候选落库所需的文档身份。
struct DocumentRefreshContext {
    /// 复用统一 Codex 提示协议的项目、Agent 与变更摘要。
    execution: ExecutionContext,
    /// 待刷新的项目文档主键。
    document_id: String,
    /// 本次候选固定面向的章节键。
    section_key: String,
    /// 生成候选时读取的章节版本，用于完成阶段检测并发修改。
    base_section_revision: i64,
    /// 受控模式根据事实源直接生成的章节候选内容。
    controlled_candidate: String,
}

/// 运行器入口：注册实例、续租、扫描兜底刷新并循环领取执行作业。
#[tokio::main]
async fn main() {
    // 读取运行时配置，保持 Runner 身份和数据库依赖可由 Compose 注入。
    let database_url = env::var("DATABASE_URL").expect("DATABASE_URL must be configured");
    let runner_id = env::var("RUNNER_ID").unwrap_or_else(|_| "runner-1".to_owned());
    let runner_name = env::var("RUNNER_NAME").unwrap_or_else(|_| runner_id.clone());
    let codex_config = Arc::new(CodexConfig::from_env().expect("load Codex runtime configuration"));
    let lease_seconds = codex_config.lease_seconds();
    println!(
        "xiexu runner {runner_id} starting with Codex mode {}",
        codex_config.mode_name()
    );

    // 同一 Runner ID 重启时，旧进程已不存在，主动回收其未完成尝试以避免长租约悬挂。
    if let Err(error) = recover_runner_jobs(&database_url, &runner_id).await {
        eprintln!("runner startup recovery failed: {error}");
    }

    // 心跳与领取使用独立节拍，后台执行期间仍持续续租 Runner 和 attempt。
    let mut heartbeat_tick = tokio::time::interval(Duration::from_secs(10));
    let mut claim_tick = tokio::time::interval(Duration::from_secs(2));
    let mut document_refresh_tick = tokio::time::interval(Duration::from_secs(30));
    let mut workflow_tick = tokio::time::interval(Duration::from_secs(5));
    let mut active_execution: Option<ActiveExecution> = None;
    loop {
        tokio::select! {
            _ = heartbeat_tick.tick() => {
                if let Err(error) = heartbeat(&database_url, &runner_id, &runner_name).await {
                    eprintln!("runner heartbeat failed: {error}");
                }
                if let Some(active) = active_execution.as_ref() {
                    if let Err(error) = heartbeat_attempt(&database_url, &active.attempt_id, lease_seconds).await {
                        eprintln!("runner attempt heartbeat failed: {error}");
                    }
                }
            }
            _ = claim_tick.tick() => {
                if active_execution.as_ref().is_some_and(|active| active.handle.is_finished()) {
                    let finished = active_execution.take().expect("finished execution exists");
                    if let Err(error) = finished.handle.await {
                        eprintln!("runner execution task ended unexpectedly: {error}");
                    }
                }
                if active_execution.is_none() {
                    if let Err(error) = scan_todo_tasks(&database_url).await {
                        eprintln!("runner todo scan failed: {error}");
                    }
                    match claim_job(&database_url, &runner_id, lease_seconds).await {
                        Ok(Some(job)) => {
                            let attempt_id = job.attempt_id.clone();
                            let execution_database_url = database_url.clone();
                            let execution_runner_id = runner_id.clone();
                            let execution_codex_config = codex_config.clone();
                            let handle = tokio::spawn(async move {
                                if let Err(error) = execute_job(&execution_database_url, &execution_runner_id, job, &execution_codex_config).await {
                                    eprintln!("runner execution failed: {error}");
                                }
                            });
                            active_execution = Some(ActiveExecution { attempt_id, handle });
                        }
                        Ok(None) => {}
                        Err(error) => eprintln!("runner claim failed: {error}"),
                    }
                }
            }
            _ = document_refresh_tick.tick() => {
                if let Err(error) = scan_project_document_refreshes(&database_url).await {
                    eprintln!("runner project document scan failed: {error}");
                }
            }
            _ = workflow_tick.tick() => {
                if let Err(error) = workflow::scan_workflow_schedules(&database_url).await {
                    eprintln!("runner workflow schedule scan failed: {error}");
                }
                if let Err(error) = workflow::scan_workflow_runs(&database_url).await {
                    eprintln!("runner workflow scan failed: {error}");
                }
            }
        }
    }
}

/// 回收同一 Runner 身份在进程重启前遗留的运行中作业。
async fn recover_runner_jobs(database_url: &str, runner_id: &str) -> Result<(), String> {
    // 尝试和作业状态在一个事务更新，后续领取会生成新的 attempt。
    let mut client = connect(database_url).await?;
    let transaction = client
        .transaction()
        .await
        .map_err(|error| error.to_string())?;
    transaction.execute("UPDATE execution_jobs SET status = 'queued', available_at = now(), updated_at = now() WHERE status = 'running' AND attempt_count < max_attempts AND EXISTS (SELECT 1 FROM execution_attempts a WHERE a.job_id = execution_jobs.id AND a.runner_instance_id = $1 AND a.status = 'running')", &[&runner_id]).await.map_err(|error| error.to_string())?;
    transaction.execute("UPDATE execution_attempts SET status = 'expired', finished_at = now(), failure_message = 'runner restarted' WHERE runner_instance_id = $1 AND status = 'running'", &[&runner_id]).await.map_err(|error| error.to_string())?;
    transaction
        .commit()
        .await
        .map_err(|error| error.to_string())
}

/// 周期扫描 Todo，按任务 revision 幂等创建方案生成作业。
async fn scan_todo_tasks(database_url: &str) -> Result<(), String> {
    let mut client = connect(database_url).await?;
    let transaction = client
        .transaction()
        .await
        .map_err(|error| error.to_string())?;

    // 没有主责的 Todo 任务由项目协调 Agent 保底认领，固定成员与动态参与关系保持分离。
    transaction.execute("INSERT INTO task_agents (task_id, agent_id, participation_type, status) SELECT t.id, pa.agent_id, 'owner', 'active' FROM tasks t JOIN project_agents pa ON pa.project_id = t.project_id AND pa.assignment_type = 'coordinator' AND pa.status = 'active' WHERE t.board_stage = 'todo' AND NOT EXISTS (SELECT 1 FROM task_agents ta WHERE ta.task_id = t.id AND ta.participation_type = 'owner' AND ta.status = 'active') ON CONFLICT (task_id, agent_id) DO UPDATE SET participation_type = 'owner', status = 'active', joined_at = now(), left_at = NULL", &[]).await.map_err(|error| error.to_string())?;

    // 扫描时同时取得项目和主责 Agent，使后续作业拥有稳定的协作归属。
    let rows = transaction.query("SELECT t.id, t.revision, t.project_id, ta.agent_id FROM tasks t JOIN task_agents ta ON ta.task_id = t.id AND ta.participation_type = 'owner' AND ta.status = 'active' WHERE t.board_stage = 'todo' AND t.requires_plan_confirmation = TRUE AND NOT EXISTS (SELECT 1 FROM execution_jobs j WHERE j.task_id = t.id AND j.kind = 'prepare_task_plan' AND j.status IN ('queued', 'running')) FOR UPDATE OF t SKIP LOCKED", &[]).await.map_err(|error| error.to_string())?;
    for row in rows {
        let task_id = row.get::<_, String>(0);
        let revision = row.get::<_, i64>(1);
        let project_id = row.get::<_, String>(2);
        let agent_id = row.get::<_, String>(3);
        let dedupe_key = format!("task:{task_id}:plan:{revision}");
        let inserted = transaction.execute("INSERT INTO execution_jobs (id, kind, status, task_id, project_id, agent_id, payload, dedupe_key) VALUES ($1, 'prepare_task_plan', 'queued', $2, $3, $4, $5, $6) ON CONFLICT (dedupe_key) DO NOTHING", &[&Uuid::new_v4().to_string(), &task_id, &project_id, &agent_id, &json!({ "task_id": task_id.clone(), "revision": revision }), &dedupe_key]).await.map_err(|error| error.to_string())?;
        if inserted == 1 {
            transaction.execute("UPDATE tasks SET execution_status = 'queued', updated_at = now() WHERE id = $1 AND board_stage = 'todo'", &[&task_id]).await.map_err(|error| error.to_string())?;
            transaction.execute("INSERT INTO task_events (task_id, event_type, actor_type, actor_id, event_data) VALUES ($1, 'execution.plan_queued', 'system', 'runner', $2)", &[&task_id, &json!({ "kind": "prepare_task_plan", "dedupe_key": dedupe_key })]).await.map_err(|error| error.to_string())?;
        }
    }
    transaction
        .commit()
        .await
        .map_err(|error| error.to_string())
}

/// 每约三十秒扫描落后于项目任务变更的文档，并按文档 revision 幂等补建刷新作业。
async fn scan_project_document_refreshes(database_url: &str) -> Result<(), String> {
    // 扫描使用短事务持有文档行锁，排队完成后立即释放，不阻塞实际刷新执行。
    let mut client = connect(database_url).await?;
    let transaction = client
        .transaction()
        .await
        .map_err(|error| error.to_string())?;

    // 一次批量锁定所有满足条件的文档；活跃作业和文档锁共同避免多 Runner 重复排队。
    let rows = transaction
        .query(
            concat!(
                "SELECT pd.id, pd.project_id, pd.revision, coordinator.agent_id, changes.latest_task_watermark FROM project_documents pd ",
                "JOIN project_agents coordinator ON coordinator.project_id = pd.project_id AND coordinator.assignment_type = 'coordinator' ",
                "AND coordinator.status = 'active' JOIN LATERAL (SELECT floor(extract(epoch FROM max(t.updated_at)) * 1000000)::bigint ",
                "AS latest_task_watermark FROM tasks t WHERE t.project_id = pd.project_id HAVING max(t.updated_at) > ",
                "COALESCE(pd.last_refreshed_at, '-infinity'::timestamptz)) changes ON TRUE WHERE pd.status = 'active' AND NOT EXISTS ",
                "(SELECT 1 FROM execution_jobs j WHERE j.kind = 'refresh_project_document' AND j.status IN ('queued', 'running') ",
                "AND j.payload ->> 'document_id' = pd.id) FOR UPDATE OF pd SKIP LOCKED"
            ),
            &[],
        )
        .await
        .map_err(|error| error.to_string())?;
    for row in rows {
        let document_id = row.get::<_, String>(0);
        let project_id = row.get::<_, String>(1);
        let document_revision = row.get::<_, i64>(2);
        let agent_id = row.get::<_, String>(3);
        let latest_task_watermark = row.get::<_, i64>(4);
        let dedupe_key = format!("document:{document_id}:refresh:scheduled:{latest_task_watermark}:revision:{document_revision}");

        // payload 保存触发事实和扫描水位，执行时仍从数据库重新加载最新章节与任务状态。
        transaction
            .execute(
                concat!(
                    "INSERT INTO execution_jobs (id, kind, status, project_id, agent_id, payload, dedupe_key) ",
                    "VALUES ($1, 'refresh_project_document', 'queued', $2, $3, $4, $5) ON CONFLICT (dedupe_key) DO NOTHING"
                ),
                &[
                    &Uuid::new_v4().to_string(),
                    &project_id,
                    &agent_id,
                    &json!({
                        "document_id": document_id,
                        "trigger_type": "scheduled",
                        "document_revision": document_revision,
                        "latest_task_watermark": latest_task_watermark
                    }),
                    &dedupe_key,
                ],
            )
            .await
            .map_err(|error| error.to_string())?;
    }

    // 扫描批次统一提交，保证判重结果和所有新作业同时可见。
    transaction
        .commit()
        .await
        .map_err(|error| error.to_string())
}

/// 写入可幂等的实例心跳，租约过期后由控制面将实例视为 stale。
async fn heartbeat(database_url: &str, runner_id: &str, runner_name: &str) -> Result<(), String> {
    let client = connect(database_url).await?;
    client
        .execute(
            "INSERT INTO runner_instances (id, name, status, last_heartbeat_at, lease_expires_at) \
             VALUES ($1, $2, 'ready', now(), now() + interval '30 seconds') \
             ON CONFLICT (id) DO UPDATE SET name = EXCLUDED.name, status = 'ready', \
             last_heartbeat_at = now(), lease_expires_at = now() + interval '30 seconds'",
            &[&runner_id, &runner_name],
        )
        .await
        .map(|_| ())
        .map_err(|error| error.to_string())
}

/// 为正在运行的 attempt 续租，长时间 Codex 运行不会被其他 Runner 重复领取。
async fn heartbeat_attempt(
    database_url: &str,
    attempt_id: &str,
    lease_seconds: i32,
) -> Result<(), String> {
    let client = connect(database_url).await?;
    // 租约秒数已由配置边界限制为正整数，嵌入 SQL 字面量可避免驱动缺少 interval 参数编码器的问题。
    let query = format!(
        "UPDATE execution_attempts SET heartbeat_at = now(), lease_expires_at = now() + interval '{lease_seconds} seconds' WHERE id = $1 AND status = 'running'",
    );
    client
        .execute(&query, &[&attempt_id])
        .await
        .map(|_| ())
        .map_err(|error| error.to_string())
}

/// 建立短生命周期数据库连接，并把连接任务交给 Tokio 维护。
async fn connect(database_url: &str) -> Result<Client, String> {
    let (client, connection) = tokio_postgres::connect(database_url, NoTls)
        .await
        .map_err(|error| error.to_string())?;
    tokio::spawn(async move {
        if let Err(error) = connection.await {
            eprintln!("runner database connection ended: {error}");
        }
    });
    Ok(client)
}

/// 原子领取一个到期可执行作业，并为本次领取建立租约尝试。
async fn claim_job(
    database_url: &str,
    runner_id: &str,
    lease_seconds: i32,
) -> Result<Option<ClaimedJob>, String> {
    let mut client = connect(database_url).await?;
    let transaction = client
        .transaction()
        .await
        .map_err(|error| error.to_string())?;

    // 先回收已经失去租约的作业，允许其在最大尝试次数内重新排队。
    transaction.execute("UPDATE execution_jobs SET status = 'queued', available_at = now(), updated_at = now() WHERE status = 'running' AND attempt_count < max_attempts AND EXISTS (SELECT 1 FROM execution_attempts a WHERE a.job_id = execution_jobs.id AND a.status = 'running' AND a.lease_expires_at < now())", &[]).await.map_err(|error| error.to_string())?;
    transaction.execute("UPDATE execution_attempts SET status = 'expired', finished_at = now(), failure_message = 'lease expired' WHERE status = 'running' AND lease_expires_at < now()", &[]).await.map_err(|error| error.to_string())?;
    let row = transaction.query_opt("SELECT id, kind, task_id, project_id, agent_id, conversation_id, payload FROM execution_jobs WHERE status = 'queued' AND available_at <= now() AND attempt_count < max_attempts ORDER BY created_at ASC FOR UPDATE SKIP LOCKED LIMIT 1", &[]).await.map_err(|error| error.to_string())?;
    let Some(row) = row else {
        transaction.commit().await.map_err(|error| error.to_string())?;
        return Ok(None);
    };
    let job_id = row.get::<_, String>(0);
    let kind = row.get::<_, String>(1);
    let task_id = row.get::<_, Option<String>>(2);
    let project_id = row.get::<_, Option<String>>(3);
    let agent_id = row.get::<_, Option<String>>(4);
    let conversation_id = row.get::<_, Option<String>>(5);
    let payload = row.get::<_, serde_json::Value>(6);
    let attempt_id = Uuid::new_v4().to_string();

    // 领取和任务执行态更新处于同一事务；文档刷新即使关联来源任务也不得改变任务主状态。
    transaction.execute("UPDATE execution_jobs SET status = 'running', attempt_count = attempt_count + 1, updated_at = now() WHERE id = $1", &[&job_id]).await.map_err(|error| error.to_string())?;
    // 租约秒数只来自内部受限配置，使用字面量避免 interval 参数的驱动编码限制。
    let attempt_insert = format!(
        "INSERT INTO execution_attempts (id, job_id, runner_instance_id, status, lease_expires_at) VALUES ($1, $2, $3, 'running', now() + interval '{lease_seconds} seconds')",
    );
    transaction
        .execute(&attempt_insert, &[&attempt_id, &job_id, &runner_id])
        .await
        .map_err(|error| error.to_string())?;
    if kind != "refresh_project_document" {
        if let Some(task_id) = task_id.as_ref() {
            transaction
                .execute("UPDATE tasks SET execution_status = 'running', updated_at = now() WHERE id = $1 AND board_stage = 'in_progress'", &[task_id])
                .await
                .map_err(|error| error.to_string())?;
        }
    }
    transaction.execute("INSERT INTO execution_events (job_id, attempt_id, task_id, event_type, payload) VALUES ($1, $2, $3, 'job.claimed', $4)", &[&job_id, &attempt_id, &task_id, &json!({ "runner_id": runner_id })]).await.map_err(|error| error.to_string())?;
    transaction
        .commit()
        .await
        .map_err(|error| error.to_string())?;
    Ok(Some(ClaimedJob {
        job_id,
        kind,
        task_id,
        project_id,
        agent_id,
        conversation_id,
        payload,
        attempt_id,
    }))
}

/// 执行白名单作业，真实模式调用 Codex，受控模式保留可验证的本地输出。
async fn execute_job(
    database_url: &str,
    runner_id: &str,
    mut job: ClaimedJob,
    codex_config: &CodexConfig,
) -> Result<(), String> {
    // 先验证作业类型，避免无效请求启动外部进程。
    let supported = [
        "execute_task",
        "prepare_task_plan",
        "optimize_agent_profile",
        "summarize_conversation",
        "refresh_project_document",
        "run_workflow",
    ];
    if !supported.contains(&job.kind.as_str()) {
        return mark_failed(database_url, &job, "unsupported execution kind").await;
    }

    // 工作流由专用状态机分段推进，不把整个画布误当成一次 Codex 调用。
    if job.kind == "run_workflow" {
        return match workflow::execute_workflow_job(database_url, runner_id, &mut job, codex_config)
            .await
        {
            Ok(()) => Ok(()),
            Err(error) => {
                mark_failed(database_url, &job, &error).await?;
                Err(error)
            }
        };
    }

    // 按作业范围加载最小业务上下文，职责和记忆始终由服务端事实源生成。
    let mut document_refresh_context = None;
    let context = match job.kind.as_str() {
        "execute_task" | "prepare_task_plan" => {
            let Some(task_id) = job.task_id.as_ref() else { return mark_failed(database_url, &job, "task execution requires task_id").await; };
            load_task_context(database_url, task_id).await?
        }
        "optimize_agent_profile" => load_agent_profile_context(database_url, &job).await?,
        "summarize_conversation" => {
            let Some(conversation_id) = job.conversation_id.as_ref() else { return mark_failed(database_url, &job, "conversation summary requires conversation_id").await; };
            load_conversation_context(database_url, conversation_id).await?
        }
        "refresh_project_document" => {
            let refresh_context = match load_project_document_context(database_url, &job).await {
                Ok(context) => context,
                Err(error) => {
                    mark_failed(database_url, &job, &error).await?;
                    return Err(error);
                }
            };
            let execution = ExecutionContext {
                project_id: refresh_context.execution.project_id.clone(),
                project_name: refresh_context.execution.project_name.clone(),
                title: refresh_context.execution.title.clone(),
                description: refresh_context.execution.description.clone(),
                agent_id: refresh_context.execution.agent_id.clone(),
                agent_name: refresh_context.execution.agent_name.clone(),
                agent_instructions: refresh_context.execution.agent_instructions.clone(),
                memories: refresh_context.execution.memories.clone(),
            };
            document_refresh_context = Some(refresh_context);
            execution
        }
        _ => unreachable!("job kind was validated"),
    };

    // 兼容 M2 已入队或旧接口创建的任务作业，在执行前补齐项目和 Agent 作用域。
    if job.project_id.is_none() && job.kind != "optimize_agent_profile" {
        job.project_id = Some(context.project_id.clone());
    }
    if job.agent_id.is_none() {
        job.agent_id = context.agent_id.clone();
    }
    let scope_client = connect(database_url).await?;
    scope_client.execute("UPDATE execution_jobs SET project_id = COALESCE(project_id, $2), agent_id = COALESCE(agent_id, $3), updated_at = now() WHERE id = $1", &[&job.job_id, &job.project_id, &job.agent_id]).await.map_err(|error| error.to_string())?;
    mark_attempt_started(database_url, runner_id, &job, codex_config.mode_name()).await?;

    // 仅研发执行任务允许使用会话 worktree；其他作业始终保持默认项目目录或只读受控执行。
    let workspace_override = if job.kind == "execute_task" {
        resolve_task_worktree_workspace(
            database_url,
            job.task_id.as_deref().expect("validated task id"),
            &context.project_id,
            codex_config,
        )
        .await?
    } else {
        None
    };

    // 真实模式只把必要业务字段传给 Codex，数据库配置和其他环境不会进入提示。
    let execution_result = if codex_config.is_real() {
        codex_config
            .run_in_workspace(
                &job.kind,
                TaskPromptContext {
                    project_id: &context.project_id,
                    project_name: &context.project_name,
                    title: &context.title,
                    description: &context.description,
                    agent_name: &context.agent_name,
                    agent_instructions: &context.agent_instructions,
                    memories: &context.memories,
                },
                workspace_override.as_deref(),
            )
            .await
            .map(|output| (output.content, output.thread_id))
    } else {
        let content = match job.kind.as_str() {
            "prepare_task_plan" => "方案草案已生成，等待 Human 确认。",
            "execute_task" => "受控执行完成，等待 Human 验收。",
            "optimize_agent_profile" => "职责草案：明确目标、核心职责、工作边界、协作对象与结果标准，并以事实和验证结果完成交付。",
            "summarize_conversation" => "对话总结已生成：保留目标、关键决定、任务关联、未完成事项与后续动作。",
            "refresh_project_document" => document_refresh_context.as_ref().expect("document refresh context exists").controlled_candidate.as_str(),
            _ => unreachable!("job kind was validated"),
        };
        Ok((content.to_owned(), None))
    };
    let (output_content, thread_id) = match execution_result {
        Ok(output) => output,
        Err(error) => {
            mark_failed(database_url, &job, &error).await?;
            return Err(error);
        }
    };
    // 不同作业类型只在完成后的业务状态变化上分支，执行追踪语义保持一致。
    match job.kind.as_str() {
        "prepare_task_plan" => {
            finish_plan_job(
                database_url,
                runner_id,
                &job,
                job.task_id.as_deref().expect("validated task id"),
                &output_content,
                thread_id.as_deref(),
            )
            .await
        }
        "execute_task" => {
            finish_execution_job(
                database_url,
                runner_id,
                &job,
                job.task_id.as_deref().expect("validated task id"),
                &output_content,
                thread_id.as_deref(),
            )
            .await
        }
        "optimize_agent_profile" => {
            finish_general_job(
                database_url,
                runner_id,
                &job,
                "responsibility_draft",
                &output_content,
                thread_id.as_deref(),
            )
            .await
        }
        "summarize_conversation" => {
            finish_conversation_summary(
                database_url,
                runner_id,
                &job,
                &output_content,
                thread_id.as_deref(),
            )
            .await
        }
        "refresh_project_document" => {
            let refresh_context = document_refresh_context
                .as_ref()
                .expect("document refresh context exists");
            finish_document_refresh_job(
                database_url,
                runner_id,
                &job,
                refresh_context,
                &output_content,
                thread_id.as_deref(),
            )
            .await
        }
        _ => unreachable!("job kind was validated"),
    }
}

/// 读取任务绑定的 worktree 会话，验证数据库归属和状态后交由 Codex 层解析真实目录。
async fn resolve_task_worktree_workspace(
    database_url: &str,
    task_id: &str,
    expected_project_id: &str,
    codex_config: &CodexConfig,
) -> Result<Option<PathBuf>, String> {
    // 任务和会话必须在同一查询中读取，缺失会话时不能静默回退到默认项目目录执行。
    let client = connect(database_url).await?;
    let row = client
        .query_one(
            concat!(
                "SELECT t.project_id, t.workspace_session_id, session.project_id, session.worktree_path, session.status FROM tasks t ",
                "LEFT JOIN git_worktree_sessions session ON session.id = t.workspace_session_id WHERE t.id = $1"
            ),
            &[&task_id],
        )
        .await
        .map_err(|error| error.to_string())?;
    let task_project_id = row.get::<_, String>(0);
    let session_id = row.get::<_, Option<String>>(1);
    if task_project_id != expected_project_id {
        return Err("task project does not match execution context".to_owned());
    }
    let Some(session_id) = session_id else {
        return Ok(None);
    };
    let session_project_id = row
        .get::<_, Option<String>>(2)
        .ok_or_else(|| format!("worktree session {session_id} not found"))?;
    let session_path = row
        .get::<_, Option<String>>(3)
        .ok_or_else(|| format!("worktree session {session_id} has no path"))?;
    let session_status = row
        .get::<_, Option<String>>(4)
        .ok_or_else(|| format!("worktree session {session_id} has no status"))?;
    if session_project_id != expected_project_id {
        return Err("worktree session project does not match task project".to_owned());
    }
    if session_status != "active" {
        return Err(format!("worktree session {session_id} is not active"));
    }

    // 路径校验会解析符号链接且强制要求位于项目 .xiexu-worktrees 下，不自动创建 Git 工作区。
    codex_config
        .prepare_worktree_workspace(expected_project_id, std::path::Path::new(&session_path))
        .await
        .map(Some)
}

/// 查询任务、项目、主责 Agent 和范围内记忆，不把执行控制字段交给 Codex。
async fn load_task_context(database_url: &str, task_id: &str) -> Result<ExecutionContext, String> {
    // 主责 Agent 优先，缺失时使用项目协调 Agent 作为执行上下文兜底。
    let client = connect(database_url).await?;
    let row = client.query_opt("SELECT p.id, p.name, t.title, t.description, a.id, a.name, concat_ws(E'\n', a.instructions, NULLIF(a.responsibility_supplement, ''), NULLIF(pa.responsibility_override, '')) FROM tasks t JOIN projects p ON p.id = t.project_id JOIN project_agents coordinator ON coordinator.project_id = p.id AND coordinator.assignment_type = 'coordinator' AND coordinator.status = 'active' LEFT JOIN task_agents owner ON owner.task_id = t.id AND owner.participation_type = 'owner' AND owner.status = 'active' JOIN agents a ON a.id = COALESCE(owner.agent_id, coordinator.agent_id) LEFT JOIN project_agents pa ON pa.project_id = p.id AND pa.agent_id = a.id AND pa.status = 'active' WHERE t.id = $1", &[&task_id]).await.map_err(|error| error.to_string())?.ok_or_else(|| "task or active coordinator not found".to_owned())?;
    let project_id = row.get::<_, String>(0);
    let agent_id = row.get::<_, String>(4);

    // 仅加载当前 Agent 的全局记忆和与当前项目、任务相符的记忆，限制数量防止提示无限膨胀。
    let memories = load_memories(&client, &agent_id, Some(&project_id), Some(task_id)).await?;
    Ok(ExecutionContext {
        project_id,
        project_name: row.get(1),
        title: row.get(2),
        description: row.get(3),
        agent_id: Some(agent_id),
        agent_name: row.get(5),
        agent_instructions: row.get(6),
        memories,
    })
}

/// 从作业载荷构造职责优化上下文，已有 Agent 的职责与记忆会作为草案参考。
async fn load_agent_profile_context(
    database_url: &str,
    job: &ClaimedJob,
) -> Result<ExecutionContext, String> {
    // 用户输入必须在作业创建时完成校验，Runner 仍以空值保护处理异常历史数据。
    let client = connect(database_url).await?;
    let name = job
        .payload
        .get("name")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("未命名 Agent")
        .to_owned();
    let description = job
        .payload
        .get("description")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .to_owned();
    let supplement = job
        .payload
        .get("responsibility_supplement")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let mut instructions = supplement.to_owned();
    let mut memories = "无相关记忆".to_owned();
    if let Some(agent_id) = job.agent_id.as_ref() {
        if let Some(row) = client.query_opt("SELECT concat_ws(E'\n', instructions, NULLIF(responsibility_supplement, '')) FROM agents WHERE id = $1", &[&agent_id]).await.map_err(|error| error.to_string())? {
            instructions = row.get(0);
            memories = load_memories(&client, agent_id, None, None).await?;
        }
    }
    Ok(ExecutionContext {
        project_id: "00000000-0000-0000-0000-000000000000".to_owned(),
        project_name: "协序 Agent 身份中心".to_owned(),
        title: name.clone(),
        description,
        agent_id: job.agent_id.clone(),
        agent_name: name,
        agent_instructions: instructions,
        memories,
    })
}

/// 将项目临时群聊整理为受控对话总结上下文。
async fn load_conversation_context(
    database_url: &str,
    conversation_id: &str,
) -> Result<ExecutionContext, String> {
    // 只允许加载属于项目且处于归档中的临时群聊。
    let client = connect(database_url).await?;
    let row = client.query_opt("SELECT p.id, p.name, c.title, a.id, a.name, concat_ws(E'\n', a.instructions, NULLIF(a.responsibility_supplement, ''), NULLIF(pa.responsibility_override, '')) FROM conversations c JOIN projects p ON p.id = c.project_id JOIN project_agents pa ON pa.project_id = p.id AND pa.assignment_type = 'coordinator' AND pa.status = 'active' JOIN agents a ON a.id = pa.agent_id WHERE c.id = $1 AND c.conversation_type = 'project_temporary' AND c.status = 'archiving'", &[&conversation_id]).await.map_err(|error| error.to_string())?.ok_or_else(|| "archiving project conversation not found".to_owned())?;
    let project_id = row.get::<_, String>(0);
    let agent_id = row.get::<_, String>(3);

    // 消息按时间聚合并限制总长度，原始消息仍完整保存在数据库中。
    let messages = client.query("SELECT author_type, author_id, content FROM conversation_messages WHERE conversation_id = $1 ORDER BY created_at, id", &[&conversation_id]).await.map_err(|error| error.to_string())?;
    let transcript = messages
        .iter()
        .map(|message| {
            format!(
                "[{}:{}] {}",
                message.get::<_, String>(0),
                message.get::<_, String>(1),
                message.get::<_, String>(2)
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let description = transcript.chars().take(60_000).collect::<String>();
    let memories = load_memories(&client, &agent_id, Some(&project_id), None).await?;
    Ok(ExecutionContext {
        project_id,
        project_name: row.get(1),
        title: row.get(2),
        description,
        agent_id: Some(agent_id),
        agent_name: row.get(4),
        agent_instructions: row.get(5),
        memories,
    })
}

/// 加载项目、目标章节、任务快照与协调 Agent，生成文档刷新所需的完整事实上下文。
async fn load_project_document_context(
    database_url: &str,
    job: &ClaimedJob,
) -> Result<DocumentRefreshContext, String> {
    // 文档身份必须来自受控 payload，并且在作业声明项目时必须属于同一项目。
    let document_id = job
        .payload
        .get("document_id")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| "document refresh requires document_id".to_owned())?;
    let client = connect(database_url).await?;
    let row = client
        .query_opt(
            concat!(
                "SELECT p.id, p.name, pd.title, section.title, section.content, a.id, a.name, concat_ws(E'\n', a.instructions, ",
                "NULLIF(a.responsibility_supplement, ''), NULLIF(coordinator.responsibility_override, '')), ",
                "pd.last_refreshed_at, section.revision FROM project_documents pd ",
                "JOIN projects p ON p.id = pd.project_id JOIN project_document_sections section ON section.document_id = pd.id ",
                "AND section.section_key = $2 JOIN project_agents coordinator ON coordinator.project_id = p.id ",
                "AND coordinator.assignment_type = 'coordinator' AND coordinator.status = 'active' JOIN agents a ON a.id = coordinator.agent_id ",
                "WHERE pd.id = $1 AND pd.status = 'active' AND ($3::text IS NULL OR p.id = $3)"
            ),
            &[&document_id, &DOCUMENT_REFRESH_SECTION_KEY, &job.project_id],
        )
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "active project document, progress section, or coordinator not found".to_owned())?;
    let project_id = row.get::<_, String>(0);
    let agent_id = row.get::<_, String>(5);
    let last_refreshed_at = row.get::<_, Option<std::time::SystemTime>>(8);

    // 当前所有章节作为只读背景提供给协调 Agent，避免进度候选与 Human 已维护内容相互矛盾。
    let section_rows = client
        .query(
            "SELECT section_key, title, content, locked_by_human, revision FROM project_document_sections WHERE document_id = $1 ORDER BY sort_order, section_key",
            &[&document_id],
        )
        .await
        .map_err(|error| error.to_string())?;
    let sections = section_rows
        .iter()
        .map(|section| {
            format!(
                "- [{}] {}（revision {}{}）：{}",
                section.get::<_, String>(0),
                section.get::<_, String>(1),
                section.get::<_, i64>(4),
                if section.get::<_, bool>(3) {
                    "，Human 锁定"
                } else {
                    ""
                },
                section.get::<_, String>(2)
            )
        })
        .collect::<Vec<_>>()
        .join("\n");

    // 一次查询取得完整任务快照并标记自上次刷新后的变更，生成候选时不做逐任务访问。
    let source_task_id = job
        .payload
        .get("source_task_id")
        .and_then(serde_json::Value::as_str);
    let task_rows = client
        .query(
            concat!(
                "SELECT id, title, board_stage, plan_status, execution_status, acceptance_status, progress_percent, revision, ",
                "COALESCE(updated_at > COALESCE($2::timestamptz, '-infinity'::timestamptz) OR id = $3, FALSE), ",
                "to_char(updated_at, 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"') FROM tasks ",
                "WHERE project_id = $1 ORDER BY created_at, id LIMIT 200"
            ),
            &[&project_id, &last_refreshed_at, &source_task_id],
        )
        .await
        .map_err(|error| error.to_string())?;
    let task_snapshot = task_rows
        .iter()
        .map(|task| {
            format!(
                "- {} [{}] {}：进度 {}%，plan={}，execution={}，acceptance={}，revision={}，updated_at={}",
                if task.get::<_, bool>(8) { "[本次变更]" } else { "[未变更]" },
                task.get::<_, String>(2),
                task.get::<_, String>(1),
                task.get::<_, i16>(6),
                task.get::<_, String>(3),
                task.get::<_, String>(4),
                task.get::<_, String>(5),
                task.get::<_, i64>(7),
                task.get::<_, String>(9)
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let controlled_candidate = if task_rows.is_empty() {
        "项目当前尚无任务。".to_owned()
    } else {
        format!("项目任务进展：\n{task_snapshot}")
    };

    // 刷新提示显式区分当前章节、其他章节和任务事实，真实模式只能返回目标章节的完整替换候选。
    let description = format!(
        concat!(
            "目标章节当前内容：\n{}\n\n项目全部章节：\n{}\n\n",
            "项目任务快照（[本次变更] 表示晚于上次成功刷新或为显式来源任务）：\n{}"
        ),
        row.get::<_, String>(4),
        sections,
        if task_snapshot.is_empty() {
            "无任务"
        } else {
            &task_snapshot
        }
    );
    let memories = load_memories(&client, &agent_id, Some(&project_id), source_task_id).await?;
    Ok(DocumentRefreshContext {
        execution: ExecutionContext {
            project_id,
            project_name: row.get(1),
            title: format!(
                "刷新《{}》的{}章节",
                row.get::<_, String>(2),
                row.get::<_, String>(3)
            ),
            description,
            agent_id: Some(agent_id),
            agent_name: row.get(6),
            agent_instructions: row.get(7),
            memories,
        },
        document_id: document_id.to_owned(),
        section_key: DOCUMENT_REFRESH_SECTION_KEY.to_owned(),
        base_section_revision: row.get(9),
        controlled_candidate,
    })
}

/// 批量加载一个 Agent 在给定项目、任务范围内可用的最近记忆。
async fn load_memories(
    client: &Client,
    agent_id: &str,
    project_id: Option<&String>,
    task_id: Option<&str>,
) -> Result<String, String> {
    // 项目或任务记忆只能命中相同范围；无范围的长期经验可跨项目复用。
    let rows = client.query("SELECT tier, content FROM agent_memories WHERE agent_id = $1 AND status = 'active' AND (project_id IS NULL OR project_id = $2) AND (task_id IS NULL OR task_id = $3) ORDER BY updated_at DESC LIMIT 20", &[&agent_id, &project_id, &task_id]).await.map_err(|error| error.to_string())?;
    if rows.is_empty() {
        return Ok("无相关记忆".to_owned());
    }
    Ok(rows
        .iter()
        .map(|row| {
            format!(
                "- [{}] {}",
                row.get::<_, String>(0),
                row.get::<_, String>(1)
            )
        })
        .collect::<Vec<_>>()
        .join("\n"))
}

/// 在外部执行前提交 started 事件，长任务运行期间前端即可看到真实状态。
async fn mark_attempt_started(
    database_url: &str,
    runner_id: &str,
    job: &ClaimedJob,
    mode: &str,
) -> Result<(), String> {
    let client = connect(database_url).await?;
    client.execute("INSERT INTO execution_events (job_id, attempt_id, task_id, event_type, payload) VALUES ($1, $2, $3, 'attempt.started', $4)", &[&job.job_id, &job.attempt_id, &job.task_id, &json!({ "runner_id": runner_id, "kind": job.kind.clone(), "mode": mode })]).await.map(|_| ()).map_err(|error| error.to_string())
}

/// 完成方案作业，把 Todo 推进到等待 Human 确认并保存 Codex 输出。
async fn finish_plan_job(
    database_url: &str,
    runner_id: &str,
    job: &ClaimedJob,
    task_id: &str,
    output_content: &str,
    thread_id: Option<&str>,
) -> Result<(), String> {
    let mut client = connect(database_url).await?;
    let transaction = client
        .transaction()
        .await
        .map_err(|error| error.to_string())?;
    let updated = transaction.execute("UPDATE tasks SET board_stage = 'plan_review', plan_status = 'reviewing', execution_status = 'succeeded', revision = revision + 1, updated_at = now() WHERE id = $1 AND board_stage = 'todo'", &[&task_id]).await.map_err(|error| error.to_string())?;
    if updated != 1 {
        drop(transaction);
        return mark_failed(database_url, job, "task is no longer in todo").await;
    }
    transaction.execute("INSERT INTO task_events (task_id, event_type, actor_type, actor_id, after_data, event_data) VALUES ($1, 'plan.generated', 'runner', $2, $3, $4)", &[&task_id, &runner_id, &json!({ "board_stage": "plan_review", "plan_status": "reviewing" }), &json!({ "job_id": job.job_id.clone(), "attempt_id": job.attempt_id.clone(), "codex_thread_id": thread_id })]).await.map_err(|error| error.to_string())?;
    finish_job_records(
        &transaction,
        job,
        task_id,
        "plan",
        output_content,
        runner_id,
        thread_id,
    )
    .await?;
    transaction
        .commit()
        .await
        .map_err(|error| error.to_string())
}

/// 完成执行作业，把任务推进到等待验收并保存 Codex 输出。
async fn finish_execution_job(
    database_url: &str,
    runner_id: &str,
    job: &ClaimedJob,
    task_id: &str,
    output_content: &str,
    thread_id: Option<&str>,
) -> Result<(), String> {
    let mut client = connect(database_url).await?;
    let transaction = client
        .transaction()
        .await
        .map_err(|error| error.to_string())?;
    // 工作流执行节点已经由已发布流程定义约束，不额外进入 Human 验收；普通研发任务仍保持既有验收阶段。
    let workflow_run_id = job
        .payload
        .get("workflow_run_id")
        .and_then(serde_json::Value::as_str);

    // 工作流实例控制与子任务完成竞争时锁定运行记录；暂停或终止先提交后，晚到结果不得覆盖控制终态。
    if let Some(workflow_run_id) = workflow_run_id {
        let run = transaction
            .query_opt(
                "SELECT status FROM workflow_runs WHERE id = $1 FOR UPDATE",
                &[&workflow_run_id],
            )
            .await
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "workflow run not found while completing child task".to_owned())?;
        let run_status = run.get::<_, String>(0);
        if run_status == "paused" || run_status == "terminated" {
            let job_status = if run_status == "paused" { "paused" } else { "cancelled" };
            transaction
                .execute(
                    "UPDATE execution_attempts SET status = 'cancelled', finished_at = now(), failure_message = $2 WHERE id = $1",
                    &[&job.attempt_id, &format!("workflow run is {run_status}")],
                )
                .await
                .map_err(|error| error.to_string())?;
            transaction
                .execute(
                    "UPDATE execution_jobs SET status = $2, updated_at = now() WHERE id = $1",
                    &[&job.job_id, &job_status],
                )
                .await
                .map_err(|error| error.to_string())?;
            transaction
                .execute(
                    concat!(
                        "INSERT INTO execution_events (job_id, attempt_id, task_id, event_type, payload) ",
                        "VALUES ($1, $2, $3, 'job.result_discarded', $4)"
                    ),
                    &[
                        &job.job_id,
                        &job.attempt_id,
                        &job.task_id,
                        &json!({ "workflow_run_id": workflow_run_id, "run_status": run_status }),
                    ],
                )
                .await
                .map_err(|error| error.to_string())?;
            transaction
                .commit()
                .await
                .map_err(|error| error.to_string())?;
            return Ok(());
        }
    }

    // 普通任务进入验收阶段，工作流子任务则由流程定义直接判定完成。
    let (target_stage, acceptance_status) = if workflow_run_id.is_some() {
        ("done", "passed")
    } else {
        ("acceptance", "not_started")
    };
    let updated = transaction
        .execute(
            concat!(
                "UPDATE tasks SET board_stage = $2, execution_status = 'succeeded', acceptance_status = $3, progress_percent = 100, ",
                "revision = revision + 1, updated_at = now() WHERE id = $1 AND board_stage = 'in_progress'"
            ),
            &[&task_id, &target_stage, &acceptance_status],
        )
        .await
        .map_err(|error| error.to_string())?;
    if updated != 1 {
        drop(transaction);
        return mark_failed(database_url, job, "task is no longer in progress").await;
    }
    transaction
        .execute(
            "INSERT INTO task_events (task_id, event_type, actor_type, actor_id, after_data, event_data) VALUES ($1, 'execution.completed', 'runner', $2, $3, $4)",
            &[
                &task_id,
                &runner_id,
                &json!({ "board_stage": target_stage, "execution_status": "succeeded", "acceptance_status": acceptance_status }),
                &json!({ "job_id": job.job_id.clone(), "attempt_id": job.attempt_id.clone(), "codex_thread_id": thread_id, "workflow_run_id": workflow_run_id }),
            ],
        )
        .await
        .map_err(|error| error.to_string())?;
    finish_job_records(
        &transaction,
        job,
        task_id,
        "summary",
        output_content,
        runner_id,
        thread_id,
    )
    .await?;
    transaction
        .commit()
        .await
        .map_err(|error| error.to_string())
}

/// 完成不改变任务状态的通用 Agent 作业，例如职责草案。
async fn finish_general_job(
    database_url: &str,
    runner_id: &str,
    job: &ClaimedJob,
    output_type: &str,
    output_content: &str,
    thread_id: Option<&str>,
) -> Result<(), String> {
    // 通用作业只写执行事实和输出，不自动覆盖 Agent 配置。
    let mut client = connect(database_url).await?;
    let transaction = client
        .transaction()
        .await
        .map_err(|error| error.to_string())?;
    finish_general_records(
        &transaction,
        runner_id,
        job,
        output_type,
        output_content,
        thread_id,
    )
    .await?;
    transaction
        .commit()
        .await
        .map_err(|error| error.to_string())
}

/// 完成对话总结作业，将总结追加为系统消息并正式归档临时群聊。
async fn finish_conversation_summary(
    database_url: &str,
    runner_id: &str,
    job: &ClaimedJob,
    output_content: &str,
    thread_id: Option<&str>,
) -> Result<(), String> {
    // 对话必须仍处于 archiving，避免用户恢复或其他作业改变状态后写入过期总结。
    let conversation_id = job
        .conversation_id
        .as_ref()
        .ok_or_else(|| "conversation summary requires conversation_id".to_owned())?;
    let mut client = connect(database_url).await?;
    let transaction = client
        .transaction()
        .await
        .map_err(|error| error.to_string())?;
    let updated = transaction.execute("UPDATE conversations SET status = 'archived', archived_at = now(), updated_at = now() WHERE id = $1 AND status = 'archiving'", &[&conversation_id]).await.map_err(|error| error.to_string())?;
    if updated != 1 {
        drop(transaction);
        return mark_failed(database_url, job, "conversation is no longer archiving").await;
    }

    // 总结以追加消息保存，原始对话顺序和内容不做修改。
    transaction.execute("INSERT INTO conversation_messages (id, conversation_id, author_type, author_id, content, message_type) VALUES ($1, $2, 'system', 'runner', $3, 'summary')", &[&Uuid::new_v4().to_string(), &conversation_id, &output_content]).await.map_err(|error| error.to_string())?;
    finish_general_records(
        &transaction,
        runner_id,
        job,
        "conversation_summary",
        output_content,
        thread_id,
    )
    .await?;
    transaction
        .commit()
        .await
        .map_err(|error| error.to_string())
}

/// 完成文档刷新：锁定章节只留待审候选，存在待审候选时记冲突，否则应用候选并创建完整版本。
async fn finish_document_refresh_job(
    database_url: &str,
    runner_id: &str,
    job: &ClaimedJob,
    context: &DocumentRefreshContext,
    output_content: &str,
    thread_id: Option<&str>,
) -> Result<(), String> {
    // 空候选不能覆盖现有章节，按普通执行失败进入既有重试机制且不触碰任务状态。
    let proposed_content = output_content.trim();
    if proposed_content.is_empty() {
        return mark_failed(
            database_url,
            job,
            "document refresh produced empty candidate",
        )
        .await;
    }

    // 同时锁定文档和目标章节，候选判重、章节更新及版本号递增在一个事务内串行化。
    let mut client = connect(database_url).await?;
    let transaction = client
        .transaction()
        .await
        .map_err(|error| error.to_string())?;
    let row = transaction
        .query_opt(
            concat!(
                "SELECT pd.current_version_no, section.locked_by_human, section.revision FROM project_documents pd ",
                "JOIN project_document_sections section ON section.document_id = pd.id AND section.section_key = $2 ",
                "WHERE pd.id = $1 AND pd.status = 'active' AND pd.project_id = $3 FOR UPDATE OF pd, section"
            ),
            &[&context.document_id, &context.section_key, &context.execution.project_id],
        )
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "active project document section not found".to_owned())?;
    let current_version_no = row.get::<_, i32>(0);
    let locked_by_human = row.get::<_, bool>(1);
    let current_section_revision = row.get::<_, i64>(2);
    let pending_exists = transaction
        .query_opt(
            "SELECT 1 FROM project_document_update_candidates WHERE document_id = $1 AND section_key = $2 AND status = 'pending' LIMIT 1",
            &[&context.document_id, &context.section_key],
        )
        .await
        .map_err(|error| error.to_string())?
        .is_some();
    let candidate_id = Uuid::new_v4().to_string();
    let trigger_type = job
        .payload
        .get("trigger_type")
        .or_else(|| job.payload.get("trigger"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("unspecified");
    let source_task_id = job
        .payload
        .get("source_task_id")
        .and_then(serde_json::Value::as_str);
    let source_conversation_id = job
        .payload
        .get("source_conversation_id")
        .and_then(serde_json::Value::as_str);
    let source_id = source_task_id.or(source_conversation_id);

    // 已有待审候选或生成期间发生章节修改时保留冲突候选，避免多个来源静默覆盖。
    let candidate_status = if pending_exists {
        transaction
            .execute(
                concat!(
                    "INSERT INTO project_document_update_candidates ",
                    "(id, document_id, section_key, proposed_content, source_type, source_id, base_section_revision, status, conflict_reason) ",
                    "VALUES ($1, $2, $3, $4, $5, $6, $7, 'conflict', 'pending candidate already exists')"
                ),
                &[&candidate_id, &context.document_id, &context.section_key, &proposed_content, &trigger_type, &source_id, &context.base_section_revision],
            )
            .await
            .map_err(|error| error.to_string())?;
        "conflict"
    } else if current_section_revision != context.base_section_revision {
        transaction
            .execute(
                concat!(
                    "INSERT INTO project_document_update_candidates ",
                    "(id, document_id, section_key, proposed_content, source_type, source_id, base_section_revision, status, conflict_reason) ",
                    "VALUES ($1, $2, $3, $4, $5, $6, $7, 'conflict', 'section changed while refresh was running')"
                ),
                &[&candidate_id, &context.document_id, &context.section_key, &proposed_content, &trigger_type, &source_id, &context.base_section_revision],
            )
            .await
            .map_err(|error| error.to_string())?;
        "conflict"
    } else if locked_by_human {
        // Human 锁定章节只生成 pending 候选，文档内容、版本号和刷新水位保持不变。
        transaction
            .execute(
                concat!(
                    "INSERT INTO project_document_update_candidates ",
                    "(id, document_id, section_key, proposed_content, source_type, source_id, base_section_revision, status) ",
                    "VALUES ($1, $2, $3, $4, $5, $6, $7, 'pending')"
                ),
                &[&candidate_id, &context.document_id, &context.section_key, &proposed_content, &trigger_type, &source_id, &context.base_section_revision],
            )
            .await
            .map_err(|error| error.to_string())?;
        "pending"
    } else {
        // 未锁定且无待审候选时立即应用，并把候选保留为可审计的 applied 记录。
        transaction
            .execute(
                concat!(
                    "INSERT INTO project_document_update_candidates ",
                    "(id, document_id, section_key, proposed_content, source_type, source_id, base_section_revision, status, resolved_at) ",
                    "VALUES ($1, $2, $3, $4, $5, $6, $7, 'applied', now())"
                ),
                &[&candidate_id, &context.document_id, &context.section_key, &proposed_content, &trigger_type, &source_id, &context.base_section_revision],
            )
            .await
            .map_err(|error| error.to_string())?;
        transaction
            .execute(
                "UPDATE project_document_sections SET content = $3, revision = revision + 1, updated_at = now() WHERE document_id = $1 AND section_key = $2",
                &[&context.document_id, &context.section_key, &proposed_content],
            )
            .await
            .map_err(|error| error.to_string())?;

        // 新版本保存更新后所有章节的完整快照，回滚和历史查看不依赖当前章节表。
        let version_content = transaction
            .query_one(
                concat!(
                    "SELECT jsonb_build_object('sections', jsonb_agg(jsonb_build_object('section_key', section_key, 'title', title, ",
                    "'content', content, 'sort_order', sort_order, 'locked_by_human', locked_by_human, 'revision', revision) ",
                    "ORDER BY sort_order, section_key))::text FROM project_document_sections WHERE document_id = $1"
                ),
                &[&context.document_id],
            )
            .await
            .map_err(|error| error.to_string())?
            .get::<_, String>(0);
        let next_version_no = current_version_no + 1;
        transaction
            .execute(
                concat!(
                    "INSERT INTO project_document_versions ",
                    "(id, document_id, version_no, content, content_hash, source_type, source_task_id, created_by_actor_id, metadata) ",
                    "VALUES ($1, $2, $3, $4, md5($4), 'agent_refresh', $5, $6, $7)"
                ),
                &[
                    &Uuid::new_v4().to_string(),
                    &context.document_id,
                    &next_version_no,
                    &version_content,
                    &source_task_id,
                    &runner_id,
                    &json!({
                        "job_id": job.job_id.clone(),
                        "attempt_id": job.attempt_id.clone(),
                        "candidate_id": candidate_id.clone(),
                        "section_key": context.section_key.clone(),
                        "trigger_type": trigger_type,
                        "source_conversation_id": source_conversation_id
                    }),
                ],
            )
            .await
            .map_err(|error| error.to_string())?;
        transaction
            .execute(
                "UPDATE project_documents SET current_version_no = $2, revision = revision + 1, last_refreshed_at = now(), updated_at = now() WHERE id = $1",
                &[&context.document_id, &next_version_no],
            )
            .await
            .map_err(|error| error.to_string())?;
        "applied"
    };

    // 候选结果和 Codex 原始输出进入统一运行记录，调用方可从事件直接识别 applied、pending 或 conflict。
    transaction
        .execute(
            concat!(
                "INSERT INTO execution_events (job_id, attempt_id, task_id, project_id, agent_id, conversation_id, event_type, payload) ",
                "VALUES ($1, $2, $3, $4, $5, $6, 'document.refresh_completed', $7)"
            ),
            &[
                &job.job_id,
                &job.attempt_id,
                &job.task_id,
                &job.project_id,
                &job.agent_id,
                &job.conversation_id,
                &json!({
                    "document_id": context.document_id.clone(),
                    "section_key": context.section_key.clone(),
                    "candidate_id": candidate_id,
                    "candidate_status": candidate_status
                }),
            ],
        )
        .await
        .map_err(|error| error.to_string())?;
    finish_general_records(
        &transaction,
        runner_id,
        job,
        "document_update_candidate",
        proposed_content,
        thread_id,
    )
    .await?;
    transaction
        .commit()
        .await
        .map_err(|error| error.to_string())
}

/// 在调用方事务内保存通用作业输出、尝试状态和审计事件。
async fn finish_general_records(
    transaction: &tokio_postgres::Transaction<'_>,
    runner_id: &str,
    job: &ClaimedJob,
    output_type: &str,
    output_content: &str,
    thread_id: Option<&str>,
) -> Result<(), String> {
    // 输出携带全部可用业务作用域，后续页面无需解析 payload 才能定位来源。
    let workflow_run_id = job
        .payload
        .get("run_id")
        .or_else(|| job.payload.get("workflow_run_id"))
        .and_then(serde_json::Value::as_str);
    let node_run_id = job
        .payload
        .get("node_run_id")
        .and_then(serde_json::Value::as_str);
    transaction
        .execute(
            concat!(
                "INSERT INTO run_outputs (id, job_id, task_id, project_id, agent_id, conversation_id, workflow_run_id, node_run_id, output_type, content) ",
                "VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)"
            ),
            &[
                &Uuid::new_v4().to_string(), &job.job_id, &job.task_id, &job.project_id, &job.agent_id, &job.conversation_id,
                &workflow_run_id, &node_run_id, &output_type, &output_content,
            ],
        )
        .await
        .map_err(|error| error.to_string())?;
    transaction.execute("INSERT INTO execution_events (job_id, attempt_id, task_id, project_id, agent_id, conversation_id, event_type, payload) VALUES ($1, $2, $3, $4, $5, $6, 'output.created', $7)", &[&job.job_id, &job.attempt_id, &job.task_id, &job.project_id, &job.agent_id, &job.conversation_id, &json!({ "output_type": output_type, "codex_thread_id": thread_id })]).await.map_err(|error| error.to_string())?;

    // 尝试与作业在同一事务进入成功状态，避免输出可见但作业仍显示运行中。
    transaction.execute("UPDATE execution_attempts SET status = 'succeeded', heartbeat_at = now(), finished_at = now(), codex_thread_id = $2 WHERE id = $1", &[&job.attempt_id, &thread_id]).await.map_err(|error| error.to_string())?;
    transaction
        .execute(
            "UPDATE execution_jobs SET status = 'succeeded', updated_at = now() WHERE id = $1",
            &[&job.job_id],
        )
        .await
        .map_err(|error| error.to_string())?;
    transaction.execute("INSERT INTO execution_events (job_id, attempt_id, task_id, project_id, agent_id, conversation_id, event_type, payload) VALUES ($1, $2, $3, $4, $5, $6, 'job.succeeded', $7)", &[&job.job_id, &job.attempt_id, &job.task_id, &job.project_id, &job.agent_id, &job.conversation_id, &json!({ "runner_id": runner_id, "kind": job.kind.clone(), "codex_thread_id": thread_id })]).await.map_err(|error| error.to_string())?;
    Ok(())
}

/// 在业务状态更新事务内统一写入输出、成功状态和审计事件。
async fn finish_job_records(
    transaction: &tokio_postgres::Transaction<'_>,
    job: &ClaimedJob,
    task_id: &str,
    output_type: &str,
    output_content: &str,
    runner_id: &str,
    thread_id: Option<&str>,
) -> Result<(), String> {
    // 任务输出同时带上项目和 Agent 作用域，便于后续运行记录和记忆提取使用。
    let workflow_run_id = job
        .payload
        .get("workflow_run_id")
        .and_then(serde_json::Value::as_str);
    let node_run_id = job
        .payload
        .get("node_run_id")
        .and_then(serde_json::Value::as_str);
    transaction
        .execute(
            concat!(
                "INSERT INTO run_outputs (id, job_id, task_id, project_id, agent_id, workflow_run_id, node_run_id, output_type, content) ",
                "VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)"
            ),
            &[&Uuid::new_v4().to_string(), &job.job_id, &task_id, &job.project_id, &job.agent_id, &workflow_run_id, &node_run_id, &output_type, &output_content],
        )
        .await
        .map_err(|error| error.to_string())?;
    transaction.execute("INSERT INTO execution_events (job_id, attempt_id, task_id, event_type, payload) VALUES ($1, $2, $3, 'output.created', $4)", &[&job.job_id, &job.attempt_id, &job.task_id, &json!({ "output_type": output_type, "codex_thread_id": thread_id })]).await.map_err(|error| error.to_string())?;
    transaction.execute("UPDATE execution_attempts SET status = 'succeeded', heartbeat_at = now(), finished_at = now(), codex_thread_id = $2 WHERE id = $1", &[&job.attempt_id, &thread_id]).await.map_err(|error| error.to_string())?;
    transaction
        .execute(
            "UPDATE execution_jobs SET status = 'succeeded', updated_at = now() WHERE id = $1",
            &[&job.job_id],
        )
        .await
        .map_err(|error| error.to_string())?;
    transaction.execute("INSERT INTO execution_events (job_id, attempt_id, task_id, event_type, payload) VALUES ($1, $2, $3, 'job.succeeded', $4)", &[&job.job_id, &job.attempt_id, &job.task_id, &json!({ "runner_id": runner_id, "kind": job.kind.clone(), "codex_thread_id": thread_id })]).await.map_err(|error| error.to_string())?;
    Ok(())
}

/// 为失败作业补写失败尝试和事件，避免执行事务回滚后丢失失败原因。
async fn mark_failed(database_url: &str, job: &ClaimedJob, message: &str) -> Result<(), String> {
    let mut client = connect(database_url).await?;
    let transaction = client
        .transaction()
        .await
        .map_err(|error| error.to_string())?;
    transaction.execute("UPDATE execution_attempts SET status = 'failed', finished_at = now(), failure_message = $2 WHERE id = $1", &[&job.attempt_id, &message]).await.map_err(|error| error.to_string())?;
    // 第一次和第二次失败分别延迟 1 分钟、5 分钟，达到三次总尝试后停止自动重试。
    let job_row = transaction.query_one("UPDATE execution_jobs SET status = CASE WHEN attempt_count >= max_attempts THEN 'failed' ELSE 'queued' END, available_at = CASE WHEN attempt_count >= max_attempts THEN available_at WHEN attempt_count = 1 THEN now() + interval '1 minute' ELSE now() + interval '5 minutes' END, updated_at = now() WHERE id = $1 RETURNING status", &[&job.job_id]).await.map_err(|error| error.to_string())?;
    let job_status = job_row.get::<_, String>(0);
    if job.kind != "refresh_project_document" {
        if let Some(task_id) = job.task_id.as_ref() {
            // 达到重试上限才让任务显示失败，仍可重试的作业保持 queued 语义。
            let task_status = if job_status == "failed" {
                "failed"
            } else {
                "queued"
            };
            transaction
                .execute(
                    "UPDATE tasks SET execution_status = $2, updated_at = now() WHERE id = $1 AND board_stage IN ('todo', 'in_progress')",
                    &[task_id, &task_status],
                )
                .await
                .map_err(|error| error.to_string())?;
        }
    }

    // 对话总结达到重试上限后恢复可编辑状态，避免永久停留在 archiving。
    if job_status == "failed" {
        if let Some(conversation_id) = job.conversation_id.as_ref() {
            transaction.execute("UPDATE conversations SET status = 'active', updated_at = now() WHERE id = $1 AND status = 'archiving'", &[&conversation_id]).await.map_err(|error| error.to_string())?;
        }
        if job.kind == "run_workflow" {
            if let Some(run_id) = job
                .payload
                .get("run_id")
                .and_then(serde_json::Value::as_str)
            {
                // 工作流推进作业耗尽重试后同步关闭运行和父任务，避免看板永久停留在运行中。
                transaction
                    .execute(
                        concat!(
                            "UPDATE workflow_runs SET status = 'failed', error_message = $2, finished_at = now(), updated_at = now() ",
                            "WHERE id = $1 AND status NOT IN ('succeeded', 'terminated')"
                        ),
                        &[&run_id, &message],
                    )
                    .await
                    .map_err(|error| error.to_string())?;
                transaction
                    .execute(
                        concat!(
                            "UPDATE tasks SET execution_status = 'failed', revision = revision + 1, updated_at = now() ",
                            "WHERE id = (SELECT parent_task_id FROM workflow_runs WHERE id = $1)"
                        ),
                        &[&run_id],
                    )
                    .await
                    .map_err(|error| error.to_string())?;
                transaction
                    .execute(
                        "INSERT INTO workflow_run_events (run_id, event_type, payload) VALUES ($1, 'run.failed', $2)",
                        &[&run_id, &json!({ "message": message, "job_id": job.job_id.clone() })],
                    )
                    .await
                    .map_err(|error| error.to_string())?;
            }
        }
    }
    transaction.execute("INSERT INTO execution_events (job_id, attempt_id, task_id, project_id, agent_id, conversation_id, event_type, payload) VALUES ($1, $2, $3, $4, $5, $6, 'job.failed', $7)", &[&job.job_id, &job.attempt_id, &job.task_id, &job.project_id, &job.agent_id, &job.conversation_id, &json!({ "message": message })]).await.map_err(|error| error.to_string())?;
    transaction
        .commit()
        .await
        .map_err(|error| error.to_string())
}
