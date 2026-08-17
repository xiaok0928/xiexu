import { useEffect, useMemo, useRef, useState } from "react";
import { createRoot } from "react-dom/client";
import "./styles.css";

/** 一级导航模块标识。 */
type ViewKey = "board" | "project" | "workflow" | "chat" | "agents" | "runs" | "settings";
/** 项目列表与当前项目使用的稳定字段。 */
type Project = { id: string; name: string; description: string };
/** 看板卡片及其任务状态聚合。 */
type TaskCard = {
  id: string;
  project_id: string;
  parent_task_id?: string;
  title: string;
  description: string;
  board_stage: string;
  plan_status: string;
  execution_status: string;
  progress_percent: number;
  requires_plan_confirmation: boolean;
  children_count: number;
  /** 是否存在尚未解除的结构化协作请求。 */
  collaboration_status: "ready" | "waiting";
  revision: number;
};
/** 内置 Agent 角色模板。 */
type AgentTemplate = { code: string; name: string; category: string; description: string };
/** 可独立补充职责的 Agent 实例。 */
type Agent = { id: string; template_code?: string; name: string; description: string; instructions: string; responsibility_supplement: string; status: string };
/** Agent 与项目之间的长期协作关系。 */
type ProjectAgent = { agent_id: string; name: string; assignment_type: string; assignment_status: string };
/** 固定归属于 Agent 的执行经验。 */
type Memory = { id: string; tier: string; content: string; source_type: string };
/** 私聊、项目主群或项目临时群的摘要字段。 */
type Conversation = { id: string; conversation_type: string; project_id?: string; title: string; status: string; participant_count: number; task_count: number };
/** 对话中的追加消息事实。 */
type Message = { id: string; author_type: string; author_id: string; content: string; message_type: string; task_id?: string };
/** 任务详情需要展示的执行输出与事件摘要。 */
type ExecutionSnapshot = { outputs: Array<{ id: string; output_type: string; content: string }>; events: Array<{ id: number; event_type: string }> };
/** 评论中的结构化提及及其解除状态。 */
type TaskMention = {
  /** 提及事实主键。 */
  id: string;
  /** 接收目标的实体类型。 */
  target_type: "task" | "agent";
  /** 接收任务或 Agent 的稳定主键。 */
  target_id: string;
  /** 服务端解析的可读目标名称。 */
  target_name?: string | null;
  /** 提及当前是否已解除。 */
  status: string;
  /** 相对当前任务的提及方向。 */
  direction?: "outgoing" | "incoming";
  /** 发出提及的任务主键。 */
  source_task_id?: string | null;
  /** 发出提及的任务名称。 */
  source_task_title?: string | null;
  /** 解除提及的评论主键。 */
  resolved_by_comment_id?: string | null;
};
/** 任务评论及其父评论和提及关系。 */
type TaskComment = {
  /** 评论事实主键。 */
  id: string;
  /** 当前评论直接回复的父评论。 */
  parent_comment_id?: string | null;
  /** 评论作者显示名。 */
  author_name: string;
  /** 评论正文。 */
  content: string;
  /** 显式状态机意图。 */
  intent: string;
  /** 由当前评论创建的结构化提及。 */
  mentions: TaskMention[];
};
/** 项目文档列表项及其待处理变更聚合。 */
type ProjectDocumentSummary = {
  /** 文档主键。 */
  id: string;
  /** 归属项目主键。 */
  project_id: string;
  /** 稳定文档类型。 */
  doc_type: string;
  /** 文档标题。 */
  title: string;
  /** 文档聚合修订号。 */
  revision: number;
  /** 当前不可变版本号。 */
  current_version_no: number;
  /** 文档生命周期状态。 */
  status: string;
  /** 当前章节数量。 */
  section_count: number;
  /** 待处理或冲突候选数量。 */
  pending_candidate_count: number;
  /** 最近一次后台刷新完成时间。 */
  last_refreshed_at?: string | null;
  /** 文档聚合更新时间。 */
  updated_at: string;
};
/** 可独立编辑和锁定的项目文档章节。 */
type DocumentSection = {
  /** 章节稳定业务键。 */
  section_key: string;
  /** 章节标题。 */
  title: string;
  /** 当前正文内容。 */
  content: string;
  /** 文档内展示顺序。 */
  sort_order: number;
  /** Human 是否禁止自动覆盖。 */
  locked_by_human: boolean;
  /** 章节并发修订号。 */
  revision: number;
  /** 章节事实更新时间。 */
  updated_at: string;
};
/** 文档候选变更，Human 明确处置前不会覆盖当前章节。 */
type DocumentCandidate = {
  /** 候选事实主键。 */
  id: string;
  /** 建议更新的章节键。 */
  section_key: string;
  /** 建议替换的完整正文。 */
  proposed_content: string;
  /** 候选变更来源类型。 */
  source_type: string;
  /** 来源任务或作业主键。 */
  source_id?: string | null;
  /** 生成候选时的章节修订号。 */
  base_section_revision: number;
  /** 候选当前处置状态。 */
  status: string;
  /** 无法直接应用时的冲突原因。 */
  conflict_reason?: string;
  /** 候选生成时间。 */
  created_at: string;
  /** 候选完成处置的时间。 */
  resolved_at?: string | null;
};
/** 项目文档当前事实及其章节、候选更新。 */
type ProjectDocument = Omit<ProjectDocumentSummary, "section_count" | "pending_candidate_count"> & { sections: DocumentSection[]; candidates: DocumentCandidate[] };
/** 不可变文档版本的审计摘要。 */
type DocumentVersion = {
  /** 版本事实主键。 */
  id: string;
  /** 文档内单调递增版本号。 */
  version_no: number;
  /** 用于审计的快照内容哈希。 */
  content_hash: string;
  /** 版本生成来源。 */
  source_type: string;
  /** 创建版本的主体主键。 */
  created_by_actor_id: string;
  /** 触发版本的任务主键。 */
  source_task_id?: string | null;
  /** 回退操作引用的历史版本号。 */
  rollback_from_version_no?: number | null;
  /** 版本创建时间。 */
  created_at: string;
};
/** 章节级文档版本差异。 */
type DocumentDiff = {
  /** 差异基线版本。 */
  from: number;
  /** 差异目标版本。 */
  to: number;
  /** 仅包含实际变化章节的前后快照。 */
  changes: Array<{ section_key: string; change_type: string; before?: DocumentSection | null; after?: DocumentSection | null }>;
};
/** 后端持久化的工作流节点类型，界面分别展示为开始、结束、执行、判断和人工确认。 */
type WorkflowNodeType = "start" | "end" | "execute" | "condition" | "human_confirm";
/** 工作流画布节点，坐标以画布左上角为原点并持久化到版本定义。 */
type WorkflowNode = { id: string; type: WorkflowNodeType; label: string; config: Record<string, unknown>; x: number; y: number };
/** 工作流画布连线，判断节点的出口使用是/否标签表达分支。 */
type WorkflowEdge = { id: string; source: string; target: string; label: string; condition: Record<string, unknown> };
/** 工作流摘要及其当前不可变版本号。 */
type WorkflowSummary = {
  id: string;
  project_id: string;
  name: string;
  description: string;
  status: string;
  current_version_no: number;
  created_by: string;
  created_at: string;
  updated_at: string;
};
/** 工作流详情包含当前版本及可恢复的结构化画布。 */
type WorkflowDetail = WorkflowSummary & {
  version: {
    id: string;
    version_no: number;
    status: string;
    created_by: string;
    created_at: string;
    nodes: WorkflowNode[];
    edges: WorkflowEdge[];
  };
};
/** 工作流运行摘要，控制接口会在同一结构上返回最新状态。 */
type WorkflowRun = {
  id: string;
  workflow_id: string;
  version_id: string;
  project_id: string;
  status: string;
  trigger_type: string;
  input: unknown;
  output: unknown;
  error_message?: string | null;
  started_at?: string | null;
  finished_at?: string | null;
  created_at: string;
  updated_at: string;
};
/** 工作流运行事件用于还原节点执行状态和输出时间线。 */
type WorkflowRunEvent = { id: number; event_type: string; node_key?: string | null; payload: Record<string, unknown>; created_at: string };
/** 单次运行详情追加完整事件列表。 */
type WorkflowRunDetail = WorkflowRun & { events: WorkflowRunEvent[] };
/** 工作流定时规则，AI 解析结果必须以结构化对象持久化。 */
type WorkflowSchedule = {
  id: string;
  workflow_id: string;
  schedule_type: "periodic" | "scheduled" | "ai_parsed";
  schedule_expression: string;
  parsed_rule: Record<string, unknown>;
  timezone: string;
  enabled: boolean;
  next_run_at?: string | null;
  created_at: string;
  updated_at: string;
};
/** 人工确认请求，评论与布尔决定会共同进入运行快照。 */
type WorkflowApproval = {
  id: string;
  request_type: string;
  workflow_run_id?: string | null;
  node_run_id?: string | null;
  task_id?: string | null;
  status: string;
  prompt: string;
  response_data: Record<string, unknown>;
  requested_at: string;
  resolved_at?: string | null;
  resolved_by?: string | null;
};
/** 工作流运行及其子任务产生的统一输出。 */
type WorkflowOutput = {
  id: string;
  job_id: string;
  task_id?: string | null;
  output_type: string;
  content: string;
  node_run_id?: string | null;
  created_at: string;
};

/** 项目 Git 可选能力的服务端配置快照，只消费服务端返回的真实仓库信息。 */
type ProjectGitConfig = {
  enabled: boolean;
  repository_available?: boolean;
  repository_path?: string | null;
  status_summary?: string | null;
  current_head?: string | null;
};

/** 已创建 detached worktree 的只读会话记录。 */
type GitWorktreeSession = {
  id: string;
  task_id?: string | null;
  status?: string | null;
  worktree_path?: string | null;
  created_at?: string | null;
};

const stages = ["backlog", "todo", "plan_review", "in_progress", "acceptance"];
const stageLabels: Record<string, string> = { backlog: "Backlog", todo: "Todo", plan_review: "方案待确认", in_progress: "处理中", acceptance: "等待验收" };
const navItems: Array<{ key: ViewKey; label: string; icon: string }> = [
  { key: "board", label: "任务面板", icon: "▦" },
  { key: "project", label: "项目空间", icon: "□" },
  { key: "workflow", label: "工作流", icon: "⌘" },
  { key: "chat", label: "新对话", icon: "○" },
  { key: "agents", label: "Agent", icon: "♙" },
  { key: "runs", label: "运行记录", icon: "≡" },
  { key: "settings", label: "设置", icon: "⚙" },
];

/** 从当前 URL 恢复一级模块。 */
function viewFromLocation(): ViewKey {
  // 路径只影响模块入口，模块内部状态由真实数据恢复。
  const path = window.location.pathname;
  for (const key of ["project", "workflow", "chat", "agents", "runs", "settings"] as ViewKey[]) if (path.startsWith(`/${key}`)) return key;
  return "board";
}

/** 生成一级模块的稳定深链。 */
function pathForView(view: ViewKey): string {
  // 看板使用根路径，其余模块直接使用模块名。
  return view === "board" ? "/" : `/${view}`;
}

/** 统一执行 JSON API 请求，并保留服务端错误正文。 */
async function api<T>(path: string, options?: RequestInit): Promise<T> {
  // 当前 API 均返回 JSON，因此非成功响应直接作为异常抛出。
  const response = await fetch(path, { headers: { "Content-Type": "application/json" }, ...options });
  if (!response.ok) throw new Error(await response.text());
  if (response.status === 204) return undefined as T;
  return response.json() as Promise<T>;
}

