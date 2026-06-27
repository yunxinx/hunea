use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::Line,
    widgets::{Clear, Paragraph},
};

use super::{
    heatmap::allocate_heatmap_cells,
    segment_colors::context_budget_color_for_kind,
    state::{header_summary, segment_share_percent, sorted_legend_indices},
};
use crate::{
    Model,
    fullscreen_list_chrome::fullscreen_list_chrome_rects,
    render_frame::RenderFrame,
    status_line::truncate_display_width_with_ellipsis,
    theme::{subtle_rule_line, tertiary_text_style},
};
use runtime_domain::context_budget::SegmentKind;

impl Model {
    pub(crate) fn render_context_budget(&mut self, frame: &mut RenderFrame<'_>, area: Rect) {
        let Some(state) = self.context_budget.as_ref() else {
            return;
        };
        frame.render_widget(Clear, area);
        let Some(chrome) = fullscreen_list_chrome_rects(area) else {
            return;
        };

        let header_text = if let Some(snapshot) = state.snapshot.as_ref() {
            header_summary(&snapshot.model_id, snapshot.display)
        } else if state.loading {
            "Context budget · loading…".to_string()
        } else if let Some(error) = state.error.as_ref() {
            format!("Context budget · {error}")
        } else {
            "Context budget".to_string()
        };

        frame.render_widget(Paragraph::new(Line::from(header_text)), chrome.header);
        frame.render_widget(
            Paragraph::new(subtle_rule_line(usize::from(area.width), self.palette)),
            chrome.header_rule,
        );

        let body_h = chrome.body.height;
        let upper_h = body_h / 2;
        let lower_h = body_h.saturating_sub(upper_h);
        let upper = Rect::new(chrome.body.x, chrome.body.y, chrome.body.width, upper_h);
        let lower = Rect::new(
            chrome.body.x,
            chrome.body.y + upper_h,
            chrome.body.width,
            lower_h,
        );

        if let Some(snapshot) = state.snapshot.as_ref() {
            render_heatmap(frame.buffer_mut(), upper, snapshot, self.palette);
            render_legend(frame.buffer_mut(), lower, snapshot, self.palette);
        }

        frame.render_widget(
            Paragraph::new(Line::styled(
                "Esc close",
                tertiary_text_style(self.palette).add_modifier(Modifier::ITALIC),
            )),
            chrome.footer,
        );
    }
}

fn render_heatmap(
    buffer: &mut Buffer,
    area: Rect,
    snapshot: &runtime_domain::session::ContextBudgetSnapshotPayload,
    palette: crate::theme::TerminalPalette,
) {
    let width = usize::from(area.width);
    let height = usize::from(area.height);
    if width == 0 || height == 0 {
        return;
    }
    let total_cells = width.saturating_mul(height);
    let segments = ordered_segments_for_heatmap(snapshot);
    let cell_counts = allocate_heatmap_cells(&segments, total_cells);
    let mut cell_index = 0usize;
    for (segment, count) in segments.iter().zip(cell_counts.iter()) {
        let color = context_budget_color_for_kind(segment.kind, &palette);
        for _ in 0..*count {
            if cell_index >= total_cells {
                break;
            }
            let row = cell_index / width;
            let col = cell_index % width;
            let x = area.x.saturating_add(col as u16);
            let y = area.y.saturating_add(row as u16);
            if let Some(cell) = buffer.cell_mut((x, y)) {
                cell.set_char(' ');
                cell.set_fg(color);
                cell.set_bg(color);
            }
            cell_index += 1;
        }
    }
}

fn render_legend(
    buffer: &mut Buffer,
    area: Rect,
    snapshot: &runtime_domain::session::ContextBudgetSnapshotPayload,
    palette: crate::theme::TerminalPalette,
) {
    let width = usize::from(area.width);
    let height = usize::from(area.height);
    if width == 0 || height == 0 {
        return;
    }
    let col_width = width / 2;
    let indices = sorted_legend_indices(&snapshot.segments);
    let total = snapshot.total_estimated_tokens;
    let display = snapshot.display;
    let rows_per_col = height.max(1);
    for (rank, &seg_index) in indices.iter().enumerate() {
        let segment = &snapshot.segments[seg_index];
        let col = if rank < rows_per_col { 0 } else { 1 };
        let row = if rank < rows_per_col {
            rank
        } else {
            rank - rows_per_col
        };
        if row >= rows_per_col {
            break;
        }
        let x_offset = col * col_width;
        let percent = segment_share_percent(segment.estimated_tokens, total, display);
        let color = context_budget_color_for_kind(kind_from_tag(&segment.kind_tag), &palette);
        let label =
            truncate_display_width_with_ellipsis(&segment.label, col_width.saturating_sub(12));
        let line = format!("{label} {percent:.1}%");
        for (i, ch) in line.chars().enumerate() {
            if i >= col_width {
                break;
            }
            let x = area.x.saturating_add((x_offset + i) as u16);
            let y = area.y.saturating_add(row as u16);
            if let Some(cell) = buffer.cell_mut((x, y)) {
                let style = if i == 0 {
                    Style::default().fg(color)
                } else {
                    Style::default()
                };
                cell.set_char(ch);
                cell.set_style(style);
            }
        }
    }
}

fn ordered_segments_for_heatmap(
    snapshot: &runtime_domain::session::ContextBudgetSnapshotPayload,
) -> Vec<runtime_domain::context_budget::ContextSegment> {
    use runtime_domain::context_budget::ContextSegment;
    let mut segments: Vec<ContextSegment> = snapshot
        .segments
        .iter()
        .map(|s| ContextSegment {
            kind: kind_from_tag(&s.kind_tag),
            stack_order: s.stack_order,
            estimated_tokens: s.estimated_tokens,
            label: s.label.clone(),
        })
        .collect();
    segments.sort_by_key(|s| s.stack_order);
    segments
}

fn kind_from_tag(tag: &str) -> SegmentKind {
    match tag {
        "system" => SegmentKind::System,
        "user" => SegmentKind::UserMessage,
        "assistant" => SegmentKind::AssistantMessage,
        "tool_result" => SegmentKind::ToolResult,
        "reasoning" => SegmentKind::Reasoning,
        "tools" => SegmentKind::ToolDefinitions,
        _ => SegmentKind::System,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtime_domain::session::ContextBudgetDisplayPayload;

    #[test]
    fn header_relative_shows_question_mark_limit() {
        let text = header_summary(
            "qwen3",
            ContextBudgetDisplayPayload::Relative { used: 42_000 },
        );
        assert!(text.contains("qwen3"));
        assert!(text.contains("/ ?"));
    }

    #[test]
    fn header_absolute_shows_limit_and_percent() {
        let text = header_summary(
            "gpt-4o",
            ContextBudgetDisplayPayload::Absolute {
                limit: 128_000,
                used: 32_000,
                percent: 25.0,
            },
        );
        assert!(text.contains("128k"));
        assert!(text.contains("25.0%"));
    }
}
