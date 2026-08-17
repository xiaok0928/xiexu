import { useEffect, useMemo, useState } from "react";
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
            <small>xiexu M3</small>
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
        {view === "workflow" && <Placeholder title="工作流" description="工作流画布属于后续里程碑，本阶段不伪造运行数据。" />}
        {view === "runs" && <Placeholder title="运行记录" description="任务运行记录已在任务详情展示，跨任务总览将在后续阶段接入。" />}
        {view === "settings" && <Placeholder title="设置" description="MVP 暂不启用权限管理，系统设置将在后续接入。" />}
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
        {selectedTask ? <TaskDetail task={selectedTask} onReload={onReload} /> : <div className="detail">选择任务查看详情</div>}
      </div>
    </section>
  );
}

/** 任务详情：展示主责 Agent、运行输出和评论状态流转。 */
function TaskDetail({ task, onReload }: { task: TaskCard; onReload: () => Promise<void> }) {
  const [comments, setComments] = useState<Array<{ id: string; author_name: string; content: string; intent: string }>>([]);
  const [execution, setExecution] = useState<ExecutionSnapshot>({ outputs: [], events: [] });
  const [agents, setAgents] = useState<Array<{ agent_id: string; name: string; participation_type: string; status: string }>>([]);
  const [content, setContent] = useState("");
  const [intent, setIntent] = useState("note");

  /** 并行加载任务详情事实。 */
  async function load() {
    // 三类数据彼此独立，合并等待后一次刷新界面。
    const [commentResult, executionResult, agentResult] = await Promise.all([
      api<{ items: typeof comments }>(`/api/tasks/${task.id}/comments`),
      api<ExecutionSnapshot>(`/api/tasks/${task.id}/execution`),
      api<{ items: typeof agents }>(`/api/tasks/${task.id}/agents`),
    ]);
    setComments(commentResult.items);
    setExecution(executionResult);
    setAgents(agentResult.items);
  }
  useEffect(() => {
    void load();
  }, [task.id, task.revision]);

  /** 发送带显式意图提示的任务评论。 */
  async function send() {
    // 空内容不写入事实源，成功后同步任务和详情。
    if (!content.trim()) return;
    await api(`/api/tasks/${task.id}/comments`, { method: "POST", body: JSON.stringify({ content: content.trim(), intent }) });
    setContent("");
    await onReload();
    await load();
  }

  const owner = agents.find((item) => item.participation_type === "owner" && item.status === "active");
  return (
    <aside className="detail">
      <small>{task.id}</small>
      <h2>{task.title}</h2>
      <div className="fact">
        <span>阶段</span>
        <b>{stageLabels[task.board_stage]}</b>
        <span>执行</span>
        <b>{task.execution_status}</b>
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
        {comments.map((item) => (
          <div className="comment" key={item.id}>
            <b>
              {item.author_name} · {item.intent}
            </b>
            <p>{item.content}</p>
          </div>
        ))}
      </div>
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
      <div className="project-grid">
        <aside className="panel project-members">
          <div className="panel-head">
            <h3>项目 Agent</h3>
            <button className="icon-action" onClick={() => void addMember()}>
              ＋
            </button>
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
            <button className="icon-action" onClick={() => void createGroup()}>
              ＋
            </button>
          </div>
          {conversations.map((item) => (
            <button key={item.id} className={item.id === selectedId ? "conversation-row active" : "conversation-row"} onClick={() => setSelectedId(item.id)}>
              <b>{item.title}</b>
              <small>
                {item.conversation_type === "project_main" ? "主群聊" : "临时群聊"} · {item.status}
              </small>
            </button>
          ))}
        </aside>
        {selectedId && (
          <ConversationPanel
            conversationId={selectedId}
            project={project}
            onChanged={async () => {
              await load();
              await onReload();
            }}
          />
        )}
      </div>
    </section>
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
