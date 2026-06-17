use ratatui::{
    layout::Rect,
    style::Modifier,
    text::{Line, Span},
    widgets::{Clear, Paragraph, Widget},
};
use runtime_domain::session::SessionPickerRow;

use crate::{
    Model,
    fullscreen_list_chrome::fullscreen_list_chrome_rects,
    render_frame::RenderFrame,
    status_line::truncate_display_width_with_ellipsis,
    styled_text::render_line_with_full_width_background,
    theme::{
        build_page_rule, command_accent_text_style, primary_text_style, secondary_text_style,
        subtle_rule_line, tertiary_text_style,
    },
};

use super::{
    SESSION_PICKER_PROMPT_MARKER_WIDTH, SessionPickerState, session_picker_page_size_for_height,
};

impl Model {
    pub(crate) fn render_session_picker(&self, frame: &mut RenderFrame<'_>, area: Rect) {
        let Some(state) = self.session_picker.as_ref() else {
            return;
        };
        frame.render_widget(Clear, area);
        let Some(chrome) = fullscreen_list_chrome_rects(area) else {
            return;
        };

        let page_size = session_picker_page_size_for_height(area.height);

        frame.render_widget(
            Paragraph::new(self.session_picker_header_line(state, usize::from(area.width))),
            chrome.header,
        );

        frame.render_widget(
            Paragraph::new(session_picker_header_rule_line(
                usize::from(area.width),
                self.palette,
            )),
            chrome.header_rule,
        );

        let lines = self.session_picker_body_lines(
            state,
            usize::from(area.width),
            usize::from(chrome.body.height),
            page_size,
        );
        frame.render_widget(SessionPickerWidget { lines: &lines }, chrome.body);

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
                session_picker_footer_hint(area.width),
                tertiary_text_style(self.palette).add_modifier(Modifier::ITALIC),
            )),
            chrome.footer,
        );
    }

    fn session_picker_header_line(
        &self,
        state: &SessionPickerState,
        width: usize,
    ) -> Line<'static> {
        let width = width.max(1);
        let title = format!(
            "Resume Session ({} of {})",
            state.selected_position_label(),
            state.filtered_indices.len()
        );
        let title_width = width.saturating_sub(2).max(1);
        let mut spans = vec![
            Span::raw("  "),
            Span::styled(
                truncate_display_width_with_ellipsis(&title, title_width),
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

    fn session_picker_body_lines(
        &self,
        state: &SessionPickerState,
        width: usize,
        body_height: usize,
        page_size: usize,
    ) -> Vec<Line<'static>> {
        let width = width.max(1);
        let mut lines = Vec::new();

        if state.is_loading {
            lines.push(Line::styled(
                "Loading sessions...",
                tertiary_text_style(self.palette),
            ));
        } else if let Some(error) = state.error.as_deref() {
            lines.push(Line::styled(
                truncate_display_width_with_ellipsis(error, width),
                tertiary_text_style(self.palette),
            ));
        } else if state.filtered_indices.is_empty() {
            let empty_message = if state.search_query.is_empty() {
                "No sessions"
            } else {
                "No sessions match search"
            };
            lines.push(Line::styled(
                truncate_display_width_with_ellipsis(&format!("  {empty_message}"), width),
                tertiary_text_style(self.palette),
            ));
        } else {
            let page_start = state.page_start(page_size);
            for (visible_position, row_index) in state.page_indices(page_size).enumerate() {
                let row = &state.rows[row_index];
                let is_selected = page_start + visible_position == state.selected;
                lines.extend(self.session_picker_row_lines(
                    row,
                    width,
                    is_selected,
                    state.opened_at_ms,
                ));
                lines.push(Line::raw(""));
            }
        }

        lines.truncate(body_height);
        lines
    }

    fn session_picker_row_lines(
        &self,
        row: &SessionPickerRow,
        width: usize,
        is_selected: bool,
        opened_at_ms: i64,
    ) -> Vec<Line<'static>> {
        let marker_width = SESSION_PICKER_PROMPT_MARKER_WIDTH;
        let text_width = width.saturating_sub(marker_width);
        let title = if row.first_user_message.trim().is_empty() {
            row.title.as_str()
        } else {
            row.first_user_message.as_str()
        };
        let assistant = if row.last_assistant_message.trim().is_empty() {
            row.title.as_str()
        } else {
            row.last_assistant_message.as_str()
        };
        let title_style = if is_selected {
            primary_text_style(self.palette).bold()
        } else {
            secondary_text_style(self.palette)
        };

        vec![
            Line::from(vec![
                session_picker_prompt_block_span(is_selected, self.palette),
                Span::raw(" "),
                Span::styled(
                    truncate_display_width_with_ellipsis(title, text_width),
                    title_style,
                ),
            ]),
            Line::from(vec![
                session_picker_prompt_block_span(is_selected, self.palette),
                Span::raw(" "),
                Span::styled(
                    truncate_display_width_with_ellipsis(assistant, text_width),
                    secondary_text_style(self.palette),
                ),
            ]),
            Line::from(vec![
                session_picker_prompt_block_span(is_selected, self.palette),
                Span::raw(" "),
                Span::styled(
                    truncate_display_width_with_ellipsis(
                        &session_picker_meta_text(row, opened_at_ms),
                        text_width,
                    ),
                    tertiary_text_style(self.palette),
                ),
            ]),
        ]
    }
}

