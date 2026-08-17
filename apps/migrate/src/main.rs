use std::env;
use tokio_postgres::NoTls;

/// 迁移入口：按版本幂等创建运行时、项目、任务、评论和执行追踪表。
#[tokio::main]
async fn main() {
    // 读取数据库连接配置并建立迁移连接，失败时阻止后续服务启动。
    let database_url = env::var("DATABASE_URL").expect("DATABASE_URL must be configured");
    let (client, connection) = tokio_postgres::connect(&database_url, NoTls)
        .await
        .expect("connect database");
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
             INSERT INTO schema_migrations (version) VALUES ('0004_m2_codex_runtime') ON CONFLICT (version) DO NOTHING;
             CREATE TABLE IF NOT EXISTS agent_role_templates (
               code TEXT PRIMARY KEY,
               name TEXT NOT NULL,
               category TEXT NOT NULL,
               description TEXT NOT NULL,
               default_instructions TEXT NOT NULL,
               builtin BOOLEAN NOT NULL DEFAULT TRUE,
               active BOOLEAN NOT NULL DEFAULT TRUE,
               created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
               updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
             );
             CREATE TABLE IF NOT EXISTS agents (
               id TEXT PRIMARY KEY,
               template_code TEXT REFERENCES agent_role_templates(code),
               name TEXT NOT NULL,
               description TEXT NOT NULL DEFAULT '',
               instructions TEXT NOT NULL DEFAULT '',
               responsibility_supplement TEXT NOT NULL DEFAULT '',
               status TEXT NOT NULL DEFAULT 'active',
               created_by TEXT NOT NULL DEFAULT 'human',
               created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
               updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
             );
             CREATE TABLE IF NOT EXISTS project_agents (
               project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
               agent_id TEXT NOT NULL REFERENCES agents(id),
               assignment_type TEXT NOT NULL,
               responsibility_override TEXT NOT NULL DEFAULT '',
               status TEXT NOT NULL DEFAULT 'active',
               assigned_at TIMESTAMPTZ NOT NULL DEFAULT now(),
               PRIMARY KEY (project_id, agent_id)
             );
             CREATE UNIQUE INDEX IF NOT EXISTS project_single_coordinator_idx ON project_agents(project_id) WHERE assignment_type = 'coordinator' AND status = 'active';
             CREATE TABLE IF NOT EXISTS task_agents (
               task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
               agent_id TEXT NOT NULL REFERENCES agents(id),
               participation_type TEXT NOT NULL,
               status TEXT NOT NULL DEFAULT 'active',
               joined_at TIMESTAMPTZ NOT NULL DEFAULT now(),
               left_at TIMESTAMPTZ,
               PRIMARY KEY (task_id, agent_id)
             );
             CREATE UNIQUE INDEX IF NOT EXISTS task_single_owner_idx ON task_agents(task_id) WHERE participation_type = 'owner' AND status = 'active';
             CREATE TABLE IF NOT EXISTS agent_memories (
               id TEXT PRIMARY KEY,
               agent_id TEXT NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
               tier TEXT NOT NULL,
               project_id TEXT REFERENCES projects(id) ON DELETE CASCADE,
               task_id TEXT REFERENCES tasks(id) ON DELETE CASCADE,
               job_id TEXT REFERENCES execution_jobs(id) ON DELETE SET NULL,
               content TEXT NOT NULL,
               source_type TEXT NOT NULL DEFAULT 'human',
               source_id TEXT,
               status TEXT NOT NULL DEFAULT 'active',
               created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
               updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
             );
             CREATE INDEX IF NOT EXISTS agent_memories_context_idx ON agent_memories(agent_id, project_id, task_id, status, updated_at DESC);
             CREATE TABLE IF NOT EXISTS conversations (
               id TEXT PRIMARY KEY,
               conversation_type TEXT NOT NULL,
               project_id TEXT REFERENCES projects(id) ON DELETE CASCADE,
               title TEXT NOT NULL,
               status TEXT NOT NULL DEFAULT 'active',
               created_by TEXT NOT NULL DEFAULT 'human',
               created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
               updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
               archived_at TIMESTAMPTZ
             );
             CREATE UNIQUE INDEX IF NOT EXISTS project_main_conversation_idx ON conversations(project_id) WHERE conversation_type = 'project_main';
             CREATE TABLE IF NOT EXISTS conversation_participants (
               conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
               actor_type TEXT NOT NULL,
               actor_id TEXT NOT NULL,
               joined_at TIMESTAMPTZ NOT NULL DEFAULT now(),
               left_at TIMESTAMPTZ,
               PRIMARY KEY (conversation_id, actor_type, actor_id)
             );
             CREATE TABLE IF NOT EXISTS conversation_messages (
               id TEXT PRIMARY KEY,
               conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
               author_type TEXT NOT NULL,
               author_id TEXT NOT NULL,
               content TEXT NOT NULL,
               message_type TEXT NOT NULL DEFAULT 'text',
               task_id TEXT REFERENCES tasks(id) ON DELETE SET NULL,
               created_at TIMESTAMPTZ NOT NULL DEFAULT now()
             );
             CREATE INDEX IF NOT EXISTS conversation_messages_order_idx ON conversation_messages(conversation_id, created_at);
             CREATE TABLE IF NOT EXISTS conversation_task_links (
               conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
               task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
               linked_at TIMESTAMPTZ NOT NULL DEFAULT now(),
               PRIMARY KEY (conversation_id, task_id)
             );
             ALTER TABLE execution_jobs ADD COLUMN IF NOT EXISTS project_id TEXT REFERENCES projects(id) ON DELETE CASCADE;
             ALTER TABLE execution_jobs ADD COLUMN IF NOT EXISTS agent_id TEXT REFERENCES agents(id) ON DELETE SET NULL;
             ALTER TABLE execution_jobs ADD COLUMN IF NOT EXISTS conversation_id TEXT REFERENCES conversations(id) ON DELETE CASCADE;
             ALTER TABLE execution_events ADD COLUMN IF NOT EXISTS project_id TEXT REFERENCES projects(id) ON DELETE CASCADE;
             ALTER TABLE execution_events ADD COLUMN IF NOT EXISTS agent_id TEXT REFERENCES agents(id) ON DELETE SET NULL;
             ALTER TABLE execution_events ADD COLUMN IF NOT EXISTS conversation_id TEXT REFERENCES conversations(id) ON DELETE CASCADE;
             ALTER TABLE run_outputs ADD COLUMN IF NOT EXISTS project_id TEXT REFERENCES projects(id) ON DELETE CASCADE;
             ALTER TABLE run_outputs ADD COLUMN IF NOT EXISTS agent_id TEXT REFERENCES agents(id) ON DELETE SET NULL;
             ALTER TABLE run_outputs ADD COLUMN IF NOT EXISTS conversation_id TEXT REFERENCES conversations(id) ON DELETE CASCADE;
             INSERT INTO agent_role_templates (code, name, category, description, default_instructions) VALUES
               ('project_manager', '项目经理', 'management', '把目标转化为可交付任务，协调参与者、依赖、风险和验收节奏。', '先澄清目标和完成标准，再拆分任务、选择合适 Agent、跟踪阻塞并向 Human 汇报结果。'),
               ('product_manager', '产品经理', 'management', '理解用户问题并维护范围、优先级、验收标准与价值判断。', '围绕用户结果工作，区分事实与假设，给出清晰范围、优先级和可验证验收标准。'),
               ('technical_manager', '技术经理', 'management', '组织工程方案和研发协作，确保技术决策能够稳定交付。', '明确技术边界与责任分工，控制风险、质量和交付节奏，必要时协调专业 Agent。'),
               ('solution_architect', '解决方案架构师', 'architecture_quality', '设计系统边界、关键数据流、接口契约与非功能约束。', '从业务约束出发设计可演进架构，明确权衡、失败路径、兼容性与验证方式。'),
               ('security_engineer', '安全工程师', 'architecture_quality', '识别威胁和敏感边界，设计安全控制与验证门禁。', '按资产、威胁、控制和证据开展工作，不泄露凭据，对高风险操作设置明确门禁。'),
               ('qa_engineer', '测试与质量工程师', 'architecture_quality', '依据风险设计测试并提供可复核的质量结论。', '优先覆盖关键业务路径、异常边界和回归风险，记录环境、步骤、证据与剩余风险。'),
               ('software_engineer', '软件工程师', 'engineering', '完成通用软件实现、测试和交付。', '先阅读现有实现，遵循项目约束，以最小完整改动交付并验证真实行为。'),
               ('fullstack_engineer', '全栈工程师', 'engineering', '贯通 Web 界面、服务、数据与部署完成端到端功能。', '以用户流程为单位打通前后端和数据契约，验证真实接口与主要响应式场景。'),
               ('frontend_engineer', '前端工程师', 'engineering', '实现清晰、高效、可访问的 Web 交互并接入真实数据。', '遵循现有设计系统，处理加载、空态、错误和响应式边界，不伪造后端事实。'),
               ('backend_engineer', '后端工程师', 'engineering', '实现业务服务、API、持久化、事务和外部集成。', '保持业务规则可见，明确事务和幂等边界，避免 N+1，并验证异常与并发路径。'),
               ('mobile_engineer', '移动端工程师', 'engineering', '实现移动设备上的客户端体验与平台集成。', '关注触控、弱网、生命周期、设备能力和平台差异，并提供可复现验证结果。'),
               ('desktop_engineer', '桌面端工程师', 'engineering', '实现桌面客户端、安装更新与本地系统集成。', '明确操作系统差异、文件权限、升级和回滚边界，避免破坏用户本地数据。'),
               ('game_engineer', '游戏工程师', 'engineering', '实现玩法、引擎模块、工具链、性能与平台发布。', '保持帧循环和状态变化可验证，关注性能预算、确定性、资源生命周期和平台限制。'),
               ('embedded_iot_engineer', '嵌入式与 IoT 工程师', 'engineering', '实现固件、设备协议、边缘控制、OTA 与故障恢复。', '优先保证安全状态、协议兼容和可恢复升级，对硬件约束与断连场景进行验证。'),
               ('database_engineer', '数据库工程师', 'engineering', '负责数据模型、查询、索引、迁移、备份与容量。', '保护数据完整性和可回滚性，评估锁、并发、索引成本与迁移兼容。'),
               ('devops_engineer', '平台 / DevOps / SRE 工程师', 'engineering', '维护构建发布、基础设施、可观测性与故障恢复。', '以可重复部署和可观测运行状态为目标，明确 SLO、回滚步骤和环境差异。'),
               ('data_engineer', '数据工程师', 'data_ai', '构建数据契约、模型、管道、血缘与质量控制。', '保持数据口径、时序和幂等边界清晰，验证回填、延迟、重复与坏数据处理。'),
               ('data_analyst', '数据分析师', 'data_ai', '定义指标并分析数据，为决策提供量化证据。', '说明数据来源、口径、样本限制和不确定性，区分相关性、因果与推断。'),
               ('machine_learning_engineer', '机器学习工程师', 'data_ai', '实现训练评估、推理服务、监控和模型生命周期。', '建立可复现实验和离线在线一致性，关注漂移、成本、延迟与失败回退。'),
               ('research_specialist', '研究员', 'data_ai', '围绕问题收集、核验和综合证据，形成可执行结论。', '先制定检索与证据计划，标注事实、推断和假设，优先使用权威一手来源。'),
               ('product_designer', '产品设计师', 'design_content', '连接用户问题、信息架构、交互、视觉与实现验收。', '从高频任务和用户心智出发设计完整状态，确保设计可实现、可访问并可验收。'),
               ('ui_designer', 'UI / 视觉设计师', 'design_content', '定义视觉层级、组件、状态、动效与交付规范。', '遵循品牌和现有组件规则，覆盖响应式与交互状态，并提供明确实现约束。'),
               ('ux_designer', 'UX / 交互设计师', 'design_content', '研究并优化用户旅程、信息架构、任务流和可用性。', '以真实任务为中心识别摩擦，验证关键流程并兼顾无障碍和错误恢复。'),
               ('game_designer', '游戏策划 / 系统设计师', 'design_content', '设计玩法循环、成长、经济、关卡和可玩性验证。', '将规则、反馈和数值关系结构化，明确目标体验、边界条件和测试方法。'),
               ('technical_writer', '技术写作与文档工程师', 'design_content', '创建面向任务的产品文档、API 参考和运行手册。', '以读者目标组织内容，核对真实行为、版本和示例，避免复制过时实现。'),
               ('business_analyst', '业务分析师', 'business_delivery', '建模业务流程、规则、例外和可验证需求。', '区分现状与目标，明确角色、数据、规则、异常和业务验收证据。'),
               ('implementation_consultant', '实施顾问', 'business_delivery', '负责差距分析、配置、迁移、UAT、培训与交接。', '按环境和客户事实制定实施步骤，保护数据，管理切换风险并留下可运营交付物。'),
               ('erp_consultant', 'ERP 业务顾问', 'business_delivery', '设计财务供应链流程、主数据、单据与业务对账。', '维护业务口径和审计链，覆盖期初、例外、权限边界及端到端对账。'),
               ('wms_consultant', 'WMS 仓储顾问', 'business_delivery', '设计仓储作业、库存约束、设备流程与集成对账。', '以库存准确和作业连续为核心，覆盖并发、断网、设备异常与差异处理。'),
               ('domain_expert', '领域专家', 'business_delivery', '验证领域术语、事实、规则、例外和风险控制。', '指出不符合领域事实的假设，补齐关键例外，并给出可验证业务依据。'),
               ('operations_specialist', '运营专员', 'business_delivery', '执行持续运营、维护业务数据、监控指标并处理异常。', '按规则稳定执行，记录异常、影响和处置结果，并持续提出可验证改进。'),
               ('growth_marketing_specialist', '增长与市场专员', 'business_delivery', '设计受众、信息、渠道、活动、实验和归因。', '以合规和增量效果为边界设计实验，明确目标人群、成本、归因与停止条件。'),
               ('general_member', '通用成员', 'general', '处理明确分配的工作并同步进度、阻塞和结果。', '严格按任务目标和边界执行，及时说明阻塞、失败和完成证据。')
             ON CONFLICT (code) DO UPDATE SET name = EXCLUDED.name, category = EXCLUDED.category, description = EXCLUDED.description,
               default_instructions = EXCLUDED.default_instructions, active = TRUE, updated_at = now();
             INSERT INTO agents (id, template_code, name, description, instructions, created_by)
               SELECT 'project-coordinator-' || p.id, 'project_manager', p.name || ' 协调 Agent', '负责该项目的任务拆分、指派、依赖协调和结果汇总。',
                 '以项目目标为边界协调固定与动态 Agent；Human 负责确认关键方案和验收最终结果。', 'system'
               FROM projects p
               WHERE NOT EXISTS (SELECT 1 FROM project_agents pa WHERE pa.project_id = p.id AND pa.assignment_type = 'coordinator' AND pa.status = 'active')
             ON CONFLICT (id) DO NOTHING;
             INSERT INTO project_agents (project_id, agent_id, assignment_type)
               SELECT p.id, 'project-coordinator-' || p.id, 'coordinator' FROM projects p
               WHERE NOT EXISTS (SELECT 1 FROM project_agents pa WHERE pa.project_id = p.id AND pa.assignment_type = 'coordinator' AND pa.status = 'active')
             ON CONFLICT (project_id, agent_id) DO NOTHING;
             INSERT INTO conversations (id, conversation_type, project_id, title, created_by)
               SELECT 'project-main-' || p.id, 'project_main', p.id, p.name || ' 项目群', 'system' FROM projects p
               WHERE NOT EXISTS (SELECT 1 FROM conversations c WHERE c.project_id = p.id AND c.conversation_type = 'project_main')
             ON CONFLICT (id) DO NOTHING;
             INSERT INTO conversation_participants (conversation_id, actor_type, actor_id)
               SELECT c.id, 'human', 'human' FROM conversations c WHERE c.conversation_type = 'project_main' ON CONFLICT DO NOTHING;
             INSERT INTO conversation_participants (conversation_id, actor_type, actor_id)
               SELECT c.id, 'agent', pa.agent_id FROM project_agents pa JOIN conversations c ON c.project_id = pa.project_id AND c.conversation_type = 'project_main'
               WHERE pa.assignment_type = 'coordinator' AND pa.status = 'active' ON CONFLICT DO NOTHING;
             INSERT INTO schema_migrations (version) VALUES ('0005_m3_agent_collaboration') ON CONFLICT (version) DO NOTHING;
             ALTER TABLE projects ADD COLUMN IF NOT EXISTS document_refresh_requested_at TIMESTAMPTZ;
             ALTER TABLE tasks ADD COLUMN IF NOT EXISTS collaboration_status TEXT NOT NULL DEFAULT 'ready';
             ALTER TABLE task_comments ADD COLUMN IF NOT EXISTS parent_comment_id TEXT REFERENCES task_comments(id) ON DELETE SET NULL;
             ALTER TABLE task_relations ADD COLUMN IF NOT EXISTS status TEXT NOT NULL DEFAULT 'active';
             ALTER TABLE task_relations ADD COLUMN IF NOT EXISTS source_comment_id TEXT REFERENCES task_comments(id) ON DELETE SET NULL;
             ALTER TABLE task_relations ADD COLUMN IF NOT EXISTS resolved_comment_id TEXT REFERENCES task_comments(id) ON DELETE SET NULL;
             ALTER TABLE task_relations ADD COLUMN IF NOT EXISTS resolved_at TIMESTAMPTZ;
             ALTER TABLE project_documents ADD COLUMN IF NOT EXISTS current_version_no INTEGER NOT NULL DEFAULT 0;
             ALTER TABLE project_documents ADD COLUMN IF NOT EXISTS status TEXT NOT NULL DEFAULT 'active';
             ALTER TABLE project_documents ADD COLUMN IF NOT EXISTS last_refreshed_at TIMESTAMPTZ;
             ALTER TABLE project_document_versions ADD COLUMN IF NOT EXISTS source_task_id TEXT REFERENCES tasks(id) ON DELETE SET NULL;
             ALTER TABLE project_document_versions ADD COLUMN IF NOT EXISTS rollback_from_version_no INTEGER;
             ALTER TABLE project_document_versions ADD COLUMN IF NOT EXISTS metadata JSONB NOT NULL DEFAULT '{}'::jsonb;
             CREATE TABLE IF NOT EXISTS project_document_sections (
               document_id TEXT NOT NULL REFERENCES project_documents(id) ON DELETE CASCADE,
               section_key TEXT NOT NULL,
               title TEXT NOT NULL,
               content TEXT NOT NULL DEFAULT '',
               sort_order INTEGER NOT NULL DEFAULT 0,
               locked_by_human BOOLEAN NOT NULL DEFAULT FALSE,
               revision BIGINT NOT NULL DEFAULT 0,
               created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
               updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
               PRIMARY KEY (document_id, section_key)
             );
             CREATE TABLE IF NOT EXISTS project_document_update_candidates (
               id TEXT PRIMARY KEY,
               document_id TEXT NOT NULL REFERENCES project_documents(id) ON DELETE CASCADE,
               section_key TEXT NOT NULL,
               proposed_content TEXT NOT NULL,
               source_type TEXT NOT NULL,
               source_id TEXT,
               base_section_revision BIGINT NOT NULL,
               status TEXT NOT NULL DEFAULT 'pending',
               conflict_reason TEXT,
               created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
               resolved_at TIMESTAMPTZ
             );
             CREATE INDEX IF NOT EXISTS project_document_candidates_idx ON project_document_update_candidates(document_id, status, created_at);
             CREATE TABLE IF NOT EXISTS task_mentions (
               id TEXT PRIMARY KEY,
               comment_id TEXT NOT NULL REFERENCES task_comments(id) ON DELETE CASCADE,
               source_task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
               target_type TEXT NOT NULL,
               target_id TEXT NOT NULL,
               status TEXT NOT NULL DEFAULT 'pending',
               resolved_by_comment_id TEXT REFERENCES task_comments(id) ON DELETE SET NULL,
               created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
               resolved_at TIMESTAMPTZ
             );
             CREATE INDEX IF NOT EXISTS task_mentions_source_idx ON task_mentions(source_task_id, status, created_at);
             CREATE INDEX IF NOT EXISTS task_mentions_target_idx ON task_mentions(target_type, target_id, status, created_at);
             INSERT INTO project_document_sections (document_id, section_key, title, content, sort_order)
               SELECT pd.id, seed.section_key, seed.title,
                 CASE seed.section_key
                   WHEN 'goal' THEN CASE WHEN p.description = '' THEN '项目目标待 Human 补充。' ELSE p.description END
                   WHEN 'scope' THEN '记录当前确认的范围、明确排除项与兼容边界。'
                   WHEN 'progress' THEN '项目已创建，任务进展将在文档刷新后汇总。'
                   WHEN 'decisions' THEN '尚无已确认决策。'
                   ELSE '尚无已识别风险。'
                 END,
                 seed.sort_order
               FROM project_documents pd
               JOIN projects p ON p.id = pd.project_id
               CROSS JOIN (VALUES ('goal', '项目目标', 10), ('scope', '范围与边界', 20), ('progress', '交付进展', 30),
                 ('decisions', '关键决策', 40), ('risks', '风险与待办', 50)) AS seed(section_key, title, sort_order)
               WHERE pd.doc_type = 'overview'
             ON CONFLICT (document_id, section_key) DO NOTHING;
             INSERT INTO project_document_versions (id, document_id, version_no, content, content_hash, source_type, created_by_actor_id, metadata)
               SELECT 'm4-initial-version-' || pd.id, pd.id, 1,
                 jsonb_build_object('sections', jsonb_agg(jsonb_build_object('section_key', s.section_key, 'title', s.title, 'content', s.content,
                   'sort_order', s.sort_order, 'locked_by_human', s.locked_by_human, 'revision', s.revision) ORDER BY s.sort_order, s.section_key))::text,
                 md5(jsonb_build_object('sections', jsonb_agg(jsonb_build_object('section_key', s.section_key, 'title', s.title, 'content', s.content,
                   'sort_order', s.sort_order, 'locked_by_human', s.locked_by_human, 'revision', s.revision) ORDER BY s.sort_order, s.section_key))::text),
                 'initial_generation', 'system', jsonb_build_object('migration', '0006_m4_project_context')
               FROM project_documents pd
               JOIN project_document_sections s ON s.document_id = pd.id
               WHERE NOT EXISTS (SELECT 1 FROM project_document_versions existing WHERE existing.document_id = pd.id)
               GROUP BY pd.id
             ON CONFLICT (document_id, version_no) DO NOTHING;
             UPDATE project_documents pd SET current_version_no = versions.version_no, revision = GREATEST(pd.revision, versions.version_no),
               last_refreshed_at = COALESCE(pd.last_refreshed_at, now())
               FROM (SELECT document_id, max(version_no) AS version_no FROM project_document_versions GROUP BY document_id) versions
               WHERE pd.id = versions.document_id AND pd.current_version_no < versions.version_no;
             INSERT INTO schema_migrations (version) VALUES ('0006_m4_project_context') ON CONFLICT (version) DO NOTHING;
             CREATE TABLE IF NOT EXISTS workflows (
               id TEXT PRIMARY KEY,
               project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
               name TEXT NOT NULL,
               description TEXT NOT NULL DEFAULT '',
               status TEXT NOT NULL DEFAULT 'active',
               current_version_no INTEGER NOT NULL DEFAULT 0,
               created_by TEXT NOT NULL DEFAULT 'human',
               created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
               updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
             );
             CREATE INDEX IF NOT EXISTS workflows_project_idx ON workflows(project_id, status, updated_at DESC);
             CREATE TABLE IF NOT EXISTS workflow_versions (
               id TEXT PRIMARY KEY,
               workflow_id TEXT NOT NULL REFERENCES workflows(id) ON DELETE CASCADE,
               version_no INTEGER NOT NULL,
               status TEXT NOT NULL DEFAULT 'draft',
               definition JSONB NOT NULL DEFAULT '{}'::jsonb,
               created_by TEXT NOT NULL DEFAULT 'human',
               created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
               UNIQUE (workflow_id, version_no)
             );
             CREATE TABLE IF NOT EXISTS workflow_nodes (
               id TEXT PRIMARY KEY,
               version_id TEXT NOT NULL REFERENCES workflow_versions(id) ON DELETE CASCADE,
               node_key TEXT NOT NULL,
               node_type TEXT NOT NULL,
               label TEXT NOT NULL,
               config JSONB NOT NULL DEFAULT '{}'::jsonb,
               position_x DOUBLE PRECISION NOT NULL DEFAULT 0,
               position_y DOUBLE PRECISION NOT NULL DEFAULT 0,
               UNIQUE (version_id, node_key)
             );
             CREATE TABLE IF NOT EXISTS workflow_edges (
               id TEXT PRIMARY KEY,
               version_id TEXT NOT NULL REFERENCES workflow_versions(id) ON DELETE CASCADE,
               edge_key TEXT NOT NULL,
               source_node_key TEXT NOT NULL,
               target_node_key TEXT NOT NULL,
               label TEXT NOT NULL DEFAULT '',
               condition JSONB NOT NULL DEFAULT '{}'::jsonb,
               UNIQUE (version_id, edge_key)
             );
             CREATE INDEX IF NOT EXISTS workflow_nodes_version_idx ON workflow_nodes(version_id);
             CREATE INDEX IF NOT EXISTS workflow_edges_version_idx ON workflow_edges(version_id);
             CREATE TABLE IF NOT EXISTS workflow_schedules (
               id TEXT PRIMARY KEY,
               workflow_id TEXT NOT NULL REFERENCES workflows(id) ON DELETE CASCADE,
               schedule_type TEXT NOT NULL,
               schedule_expression TEXT NOT NULL,
               parsed_rule JSONB NOT NULL DEFAULT '{}'::jsonb,
               timezone TEXT NOT NULL DEFAULT 'Asia/Shanghai',
               enabled BOOLEAN NOT NULL DEFAULT FALSE,
               next_run_at TIMESTAMPTZ,
               created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
               updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
             );
             CREATE INDEX IF NOT EXISTS workflow_schedules_due_idx ON workflow_schedules(enabled, next_run_at);
             CREATE TABLE IF NOT EXISTS workflow_runs (
               id TEXT PRIMARY KEY,
               workflow_id TEXT NOT NULL REFERENCES workflows(id) ON DELETE CASCADE,
               version_id TEXT NOT NULL REFERENCES workflow_versions(id),
               project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
               status TEXT NOT NULL DEFAULT 'queued',
               trigger_type TEXT NOT NULL DEFAULT 'manual',
               input JSONB NOT NULL DEFAULT '{}'::jsonb,
               output JSONB NOT NULL DEFAULT '{}'::jsonb,
               error_message TEXT,
               started_at TIMESTAMPTZ,
               finished_at TIMESTAMPTZ,
               created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
               updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
             );
             CREATE INDEX IF NOT EXISTS workflow_runs_workflow_idx ON workflow_runs(workflow_id, created_at DESC);
             CREATE TABLE IF NOT EXISTS workflow_node_runs (
               id TEXT PRIMARY KEY,
               run_id TEXT NOT NULL REFERENCES workflow_runs(id) ON DELETE CASCADE,
               node_key TEXT NOT NULL,
               node_type TEXT NOT NULL,
               status TEXT NOT NULL DEFAULT 'queued',
               task_id TEXT REFERENCES tasks(id) ON DELETE SET NULL,
               attempt_count INTEGER NOT NULL DEFAULT 0,
               input JSONB NOT NULL DEFAULT '{}'::jsonb,
               output JSONB NOT NULL DEFAULT '{}'::jsonb,
               error_message TEXT,
               started_at TIMESTAMPTZ,
               finished_at TIMESTAMPTZ,
               created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
               updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
               UNIQUE (run_id, node_key)
             );
             CREATE INDEX IF NOT EXISTS workflow_node_runs_run_idx ON workflow_node_runs(run_id, created_at);
             CREATE TABLE IF NOT EXISTS approval_requests (
               id TEXT PRIMARY KEY,
               request_type TEXT NOT NULL,
               workflow_run_id TEXT REFERENCES workflow_runs(id) ON DELETE CASCADE,
               node_run_id TEXT REFERENCES workflow_node_runs(id) ON DELETE CASCADE,
               task_id TEXT REFERENCES tasks(id) ON DELETE SET NULL,
               execution_job_id TEXT REFERENCES execution_jobs(id) ON DELETE SET NULL,
               status TEXT NOT NULL DEFAULT 'pending',
               prompt TEXT NOT NULL DEFAULT '',
               response_data JSONB NOT NULL DEFAULT '{}'::jsonb,
               requested_at TIMESTAMPTZ NOT NULL DEFAULT now(),
               resolved_at TIMESTAMPTZ,
               resolved_by TEXT
             );
             CREATE INDEX IF NOT EXISTS approval_requests_pending_idx ON approval_requests(status, requested_at);
             CREATE INDEX IF NOT EXISTS approval_requests_run_idx ON approval_requests(workflow_run_id, requested_at);
             ALTER TABLE tasks ADD COLUMN IF NOT EXISTS source_type TEXT NOT NULL DEFAULT 'manual';
             ALTER TABLE tasks ADD COLUMN IF NOT EXISTS source_workflow_run_id TEXT REFERENCES workflow_runs(id) ON DELETE SET NULL;
             ALTER TABLE tasks ADD COLUMN IF NOT EXISTS source_node_run_id TEXT REFERENCES workflow_node_runs(id) ON DELETE SET NULL;
             ALTER TABLE tasks ADD COLUMN IF NOT EXISTS workflow_name TEXT;
             ALTER TABLE workflow_runs ADD COLUMN IF NOT EXISTS parent_task_id TEXT REFERENCES tasks(id) ON DELETE SET NULL;
             ALTER TABLE run_outputs ADD COLUMN IF NOT EXISTS workflow_run_id TEXT REFERENCES workflow_runs(id) ON DELETE CASCADE;
             ALTER TABLE run_outputs ADD COLUMN IF NOT EXISTS node_run_id TEXT REFERENCES workflow_node_runs(id) ON DELETE SET NULL;
             CREATE TABLE IF NOT EXISTS workflow_run_events (
               id BIGSERIAL PRIMARY KEY,
               run_id TEXT NOT NULL REFERENCES workflow_runs(id) ON DELETE CASCADE,
               event_type TEXT NOT NULL,
               node_key TEXT,
               payload JSONB NOT NULL DEFAULT '{}'::jsonb,
               created_at TIMESTAMPTZ NOT NULL DEFAULT now()
             );
             CREATE INDEX IF NOT EXISTS workflow_run_events_order_idx ON workflow_run_events(run_id, created_at, id);
             INSERT INTO schema_migrations (version) VALUES ('0007_m5_workflows') ON CONFLICT (version) DO NOTHING;",
        )
        .await
        .expect("apply migration");
    println!("xiexu migration complete");
}