/** 协序工作台：维护当前项目并挂载任务与 Agent 协作模块。 */
function App() {
  const [view, setView] = useState<ViewKey>(viewFromLocation);
  const [projects, setProjects] = useState<Project[]>([]);
  const [project, setProject] = useState<Project>();
  const [tasks, setTasks] = useState<TaskCard[]>([]);
  const [selectedTask, setSelectedTask] = useState<TaskCard>();
  const [error, setError] = useState("");

  /** 加载项目和任务，空数据库自动创建一个完整初始项目。 */
  async function loadBoard(preferredProjectId?: string) {
    try {
      // 项目初始化由后端原子创建协调 Agent 和项目主群聊。
      setError("");
      let result = await api<{ items: Project[] }>("/api/projects");
      if (!result.items.length) {
        await api("/api/projects", { method: "POST", body: JSON.stringify({ name: "xiexu", description: "协序多 Agent 项目空间" }) });
        result = await api<{ items: Project[] }>("/api/projects");
      }

      // 优先保留用户当前项目，找不到时使用最近项目。
      const current = result.items.find((item) => item.id === (preferredProjectId ?? project?.id)) ?? result.items[0];
      setProjects(result.items);
      setProject(current);
      const taskResult = await api<{ items: TaskCard[] }>(`/api/projects/${current.id}/tasks`);
      setTasks(taskResult.items);
      setSelectedTask((previous) => taskResult.items.find((task) => task.id === previous?.id) ?? taskResult.items[0]);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : "加载失败");
    }
  }

  useEffect(() => {
    void loadBoard();
  }, []);

  /** 切换模块并同步浏览器地址。 */
  function navigate(next: ViewKey) {
    // 不刷新页面，保留当前项目和任务选择。
    window.history.pushState({}, "", pathForView(next));
    setView(next);
  }

  /** 记录一个不会自动执行的 Backlog 想法。 */
  async function createIdea() {
    // 快速入口只收集标题，后续可在任务详情补充。
    if (!project) return;
    const title = window.prompt("记录一个想法");
    if (!title?.trim()) return;
    await api(`/api/projects/${project.id}/tasks`, { method: "POST", body: JSON.stringify({ title: title.trim(), description: "", requires_plan_confirmation: true }) });
    await loadBoard(project.id);
  }

  return (
    <div className="app-shell">
      <aside className="sidebar">
        <div className="brand">
          <span className="brand-mark">协</span>
          <div>
            <b>协序</b>
            <small>xiexu M5</small>
          </div>
        </div>
        <nav className="nav-list">
          {navItems.map((item) => (
            <button key={item.key} className={view === item.key ? "nav-item active" : "nav-item"} onClick={() => navigate(item.key)}>
              <span>{item.icon}</span>
              {item.label}
            </button>
          ))}
        </nav>
        <div className="recent">
          <small>最近项目</small>
          {projects.map((item) => (
            <button className={project?.id === item.id ? "recent-active" : ""} key={item.id} onClick={() => void loadBoard(item.id)}>
              {item.name}
            </button>
          ))}
        </div>
        <div className="user-box">
          <span className="avatar">H</span>
          <div>
            <b>Human</b>
            <small>本地管理员</small>
          </div>
        </div>
      </aside>
      <main className="workspace">
        <header className="topbar">
          <div>
            <small>协序 / Agent 协作</small>
            <h1>{navItems.find((item) => item.key === view)?.label}</h1>
          </div>
          <div className="toolbar">
            <button title="搜索">⌕</button>
            <button title="过滤">≡</button>
            <button className="automation" onClick={() => navigate("workflow")}>
              ▷ 自动化
            </button>
            <button className="primary" title="记录想法" onClick={() => void createIdea()}>
              ＋
            </button>
          </div>
        </header>
        {error && <div className="error-banner">{error}</div>}
        {view === "board" && project && (
          <Board tasks={tasks} selectedTask={selectedTask} onSelect={setSelectedTask} onReload={() => loadBoard(project.id)} onError={setError} />
        )}
        {view === "project" && project && <ProjectSpace project={project} onReload={() => loadBoard(project.id)} onError={setError} />}
        {view === "chat" && <DirectChat onError={setError} />}
        {view === "agents" && <AgentCenter project={project} onError={setError} />}
        {view === "workflow" && project && <WorkflowCenter project={project} onError={setError} />}
        {view === "runs" && <Placeholder title="运行记录" description="任务运行记录已在任务详情展示，跨任务总览将在后续阶段接入。" />}
        {view === "settings" &&
          (project ? (
            <GitSettings project={project} tasks={tasks} onError={setError} />
          ) : (
            <Placeholder title="设置" description="项目加载中，请稍后重试。" />
          ))}
      </main>
    </div>
  );
}

/** 任务面板：展示真实阶段并通过服务端状态机移动任务。 */
function Board({
  tasks,
  selectedTask,
  onSelect,
  onReload,
  onError,
}: {
  tasks: TaskCard[];
  selectedTask?: TaskCard;
  onSelect: (task: TaskCard) => void;
  onReload: () => Promise<void>;
  onError: (message: string) => void;
}) {
  const [mobileStage, setMobileStage] = useState(stages[0]);
  const grouped = useMemo(() => Object.fromEntries(stages.map((stage) => [stage, tasks.filter((task) => task.board_stage === stage)])), [tasks]);

  /** 请求服务端执行阶段转换。 */
  async function move(task: TaskCard, target: string) {
    // 前端不直接改写阶段，冲突由服务端返回。
    try {
      await api(`/api/tasks/${task.id}/transitions`, { method: "POST", body: JSON.stringify({ target_stage: target, reason: "Human 在任务面板中移动" }) });
      await onReload();
    } catch (cause) {
      onError(cause instanceof Error ? cause.message : "状态转换失败");
    }
  }

  /** 切换单卡方案确认设置。 */
  async function togglePlan(task: TaskCard) {
    // 默认需要确认，用户可在 Todo 卡片上取消。
    await api(`/api/tasks/${task.id}`, { method: "PATCH", body: JSON.stringify({ requires_plan_confirmation: !task.requires_plan_confirmation }) });
    await onReload();
  }

  return (
    <section className="view">
      <div className="view-head">
        <div className="tabs">
          <button className="tab active">看板</button>
          <button className="tab">列表</button>
          <button className="tab">甘特</button>
        </div>
        <select value={mobileStage} onChange={(event) => setMobileStage(event.target.value)}>
          {stages.map((stage) => (
            <option key={stage} value={stage}>
              {stageLabels[stage]}
            </option>
          ))}
        </select>
      </div>
      <div className="board-layout">
        <div className="kanban">
          {stages.map((stage) => (
            <section
              className={`column ${stage === mobileStage ? "mobile-visible" : ""}`}
              key={stage}
              onDragOver={(event) => event.preventDefault()}
              onDrop={(event) => {
                const task = tasks.find((item) => item.id === event.dataTransfer.getData("text/plain"));
                if (task && task.board_stage !== stage) void move(task, stage);
              }}
            >
              <div className="column-head">
                <span>{stageLabels[stage]}</span>
                <small>{grouped[stage].length}</small>
              </div>
              {grouped[stage].map((task) => (
                <button
                  className={selectedTask?.id === task.id ? "task-card selected" : "task-card"}
                  draggable
                  onDragStart={(event) => event.dataTransfer.setData("text/plain", task.id)}
                  key={task.id}
                  onClick={() => onSelect(task)}
                >
                  <small>
                    {task.id.slice(0, 8)}
                    {task.parent_task_id ? ` · 父 ${task.parent_task_id.slice(0, 8)}` : ""}
                  </small>
                  <b>{task.title}</b>
                  <span className="task-status">
                    {task.execution_status} · {task.progress_percent}%
                  </span>
                  {stage === "todo" && (
                    <span
                      className="confirm"
                      onClick={(event) => {
                        event.stopPropagation();
                        void togglePlan(task);
                      }}
                    >
                      ☑ 方案确认 {task.requires_plan_confirmation ? "开" : "关"}
                    </span>
                  )}
                </button>
              ))}
            </section>
          ))}
        </div>
        {selectedTask ? (
          <TaskDetail task={selectedTask} projectTasks={tasks} onReload={onReload} onError={onError} />
        ) : (
          <div className="detail">选择任务查看详情</div>
        )}
      </div>
    </section>
  );
}

/** 任务详情：展示运行事实，并通过结构化评论维护提及、回复和解除关系。 */
function TaskDetail({
  task,
  projectTasks,
  onReload,
  onError,
}: {
  task: TaskCard;
  projectTasks: TaskCard[];
  onReload: () => Promise<void>;
  onError: (message: string) => void;
}) {
  const [comments, setComments] = useState<TaskComment[]>([]);
  const [execution, setExecution] = useState<ExecutionSnapshot>({ outputs: [], events: [] });
  const [agents, setAgents] = useState<Array<{ agent_id: string; name: string; participation_type: string; status: string }>>([]);
  const [projectAgents, setProjectAgents] = useState<ProjectAgent[]>([]);
  const [taskMentions, setTaskMentions] = useState<TaskMention[]>([]);
  const [content, setContent] = useState("");
  const [intent, setIntent] = useState("note");
  const [mentionType, setMentionType] = useState<"" | "agent" | "task">("");
  const [mentionTargetId, setMentionTargetId] = useState("");
  const [parentCommentId, setParentCommentId] = useState("");
  const [resolvesMentionId, setResolvesMentionId] = useState("");

  /** 并行加载任务详情事实。 */
  async function load() {
    // 评论、运行、参与者、项目成员和提及彼此独立，合并等待后一次刷新界面。
    const [commentResult, executionResult, agentResult, projectAgentResult, mentionResult] = await Promise.all([
      api<{ items: typeof comments }>(`/api/tasks/${task.id}/comments`),
      api<ExecutionSnapshot>(`/api/tasks/${task.id}/execution`),
      api<{ items: typeof agents }>(`/api/tasks/${task.id}/agents`),
      api<{ items: ProjectAgent[] }>(`/api/projects/${task.project_id}/agents`),
      api<{ items: TaskMention[] }>(`/api/tasks/${task.id}/mentions`),
    ]);
    setComments(commentResult.items);
    setExecution(executionResult);
    setAgents(agentResult.items);
    setProjectAgents(projectAgentResult.items.filter((item) => item.assignment_status === "active"));
    setTaskMentions(mentionResult.items);
  }
  useEffect(() => {
    // 切换任务时清理上一任务的回复上下文，避免关系误提交到新任务。
    setParentCommentId("");
    setResolvesMentionId("");
    setMentionType("");
    setMentionTargetId("");
    void load().catch((cause) => onError(cause instanceof Error ? cause.message : "任务详情加载失败"));
  }, [task.id, task.revision]);

  /** 发送带显式意图、提及、父评论和解除目标的任务评论。 */
  async function send() {
    // 空内容不写入事实源；提及目标必须完整选择，不能依赖正文解析。
    if (!content.trim()) return;
    if (mentionType && !mentionTargetId) return;

    // 关系字段只在用户显式选择时提交，服务端在一个事务中维护等待状态。
    try {
      await api(`/api/tasks/${task.id}/comments`, {
        method: "POST",
        body: JSON.stringify({
          content: content.trim(),
          intent: mentionType && intent === "note" ? "mention" : intent,
          parent_comment_id: parentCommentId || undefined,
          mentions: mentionType && mentionTargetId ? [{ target_type: mentionType, target_id: mentionTargetId }] : [],
          resolves_mention_id: resolvesMentionId || undefined,
        }),
      });

      // 成功后清空一次性关系选择，并同步任务聚合和评论事实。
      setContent("");
      setMentionType("");
      setMentionTargetId("");
      setParentCommentId("");
      setResolvesMentionId("");
      await onReload();
      await load();
    } catch (cause) {
      onError(cause instanceof Error ? cause.message : "评论发送失败");
    }
  }

  /** 选择父评论作为回复上下文。 */
  function replyTo(commentId: string) {
    // 解除提及回复会同时关联原评论，普通回复只建立父子关系。
    setParentCommentId(commentId);
    setContent((current) => current || "回复：");
  }

  /** 选择一个待处理提及，并准备通过新评论显式解除。 */
  function resolveMention(mention: TaskMention, commentId?: string) {
    // 只有当前任务发出的 pending 提及可由该评论接口解除。
    if (mention.status !== "pending" || mention.direction === "incoming") return;
    setResolvesMentionId(mention.id);
    setParentCommentId(commentId ?? "");
    setContent((current) => current || "已处理：");
  }

  // 主责参与者和当前回复对象只从已加载事实中派生，不维护重复状态。
  const owner = agents.find((item) => item.participation_type === "owner" && item.status === "active");
  const parentComment = comments.find((item) => item.id === parentCommentId);
  const resolvingMention =
    taskMentions.find((item) => item.id === resolvesMentionId) ?? comments.flatMap((item) => item.mentions).find((item) => item.id === resolvesMentionId);
  const mentionTargets =
    mentionType === "agent"
      ? projectAgents.map((item) => ({ id: item.agent_id, label: item.name }))
      : projectTasks.filter((item) => item.id !== task.id).map((item) => ({ id: item.id, label: item.title }));
  return (
    <aside className="detail">
      <small>{task.id}</small>
      <h2>{task.title}</h2>
      <div className="fact">
        <span>阶段</span>
        <b>{stageLabels[task.board_stage]}</b>
        <span>执行</span>
        <b>{task.execution_status}</b>
        <span>协作</span>
        <b className={task.collaboration_status === "waiting" ? "status-waiting" : "status-ready"}>
          {task.collaboration_status === "waiting" ? "等待协作" : "可继续"}
        </b>
        <span>主责 Agent</span>
        <b>{owner?.name ?? "等待协调"}</b>
        <span>进度</span>
        <b>{task.progress_percent}%</b>
      </div>
      <h3>运行记录</h3>
      <div className="execution-list">
        {execution.outputs.map((item) => (
          <div className="execution-output" key={item.id}>
            <b>{item.output_type}</b>
            <p>{item.content}</p>
          </div>
        ))}
        {!execution.outputs.length && <p className="muted">暂无运行输出</p>}
      </div>
      <h3>评论</h3>
      <div className="comment-list">
        {comments.map((item) => {
          const parent = comments.find((candidate) => candidate.id === item.parent_comment_id);
          return (
            <div className={item.parent_comment_id ? "comment reply-comment" : "comment"} key={item.id}>
              <div className="comment-head">
                <b>
                  {item.author_name} · {item.intent}
                </b>
                <button className="inline-action" onClick={() => replyTo(item.id)}>回复</button>
              </div>
              {parent && <small className="reply-reference">回复 {parent.author_name}：{parent.content}</small>}
              <p>{item.content}</p>
              {!!item.mentions?.length && (
                <div className="mention-list">
                  {item.mentions.map((mention) => (
                    <span className={`mention-chip ${mention.status}`} key={mention.id}>
                      @{mention.target_name ?? mention.target_id.slice(0, 8)} · {mention.status}
                      {mention.status === "pending" && <button title="通过回复解除提及" onClick={() => resolveMention(mention, item.id)}>✓</button>}
                    </span>
                  ))}
                </div>
              )}
            </div>
          );
        })}
        {!comments.length && <p className="muted">暂无评论</p>}
      </div>
      {(parentComment || resolvingMention) && (
        <div className="reply-context">
          <span>{resolvingMention ? `解除 @${resolvingMention.target_name ?? resolvingMention.target_id.slice(0, 8)}` : `回复 ${parentComment?.author_name}`}</span>
          <button
            title="清除回复关系"
            onClick={() => {
              setParentCommentId("");
              setResolvesMentionId("");
            }}
          >
            ×
          </button>
        </div>
      )}
      <div className="comment-input">
        <input value={content} onChange={(event) => setContent(event.target.value)} placeholder="输入评论" />
        <select value={intent} onChange={(event) => setIntent(event.target.value)}>
          <option value="note">记录</option>
          <option value="approve_plan">确认方案</option>
          <option value="accept">验收通过</option>
          <option value="rework">返工</option>
        </select>
        <button onClick={() => void send()}>发送</button>
      </div>
      <div className="mention-picker">
        <select
          value={mentionType}
          onChange={(event) => {
            setMentionType(event.target.value as "" | "agent" | "task");
            setMentionTargetId("");
          }}
        >
          <option value="">不提及</option>
          <option value="agent">@Agent</option>
          <option value="task">@任务</option>
        </select>
        <select value={mentionTargetId} disabled={!mentionType} onChange={(event) => setMentionTargetId(event.target.value)}>
          <option value="">选择目标</option>
          {mentionTargets.map((target) => <option key={target.id} value={target.id}>{target.label}</option>)}
        </select>
      </div>
    </aside>
  );
}

