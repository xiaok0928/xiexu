import { useEffect, useMemo, useState } from "react";
import { createRoot } from "react-dom/client";
import "./styles.css";

type ViewKey = "board" | "project" | "workflow" | "chat" | "agents" | "runs" | "settings";
type TaskCard = { id: string; project_id: string; parent_task_id?: string; title: string; description: string; board_stage: string; plan_status: string; execution_status: string; progress_percent: number; requires_plan_confirmation: boolean; children_count: number; revision: number };
type ExecutionSnapshot = { outputs: Array<{ id: string; output_type: string; content: string; created_at: string }>; events: Array<{ id: number; event_type: string; created_at: string }> };
type Project = { id: string; name: string; description: string };
const stages = ["backlog", "todo", "plan_review", "in_progress", "acceptance"];
const stageLabels: Record<string, string> = { backlog: "Backlog", todo: "Todo", plan_review: "方案待确认", in_progress: "处理中", acceptance: "等待验收" };
const navItems: Array<{ key: ViewKey; label: string; icon: string }> = [
  { key: "board", label: "任务面板", icon: "▦" }, { key: "project", label: "项目空间", icon: "□" }, { key: "workflow", label: "工作流", icon: "⌘" },
  { key: "chat", label: "新对话", icon: "□" }, { key: "agents", label: "Agent", icon: "♙" }, { key: "runs", label: "运行记录", icon: "≡" }, { key: "settings", label: "设置", icon: "⚙" },
];

/** 从当前 URL 恢复模块入口，刷新页面后保留用户上下文。 */
function viewFromLocation(): ViewKey { const path = window.location.pathname; if (path.startsWith("/project")) return "project"; if (path.startsWith("/workflow")) return "workflow"; if (path.startsWith("/chat")) return "chat"; if (path.startsWith("/agents")) return "agents"; if (path.startsWith("/runs")) return "runs"; if (path.startsWith("/settings")) return "settings"; return "board"; }

/** 根据模块入口生成稳定路径，供桌面导航和移动端切换使用。 */
function pathForView(view: ViewKey): string { if (view === "board") return "/"; if (view === "project") return "/project/xiexu"; if (view === "workflow") return "/workflow/demo"; if (view === "chat") return "/chat/new"; return `/${view}`; }

/** 统一封装 M1 JSON API 请求，错误正文保留给页面状态展示。 */
async function api<T>(path: string, options?: RequestInit): Promise<T> { const response = await fetch(path, { headers: { "Content-Type": "application/json" }, ...options }); if (!response.ok) throw new Error(await response.text()); return response.json() as Promise<T>; }

