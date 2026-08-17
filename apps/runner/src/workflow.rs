use crate::{
    codex::{CodexConfig, TaskPromptContext},
    connect, finish_general_records, mark_attempt_started, ClaimedJob,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use tokio_postgres::Transaction;
use uuid::Uuid;

/// 单次工作流作业最多连续推进的节点数，阻止异常环路长期占用 Runner。
const MAX_NODES_PER_SLICE: usize = 100;

/// 发布版本中的工作流节点，运行期只读取快照版本而不跟随画布草稿变化。
struct WorkflowNode {
    /// 版本内稳定节点键。
    key: String,
    /// 节点类型，支持开始、结束、执行、判断和人工确认。
    node_type: String,
    /// 画布展示名称，同时作为执行任务的默认标题。
    label: String,
    /// 节点保存的结构化配置和用户文字说明。
    config: Value,
}

/// 发布版本中的有向连接线，判断分支通过是/否标签选择。
struct WorkflowEdge {
    /// 来源节点键。
    source: String,
    /// 目标节点键。
    target: String,
    /// 画布上展示的分支标签。
    label: String,
    /// 预留的结构化条件，当前只用于兼容显式布尔分支值。
    condition: Value,
}

/// 工作流运行快照和执行 Agent 上下文。
struct WorkflowSnapshot {
    /// 工作流运行主键。
    run_id: String,
    /// 所属工作流主键。
    workflow_id: String,
    /// 创建运行时固定的发布版本主键。
    version_id: String,
    /// 所属项目主键。
    project_id: String,
    /// 工作流名称。
    workflow_name: String,
    /// 项目名称。
    project_name: String,
    /// 项目协调 Agent 主键。
    agent_id: String,
    /// 项目协调 Agent 名称。
    agent_name: String,
    /// 项目协调 Agent 的合并职责约束。
    agent_instructions: String,
    /// 本次运行的不可变业务输入。
    input: Value,
    /// 任务面板中代表整个工作流运行的父任务。
    parent_task_id: String,
    /// 发布版本节点表。
    nodes: BTreeMap<String, WorkflowNode>,
    /// 发布版本连接线表。
    edges: Vec<WorkflowEdge>,
}

/// 保存在 workflow_runs.output.runtime 下的可恢复运行游标。
#[derive(Default, Deserialize, Serialize)]
struct WorkflowRuntimeState {
    /// 下一步需要处理的节点键。
    current_node_key: Option<String>,
    /// 已完成节点键，既用于展示也用于阻止异常环路重复执行。
    #[serde(default)]
    completed_node_keys: Vec<String>,
    /// 正在等待的执行或人工确认节点键。
    waiting_node_key: Option<String>,
    /// 执行节点派生的任务主键。
    waiting_task_id: Option<String>,
    /// 各节点的结构化输出，供后续判断和运行详情读取。
    #[serde(default)]
    node_outputs: BTreeMap<String, Value>,
}

/// 扫描到期调度并创建一次运行；停机期间历史触发不补建，只推进到未来最近时间。
pub(crate) async fn scan_workflow_schedules(database_url: &str) -> Result<(), String> {
    // 到期调度和工作流定义同时加锁，多个 Runner 只会消费同一触发一次。
    let mut client = connect(database_url).await?;
    let transaction = client
        .transaction()
        .await
        .map_err(|error| error.to_string())?;
    let rows = transaction
        .query(
            concat!(
                "SELECT schedule.id, schedule.workflow_id, schedule.schedule_type, schedule.schedule_expression, schedule.parsed_rule, ",
                "workflow.project_id, workflow.current_version_no, workflow.name, version.id, coordinator.agent_id ",
                "FROM workflow_schedules schedule JOIN workflows workflow ON workflow.id = schedule.workflow_id AND workflow.status = 'active' ",
                "JOIN workflow_versions version ON version.workflow_id = workflow.id AND version.version_no = workflow.current_version_no ",
                "JOIN project_agents coordinator ON coordinator.project_id = workflow.project_id AND coordinator.assignment_type = 'coordinator' ",
                "AND coordinator.status = 'active' WHERE schedule.enabled = TRUE AND schedule.next_run_at IS NOT NULL ",
                "AND schedule.next_run_at <= now() FOR UPDATE OF schedule SKIP LOCKED"
            ),
            &[],
        )
        .await
        .map_err(|error| format!("load due workflow schedules: {error:?}"))?;

    // 每条到期规则只创建当前一次运行，并为任务面板创建同源父任务。
    for row in rows {
        let schedule_id = row.get::<_, String>(0);
        let workflow_id = row.get::<_, String>(1);
        let schedule_type = row.get::<_, String>(2);
        let expression = row.get::<_, String>(3);
        let parsed_rule = row.get::<_, Value>(4);
        let project_id = row.get::<_, String>(5);
        let workflow_name = row.get::<_, String>(7);
        let version_id = row.get::<_, String>(8);
        let agent_id = row.get::<_, String>(9);
        let run_id = Uuid::new_v4().to_string();
        let parent_task_id = Uuid::new_v4().to_string();
        let job_id = Uuid::new_v4().to_string();
        transaction
            .execute(
                concat!(
                    "INSERT INTO workflow_runs (id, workflow_id, version_id, project_id, status, trigger_type, input) ",
                    "VALUES ($1, $2, $3, $4, 'queued', 'scheduled', $5)"
                ),
                &[&run_id, &workflow_id, &version_id, &project_id, &json!({ "schedule_id": schedule_id.clone() })],
            )
            .await
            .map_err(|error| format!("insert scheduled workflow run: {error:?}"))?;
        transaction
            .execute(
                concat!(
                    "INSERT INTO tasks (id, project_id, title, description, board_stage, plan_status, execution_status, acceptance_status, ",
                    "requires_plan_confirmation, source_type, source_workflow_run_id, workflow_name) ",
                    "VALUES ($1, $2, $3, '工作流运行汇总任务', 'in_progress', 'not_required', 'queued', 'not_started', FALSE, 'workflow_run', $4, $3)"
                ),
                &[&parent_task_id, &project_id, &workflow_name, &run_id],
            )
            .await
            .map_err(|error| format!("insert scheduled workflow parent task: {error:?}"))?;
        transaction
            .execute(
                "UPDATE workflow_runs SET parent_task_id = $2 WHERE id = $1",
                &[&run_id, &parent_task_id],
            )
            .await
            .map_err(|error| format!("link scheduled workflow parent task: {error:?}"))?;
        transaction
            .execute(
                concat!(
                    "INSERT INTO workflow_node_runs (id, run_id, node_key, node_type) SELECT $1 || ':' || node_key, $1, ",
                    "node_key, node_type FROM workflow_nodes WHERE version_id = $2 ON CONFLICT (run_id, node_key) DO NOTHING"
                ),
                &[&run_id, &version_id],
            )
            .await
            .map_err(|error| format!("insert scheduled workflow node runs: {error:?}"))?;
        transaction
            .execute(
                "INSERT INTO execution_jobs (id, kind, status, project_id, agent_id, payload) VALUES ($1, 'run_workflow', 'queued', $2, $3, $4)",
                &[
                    &job_id,
                    &project_id,
                    &agent_id,
                    &json!({ "run_id": run_id.clone(), "schedule_id": schedule_id.clone() }),
                ],
            )
            .await
            .map_err(|error| format!("insert scheduled workflow job: {error:?}"))?;
        transaction
            .execute(
                "INSERT INTO workflow_run_events (run_id, event_type, payload) VALUES ($1, 'run.queued', $2)",
                &[&run_id, &json!({ "trigger_type": "scheduled", "schedule_id": schedule_id.clone() })],
            )
            .await
            .map_err(|error| format!("insert scheduled workflow event: {error:?}"))?;

        // 单次规则消费后停用；周期和 AI 规则直接跳到未来时间，不重放停机期间的遗漏周期。
        let interval_seconds = parsed_rule
            .get("interval_seconds")
            .and_then(Value::as_i64)
            .or_else(|| {
                parsed_rule
                    .pointer("/repeat/interval_seconds")
                    .and_then(Value::as_i64)
            })
            .or_else(|| {
                parsed_rule
                    .get("interval_minutes")
                    .and_then(Value::as_i64)
                    .map(|value| value.saturating_mul(60))
            })
            .or_else(|| {
                parsed_rule
                    .get("interval_hours")
                    .and_then(Value::as_i64)
                    .map(|value| value.saturating_mul(3_600))
            })
            .or_else(|| {
                parsed_rule
                    .get("interval_days")
                    .and_then(Value::as_i64)
                    .map(|value| value.saturating_mul(86_400))
            })
            .or_else(|| expression.parse::<i64>().ok())
            .filter(|seconds| *seconds >= 60);
        if schedule_type == "scheduled" {
            transaction
                .execute(
                    "UPDATE workflow_schedules SET enabled = FALSE, next_run_at = NULL, updated_at = now() WHERE id = $1",
                    &[&schedule_id],
                )
                .await
                .map_err(|error| error.to_string())?;
        } else if let Some(seconds) = interval_seconds {
            transaction
                .execute(
                    "UPDATE workflow_schedules SET next_run_at = now() + ($2::bigint * interval '1 second'), updated_at = now() WHERE id = $1",
                    &[&schedule_id, &seconds],
                )
                .await
                .map_err(|error| error.to_string())?;
        } else {
            transaction
                .execute(
                    concat!(
                        "UPDATE workflow_schedules SET next_run_at = (SELECT min(value::timestamptz) FROM ",
                        "jsonb_array_elements_text(CASE WHEN jsonb_typeof(parsed_rule -> 'occurrences') = 'array' THEN parsed_rule -> 'occurrences' ",
                        "WHEN jsonb_typeof(parsed_rule -> 'next_occurrences') = 'array' THEN parsed_rule -> 'next_occurrences' ELSE '[]'::jsonb END) value ",
                        "WHERE value::timestamptz > now()), enabled = EXISTS (SELECT 1 FROM jsonb_array_elements_text(CASE WHEN ",
                        "jsonb_typeof(parsed_rule -> 'occurrences') = 'array' THEN parsed_rule -> 'occurrences' WHEN ",
                        "jsonb_typeof(parsed_rule -> 'next_occurrences') = 'array' THEN parsed_rule -> 'next_occurrences' ELSE '[]'::jsonb END) value ",
                        "WHERE value::timestamptz > now()), updated_at = now() WHERE id = $1"
                    ),
                    &[&schedule_id],
                )
                .await
                .map_err(|error| error.to_string())?;
        }
    }

    // 运行、父任务、节点实例、作业和下一触发时间作为一个原子批次提交。
    transaction
        .commit()
        .await
        .map_err(|error| error.to_string())
}

/// 周期发现新运行及已完成子任务的挂起运行，并幂等补建推进作业。
pub(crate) async fn scan_workflow_runs(database_url: &str) -> Result<(), String> {
    // 工作流行锁和活跃作业判重放在同一事务，支持未来多个 Runner 并发扫描。
    let mut client = connect(database_url).await?;
    let transaction = client
        .transaction()
        .await
        .map_err(|error| error.to_string())?;
    let rows = transaction
        .query(
            concat!(
                "SELECT wr.id, wr.project_id, coordinator.agent_id FROM workflow_runs wr ",
                "JOIN project_agents coordinator ON coordinator.project_id = wr.project_id AND coordinator.assignment_type = 'coordinator' ",
                "AND coordinator.status = 'active' WHERE (wr.status = 'queued' OR (wr.status = 'waiting_child' AND EXISTS ",
                "(SELECT 1 FROM tasks child WHERE child.id = wr.output #>> '{runtime,waiting_task_id}' AND ",
                "(child.board_stage = 'done' OR child.execution_status = 'failed')))) AND NOT EXISTS (SELECT 1 FROM execution_jobs job ",
                "WHERE job.kind = 'run_workflow' AND job.status IN ('queued', 'running') AND job.payload ->> 'run_id' = wr.id) ",
                "FOR UPDATE OF wr SKIP LOCKED"
            ),
            &[],
        )
        .await
        .map_err(|error| error.to_string())?;

    // 每个运行只建立一个短作业，子任务完成后的继续执行仍复用固定 version_id 快照。
    for row in rows {
        let run_id = row.get::<_, String>(0);
        let project_id = row.get::<_, String>(1);
        let agent_id = row.get::<_, String>(2);
        transaction
            .execute(
                "UPDATE workflow_runs SET status = 'queued', updated_at = now() WHERE id = $1 AND status = 'waiting_child'",
                &[&run_id],
            )
            .await
            .map_err(|error| error.to_string())?;
        transaction
            .execute(
                "INSERT INTO execution_jobs (id, kind, status, project_id, agent_id, payload) VALUES ($1, 'run_workflow', 'queued', $2, $3, $4)",
                &[&Uuid::new_v4().to_string(), &project_id, &agent_id, &json!({ "run_id": run_id })],
            )
            .await
            .map_err(|error| error.to_string())?;
    }

    // 扫描产生的状态恢复和作业同时提交，避免出现 queued 运行却没有可领取作业。
    transaction
        .commit()
        .await
        .map_err(|error| error.to_string())
}

/// 执行一个工作流推进作业，遇到子任务或人工确认时主动结束当前切片。
pub(crate) async fn execute_workflow_job(
    database_url: &str,
    runner_id: &str,
    job: &mut ClaimedJob,
    codex_config: &CodexConfig,
) -> Result<(), String> {
    // run_id 只接受服务端写入的结构化载荷，异常历史作业不会猜测运行身份。
    let run_id = job
        .payload
        .get("run_id")
        .and_then(Value::as_str)
        .ok_or_else(|| "workflow execution requires run_id".to_owned())?
        .to_owned();
    let snapshot = load_workflow_snapshot(database_url, &run_id).await?;
    job.project_id = Some(snapshot.project_id.clone());
    job.agent_id = Some(snapshot.agent_id.clone());

    // 先补齐通用作业作用域并记录 attempt，后续每个节点事件都能追溯到本次领取。
    let scope_client = connect(database_url).await?;
    scope_client
        .execute(
            "UPDATE execution_jobs SET project_id = $2, agent_id = $3, updated_at = now() WHERE id = $1",
            &[&job.job_id, &job.project_id, &job.agent_id],
        )
        .await
        .map_err(|error| error.to_string())?;
    mark_attempt_started(database_url, runner_id, job, codex_config.mode_name()).await?;

    // 初始化运行游标并把 queued 切换为 running；暂停、终止和已完成运行不会被 Runner 反向覆盖。
    if !initialize_workflow_run(database_url, job, &snapshot).await? {
        return finish_workflow_slice(
            database_url,
            runner_id,
            job,
            "工作流当前状态不允许继续执行。",
            "workflow_run_skipped",
        )
        .await;
    }

    // 单个切片连续推进无等待节点，遇到外部等待后释放 Runner 给其他作业。
    for _ in 0..MAX_NODES_PER_SLICE {
        let (status, output, state) = load_workflow_runtime(database_url, &run_id).await?;
        if matches!(
            status.as_str(),
            "paused" | "terminated" | "succeeded" | "failed"
        ) {
            return finish_workflow_slice(
                database_url,
                runner_id,
                job,
                &format!("工作流运行状态：{status}"),
                "workflow_run_state",
            )
            .await;
        }
        let node_key = state
            .current_node_key
            .clone()
            .ok_or_else(|| "workflow runtime has no current node".to_owned())?;
        let node = snapshot
            .nodes
            .get(&node_key)
            .ok_or_else(|| format!("workflow node not found: {node_key}"))?;
        if state
            .completed_node_keys
            .iter()
            .any(|completed| completed == &node_key)
        {
            return fail_workflow_run(
                database_url,
                runner_id,
                job,
                &run_id,
                &node_key,
                "workflow cycle detected",
            )
            .await;
        }

        // 节点类型使用稳定小写值，同时兼容早期 action/decision 命名。
        match node.node_type.as_str() {
            "start" => {
                let target = linear_target(&snapshot, &node.key)?;
                complete_node(
                    database_url,
                    job,
                    &run_id,
                    node,
                    target,
                    json!({ "started": true }),
                )
                .await?;
            }
            "end" => {
                return finish_workflow_success(database_url, runner_id, job, &run_id, node).await
            }
            "execution" | "execute" | "action" => {
                if let Some((failed, result)) =
                    continue_execution_node(database_url, job, &snapshot, node, &output, &state)
                        .await?
                {
                    if failed {
                        return fail_workflow_run(
                            database_url,
                            runner_id,
                            job,
                            &run_id,
                            &node.key,
                            &result,
                        )
                        .await;
                    }
                    return finish_workflow_slice(
                        database_url,
                        runner_id,
                        job,
                        &result,
                        "workflow_execution_wait",
                    )
                    .await;
                }
            }
            "condition" | "decision" => {
                let decision =
                    evaluate_condition_node(codex_config, &snapshot, node, &state).await?;
                let target = decision_target(&snapshot, &node.key, decision)?;
                complete_node(
                    database_url,
                    job,
                    &run_id,
                    node,
                    target,
                    json!({ "decision": decision }),
                )
                .await?;
            }
            "human_confirm" | "human_confirmation" | "human_approval" | "manual_confirmation" => {
                if let Some(decision) = resolved_human_decision(
                    database_url,
                    job,
                    &snapshot.input,
                    &output,
                    &run_id,
                    &node.key,
                )
                .await?
                {
                    let target = human_confirmation_target(&snapshot, &node.key, decision)?;
                    complete_node(
                        database_url,
                        job,
                        &run_id,
                        node,
                        target,
                        json!({ "decision": decision, "actor": "human" }),
                    )
                    .await?;
                } else {
                    return wait_for_human(database_url, runner_id, job, &run_id, node).await;
                }
            }
            unsupported => {
                return fail_workflow_run(
                    database_url,
                    runner_id,
                    job,
                    &run_id,
                    &node.key,
                    &format!("unsupported workflow node type: {unsupported}"),
                )
                .await
            }
        }
    }

    // 超过单切片节点上限通常表示画布存在未被已完成集合捕获的异常分支。
    fail_workflow_run(
        database_url,
        runner_id,
        job,
        &run_id,
        "",
        "workflow exceeded node execution limit",
    )
    .await
}

/// 从固定 version_id 一次加载整张发布画布和协调 Agent，运行期间不读取 current_version_no。
async fn load_workflow_snapshot(
    database_url: &str,
    run_id: &str,
) -> Result<WorkflowSnapshot, String> {
    let client = connect(database_url).await?;
    let row = client
        .query_opt(
            concat!(
                "SELECT wr.workflow_id, wr.version_id, wr.project_id, wr.input, w.name, p.name, a.id, a.name, ",
                "concat_ws(E'\\n', a.instructions, NULLIF(a.responsibility_supplement, ''), NULLIF(coordinator.responsibility_override, '')), wr.parent_task_id ",
                "FROM workflow_runs wr JOIN workflows w ON w.id = wr.workflow_id JOIN workflow_versions version ON version.id = wr.version_id ",
                "AND version.workflow_id = wr.workflow_id JOIN projects p ON p.id = wr.project_id JOIN project_agents coordinator ON ",
                "coordinator.project_id = wr.project_id AND coordinator.assignment_type = 'coordinator' AND coordinator.status = 'active' ",
                "JOIN agents a ON a.id = coordinator.agent_id WHERE wr.id = $1 AND w.project_id = wr.project_id AND wr.parent_task_id IS NOT NULL"
            ),
            &[&run_id],
        )
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "workflow run, fixed version, project, or coordinator not found".to_owned())?;
    let version_id = row.get::<_, String>(1);

    // 节点和连线批量读取，避免每推进一个节点重复访问画布定义。
    let node_rows = client
        .query(
            "SELECT node_key, node_type, label, config FROM workflow_nodes WHERE version_id = $1 ORDER BY node_key",
            &[&version_id],
        )
        .await
        .map_err(|error| error.to_string())?;
    let nodes = node_rows
        .into_iter()
        .map(|node| {
            let key = node.get::<_, String>(0);
            (
                key.clone(),
                WorkflowNode {
                    key,
                    node_type: node.get(1),
                    label: node.get(2),
                    config: node.get(3),
                },
            )
        })
        .collect::<BTreeMap<_, _>>();
    let edge_rows = client
        .query(
            "SELECT source_node_key, target_node_key, label, condition FROM workflow_edges WHERE version_id = $1 ORDER BY edge_key",
            &[&version_id],
        )
        .await
        .map_err(|error| error.to_string())?;
    let edges = edge_rows
        .into_iter()
        .map(|edge| WorkflowEdge {
            source: edge.get(0),
            target: edge.get(1),
            label: edge.get(2),
            condition: edge.get(3),
        })
        .collect();
    Ok(WorkflowSnapshot {
        run_id: run_id.to_owned(),
        workflow_id: row.get(0),
        version_id,
        project_id: row.get(2),
        input: row.get(3),
        parent_task_id: row.get(9),
        workflow_name: row.get(4),
        project_name: row.get(5),
        agent_id: row.get(6),
        agent_name: row.get(7),
        agent_instructions: row.get(8),
        nodes,
        edges,
    })
}