/** Agent 中心：管理身份、特定职责和私有记忆。 */
function AgentCenter({ project, onError }: { project?: Project; onError: (message: string) => void }) {
  const [templates, setTemplates] = useState<AgentTemplate[]>([]);
  const [agents, setAgents] = useState<Agent[]>([]);
  const [selectedId, setSelectedId] = useState("");
  const [memories, setMemories] = useState<Memory[]>([]);
  const [templateCode, setTemplateCode] = useState("project_manager");
  const [name, setName] = useState("");
  const [supplement, setSupplement] = useState("");
  const [draft, setDraft] = useState("");
  const selected = agents.find((agent) => agent.id === selectedId);

  /** 加载角色模板与 Agent 实例。 */
  async function load() {
    // 模板和实例分开维护，模板更新不覆盖现有实例。
    const [templatesResult, agentsResult] = await Promise.all([api<{ items: AgentTemplate[] }>("/api/agent-templates"), api<{ items: Agent[] }>("/api/agents")]);
    setTemplates(templatesResult.items);
    setAgents(agentsResult.items);
    setSelectedId((current) => (agentsResult.items.some((item) => item.id === current) ? current : (agentsResult.items[0]?.id ?? "")));
  }
  useEffect(() => {
    void load().catch((cause) => onError(cause instanceof Error ? cause.message : "Agent 加载失败"));
  }, []);
  useEffect(() => {
    setSupplement(selected?.responsibility_supplement ?? "");
    setDraft("");
  }, [selectedId]);
  useEffect(() => {
    // 记忆固定按 Agent 查询，当前项目用于收窄业务上下文。
    if (!selectedId) return;
    const query = project ? `?project_id=${encodeURIComponent(project.id)}` : "";
    void api<{ items: Memory[] }>(`/api/agents/${selectedId}/memories${query}`).then((result) => setMemories(result.items));
  }, [selectedId, project?.id]);

  /** 从模板创建独立 Agent。 */
  async function createAgent() {
    // 模板提供基础职责，补充职责只属于新实例。
    if (!name.trim()) return;
    await api("/api/agents", { method: "POST", body: JSON.stringify({ name: name.trim(), template_code: templateCode, responsibility_supplement: supplement }) });
    setName("");
    setSupplement("");
    await load();
  }

  /** 保存当前 Agent 的职责补充。 */
  async function saveSupplement() {
    // 保存不会修改同模板的其他 Agent。
    if (!selected) return;
    await api(`/api/agents/${selected.id}`, { method: "PATCH", body: JSON.stringify({ responsibility_supplement: supplement }) });
    await load();
  }

  /** 创建职责优化作业并轮询 AI 草案输出。 */
  async function generateDraft() {
    // 草案不会自动覆盖 Agent，Human 采用后仍需显式保存。
    if (!selected) return;
    const description = window.prompt("补充说明这个 Agent 应承担的工作", selected.description);
    if (!description?.trim()) return;
    const created = await api<{ job_id: string }>("/api/agents/responsibility-drafts", {
      method: "POST",
      body: JSON.stringify({ agent_id: selected.id, name: selected.name, description: description.trim(), responsibility_supplement: supplement }),
    });
    setDraft("正在生成职责草案...");
    for (let attempt = 0; attempt < 20; attempt += 1) {
      await new Promise((resolve) => window.setTimeout(resolve, 1000));
      const job = await api<{ status: string; outputs: Array<{ content: string }> }>(`/api/execution-jobs/${created.job_id}`);
      if (job.status === "succeeded") {
        setDraft(job.outputs[0]?.content ?? "职责草案已生成");
        return;
      }
      if (job.status === "failed") {
        setDraft("职责草案生成失败，可重新尝试。");
        return;
      }
    }
    setDraft("职责草案仍在后台生成，可稍后从运行记录查看。");
  }

  /** 为当前 Agent 写入项目范围短期经验。 */
  async function addMemory() {
    // 记忆只绑定当前 Agent，不自动共享给其他身份。
    if (!selected) return;
    const content = window.prompt("记录该 Agent 的执行经验");
    if (!content?.trim()) return;
    await api(`/api/agents/${selected.id}/memories`, { method: "POST", body: JSON.stringify({ tier: "short_term", project_id: project?.id, content: content.trim() }) });
    const query = project ? `?project_id=${encodeURIComponent(project.id)}` : "";
    setMemories((await api<{ items: Memory[] }>(`/api/agents/${selected.id}/memories${query}`)).items);
  }

  return (
    <section className="view split-view">
      <aside className="panel list-panel">
        <div className="panel-head">
          <div>
            <small>身份目录</small>
            <h2>Agent</h2>
          </div>
          <span className="count">{agents.length}</span>
        </div>
        <div className="agent-list">
          {agents.map((agent) => (
            <button key={agent.id} className={agent.id === selectedId ? "agent-row active" : "agent-row"} onClick={() => setSelectedId(agent.id)}>
              <span className="agent-avatar">{agent.name.slice(0, 1)}</span>
              <span>
                <b>{agent.name}</b>
                <small>{agent.template_code ?? "custom"}</small>
              </span>
            </button>
          ))}
        </div>
        <div className="create-box">
          <h3>创建 Agent</h3>
          <select value={templateCode} onChange={(event) => setTemplateCode(event.target.value)}>
            {templates.map((template) => (
              <option key={template.code} value={template.code}>
                {template.name}
              </option>
            ))}
          </select>
          <input value={name} onChange={(event) => setName(event.target.value)} placeholder="Agent 名称" />
          <textarea value={supplement} onChange={(event) => setSupplement(event.target.value)} placeholder="特定职责补充" />
          <button className="primary-action" onClick={() => void createAgent()}>
            创建
          </button>
        </div>
      </aside>
      <div className="panel detail-panel">
        {selected ? (
          <>
            <div className="identity-head">
              <span className="agent-avatar large">{selected.name.slice(0, 1)}</span>
              <div>
                <small>{selected.template_code ?? "自定义"}</small>
                <h2>{selected.name}</h2>
                <p>{selected.description}</p>
              </div>
            </div>
            <label>
              基础指令
              <textarea value={selected.instructions} readOnly />
            </label>
            <label>
              特定职责补充
              <textarea value={supplement} onChange={(event) => setSupplement(event.target.value)} />
            </label>
            <div className="actions">
              <button onClick={() => void saveSupplement()}>保存职责</button>
              <button onClick={() => void generateDraft()}>AI 优化</button>
            </div>
            {draft && (
              <div className="draft-box">
                <b>职责草案</b>
                <p>{draft}</p>
                <button onClick={() => setSupplement(draft)}>采用草案</button>
              </div>
            )}
            <div className="section-head">
              <h3>私有记忆</h3>
              <button onClick={() => void addMemory()}>＋ 添加</button>
            </div>
            <div className="memory-list">
              {memories.map((memory) => (
                <div className="memory" key={memory.id}>
                  <small>
                    {memory.tier} · {memory.source_type}
                  </small>
                  <p>{memory.content}</p>
                </div>
              ))}
              {!memories.length && <p className="muted">当前范围暂无记忆</p>}
            </div>
          </>
        ) : (
          <p className="muted">选择 Agent</p>
        )}
      </div>
    </section>
  );
}

/** 工作流节点类型对应的中文业务名称。 */
const workflowNodeLabels: Record<WorkflowNodeType, string> = {
  start: "开始",
  end: "结束",
  execute: "执行",
  condition: "判断",
  human_confirm: "人工确认",
};
/** 工作流节点在工具箱和画布中使用的紧凑符号。 */
const workflowNodeIcons: Record<WorkflowNodeType, string> = { start: "▶", end: "■", execute: "⚙", condition: "◇", human_confirm: "✓" };

