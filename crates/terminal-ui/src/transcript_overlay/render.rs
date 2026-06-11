use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::Line,
    widgets::Paragraph,
};

use crate::{
    Model,
    message::assistant_message_visual_inset,
    render_frame::RenderFrame,
    styled_text::{line_to_plain_text, render_line_with_full_width_background},
    theme::{TerminalPalette, muted_text_style, tertiary_text_style},
    transcript::{Transcript, TranscriptItem, TranscriptItemMetricsIndex},
};

const FOOTER_HINT: &str = "  Esc close · ↑↓ scroll · PgUp/PgDn page · Home/End jump";
const MESSAGE_REVISIT_FOOTER_HINT: &str =
    "  Enter edit · ← older · → newer · ↑↓ scroll · Esc close";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TranscriptOverlayProgressStyle {
    Percentage,
    Page,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct TranscriptOverlayRenderOptions<'a> {
    pub(crate) palette: TerminalPalette,
    pub(crate) content_height: usize,
    pub(crate) footer_hint: &'a str,
    pub(crate) progress_style: TranscriptOverlayProgressStyle,
}

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

/// 右对齐页码的分隔线，保持和百分比分隔线相同的位置与视觉重量。
pub(crate) fn build_page_rule(
    width: u16,
    page_number: usize,
    page_count: usize,
    palette: TerminalPalette,
) -> Line<'static> {
    let width = width as usize;
    let compact_label = format!(" {page_number}/{page_count} ");
    let full_label = format!(" Page {page_number}/{page_count} ");
    let label = if width >= 24 {
        full_label
    } else {
        compact_label
    };
    let label_len = label.chars().count();
    let right_pad = 2;

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

pub(crate) fn render_transcript_overlay_view(
    frame: &mut RenderFrame<'_>,
    area: Rect,
    transcript: &mut Transcript,
    overlay: &mut super::TranscriptOverlayState,
    options: TranscriptOverlayRenderOptions<'_>,
) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    let palette = options.palette;
    let content_height = options
        .content_height
        .min(area.height.saturating_sub(2) as usize);
    let metrics_index = transcript.progressive_item_metrics_index();
    let total_lines = metrics_index.line_count;
    let startup_banner_lines =
        transcript_overlay_startup_banner_lines_for_index(transcript, &metrics_index);
    let effective_total = total_lines.saturating_sub(startup_banner_lines);
    let max_offset = effective_total.saturating_sub(content_height);
    let mut scroll_offset = overlay.scroll_offset.min(max_offset);
    let highlight_item_index = overlay.highlight_item_index;

    if effective_total > 0 && content_height > 0 {
        let mut window = transcript
            .materialize_line_window(startup_banner_lines + scroll_offset, content_height);
        let exact_startup_banner_lines =
            transcript_overlay_startup_banner_lines_for_index(transcript, &window.index);
        let exact_total = window.index.line_count;
        let exact_effective_total = exact_total.saturating_sub(exact_startup_banner_lines);
        let exact_max_offset = exact_effective_total.saturating_sub(content_height);
        let exact_scroll_offset = scroll_offset.min(exact_max_offset);
        if exact_startup_banner_lines != startup_banner_lines
            || exact_scroll_offset != scroll_offset
        {
            scroll_offset = exact_scroll_offset;
            window = transcript.materialize_line_window(
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
                    message_revisit_highlight_line(line.line, palette)
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
            render_line_with_full_width_background(&line_content, line_rect, frame.buffer_mut());
            row += 1;
        }

        fill_empty_transcript_overlay_rows(frame, area, palette, row, content_bottom);
    } else if content_height > 0 {
        let content_bottom = area.y.saturating_add(content_height as u16);
        fill_empty_transcript_overlay_rows(frame, area, palette, area.y, content_bottom);
    }

    overlay.scroll_offset = scroll_offset;

    if area.height >= 2 {
        let rule_y = area.y + area.height - 2;
        let rule_line = match options.progress_style {
            TranscriptOverlayProgressStyle::Percentage => {
                let percentage = transcript_overlay_scroll_percentage(
                    effective_total,
                    content_height,
                    scroll_offset,
                    max_offset,
                );
                build_percentage_rule(area.width, percentage, palette)
            }
            TranscriptOverlayProgressStyle::Page => {
                let (page_number, page_count) = transcript_overlay_page_progress(
                    effective_total,
                    content_height,
                    scroll_offset,
                );
                build_page_rule(area.width, page_number, page_count, palette)
            }
        };
        frame.render_widget(
            Paragraph::new(rule_line),
            Rect::new(area.x, rule_y, area.width, 1),
        );
    }

    let footer_y = area.y + area.height - 1;
    let hint_style = tertiary_text_style(palette).add_modifier(Modifier::ITALIC);
    frame.render_widget(
        Paragraph::new(Line::styled(options.footer_hint.to_string(), hint_style)),
        Rect::new(area.x, footer_y, area.width, 1),
    );
}