/// 初始化开始节点和运行状态；返回 false 表示暂停、终止或终态运行无需执行。
async fn initialize_workflow_run(
    database_url: &str,
    job: &ClaimedJob,
    snapshot: &WorkflowSnapshot,
) -> Result<bool, String> {
    let mut client = connect(database_url).await?;
    let transaction = client
        .transaction()
        .await
        .map_err(|error| error.to_string())?;
    let row = transaction
        .query_one(
            "SELECT status, output FROM workflow_runs WHERE id = $1 FOR UPDATE",
            &[&snapshot.run_id],
        )
        .await
        .map_err(|error| error.to_string())?;
    let status = row.get::<_, String>(0);
    if matches!(
        status.as_str(),
        "paused" | "terminated" | "succeeded" | "failed"
    ) {
        transaction
            .commit()
            .await
            .map_err(|error| error.to_string())?;
        return Ok(false);
    }
    let mut output = row.get::<_, Value>(1);
    let mut state = runtime_state(&output)?;
    if state.current_node_key.is_none() {
        let starts = snapshot
            .nodes
            .values()
            .filter(|node| node.node_type == "start")
            .collect::<Vec<_>>();
        if starts.len() != 1 {
            return Err(format!(
                "workflow requires exactly one start node, found {}",
                starts.len()
            ));
        }
        state.current_node_key = Some(starts[0].key.clone());
        write_runtime_state(&mut output, &state)?;
        transaction
            .execute(
                "INSERT INTO workflow_run_events (run_id, event_type, payload) VALUES ($1, 'run.started', $2)",
                &[&snapshot.run_id, &json!({ "job_id": job.job_id.clone(), "version_id": snapshot.version_id.clone() })],
            )
            .await
            .map_err(|error| error.to_string())?;
    }
    transaction
        .execute(
            "UPDATE workflow_runs SET status = 'running', output = $2, started_at = COALESCE(started_at, now()), updated_at = now() WHERE id = $1",
            &[&snapshot.run_id, &output],
        )
        .await
        .map_err(|error| error.to_string())?;
    transaction
        .commit()
        .await
        .map_err(|error| error.to_string())?;
    Ok(true)
}