/** 工作流模块：在一个独立工作区内维护画布、版本保存和运行审计。 */
function WorkflowCenter({ project, onError }: { project: Project; onError: (message: string) => void }) {
  const [workflows, setWorkflows] = useState<WorkflowSummary[]>([]);
  const [selectedId, setSelectedId] = useState("");
  const [detail, setDetail] = useState<WorkflowDetail>();
  const [name, setName] = useState("");
  const [description, setDescription] = useState("");
  const [nodes, setNodes] = useState<WorkflowNode[]>([]);
  const [edges, setEdges] = useState<WorkflowEdge[]>([]);
  const [selectedNodeId, setSelectedNodeId] = useState("");
  const [connectionSourceId, setConnectionSourceId] = useState("");
  const [branchLabel, setBranchLabel] = useState<"是" | "否">("是");
  const [runs, setRuns] = useState<WorkflowRun[]>([]);
  const [selectedRunId, setSelectedRunId] = useState("");
  const [runDetail, setRunDetail] = useState<WorkflowRunDetail>();
  const [approvals, setApprovals] = useState<WorkflowApproval[]>([]);
  const [outputs, setOutputs] = useState<WorkflowOutput[]>([]);
  const [approvalComments, setApprovalComments] = useState<Record<string, string>>({});
  const [schedules, setSchedules] = useState<WorkflowSchedule[]>([]);
  const [scheduleType, setScheduleType] = useState<WorkflowSchedule["schedule_type"]>("periodic");
  const [scheduleExpression, setScheduleExpression] = useState("");
  const [scheduleRule, setScheduleRule] = useState("{}");
  const [scheduleTimezone, setScheduleTimezone] = useState("Asia/Shanghai");
  const [scheduleNextRunAt, setScheduleNextRunAt] = useState("");
  const [editingScheduleId, setEditingScheduleId] = useState("");
  const [activePane, setActivePane] = useState<"canvas" | "schedules" | "runs">("canvas");
  const [saving, setSaving] = useState(false);
  const [notice, setNotice] = useState("");
  const canvasRef = useRef<HTMLDivElement>(null);
  const selectedNode = nodes.find((node) => node.id === selectedNodeId);

  /** 加载当前项目的工作流目录，并保留仍然存在的选择。 */
  async function loadWorkflows(preferredId?: string) {
    // 项目是工作流的数据边界，切换项目时不沿用其他项目的工作流主键。
    const result = await api<{ items: WorkflowSummary[] }>(`/api/projects/${project.id}/workflows`);
    const nextId = result.items.find((item) => item.id === (preferredId ?? selectedId))?.id ?? result.items[0]?.id ?? "";
    setWorkflows(result.items);
    setSelectedId(nextId);
  }

  /** 加载工作流当前版本和运行摘要，保存后的画布事实以服务端响应为准。 */
  async function loadWorkflow(workflowId: string) {
    // 定义和运行记录可以独立读取，合并等待后再刷新编辑器，避免出现半新半旧状态。
    const [workflow, runResult, scheduleResult] = await Promise.all([
      api<WorkflowDetail>(`/api/workflows/${workflowId}`),
      api<{ items: WorkflowRun[] }>(`/api/workflows/${workflowId}/runs`),
      api<{ items: WorkflowSchedule[] }>(`/api/workflows/${workflowId}/schedules`),
    ]);

    // 旧版默认骨架没有坐标时按水平方向排布，确保第一次打开即可读。
    const restoredNodes = workflow.version.nodes.map((node, index) =>
      node.x === 0 && node.y === 0 ? { ...node, x: 72 + index * 230, y: 210 } : node,
    );

    // 仅在所有事实读取成功后替换本地编辑草稿和运行选择。
    setDetail(workflow);
    setName(workflow.name);
    setDescription(workflow.description);
    setNodes(restoredNodes);
    setEdges(workflow.version.edges);
    setSelectedNodeId((current) => (workflow.version.nodes.some((node) => node.id === current) ? current : ""));
    setConnectionSourceId("");
    setRuns(runResult.items);
    setSchedules(scheduleResult.items);
    setSelectedRunId((current) => (runResult.items.some((run) => run.id === current) ? current : runResult.items[0]?.id ?? ""));
  }

  /** 加载单次运行的事件时间线和节点输出。 */
  async function loadRun(runId: string) {
    // 运行详情始终覆盖旧详情，防止控制状态变化后继续展示过期输出。
    const [result, approvalResult, outputResult] = await Promise.all([
      api<WorkflowRunDetail>(`/api/workflow-runs/${runId}`),
      api<{ items: WorkflowApproval[] }>(`/api/workflow-runs/${runId}/approvals`),
      api<{ items: WorkflowOutput[] }>(`/api/workflow-runs/${runId}/outputs`),
    ]);

    // 三类运行事实成功读取后一次性替换详情，避免审批已刷新而输出仍来自上一运行。
    setRunDetail(result);
    setApprovals(approvalResult.items);
    setOutputs(outputResult.items);
  }

  useEffect(() => {
    // 项目变化时清空上一个项目的编辑上下文，再建立新的工作流目录。
    setSelectedId("");
    setDetail(undefined);
    setRunDetail(undefined);
    void loadWorkflows().catch((cause) => onError(cause instanceof Error ? cause.message : "工作流目录加载失败"));
  }, [project.id]);
  useEffect(() => {
    // 选择工作流后恢复其当前版本和运行历史。
    if (!selectedId) return;
    void loadWorkflow(selectedId).catch((cause) => onError(cause instanceof Error ? cause.message : "工作流加载失败"));
  }, [selectedId]);
  useEffect(() => {
    // 运行选择变化时读取完整事件；空选择必须同步清空右侧详情。
    if (!selectedRunId) {
      setRunDetail(undefined);
      setApprovals([]);
      setOutputs([]);
      return;
    }
    void loadRun(selectedRunId).catch((cause) => onError(cause instanceof Error ? cause.message : "运行详情加载失败"));
  }, [selectedRunId]);

  /** 创建一个带开始和结束节点的工作流。 */
  async function createWorkflow() {
    // 仅名称是创建必填项，默认骨架由服务端生成并成为版本 1。
    const workflowName = window.prompt("工作流名称");
    if (!workflowName?.trim()) return;
    try {
      const created = await api<WorkflowSummary>(`/api/projects/${project.id}/workflows`, {
        method: "POST",
        body: JSON.stringify({ name: workflowName.trim(), description: "" }),
      });
      await loadWorkflows(created.id);
      setSelectedId(created.id);
      setNotice("工作流已创建");
    } catch (cause) {
      onError(cause instanceof Error ? cause.message : "工作流创建失败");
    }
  }

  /** 将当前画布追加保存为不可变版本。 */
  async function saveWorkflow() {
    // 保存前保持最小结构约束，具体连线引用和类型仍由服务端做权威校验。
    if (!detail || !name.trim() || !nodes.length) return;
    setSaving(true);
    try {
      await api(`/api/workflows/${detail.id}`, {
        method: "PATCH",
        body: JSON.stringify({ name: name.trim(), description, nodes, edges }),
      });
      await loadWorkflow(detail.id);
      await loadWorkflows(detail.id);
      setNotice("工作流已保存为新版本");
    } catch (cause) {
      onError(cause instanceof Error ? cause.message : "工作流保存失败");
    } finally {
      setSaving(false);
    }
  }

  /** 暂停、恢复或终止工作流定义，只控制未来触发而不修改既有运行。 */
  async function controlWorkflow(action: "pause" | "resume" | "terminate") {
    // 终止不可恢复，因此提交前要求 Human 明确确认。
    if (!detail || (action === "terminate" && !window.confirm("确认终止该工作流？终止后不可恢复。"))) return;
    try {
      await api(`/api/workflows/${detail.id}/${action}`, { method: "POST", body: "{}" });
      await loadWorkflow(detail.id);
      await loadWorkflows(detail.id);
      setNotice(action === "pause" ? "工作流已暂停" : action === "resume" ? "工作流已恢复" : "工作流已终止");
    } catch (cause) {
      onError(cause instanceof Error ? cause.message : "工作流控制失败");
    }
  }

  /** 清空调度编辑器并恢复默认时区和规则类型。 */
  function resetScheduleForm() {
    // 编辑完成或取消后不保留旧规则，避免后续新增误覆盖原调度。
    setEditingScheduleId("");
    setScheduleType("periodic");
    setScheduleExpression("");
    setScheduleRule("{}");
    setScheduleTimezone("Asia/Shanghai");
    setScheduleNextRunAt("");
  }

  /** 将已有调度加载到表单以执行显式编辑。 */
  function editSchedule(schedule: WorkflowSchedule) {
    // 时间输入使用浏览器本地格式，仅展示精确到分钟的预定时间。
    setEditingScheduleId(schedule.id);
    setScheduleType(schedule.schedule_type);
    setScheduleExpression(schedule.schedule_expression);
    setScheduleRule(JSON.stringify(schedule.parsed_rule, null, 2));
    setScheduleTimezone(schedule.timezone);
    setScheduleNextRunAt(schedule.next_run_at ? schedule.next_run_at.slice(0, 16) : "");
  }

  /** 新建或更新周期、预定时间和 AI 解析调度。 */
  async function saveSchedule() {
    // 所有调度都要求可读表达式，AI 解析类型还必须提供合法的非空结构化规则。
    if (!detail || !scheduleExpression.trim()) return;
    let parsedRule: Record<string, unknown> = {};
    try {
      const parsed = JSON.parse(scheduleRule || "{}") as unknown;
      if (typeof parsed !== "object" || parsed === null || Array.isArray(parsed)) throw new Error("规则必须是 JSON 对象");
      parsedRule = parsed as Record<string, unknown>;
      if (scheduleType === "ai_parsed" && !Object.keys(parsedRule).length) throw new Error("AI 解析规则不能为空");
    } catch (cause) {
      onError(cause instanceof Error ? cause.message : "结构化规则格式错误");
      return;
    }

    // 预定时间转为 ISO 字符串，周期规则没有明确下次时间时交由后端调度器计算。
    const body = {
      schedule_type: scheduleType,
      schedule_expression: scheduleExpression.trim(),
      parsed_rule: parsedRule,
      timezone: scheduleTimezone.trim() || "Asia/Shanghai",
      next_run_at: scheduleNextRunAt ? new Date(scheduleNextRunAt).toISOString() : undefined,
      ...(editingScheduleId ? {} : { enabled: false }),
    };
    try {
      const path = editingScheduleId ? `/api/workflow-schedules/${editingScheduleId}` : `/api/workflows/${detail.id}/schedules`;
      await api(path, { method: editingScheduleId ? "PATCH" : "POST", body: JSON.stringify(body) });
      resetScheduleForm();
      await loadWorkflow(detail.id);
      setNotice(editingScheduleId ? "调度已更新" : "调度已创建，默认未启用");
    } catch (cause) {
      onError(cause instanceof Error ? cause.message : "调度保存失败");
    }
  }

  /** 显式启用或停用一条调度规则。 */
  async function toggleSchedule(schedule: WorkflowSchedule) {
    // 启停不修改表达式和下次执行时间，便于后续原样恢复。
    if (!detail) return;
    try {
      await api(`/api/workflow-schedules/${schedule.id}/${schedule.enabled ? "disable" : "enable"}`, { method: "POST", body: "{}" });
      await loadWorkflow(detail.id);
    } catch (cause) {
      onError(cause instanceof Error ? cause.message : "调度状态更新失败");
    }
  }

  /** 删除一条尚未需要保留历史的调度配置。 */
  async function removeSchedule(schedule: WorkflowSchedule) {
    // 删除不会影响已经产生的运行记录，但仍要求 Human 明确确认目标。
    if (!detail || !window.confirm(`删除调度“${schedule.schedule_expression}”？`)) return;
    try {
      await api(`/api/workflow-schedules/${schedule.id}`, { method: "DELETE" });
      if (editingScheduleId === schedule.id) resetScheduleForm();
      await loadWorkflow(detail.id);
    } catch (cause) {
      onError(cause instanceof Error ? cause.message : "调度删除失败");
    }
  }

  /** 向画布添加一种业务节点，并使用稳定坐标避免布局跳动。 */
  function addNode(type: WorkflowNodeType) {
    // 开始和结束节点只允许各存在一个，避免生成无法解释的多入口或多终点骨架。
    if ((type === "start" || type === "end") && nodes.some((node) => node.type === type)) return;
    const sequence = nodes.length + 1;
    const id = `${type}-${Date.now().toString(36)}`;
    const node: WorkflowNode = {
      id,
      type,
      label: type === "execute" ? `执行步骤 ${sequence}` : workflowNodeLabels[type],
      config: {},
      x: 72 + ((sequence - 1) % 3) * 230,
      y: 72 + Math.floor((sequence - 1) / 3) * 150,
    };
    setNodes((current) => [...current, node]);
    setSelectedNodeId(id);
  }

  /** 在拖动结束时把节点位置换算为画布内坐标。 */
  function moveNode(nodeId: string, clientX: number, clientY: number) {
    // 坐标约束给节点保留完整可见区域，并避免拖到画布负坐标。
    const bounds = canvasRef.current?.getBoundingClientRect();
    if (!bounds) return;
    const x = Math.max(12, Math.min(bounds.width - 184, clientX - bounds.left - 84));
    const y = Math.max(12, Math.min(bounds.height - 92, clientY - bounds.top - 40));
    setNodes((current) => current.map((node) => (node.id === nodeId ? { ...node, x, y } : node)));
  }

  /** 选择连线起点或完成一条连接，判断节点强制写入是/否标签。 */
  function connectNode(nodeId: string) {
    // 未进入连线模式时，普通点击只切换节点检查器。
    if (!connectionSourceId) {
      setSelectedNodeId(nodeId);
      return;
    }
    if (connectionSourceId === nodeId) {
      setConnectionSourceId("");
      return;
    }

    // 相同起止节点只保留一条边；判断节点允许通过不同标签形成两条明确分支。
    const source = nodes.find((node) => node.id === connectionSourceId);
    const label = source?.type === "condition" ? branchLabel : "";
    const duplicate = edges.some((edge) => edge.source === connectionSourceId && edge.target === nodeId && edge.label === label);
    if (!duplicate) {
      setEdges((current) => [
        ...current,
        { id: `edge-${Date.now().toString(36)}`, source: connectionSourceId, target: nodeId, label, condition: label ? { branch: label } : {} },
      ]);
    }
    setConnectionSourceId("");
  }

  /** 删除选中节点以及引用该节点的全部连线。 */
  function removeSelectedNode() {
    // 开始和结束节点属于可运行画布的基本边界，不允许从检查器删除。
    if (!selectedNode || selectedNode.type === "start" || selectedNode.type === "end") return;
    setNodes((current) => current.filter((node) => node.id !== selectedNode.id));
    setEdges((current) => current.filter((edge) => edge.source !== selectedNode.id && edge.target !== selectedNode.id));
    setSelectedNodeId("");
  }

  /** 更新选中节点的标题或自然语言执行说明。 */
  function updateSelectedNode(patch: Partial<WorkflowNode>) {
    // 节点主键和类型不可在检查器中改写，避免已有连线失效。
    if (!selectedNode) return;
    setNodes((current) => current.map((node) => (node.id === selectedNode.id ? { ...node, ...patch } : node)));
  }

  /** 使用当前已保存版本创建一次手动运行。 */
  async function startRun() {
    // 运行固定服务端当前版本，本地尚未保存的画布不会混入执行记录。
    if (!detail) return;
    try {
      const created = await api<WorkflowRun>(`/api/workflows/${detail.id}/runs`, { method: "POST", body: JSON.stringify({ input: {} }) });
      await loadWorkflow(detail.id);
      setSelectedRunId(created.id);
      setActivePane("runs");
      setNotice("运行已进入队列");
    } catch (cause) {
      onError(cause instanceof Error ? cause.message : "工作流运行失败");
    }
  }

  /** 暂停、恢复或终止当前运行，并刷新运行列表与事件详情。 */
  async function controlRun(action: "pause" | "resume" | "terminate") {
    // 终止前进行显式确认，暂停和恢复保持可逆并直接提交。
    if (!detail || !runDetail || (action === "terminate" && !window.confirm("确认终止本次运行？"))) return;
    try {
      await api(`/api/workflow-runs/${runDetail.id}/${action}`, { method: "POST", body: "{}" });
      await loadWorkflow(detail.id);
      await loadRun(runDetail.id);
    } catch (cause) {
      onError(cause instanceof Error ? cause.message : "运行控制失败");
    }
  }

  /** 提交人工确认的布尔决定和评论，并继续读取该运行的最新事实。 */
  async function resolveApproval(approval: WorkflowApproval, decision: boolean) {
    // 评论可为空，但决定只能通过显式的通过或否决按钮产生。
    if (!detail || !runDetail || approval.status !== "pending") return;
    try {
      await api(`/api/approval-requests/${approval.id}/resolve`, {
        method: "POST",
        body: JSON.stringify({ decision, comment: approvalComments[approval.id] ?? "" }),
      });
      setApprovalComments((current) => ({ ...current, [approval.id]: "" }));
      await loadWorkflow(detail.id);
      await loadRun(runDetail.id);
      setNotice(decision ? "人工确认已通过" : "人工确认已否决");
    } catch (cause) {
      onError(cause instanceof Error ? cause.message : "人工确认提交失败");
    }
  }

  /** 根据事件名称提取节点执行态，兼容 Runner 后续追加更细事件。 */
  function nodeRunStatus(nodeId: string): string {
    // 同一节点取最后一条事件，事件命名未匹配时保持等待状态。
    const event = [...(runDetail?.events ?? [])].reverse().find((item) => item.node_key === nodeId);
    if (!event) return "等待";
    if (event.event_type.includes("failed")) return "失败";
    if (event.event_type.includes("completed") || event.event_type.includes("finished")) return "完成";
    if (event.event_type.includes("waiting") || event.event_type.includes("confirm")) return "待确认";
    if (event.event_type.includes("started") || event.event_type.includes("running")) return "执行中";
    return event.event_type;
  }

  // 画布高度随节点向下扩展，拖动和动态内容不会压缩外围布局。
  const canvasHeight = Math.max(540, ...nodes.map((node) => node.y + 130));

  return (
    <section className="view workflow-view">
      <aside className="panel workflow-list-panel">
        <div className="panel-head">
          <div>
            <small>{project.name}</small>
            <h3>工作流</h3>
          </div>
          <button className="icon-action" title="新建工作流" aria-label="新建工作流" onClick={() => void createWorkflow()}>＋</button>
        </div>
        <div className="workflow-list">
          {workflows.map((workflow) => (
            <button key={workflow.id} className={workflow.id === selectedId ? "workflow-row active" : "workflow-row"} onClick={() => setSelectedId(workflow.id)}>
              <span><b>{workflow.name}</b><small>v{workflow.current_version_no} · {workflow.status}</small></span>
              <span className="workflow-status-dot" />
            </button>
          ))}
          {!workflows.length && <p className="muted empty-state">暂无工作流</p>}
        </div>
      </aside>
      <main className="panel workflow-main-panel">
        {detail ? (
          <>
            <div className="workflow-titlebar">
              <div className="workflow-title-fields">
                <input aria-label="工作流名称" value={name} onChange={(event) => setName(event.target.value)} />
                <input aria-label="工作流说明" placeholder="工作流说明" value={description} onChange={(event) => setDescription(event.target.value)} />
              </div>
              <div className="workflow-title-actions">
                <div className="segmented-control" aria-label="工作流视图">
                  <button className={activePane === "canvas" ? "active" : ""} onClick={() => setActivePane("canvas")}>画布</button>
                  <button className={activePane === "schedules" ? "active" : ""} onClick={() => setActivePane("schedules")}>调度 {schedules.length}</button>
                  <button className={activePane === "runs" ? "active" : ""} onClick={() => setActivePane("runs")}>运行 {runs.length}</button>
                </div>
                {detail.status === "active" && (
                  <button className="icon-action" title="暂停自动化" aria-label="暂停自动化" onClick={() => void controlWorkflow("pause")}>Ⅱ</button>
                )}
                {detail.status === "paused" && (
                  <button className="icon-action" title="恢复自动化" aria-label="恢复自动化" onClick={() => void controlWorkflow("resume")}>▶</button>
                )}
                {detail.status !== "terminated" && (
                  <button className="danger-action" title="终止自动化" aria-label="终止自动化" onClick={() => void controlWorkflow("terminate")}>■</button>
                )}
                <button
                  className="icon-action"
                  title="运行工作流"
                  aria-label="运行工作流"
                  disabled={detail.status !== "active"}
                  onClick={() => void startRun()}
                >
                  ▷
                </button>
                <button className="primary-action" disabled={saving || detail.status === "terminated"} onClick={() => void saveWorkflow()}>
                  {saving ? "保存中" : "保存"}
                </button>
              </div>
            </div>
            {notice && <div className="notice-bar">{notice}</div>}
            {activePane === "canvas" ? (
              <div className="workflow-editor">
                <div className="workflow-toolbox">
                  {(Object.keys(workflowNodeLabels) as WorkflowNodeType[]).map((type) => (
                    <button key={type} title={`添加${workflowNodeLabels[type]}节点`} onClick={() => addNode(type)}>
                      <span>{workflowNodeIcons[type]}</span>{workflowNodeLabels[type]}
                    </button>
                  ))}
                  <span className="toolbox-divider" />
                  <button
                    className={connectionSourceId ? "active" : ""}
                    title={selectedNodeId ? "从选中节点开始连线" : "先选择一个节点"}
                    disabled={!selectedNodeId}
                    onClick={() => setConnectionSourceId((current) => (current ? "" : selectedNodeId))}
                  >
                    <span>↗</span>连线
                  </button>
                  {connectionSourceId && nodes.find((node) => node.id === connectionSourceId)?.type === "condition" && (
                    <div className="branch-control" aria-label="判断分支">
                      <button className={branchLabel === "是" ? "active" : ""} onClick={() => setBranchLabel("是")}>是</button>
                      <button className={branchLabel === "否" ? "active" : ""} onClick={() => setBranchLabel("否")}>否</button>
                    </div>
                  )}
                </div>
                <div className="workflow-canvas-scroll">
                  <div ref={canvasRef} className={connectionSourceId ? "workflow-canvas connecting" : "workflow-canvas"} style={{ height: canvasHeight }}>
                    <svg className="workflow-edges" width="100%" height={canvasHeight} aria-hidden="true">
                      <defs>
                        <marker id="workflow-arrow" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="6" markerHeight="6" orient="auto-start-reverse">
                          <path d="M 0 0 L 10 5 L 0 10 z" />
                        </marker>
                      </defs>
                      {edges.map((edge) => {
                        const source = nodes.find((node) => node.id === edge.source);
                        const target = nodes.find((node) => node.id === edge.target);
                        if (!source || !target) return null;
                        const forward = target.x >= source.x;
                        const x1 = forward ? source.x + 168 : source.x;
                        const y1 = source.y + 40;
                        const x2 = forward ? target.x : target.x + 168;
                        const y2 = target.y + 40;
                        const controlOffset = forward ? 70 : -70;
                        return (
                          <g key={edge.id} className="workflow-edge">
                            <path
                              d={`M ${x1} ${y1} C ${x1 + controlOffset} ${y1}, ${x2 - controlOffset} ${y2}, ${x2} ${y2}`}
                              markerEnd="url(#workflow-arrow)"
                            />
                            {edge.label && <text x={(x1 + x2) / 2} y={(y1 + y2) / 2 - 7}>{edge.label}</text>}
                            <circle
                              cx={(x1 + x2) / 2}
                              cy={(y1 + y2) / 2 + 8}
                              r="8"
                              onClick={() => setEdges((current) => current.filter((item) => item.id !== edge.id))}
                            />
                            <text
                              className="edge-remove"
                              x={(x1 + x2) / 2}
                              y={(y1 + y2) / 2 + 11}
                              onClick={() => setEdges((current) => current.filter((item) => item.id !== edge.id))}
                            >
                              ×
                            </text>
                          </g>
                        );
                      })}
                    </svg>
                    {nodes.map((node) => (
                      <button
                        key={node.id}
                        className={
                          `workflow-node node-${node.type} ${node.id === selectedNodeId ? "selected" : ""} ` +
                          `${node.id === connectionSourceId ? "connection-source" : ""}`
                        }
                        style={{ left: node.x, top: node.y }}
                        draggable
                        onDragEnd={(event) => moveNode(node.id, event.clientX, event.clientY)}
                        onClick={() => connectNode(node.id)}
                      >
                        <span className="workflow-node-icon">{workflowNodeIcons[node.type]}</span>
                        <span><small>{workflowNodeLabels[node.type]}</small><b>{node.label}</b></span>
                      </button>
                    ))}
                  </div>
                </div>
                <aside className="node-inspector">
                  <div className="panel-head"><h3>节点</h3>{selectedNode && <small>{selectedNode.id}</small>}</div>
                  {selectedNode ? (
                    <div className="node-fields">
                      <label>类型<input value={workflowNodeLabels[selectedNode.type]} disabled /></label>
                      <label>名称<input value={selectedNode.label} onChange={(event) => updateSelectedNode({ label: event.target.value })} /></label>
                      <label>
                        节点说明
                        <textarea
                          value={String(selectedNode.config.instruction ?? "")}
                          onChange={(event) => updateSelectedNode({ config: { ...selectedNode.config, instruction: event.target.value } })}
                        />
                      </label>
                      <button
                        className="danger-action"
                        disabled={selectedNode.type === "start" || selectedNode.type === "end"}
                        onClick={removeSelectedNode}
                      >
                        删除节点
                      </button>
                    </div>
                  ) : <p className="muted empty-state">选择节点</p>}
                </aside>
              </div>
            ) : activePane === "schedules" ? (
              <div className="workflow-schedules-layout">
                <aside className="schedule-form-panel">
                  <div className="panel-head">
                    <div><small>{editingScheduleId ? "修改现有规则" : "新触发规则"}</small><h3>{editingScheduleId ? "编辑调度" : "新建调度"}</h3></div>
                    {editingScheduleId && <button className="icon-action" title="取消编辑" onClick={resetScheduleForm}>×</button>}
                  </div>
                  <div className="schedule-form">
                    <label>
                      类型
                      <select value={scheduleType} onChange={(event) => setScheduleType(event.target.value as WorkflowSchedule["schedule_type"])}>
                        <option value="periodic">周期性重复</option>
                        <option value="scheduled">预定时间</option>
                        <option value="ai_parsed">AI 解析规则</option>
                      </select>
                    </label>
                    <label>
                      规则描述
                      <textarea
                        value={scheduleExpression}
                        onChange={(event) => setScheduleExpression(event.target.value)}
                        placeholder={
                          scheduleType === "periodic"
                            ? "例如：每周一 09:00"
                            : scheduleType === "scheduled" ? "例如：2026-08-20 14:30" : "例如：每个工作日下班前执行"
                        }
                      />
                    </label>
                    <label>
                      时区
                      <input value={scheduleTimezone} onChange={(event) => setScheduleTimezone(event.target.value)} />
                    </label>
                    <label>
                      下次执行时间
                      <input type="datetime-local" value={scheduleNextRunAt} onChange={(event) => setScheduleNextRunAt(event.target.value)} />
                    </label>
                    <label>
                      结构化规则 JSON
                      <textarea className="schedule-rule-input" value={scheduleRule} onChange={(event) => setScheduleRule(event.target.value)} />
                    </label>
                    <button className="primary-action" disabled={!scheduleExpression.trim() || detail.status === "terminated"} onClick={() => void saveSchedule()}>
                      {editingScheduleId ? "保存修改" : "创建调度"}
                    </button>
                  </div>
                </aside>
                <div className="schedule-list-panel">
                  <div className="panel-head"><div><small>按工作流保存</small><h3>调度规则</h3></div><span className="count">{schedules.length}</span></div>
                  <div className="schedule-list">
                    {schedules.map((schedule) => (
                      <article className="schedule-card" key={schedule.id}>
                        <div className="schedule-card-head">
                          <span><small>{schedule.schedule_type} · {schedule.timezone}</small><b>{schedule.schedule_expression}</b></span>
                          <span className={schedule.enabled ? "schedule-enabled" : "schedule-disabled"}>{schedule.enabled ? "已启用" : "未启用"}</span>
                        </div>
                        <div className="schedule-facts">
                          <small>下次执行：{schedule.next_run_at ? new Date(schedule.next_run_at).toLocaleString() : "等待调度器计算"}</small>
                          <pre>{JSON.stringify(schedule.parsed_rule, null, 2)}</pre>
                        </div>
                        <div className="schedule-actions">
                          <button title="编辑调度" onClick={() => editSchedule(schedule)}>编辑</button>
                          <button title={schedule.enabled ? "停用调度" : "启用调度"} onClick={() => void toggleSchedule(schedule)}>
                            {schedule.enabled ? "停用" : "启用"}
                          </button>
                          <button className="danger-action" title="删除调度" onClick={() => void removeSchedule(schedule)}>删除</button>
                        </div>
                      </article>
                    ))}
                    {!schedules.length && <p className="muted empty-state">暂无调度规则</p>}
                  </div>
                </div>
              </div>
            ) : (
              <div className="workflow-runs-layout">
                <aside className="workflow-run-list">
                  {runs.map((run) => (
                    <button key={run.id} className={run.id === selectedRunId ? "workflow-run-row active" : "workflow-run-row"} onClick={() => setSelectedRunId(run.id)}>
                      <span><b>{run.status}</b><small>{new Date(run.created_at).toLocaleString()}</small></span>
                      <small>{run.trigger_type}</small>
                    </button>
                  ))}
                  {!runs.length && <p className="muted empty-state">暂无运行记录</p>}
                </aside>
                <div className="workflow-run-detail">
                  {runDetail ? (
                    <>
                      <div className="run-detail-head">
                        <div><small>{runDetail.id}</small><h2>{runDetail.status}</h2></div>
                        <div className="run-controls">
                          {(runDetail.status === "queued" || runDetail.status === "running" || runDetail.status === "waiting_child") && (
                            <button title="暂停" aria-label="暂停" onClick={() => void controlRun("pause")}>Ⅱ</button>
                          )}
                          {runDetail.status === "paused" && <button title="恢复" aria-label="恢复" onClick={() => void controlRun("resume")}>▶</button>}
                          {["queued", "running", "waiting_child", "paused"].includes(runDetail.status) && (
                            <button className="danger-action" title="终止" aria-label="终止" onClick={() => void controlRun("terminate")}>■</button>
                          )}
                          <button title="刷新" aria-label="刷新" onClick={() => void loadRun(runDetail.id)}>↻</button>
                        </div>
                      </div>
                      {!!approvals.length && (
                        <section className="run-approvals">
                          <div className="subsection-head"><h3>人工确认</h3><span className="count">{approvals.length}</span></div>
                          {approvals.map((approval) => (
                            <article className={approval.status === "pending" ? "approval-card pending" : "approval-card"} key={approval.id}>
                              <div className="approval-head">
                                <span><small>{approval.request_type}</small><b>{approval.prompt}</b></span>
                                <span className="approval-status">{approval.status === "pending" ? "待确认" : "已处理"}</span>
                              </div>
                              {approval.status === "pending" ? (
                                <>
                                  <textarea
                                    value={approvalComments[approval.id] ?? ""}
                                    onChange={(event) => setApprovalComments((current) => ({ ...current, [approval.id]: event.target.value }))}
                                    placeholder="填写确认意见或补充说明"
                                  />
                                  <div className="approval-actions">
                                    <button className="danger-action" onClick={() => void resolveApproval(approval, false)}>否决</button>
                                    <button className="primary-action" onClick={() => void resolveApproval(approval, true)}>通过</button>
                                  </div>
                                </>
                              ) : (
                                <div className="approval-response">
                                  <small>
                                    {approval.resolved_by ?? "human"} · {approval.resolved_at ? new Date(approval.resolved_at).toLocaleString() : "已处理"}
                                  </small>
                                  <pre>{JSON.stringify(approval.response_data, null, 2)}</pre>
                                </div>
                              )}
                            </article>
                          ))}
                        </section>
                      )}
                      <div className="run-node-grid">
                        {nodes.map((node) => (
                          <div className="run-node-card" key={node.id}>
                            <span className={`run-state state-${nodeRunStatus(node.id)}`}>{nodeRunStatus(node.id)}</span>
                            <small>{workflowNodeLabels[node.type]}</small>
                            <b>{node.label}</b>
                            {(runDetail.events ?? []).filter((event) => event.node_key === node.id).map((event) => (
                              <div className="run-event-output" key={event.id}><small>{event.event_type}</small><pre>{JSON.stringify(event.payload, null, 2)}</pre></div>
                            ))}
                          </div>
                        ))}
                      </div>
                      {!!outputs.length && (
                        <section className="run-outputs">
                          <div className="subsection-head"><h3>执行输出</h3><span className="count">{outputs.length}</span></div>
                          <div className="run-output-list">
                            {outputs.map((output) => (
                              <article className="run-output-card" key={output.id}>
                                <div>
                                  <b>{output.output_type}</b>
                                  <small>
                                    {output.node_run_id
                                      ? `节点 ${output.node_run_id}`
                                      : output.task_id ? `任务 ${output.task_id}` : `作业 ${output.job_id}`}
                                  </small>
                                </div>
                                <pre>{output.content}</pre>
                                <small>{new Date(output.created_at).toLocaleString()}</small>
                              </article>
                            ))}
                          </div>
                        </section>
                      )}
                      {(runDetail.output != null || runDetail.error_message) && (
                        <div className="run-final-output">
                          <h3>{runDetail.error_message ? "错误" : "运行输出"}</h3>
                          <pre>{runDetail.error_message ?? JSON.stringify(runDetail.output, null, 2)}</pre>
                        </div>
                      )}
                      <div className="run-timeline">
                        {runDetail.events.map((event) => (
                          <div className="run-timeline-row" key={event.id}>
                            <span />
                            <div><b>{event.event_type}</b><small>{event.node_key ?? "运行"} · {new Date(event.created_at).toLocaleString()}</small></div>
                          </div>
                        ))}
                      </div>
                    </>
                  ) : <p className="muted empty-state">选择运行记录</p>}
                </div>
              </div>
            )}
          </>
        ) : <p className="muted empty-state">新建或选择工作流</p>}
      </main>
    </section>
  );
}

