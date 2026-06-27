use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::text::{Line, Span};

use super::{
    Model,
    display_width::display_width,
    file_search::{FileSearchMatch, common_path_completion_prefix},
    inline_panel::InlinePanelRenderResult,
    overlay_input_result::OverlayInputResult,
    path_resolve::{resolve_configured_current_dir, resolve_path_token},
    selection::{SelectableLineRange, selectable_range_for_plain_line},
    status_line::truncate_display_width_with_ellipsis,
    theme::{command_accent_text_style, secondary_text_style, tertiary_text_style},
};

const FILE_PICKER_INSET_WIDTH: usize = 2;
pub(super) const FILE_PICKER_POPUP_MIN_HEIGHT: u16 = 3;
pub(super) const FILE_PICKER_POPUP_MAX_HEIGHT: u16 = 21;

/// `FilePickerState` 保存 `@` 文件选择器的当前查询、结果和导航位置。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct FilePickerState {
    pub(crate) query: String,
    pub(crate) items: Vec<FileSearchMatch>,
    pub(crate) selected: usize,
    pub(crate) scroll: usize,
}

impl Model {
    pub(crate) fn file_picker_active(&self) -> bool {
        self.file_picker.is_some()
    }

    pub(crate) fn sync_file_picker_state(&mut self) {
        if self.blocks_composer_input() || self.command_panel_active() {
            self.close_file_picker();
            return;
        }

        let Some(query) = self.composer.current_at_token() else {
            self.close_file_picker();
            self.dismissed_file_picker_token = None;
            return;
        };

        if self.dismissed_file_picker_token.as_ref() == Some(&query) {
            self.close_file_picker();
            return;
        }

        let root = self.file_search_root();
        let items = self.file_search_cache.search(&root, &query);
        let visible_rows = self.file_picker_list_visible_rows();
        let previous = self.file_picker.as_ref();
        let query_changed = previous.is_none_or(|state| state.query != query);
        let mut selected = if query_changed {
            0
        } else {
            previous.map(|state| state.selected).unwrap_or(0)
        };
        let mut scroll = if query_changed {
            0
        } else {
            previous.map(|state| state.scroll).unwrap_or(0)
        };

        if items.is_empty() {
            selected = 0;
            scroll = 0;
        } else {
            selected = selected.min(items.len() - 1);
            scroll = clamp_picker_scroll(scroll, selected, items.len(), visible_rows);
        }

        self.file_picker = Some(FilePickerState {
            query,
            items,
            selected,
            scroll,
        });
    }

    pub(crate) fn handle_file_picker_key(&mut self, key: KeyEvent) -> OverlayInputResult {
        if !self.file_picker_active() {
            return OverlayInputResult::Ignored;
        }

        match key.code {
            KeyCode::Up if key.modifiers.is_empty() => {
                self.move_file_picker_selection(-1);
                OverlayInputResult::Handled
            }
            KeyCode::Down if key.modifiers.is_empty() => {
                self.move_file_picker_selection(1);
                OverlayInputResult::Handled
            }
            KeyCode::Char('p') if key.modifiers == KeyModifiers::CONTROL => {
                self.move_file_picker_selection(-1);
                OverlayInputResult::Handled
            }
            KeyCode::Char('n') if key.modifiers == KeyModifiers::CONTROL => {
                self.move_file_picker_selection(1);
                OverlayInputResult::Handled
            }
            KeyCode::Esc if key.modifiers.is_empty() => {
                self.dismiss_current_file_picker_token();
                self.close_file_picker();
                OverlayInputResult::Handled
            }
            KeyCode::Tab if key.modifiers.is_empty() => {
                self.complete_file_picker_common_prefix();
                OverlayInputResult::Handled
            }
            KeyCode::Enter if key.modifiers.is_empty() => {
                if self.current_file_picker_query_resolves_to_file() {
                    self.close_file_picker();
                    self.dismissed_file_picker_token = None;
                    return OverlayInputResult::Ignored;
                }
                if self.insert_selected_file_picker_path() {
                    return OverlayInputResult::Handled;
                }
                OverlayInputResult::Handled
            }
            _ => OverlayInputResult::Ignored,
        }
    }