/// 读取运行状态和游标，每个节点前重新读取以感知用户暂停或终止操作。
async fn load_workflow_runtime(
    database_url: &str,
    run_id: &str,
) -> Result<(String, Value, WorkflowRuntimeState), String> {
    let client = connect(database_url).await?;
    let row = client
        .query_one(
            "SELECT status, output FROM workflow_runs WHERE id = $1",
            &[&run_id],
        )
        .await
        .map_err(|error| error.to_string())?;
    let status = row.get::<_, String>(0);
    let output = row.get::<_, Value>(1);
    let state = runtime_state(&output)?;
    Ok((status, output, state))
}

/// 推进开始、判断或已完成执行节点，并在同一事务写入节点完成事件。
async fn complete_node(
    database_url: &str,
    job: &ClaimedJob,
    run_id: &str,
    node: &WorkflowNode,
    target: String,
    node_output: Value,
) -> Result<(), String> {
    let mut client = connect(database_url).await?;
    let transaction = client
        .transaction()
        .await
        .map_err(|error| error.to_string())?;
    let row = transaction
        .query_one(
            "SELECT status, output FROM workflow_runs WHERE id = $1 FOR UPDATE",
            &[&run_id],
        )
        .await
        .map_err(|error| error.to_string())?;
    let status = row.get::<_, String>(0);
    if status != "running" && status != "queued" {
        return Err(format!("workflow run cannot advance from status {status}"));
    }
    let mut output = row.get::<_, Value>(1);
    let mut state = runtime_state(&output)?;
    if state.current_node_key.as_deref() != Some(node.key.as_str()) {
        return Err("workflow current node changed during execution".to_owned());
    }
    state.completed_node_keys.push(node.key.clone());
    state
        .node_outputs
        .insert(node.key.clone(), node_output.clone());
    state.current_node_key = Some(target.clone());
    state.waiting_node_key = None;
    state.waiting_task_id = None;
    write_runtime_state(&mut output, &state)?;

    // 节点结果和新游标同时提交，重启后不会重复执行已经完成的节点。
    transaction
        .execute(
            "UPDATE workflow_runs SET status = 'running', output = $2, updated_at = now() WHERE id = $1",
            &[&run_id, &output],
        )
        .await
        .map_err(|error| error.to_string())?;
    transaction
        .execute(
            concat!(
                "UPDATE workflow_node_runs SET status = 'succeeded', input = CASE WHEN input = '{}'::jsonb THEN $3 ELSE input END, ",
                "output = $4, attempt_count = GREATEST(attempt_count, 1), started_at = COALESCE(started_at, now()), finished_at = now(), ",
                "updated_at = now() WHERE run_id = $1 AND node_key = $2"
            ),
            &[&run_id, &node.key, &json!({ "job_id": job.job_id.clone() }), &node_output],
        )
        .await
        .map_err(|error| error.to_string())?;
    transaction
        .execute(
            "INSERT INTO workflow_run_events (run_id, event_type, node_key, payload) VALUES ($1, 'node.completed', $2, $3)",
            &[
                &run_id,
                &node.key,
                &json!({ "job_id": job.job_id.clone(), "node_type": node.node_type.clone(), "output": node_output, "next_node_key": target }),
            ],
        )
        .await
        .map_err(|error| error.to_string())?;
    update_parent_progress(&transaction, run_id, false).await?;
    transaction
        .commit()
        .await
        .map_err(|error| error.to_string())
}