/** 新对话模块：仅创建 Human 与一个 Agent 的一对一会话。 */
function DirectChat({ onError }: { onError: (message: string) => void }) {
  const [agents, setAgents] = useState<Agent[]>([]);
  const [conversations, setConversations] = useState<Conversation[]>([]);
  const [selectedId, setSelectedId] = useState("");

  /** 加载可用 Agent 和私聊。 */
  async function load() {
    // 私聊与项目群聊按类型隔离。
    const [agentResult, conversationResult] = await Promise.all([
      api<{ items: Agent[] }>("/api/agents?status=active"),
      api<{ items: Conversation[] }>("/api/conversations?conversation_type=direct"),
    ]);
    setAgents(agentResult.items);
    setConversations(conversationResult.items);
    setSelectedId((current) => (conversationResult.items.some((item) => item.id === current) ? current : (conversationResult.items[0]?.id ?? "")));
  }
  useEffect(() => {
    void load().catch((cause) => onError(cause instanceof Error ? cause.message : "对话加载失败"));
  }, []);

  /** 选择 Agent 发起私聊。 */
  async function start() {
    // 使用选择框外的快速提示输入 Agent ID，后端确保恰好两个参与者。
    const agentId = window.prompt(agents.map((agent) => `${agent.name}: ${agent.id}`).join("\n"));
    const agent = agents.find((item) => item.id === agentId?.trim());
    if (!agent) return;
    const created = await api<Conversation>("/api/conversations", {
      method: "POST",
      body: JSON.stringify({ conversation_type: "direct", agent_id: agent.id, title: `与 ${agent.name} 的对话` }),
    });
    await load();
    setSelectedId(created.id);
  }

  return (
    <section className="view chat-view">
      <aside className="panel conversation-list">
        <div className="panel-head">
          <div>
            <small>Human · Agent</small>
            <h2>对话</h2>
          </div>
          <button className="icon-action" onClick={() => void start()}>
            ＋
          </button>
        </div>
        {conversations.map((item) => (
          <button key={item.id} className={item.id === selectedId ? "conversation-row active" : "conversation-row"} onClick={() => setSelectedId(item.id)}>
            <b>{item.title}</b>
            <small>{item.status}</small>
          </button>
        ))}
      </aside>
      {selectedId ? <ConversationPanel conversationId={selectedId} onChanged={load} /> : <div className="panel empty-chat">选择 Agent 发起一对一对话</div>}
    </section>
  );
}

