use std::env;
use tokio_postgres::NoTls;

/// 迁移入口：按版本幂等创建运行时、项目、任务、评论和执行追踪表。
#[tokio::main]
async fn main() {
    // 读取数据库连接配置并建立迁移连接，失败时阻止后续服务启动。
    let database_url = env::var("DATABASE_URL").expect("DATABASE_URL must be configured");
    let (client, connection) = tokio_postgres::connect(&database_url, NoTls).await.expect("connect database");
    tokio::spawn(async move {
        if let Err(error) = connection.await {
            eprintln!("migration connection ended: {error}");
        }
    });
    // 以幂等 SQL 创建 M0/M1/M2 表，重复运行只补齐缺失结构，不删除已有业务数据。
    client
        .batch_execute(
            "CREATE TABLE IF NOT EXISTS schema_migrations (version TEXT PRIMARY KEY, applied_at TIMESTAMPTZ NOT NULL DEFAULT now());
             CREATE TABLE IF NOT EXISTS runner_instances (
               id TEXT PRIMARY KEY,
               name TEXT NOT NULL,
               status TEXT NOT NULL,
               last_heartbeat_at TIMESTAMPTZ NOT NULL,
               lease_expires_at TIMESTAMPTZ NOT NULL
             );
             INSERT INTO schema_migrations (version) VALUES ('0001_m0_runtime') ON CONFLICT (version) DO NOTHING;
             CREATE TABLE IF NOT EXISTS projects (
               id TEXT PRIMARY KEY,
               name TEXT NOT NULL,
               description TEXT NOT NULL DEFAULT '',
               status TEXT NOT NULL DEFAULT 'active',
               created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
               updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
             );
             ALTER TABLE projects ADD COLUMN IF NOT EXISTS status TEXT NOT NULL DEFAULT 'active';
             CREATE TABLE IF NOT EXISTS tasks (
               id TEXT PRIMARY KEY,
               project_id TEXT NOT NULL REFERENCES projects(id),
               parent_task_id TEXT REFERENCES tasks(id),
               title TEXT NOT NULL,
               description TEXT NOT NULL DEFAULT '',
               board_stage TEXT NOT NULL,
               plan_status TEXT NOT NULL,
               execution_status TEXT NOT NULL,
               acceptance_status TEXT NOT NULL DEFAULT 'not_started',
               progress_percent SMALLINT NOT NULL DEFAULT 0,
               revision BIGINT NOT NULL DEFAULT 0,
               requires_plan_confirmation BOOLEAN NOT NULL DEFAULT TRUE,
               created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
               updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
             );
             CREATE INDEX IF NOT EXISTS tasks_project_stage_idx ON tasks(project_id, board_stage);
             CREATE INDEX IF NOT EXISTS tasks_parent_idx ON tasks(parent_task_id);
             CREATE TABLE IF NOT EXISTS task_comments (
               id TEXT PRIMARY KEY,
               task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
               author_type TEXT NOT NULL,
               author_name TEXT NOT NULL,
               content TEXT NOT NULL,
               intent TEXT NOT NULL DEFAULT 'note',
               transition_applied BOOLEAN NOT NULL DEFAULT FALSE,
               created_at TIMESTAMPTZ NOT NULL DEFAULT now()
             );
             CREATE INDEX IF NOT EXISTS task_comments_task_idx ON task_comments(task_id, created_at);
             CREATE TABLE IF NOT EXISTS task_relations (
               id TEXT PRIMARY KEY,
               project_id TEXT NOT NULL REFERENCES projects(id),
               from_task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
               to_task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
               relation_type TEXT NOT NULL,
               created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
               UNIQUE (from_task_id, to_task_id, relation_type)
             );
             CREATE TABLE IF NOT EXISTS task_events (
               id BIGSERIAL PRIMARY KEY,
               task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
               event_type TEXT NOT NULL,
               actor_type TEXT NOT NULL,
               actor_id TEXT NOT NULL,
               before_data JSONB,
               after_data JSONB,
               event_data JSONB NOT NULL DEFAULT '{}'::jsonb,
               created_at TIMESTAMPTZ NOT NULL DEFAULT now()
             );
             CREATE INDEX IF NOT EXISTS task_events_task_idx ON task_events(task_id, created_at);
             CREATE TABLE IF NOT EXISTS task_transitions (
               id TEXT PRIMARY KEY,
               task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
               from_stage TEXT NOT NULL,
               to_stage TEXT NOT NULL,
               reason TEXT NOT NULL DEFAULT '',
               created_at TIMESTAMPTZ NOT NULL DEFAULT now()
             );
             CREATE TABLE IF NOT EXISTS project_documents (
               id TEXT PRIMARY KEY,
               project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
               doc_type TEXT NOT NULL,
               title TEXT NOT NULL,
               revision BIGINT NOT NULL DEFAULT 0,
               created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
               updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
               UNIQUE (project_id, doc_type)
             );
             CREATE TABLE IF NOT EXISTS project_document_versions (
               id TEXT PRIMARY KEY,
               document_id TEXT NOT NULL REFERENCES project_documents(id) ON DELETE CASCADE,
               version_no INTEGER NOT NULL,
               content TEXT NOT NULL,
               content_hash TEXT NOT NULL,
               source_type TEXT NOT NULL,
               created_by_actor_id TEXT NOT NULL,
               created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
               UNIQUE (document_id, version_no)
             );
             INSERT INTO schema_migrations (version) VALUES ('0002_m1_task_domain') ON CONFLICT (version) DO NOTHING;
             CREATE TABLE IF NOT EXISTS execution_jobs (
               id TEXT PRIMARY KEY,
               kind TEXT NOT NULL,
               status TEXT NOT NULL,
               task_id TEXT REFERENCES tasks(id) ON DELETE CASCADE,
               payload JSONB NOT NULL DEFAULT '{}'::jsonb,
               dedupe_key TEXT UNIQUE,
               attempt_count INTEGER NOT NULL DEFAULT 0,
               max_attempts INTEGER NOT NULL DEFAULT 3,
               available_at TIMESTAMPTZ NOT NULL DEFAULT now(),
               created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
               updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
             );
             CREATE INDEX IF NOT EXISTS execution_jobs_queue_idx ON execution_jobs(status, available_at, created_at);
             CREATE TABLE IF NOT EXISTS execution_attempts (
               id TEXT PRIMARY KEY,
               job_id TEXT NOT NULL REFERENCES execution_jobs(id) ON DELETE CASCADE,
               runner_instance_id TEXT NOT NULL REFERENCES runner_instances(id),
               status TEXT NOT NULL,
               lease_expires_at TIMESTAMPTZ NOT NULL,
               started_at TIMESTAMPTZ NOT NULL DEFAULT now(),
               heartbeat_at TIMESTAMPTZ NOT NULL DEFAULT now(),
               finished_at TIMESTAMPTZ,
               failure_message TEXT
             );
             ALTER TABLE execution_jobs ALTER COLUMN max_attempts SET DEFAULT 3;
             ALTER TABLE execution_attempts ADD COLUMN IF NOT EXISTS codex_thread_id TEXT;
             CREATE INDEX IF NOT EXISTS execution_attempts_job_idx ON execution_attempts(job_id, started_at);
             CREATE TABLE IF NOT EXISTS execution_events (
               id BIGSERIAL PRIMARY KEY,
               job_id TEXT NOT NULL REFERENCES execution_jobs(id) ON DELETE CASCADE,
               attempt_id TEXT REFERENCES execution_attempts(id) ON DELETE CASCADE,
               task_id TEXT REFERENCES tasks(id) ON DELETE CASCADE,
               event_type TEXT NOT NULL,
               payload JSONB NOT NULL DEFAULT '{}'::jsonb,
               created_at TIMESTAMPTZ NOT NULL DEFAULT now()
             );
             CREATE INDEX IF NOT EXISTS execution_events_job_idx ON execution_events(job_id, created_at);
             CREATE INDEX IF NOT EXISTS execution_events_task_idx ON execution_events(task_id, created_at);
             CREATE TABLE IF NOT EXISTS run_outputs (
               id TEXT PRIMARY KEY,
               job_id TEXT NOT NULL REFERENCES execution_jobs(id) ON DELETE CASCADE,
               task_id TEXT REFERENCES tasks(id) ON DELETE CASCADE,
               output_type TEXT NOT NULL,
               content TEXT NOT NULL,
               created_at TIMESTAMPTZ NOT NULL DEFAULT now()
             );
             CREATE INDEX IF NOT EXISTS run_outputs_task_idx ON run_outputs(task_id, created_at);
             INSERT INTO schema_migrations (version) VALUES ('0003_m2_execution_control') ON CONFLICT (version) DO NOTHING;
             INSERT INTO schema_migrations (version) VALUES ('0004_m2_codex_runtime') ON CONFLICT (version) DO NOTHING;",
        )
        .await
        .expect("apply migration");
    println!("xiexu migration complete");
}