/// 创建或继续执行节点的可见子任务；返回 Some 表示当前工作流切片需要结束等待。
async fn continue_execution_node(
    database_url: &str,
    job: &ClaimedJob,
    snapshot: &WorkflowSnapshot,
    node: &WorkflowNode,
    _output: &Value,
    state: &WorkflowRuntimeState,
) -> Result<Option<(bool, String)>, String> {
    if state.waiting_node_key.as_deref() == Some(node.key.as_str()) {
        let task_id = state
            .waiting_task_id
            .as_deref()
            .ok_or_else(|| "workflow execution node is missing child task id".to_owned())?;
        let client = connect(database_url).await?;
        let task = client
            .query_one(
                "SELECT board_stage, execution_status FROM tasks WHERE id = $1",
                &[&task_id],
            )
            .await
            .map_err(|error| error.to_string())?;
        let board_stage = task.get::<_, String>(0);
        let execution_status = task.get::<_, String>(1);
        if execution_status == "failed" {
            return Ok(Some((true, format!("工作流子任务 {task_id} 执行失败。"))));
        }
        if board_stage != "done" {
            let client = connect(database_url).await?;
            client
                .execute(
                    "UPDATE workflow_runs SET status = 'waiting_child', updated_at = now() WHERE id = $1 AND status = 'running'",
                    &[&snapshot.run_id],
                )
                .await
                .map_err(|error| error.to_string())?;
            return Ok(Some((
                false,
                format!("工作流正在等待子任务 {task_id} 完成。"),
            )));
        }
        let child_output = client
            .query_opt("SELECT content FROM run_outputs WHERE task_id = $1 ORDER BY created_at DESC LIMIT 1", &[&task_id])
            .await
            .map_err(|error| error.to_string())?
            .map(|row| row.get::<_, String>(0))
            .unwrap_or_else(|| "子任务已完成，未产生文本输出。".to_owned());
        let target = linear_target(snapshot, &node.key)?;
        complete_node(
            database_url,
            job,
            &snapshot.run_id,
            node,
            target,
            json!({ "task_id": task_id, "content": child_output }),
        )
        .await?;
        return Ok(None);
    }

    // 首次进入执行节点时由协调 Agent 指派配置 Agent，缺失或失效时回退项目协调者。
    let requested_agent_id = node.config.get("agent_id").and_then(Value::as_str);
    let mut client = connect(database_url).await?;
    let transaction = client
        .transaction()
        .await
        .map_err(|error| error.to_string())?;
    let row = transaction
        .query_one(
            "SELECT status, output FROM workflow_runs WHERE id = $1 FOR UPDATE",
            &[&snapshot.run_id],
        )
        .await
        .map_err(|error| error.to_string())?;
    if row.get::<_, String>(0) != "running" {
        return Err("workflow run was paused or terminated before child task creation".to_owned());
    }
    let agent_id = if let Some(requested) = requested_agent_id {
        if transaction
            .query_opt(
                "SELECT 1 FROM agents WHERE id = $1 AND status = 'active'",
                &[&requested],
            )
            .await
            .map_err(|error| error.to_string())?
            .is_some()
        {
            requested.to_owned()
        } else {
            snapshot.agent_id.clone()
        }
    } else {
        snapshot.agent_id.clone()
    };
    let task_id = Uuid::new_v4().to_string();
    let title = node
        .config
        .get("title")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(&node.label);
    let description = node_text(&node.config).unwrap_or_else(|| node.label.clone());
    let dedupe_key = format!("workflow:{}:node:{}:task", snapshot.run_id, node.key);
    let node_run_id = format!("{}:{}", snapshot.run_id, node.key);

    // 工作流子任务直接进入执行态且不要求方案或验收，同时保留在统一任务面板中。
    transaction
        .execute(
            concat!(
                "INSERT INTO tasks (id, project_id, parent_task_id, title, description, board_stage, plan_status, execution_status, acceptance_status, ",
                "requires_plan_confirmation, source_type, source_workflow_run_id, source_node_run_id, workflow_name) ",
                "VALUES ($1, $2, $3, $4, $5, 'in_progress', 'not_required', 'queued', 'not_started', FALSE, 'workflow_node', $6, $7, $8)"
            ),
            &[
                &task_id,
                &snapshot.project_id,
                &snapshot.parent_task_id,
                &title,
                &description,
                &snapshot.run_id,
                &node_run_id,
                &snapshot.workflow_name,
            ],
        )
        .await
        .map_err(|error| error.to_string())?;
    transaction
        .execute(
            "INSERT INTO task_agents (task_id, agent_id, participation_type, status) VALUES ($1, $2, 'owner', 'active')",
            &[&task_id, &agent_id],
        )
        .await
        .map_err(|error| error.to_string())?;
    transaction
        .execute(
            concat!(
                "INSERT INTO execution_jobs (id, kind, status, task_id, project_id, agent_id, payload, dedupe_key) ",
                "VALUES ($1, 'execute_task', 'queued', $2, $3, $4, $5, $6) ON CONFLICT (dedupe_key) DO NOTHING"
            ),
            &[
                &Uuid::new_v4().to_string(),
                &task_id,
                &snapshot.project_id,
                &agent_id,
                &json!({
                    "task_id": task_id.clone(), "workflow_run_id": snapshot.run_id.clone(), "workflow_id": snapshot.workflow_id.clone(),
                    "node_key": node.key.clone(), "node_run_id": node_run_id.clone()
                }),
                &dedupe_key,
            ],
        )
        .await
        .map_err(|error| error.to_string())?;
    transaction
        .execute(
            concat!(
                "UPDATE workflow_node_runs SET status = 'waiting_child', task_id = $2, input = $3, attempt_count = attempt_count + 1, ",
                "started_at = COALESCE(started_at, now()), updated_at = now() WHERE run_id = $1 AND node_key = $4"
            ),
            &[&snapshot.run_id, &task_id, &json!({ "title": title, "description": description }), &node.key],
        )
        .await
        .map_err(|error| error.to_string())?;
    transaction
        .execute(
            "INSERT INTO task_events (task_id, event_type, actor_type, actor_id, after_data, event_data) VALUES ($1, 'task.created', 'workflow', $2, $3, $4)",
            &[
                &task_id,
                &snapshot.run_id,
                &json!({ "board_stage": "in_progress", "execution_status": "queued" }),
                &json!({ "workflow_id": snapshot.workflow_id.clone(), "node_key": node.key.clone() }),
            ],
        )
        .await
        .map_err(|error| error.to_string())?;

    // 保存等待游标后完成本次工作流作业，子任务由普通执行链独立领取和追踪。
    let mut output = row.get::<_, Value>(1);
    let mut runtime = runtime_state(&output)?;
    runtime.waiting_node_key = Some(node.key.clone());
    runtime.waiting_task_id = Some(task_id.clone());
    write_runtime_state(&mut output, &runtime)?;
    transaction
        .execute(
            "UPDATE workflow_runs SET status = 'waiting_child', output = $2, updated_at = now() WHERE id = $1",
            &[&snapshot.run_id, &output],
        )
        .await
        .map_err(|error| error.to_string())?;
    transaction
        .execute(
            "INSERT INTO workflow_run_events (run_id, event_type, node_key, payload) VALUES ($1, 'node.waiting_child', $2, $3)",
            &[&snapshot.run_id, &node.key, &json!({ "task_id": task_id.clone(), "agent_id": agent_id })],
        )
        .await
        .map_err(|error| error.to_string())?;
    transaction
        .commit()
        .await
        .map_err(|error| error.to_string())?;
    Ok(Some((
        false,
        format!("工作流执行节点已创建子任务 {task_id}，等待任务完成。"),
    )))
}

