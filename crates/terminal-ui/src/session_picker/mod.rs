use std::time::{SystemTime, UNIX_EPOCH};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    layout::Rect,
    style::Modifier,
    text::{Line, Span},
    widgets::{Clear, Paragraph, Widget},
};
use runtime_domain::session::SessionPickerRow;

use crate::{
    AppEffect, Model,
    display_width::display_width,
    render_frame::RenderFrame,
    status_line::truncate_display_width_with_ellipsis,
    styled_text::render_line_with_full_width_background,
    theme::{
        command_accent_text_style, muted_text_style, primary_text_style, secondary_text_style,
        subtle_rule_line, tertiary_text_style,
    },
};

#[cfg(test)]
mod tests;

const SESSION_PICKER_ROW_HEIGHT: usize = 4;
const SESSION_PICKER_PROMPT_MARKER_WIDTH: usize = 2;
const SESSION_PICKER_HEADER_HEIGHT: u16 = 1;
const SESSION_PICKER_HEADER_RULE_HEIGHT: u16 = 1;
const SESSION_PICKER_PAGE_RULE_HEIGHT: u16 = 1;
const SESSION_PICKER_FOOTER_HEIGHT: u16 = 1;
const SESSION_PICKER_CHROME_HEIGHT: u16 = SESSION_PICKER_HEADER_HEIGHT
    + SESSION_PICKER_HEADER_RULE_HEIGHT
    + SESSION_PICKER_PAGE_RULE_HEIGHT
    + SESSION_PICKER_FOOTER_HEIGHT;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct SessionPickerState {
    rows: Vec<SessionPickerRow>,
    filtered_indices: Vec<usize>,
    selected: usize,
    selected_session_id: Option<String>,
    opened_at_ms: i64,
    search_query: String,
    is_searching: bool,
    is_loading: bool,
    error: Option<String>,
}

impl Model {
    pub(crate) fn move_session_picker_selection(&mut self, direction: isize) {
        if let Some(state) = self.session_picker.as_mut() {
            state.move_selection(direction);
        }
    }

    pub(crate) fn session_picker_active(&self) -> bool {
        self.session_picker.is_some()
    }

    pub(crate) fn open_session_picker_loading(&mut self) {
        self.open_session_picker_loading_at(current_unix_time_ms());
    }

    pub(crate) fn open_session_picker_loading_at(&mut self, opened_at_ms: i64) {
        self.session_picker = Some(SessionPickerState {
            is_loading: true,
            opened_at_ms,
            ..SessionPickerState::default()
        });
    }

    pub(crate) fn apply_session_picker_rows(&mut self, rows: Vec<SessionPickerRow>) {
        let mut state = self.session_picker.take().unwrap_or_default();
        state.rows = rows;
        state.is_loading = false;
        state.error = None;
        state.apply_filter();
        self.session_picker = Some(state);
    }