/** 项目空间：展示固定 Agent、主群聊和临时群聊。 */
function ProjectSpace({ project, onReload, onError }: { project: Project; onReload: () => Promise<void>; onError: (message: string) => void }) {
  const [members, setMembers] = useState<ProjectAgent[]>([]);
  const [agents, setAgents] = useState<Agent[]>([]);
  const [conversations, setConversations] = useState<Conversation[]>([]);
  const [selectedId, setSelectedId] = useState("");
  const [projectMode, setProjectMode] = useState<"collaboration" | "documents">("collaboration");

  /** 加载项目协作关系。 */
  async function load() {
    // 成员、全局 Agent 和对话并行读取。
    const [memberResult, agentResult, conversationResult] = await Promise.all([
      api<{ items: ProjectAgent[] }>(`/api/projects/${project.id}/agents`),
      api<{ items: Agent[] }>("/api/agents?status=active"),
      api<{ items: Conversation[] }>(`/api/conversations?project_id=${project.id}`),
    ]);
    setMembers(memberResult.items);
    setAgents(agentResult.items);
    setConversations(conversationResult.items);
    setSelectedId((current) =>
      conversationResult.items.some((item) => item.id === current)
        ? current
        : (conversationResult.items.find((item) => item.conversation_type === "project_main")?.id ?? ""),
    );
  }
  useEffect(() => {
    void load().catch((cause) => onError(cause instanceof Error ? cause.message : "项目空间加载失败"));
  }, [project.id]);

  /** 添加项目固定 Agent。 */
  async function addMember() {
    // Human 维护长期成员，具体任务指派仍由协调 Agent 负责。
    const agentId = window.prompt(agents.map((agent) => `${agent.name}: ${agent.id}`).join("\n"));
    if (!agentId?.trim()) return;
    await api(`/api/projects/${project.id}/agents`, { method: "POST", body: JSON.stringify({ agent_id: agentId.trim(), assignment_type: "fixed" }) });
    await load();
  }

  /** 创建项目临时群聊。 */
  async function createGroup() {
    // 默认邀请全部在岗项目 Agent，后续消息可关联多个任务。
    const title = window.prompt("临时群聊名称");
    if (!title?.trim()) return;
    const created = await api<Conversation>("/api/conversations", {
      method: "POST",
      body: JSON.stringify({
        conversation_type: "project_temporary",
        project_id: project.id,
        title: title.trim(),
        agent_ids: members.filter((item) => item.assignment_status === "active").map((item) => item.agent_id),
      }),
    });
    await load();
    setSelectedId(created.id);
  }

  return (
    <section className="view project-space">
      <div className="project-summary">
        <div>
          <small>项目空间</small>
          <h2>{project.name}</h2>
          <p>{project.description || "暂无项目说明"}</p>
        </div>
        <div className="project-stat">
          <b>{members.length}</b>
          <small>项目 Agent</small>
        </div>
        <div className="project-stat">
          <b>{conversations.length}</b>
          <small>项目对话</small>
        </div>
      </div>
      <div className="project-mode-switch" role="tablist" aria-label="项目空间视图">
        <button className={projectMode === "collaboration" ? "active" : ""} onClick={() => setProjectMode("collaboration")}>协作</button>
        <button className={projectMode === "documents" ? "active" : ""} onClick={() => setProjectMode("documents")}>项目文档</button>
      </div>
      {projectMode === "collaboration" ? (
        <div className="project-grid">
          <aside className="panel project-members">
            <div className="panel-head">
              <h3>项目 Agent</h3>
              <button className="icon-action" title="添加项目 Agent" onClick={() => void addMember()}>＋</button>
            </div>
            {members.map((member) => (
              <div className="member-row" key={member.agent_id}>
                <span className="agent-avatar">{member.name.slice(0, 1)}</span>
                <span>
                  <b>{member.name}</b>
                  <small>{member.assignment_type === "coordinator" ? "协调 Agent" : "固定 Agent"}</small>
                </span>
              </div>
            ))}
          </aside>
          <aside className="panel conversation-list">
            <div className="panel-head">
              <h3>项目对话</h3>
              <button className="icon-action" title="创建临时群聊" onClick={() => void createGroup()}>＋</button>
            </div>
            {conversations.map((item) => (
              <button key={item.id} className={item.id === selectedId ? "conversation-row active" : "conversation-row"} onClick={() => setSelectedId(item.id)}>
                <b>{item.title}</b>
                <small>{item.conversation_type === "project_main" ? "主群聊" : "临时群聊"} · {item.status}</small>
              </button>
            ))}
          </aside>
          {selectedId && (
            <ConversationPanel
              conversationId={selectedId}
              project={project}
              onChanged={async () => {
                // 项目对话变更后同步对话目录与全局任务聚合。
                await load();
                await onReload();
              }}
            />
          )}
        </div>
      ) : (
        <ProjectDocuments project={project} onError={onError} />
      )}
    </section>
  );
}