    pub(crate) fn current_file_picker_render_result(&self) -> InlinePanelRenderResult {
        let Some(state) = self.file_picker.as_ref() else {
            return InlinePanelRenderResult::default();
        };

        let visible_rows = self.file_picker_list_visible_rows();
        let width = usize::from(self.width.max(1));
        let has_scrollbar = state.items.len() > visible_rows;
        let content_width = width.saturating_sub(usize::from(has_scrollbar && width > 1));
        let (lines, plain_lines, selectable) =
            self.render_file_picker_lines(state, content_width, visible_rows);

        InlinePanelRenderResult {
            lines,
            plain_lines,
            selectable,
            has_content: true,
        }
    }

    pub(crate) fn file_picker_list_visible_rows(&self) -> usize {
        usize::from(self.file_picker_popup_height.max(1))
    }

    fn render_file_picker_lines(
        &self,
        state: &FilePickerState,
        width: usize,
        visible_rows: usize,
    ) -> (Vec<Line<'static>>, Vec<String>, Vec<SelectableLineRange>) {
        let width = width.max(1);
        let visible_rows = visible_rows.max(1);
        let mut lines = Vec::with_capacity(visible_rows);
        let mut plain_lines = Vec::with_capacity(visible_rows);
        let mut selectable = Vec::with_capacity(visible_rows);

        if state.items.is_empty() {
            let plain_line = pad_display_width_right("  No files", width);
            lines.push(Line::styled(
                plain_line.clone(),
                tertiary_text_style(self.palette),
            ));
            plain_lines.push(plain_line.clone());
            selectable.push(selectable_range_for_plain_line(&plain_line));
            return (lines, plain_lines, selectable);
        }

        for row in 0..visible_rows {
            let index = state.scroll + row;
            let Some(item) = state.items.get(index) else {
                lines.push(Line::raw(""));
                plain_lines.push(String::new());
                selectable.push(SelectableLineRange::default());
                continue;
            };

            let selected = index == state.selected;
            let (line, plain_line) =
                self.render_file_picker_line(item, selected, width, &state.query);
            selectable.push(file_picker_selectable_range(&plain_line, width));
            lines.push(line);
            plain_lines.push(plain_line);
        }

        (lines, plain_lines, selectable)
    }

    fn render_file_picker_line(
        &self,
        item: &FileSearchMatch,
        selected: bool,
        width: usize,
        query: &str,
    ) -> (Line<'static>, String) {
        let inset = FILE_PICKER_INSET_WIDTH.min(width);
        let path_width = width.saturating_sub(inset);
        let display_path = file_picker_display_path(&item.path, query);
        let path = truncate_display_width_with_ellipsis(&display_path, path_width);
        let mut plain_line = format!("{}{}", " ".repeat(inset), path);
        plain_line.push_str(&" ".repeat(width.saturating_sub(display_width(&plain_line))));
        let style = if selected {
            command_accent_text_style(self.palette).bold()
        } else {
            secondary_text_style(self.palette)
        };

        (
            Line::from(vec![
                Span::raw(" ".repeat(inset)),
                Span::styled(path, style),
                Span::raw(" ".repeat(width.saturating_sub(display_width(plain_line.trim_end())))),
            ]),
            plain_line,
        )
    }

    fn move_file_picker_selection(&mut self, delta: isize) {
        let visible_rows = self.file_picker_list_visible_rows();
        let Some(state) = self.file_picker.as_mut() else {
            return;
        };
        if state.items.is_empty() {
            return;
        }

        let last = state.items.len() - 1;
        if delta.is_negative() {
            state.selected = state.selected.saturating_sub(delta.unsigned_abs());
        } else {
            state.selected = state.selected.saturating_add(delta as usize).min(last);
        }
        state.scroll = clamp_picker_scroll(
            state.scroll,
            state.selected,
            state.items.len(),
            visible_rows,
        );
    }