/// 评估判断节点，优先使用结构化输入；真实模式才委托 Codex 解释自然语言规则。
async fn evaluate_condition_node(
    codex_config: &CodexConfig,
    snapshot: &WorkflowSnapshot,
    node: &WorkflowNode,
    state: &WorkflowRuntimeState,
) -> Result<bool, String> {
    if let Some(value) = snapshot
        .input
        .get("condition_results")
        .and_then(|values| values.get(&node.key))
        .and_then(boolean_value)
    {
        return Ok(value);
    }
    if let Some(value) = node
        .config
        .get("result")
        .or_else(|| node.config.get("controlled_result"))
        .and_then(boolean_value)
    {
        return Ok(value);
    }
    if !codex_config.is_real() {
        return Ok(true);
    }

    // 自然语言判断只允许输出 yes/no，节点结果和运行输入均作为只读事实传入。
    let rule = node_text(&node.config).unwrap_or_else(|| node.label.clone());
    let description = format!(
        "判断规则：{rule}\n运行输入：{}\n已完成节点输出：{}",
        snapshot.input,
        json!(state.node_outputs)
    );
    let result = codex_config
        .run(
            "evaluate_workflow_condition",
            TaskPromptContext {
                project_id: &snapshot.project_id,
                project_name: &snapshot.project_name,
                title: &format!(
                    "判断工作流《{}》节点：{}",
                    snapshot.workflow_name, node.label
                ),
                description: &description,
                agent_name: &snapshot.agent_name,
                agent_instructions: &snapshot.agent_instructions,
                memories: "判断必须仅基于本次运行输入和已完成节点输出。",
            },
        )
        .await?;
    boolean_value(&Value::String(result.content))
        .ok_or_else(|| "condition agent must return yes or no".to_owned())
}

