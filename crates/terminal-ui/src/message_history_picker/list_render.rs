use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Clear, Paragraph, Widget},
};

use runtime_domain::session::MessageHistoryRow;

use crate::{
    Model,
    display_width::display_width,
    fullscreen_list_chrome::{fullscreen_list_chrome_rects, fullscreen_list_page_size_for_height},
    message_history_picker::MessageHistoryPickerState,
    relative_age::{
        RELATIVE_AGE_LIST_BEFORE_DOT_WIDTH, RELATIVE_AGE_LIST_COLUMN_WIDTH,
        relative_age_label_fixed_column,
    },
    render_frame::RenderFrame,
    search_highlight::{highlighted_substring_spans, search_match_style},
    status_line::truncate_display_width_with_ellipsis,
    styled_text::render_line_with_full_width_background,
    theme::{
        TerminalPalette, build_page_rule, command_accent_text_style, primary_text_style,
        subtle_rule_line, surface_text_style, tertiary_text_style,
    },
};

pub(super) const MESSAGE_HISTORY_BODY_LEFT_PADDING: &str = "  ";
const MESSAGE_HISTORY_BODY_HORIZONTAL_PADDING: usize = MESSAGE_HISTORY_BODY_LEFT_PADDING.len();
const MESSAGE_HISTORY_TIME_GAP_WIDTH: usize = 1;

impl Model {
    pub(crate) fn render_message_history_picker_list(
        &mut self,
        frame: &mut RenderFrame<'_>,
        area: Rect,
    ) {
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
                message_history_picker_list_footer_hint(area.width),
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
        let title = format!(
            "Message history ({} of {})",
            state.selected_position_label(),
            state.filtered_count()
        );
        let title_width = width.saturating_sub(2).max(1);
        let mut spans = vec![
            Span::raw("  "),
            Span::styled(
                truncate_display_width_with_ellipsis(&title, title_width),
                primary_text_style(self.palette).bold(),
            ),
        ];
        if state.is_searching() || !state.search_query().is_empty() {
            spans.push(Span::styled(" · ", primary_text_style(self.palette).bold()));
            spans.push(Span::styled(
                "Search:",
                command_accent_text_style(self.palette).bold(),
            ));
            spans.push(Span::styled(
                format!(" {}", state.search_query()),
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
        let width = width.max(1);
        let mut lines = Vec::new();

        if state.is_loading {
            lines.push(Line::styled(
                "  Loading message history...",
                tertiary_text_style(self.palette),
            ));
        } else if let Some(error) = state.error.as_deref() {
            lines.push(Line::styled(
                truncate_display_width_with_ellipsis(&format!("  {error}"), width),
                tertiary_text_style(self.palette),
            ));
        } else if !state.has_rows() {
            lines.push(Line::styled(
                "  No sent messages yet",
                tertiary_text_style(self.palette),
            ));
        } else if !state.has_filtered_rows() {
            let empty_message = if state.search_query().is_empty() {
                "  No sent messages yet"
            } else {
                "  No messages match search"
            };
            lines.push(Line::styled(
                truncate_display_width_with_ellipsis(empty_message, width),
                tertiary_text_style(self.palette),
            ));
        } else {
            let page_start = state.page_start(page_size);
            for (visible_position, row_index) in state.page_indices(page_size).enumerate() {
                let Some(row) = state.row(row_index) else {
                    continue;
                };
                let absolute_position = page_start + visible_position;
                lines.push(self.message_history_picker_row_line(
                    row,
                    width,
                    state.is_selected_visible_position(absolute_position),
                    absolute_position.is_multiple_of(2),
                    state.opened_at_ms,
                    state.search_query(),
                ));
            }
        }

        lines.truncate(body_height);
        lines
    }

    fn message_history_picker_row_line(
        &self,
        row: &MessageHistoryRow,
        width: usize,
        is_cursor: bool,
        is_even: bool,
        opened_at_ms: i64,
        search_query: &str,
    ) -> Line<'static> {
        let timestamp = relative_age_label_fixed_column(
            opened_at_ms,
            row.ts,
            RELATIVE_AGE_LIST_COLUMN_WIDTH,
            RELATIVE_AGE_LIST_BEFORE_DOT_WIDTH,
        );
        let prefix_width = display_width(MESSAGE_HISTORY_BODY_LEFT_PADDING)
            + RELATIVE_AGE_LIST_COLUMN_WIDTH
            + MESSAGE_HISTORY_TIME_GAP_WIDTH;
        let text_width = width
            .saturating_sub(prefix_width)
            .saturating_sub(MESSAGE_HISTORY_BODY_HORIZONTAL_PADDING);
        let row_style = message_history_picker_row_style(self.palette, is_even);
        let text_style = message_history_picker_content_style(self.palette, is_cursor);
        let summary_style = if is_cursor {
            text_style.bg(Color::Reset).add_modifier(Modifier::REVERSED)
        } else {
            text_style
        };
        let summary = truncate_display_width_with_ellipsis(&row.text, text_width);
        let highlighted_summary_style =
            message_history_picker_match_style(self.palette, summary_style, is_cursor, is_even);
        let mut spans = vec![
            Span::raw(MESSAGE_HISTORY_BODY_LEFT_PADDING),
            Span::styled(timestamp, tertiary_text_style(self.palette)),
            Span::raw(" "),
        ];
        spans.extend(highlighted_substring_spans(
            &summary,
            search_query,
            summary_style,
            highlighted_summary_style,
        ));

        Line::from(spans).style(row_style)
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

fn message_history_picker_content_style(palette: TerminalPalette, is_cursor: bool) -> Style {
    if is_cursor {
        primary_text_style(palette).bold()
    } else {
        primary_text_style(palette)
    }
}

fn message_history_picker_row_style(palette: TerminalPalette, is_even: bool) -> Style {
    if is_even {
        surface_text_style(palette)
    } else {
        Style::new()
    }
}

fn message_history_picker_match_style(
    palette: TerminalPalette,
    base_style: Style,
    is_cursor: bool,
    is_even: bool,
) -> Style {
    // 斑马纹偶数行本身已经使用 surface 背景；继续叠同色背景会丢失可见对比。
    if !is_cursor && is_even && palette.surface.is_some() {
        return base_style.reversed();
    }

    search_match_style(base_style, palette.surface)
}

fn message_history_picker_list_footer_hint(width: u16) -> String {
    if width < 90 {
        "  Esc close · Space preview · Enter recall · c copy · / search · j/k · h/l page"
            .to_string()
    } else {
        "  Esc close · Space preview · Enter recall · c copy · / search · ↑/↓/j/k move · ←/→/h/l page"
            .to_string()
    }
}