/** Web 工作台：加载真实项目、任务和执行事实源，暂不伪造 Agent 或 Codex 结果。 */
function App() {
  const [view, setView] = useState<ViewKey>(viewFromLocation);
  const [projects, setProjects] = useState<Project[]>([]);
  const [project, setProject] = useState<Project>();
  const [tasks, setTasks] = useState<TaskCard[]>([]);
  const [selectedTask, setSelectedTask] = useState<TaskCard>();
  const [error, setError] = useState("");

  /** 初始化本地演示项目，真实用户项目仍通过 API 创建。 */
  async function loadBoard() { try { setError(""); let result = await api<{ items: Project[] }>("/api/projects"); if (!result.items.length) { await api<Project>("/api/projects", { method: "POST", body: JSON.stringify({ name: "xiexu", description: "协序 M1 任务空间" }) }); result = await api<{ items: Project[] }>("/api/projects"); } const current = result.items[0]; setProjects(result.items); setProject(current); const taskResult = await api<{ items: TaskCard[] }>(`/api/projects/${current.id}/tasks`); setTasks(taskResult.items); setSelectedTask((previous) => taskResult.items.find((task) => task.id === previous?.id) ?? taskResult.items[0]); } catch (cause) { setError(cause instanceof Error ? cause.message : "加载失败"); } }
  useEffect(() => { void loadBoard(); }, []);

  /** 切换模块时同步浏览器路径，避免刷新回到默认页面。 */
  function navigate(nextView: ViewKey) { window.history.pushState({}, "", pathForView(nextView)); setView(nextView); }

  /** 创建一个 Backlog 想法，符合 M1 “先记录、不执行”规则。 */
  async function createIdea() { if (!project) return; const title = window.prompt("记录一个想法"); if (!title?.trim()) return; await api(`/api/projects/${project.id}/tasks`, { method: "POST", body: JSON.stringify({ title: title.trim(), description: "", requires_plan_confirmation: true }) }); await loadBoard(); }

  /** 通过服务端状态机移动任务，前端不直接修改看板阶段。 */
  async function moveTask(task: TaskCard, target: string) { try { await api(`/api/tasks/${task.id}/transitions`, { method: "POST", body: JSON.stringify({ target_stage: target, reason: "Human 在任务面板中移动" }) }); await loadBoard(); } catch (cause) { setError(cause instanceof Error ? cause.message : "状态转换失败"); } }

  /** 更新 Todo 卡片的方案确认开关，设置只影响后续生命周期。 */
  async function togglePlan(task: TaskCard) { await api(`/api/tasks/${task.id}`, { method: "PATCH", body: JSON.stringify({ requires_plan_confirmation: !task.requires_plan_confirmation }) }); await loadBoard(); }

  return <div className="app-shell"><aside className="sidebar"><div className="brand"><span className="brand-mark">协</span><div><b>协序</b><small>xiexu v0.1</small></div></div><nav className="nav-list" aria-label="主导航">{navItems.map((item) => <button key={item.key} className={view === item.key ? "nav-item active" : "nav-item"} onClick={() => navigate(item.key)}><span>{item.icon}</span>{item.label}</button>)}</nav><div className="recent"><small>最近项目</small>{projects.map((item) => <button className={project?.id === item.id ? "recent-active" : ""} key={item.id}>{item.name}</button>)}</div><div className="user-box"><span className="avatar">HB</span><div><b>Human</b><small>本地管理员</small></div></div></aside><main className="workspace"><header className="topbar"><div><small>协序 / M1 任务域</small><h1>{navItems.find((item) => item.key === view)?.label}</h1></div><div className="toolbar"><button title="搜索">⌕</button><button title="过滤">≡</button><button className="automation" onClick={() => navigate("workflow")}>▷ 自动化</button><button className="primary" title="记录想法" onClick={() => void createIdea()}>＋</button></div></header>{error && <div className="error-banner">{error}</div>}{view === "board" && project && <Board tasks={tasks} selectedTask={selectedTask} onSelect={setSelectedTask} onMove={moveTask} onTogglePlan={togglePlan} onReload={loadBoard} onAutomation={() => navigate("workflow")} />}{view !== "board" && <Placeholder title={navItems.find((item) => item.key === view)?.label ?? "模块"} description={view === "project" ? "项目空间将复用当前任务事实源，文档和群聊在后续里程碑接入。" : "该模块已保留入口，业务能力按 M1 之后的里程碑接入。"} />}</main></div>;
}

type BoardProps = { tasks: TaskCard[]; selectedTask?: TaskCard; onSelect: (task: TaskCard) => void; onMove: (task: TaskCard, target: string) => Promise<void>; onTogglePlan: (task: TaskCard) => Promise<void>; onReload: () => Promise<void>; onAutomation: () => void };

/** 任务面板：支持 taskboard 风格列、父子关系、拖拽移动和详情评论。 */
function Board({ tasks, selectedTask, onSelect, onMove, onTogglePlan, onReload, onAutomation }: BoardProps) {
  const [mobileStage, setMobileStage] = useState(stages[0]);
  const grouped = useMemo(() => Object.fromEntries(stages.map((stage) => [stage, tasks.filter((task) => task.board_stage === stage)])), [tasks]);
  return <section className="view"><div className="view-head"><div className="tabs"><button className="tab active">看板</button><button className="tab">列表</button><button className="tab">甘特</button></div><select value={mobileStage} onChange={(event) => setMobileStage(event.target.value)} aria-label="选择看板列">{stages.map((stage) => <option key={stage} value={stage}>{stageLabels[stage]}</option>)}</select></div><div className="board-layout"><div className="kanban">{stages.map((stage) => <section className={`column ${stage === mobileStage ? "mobile-visible" : ""}`} key={stage} onDragOver={(event) => event.preventDefault()} onDrop={(event) => { const task = tasks.find((item) => item.id === event.dataTransfer.getData("text/plain")); if (task && task.board_stage !== stage) void onMove(task, stage); }}><div className="column-head"><span>{stageLabels[stage]}</span><small>{grouped[stage].length}</small></div>{grouped[stage].map((task) => <button className={selectedTask?.id === task.id ? "task-card selected" : "task-card"} draggable onDragStart={(event) => event.dataTransfer.setData("text/plain", task.id)} key={task.id} onClick={() => onSelect(task)}><small>{task.id}{task.parent_task_id ? ` · 父 ${task.parent_task_id.slice(0, 8)}` : ""}</small><b>{task.title}</b><span className="task-status">{task.execution_status} · {task.progress_percent}%</span>{stage === "todo" && <span className="confirm" onClick={(event) => { event.stopPropagation(); void onTogglePlan(task); }}>☑ 方案确认 {task.requires_plan_confirmation ? "开" : "关"}</span>}</button>)}</section>)}</div>{selectedTask ? <TaskDetail task={selectedTask} onReload={onReload} onAutomation={onAutomation} /> : <div className="detail empty-detail">选择任务查看评论与事实字段</div>}</div></section>;
}