/// 将人工确认节点置为挂起并结束当前作业，恢复必须由 Human 操作重新入队。
async fn wait_for_human(
    database_url: &str,
    runner_id: &str,
    job: &ClaimedJob,
    run_id: &str,
    node: &WorkflowNode,
) -> Result<(), String> {
    let mut client = connect(database_url).await?;
    let transaction = client
        .transaction()
        .await
        .map_err(|error| error.to_string())?;
    let row = transaction
        .query_one(
            "SELECT status, output FROM workflow_runs WHERE id = $1 FOR UPDATE",
            &[&run_id],
        )
        .await
        .map_err(|error| error.to_string())?;
    if row.get::<_, String>(0) != "running" {
        return Err("workflow run was paused or terminated before human confirmation".to_owned());
    }
    let mut output = row.get::<_, Value>(1);
    let mut state = runtime_state(&output)?;
    state.waiting_node_key = Some(node.key.clone());
    state.waiting_task_id = None;
    write_runtime_state(&mut output, &state)?;
    transaction
        .execute(
            "UPDATE workflow_runs SET status = 'waiting_approval', output = $2, updated_at = now() WHERE id = $1",
            &[&run_id, &output],
        )
        .await
        .map_err(|error| error.to_string())?;
    let node_run_id = format!("{}:{}", run_id, node.key);
    transaction
        .execute(
            concat!(
                "UPDATE workflow_node_runs SET status = 'waiting_approval', input = $3, attempt_count = attempt_count + 1, ",
                "started_at = COALESCE(started_at, now()), updated_at = now() WHERE run_id = $1 AND node_key = $2"
            ),
            &[&run_id, &node.key, &json!({ "job_id": job.job_id.clone() })],
        )
        .await
        .map_err(|error| error.to_string())?;
    transaction
        .execute(
            concat!(
                "INSERT INTO approval_requests (id, request_type, workflow_run_id, node_run_id, execution_job_id, status, prompt) ",
                "SELECT $1, 'workflow_human_confirm', $2, $3, $4, 'pending', $5 WHERE NOT EXISTS ",
                "(SELECT 1 FROM approval_requests WHERE workflow_run_id = $2 AND node_run_id = $3 AND status = 'pending')"
            ),
            &[
                &Uuid::new_v4().to_string(),
                &run_id,
                &node_run_id,
                &job.job_id,
                &node_text(&node.config).unwrap_or_else(|| node.label.clone()),
            ],
        )
        .await
        .map_err(|error| error.to_string())?;
    transaction
        .execute(
            "INSERT INTO workflow_run_events (run_id, event_type, node_key, payload) VALUES ($1, 'human_confirmation.requested', $2, $3)",
            &[&run_id, &node.key, &json!({ "label": node.label.clone(), "instruction": node_text(&node.config) })],
        )
        .await
        .map_err(|error| error.to_string())?;
    finish_general_records(
        &transaction,
        runner_id,
        job,
        "workflow_waiting_approval",
        "工作流已挂起，等待 Human 确认。",
        None,
    )
    .await?;
    transaction
        .commit()
        .await
        .map_err(|error| error.to_string())
}