impl Model {
    /// `render_transcript_overlay` 将完整对话历史渲染为全屏覆盖层。
    pub(crate) fn render_transcript_overlay(&mut self, frame: &mut RenderFrame<'_>, area: Rect) {
        let palette = self.palette;
        let content_height = area.height.saturating_sub(2) as usize; // 1 rule + 1 footer
        let footer_hint = if self.message_revisit.is_overlay_active {
            MESSAGE_REVISIT_FOOTER_HINT
        } else {
            FOOTER_HINT
        };
        let Some(overlay) = self.transcript_overlay.as_mut() else {
            return;
        };
        render_transcript_overlay_view(
            frame,
            area,
            &mut self.transcript,
            overlay,
            TranscriptOverlayRenderOptions {
                palette,
                content_height,
                footer_hint,
                progress_style: TranscriptOverlayProgressStyle::Percentage,
            },
        );
    }
}

fn transcript_overlay_startup_banner_lines_for_index(
    transcript: &Transcript,
    index: &TranscriptItemMetricsIndex,
) -> usize {
    let Some(first_pos) = index.visible_items.first() else {
        return 0;
    };
    let items = transcript.items_snapshot();
    let Some(first_item) = items.get(first_pos.item_index) else {
        return 0;
    };
    if matches!(first_item.as_ref(), TranscriptItem::StartupBanner(_)) {
        first_pos.total_line_count
    } else {
        0
    }
}

fn fill_empty_transcript_overlay_rows(
    frame: &mut RenderFrame<'_>,
    area: Rect,
    palette: TerminalPalette,
    mut row: u16,
    content_bottom: u16,
) {
    let fill_style = muted_text_style(palette);
    while row < content_bottom {
        frame.render_widget(
            Paragraph::new(Line::styled("~", fill_style)),
            Rect::new(area.x, row, area.width, 1),
        );
        row += 1;
    }
}

fn transcript_overlay_scroll_percentage(
    effective_total: usize,
    content_height: usize,
    scroll_offset: usize,
    max_offset: usize,
) -> usize {
    if effective_total == 0 || content_height >= effective_total || max_offset == 0 {
        return 0;
    }

    ((scroll_offset * 100 + max_offset / 2) / max_offset).clamp(0, 100)
}

pub(crate) fn transcript_overlay_page_progress(
    effective_total: usize,
    content_height: usize,
    scroll_offset: usize,
) -> (usize, usize) {
    let page_size = content_height.max(1);
    if effective_total == 0 {
        return (1, 1);
    }

    let page_count = effective_total.saturating_sub(1) / page_size + 1;
    let max_offset = effective_total.saturating_sub(content_height);
    let page_number = if scroll_offset >= max_offset {
        page_count
    } else {
        scroll_offset / page_size + 1
    };
    (page_number.clamp(1, page_count), page_count)
}

fn message_revisit_highlight_style() -> Style {
    Style::new().add_modifier(Modifier::REVERSED)
}

fn message_revisit_highlight_line(line: Line<'static>, palette: TerminalPalette) -> Line<'static> {
    let Some(surface) = palette.surface else {
        return line.patch_style(message_revisit_highlight_style());
    };
    let highlight_style = Style::new()
        .fg(palette.main)
        .bg(surface)
        .add_modifier(Modifier::REVERSED);

    if is_surface_half_block_line(&line, palette) {
        return solid_message_revisit_highlight_line(line, highlight_style);
    }

    restyle_message_revisit_highlight_line(line, highlight_style)
}

fn is_surface_half_block_line(line: &Line<'_>, palette: TerminalPalette) -> bool {
    let Some(surface) = palette.surface else {
        return false;
    };
    let text = line_to_plain_text(line);

    !text.is_empty()
        && text.chars().all(|character| matches!(character, '▄' | '▀'))
        && line.style.fg == Some(surface)
        && !matches!(line.style.bg, Some(background) if background != Color::Reset)
}

fn restyle_message_revisit_highlight_line(
    mut line: Line<'static>,
    highlight_style: Style,
) -> Line<'static> {
    line.style = Style::new();
    for span in &mut line.spans {
        span.style = highlight_style.add_modifier(span.style.add_modifier);
    }
    line
}

fn solid_message_revisit_highlight_line(
    line: Line<'static>,
    highlight_style: Style,
) -> Line<'static> {
    let width = line_to_plain_text(&line).chars().count().max(1);
    Line::styled(" ".repeat(width), highlight_style)
}
