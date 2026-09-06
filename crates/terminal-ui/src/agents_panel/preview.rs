use crossterm::event::{KeyCode, KeyEvent};

use crate::{
    Model, agents_panel::agent_activity_summary_text, overlay_input_result::OverlayInputResult,
    relative_age::left_pad_display_width, status_line::truncate_display_width_with_ellipsis,
    transcript::wrap_plain_text,
};

use super::{
    AGENTS_ELAPSED_COLUMN_WIDTH, AgentsPanelSurface, agent_status_label, format_agent_elapsed_ms,
};

/// quick preview 无 committed answer 时的中性 empty state 首行。
pub(super) const AGENTS_PREVIEW_EMPTY_ANSWER_TEXT: &str = "No committed answer yet";
/// quick preview 左右留白（与 message history preview 一致）。
pub(super) const AGENTS_PREVIEW_HORIZONTAL_PADDING: usize = 2;

impl Model {
    pub(crate) fn agents_panel_preview_active(&self) -> bool {
        self.agents_panel
            .as_ref()
            .is_some_and(|panel| matches!(panel.surface, Some(AgentsPanelSurface::Preview { .. })))
    }

    /// preview 正文区高度：header 1 + page rule 1 + footer 1 之外的部分。
    pub(crate) fn agents_panel_preview_content_height(&self) -> usize {
        usize::from(self.height.saturating_sub(3).max(1))
    }

    pub(crate) fn move_agents_panel_preview_page(&mut self, direction: isize) {
        let page_size = self.agents_panel_preview_content_height();
        let line_count = self
            .agents_panel_preview_body_lines()
            .map_or(0, |lines| lines.len());
        if let Some(panel) = self.agents_panel.as_mut()
            && let Some(AgentsPanelSurface::Preview { scroll_offset, .. }) = panel.surface.as_mut()
        {
            let max_offset = line_count.saturating_sub(page_size);
            let delta = direction.signum() * isize::try_from(page_size).unwrap_or(0);
            let next = isize::try_from(*scroll_offset)
                .unwrap_or(0)
                .saturating_add(delta);
            let max_offset_isize = isize::try_from(max_offset).unwrap_or(0);
            *scroll_offset = usize::try_from(next.clamp(0, max_offset_isize)).unwrap_or(0);
        }
    }

    /// preview 正文行：committed answer 的按词换行，或中性 fallback 两行。
    fn agents_panel_preview_body_lines(&self) -> Option<Vec<String>> {
        let panel = self.agents_panel.as_ref()?;
        let AgentsPanelSurface::Preview { agent_id, .. } = panel.surface.as_ref()? else {
            return None;
        };
        let record = panel.agent_view_for_agent(*agent_id)?;
        let preview = record.snapshot.as_ref().map(|snapshot| &snapshot.preview)?;
        let wrap_width = agents_panel_preview_wrap_width(self.width);
        Some(match preview.latest_committed_answer.as_deref() {
            // 与 fallback 行同样携带左缩进，正文块对齐（wrap 宽度已预留该缩进）。
            Some(answer) => wrap_plain_text(answer, wrap_width, 0)
                .into_iter()
                .map(|line| format!("  {line}"))
                .collect(),
            None => vec![
                format!("  {AGENTS_PREVIEW_EMPTY_ANSWER_TEXT}"),
                format!(
                    "  Latest activity: {}",
                    agent_activity_summary_text(&preview.latest_activity)
                ),
            ],
        })
    }

    /// preview 正文行（渲染/滚动共用），含 loading/error 分支的中性文案。
    pub(super) fn agents_panel_preview_display_lines(&self) -> Option<Vec<String>> {
        let panel = self.agents_panel.as_ref()?;
        let AgentsPanelSurface::Preview { agent_id, .. } = panel.surface.as_ref()? else {
            return None;
        };
        let record = panel.agent_view_for_agent(*agent_id)?;
        if let Some(error) = record.error.as_deref() {
            return Some(vec![format!("  {error}")]);
        }
        if record.snapshot.is_none() {
            return Some(vec!["  Loading agent preview...".to_string()]);
        }
        self.agents_panel_preview_body_lines()
    }

    /// preview 的 Space/Esc 只返回 overview，不提供 cancel/interrupt/steer（R15 基础形态）。
    pub(super) fn handle_agents_panel_preview_key(&mut self, key: KeyEvent) -> OverlayInputResult {
        match key.code {
            KeyCode::Esc | KeyCode::Char(' ') if key.modifiers.is_empty() => {
                self.close_agents_panel_surface();
                OverlayInputResult::Handled
            }
            KeyCode::Left | KeyCode::Up | KeyCode::Char('h') if key.modifiers.is_empty() => {
                self.move_agents_panel_preview_page(-1);
                OverlayInputResult::Handled
            }
            KeyCode::Right | KeyCode::Down | KeyCode::Char('l') if key.modifiers.is_empty() => {
                self.move_agents_panel_preview_page(1);
                OverlayInputResult::Handled
            }
            _ => OverlayInputResult::Handled,
        }
    }
}

/// preview header 单行布局：status(固定列) + title(弹性截断) + elapsed(空间不足先隐藏)。
pub(super) struct AgentsPanelPreviewHeader {
    pub(super) status: String,
    pub(super) title: String,
    pub(super) elapsed: Option<String>,
}

/// preview header 的 title 保底宽度；低于此值时 elapsed 让位。
pub(super) const AGENTS_PREVIEW_TITLE_MIN_WIDTH: usize = 8;

pub(super) fn agents_panel_preview_header(
    status: runtime_domain::agent::AgentProjectionStatus,
    title: &str,
    elapsed_ms: Option<u64>,
    width: usize,
) -> AgentsPanelPreviewHeader {
    use crate::display_width::display_width;
    let status_budget = width.saturating_sub(AGENTS_PREVIEW_HORIZONTAL_PADDING);
    let status = super::pad_agents_status_column(agent_status_label(status), status_budget);
    let status_width = display_width(&status);
    let elapsed_label = elapsed_ms.map(|ms| {
        left_pad_display_width(&format_agent_elapsed_ms(ms), AGENTS_ELAPSED_COLUMN_WIDTH)
    });
    let title_budget = width
        .saturating_sub(
            AGENTS_PREVIEW_HORIZONTAL_PADDING
                + status_width
                + 1
                + AGENTS_PREVIEW_HORIZONTAL_PADDING,
        )
        .max(1);
    // 宽度不足时先隐藏 elapsed，再对 title 做 display-width 安全截断。
    let elapsed_width = elapsed_label.as_deref().map(display_width).unwrap_or(0);
    let show_elapsed = elapsed_label.is_some()
        && title_budget > elapsed_width + 1 + AGENTS_PREVIEW_TITLE_MIN_WIDTH;
    let title_width = if show_elapsed {
        title_budget - elapsed_width - 1
    } else {
        title_budget
    };
    AgentsPanelPreviewHeader {
        status,
        title: truncate_display_width_with_ellipsis(title, title_width),
        elapsed: show_elapsed.then(|| elapsed_label.unwrap_or_default()),
    }
}

pub(super) fn agents_panel_preview_wrap_width(window_width: u16) -> usize {
    usize::from(window_width)
        .saturating_sub(AGENTS_PREVIEW_HORIZONTAL_PADDING * 2)
        .max(1)
}