/// 完成结束节点并保存整个运行的聚合输出。
async fn finish_workflow_success(
    database_url: &str,
    runner_id: &str,
    job: &ClaimedJob,
    run_id: &str,
    node: &WorkflowNode,
) -> Result<(), String> {
    let mut client = connect(database_url).await?;
    let transaction = client
        .transaction()
        .await
        .map_err(|error| error.to_string())?;
    let row = transaction
        .query_one(
            "SELECT status, output FROM workflow_runs WHERE id = $1 FOR UPDATE",
            &[&run_id],
        )
        .await
        .map_err(|error| error.to_string())?;
    let mut output = row.get::<_, Value>(1);
    let mut state = runtime_state(&output)?;
    state.completed_node_keys.push(node.key.clone());
    state
        .node_outputs
        .insert(node.key.clone(), json!({ "ended": true }));
    state.current_node_key = None;
    state.waiting_node_key = None;
    state.waiting_task_id = None;
    write_runtime_state(&mut output, &state)?;
    set_output_field(
        &mut output,
        "result",
        json!({ "status": "succeeded", "node_outputs": state.node_outputs }),
    )?;

    // 运行终态、结束事件和通用作业输出在同一事务提交。
    transaction
        .execute(
            "UPDATE workflow_runs SET status = 'succeeded', output = $2, finished_at = now(), updated_at = now() WHERE id = $1",
            &[&run_id, &output],
        )
        .await
        .map_err(|error| error.to_string())?;
    transaction
        .execute(
            concat!(
                "UPDATE workflow_node_runs SET status = 'succeeded', output = $3, attempt_count = GREATEST(attempt_count, 1), ",
                "started_at = COALESCE(started_at, now()), finished_at = now(), updated_at = now() WHERE run_id = $1 AND node_key = $2"
            ),
            &[&run_id, &node.key, &json!({ "ended": true })],
        )
        .await
        .map_err(|error| error.to_string())?;
    transaction
        .execute(
            "INSERT INTO workflow_run_events (run_id, event_type, node_key, payload) VALUES ($1, 'node.completed', $2, $3)",
            &[&run_id, &node.key, &json!({ "node_type": "end" })],
        )
        .await
        .map_err(|error| error.to_string())?;
    transaction
        .execute(
            "INSERT INTO workflow_run_events (run_id, event_type, payload) VALUES ($1, 'run.succeeded', $2)",
            &[&run_id, &json!({ "job_id": job.job_id.clone() })],
        )
        .await
        .map_err(|error| error.to_string())?;
    update_parent_progress(&transaction, run_id, true).await?;
    finish_general_records(
        &transaction,
        runner_id,
        job,
        "workflow_run",
        &output.to_string(),
        None,
    )
    .await?;
    transaction
        .commit()
        .await
        .map_err(|error| error.to_string())
}

/// 将不可恢复的画布或节点错误写入运行终态，不让确定性错误进入重复重试。
async fn fail_workflow_run(
    database_url: &str,
    runner_id: &str,
    job: &ClaimedJob,
    run_id: &str,
    node_key: &str,
    message: &str,
) -> Result<(), String> {
    let mut client = connect(database_url).await?;
    let transaction = client
        .transaction()
        .await
        .map_err(|error| error.to_string())?;
    transaction
        .execute(
            "UPDATE workflow_runs SET status = 'failed', error_message = $2, finished_at = now(), updated_at = now() WHERE id = $1",
            &[&run_id, &message],
        )
        .await
        .map_err(|error| error.to_string())?;
    let optional_node_key = (!node_key.is_empty()).then_some(node_key);
    if !node_key.is_empty() {
        transaction
            .execute(
                concat!(
                    "UPDATE workflow_node_runs SET status = 'failed', error_message = $3, attempt_count = GREATEST(attempt_count, 1), ",
                    "started_at = COALESCE(started_at, now()), finished_at = now(), updated_at = now() WHERE run_id = $1 AND node_key = $2"
                ),
                &[&run_id, &node_key, &message],
            )
            .await
            .map_err(|error| error.to_string())?;
    }
    transaction
        .execute(
            "INSERT INTO workflow_run_events (run_id, event_type, node_key, payload) VALUES ($1, 'run.failed', $2, $3)",
            &[&run_id, &optional_node_key, &json!({ "message": message })],
        )
        .await
        .map_err(|error| error.to_string())?;
    mark_parent_failed(&transaction, run_id, message).await?;
    finish_general_records(
        &transaction,
        runner_id,
        job,
        "workflow_run_error",
        message,
        None,
    )
    .await?;
    transaction
        .commit()
        .await
        .map_err(|error| error.to_string())
}

/// 完成仍在等待或已被外部改变状态的工作流作业切片。
async fn finish_workflow_slice(
    database_url: &str,
    runner_id: &str,
    job: &ClaimedJob,
    content: &str,
    output_type: &str,
) -> Result<(), String> {
    let mut client = connect(database_url).await?;
    let transaction = client
        .transaction()
        .await
        .map_err(|error| error.to_string())?;
    finish_general_records(&transaction, runner_id, job, output_type, content, None).await?;
    transaction
        .commit()
        .await
        .map_err(|error| error.to_string())
}

/// 读取线性节点的唯一出口，开始、执行节点不允许隐式选择多条连接线。
fn linear_target(snapshot: &WorkflowSnapshot, node_key: &str) -> Result<String, String> {
    let edges = snapshot
        .edges
        .iter()
        .filter(|edge| edge.source == node_key)
        .collect::<Vec<_>>();
    if edges.len() != 1 {
        return Err(format!(
            "workflow node {node_key} requires exactly one outgoing edge"
        ));
    }
    Ok(edges[0].target.clone())
}

/// 按连线的明确是/否标签选择判断结果，不执行任意表达式或隐藏规则。
fn decision_target(
    snapshot: &WorkflowSnapshot,
    node_key: &str,
    decision: bool,
) -> Result<String, String> {
    let edges = snapshot
        .edges
        .iter()
        .filter(|edge| edge.source == node_key)
        .collect::<Vec<_>>();
    for edge in edges {
        if boolean_value(&Value::String(edge.label.clone())) == Some(decision)
            || edge.condition.get("value").and_then(boolean_value) == Some(decision)
        {
            return Ok(edge.target.clone());
        }
    }
    Err(format!(
        "workflow decision node {node_key} is missing a labeled {} edge",
        if decision { "yes" } else { "no" }
    ))
}

/// 人工确认默认沿唯一出口继续；存在显式是/否分支时才按确认结果选择。
fn human_confirmation_target(
    snapshot: &WorkflowSnapshot,
    node_key: &str,
    decision: bool,
) -> Result<String, String> {
    let edges = snapshot
        .edges
        .iter()
        .filter(|edge| edge.source == node_key)
        .collect::<Vec<_>>();
    if edges.len() == 1 && decision {
        return Ok(edges[0].target.clone());
    }
    decision_target(snapshot, node_key, decision)
}

