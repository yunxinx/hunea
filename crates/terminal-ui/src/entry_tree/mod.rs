use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    layout::Rect,
    text::{Line, Span},
    widgets::{Clear, Widget},
};
use runtime_domain::session::{SessionTreeEntry, SessionTreeEntryKind, SessionTreePayload};

use crate::{
    AppEffect, Model,
    display_width::display_width,
    render_frame::RenderFrame,
    status_line::truncate_display_width_with_ellipsis,
    styled_text::render_line_with_full_width_background,
    theme::{primary_text_style, secondary_text_style, tertiary_text_style},
};

#[cfg(test)]
mod tests;

const ENTRY_TREE_ROW_HEIGHT: usize = 2;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct EntryTreeState {
    entries: Vec<SessionTreeEntry>,
    filtered_indices: Vec<usize>,
    selected: usize,
    scroll: usize,
    search_query: String,
    is_loading: bool,
    error: Option<String>,
}

impl Model {
    pub(crate) fn entry_tree_active(&self) -> bool {
        self.entry_tree.is_some()
    }

    pub(crate) fn open_entry_tree_loading(&mut self) {
        self.entry_tree = Some(EntryTreeState {
            is_loading: true,
            ..EntryTreeState::default()
        });
    }

    pub(crate) fn apply_entry_tree_payload(&mut self, payload: SessionTreePayload) {
        let mut state = self.entry_tree.take().unwrap_or_default();
        state.entries = payload.entries;
        state.is_loading = false;
        state.error = None;
        state.apply_filter();
        self.entry_tree = Some(state);
    }

    pub(crate) fn handle_entry_tree_key(&mut self, key: KeyEvent) -> Option<Option<AppEffect>> {
        if !self.entry_tree_active() {
            return None;
        }

        match key.code {
            KeyCode::Esc => {
                self.entry_tree = None;
                Some(None)
            }
            KeyCode::Up => {
                let visible_rows = self.entry_tree_visible_rows();
                if let Some(state) = self.entry_tree.as_mut() {
                    state.move_selection(-1, visible_rows);
                }
                Some(None)
            }
            KeyCode::Down => {
                let visible_rows = self.entry_tree_visible_rows();
                if let Some(state) = self.entry_tree.as_mut() {
                    state.move_selection(1, visible_rows);
                }
                Some(None)
            }
            KeyCode::Backspace => {
                if let Some(state) = self.entry_tree.as_mut() {
                    state.search_query.pop();
                    state.apply_filter();
                }
                Some(None)
            }
            KeyCode::Enter => {
                let selected = self
                    .entry_tree
                    .as_ref()
                    .and_then(EntryTreeState::selected_entry);
                if let Some(entry) = selected {
                    let entry_id = entry.entry_id.clone();
                    let prefill = entry.rewind_prefill.clone();
                    self.entry_tree = None;
                    return Some(Some(AppEffect::SelectEntryRewind { entry_id, prefill }));
                }
                Some(None)
            }
            KeyCode::Char(character) if key.modifiers.is_empty() => {
                if let Some(state) = self.entry_tree.as_mut() {
                    state.search_query.push(character);
                    state.apply_filter();
                }
                Some(None)
            }
            _ => Some(None),
        }
    }

    pub(crate) fn render_entry_tree(&self, frame: &mut RenderFrame<'_>, area: Rect) {
        let Some(state) = self.entry_tree.as_ref() else {
            return;
        };
        frame.render_widget(Clear, area);
        let lines = self.entry_tree_lines(state, usize::from(area.width));
        frame.render_widget(EntryTreeWidget { lines: &lines }, area);
    }

    fn entry_tree_visible_rows(&self) -> usize {
        usize::from(self.height.saturating_sub(6)).max(1) / ENTRY_TREE_ROW_HEIGHT
    }

    fn entry_tree_lines(&self, state: &EntryTreeState, width: usize) -> Vec<Line<'static>> {
        let width = width.max(1);
        let mut lines = Vec::new();
        let count = state.filtered_indices.len();
        lines.push(Line::styled(
            truncate_display_width_with_ellipsis(&format!("Session Tree ({count})"), width),
            primary_text_style(self.palette).bold(),
        ));
        lines.push(Line::styled(
            truncate_display_width_with_ellipsis(&format!("Search: {}", state.search_query), width),
            secondary_text_style(self.palette),
        ));
        lines.push(Line::raw(""));

        if state.is_loading {
            lines.push(Line::styled(
                "Loading session tree...",
                tertiary_text_style(self.palette),
            ));
        } else if let Some(error) = state.error.as_deref() {
            lines.push(Line::styled(
                truncate_display_width_with_ellipsis(error, width),
                tertiary_text_style(self.palette),
            ));
        } else if state.filtered_indices.is_empty() {
            lines.push(Line::styled(
                "No entries",
                tertiary_text_style(self.palette),
            ));
        } else {
            for (visible_position, entry_index) in state
                .filtered_indices
                .iter()
                .skip(state.scroll)
                .take(self.entry_tree_visible_rows())
                .copied()
                .enumerate()
            {
                let entry = &state.entries[entry_index];
                let is_selected = state.scroll + visible_position == state.selected;
                lines.extend(self.entry_tree_row_lines(entry, width, is_selected));
            }
        }