struct SessionPickerWidget<'a> {
    lines: &'a [Line<'static>],
}

impl Widget for SessionPickerWidget<'_> {
    fn render(self, area: Rect, buf: &mut ratatui::buffer::Buffer) {
        for (row, line) in self.lines.iter().take(usize::from(area.height)).enumerate() {
            let y = area.y + u16::try_from(row).unwrap_or(u16::MAX);
            render_line_with_full_width_background(line, Rect::new(area.x, y, area.width, 1), buf);
        }
    }
}

fn session_picker_prompt_block_span(
    is_selected: bool,
    palette: crate::theme::TerminalPalette,
) -> Span<'static> {
    if is_selected {
        Span::styled("█", command_accent_text_style(palette))
    } else {
        Span::raw(" ")
    }
}

fn session_picker_header_rule_line(
    width: usize,
    palette: crate::theme::TerminalPalette,
) -> Line<'static> {
    subtle_rule_line(width, palette)
}

fn session_picker_footer_hint(width: u16) -> &'static str {
    if width < 76 {
        "  Esc close · Type / to search · Enter · j/k · h/l page"
    } else {
        "  Esc close · Type / to search · Enter resume · ↑/↓/j/k move · ←/→/h/l page"
    }
}

fn session_picker_meta_text(row: &SessionPickerRow, now_ms: i64) -> String {
    session_picker_meta_text_at(row, now_ms)
}

pub(super) fn session_picker_meta_text_at(row: &SessionPickerRow, now_ms: i64) -> String {
    let mut parts = vec![
        format_session_age(row.updated_at_ms, now_ms),
        row.work_dir.clone(),
    ];
    if let Some(size_bytes) = row.size_bytes {
        parts.push(format_size(size_bytes));
    }
    parts.join(" · ")
}

fn format_session_age(updated_at_ms: i64, now_ms: i64) -> String {
    if updated_at_ms <= 0 || now_ms <= 0 {
        return "unknown".to_string();
    }

    let elapsed_ms = now_ms.saturating_sub(updated_at_ms).max(0);
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

    parts.join(" ")
}

fn format_size(size_bytes: u64) -> String {
    if size_bytes < 1024 {
        return format!("{size_bytes} B");
    }
    let kib = size_bytes as f64 / 1024.0;
    if kib < 1024.0 {
        return format!("{kib:.1} KiB");
    }
    let mib = kib / 1024.0;
    format!("{mib:.1} MiB")
}
