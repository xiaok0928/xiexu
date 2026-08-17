const views = Array.from(document.querySelectorAll(".view"));
const navButtons = Array.from(document.querySelectorAll("[data-view]"));
const viewTitle = document.getElementById("viewTitle");

function activateView(name) {
  views.forEach((view) => {
    view.classList.toggle("active", view.id === `view-${name}`);
  });
  navButtons.forEach((button) => {
    button.classList.toggle("active", button.dataset.view === name && button.classList.contains("nav-item"));
  });
  const active = document.getElementById(`view-${name}`);
  if (active && viewTitle) viewTitle.textContent = active.dataset.title || "协序";
}

navButtons.forEach((button) => {
  button.addEventListener("click", () => activateView(button.dataset.view));
});

const drawerTitle = document.getElementById("drawerTitle");
const drawerMeta = document.getElementById("drawerMeta");
const taskData = {
  idea: ["IDEA-18 · Backlog", "记录一个可通过自然语言创建任务的想法"],
  ops: ["IDEA-21 · Backlog", "每周整理客户反馈摘要"],
  parent: ["REQ-104 · xiexu · 父任务", "实现任务面板与项目空间的共享任务事实源"],
  chat: ["REQ-109 · 项目群聊生成", "从项目群聊发布需求并同步任务卡片"],
  plan: ["REQ-096 · 方案 v2", "工作流人工确认节点评论流转"],
  workflow: ["WF-12 · 每周项目文档刷新", "扫描父任务完成后的项目文档遗漏"],
  subtask: ["REQ-104-2 · 子任务", "前端看板订阅 execution_events"],
  accept: ["REQ-088 · 部分验收", "Agent 身份管理和固定记忆摘要"]
};

document.querySelectorAll(".task-card").forEach((card) => {
  card.addEventListener("click", (event) => {
    if (event.target instanceof HTMLElement && event.target.closest("[data-toggle-children]")) return;
    document.querySelectorAll(".task-card").forEach((item) => item.classList.remove("selected"));
    card.classList.add("selected");
    const data = taskData[card.dataset.task] || taskData.parent;
    if (drawerMeta) drawerMeta.textContent = data[0];
    if (drawerTitle) drawerTitle.textContent = data[1];
  });
});

document.querySelectorAll("[data-toggle-children]").forEach((button) => {
  button.addEventListener("click", (event) => {
    event.stopPropagation();
    const card = button.closest(".task-card");
    if (!card) return;
    const expanded = card.classList.toggle("expanded");
    button.textContent = expanded ? "收起子任务" : "展开 3 个子任务";
  });
});

const mobileColumnSelect = document.querySelector(".mobile-column-select");
if (mobileColumnSelect) {
  mobileColumnSelect.addEventListener("change", () => {
    const columns = Array.from(document.querySelectorAll(".column"));
    columns.forEach((column, index) => {
      column.style.display = index === mobileColumnSelect.selectedIndex ? "block" : "";
    });
  });
}