        lines.push(Line::raw(""));
        lines.push(Line::styled(
            "Esc close · ↑/↓ move · type search · Enter select",
            secondary_text_style(self.palette),
        ));
        lines
    }

    fn entry_tree_row_lines(
        &self,
        entry: &SessionTreeEntry,
        width: usize,
        is_selected: bool,
    ) -> Vec<Line<'static>> {
        let marker = if is_selected { "> " } else { "  " };
        let branch = tree_branch_prefix(entry.depth, entry.is_current_leaf);
        let kind = entry_tree_kind_label(entry.kind);
        let prefix = format!("{marker}{branch}{kind} ");
        let prefix_width = display_width(&prefix);
        let text_width = width.saturating_sub(prefix_width);
        let title_style = if is_selected {
            primary_text_style(self.palette).bold()
        } else if entry.is_active_path {
            primary_text_style(self.palette)
        } else {
            secondary_text_style(self.palette)
        };

        vec![
            Line::from(vec![
                Span::styled(prefix, secondary_text_style(self.palette)),
                Span::styled(
                    truncate_display_width_with_ellipsis(&entry.label, text_width),
                    title_style,
                ),
            ]),
            Line::from(vec![
                Span::raw(" ".repeat(prefix_width)),
                Span::styled(
                    truncate_display_width_with_ellipsis(&entry.content, text_width),
                    tertiary_text_style(self.palette),
                ),
            ]),
        ]
    }
}

impl EntryTreeState {
    fn apply_filter(&mut self) {
        let query = self.search_query.to_lowercase();
        self.filtered_indices = self
            .entries
            .iter()
            .enumerate()
            .filter_map(|(index, entry)| {
                (query.is_empty() || entry_tree_entry_matches(entry, &query)).then_some(index)
            })
            .collect();
        self.selected = self
            .selected
            .min(self.filtered_indices.len().saturating_sub(1));
        self.scroll = self.scroll.min(self.selected);
    }

    fn move_selection(&mut self, direction: isize, visible_rows: usize) {
        if self.filtered_indices.is_empty() {
            self.selected = 0;
            self.scroll = 0;
            return;
        }
        let last = self.filtered_indices.len().saturating_sub(1);
        self.selected = if direction.is_negative() {
            self.selected.saturating_sub(direction.unsigned_abs())
        } else {
            self.selected.saturating_add(direction as usize).min(last)
        };
        let visible_rows = visible_rows.max(1);
        if self.selected < self.scroll {
            self.scroll = self.selected;
        } else if self.selected >= self.scroll + visible_rows {
            self.scroll = self.selected + 1 - visible_rows;
        }
    }

    fn selected_entry(&self) -> Option<&SessionTreeEntry> {
        let entry_index = *self.filtered_indices.get(self.selected)?;
        self.entries.get(entry_index)
    }
}

struct EntryTreeWidget<'a> {
    lines: &'a [Line<'static>],
}

impl Widget for EntryTreeWidget<'_> {
    fn render(self, area: Rect, buf: &mut ratatui::buffer::Buffer) {
        for (row, line) in self.lines.iter().take(usize::from(area.height)).enumerate() {
            let y = area.y + u16::try_from(row).unwrap_or(u16::MAX);
            render_line_with_full_width_background(line, Rect::new(area.x, y, area.width, 1), buf);
        }
    }
}

fn entry_tree_entry_matches(entry: &SessionTreeEntry, query: &str) -> bool {
    entry.entry_id.to_lowercase().contains(query)
        || entry.label.to_lowercase().contains(query)
        || entry.content.to_lowercase().contains(query)
        || entry_tree_kind_label(entry.kind).contains(query)
}

fn tree_branch_prefix(depth: usize, is_current_leaf: bool) -> String {
    let marker = if is_current_leaf { "*" } else { "-" };
    format!("{}{marker}", "  ".repeat(depth))
}

fn entry_tree_kind_label(kind: SessionTreeEntryKind) -> &'static str {
    match kind {
        SessionTreeEntryKind::Header => "header",
        SessionTreeEntryKind::User => "user",
        SessionTreeEntryKind::Assistant => "assistant",
        SessionTreeEntryKind::Tool => "tool",
        SessionTreeEntryKind::Reasoning => "reasoning",
        SessionTreeEntryKind::Config => "config",
        SessionTreeEntryKind::Leaf => "leaf",
        SessionTreeEntryKind::Other => "entry",
    }
}
