mod codex;

use codex::{CodexConfig, TaskPromptContext};
use serde_json::json;
use std::{env, sync::Arc, time::Duration};
use tokio::task::JoinHandle;
use tokio_postgres::{Client, NoTls};
use uuid::Uuid;

/// 已领取的执行作业，携带本次尝试的租约身份。
struct ClaimedJob {
    /// 作业主键。
    job_id: String,
    /// 作业类型，M2 只允许受控的 execute_task。
    kind: String,
    /// 关联任务主键。
    task_id: Option<String>,
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

/// 从数据库读取的最小任务上下文，用于构造 Codex 提示和工作区。
struct TaskContext {
    /// 项目主键。
    project_id: String,
    /// 项目名称。
    project_name: String,
    /// 任务标题。
    title: String,
    /// 任务说明。
    description: String,
}

/// 运行器入口：注册实例、续租并循环领取 M2 执行作业。
#[tokio::main]
async fn main() {
    // 读取运行时配置，保持 Runner 身份和数据库依赖可由 Compose 注入。
    let database_url = env::var("DATABASE_URL").expect("DATABASE_URL must be configured");
    let runner_id = env::var("RUNNER_ID").unwrap_or_else(|_| "runner-1".to_owned());
    let runner_name = env::var("RUNNER_NAME").unwrap_or_else(|_| runner_id.clone());
    let codex_config = Arc::new(CodexConfig::from_env().expect("load Codex runtime configuration"));
    let lease_seconds = codex_config.lease_seconds();
    println!("xiexu runner {runner_id} starting with Codex mode {}", codex_config.mode_name());

    // 心跳与领取使用独立节拍，后台执行期间仍持续续租 Runner 和 attempt。
    let mut heartbeat_tick = tokio::time::interval(Duration::from_secs(10));
    let mut claim_tick = tokio::time::interval(Duration::from_secs(2));
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
        }
    }
}

