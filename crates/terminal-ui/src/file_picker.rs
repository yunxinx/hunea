use std::path::PathBuf;

use crossterm::event::KeyEvent;
use ratatui::text::{Line, Span};

use super::{
    Model,
    composer_inline_picker::{
        ComposerInlinePickerCommand, ComposerInlinePickerInputResult,
        ComposerInlinePickerRenderedRows, ComposerInlinePickerState,
        handle_composer_inline_picker_input, reconcile_composer_inline_picker_state,
        render_composer_inline_picker_panel, render_composer_inline_picker_rows,
    },
    display_width::display_width,
    file_search::{FileSearchMatch, common_path_completion_prefix},
    image_attachment::{is_supported_image_path, load_image_attachment},
    inline_panel::InlinePanelRenderResult,
    overlay_input_result::OverlayInputResult,
    path_resolve::{resolve_configured_current_dir, resolve_path_token},
    search_highlight::{highlighted_substring_or_subsequence_spans, search_match_style},
    selection::SelectableLineRange,
    status_line::truncate_display_width_with_ellipsis,
    theme::{command_accent_text_style, secondary_text_style, tertiary_text_style},
    toast::ToastSeverity,
};

const FILE_PICKER_INSET_WIDTH: usize = 2;
pub(super) const FILE_PICKER_POPUP_MIN_HEIGHT: u16 = 3;
pub(super) const FILE_PICKER_POPUP_MAX_HEIGHT: u16 = 21;

/// `FilePickerState` 保存 `@` 文件选择器的当前查询、结果和导航位置。
pub(crate) type FilePickerState = ComposerInlinePickerState<FileSearchMatch>;

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
        self.file_picker = Some(reconcile_composer_inline_picker_state(
            query,
            items,
            previous,
            visible_rows,
            0,
        ));
    }

    pub(crate) fn handle_file_picker_key(&mut self, key: KeyEvent) -> OverlayInputResult {
        let visible_rows = self.file_picker_list_visible_rows();
        let Some(state) = self.file_picker.as_mut() else {
            return OverlayInputResult::Ignored;
        };

        match handle_composer_inline_picker_input(state, key, visible_rows) {
            ComposerInlinePickerInputResult::Handled => OverlayInputResult::Handled,
            ComposerInlinePickerInputResult::Command(ComposerInlinePickerCommand::Dismiss) => {
                self.dismiss_current_file_picker_token();
                self.close_file_picker();
                OverlayInputResult::Handled
            }
            ComposerInlinePickerInputResult::Command(ComposerInlinePickerCommand::Complete) => {
                self.complete_file_picker_common_prefix();
                OverlayInputResult::Handled
            }
            ComposerInlinePickerInputResult::Command(ComposerInlinePickerCommand::Accept) => {
                if self.current_file_picker_query_resolves_to_file() {
                    if self.insert_exact_file_picker_image_attachment() {
                        return OverlayInputResult::Handled;
                    }
                    self.close_file_picker();
                    self.dismissed_file_picker_token = None;
                    return OverlayInputResult::Ignored;
                }
                if self.insert_selected_file_picker_path() {
                    return OverlayInputResult::Handled;
                }
                OverlayInputResult::Handled
            }
            ComposerInlinePickerInputResult::Ignored => OverlayInputResult::Ignored,
        }
    }

    pub(crate) fn current_file_picker_render_result(&self) -> InlinePanelRenderResult {
        render_composer_inline_picker_panel(
            self.file_picker.as_ref(),
            self.width,
            self.file_picker_list_visible_rows(),
            |state, width, visible_rows| self.render_file_picker_lines(state, width, visible_rows),
        )
    }

    pub(crate) fn file_picker_list_visible_rows(&self) -> usize {
        usize::from(self.file_picker_popup_height.max(1))
    }

    fn render_file_picker_lines(
        &self,
        state: &FilePickerState,
        width: usize,
        visible_rows: usize,
    ) -> ComposerInlinePickerRenderedRows {
        render_composer_inline_picker_rows(
            state,
            width,
            visible_rows,
            "  No files",
            tertiary_text_style(self.palette),
            |item, query, selected, width| {
                self.render_file_picker_line(item, selected, width, query)
            },
            file_picker_selectable_range,
        )
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
        let highlighted_style = search_match_style(style, self.palette.surface);
        let display_query = file_picker_display_query(query);
        let mut spans = vec![Span::raw(" ".repeat(inset))];
        spans.extend(highlighted_substring_or_subsequence_spans(
            &path,
            display_query,
            style,
            highlighted_style,
        ));
        spans.push(Span::raw(" ".repeat(
            width.saturating_sub(display_width(plain_line.trim_end())),
        )));

        (Line::from(spans), plain_line)
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

        if self.insert_file_picker_image_attachment(&path) {
            return true;
        }
        self.replace_file_picker_token(format!("@{path} "));
        self.close_file_picker();
        true
    }

    fn insert_exact_file_picker_image_attachment(&mut self) -> bool {
        let Some(path) = self.file_picker.as_ref().map(|state| state.query.clone()) else {
            return false;
        };
        self.insert_file_picker_image_attachment(&path)
    }

    fn insert_file_picker_image_attachment(&mut self, uri: &str) -> bool {
        let root = resolve_configured_current_dir(&self.current_dir);
        let path = resolve_path_token(&root, uri);
        if !is_supported_image_path(&path) {
            return false;
        }

        let attachment = match load_image_attachment(uri, &path) {
            Ok(attachment) => attachment,
            Err(error) => {
                self.show_toast(
                    ToastSeverity::Error,
                    format!("Image attachment failed: {error}"),
                );
                return true;
            }
        };

        self.replace_file_picker_token_with_image_attachment(attachment);
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
            self.sync_composer_attached_picker_state();
            self.sync_external_editor_helper_after_draft_change(&old_value);
            self.sync_composer_height();
            self.sync_document_viewport_after_composer_interaction(
                &old_value, old_line, old_column,
            );
        }
    }

    fn replace_file_picker_token_with_image_attachment(
        &mut self,
        attachment: runtime_domain::session::TranscriptUserAttachment,
    ) {
        let old_value = self.composer_text().to_string();
        let old_line = self.composer.line();
        let old_column = self.composer.column();
        if self
            .composer
            .replace_current_at_token_with_image_attachment(attachment)
        {
            self.dismissed_file_picker_token = None;
            self.sync_command_panel_navigation();
            self.sync_composer_attached_picker_state();
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
        self.close_skill_picker();
        self.close_custom_prompt_picker();
        self.close_context_budget();
        self.close_floating_command_menu();
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

fn file_picker_selectable_range(plain_line: &str, width: usize) -> SelectableLineRange {
    let end_column = display_width(plain_line.trim_end());
    if end_column <= FILE_PICKER_INSET_WIDTH {
        return SelectableLineRange::blank_hit_range(0, width);
    }

    SelectableLineRange::new(FILE_PICKER_INSET_WIDTH, end_column)
}

fn file_picker_display_path(path: &str, query: &str) -> String {
    let prefix = completed_directory_prefix(query);
    path.strip_prefix(prefix).unwrap_or(path).to_string()
}

fn file_picker_display_query(query: &str) -> &str {
    query
        .strip_prefix(completed_directory_prefix(query))
        .unwrap_or(query)
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
