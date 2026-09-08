//! `/agents` 全屏 overview panel：child Agent tree 的实时派生状态。

mod input;
mod list_render;
mod pending_permission;
mod permission_choice;
mod render;
mod state;
mod transcript;
mod transcript_render;

#[cfg(test)]
mod tests;

pub(crate) use pending_permission::{AgentPendingPermissionProjection, AgentsPanelPillNavigation};
pub(crate) use state::{
    AgentsPanelActivityFold, AgentsPanelAgentView, AgentsPanelPermissionChoice, AgentsPanelState,
    AgentsPanelStopConfirmation, AgentsPanelSurface, PendingAgentObservationStops,
};

use ratatui::style::Style;
use runtime_domain::agent::{AgentActivitySummary, AgentProjectionStatus, AgentTranscriptItem};

use crate::theme::{
    TerminalColorCapability, TerminalPalette, accent_text_style, approval_rejected_text_style,
    command_accent_text_style, success_text_style, system_error_text_style, tertiary_text_style,
};

/// 状态列固定显示宽度；8 态文本标签在此列内左对齐。
/// 上限由最长标签 `Permission` 决定。
pub(super) const AGENTS_STATUS_COLUMN_WIDTH: usize = 10;
/// metrics 三列的固定显示宽度（列内右对齐、前置填充）；列间以单空格分隔。
/// 宽度按各列的常规最大内容核定：`999h59m`、`99999`、`99.9M`。
pub(super) const AGENTS_ELAPSED_COLUMN_WIDTH: usize = 8;
pub(super) const AGENTS_TOOLS_COLUMN_WIDTH: usize = 6;
pub(super) const AGENTS_TOKENS_COLUMN_WIDTH: usize = 7;

/// status 的文本标签——状态语义由文本承载，不能只靠颜色表达。
pub(super) fn agent_status_label(status: AgentProjectionStatus) -> &'static str {
    match status {
        AgentProjectionStatus::Pending => "Pending",
        AgentProjectionStatus::Working => "Working",
        AgentProjectionStatus::WaitingPermission => "Permission",
        AgentProjectionStatus::Completed => "Done",
        AgentProjectionStatus::Failed => "Failed",
        AgentProjectionStatus::Cancelled => "Cancelled",
        AgentProjectionStatus::Stopping => "Stopping",
        AgentProjectionStatus::CleanupBlocked => "Cleanup",
    }
}

/// 状态点与状态文字共用的语义样式：颜色按状态映射到既有 palette 槽位，
/// 终端默认配色下部分槽位退化为 `Color::Reset`（无前景色），
/// 由点符号与文字标签保底区分。
pub(super) fn agent_status_dot_style(
    status: AgentProjectionStatus,
    palette: &TerminalPalette,
) -> Style {
    match status {
        AgentProjectionStatus::Working => command_accent_text_style(*palette),
        AgentProjectionStatus::WaitingPermission => accent_text_style(*palette),
        AgentProjectionStatus::Completed => success_text_style(*palette),
        AgentProjectionStatus::Failed => system_error_text_style(*palette),
        AgentProjectionStatus::Cancelled => approval_rejected_text_style(*palette),
        AgentProjectionStatus::Pending
        | AgentProjectionStatus::Stopping
        | AgentProjectionStatus::CleanupBlocked => tertiary_text_style(*palette),
    }
}

