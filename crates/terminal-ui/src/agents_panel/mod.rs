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
/// elapsed 档宽覆盖共享 elapsed 格式的小时档（`9h 59m 59s`），更长的极端值由
/// metric 超宽保留策略兜底；tools/tokens 按常规最大内容 `99999` / `99.9m` 核定。
pub(super) const AGENTS_ELAPSED_COLUMN_WIDTH: usize = 10;
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

/// latest activity 的 delivery-safe 单行文本：直接展示投影携带的 label/summary，
/// 不加合成前缀（主 UI 的活动行同样显示裸 title）。`Idle` 不携带有效信息，
/// latest 列对其整列隐藏（`list_render` 预过滤），占位文本仅供枚举穷尽。
pub(super) fn agent_activity_summary_text(activity: &AgentActivitySummary) -> String {
    match activity {
        AgentActivitySummary::Preparing => "preparing".to_string(),
        AgentActivitySummary::Thinking => "thinking".to_string(),
        AgentActivitySummary::Retrying { summary } => summary.clone(),
        AgentActivitySummary::UsingTool { title } => normalized_tool_entry_title(title).to_string(),
        AgentActivitySummary::WaitingPermission { summary } => summary.clone(),
        AgentActivitySummary::Idle => "idle".to_string(),
    }
}

/// tool 条目 title 的展示归一化：剥 `Shell:` 一类传输前缀，与主 transcript 的
/// `tool_result::activity::runtime_tool_activity_display_title` 同语义。transcript
/// item 只携带 title 字符串（无完整 activity/kind 回退面），故在本模块内以字符串
/// 等价实现。
fn normalized_tool_entry_title(title: &str) -> &str {
    let title = title.trim();
    title
        .strip_prefix("Shell:")
        .map(str::trim_start)
        .unwrap_or(title)
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
/// User 条目是发起指令而非 agent 活动，不进入折叠区。返回（最近 entries,
/// 被折叠的更早条数）。
pub(super) fn agents_activity_fold_entries(items: &[AgentTranscriptItem]) -> (Vec<String>, usize) {
    let eligible: Vec<String> = items.iter().filter_map(activity_fold_entry_text).collect();
    let hidden = eligible
        .len()
        .saturating_sub(AGENTS_ACTIVITY_FOLD_ENTRY_COUNT);
    (eligible[hidden..].to_vec(), hidden)
}

/// 单条活动的单行摘要：tool 条目用剥传输前缀后的归一化 title（空 title 回退
/// content 首行），assistant 条目取 plain-text 首行（剥行内 markdown 强调标记，
/// 不泄漏 raw markdown）；多行内容只取首个非空行，空内容条目整体跳过。
fn activity_fold_entry_text(item: &AgentTranscriptItem) -> Option<String> {
    let entry = match item {
        AgentTranscriptItem::Tool { title, content } => {
            let title = normalized_tool_entry_title(title);
            if title.is_empty() {
                first_non_empty_line(content)?.to_string()
            } else {
                title.to_string()
            }
        }
        AgentTranscriptItem::Assistant { content } => {
            let line = first_non_empty_line(content)?;
            let plain = strip_inline_markdown_emphasis(line);
            if plain.is_empty() {
                line.to_string()
            } else {
                plain
            }
        }
        AgentTranscriptItem::User { .. } => return None,
    };
    Some(crate::status_line::truncate_display_width_with_ellipsis(
        &entry,
        AGENTS_ACTIVITY_FOLD_ENTRY_CACHE_WIDTH,
    ))
}

/// 多行内容只取首个非空行（trim 后）。
fn first_non_empty_line(text: &str) -> Option<&str> {
    text.lines().map(str::trim).find(|line| !line.is_empty())
}

/// 行内 markdown 强调标记的朴素清理：只剥 `**`/`__`/`` ` ``，不引入 markdown
/// 解析；单字符 `*`/`_` 保留（避免误伤 snake_case 词）。
fn strip_inline_markdown_emphasis(line: &str) -> String {
    line.replace("**", "").replace("__", "").replace('`', "")
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

/// transcript surface 的正文框架高度：surface chrome 与全屏列表同构——标题行（承担
/// list header 角色）+ 分割线 + page rule + footer，共用 `FULLSCREEN_LIST_CHROME_HEIGHT`
/// 预算；permission 区块再从该高度内扣除。渲染与输入侧共用本函数，滚动边界才一致。
pub(super) fn agents_panel_surface_frame_height(height: u16) -> usize {
    usize::from(height.saturating_sub(crate::fullscreen_list_chrome::FULLSCREEN_LIST_CHROME_HEIGHT))
        .max(1)
}

/// token usage 的列标签：k/m 小写缩放，取整形态与 context budget / spinner 的
/// token 缩放一致（tenths 四舍五入、`.0` 省略）；百万级以上进 m 档，封顶
/// `999.9m` 不再加宽标签。不带单位后缀（列头已表达语义）。
pub(super) fn format_agent_token_usage(token_usage: usize) -> String {
    if token_usage < 1_000 {
        return token_usage.to_string();
    }
    if token_usage < 1_000_000 {
        let tenths = (token_usage.saturating_mul(10).saturating_add(500)) / 1_000;
        return format_scaled_token_tenths(tenths, 'k');
    }
    // 封顶保证标签宽度有稳定上界，极端累计值不再加宽列。
    let tenths = ((token_usage.saturating_mul(10).saturating_add(500_000)) / 1_000_000).min(9_999);
    format_scaled_token_tenths(tenths, 'm')
}

/// tenths（十分位计数）转 `N` / `N.d` + 单位；`.0` 省略小数。
fn format_scaled_token_tenths(tenths: usize, unit: char) -> String {
    let whole = tenths / 10;
    let fraction = tenths % 10;
    if fraction == 0 {
        format!("{whole}{unit}")
    } else {
        format!("{whole}.{fraction}{unit}")
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