/** 项目文档工作区：维护章节事实、版本历史、差异、回退和候选变更。 */
function ProjectDocuments({ project, onError }: { project: Project; onError: (message: string) => void }) {
  const [documents, setDocuments] = useState<ProjectDocumentSummary[]>([]);
  const [selectedDocumentId, setSelectedDocumentId] = useState("");
  const [document, setDocument] = useState<ProjectDocument>();
  const [versions, setVersions] = useState<DocumentVersion[]>([]);
  const [candidates, setCandidates] = useState<DocumentCandidate[]>([]);
  const [drafts, setDrafts] = useState<Record<string, string>>({});
  const [diffFrom, setDiffFrom] = useState("");
  const [diffTo, setDiffTo] = useState("");
  const [diff, setDiff] = useState<DocumentDiff>();
  const [notice, setNotice] = useState("");

  /** 加载项目文档目录并保留有效选择。 */
  async function loadDocuments() {
    // 项目切换时目录是选择源，空目录不伪造默认文档。
    const result = await api<{ items: ProjectDocumentSummary[] }>(`/api/projects/${project.id}/documents`);
    setDocuments(result.items);
    setSelectedDocumentId((current) => (result.items.some((item) => item.id === current) ? current : (result.items[0]?.id ?? "")));
  }

  /** 加载当前文档、不可变版本和候选变更。 */
  async function loadDocument(documentId: string) {
    // 当前详情、不可变版本与候选处置状态独立读取，合并等待后一次性更新界面。
    const [detail, versionResult, candidateResult] = await Promise.all([
      api<ProjectDocument>(`/api/documents/${documentId}`),
      api<{ items: DocumentVersion[] }>(`/api/documents/${documentId}/versions`),
      api<{ items: DocumentCandidate[] }>(`/api/documents/${documentId}/candidates`),
    ]);

    // 所有远端事实成功读取后一次性替换本地视图和编辑草稿。
    setDocument(detail);
    setVersions(versionResult.items);
    setCandidates(candidateResult.items);
    setDrafts(Object.fromEntries(detail.sections.map((section) => [section.section_key, section.content])));
    setDiff(undefined);
    setDiffFrom((current) => (versionResult.items.some((item) => String(item.version_no) === current) ? current : String(versionResult.items[1]?.version_no ?? "")));
    setDiffTo((current) => (versionResult.items.some((item) => String(item.version_no) === current) ? current : String(versionResult.items[0]?.version_no ?? "")));
  }

  useEffect(() => {
    // 项目切换后重新建立文档目录，不沿用其他项目的选择。
    setSelectedDocumentId("");
    setDocument(undefined);
    void loadDocuments().catch((cause) => onError(cause instanceof Error ? cause.message : "项目文档加载失败"));
  }, [project.id]);
  useEffect(() => {
    // 文档选择变化时同步完整内容和审计信息。
    if (!selectedDocumentId) return;
    void loadDocument(selectedDocumentId).catch((cause) => onError(cause instanceof Error ? cause.message : "文档详情加载失败"));
  }, [selectedDocumentId]);

  /** 保存 Human 对章节正文的显式编辑。 */
  async function saveSection(section: DocumentSection) {
    // 详情缺失或正文未改变时不生成无意义版本。
    if (!document) return;
    const content = drafts[section.section_key] ?? "";
    if (content === section.content) return;

    // 服务端更新成功后再替换本地事实，避免并发冲突时展示未提交内容。
    try {
      await api(`/api/documents/${document.id}/sections/${encodeURIComponent(section.section_key)}`, { method: "PATCH", body: JSON.stringify({ content }) });
      setNotice(`${section.title} 已保存`);
      await loadDocument(document.id);
      await loadDocuments();
    } catch (cause) {
      onError(cause instanceof Error ? cause.message : "章节保存失败");
    }
  }

  /** 锁定或解锁章节，控制自动刷新是否可以直接覆盖。 */
  async function toggleSectionLock(section: DocumentSection) {
    // 详情缺失时不发请求；锁定状态由服务端写入版本，前端不做乐观覆盖。
    if (!document) return;
    try {
      await api(`/api/documents/${document.id}/sections/${encodeURIComponent(section.section_key)}`, {
        method: "PATCH",
        body: JSON.stringify({ locked_by_human: !section.locked_by_human }),
      });
      setNotice(section.locked_by_human ? `${section.title} 已解锁` : `${section.title} 已锁定`);
      await loadDocument(document.id);
      await loadDocuments();
    } catch (cause) {
      onError(cause instanceof Error ? cause.message : "章节锁定更新失败");
    }
  }

  /** 手动请求后台刷新当前文档。 */
  async function refreshDocument() {
    // 刷新为异步作业，成功只表示已入队，不伪装为内容已更新。
    if (!document) return;
    try {
      await api(`/api/documents/${document.id}/refresh`, { method: "POST", body: "{}" });
      setNotice("文档刷新已进入队列");
    } catch (cause) {
      onError(cause instanceof Error ? cause.message : "文档刷新失败");
    }
  }

  /** 比较两个不可变版本并展示章节级差异。 */
  async function compareVersions() {
    // 版本必须完整且不同，避免服务端返回无意义差异。
    if (!document || !diffFrom || !diffTo || diffFrom === diffTo) return;
    try {
      const result = await api<DocumentDiff>(`/api/documents/${document.id}/diff?from=${encodeURIComponent(diffFrom)}&to=${encodeURIComponent(diffTo)}`);
      setDiff(result);
    } catch (cause) {
      onError(cause instanceof Error ? cause.message : "版本对比失败");
    }
  }

  /** 将历史版本恢复为新的当前版本。 */
  async function rollback(versionNo: number) {
    // 二次确认明确回退会生成新版本，不删除后续审计历史。
    if (!document || !window.confirm(`将版本 v${versionNo} 恢复为新的当前版本？`)) return;
    try {
      await api(`/api/documents/${document.id}/rollback`, { method: "POST", body: JSON.stringify({ version_no: versionNo }) });
      setNotice(`已从 v${versionNo} 创建回退版本`);
      await loadDocument(document.id);
      await loadDocuments();
    } catch (cause) {
      onError(cause instanceof Error ? cause.message : "版本回退失败");
    }
  }

  /** 接受或拒绝一条待处理候选变更。 */
  async function resolveCandidate(candidate: DocumentCandidate, action: "accept" | "reject") {
    // 候选处置由专用接口执行，锁定冲突和过期基线仍以服务端判断为准。
    if (!document) return;
    try {
      await api(`/api/document-candidates/${candidate.id}/resolve`, { method: "POST", body: JSON.stringify({ action }) });
      setNotice(action === "accept" ? "候选变更已采用" : "候选变更已拒绝");
      await loadDocument(document.id);
      await loadDocuments();
    } catch (cause) {
      onError(cause instanceof Error ? cause.message : "候选变更处置失败");
    }
  }

  return (
    <div className="project-documents">
      <aside className="panel document-list-panel">
        <div className="panel-head">
          <div>
            <small>项目上下文</small>
            <h3>文档</h3>
          </div>
          <span className="count">{documents.length}</span>
        </div>
        <div className="document-list">
          {documents.map((item) => (
            <button key={item.id} className={item.id === selectedDocumentId ? "document-row active" : "document-row"} onClick={() => setSelectedDocumentId(item.id)}>
              <span>
                <b>{item.title}</b>
                <small>v{item.current_version_no} · {item.section_count} 个章节</small>
              </span>
              {!!item.pending_candidate_count && <span className="candidate-count">{item.pending_candidate_count}</span>}
            </button>
          ))}
          {!documents.length && <p className="muted empty-state">暂无项目文档</p>}
        </div>
      </aside>
      <main className="panel document-content-panel">
        {document ? (
          <>
            <div className="document-head">
              <div>
                <small>{document.doc_type} · 当前 v{document.current_version_no}</small>
                <h2>{document.title}</h2>
                <p>更新于 {new Date(document.updated_at).toLocaleString()}</p>
              </div>
              <button className="icon-action" title="请求后台刷新" aria-label="请求后台刷新" onClick={() => void refreshDocument()}>↻</button>
            </div>
            {notice && <div className="notice-bar">{notice}</div>}
            <div className="document-sections">
              {document.sections.map((section) => (
                <section className="document-section" key={section.section_key}>
                  <div className="document-section-head">
                    <div>
                      <small>{section.section_key} · r{section.revision}</small>
                      <h3>{section.title}</h3>
                    </div>
                    <button className={section.locked_by_human ? "lock-action locked" : "lock-action"} onClick={() => void toggleSectionLock(section)}>
                      {section.locked_by_human ? "▣ 解锁" : "□ 锁定"}
                    </button>
                  </div>
                  <textarea
                    value={drafts[section.section_key] ?? ""}
                    onChange={(event) => setDrafts((current) => ({ ...current, [section.section_key]: event.target.value }))}
                  />
                  <div className="section-actions">
                    <small>{section.locked_by_human ? "Human 已锁定，自动刷新将生成候选变更" : "自动刷新可更新此章节"}</small>
                    <button disabled={(drafts[section.section_key] ?? "") === section.content} onClick={() => void saveSection(section)}>保存</button>
                  </div>
                </section>
              ))}
            </div>
            <section className="candidate-section">
              <div className="subsection-head">
                <h3>候选变更</h3>
                <span className="count">{candidates.filter((item) => item.status === "pending" || item.status === "conflict").length}</span>
              </div>
              {candidates.map((candidate) => (
                <div className="candidate-row" key={candidate.id}>
                  <div className="candidate-meta">
                    <b>{document.sections.find((section) => section.section_key === candidate.section_key)?.title ?? candidate.section_key}</b>
                    <small>{candidate.source_type} · 基于 r{candidate.base_section_revision} · {candidate.status}</small>
                  </div>
                  {candidate.conflict_reason && <p className="candidate-conflict">{candidate.conflict_reason}</p>}
                  <p>{candidate.proposed_content}</p>
                  {(candidate.status === "pending" || candidate.status === "conflict") && (
                    <div className="candidate-actions">
                      <button onClick={() => void resolveCandidate(candidate, "reject")}>拒绝</button>
                      <button className="primary-inline" onClick={() => void resolveCandidate(candidate, "accept")}>采用</button>
                    </div>
                  )}
                </div>
              ))}
              {!candidates.length && <p className="muted empty-state">暂无候选变更</p>}
            </section>
          </>
        ) : (
          <p className="muted empty-state">选择文档查看章节</p>
        )}
      </main>
      <aside className="panel document-history-panel">
        <div className="panel-head">
          <div>
            <small>不可变快照</small>
            <h3>版本历史</h3>
          </div>
          <span className="count">{versions.length}</span>
        </div>
        <div className="diff-controls">
          <label>
            从
            <select value={diffFrom} onChange={(event) => setDiffFrom(event.target.value)}>
              <option value="">选择</option>
              {versions.map((item) => <option key={item.id} value={item.version_no}>v{item.version_no}</option>)}
            </select>
          </label>
          <label>
            到
            <select value={diffTo} onChange={(event) => setDiffTo(event.target.value)}>
              <option value="">选择</option>
              {versions.map((item) => <option key={item.id} value={item.version_no}>v{item.version_no}</option>)}
            </select>
          </label>
          <button disabled={!diffFrom || !diffTo || diffFrom === diffTo} onClick={() => void compareVersions()}>比较</button>
        </div>
        {diff && (
          <div className="diff-result">
            <small>v{diff.from} → v{diff.to}</small>
            {diff.changes.map((change) => (
              <div className="diff-change" key={change.section_key}>
                <b>{change.section_key} · {change.change_type}</b>
                {change.before && <p className="diff-before">− {change.before.content}</p>}
                {change.after && <p className="diff-after">＋ {change.after.content}</p>}
              </div>
            ))}
            {!diff.changes.length && <p className="muted">所选版本无章节变化</p>}
          </div>
        )}
        <div className="version-list">
          {versions.map((version) => (
            <div className="version-row" key={version.id}>
              <div>
                <b>v{version.version_no}</b>
                <small>{version.source_type} · {new Date(version.created_at).toLocaleString()}</small>
              </div>
              <button disabled={version.version_no === document?.current_version_no} title="回退到此版本" onClick={() => void rollback(version.version_no)}>
                ↶
              </button>
            </div>
          ))}
        </div>
      </aside>
    </div>
  );
}