/// 状态点符号。显式配色下颜色可承载区分度，统一实心；
/// 终端默认配色下颜色不可靠，运行中用实心、终态用空心保底区分。
pub(super) fn agent_status_dot_symbol(
    status: AgentProjectionStatus,
    palette: &TerminalPalette,
) -> &'static str {
    if palette.color_capability() == TerminalColorCapability::TerminalDefault
        && !agent_status_is_running(status)
    {
        "○"
    } else {
        "●"
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

/// 是否仍在运行（未进入终态）：`x` stop 确认与状态点实/空心共用该判定。
pub(super) fn agent_status_is_running(status: AgentProjectionStatus) -> bool {
    matches!(
        status,
        AgentProjectionStatus::Pending
            | AgentProjectionStatus::Working
            | AgentProjectionStatus::WaitingPermission
            | AgentProjectionStatus::Stopping
    )
}

/// 是否为 settled 投影行（自然终态或显式停止定格）：`x` 对该类行是 delete 语义。
/// CleanupBlocked 不算——清理未收敛的行不可操作。
pub(super) fn agent_status_is_settled(status: AgentProjectionStatus) -> bool {
    matches!(
        status,
        AgentProjectionStatus::Completed
            | AgentProjectionStatus::Failed
            | AgentProjectionStatus::Cancelled
    )
}

/// latest activity 的 delivery-safe 单行文本。`Idle` 不携带有效信息，
/// latest 列与折叠区都不展示该文本（见 `AGENT_ACTIVITY_IDLE_TEXT`）。
pub(super) const AGENT_ACTIVITY_IDLE_TEXT: &str = "idle";

pub(super) fn agent_activity_summary_text(activity: &AgentActivitySummary) -> String {
    match activity {
        AgentActivitySummary::Preparing => "preparing".to_string(),
        AgentActivitySummary::Thinking => "thinking".to_string(),
        AgentActivitySummary::Retrying { summary } => format!("retrying: {summary}"),
        AgentActivitySummary::UsingTool { title } => format!("tool: {title}"),
        AgentActivitySummary::WaitingPermission { summary } => format!("waiting: {summary}"),
        AgentActivitySummary::Idle => AGENT_ACTIVITY_IDLE_TEXT.to_string(),
    }
}

/// 活动折叠区展示的最近活动条数上限；更早条目折叠为 `+N more`。
pub(super) const AGENTS_ACTIVITY_FOLD_ENTRY_COUNT: usize = 3;
/// 折叠区整区隐藏的宽度阈值：更窄的终端上主行已进入列让位区间。
pub(super) const AGENTS_ACTIVITY_FOLD_MIN_WIDTH: usize = 60;
/// 折叠区行数上限（3 条活动 + 1 行 `+N more`）：list 页行预算的固定预留量。
pub(super) const AGENTS_ACTIVITY_FOLD_MAX_LINES: usize = AGENTS_ACTIVITY_FOLD_ENTRY_COUNT + 1;
/// 折叠区单条摘要的缓存宽度上限；渲染时再按实际列宽二次截断，
/// 这里只避免把长正文整段复制进折叠区缓存。
const AGENTS_ACTIVITY_FOLD_ENTRY_CACHE_WIDTH: usize = 200;

/// 折叠区条目提取：transcript 尾部的 tool/assistant 条目转单行摘要。
///
/// User 条目是发起指令而非 agent 活动，不进入折叠区；摘要恰为 Idle 文案的条目
/// 同样跳过（无价值信息）。返回（最近 entries, 被折叠的更早条数）。
pub(super) fn agents_activity_fold_entries(items: &[AgentTranscriptItem]) -> (Vec<String>, usize) {
    let eligible: Vec<String> = items
        .iter()
        .filter_map(activity_fold_entry_text)
        .filter(|entry| entry != AGENT_ACTIVITY_IDLE_TEXT)
        .collect();
    let hidden = eligible
        .len()
        .saturating_sub(AGENTS_ACTIVITY_FOLD_ENTRY_COUNT);
    (eligible[hidden..].to_vec(), hidden)
}

/// 单条活动的单行摘要：多行内容只取首个非空行，空内容条目整体跳过。
fn activity_fold_entry_text(item: &AgentTranscriptItem) -> Option<String> {
    let source = match item {
        AgentTranscriptItem::Tool { title, content } => {
            let title = title.trim();
            if title.is_empty() { content } else { title }
        }
        AgentTranscriptItem::Assistant { content } => content,
        AgentTranscriptItem::User { .. } => return None,
    };
    let first_line = source
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())?;
    Some(crate::status_line::truncate_display_width_with_ellipsis(
        first_line,
        AGENTS_ACTIVITY_FOLD_ENTRY_CACHE_WIDTH,
    ))
}

/// list 页行预算：每行 1 行，body 首行是列头行，另为选中行的活动折叠区恒定预留
/// `AGENTS_ACTIVITY_FOLD_MAX_LINES` 行。
///
/// 预留不随折叠区实际可见性变化——page 边界若随 selection/事件抖动，
/// 翻页与鼠标行换算会在导航中错位；渲染与输入路径必须共用本函数。
pub(super) fn agents_panel_list_page_size(height: u16) -> usize {
    crate::fullscreen_list_chrome::fullscreen_list_page_size_for_height(height)
        .saturating_sub(1 + AGENTS_ACTIVITY_FOLD_MAX_LINES)
        .max(1)
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

/// token usage 的列标签：K/M 缩放，不带单位后缀（列头已表达语义）。
/// 两位以内 mantissa 保留一位小数（`8` / `1.2K`），更高位退化为整数（`999K`），
/// 保证标签宽度有稳定上界。
pub(super) fn format_agent_token_usage(token_usage: usize) -> String {
    if token_usage < 1_000 {
        format!("{token_usage}")
    } else if token_usage < 100_000 {
        format!("{:.1}K", token_usage as f64 / 1_000.0)
    } else if token_usage < 1_000_000 {
        format!("{}K", token_usage / 1_000)
    } else if token_usage < 100_000_000 {
        format!("{:.1}M", token_usage as f64 / 1_000_000.0)
    } else {
        // 亿级以上封顶 999M，不再加宽标签。
        format!("{}M", (token_usage / 1_000_000).min(999))
    }
}

/// tool 次数的列标签：纯数字（列头已表达语义）。
pub(super) fn format_agent_tool_uses(tool_uses: usize) -> String {
    tool_uses.to_string()
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