    pub(crate) fn handle_session_picker_key(&mut self, key: KeyEvent) -> Option<Option<AppEffect>> {
        if !self.session_picker_active() {
            return None;
        }

        let is_searching = self
            .session_picker
            .as_ref()
            .is_some_and(|state| state.is_searching);

        match key.code {
            KeyCode::Esc => {
                if let Some(state) = self.session_picker.as_mut()
                    && state.exit_search()
                {
                    return Some(None);
                }
                self.session_picker = None;
                Some(None)
            }
            KeyCode::Char(character) if is_searching && is_session_picker_search_text_key(&key) => {
                if let Some(state) = self.session_picker.as_mut() {
                    state.push_search_character(character);
                }
                Some(None)
            }
            KeyCode::Up => {
                if let Some(state) = self.session_picker.as_mut() {
                    state.move_selection(-1);
                }
                Some(None)
            }
            KeyCode::Down => {
                if let Some(state) = self.session_picker.as_mut() {
                    state.move_selection(1);
                }
                Some(None)
            }
            KeyCode::Left => {
                let page_size = self.session_picker_page_size();
                if let Some(state) = self.session_picker.as_mut() {
                    state.move_page(-1, page_size);
                }
                Some(None)
            }
            KeyCode::Right => {
                let page_size = self.session_picker_page_size();
                if let Some(state) = self.session_picker.as_mut() {
                    state.move_page(1, page_size);
                }
                Some(None)
            }
            KeyCode::Char('k') if key.modifiers.is_empty() => {
                if let Some(state) = self.session_picker.as_mut() {
                    state.move_selection(-1);
                }
                Some(None)
            }
            KeyCode::Char('j') if key.modifiers.is_empty() => {
                if let Some(state) = self.session_picker.as_mut() {
                    state.move_selection(1);
                }
                Some(None)
            }
            KeyCode::Char('h') if key.modifiers.is_empty() => {
                let page_size = self.session_picker_page_size();
                if let Some(state) = self.session_picker.as_mut() {
                    state.move_page(-1, page_size);
                }
                Some(None)
            }
            KeyCode::Char('l') if key.modifiers.is_empty() => {
                let page_size = self.session_picker_page_size();
                if let Some(state) = self.session_picker.as_mut() {
                    state.move_page(1, page_size);
                }
                Some(None)
            }
            KeyCode::Backspace => {
                if let Some(state) = self.session_picker.as_mut() {
                    state.backspace_search();
                }
                Some(None)
            }
            KeyCode::Char('u')
                if key.modifiers.contains(KeyModifiers::CONTROL)
                    && !key.modifiers.contains(KeyModifiers::ALT) =>
            {
                if let Some(state) = self.session_picker.as_mut() {
                    state.clear_search();
                }
                Some(None)
            }
            KeyCode::Enter => {
                let selected_session_id = self
                    .session_picker
                    .as_ref()
                    .and_then(SessionPickerState::selected_row)
                    .map(|row| row.session_id.clone());
                if let Some(session_id) = selected_session_id {
                    self.session_picker = None;
                    return Some(Some(AppEffect::ResumeSession { session_id }));
                }
                Some(None)
            }
            KeyCode::Char('/') if key.modifiers.is_empty() => {
                if let Some(state) = self.session_picker.as_mut() {
                    state.is_searching = true;
                }
                Some(None)
            }
            KeyCode::Char(' ') if key.modifiers.is_empty() => {
                let selected_session_id = self
                    .session_picker
                    .as_ref()
                    .and_then(SessionPickerState::selected_row)
                    .map(|row| row.session_id.clone());
                selected_session_id
                    .map(|session_id| Some(AppEffect::OpenSessionPreview { session_id }))
                    .map(Some)
                    .unwrap_or(Some(None))
            }
            _ => Some(None),
        }
    }

    pub(crate) fn render_session_picker(&self, frame: &mut RenderFrame<'_>, area: Rect) {
        let Some(state) = self.session_picker.as_ref() else {
            return;
        };
        frame.render_widget(Clear, area);
        if area.is_empty() || area.height < SESSION_PICKER_CHROME_HEIGHT {
            return;
        }

        let body_height = area.height.saturating_sub(SESSION_PICKER_CHROME_HEIGHT);
        let page_size = session_picker_page_size_for_height(area.height);
        let header_area = Rect::new(area.x, area.y, area.width, SESSION_PICKER_HEADER_HEIGHT);
        let header_rule_area = Rect::new(
            area.x,
            area.y + SESSION_PICKER_HEADER_HEIGHT,
            area.width,
            SESSION_PICKER_HEADER_RULE_HEIGHT,
        );
        let body_area = Rect::new(
            area.x,
            area.y + SESSION_PICKER_HEADER_HEIGHT + SESSION_PICKER_HEADER_RULE_HEIGHT,
            area.width,
            body_height,
        );
        let page_rule_area = Rect::new(
            area.x,
            area.y
                + area
                    .height
                    .saturating_sub(SESSION_PICKER_PAGE_RULE_HEIGHT + SESSION_PICKER_FOOTER_HEIGHT),
            area.width,
            SESSION_PICKER_PAGE_RULE_HEIGHT,
        );
        let footer_area = Rect::new(
            area.x,
            area.y + area.height.saturating_sub(SESSION_PICKER_FOOTER_HEIGHT),
            area.width,
            SESSION_PICKER_FOOTER_HEIGHT,
        );

        frame.render_widget(
            Paragraph::new(self.session_picker_header_line(state, usize::from(area.width))),
            header_area,
        );

        frame.render_widget(
            Paragraph::new(session_picker_header_rule_line(
                usize::from(area.width),
                self.palette,
            )),
            header_rule_area,
        );

        let lines = self.session_picker_body_lines(
            state,
            usize::from(area.width),
            usize::from(body_area.height),
        );
        frame.render_widget(SessionPickerWidget { lines: &lines }, body_area);

        frame.render_widget(
            Paragraph::new(build_session_picker_page_rule(
                area.width,
                state.page_number(page_size),
                state.page_count(page_size),
                self.palette,
            )),
            page_rule_area,
        );

        frame.render_widget(
            Paragraph::new(Line::styled(
                session_picker_footer_hint(area.width),
                tertiary_text_style(self.palette).add_modifier(Modifier::ITALIC),
            )),
            footer_area,
        );
    }