/** 对话面板：发送消息、显式创建任务并归档临时群聊。 */
function ConversationPanel({ conversationId, project, onChanged }: { conversationId: string; project?: Project; onChanged: () => Promise<void> }) {
  const [conversation, setConversation] = useState<Conversation>();
  const [messages, setMessages] = useState<Message[]>([]);
  const [content, setContent] = useState("");

  /** 加载对话主体和消息。 */
  async function load() {
    // 归档对话仍完整返回消息历史。
    const [detail, result] = await Promise.all([
      api<Conversation>(`/api/conversations/${conversationId}`),
      api<{ items: Message[] }>(`/api/conversations/${conversationId}/messages`),
    ]);
    setConversation(detail);
    setMessages(result.items);
  }
  useEffect(() => {
    void load();
  }, [conversationId]);

  /** 发送 Human 普通消息。 */
  async function send() {
    // 普通消息只记录事实，不隐式创建任务。
    if (!content.trim() || conversation?.status !== "active") return;
    await api(`/api/conversations/${conversationId}/messages`, { method: "POST", body: JSON.stringify({ content: content.trim() }) });
    setContent("");
    await load();
  }

  /** 从项目对话显式创建 Backlog 任务。 */
  async function createTask() {
    // 默认开启方案确认，任务与对话在后端同一事务关联。
    const title = window.prompt("将对话内容转为任务");
    if (!title?.trim()) return;
    await api(`/api/conversations/${conversationId}/tasks`, {
      method: "POST",
      body: JSON.stringify({ title: title.trim(), description: "来自项目群聊", requires_plan_confirmation: true }),
    });
    await load();
    await onChanged();
  }

  /** 归档临时群聊并触发总结。 */
  async function archive() {
    // 主群聊不提供归档，临时群聊归档期间停止接收消息。
    await api(`/api/conversations/${conversationId}/archive`, { method: "POST" });
    await load();
    await onChanged();
  }

  return (
    <div className="panel conversation-panel">
      <div className="conversation-head">
        <div>
          <small>
            {conversation?.conversation_type} · {conversation?.status}
          </small>
          <h2>{conversation?.title ?? "加载中"}</h2>
        </div>
        <div className="actions">
          {project && conversation?.status === "active" && <button onClick={() => void createTask()}>创建任务</button>}
          {conversation?.conversation_type === "project_temporary" && conversation.status === "active" && <button onClick={() => void archive()}>归档总结</button>}
        </div>
      </div>
      <div className="message-list">
        {messages.map((message) => (
          <div className={`message ${message.author_type}`} key={message.id}>
            <small>
              {message.author_type === "human" ? "Human" : message.author_id} · {message.message_type}
            </small>
            <p>{message.content}</p>
            {message.task_id && <span>任务 {message.task_id.slice(0, 8)}</span>}
          </div>
        ))}
        {!messages.length && <p className="muted">暂无消息</p>}
      </div>
      <div className="message-input">
        <textarea value={content} disabled={conversation?.status !== "active"} onChange={(event) => setContent(event.target.value)} placeholder="输入消息" />
        <button disabled={conversation?.status !== "active"} onClick={() => void send()}>
          发送
        </button>
      </div>
    </div>
  );
}

/** 项目设置中的 Git/worktree 面板：管理开关并展示服务端创建的 detached worktree 会话。 */
function GitSettings({ project, tasks, onError }: { project: Project; tasks: TaskCard[]; onError: (message: string) => void }) {
  // 服务端状态与会话记录分别保存，避免客户端推测或模拟 Git 状态。
  const [config, setConfig] = useState<ProjectGitConfig>();
  const [sessions, setSessions] = useState<GitWorktreeSession[]>([]);
  const [selectedTaskId, setSelectedTaskId] = useState("");
  const [loading, setLoading] = useState(true);

  // 独立提交态防止重复启停、重复创建及重复清理同一 worktree。
  const [savingEnabled, setSavingEnabled] = useState(false);
  const [creating, setCreating] = useState(false);
  const [removingSessionId, setRemovingSessionId] = useState("");
  const requestVersion = useRef(0);

  /** 获取项目 Git 配置与 worktree 会话，仅提交当前最新请求的结果。 */
  async function load() {
    const requestId = ++requestVersion.current;
    setLoading(true);

    try {
      const [nextConfig, nextSessions] = await Promise.all([
        api<ProjectGitConfig>(`/api/projects/${project.id}/git`),
        api<{ items: GitWorktreeSession[] }>(`/api/projects/${project.id}/git/sessions`),
      ]);

      // 项目切换或手动刷新后的旧响应不能覆盖较新的服务端快照。
      if (requestId !== requestVersion.current) return;

      setConfig(nextConfig);
      setSessions(nextSessions.items);
    } finally {
      // 仅由当前请求结束加载态，避免较早请求提前解除禁用状态。
      if (requestId === requestVersion.current) setLoading(false);
    }
  }

  useEffect(() => {
    // 切换项目时清除旧任务关联，并从服务端重新读取实际 Git 状态。
    setSelectedTaskId("");
    void load().catch((cause) => onError(cause instanceof Error ? cause.message : "Git 设置加载失败"));

    // 组件卸载后使未完成请求失效，防止异步响应更新离开的页面。
    return () => {
      requestVersion.current += 1;
    };
  }, [project.id]);

  /** 切换服务端 Git 开关，成功后重新读取最终配置而非乐观模拟结果。 */
  async function updateEnabled(enabled: boolean) {
    if (savingEnabled || loading) return;
    setSavingEnabled(true);

    try {
      await api<unknown>(`/api/projects/${project.id}/git`, { method: "PATCH", body: JSON.stringify({ enabled }) });
      await load();
    } catch (cause) {
      onError(cause instanceof Error ? cause.message : "Git 开关更新失败");
    } finally {
      setSavingEnabled(false);
    }
  }

  /** 通过服务端创建 detached worktree，可选关联当前项目中的任务。 */
  async function createSession() {
    if (!config?.enabled || creating || loading) return;
    setCreating(true);

    try {
      await api<unknown>(`/api/projects/${project.id}/git/sessions`, {
        method: "POST",
        body: JSON.stringify({ task_id: selectedTaskId || undefined }),
      });
      await load();
    } catch (cause) {
      onError(cause instanceof Error ? cause.message : "创建 detached worktree 失败");
    } finally {
      setCreating(false);
    }
  }

  /** 显式清理指定会话对应的 worktree，避免删除操作因误触发而执行。 */
  async function removeSession(session: GitWorktreeSession) {
    if (removingSessionId || !window.confirm(`确认清理 worktree 会话 ${session.id} 吗？`)) return;
    setRemovingSessionId(session.id);

    try {
      await api<unknown>(`/api/git-worktree-sessions/${session.id}`, { method: "DELETE" });
      await load();
    } catch (cause) {
      onError(cause instanceof Error ? cause.message : "清理 worktree 失败");
    } finally {
      setRemovingSessionId("");
    }
  }

  // 使用本地任务标题补充服务端 task_id，关联信息不写回也不推断任务状态。
  const selectedTask = tasks.find((task) => task.id === selectedTaskId);
  const isMutating = savingEnabled || creating || Boolean(removingSessionId);
  const availability = config?.repository_available === true ? "仓库可用" : config?.repository_available === false ? "仓库不可用" : "仓库状态未提供";

  return (
    <section className="git-settings view">
      <div className="view-head">
        <div>
          <small>项目设置</small>
          <h2>Git worktree</h2>
        </div>
      </div>

      {/* Git 功能默认由服务端关闭，开关不会执行 init、分支或提交操作。 */}
      <section className="panel git-config-panel">
        <div className="git-config-head">
          <div>
            <h3>可选 Git 隔离</h3>
            <p>未启用时不会创建任何 worktree。启用后仅可创建 detached worktree，会话状态保持只读。</p>
          </div>
          <label className="git-toggle">
            <span>{config?.enabled ? "已启用" : "默认关闭"}</span>
            <input
              type="checkbox"
              checked={config?.enabled ?? false}
              disabled={!config || loading || isMutating}
              onChange={(event) => void updateEnabled(event.target.checked)}
            />
          </label>
        </div>

        <dl className="git-summary">
          <div>
            <dt>仓库</dt>
            <dd>{loading ? "正在读取" : availability}</dd>
          </div>
          <div>
            <dt>状态摘要</dt>
            <dd>{loading ? "正在读取" : config?.status_summary || "服务端未提供"}</dd>
          </div>
          <div>
            <dt>仓库路径</dt>
            <dd>{loading ? "正在读取" : config?.repository_path || "服务端未提供"}</dd>
          </div>
          <div>
            <dt>当前提交</dt>
            <dd>{loading ? "正在读取" : config?.current_head || "服务端未提供"}</dd>
          </div>
        </dl>
      </section>

      {/* 创建仅向指定会话 API 提交可选 task_id，不允许客户端传递 Git 命令参数。 */}
      <section className="panel git-create-panel">
        <div>
          <h3>创建 detached worktree</h3>
          <p>可关联一个任务，便于识别隔离执行上下文。</p>
        </div>
        <div className="git-create-controls">
          <label>
            关联任务
            <select value={selectedTaskId} disabled={!config?.enabled || loading || isMutating} onChange={(event) => setSelectedTaskId(event.target.value)}>
              <option value="">不关联任务</option>
              {tasks.map((task) => (
                <option key={task.id} value={task.id}>
                  {task.title}
                </option>
              ))}
            </select>
          </label>
          <button className="primary-action" disabled={!config?.enabled || loading || isMutating} onClick={() => void createSession()}>
            {creating ? "创建中" : "创建 worktree"}
          </button>
        </div>
        {selectedTask && <small className="git-selected-task">将关联任务：{selectedTask.title}</small>}
      </section>

      {/* 会话信息完全来自服务端；清理是唯一可写操作，需用户再次确认。 */}
      <section className="panel git-sessions-panel">
        <div className="git-section-head">
          <div>
            <h3>Worktree 会话</h3>
            <small>会话状态只读，清理后将移除对应 worktree。</small>
          </div>
          <span>{sessions.length} 个会话</span>
        </div>
        {loading ? (
          <p className="git-empty">正在读取会话。</p>
        ) : sessions.length ? (
          <div className="git-session-list">
            {sessions.map((session) => {
              const task = tasks.find((item) => item.id === session.task_id);
              return (
                <article className="git-session-row" key={session.id}>
                  <div>
                    <b>{task?.title || session.task_id || "未关联任务"}</b>
                    <small>状态：{session.status || "服务端未提供"}</small>
                    <small>路径：{session.worktree_path || "服务端未提供"}</small>
                    <small>创建时间：{session.created_at || "服务端未提供"}</small>
                  </div>
                  <button className="danger-action" disabled={isMutating} onClick={() => void removeSession(session)}>
                    {removingSessionId === session.id ? "清理中" : "清理 worktree"}
                  </button>
                </article>
              );
            })}
          </div>
        ) : (
          <p className="git-empty">暂无 detached worktree 会话。</p>
        )}
      </section>
    </section>
  );
}

/** 为尚未进入当前里程碑的模块展示明确边界。 */
function Placeholder({ title, description }: { title: string; description: string }) {
  // 不渲染不可操作的假数据或伪控制项。
  return (
    <section className="view">
      <div className="placeholder">
        <span className="placeholder-mark">协</span>
        <h2>{title}</h2>
        <p>{description}</p>
      </div>
    </section>
  );
}

createRoot(document.getElementById("root")!).render(<App />);
