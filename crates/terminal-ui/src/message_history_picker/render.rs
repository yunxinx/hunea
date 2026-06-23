use ratatui::{
    layout::Rect,
    style::Modifier,
    text::{Line, Span},
    widgets::{Clear, Paragraph, Widget},
};

use session_store::MessageHistoryRow;

use crate::{
    Model,
    display_width::display_width,
    fullscreen_list_chrome::{fullscreen_list_chrome_rects, fullscreen_list_page_size_for_height},
    message_history_picker::MessageHistoryPickerState,
    render_frame::RenderFrame,
    status_line::truncate_display_width_with_ellipsis,
    styled_text::render_line_with_full_width_background,
    theme::{
        build_page_rule, command_accent_text_style, primary_text_style, secondary_text_style,
        subtle_rule_line, tertiary_text_style,
    },
    transcript_overlay::{
        TranscriptOverlayProgressStyle, TranscriptOverlayRenderOptions,
        render_transcript_overlay_view,
    },
};

const MESSAGE_HISTORY_MARKER_WIDTH: usize = 2;
impl Model {
    pub(crate) fn render_message_history_picker(
        &mut self,
        frame: &mut RenderFrame<'_>,
        area: Rect,
    ) {
        if self.message_history_picker_preview_active() {
            self.render_message_history_picker_preview(frame, area);
            return;
        }

        let Some(state) = self.message_history_picker.as_ref() else {
            return;
        };
        frame.render_widget(Clear, area);
        let Some(chrome) = fullscreen_list_chrome_rects(area) else {
            return;
        };
        let page_size = fullscreen_list_page_size_for_height(area.height);
        let width = usize::from(area.width);

        frame.render_widget(
            Paragraph::new(self.message_history_picker_header_line(state, width)),
            chrome.header,
        );
        frame.render_widget(
            Paragraph::new(subtle_rule_line(width, self.palette)),
            chrome.header_rule,
        );

        let lines = self.message_history_picker_body_lines(
            state,
            width,
            usize::from(chrome.body.height),
            page_size,
        );
        frame.render_widget(MessageHistoryPickerWidget { lines: &lines }, chrome.body);

        frame.render_widget(
            Paragraph::new(build_page_rule(
                area.width,
                state.page_number(page_size),
                state.page_count(page_size),
                self.palette,
            )),
            chrome.page_rule,
        );
        frame.render_widget(
            Paragraph::new(Line::styled(
                message_history_picker_footer_hint(
                    area.width,
                    state.selected_position_label(),
                    state.filtered_indices.len(),
                ),
                tertiary_text_style(self.palette).add_modifier(Modifier::ITALIC),
            )),
            chrome.footer,
        );
    }