    fn session_picker_page_size(&self) -> usize {
        session_picker_page_size_for_height(self.height)
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
    ) -> Vec<Line<'static>> {
        let width = width.max(1);
        let page_size = session_picker_page_size_for_height(
            u16::try_from(body_height)
                .unwrap_or(u16::MAX)
                .saturating_add(SESSION_PICKER_CHROME_HEIGHT),
        );
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
            lines.push(Line::styled(
                if state.search_query.is_empty() {
                    "No sessions"
                } else {
                    "No sessions match search"
                },
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

impl SessionPickerState {
    fn apply_filter(&mut self) {
        let query = self.search_query.trim();
        self.filtered_indices = self
            .rows
            .iter()
            .enumerate()
            .filter_map(|(index, row)| {
                (query.is_empty() || session_picker_row_matches(row, query)).then_some(index)
            })
            .collect();
        self.restore_selected_session_or_clamp();
    }

    fn move_selection(&mut self, direction: isize) {
        if self.filtered_indices.is_empty() {
            self.selected = 0;
            self.selected_session_id = None;
            return;
        }
        let last = self.filtered_indices.len().saturating_sub(1);
        self.selected = if direction.is_negative() {
            self.selected.saturating_sub(direction.unsigned_abs())
        } else {
            self.selected.saturating_add(direction as usize).min(last)
        };
        self.sync_selected_session_id();
    }

    fn move_page(&mut self, direction: isize, page_size: usize) {
        if self.filtered_indices.is_empty() {
            self.selected = 0;
            self.selected_session_id = None;
            return;
        }
        let page_size = page_size.max(1);
        let current_page = self.selected / page_size;
        let last_page = self.filtered_indices.len().saturating_sub(1) / page_size;
        let next_page = if direction.is_negative() {
            current_page.saturating_sub(direction.unsigned_abs())
        } else {
            current_page
                .saturating_add(direction as usize)
                .min(last_page)
        };
        self.selected = (next_page * page_size).min(self.filtered_indices.len().saturating_sub(1));
        self.sync_selected_session_id();
    }

    fn push_search_character(&mut self, character: char) {
        self.search_query.push(character);
        self.apply_filter();
    }

    fn backspace_search(&mut self) {
        if self.search_query.pop().is_some() {
            self.apply_filter();
        }
    }

    fn clear_search(&mut self) -> bool {
        if self.search_query.is_empty() && !self.is_searching {
            return false;
        }
        let selected_row_index = self.selected_row_index();
        self.search_query.clear();
        self.apply_filter();
        self.select_filtered_row_index_or_session(selected_row_index);
        true
    }

    fn exit_search(&mut self) -> bool {
        if !self.is_searching && self.search_query.is_empty() {
            return false;
        }
        let selected_row_index = self.selected_row_index();
        let had_query = !self.search_query.is_empty();
        self.search_query.clear();
        self.is_searching = false;
        if had_query {
            self.apply_filter();
            self.select_filtered_row_index_or_session(selected_row_index);
        }
        true
    }

    fn selected_row(&self) -> Option<&SessionPickerRow> {
        let row_index = *self.filtered_indices.get(self.selected)?;
        self.rows.get(row_index)
    }

    fn selected_row_index(&self) -> Option<usize> {
        self.filtered_indices.get(self.selected).copied()
    }

    fn select_filtered_row_index_or_session(&mut self, row_index: Option<usize>) {
        if let Some(row_index) = row_index
            && let Some(position) = self
                .filtered_indices
                .iter()
                .position(|filtered_index| *filtered_index == row_index)
        {
            self.selected = position;
            self.sync_selected_session_id();
            return;
        }

        self.restore_selected_session_or_clamp();
    }

    fn restore_selected_session_or_clamp(&mut self) {
        if let Some(selected_session_id) = self.selected_session_id.as_deref()
            && let Some(position) = self.filtered_indices.iter().position(|row_index| {
                self.rows
                    .get(*row_index)
                    .is_some_and(|row| row.session_id == selected_session_id)
            })
        {
            self.selected = position;
            return;
        }

        self.selected = self
            .selected
            .min(self.filtered_indices.len().saturating_sub(1));
        self.sync_selected_session_id();
    }

    fn sync_selected_session_id(&mut self) {
        self.selected_session_id = self.selected_row().map(|row| row.session_id.clone());
    }

    fn page_start(&self, page_size: usize) -> usize {
        let page_size = page_size.max(1);
        self.selected / page_size * page_size
    }

    fn page_indices(&self, page_size: usize) -> impl Iterator<Item = usize> + '_ {
        let page_size = page_size.max(1);
        self.filtered_indices
            .iter()
            .skip(self.page_start(page_size))
            .take(page_size)
            .copied()
    }

    fn page_number(&self, page_size: usize) -> usize {
        if self.filtered_indices.is_empty() {
            return 1;
        }
        self.selected / page_size.max(1) + 1
    }

    fn page_count(&self, page_size: usize) -> usize {
        let page_size = page_size.max(1);
        self.filtered_indices.len().saturating_sub(1) / page_size + 1
    }

    fn selected_position_label(&self) -> usize {
        if self.filtered_indices.is_empty() {
            0
        } else {
            self.selected + 1
        }
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

fn session_picker_row_matches(row: &SessionPickerRow, query: &str) -> bool {
    contains_case_insensitive(&row.title, query)
        || contains_case_insensitive(&row.first_user_message, query)
        || contains_case_insensitive(&row.last_assistant_message, query)
        || contains_case_insensitive(&row.work_dir, query)
        || row
            .model
            .as_deref()
            .is_some_and(|model| contains_case_insensitive(model, query))
}

fn is_session_picker_search_text_key(key: &KeyEvent) -> bool {
    let KeyCode::Char(character) = key.code else {
        return false;
    };
    !character.is_ascii_control()
        && !key.modifiers.contains(KeyModifiers::CONTROL)
        && !key.modifiers.contains(KeyModifiers::ALT)
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

fn contains_case_insensitive(haystack: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return true;
    }
    if needle.is_ascii() {
        let needle_bytes = needle.as_bytes();
        return haystack
            .as_bytes()
            .windows(needle_bytes.len())
            .any(|window| window.eq_ignore_ascii_case(needle_bytes));
    }

    haystack.to_lowercase().contains(&needle.to_lowercase())
}

fn build_session_picker_page_rule(
    width: u16,
    page_number: usize,
    page_count: usize,
    palette: crate::theme::TerminalPalette,
) -> Line<'static> {
    let width = usize::from(width);
    let compact_label = format!(" {page_number}/{page_count} ");
    let full_label = format!(" Page {page_number}/{page_count} ");
    let label = if width >= 24 {
        full_label
    } else {
        compact_label
    };
    let label_width = display_width(&label);
    let right_pad = 2usize;

    if width <= label_width + right_pad {
        return Line::styled(label, muted_text_style(palette));
    }

    let left_dash_count = width.saturating_sub(label_width + right_pad);
    let mut line = String::with_capacity(width);
    line.push_str(&"─".repeat(left_dash_count));
    line.push_str(&label);
    line.push_str(&"─".repeat(right_pad));

    Line::styled(line, muted_text_style(palette))
}

fn session_picker_header_rule_line(
    width: usize,
    palette: crate::theme::TerminalPalette,
) -> Line<'static> {
    subtle_rule_line(width, palette)
}

fn session_picker_page_size_for_height(height: u16) -> usize {
    (usize::from(height.saturating_sub(SESSION_PICKER_CHROME_HEIGHT)) / SESSION_PICKER_ROW_HEIGHT)
        .max(1)
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

fn session_picker_meta_text_at(row: &SessionPickerRow, now_ms: i64) -> String {
    let mut parts = vec![
        format_session_age(row.updated_at_ms, now_ms),
        row.work_dir.clone(),
    ];
    if let Some(size_bytes) = row.size_bytes {
        parts.push(format_size(size_bytes));
    }
    parts.join(" · ")
}

fn current_unix_time_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_millis()).ok())
        .unwrap_or(i64::MAX)
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
