//! 领域层占位，M1 开始承载 Task、Workflow 和 Agent 的稳定业务模型。

/// M0 运行器状态，供后续领域模型复用。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunnerStatus {
    /// 运行器已经注册并持续续租。
    Ready,
    /// 运行器租约已过期。
    Stale,
}

/// 任务在用户看板上可见的主阶段，执行子状态不应混入这些列。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoardStage {
    /// 仅记录想法，不触发执行。
    Backlog,
    /// 已确认要做，等待协调 Agent 扫描。
    Todo,
    /// 等待 Human 确认方案。
    PlanReview,
    /// 正在执行或编排执行。
    InProgress,
    /// 等待 Human 验收。
    Acceptance,
    /// 已完成并归档。
    Done,
    /// Human 明确取消的任务。
    Cancelled,
}

impl BoardStage {
    /// 将 API 或数据库中的阶段字符串解析为领域值。
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "backlog" => Some(Self::Backlog),
            "todo" => Some(Self::Todo),
            "plan_review" => Some(Self::PlanReview),
            "in_progress" => Some(Self::InProgress),
            "acceptance" => Some(Self::Acceptance),
            "done" => Some(Self::Done),
            "cancelled" => Some(Self::Cancelled),
            _ => None,
        }
    }

    /// 返回稳定的持久化字符串。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Backlog => "backlog",
            Self::Todo => "todo",
            Self::PlanReview => "plan_review",
            Self::InProgress => "in_progress",
            Self::Acceptance => "acceptance",
            Self::Done => "done",
            Self::Cancelled => "cancelled",
        }
    }
}

/// 判断任务主阶段转换是否符合协序 M1 状态机。
pub fn is_valid_transition(from: BoardStage, to: BoardStage) -> bool {
    if to == BoardStage::Cancelled && from != BoardStage::Done {
        return true;
    }
    matches!(
        (from, to),
        (BoardStage::Backlog, BoardStage::Todo)
            | (BoardStage::Todo, BoardStage::PlanReview)
            | (BoardStage::Todo, BoardStage::InProgress)
            | (BoardStage::PlanReview, BoardStage::InProgress)
            | (BoardStage::InProgress, BoardStage::Acceptance)
            | (BoardStage::Acceptance, BoardStage::Done)
            | (BoardStage::Acceptance, BoardStage::Todo)
            | (BoardStage::Acceptance, BoardStage::InProgress)
    )
}
