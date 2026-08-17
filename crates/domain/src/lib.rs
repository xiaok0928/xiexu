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

/// Agent 在协序中的可用状态，停用后不得接收新任务但历史事实继续保留。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentStatus {
    /// Agent 可以参与项目、任务和对话。
    Active,
    /// Agent 暂停参与新工作，已有记录仍可查询。
    Inactive,
}

impl AgentStatus {
    /// 将 API 或数据库中的 Agent 状态解析为领域值。
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "active" => Some(Self::Active),
            "inactive" => Some(Self::Inactive),
            _ => None,
        }
    }

    /// 返回 Agent 状态的稳定持久化字符串。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Inactive => "inactive",
        }
    }
}

/// Agent 在项目中的长期职责类型，与任务级动态参与关系分开保存。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectAgentAssignment {
    /// 负责拆分、指派、依赖协调和结果汇总的唯一协调 Agent。
    Coordinator,
    /// 长期参与项目并可由协调 Agent 分配任务的固定 Agent。
    Fixed,
}

impl ProjectAgentAssignment {
    /// 将项目 Agent 职责字符串解析为领域值。
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "coordinator" => Some(Self::Coordinator),
            "fixed" => Some(Self::Fixed),
            _ => None,
        }
    }

    /// 返回项目 Agent 职责的稳定持久化字符串。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Coordinator => "coordinator",
            Self::Fixed => "fixed",
        }
    }
}

/// Agent 在单个任务中的动态参与方式，不改变其项目固定成员身份。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskAgentParticipation {
    /// 对任务结果负责的唯一主责 Agent。
    Owner,
    /// 参与当前任务某个子阶段的执行 Agent。
    Participant,
    /// 被提及后提供临时协助的 Agent。
    Helper,
}

impl TaskAgentParticipation {
    /// 将任务参与方式字符串解析为领域值。
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "owner" => Some(Self::Owner),
            "participant" => Some(Self::Participant),
            "helper" => Some(Self::Helper),
            _ => None,
        }
    }

    /// 返回任务参与方式的稳定持久化字符串。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Owner => "owner",
            Self::Participant => "participant",
            Self::Helper => "helper",
        }
    }
}

/// Agent 私有记忆的生命周期层级，二者都固定归属于具体 Agent。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryTier {
    /// 当前项目或任务期间需要快速复用的短期经验。
    ShortTerm,
    /// 经确认可在未来工作中持续复用的长期经验。
    LongTerm,
}

impl MemoryTier {
    /// 将记忆层级字符串解析为领域值。
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "short_term" => Some(Self::ShortTerm),
            "long_term" => Some(Self::LongTerm),
            _ => None,
        }
    }

    /// 返回记忆层级的稳定持久化字符串。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ShortTerm => "short_term",
            Self::LongTerm => "long_term",
        }
    }
}

/// 对话类型决定参与者约束和是否必须关联项目。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConversationType {
    /// 一个 Human 与一个 Agent 的私聊。
    Direct,
    /// 随项目创建并长期存在的项目主群聊。
    ProjectMain,
    /// 为一个或多个任务临时组织的项目群聊。
    ProjectTemporary,
}

impl ConversationType {
    /// 将对话类型字符串解析为领域值。
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "direct" => Some(Self::Direct),
            "project_main" => Some(Self::ProjectMain),
            "project_temporary" => Some(Self::ProjectTemporary),
            _ => None,
        }
    }

    /// 返回对话类型的稳定持久化字符串。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Direct => "direct",
            Self::ProjectMain => "project_main",
            Self::ProjectTemporary => "project_temporary",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{AgentStatus, ConversationType, MemoryTier, ProjectAgentAssignment, TaskAgentParticipation};

    /// 验证 M3 枚举只接受公开契约中的稳定字符串。
    #[test]
    fn parses_m3_domain_values() {
        // 每类合法值至少覆盖一个分支，并验证未知输入被拒绝。
        assert_eq!(AgentStatus::parse("active"), Some(AgentStatus::Active));
        assert_eq!(ProjectAgentAssignment::parse("coordinator"), Some(ProjectAgentAssignment::Coordinator));
        assert_eq!(TaskAgentParticipation::parse("helper"), Some(TaskAgentParticipation::Helper));
        assert_eq!(MemoryTier::parse("long_term"), Some(MemoryTier::LongTerm));
        assert_eq!(ConversationType::parse("direct"), Some(ConversationType::Direct));
        assert_eq!(ConversationType::parse("unknown"), None);
    }
}