/// 周期扫描 Todo，按任务 revision 幂等创建方案生成作业。
async fn scan_todo_tasks(database_url: &str) -> Result<(), String> {
    let mut client = connect(database_url).await?;
    let transaction = client.transaction().await.map_err(|error| error.to_string())?;
    let rows = transaction.query("SELECT t.id, t.revision FROM tasks t WHERE t.board_stage = 'todo' AND t.requires_plan_confirmation = TRUE AND NOT EXISTS (SELECT 1 FROM execution_jobs j WHERE j.task_id = t.id AND j.kind = 'prepare_task_plan' AND j.status IN ('queued', 'running')) FOR UPDATE SKIP LOCKED", &[]).await.map_err(|error| error.to_string())?;
    for row in rows {
        let task_id = row.get::<_, String>(0);
        let revision = row.get::<_, i64>(1);
        let dedupe_key = format!("task:{task_id}:plan:{revision}");
        let inserted = transaction.execute("INSERT INTO execution_jobs (id, kind, status, task_id, payload, dedupe_key) VALUES ($1, 'prepare_task_plan', 'queued', $2, $3, $4) ON CONFLICT (dedupe_key) DO NOTHING", &[&Uuid::new_v4().to_string(), &task_id, &json!({ "task_id": task_id.clone(), "revision": revision }), &dedupe_key]).await.map_err(|error| error.to_string())?;
        if inserted == 1 {
            transaction.execute("UPDATE tasks SET execution_status = 'queued', updated_at = now() WHERE id = $1 AND board_stage = 'todo'", &[&task_id]).await.map_err(|error| error.to_string())?;
            transaction.execute("INSERT INTO task_events (task_id, event_type, actor_type, actor_id, event_data) VALUES ($1, 'execution.plan_queued', 'system', 'runner', $2)", &[&task_id, &json!({ "kind": "prepare_task_plan", "dedupe_key": dedupe_key })]).await.map_err(|error| error.to_string())?;
        }
    }
    transaction.commit().await.map_err(|error| error.to_string())
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
async fn heartbeat_attempt(database_url: &str, attempt_id: &str, lease_seconds: i32) -> Result<(), String> {
    let client = connect(database_url).await?;
    // 租约秒数已由配置边界限制为正整数，嵌入 SQL 字面量可避免驱动缺少 interval 参数编码器的问题。
    let query = format!(
        "UPDATE execution_attempts SET heartbeat_at = now(), lease_expires_at = now() + interval '{lease_seconds} seconds' WHERE id = $1 AND status = 'running'",
    );
    client.execute(&query, &[&attempt_id]).await.map(|_| ()).map_err(|error| error.to_string())
}

/// 建立短生命周期数据库连接，并把连接任务交给 Tokio 维护。
async fn connect(database_url: &str) -> Result<Client, String> {
    let (client, connection) = tokio_postgres::connect(database_url, NoTls).await.map_err(|error| error.to_string())?;
    tokio::spawn(async move {
        if let Err(error) = connection.await {
            eprintln!("runner database connection ended: {error}");
        }
    });
    Ok(client)
}

/// 原子领取一个到期可执行作业，并为本次领取建立租约尝试。
async fn claim_job(database_url: &str, runner_id: &str, lease_seconds: i32) -> Result<Option<ClaimedJob>, String> {
    let mut client = connect(database_url).await?;
    let transaction = client.transaction().await.map_err(|error| error.to_string())?;

    // 先回收已经失去租约的作业，允许其在最大尝试次数内重新排队。
    transaction.execute("UPDATE execution_jobs SET status = 'queued', available_at = now(), updated_at = now() WHERE status = 'running' AND attempt_count < max_attempts AND EXISTS (SELECT 1 FROM execution_attempts a WHERE a.job_id = execution_jobs.id AND a.status = 'running' AND a.lease_expires_at < now())", &[]).await.map_err(|error| error.to_string())?;
    transaction.execute("UPDATE execution_attempts SET status = 'expired', finished_at = now(), failure_message = 'lease expired' WHERE status = 'running' AND lease_expires_at < now()", &[]).await.map_err(|error| error.to_string())?;
    let row = transaction.query_opt("SELECT id, kind, task_id FROM execution_jobs WHERE status = 'queued' AND available_at <= now() AND attempt_count < max_attempts ORDER BY created_at ASC FOR UPDATE SKIP LOCKED LIMIT 1", &[]).await.map_err(|error| error.to_string())?;
    let Some(row) = row else {
        transaction.commit().await.map_err(|error| error.to_string())?;
        return Ok(None);
    };
    let job_id = row.get::<_, String>(0);
    let kind = row.get::<_, String>(1);
    let task_id = row.get::<_, Option<String>>(2);
    let attempt_id = Uuid::new_v4().to_string();

    // 领取和任务执行态更新处于同一事务，避免出现有尝试但任务仍显示空闲的窗口。
    transaction.execute("UPDATE execution_jobs SET status = 'running', attempt_count = attempt_count + 1, updated_at = now() WHERE id = $1", &[&job_id]).await.map_err(|error| error.to_string())?;
    // 租约秒数只来自内部受限配置，使用字面量避免 interval 参数的驱动编码限制。
    let attempt_insert = format!(
        "INSERT INTO execution_attempts (id, job_id, runner_instance_id, status, lease_expires_at) VALUES ($1, $2, $3, 'running', now() + interval '{lease_seconds} seconds')",
    );
    transaction.execute(&attempt_insert, &[&attempt_id, &job_id, &runner_id]).await.map_err(|error| error.to_string())?;
    if let Some(task_id) = task_id.as_ref() {
        transaction.execute("UPDATE tasks SET execution_status = 'running', updated_at = now() WHERE id = $1 AND board_stage = 'in_progress'", &[task_id]).await.map_err(|error| error.to_string())?;
    }
    transaction.execute("INSERT INTO execution_events (job_id, attempt_id, task_id, event_type, payload) VALUES ($1, $2, $3, 'job.claimed', $4)", &[&job_id, &attempt_id, &task_id, &json!({ "runner_id": runner_id })]).await.map_err(|error| error.to_string())?;
    transaction.commit().await.map_err(|error| error.to_string())?;
    Ok(Some(ClaimedJob { job_id, kind, task_id, attempt_id }))
}

/// 执行白名单作业，真实模式调用 Codex，受控模式保留可验证的本地输出。
async fn execute_job(database_url: &str, runner_id: &str, job: ClaimedJob, codex_config: &CodexConfig) -> Result<(), String> {
    // 先验证作业类型和任务关联，避免无效请求启动外部进程。
    if job.kind != "execute_task" && job.kind != "prepare_task_plan" {
        return mark_failed(database_url, &job, "unsupported execution kind").await;
    }
    let Some(task_id) = job.task_id.as_ref() else {
        return mark_failed(database_url, &job, "task execution requires task_id").await;
    };
    let task = load_task_context(database_url, task_id).await?;
    mark_attempt_started(database_url, runner_id, &job, codex_config.mode_name()).await?;

    // 真实模式只把任务业务字段传给 Codex，数据库配置和其他环境不会进入提示。
    let execution_result = if codex_config.is_real() {
        codex_config.run(&job.kind, TaskPromptContext { project_id: &task.project_id, project_name: &task.project_name, title: &task.title, description: &task.description }).await.map(|output| (output.content, output.thread_id))
    } else {
        let content = if job.kind == "prepare_task_plan" { "方案草案已生成，等待 Human 确认。" } else { "受控执行完成，等待 Human 验收。" };
        Ok((content.to_owned(), None))
    };
    let (output_content, thread_id) = match execution_result {
        Ok(output) => output,
        Err(error) => {
            mark_failed(database_url, &job, &error).await?;
            return Err(error);
        }
    };
    if job.kind == "prepare_task_plan" {
        finish_plan_job(database_url, runner_id, &job, task_id, &output_content, thread_id.as_deref()).await
    } else {
        finish_execution_job(database_url, runner_id, &job, task_id, &output_content, thread_id.as_deref()).await
    }
}

/// 查询任务和项目的最小提示上下文，不把执行控制字段交给 Codex。
async fn load_task_context(database_url: &str, task_id: &str) -> Result<TaskContext, String> {
    let client = connect(database_url).await?;
    let row = client.query_opt("SELECT p.id, p.name, t.title, t.description FROM tasks t JOIN projects p ON p.id = t.project_id WHERE t.id = $1", &[&task_id]).await.map_err(|error| error.to_string())?.ok_or_else(|| "task not found".to_owned())?;
    Ok(TaskContext { project_id: row.get(0), project_name: row.get(1), title: row.get(2), description: row.get(3) })
}

/// 在外部执行前提交 started 事件，长任务运行期间前端即可看到真实状态。
async fn mark_attempt_started(database_url: &str, runner_id: &str, job: &ClaimedJob, mode: &str) -> Result<(), String> {
    let client = connect(database_url).await?;
    client.execute("INSERT INTO execution_events (job_id, attempt_id, task_id, event_type, payload) VALUES ($1, $2, $3, 'attempt.started', $4)", &[&job.job_id, &job.attempt_id, &job.task_id, &json!({ "runner_id": runner_id, "kind": job.kind.clone(), "mode": mode })]).await.map(|_| ()).map_err(|error| error.to_string())
}

/// 完成方案作业，把 Todo 推进到等待 Human 确认并保存 Codex 输出。
async fn finish_plan_job(database_url: &str, runner_id: &str, job: &ClaimedJob, task_id: &str, output_content: &str, thread_id: Option<&str>) -> Result<(), String> {
    let mut client = connect(database_url).await?;
    let transaction = client.transaction().await.map_err(|error| error.to_string())?;
    let updated = transaction.execute("UPDATE tasks SET board_stage = 'plan_review', plan_status = 'reviewing', execution_status = 'succeeded', revision = revision + 1, updated_at = now() WHERE id = $1 AND board_stage = 'todo'", &[&task_id]).await.map_err(|error| error.to_string())?;
    if updated != 1 {
        drop(transaction);
        return mark_failed(database_url, job, "task is no longer in todo").await;
    }
    transaction.execute("INSERT INTO task_events (task_id, event_type, actor_type, actor_id, after_data, event_data) VALUES ($1, 'plan.generated', 'runner', $2, $3, $4)", &[&task_id, &runner_id, &json!({ "board_stage": "plan_review", "plan_status": "reviewing" }), &json!({ "job_id": job.job_id.clone(), "attempt_id": job.attempt_id.clone(), "codex_thread_id": thread_id })]).await.map_err(|error| error.to_string())?;
    finish_job_records(&transaction, job, task_id, "plan", output_content, runner_id, thread_id).await?;
    transaction.commit().await.map_err(|error| error.to_string())
}

/// 完成执行作业，把任务推进到等待验收并保存 Codex 输出。
async fn finish_execution_job(database_url: &str, runner_id: &str, job: &ClaimedJob, task_id: &str, output_content: &str, thread_id: Option<&str>) -> Result<(), String> {
    let mut client = connect(database_url).await?;
    let transaction = client.transaction().await.map_err(|error| error.to_string())?;
    let updated = transaction.execute("UPDATE tasks SET board_stage = 'acceptance', execution_status = 'succeeded', progress_percent = 100, revision = revision + 1, updated_at = now() WHERE id = $1 AND board_stage = 'in_progress'", &[&task_id]).await.map_err(|error| error.to_string())?;
    if updated != 1 {
        drop(transaction);
        return mark_failed(database_url, job, "task is no longer in progress").await;
    }
    transaction.execute("INSERT INTO task_events (task_id, event_type, actor_type, actor_id, after_data, event_data) VALUES ($1, 'execution.completed', 'runner', $2, $3, $4)", &[&task_id, &runner_id, &json!({ "board_stage": "acceptance", "execution_status": "succeeded" }), &json!({ "job_id": job.job_id.clone(), "attempt_id": job.attempt_id.clone(), "codex_thread_id": thread_id })]).await.map_err(|error| error.to_string())?;
    finish_job_records(&transaction, job, task_id, "summary", output_content, runner_id, thread_id).await?;
    transaction.commit().await.map_err(|error| error.to_string())
}

/// 在业务状态更新事务内统一写入输出、成功状态和审计事件。
async fn finish_job_records(transaction: &tokio_postgres::Transaction<'_>, job: &ClaimedJob, task_id: &str, output_type: &str, output_content: &str, runner_id: &str, thread_id: Option<&str>) -> Result<(), String> {
    transaction.execute("INSERT INTO run_outputs (id, job_id, task_id, output_type, content) VALUES ($1, $2, $3, $4, $5)", &[&Uuid::new_v4().to_string(), &job.job_id, &task_id, &output_type, &output_content]).await.map_err(|error| error.to_string())?;
    transaction.execute("INSERT INTO execution_events (job_id, attempt_id, task_id, event_type, payload) VALUES ($1, $2, $3, 'output.created', $4)", &[&job.job_id, &job.attempt_id, &job.task_id, &json!({ "output_type": output_type, "codex_thread_id": thread_id })]).await.map_err(|error| error.to_string())?;
    transaction.execute("UPDATE execution_attempts SET status = 'succeeded', heartbeat_at = now(), finished_at = now(), codex_thread_id = $2 WHERE id = $1", &[&job.attempt_id, &thread_id]).await.map_err(|error| error.to_string())?;
    transaction.execute("UPDATE execution_jobs SET status = 'succeeded', updated_at = now() WHERE id = $1", &[&job.job_id]).await.map_err(|error| error.to_string())?;
    transaction.execute("INSERT INTO execution_events (job_id, attempt_id, task_id, event_type, payload) VALUES ($1, $2, $3, 'job.succeeded', $4)", &[&job.job_id, &job.attempt_id, &job.task_id, &json!({ "runner_id": runner_id, "kind": job.kind.clone(), "codex_thread_id": thread_id })]).await.map_err(|error| error.to_string())?;
    Ok(())
}

/// 为失败作业补写失败尝试和事件，避免执行事务回滚后丢失失败原因。
async fn mark_failed(database_url: &str, job: &ClaimedJob, message: &str) -> Result<(), String> {
    let mut client = connect(database_url).await?;
    let transaction = client.transaction().await.map_err(|error| error.to_string())?;
    transaction.execute("UPDATE execution_attempts SET status = 'failed', finished_at = now(), failure_message = $2 WHERE id = $1", &[&job.attempt_id, &message]).await.map_err(|error| error.to_string())?;
    // 第一次和第二次失败分别延迟 1 分钟、5 分钟，达到三次总尝试后停止自动重试。
    let job_row = transaction.query_one("UPDATE execution_jobs SET status = CASE WHEN attempt_count >= max_attempts THEN 'failed' ELSE 'queued' END, available_at = CASE WHEN attempt_count >= max_attempts THEN available_at WHEN attempt_count = 1 THEN now() + interval '1 minute' ELSE now() + interval '5 minutes' END, updated_at = now() WHERE id = $1 RETURNING status", &[&job.job_id]).await.map_err(|error| error.to_string())?;
    let job_status = job_row.get::<_, String>(0);
    if let Some(task_id) = job.task_id.as_ref() {
        // 达到重试上限才让任务显示失败，仍可重试的作业保持 queued 语义。
        let task_status = if job_status == "failed" { "failed" } else { "queued" };
        transaction.execute("UPDATE tasks SET execution_status = $2, updated_at = now() WHERE id = $1 AND board_stage IN ('todo', 'in_progress')", &[task_id, &task_status]).await.map_err(|error| error.to_string())?;
    }
    transaction.execute("INSERT INTO execution_events (job_id, attempt_id, task_id, event_type, payload) VALUES ($1, $2, $3, 'job.failed', $4)", &[&job.job_id, &job.attempt_id, &job.task_id, &json!({ "message": message })]).await.map_err(|error| error.to_string())?;
    transaction.commit().await.map_err(|error| error.to_string())
}
