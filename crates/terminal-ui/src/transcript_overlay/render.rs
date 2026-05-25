use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::Line,
    widgets::Paragraph,
};

use crate::{
    Model,
    message::assistant_message_visual_inset,
    styled_text::render_line_with_full_width_background,
    theme::{TerminalPalette, muted_text_style, tertiary_text_style},
};

const FOOTER_HINT: &str = "  Esc/q close · ↑↓ scroll · PgUp/PgDn page · Home/End jump";
const MESSAGE_REVISIT_FOOTER_HINT: &str =
    "  Enter edit · ← older · → newer · ↑↓ scroll · Esc/q close";

/// 右对齐百分比的分隔线。
/// 格式：左侧为连续的 ─，百分比靠右，百分比右侧固定两个 ─。
pub(crate) fn build_percentage_rule(
    width: u16,
    percentage: usize,
    palette: TerminalPalette,
) -> Line<'static> {
    let width = width as usize;
    let label = format!(" {percentage}% ");
    let label_len = label.chars().count();
    let right_pad = 2; // 百分比右侧固定两个 ─

    if width <= label_len + right_pad {
        return Line::styled(label, muted_text_style(palette));
    }

    let left_dash_count = width.saturating_sub(label_len + right_pad);
    let mut line = String::with_capacity(width);
    line.push_str(&"─".repeat(left_dash_count));
    line.push_str(&label);
    line.push_str(&"─".repeat(right_pad));

    Line::styled(line, muted_text_style(palette))
}

impl Model {
    /// `render_transcript_overlay` 将完整对话历史渲染为全屏覆盖层。
    pub(crate) fn render_transcript_overlay(&mut self, frame: &mut Frame<'_>, area: Rect) {
        if area.width == 0 || area.height == 0 {
            return;
        }

        let palette = self.palette;
        let content_height = area.height.saturating_sub(2) as usize; // 1 rule + 1 footer

        let Some(overlay) = &self.transcript_overlay else {
            return;
        };
        let highlight_item_index = overlay.highlight_item_index;

        let metrics_index = self.transcript.progressive_item_metrics_index();
        let total_lines = metrics_index.line_count;
        let startup_banner_lines =
            self.transcript_overlay_startup_banner_lines_for_index(&metrics_index);
        let effective_total = total_lines.saturating_sub(startup_banner_lines);

        // 限制滚动偏移（基于排除启动欢迎块后的有效行数）
        let max_offset =
            self.transcript_overlay_max_offset_for_index(&metrics_index, content_height);
        let mut scroll_offset = overlay.scroll_offset.min(max_offset);

        // 内容区
        if effective_total > 0 && content_height > 0 {
            let mut window = self
                .transcript
                .materialize_line_window(startup_banner_lines + scroll_offset, content_height);
            let exact_startup_banner_lines =
                self.transcript_overlay_startup_banner_lines_for_index(&window.index);
            let exact_total = window.index.line_count;
            let exact_effective_total = exact_total.saturating_sub(exact_startup_banner_lines);
            let exact_max_offset = exact_effective_total.saturating_sub(content_height);
            let exact_scroll_offset = scroll_offset.min(exact_max_offset);
            if exact_startup_banner_lines != startup_banner_lines
                || exact_scroll_offset != scroll_offset
            {
                scroll_offset = exact_scroll_offset;
                window = self.transcript.materialize_line_window(
                    exact_startup_banner_lines + scroll_offset,
                    content_height,
                );
            }

            let mut row = area.y;
            let content_bottom = area.y.saturating_add(content_height as u16);
            let inset = assistant_message_visual_inset(area.width);

            for line in window.lines {
                if row >= content_bottom {
                    break;
                }
                let line_content =
                    if highlight_item_index.is_some() && line.item_index == highlight_item_index {
                        line.line.patch_style(message_revisit_highlight_style())
                    } else {
                        line.line
                    };
                let line_rect =
                    if line.is_assistant && inset > 0 && area.width > inset.saturating_mul(2) {
                        Rect::new(
                            area.x + inset,
                            row,
                            area.width.saturating_sub(inset.saturating_mul(2)),
                            1,
                        )
                    } else {
                        Rect::new(area.x, row, area.width, 1)
                    };
                render_line_with_full_width_background(
                    &line_content,
                    line_rect,
                    frame.buffer_mut(),
                );
                row += 1;
            }

            // 用 ~ 填充剩余空白行
            let fill_style = muted_text_style(palette);
            while row < content_bottom {
                frame.render_widget(
                    Paragraph::new(Line::styled("~", fill_style)),
                    Rect::new(area.x, row, area.width, 1),
                );
                row += 1;
            }
        } else if content_height > 0 {
            // 空 transcript：整片内容区用 ~ 填充
            let fill_style = muted_text_style(palette);
            for r in area.y..area.y.saturating_add(content_height as u16) {
                frame.render_widget(
                    Paragraph::new(Line::styled("~", fill_style)),
                    Rect::new(area.x, r, area.width, 1),
                );
            }
        }

        // 底部百分比分隔线（百分比在右侧，基于排除启动欢迎块后的有效行数）
        let percentage = if effective_total == 0 || content_height >= effective_total {
            0usize
        } else {
            ((scroll_offset * 100 + max_offset / 2) / max_offset).clamp(0, 100)
        };
        if area.height >= 2 {
            let rule_y = area.y + area.height - 2;
            let rule_line = build_percentage_rule(area.width, percentage, palette);
            frame.render_widget(
                Paragraph::new(rule_line),
                Rect::new(area.x, rule_y, area.width, 1),
            );
        }

        // 底部单行提示区（风格与 model_panel footer 一致）
        let footer_y = area.y + area.height - 1;
        let footer_hint = if self.message_revisit.is_overlay_active {
            MESSAGE_REVISIT_FOOTER_HINT
        } else {
            FOOTER_HINT
        };
        let hint_style = tertiary_text_style(palette).add_modifier(Modifier::ITALIC);
        frame.render_widget(
            Paragraph::new(Line::styled(footer_hint, hint_style)),
            Rect::new(area.x, footer_y, area.width, 1),
        );
    }
}

fn message_revisit_highlight_style() -> Style {
    Style::new().add_modifier(Modifier::REVERSED)
}