/// 从作业、运行输入、运行输出或审批请求读取 Human 已确认的分支结果。
async fn resolved_human_decision(
    database_url: &str,
    job: &ClaimedJob,
    input: &Value,
    output: &Value,
    run_id: &str,
    node_key: &str,
) -> Result<Option<bool>, String> {
    let direct = job
        .payload
        .get("decision")
        .or_else(|| job.payload.get("human_decision"))
        .and_then(boolean_value)
        .or_else(|| {
            input
                .get("human_confirmations")
                .and_then(|values| values.get(node_key))
                .and_then(boolean_value)
        })
        .or_else(|| {
            output
                .get("human_confirmations")
                .and_then(|values| values.get(node_key))
                .and_then(boolean_value)
        });
    if direct.is_some() {
        return Ok(direct);
    }
    let client = connect(database_url).await?;
    let node_run_id = format!("{}:{}", run_id, node_key);
    let Some(row) = client
        .query_opt(
            "SELECT status, response_data FROM approval_requests WHERE workflow_run_id = $1 AND node_run_id = $2 ORDER BY requested_at DESC LIMIT 1",
            &[&run_id, &node_run_id],
        )
        .await
        .map_err(|error| error.to_string())?
    else {
        return Ok(None);
    };
    let status = row.get::<_, String>(0);
    if status == "pending" {
        return Ok(None);
    }
    let response = row.get::<_, Value>(1);
    Ok(response
        .get("decision")
        .and_then(boolean_value)
        .or_else(|| response.get("approved").and_then(boolean_value))
        .or_else(|| boolean_value(&Value::String(status))))
}

/// 将常见中英文确认词转换成稳定布尔值。
fn boolean_value(value: &Value) -> Option<bool> {
    if let Some(value) = value.as_bool() {
        return Some(value);
    }
    let normalized = value.as_str()?.trim().to_ascii_lowercase();
    match normalized.as_str() {
        "yes" | "true" | "1" | "是" | "通过" | "同意" | "确认" => Some(true),
        "no" | "false" | "0" | "否" | "拒绝" | "不通过" | "不同意" => Some(false),
        _ => None,
    }
}

/// 从节点配置中读取用户编辑的主要执行文字。
fn node_text(config: &Value) -> Option<String> {
    ["instruction", "description", "content", "prompt", "text"]
        .iter()
        .find_map(|key| {
            config
                .get(key)
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned)
        })
}

/// 从 output.runtime 反序列化游标；历史运行没有 runtime 时返回空状态。
fn runtime_state(output: &Value) -> Result<WorkflowRuntimeState, String> {
    match output.get("runtime") {
        Some(value) => serde_json::from_value(value.clone())
            .map_err(|error| format!("invalid workflow runtime state: {error}")),
        None => Ok(WorkflowRuntimeState::default()),
    }
}

/// 在保留 output 其他业务字段的前提下更新 runtime 游标。
fn write_runtime_state(output: &mut Value, state: &WorkflowRuntimeState) -> Result<(), String> {
    let object = output
        .as_object_mut()
        .ok_or_else(|| "workflow output must be a JSON object".to_owned())?;
    object.insert(
        "runtime".to_owned(),
        serde_json::to_value(state).map_err(|error| error.to_string())?,
    );
    Ok(())
}

/// 向运行输出写入聚合结果，同时保留 Human 确认等控制面字段。
fn set_output_field(output: &mut Value, key: &str, value: Value) -> Result<(), String> {
    let object = output
        .as_object_mut()
        .ok_or_else(|| "workflow output must be a JSON object".to_owned())?;
    object.insert(key.to_owned(), value);
    Ok(())
}

/// 按已完成节点数回写工作流父任务进度，终态成功时将父任务直接完成。
async fn update_parent_progress(
    transaction: &Transaction<'_>,
    run_id: &str,
    succeeded: bool,
) -> Result<(), String> {
    let row = transaction
        .query_opt(
            concat!(
                "SELECT parent_task_id, (SELECT count(*) FROM workflow_node_runs WHERE run_id = $1), ",
                "(SELECT count(*) FROM workflow_node_runs WHERE run_id = $1 AND status = 'succeeded') FROM workflow_runs WHERE id = $1"
            ),
            &[&run_id],
        )
        .await
        .map_err(|error| error.to_string())?;
    let Some(row) = row else { return Ok(()); };
    let parent_task_id = row.get::<_, Option<String>>(0);
    let Some(parent_task_id) = parent_task_id else { return Ok(()); };
    let total = row.get::<_, i64>(1).max(1);
    let completed = row.get::<_, i64>(2);
    let progress = if succeeded {
        100
    } else {
        ((completed * 100) / total).min(99) as i16
    };
    let stage = if succeeded { "done" } else { "in_progress" };
    let execution_status = if succeeded { "succeeded" } else { "running" };
    transaction
        .execute(
            concat!(
                "UPDATE tasks SET board_stage = $2, execution_status = $3, acceptance_status = CASE WHEN $2 = 'done' THEN 'passed' ",
                "ELSE acceptance_status END, progress_percent = $4, revision = revision + 1, updated_at = now() WHERE id = $1"
            ),
            &[&parent_task_id, &stage, &execution_status, &progress],
        )
        .await
        .map_err(|error| error.to_string())?;
    Ok(())
}

/// 工作流失败时保留父任务和已完成进度，同时把失败状态暴露到统一任务面板。
async fn mark_parent_failed(
    transaction: &Transaction<'_>,
    run_id: &str,
    message: &str,
) -> Result<(), String> {
    let row = transaction
        .query_opt(
            concat!(
                "SELECT parent_task_id, (SELECT count(*) FROM workflow_node_runs WHERE run_id = $1), ",
                "(SELECT count(*) FROM workflow_node_runs WHERE run_id = $1 AND status = 'succeeded') FROM workflow_runs WHERE id = $1"
            ),
            &[&run_id],
        )
        .await
        .map_err(|error| error.to_string())?;
    let Some(row) = row else { return Ok(()); };
    let Some(parent_task_id) = row.get::<_, Option<String>>(0) else { return Ok(()); };
    let total = row.get::<_, i64>(1).max(1);
    let completed = row.get::<_, i64>(2);
    let progress = ((completed * 100) / total).min(99) as i16;
    transaction
        .execute(
            "UPDATE tasks SET execution_status = 'failed', progress_percent = $2, revision = revision + 1, updated_at = now() WHERE id = $1",
            &[&parent_task_id, &progress],
        )
        .await
        .map_err(|error| error.to_string())?;
    transaction
        .execute(
            "INSERT INTO task_events (task_id, event_type, actor_type, actor_id, event_data) VALUES ($1, 'workflow.run_failed', 'runner', 'workflow-runner', $2)",
            &[&parent_task_id, &json!({ "workflow_run_id": run_id, "message": message })],
        )
        .await
        .map_err(|error| error.to_string())?;
    Ok(())
}