    fn message_history_picker_header_line(
        &self,
        state: &MessageHistoryPickerState,
        width: usize,
    ) -> Line<'static> {
        let title = "Message history";
        let title_width = width.saturating_sub(2).max(1);
        let mut spans = vec![
            Span::raw("  "),
            Span::styled(
                truncate_display_width_with_ellipsis(title, title_width),
                primary_text_style(self.palette).bold(),
            ),
        ];
        if state.is_searching || !state.search_query.is_empty() {
            spans.push(Span::styled(" · ", primary_text_style(self.palette).bold()));
            spans.push(Span::styled(
                "Search:",
                command_accent_text_style(self.palette).bold(),
            ));
            spans.push(Span::styled(
                format!(" {}", state.search_query),
                primary_text_style(self.palette).bold(),
            ));
        }
        Line::from(spans)
    }

    fn message_history_picker_body_lines(
        &self,
        state: &MessageHistoryPickerState,
        width: usize,
        body_height: usize,
        page_size: usize,
    ) -> Vec<Line<'static>> {
        let mut lines = Vec::new();
        if state.is_loading {
            lines.push(Line::styled("Loading…", secondary_text_style(self.palette)));
            lines.truncate(body_height);
            return lines;
        }
        if let Some(error) = state.error.as_deref() {
            lines.push(Line::styled(
                error.to_string(),
                secondary_text_style(self.palette),
            ));
            lines.truncate(body_height);
            return lines;
        }
        if state.rows.is_empty() {
            lines.push(Line::styled(
                "No sent messages yet.".to_string(),
                secondary_text_style(self.palette),
            ));
            lines.truncate(body_height);
            return lines;
        }
        if state.filtered_indices.is_empty() {
            let empty_message = if state.search_query.is_empty() {
                "No sent messages yet."
            } else {
                "No messages match search"
            };
            lines.push(Line::styled(
                truncate_display_width_with_ellipsis(empty_message, width),
                secondary_text_style(self.palette),
            ));
            lines.truncate(body_height);
            return lines;
        }

        let page_start = state.page_start(page_size);
        for (visible_position, row_index) in state.page_indices(page_size).enumerate() {
            let row = &state.rows[row_index];
            let is_selected = page_start + visible_position == state.selected;
            lines.push(self.message_history_picker_row_line(
                row,
                width,
                is_selected,
                state.opened_at_ms,
            ));
        }
        lines.truncate(body_height);
        lines
    }

    fn message_history_picker_row_line(
        &self,
        row: &MessageHistoryRow,
        width: usize,
        is_selected: bool,
        opened_at_ms: i64,
    ) -> Line<'static> {
        let timestamp = format_message_history_relative_age(row.ts, opened_at_ms);
        let timestamp_width = display_width(&timestamp).max(1);
        let text_budget = width
            .saturating_sub(MESSAGE_HISTORY_MARKER_WIDTH)
            .saturating_sub(timestamp_width)
            .saturating_sub(1);
        let text_style = if is_selected {
            primary_text_style(self.palette).bold()
        } else {
            secondary_text_style(self.palette)
        };
        Line::from(vec![
            message_history_marker_span(is_selected, self.palette),
            Span::raw(" "),
            Span::styled(
                truncate_display_width_with_ellipsis(&row.text, text_budget),
                text_style,
            ),
            Span::raw(" "),
            Span::styled(timestamp, tertiary_text_style(self.palette)),
        ])
    }

    fn render_message_history_picker_preview(&mut self, frame: &mut RenderFrame<'_>, area: Rect) {
        let palette = self.palette;
        let content_height = usize::from(area.height.saturating_sub(2).max(1));
        let Some(preview) = self
            .message_history_picker
            .as_mut()
            .and_then(|state| state.preview.as_mut())
        else {
            return;
        };
        render_transcript_overlay_view(
            frame,
            area,
            &mut preview.transcript_preview.transcript,
            &mut preview.transcript_preview.overlay,
            TranscriptOverlayRenderOptions {
                palette,
                content_height,
                footer_hint: message_history_picker_preview_footer_hint(area.width),
                progress_style: TranscriptOverlayProgressStyle::Page,
            },
        );
    }
}

fn message_history_picker_preview_footer_hint(width: u16) -> &'static str {
    if usize::from(width) >= 48 {
        "Esc/Space: back  c: copy  ↑↓/hl: scroll"
    } else {
        "Esc: back"
    }
}

struct MessageHistoryPickerWidget<'a> {
    lines: &'a [Line<'static>],
}

impl Widget for MessageHistoryPickerWidget<'_> {
    fn render(self, area: Rect, buf: &mut ratatui::buffer::Buffer) {
        for (row, line) in self.lines.iter().take(usize::from(area.height)).enumerate() {
            let y = area.y + u16::try_from(row).unwrap_or(u16::MAX);
            render_line_with_full_width_background(line, Rect::new(area.x, y, area.width, 1), buf);
        }
    }
}

fn message_history_marker_span(
    is_selected: bool,
    palette: crate::theme::TerminalPalette,
) -> Span<'static> {
    if is_selected {
        Span::styled("█", command_accent_text_style(palette))
    } else {
        Span::raw(" ")
    }
}

fn format_message_history_relative_age(ts_ms: i64, now_ms: i64) -> String {
    if ts_ms <= 0 || now_ms <= 0 {
        return "—".to_string();
    }
    let elapsed_ms = now_ms.saturating_sub(ts_ms).max(0);
    let mut elapsed_minutes = elapsed_ms / 60_000;
    if elapsed_minutes < 1 {
        return "now".to_string();
    }
    let elapsed_days = elapsed_minutes / (24 * 60);
    elapsed_minutes %= 24 * 60;
    let elapsed_hours = elapsed_minutes / 60;
    elapsed_minutes %= 60;

    let mut parts = Vec::new();
    if elapsed_days > 0 {
        parts.push(format!("{elapsed_days}d"));
    }
    if elapsed_hours > 0 {
        parts.push(format!("{elapsed_hours}h"));
    }
    if elapsed_minutes > 0 {
        parts.push(format!("{elapsed_minutes}m"));
    }
    if parts.is_empty() {
        "now".to_string()
    } else {
        format!("{} 前", parts.join(" "))
    }
}

fn message_history_picker_footer_hint(width: u16, position: usize, total: usize) -> String {
    let base = "Esc: close  ↑↓/jk: move  ←→/hl: page";
    let with_pos = if total > 0 {
        format!("{base}  {position}/{total}")
    } else {
        base.to_string()
    };
    if usize::from(width) < display_width(&with_pos) {
        if total > 0 {
            format!("Esc  {position}/{total}")
        } else {
            "Esc: close".to_string()
        }
    } else {
        with_pos
    }
}
