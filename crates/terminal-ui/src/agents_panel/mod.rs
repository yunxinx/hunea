//! `/agents` 全屏 overview panel：child Agent tree 的实时派生状态。

mod input;
mod list_render;
mod preview;
mod preview_render;
mod render;
mod state;
mod transcript;
mod transcript_render;

#[cfg(test)]
mod tests;

pub(crate) use state::{
    AgentsPanelAgentView, AgentsPanelState, AgentsPanelSurface, PendingAgentObservationStops,
};

use runtime_domain::agent::{AgentActivitySummary, AgentProjectionStatus};

/// 状态列固定显示宽度；8 态文本标签在此列内左对齐。
pub(super) const AGENTS_STATUS_COLUMN_WIDTH: usize = 10;
/// elapsed 列固定显示宽度（如 `1m23s` / `999h59m`），行内右对齐。
pub(super) const AGENTS_ELAPSED_COLUMN_WIDTH: usize = 7;

/// status 的文本标签——状态语义由文本承载，不能只靠颜色表达。
pub(super) fn agent_status_label(status: AgentProjectionStatus) -> &'static str {
    match status {
        AgentProjectionStatus::Pending => "Pending",
        AgentProjectionStatus::Working => "Working",
        AgentProjectionStatus::WaitingPermission => "Permission",
        AgentProjectionStatus::Completed => "Completed",
        AgentProjectionStatus::Failed => "Failed",
        AgentProjectionStatus::Cancelled => "Cancelled",
        AgentProjectionStatus::Stopping => "Stopping",
        AgentProjectionStatus::CleanupBlocked => "Cleanup",
    }
}

/// status 列固定宽度填充：所有行的 status 标签占同一列宽，后续列纵向对齐。
pub(super) fn pad_agents_status_column(label: &str, width_budget: usize) -> String {
    use crate::display_width::display_width;

    let column_width = AGENTS_STATUS_COLUMN_WIDTH.min(width_budget);
    let label = crate::status_line::truncate_display_width(label, column_width);
    let padding = column_width.saturating_sub(display_width(&label));
    format!("{label}{}", " ".repeat(padding))
}

/// `x` 二次确认只对仍在运行的 child 生效；终态 child 无需 stop。
pub(super) fn agent_status_is_stoppable(status: AgentProjectionStatus) -> bool {
    matches!(
        status,
        AgentProjectionStatus::Pending
            | AgentProjectionStatus::Working
            | AgentProjectionStatus::WaitingPermission
            | AgentProjectionStatus::Stopping
    )
}

/// latest activity 的 delivery-safe 单行文本。
pub(super) fn agent_activity_summary_text(activity: &AgentActivitySummary) -> String {
    match activity {
        AgentActivitySummary::Preparing => "preparing".to_string(),
        AgentActivitySummary::Thinking => "thinking".to_string(),
        AgentActivitySummary::Retrying { summary } => format!("retrying: {summary}"),
        AgentActivitySummary::UsingTool { title } => format!("tool: {title}"),
        AgentActivitySummary::WaitingPermission { summary } => format!("waiting: {summary}"),
        AgentActivitySummary::Idle => "idle".to_string(),
    }
}

/// elapsed 的紧凑标签；最长 `999h59m`，列内右对齐时不超过 7 列。
pub(super) fn format_agent_elapsed_ms(elapsed_ms: u64) -> String {
    let total_seconds = elapsed_ms / 1_000;
    if total_seconds < 60 {
        format!("{total_seconds}s")
    } else if total_seconds < 3_600 {
        let minutes = total_seconds / 60;
        let seconds = total_seconds % 60;
        format!("{minutes}m{seconds:02}s")
    } else {
        // 小时封顶 999，保证标签不超过 7 个显示列。
        let hours = (total_seconds / 3_600).min(999);
        let minutes = (total_seconds % 3_600) / 60;
        format!("{hours}h{minutes:02}m")
    }
}

/// token usage 的紧凑标签（`2k tok` / `1M tok`）。
pub(super) fn format_agent_token_usage(token_usage: usize) -> String {
    if token_usage < 1_000 {
        format!("{token_usage} tok")
    } else if token_usage < 1_000_000 {
        format!("{}k tok", token_usage / 1_000)
    } else {
        format!("{}M tok", token_usage / 1_000_000)
    }
}

/// tool 次数的紧凑标签。
pub(super) fn format_agent_tool_uses(tool_uses: usize) -> String {
    if tool_uses == 1 {
        "1 tool".to_string()
    } else {
        format!("{tool_uses} tools")
    }
}

/// observation 拒绝的 closed 分类文案；不携带 raw 错误正文。
pub(super) fn agents_panel_rejection_text(
    reason: runtime_domain::agent::AgentObservationRejection,
) -> String {
    use runtime_domain::agent::AgentObservationRejection;
    match reason {
        AgentObservationRejection::UnknownAgent => "Child Agent is not available".to_string(),
        AgentObservationRejection::StaleGeneration => {
            "Agent runtime was replaced; reopen /agents".to_string()
        }
        AgentObservationRejection::Duplicate => "Agent observation is already active".to_string(),
    }
}