/** 任务详情抽屉：展示阶段、运行记录和评论事实。 */
function TaskDetail({ task, onReload, onAutomation }: { task: TaskCard; onReload: () => Promise<void>; onAutomation: () => void }) {
  const [content, setContent] = useState(""); const [intent, setIntent] = useState("note"); const [comments, setComments] = useState<Array<{ id: string; author_name: string; content: string; intent: string }>>([]);
  const [execution, setExecution] = useState<ExecutionSnapshot>({ outputs: [], events: [] });
  useEffect(() => { void api<{ items: typeof comments }>(`/api/tasks/${task.id}/comments`).then((result) => setComments(result.items)); }, [task.id]);
  useEffect(() => { void api<ExecutionSnapshot>(`/api/tasks/${task.id}/execution`).then(setExecution).catch(() => setExecution({ outputs: [], events: [] })); }, [task.id, task.execution_status, task.revision]);
  async function sendComment() { if (!content.trim()) return; await api(`/api/tasks/${task.id}/comments`, { method: "POST", body: JSON.stringify({ content: content.trim(), intent }) }); setContent(""); await onReload(); const [commentResult, executionResult] = await Promise.all([api<{ items: typeof comments }>(`/api/tasks/${task.id}/comments`), api<ExecutionSnapshot>(`/api/tasks/${task.id}/execution`)]); setComments(commentResult.items); setExecution(executionResult); }
  return <aside className="detail"><small>{task.id} · xiexu</small><h2>{task.title}</h2><div className="fact"><span>看板阶段</span><b>{stageLabels[task.board_stage] ?? task.board_stage}</b><span>执行状态</span><b>{task.execution_status}</b><span>进度</span><b>{task.progress_percent}%</b><span>父任务</span><b>{task.parent_task_id ? task.parent_task_id.slice(0, 8) : "无"}</b></div><h3>运行记录</h3><div className="execution-list">{execution.outputs.map((item) => <div className="execution-output" key={item.id}><b>{item.output_type}</b><p>{item.content}</p></div>)}{!execution.outputs.length && <p className="muted">暂无运行输出</p>}<small>{execution.events.length ? `已记录 ${execution.events.length} 个执行事件` : "暂无执行事件"}</small></div><h3>评论</h3><div className="comment-list">{comments.map((item) => <div className="comment" key={item.id}><b>{item.author_name} · {item.intent}</b><p>{item.content}</p></div>)}{!comments.length && <p className="muted">暂无评论</p>}</div><div className="comment-input"><input value={content} onChange={(event) => setContent(event.target.value)} placeholder="输入评论" onKeyDown={(event) => { if (event.key === "Enter") void sendComment(); }} /><select value={intent} onChange={(event) => setIntent(event.target.value)} aria-label="评论提示意图"><option value="note">记录</option><option value="approve_plan">方案确认提示</option><option value="accept">验收通过提示</option><option value="rework">返工提示</option></select><button onClick={() => void sendComment()}>发送</button></div><button className="text-action" onClick={onAutomation}>查看自动化</button></aside>;
}

/** 未进入业务实现的模块保留统一占位，避免提前伪造后端能力。 */
function Placeholder({ title, description }: { title: string; description: string }) { return <section className="view"><div className="placeholder"><span className="placeholder-mark">协</span><h2>{title}</h2><p>{description}</p></div></section>; }

createRoot(document.getElementById("root")!).render(<App />);