    fn complete_file_picker_common_prefix(&mut self) {
        let Some(state) = self.file_picker.as_ref() else {
            return;
        };
        let prefix = common_path_completion_prefix(&state.items, &state.query);
        if prefix.is_empty() || state.query == prefix {
            return;
        }

        self.replace_file_picker_token(format!("@{prefix}"));
    }

    fn insert_selected_file_picker_path(&mut self) -> bool {
        let Some(path) = self
            .file_picker
            .as_ref()
            .and_then(|state| state.items.get(state.selected))
            .map(|item| item.path.clone())
        else {
            return false;
        };

        self.replace_file_picker_token(format!("@{path} "));
        self.close_file_picker();
        true
    }

    fn replace_file_picker_token(&mut self, replacement: String) {
        let old_value = self.composer_text().to_string();
        let old_line = self.composer.line();
        let old_column = self.composer.column();
        if self.composer.replace_current_at_token(&replacement) {
            self.dismissed_file_picker_token = None;
            self.sync_command_panel_navigation();
            self.sync_file_picker_state();
            self.sync_external_editor_helper_after_draft_change(&old_value);
            self.sync_composer_height();
            self.sync_document_viewport_after_composer_interaction(
                &old_value, old_line, old_column,
            );
        }
    }

    fn close_file_picker(&mut self) {
        self.file_picker = None;
    }

    pub(crate) fn close_composer_attached_ui(&mut self) {
        self.close_file_picker();
        self.close_context_budget();
        self.command_panel_selected = 0;
        self.command_panel_scroll = 0;
    }

    fn dismiss_current_file_picker_token(&mut self) {
        self.dismissed_file_picker_token = self.composer.current_at_token();
    }

    fn file_search_root(&self) -> PathBuf {
        let path = resolve_configured_current_dir(&self.current_dir);
        if path.is_dir() {
            return path;
        }

        std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
    }

    fn current_file_picker_query_resolves_to_file(&self) -> bool {
        let Some(state) = self.file_picker.as_ref() else {
            return false;
        };
        if state.query.trim().is_empty() {
            return false;
        }

        let root = resolve_configured_current_dir(&self.current_dir);
        resolve_path_token(&root, &state.query).is_file()
    }
}

fn clamp_picker_scroll(
    scroll: usize,
    selected: usize,
    item_count: usize,
    visible_rows: usize,
) -> usize {
    if item_count == 0 {
        return 0;
    }
    let visible_rows = visible_rows.max(1);
    let max_scroll = item_count.saturating_sub(visible_rows);
    let mut scroll = scroll.min(max_scroll);
    if selected < scroll {
        scroll = selected;
    }
    if selected >= scroll + visible_rows {
        scroll = selected + 1 - visible_rows;
    }
    scroll.min(max_scroll)
}

fn file_picker_selectable_range(plain_line: &str, width: usize) -> SelectableLineRange {
    let end_column = display_width(plain_line.trim_end());
    if end_column <= FILE_PICKER_INSET_WIDTH {
        return SelectableLineRange::blank_hit_range(0, width);
    }

    SelectableLineRange::new(FILE_PICKER_INSET_WIDTH, end_column)
}

fn pad_display_width_right(text: &str, width: usize) -> String {
    let text = truncate_display_width_with_ellipsis(text, width);
    let padding = width.saturating_sub(display_width(&text));
    format!("{text}{}", " ".repeat(padding))
}

fn file_picker_display_path(path: &str, query: &str) -> String {
    let prefix = completed_directory_prefix(query);
    path.strip_prefix(prefix).unwrap_or(path).to_string()
}

fn completed_directory_prefix(query: &str) -> &str {
    if query.ends_with('/') {
        return query;
    }

    query
        .rfind('/')
        .map(|index| &query[..=index])
        .unwrap_or_default()
}
