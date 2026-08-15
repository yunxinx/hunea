use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::text::{Line, Span};
use runtime_domain::prompt_assembly::PromptAssemblyDiscoveredSkill;

use super::{
    Model,
    attached_prompt_picker_row::{
        ATTACHED_PROMPT_PICKER_INSET_WIDTH, AttachedPromptPickerRowContent,
        attached_prompt_picker_name_column_width, attached_prompt_picker_selectable_range,
        render_attached_prompt_picker_row,
    },
    composer_inline_picker::{
        ComposerInlinePickerCommand, ComposerInlinePickerInputResult,
        ComposerInlinePickerRenderedRows, ComposerInlinePickerSearchText,
        ComposerInlinePickerState, common_composer_inline_picker_completion_prefix,
        filter_composer_inline_picker_items, handle_composer_inline_picker_input,
        reconcile_composer_inline_picker_state, render_composer_inline_picker_rows,
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
    theme::{
        TerminalPalette, command_accent_text_style, muted_text_style, secondary_text_style,
        tertiary_text_style,
    },
    toast::ToastSeverity,
};

const FILE_PICKER_INSET_WIDTH: usize = 2;
pub(super) const FILE_PICKER_POPUP_MIN_HEIGHT: u16 = 3;
pub(super) const FILE_PICKER_POPUP_MAX_HEIGHT: u16 = 21;

const MENTION_PICKER_FOOTER_ROWS: usize = 2;

/// `MentionSearchMode` 是统一 `@` mention 搜索的范围档位。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum MentionSearchMode {
    #[default]
    All,
    Files,
    Skills,
}

impl MentionSearchMode {
    fn cycle(self, towards_next: bool) -> Self {
        match (self, towards_next) {
            (Self::All, true) | (Self::Skills, false) => Self::Files,
            (Self::Files, true) | (Self::All, false) => Self::Skills,
            (Self::Skills, true) | (Self::Files, false) => Self::All,
        }
    }

    fn empty_list_text(self) -> &'static str {
        match self {
            Self::All => "  No matches",
            Self::Files => "  No files",
            Self::Skills => "  No skills",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::All => "All Results",
            Self::Files => "Files",
            Self::Skills => "Skills",
        }
    }
}

/// `MentionPickerItem` 是统一 mention 列表中的一行。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MentionPickerItem {
    File(FileSearchMatch),
    Skill(PromptAssemblyDiscoveredSkill),
}

/// `MentionPickerState` 保存统一 `@` mention 选择器的查询、范围、结果和导航位置。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MentionPickerState {
    pub(crate) query: String,
    pub(crate) search_mode: MentionSearchMode,
    pub(crate) items: Vec<MentionPickerItem>,
    pub(crate) selected: usize,
    pub(crate) scroll: usize,
}

impl Model {
    pub(crate) fn mention_picker_active(&self) -> bool {
        self.mention_picker.is_some()
    }

    pub(crate) fn sync_composer_attached_picker_state(&mut self) {
        self.sync_mention_picker_state();
        self.sync_custom_prompt_picker_state();
    }

    pub(crate) fn sync_mention_picker_state(&mut self) {
        if self.blocks_composer_input() || self.command_panel_active() {
            self.close_mention_picker();
            return;
        }

        let Some(query) = self.composer.current_at_token() else {
            self.close_mention_picker();
            self.dismissed_mention_token = None;
            return;
        };

        if self.dismissed_mention_token.as_ref() == Some(&query) {
            self.close_mention_picker();
            return;
        }

        let search_mode = self
            .mention_picker
            .as_ref()
            .map(|state| state.search_mode)
            .unwrap_or(MentionSearchMode::All);
        let items = self.mention_picker_items(&query, search_mode);
        let visible_rows = self.mention_picker_list_visible_rows();
        let previous = self.mention_picker.as_ref().map(mention_picker_nav_state);
        let bound_skill_name = self
            .composer
            .current_skill_binding()
            .map(|binding| binding.skill_name);
        let initial_selected = bound_skill_name
            .as_deref()
            .and_then(|skill_name| {
                items.iter().position(|item| {
                    matches!(
                        item,
                        MentionPickerItem::Skill(skill) if skill.skill_name.as_str() == skill_name
                    )
                })
            })
            .unwrap_or(0);
        let reconciled = reconcile_composer_inline_picker_state(
            query,
            items,
            previous.as_ref(),
            visible_rows,
            initial_selected,
        );
        self.mention_picker = Some(MentionPickerState {
            query: reconciled.query,
            search_mode,
            items: reconciled.items,
            selected: reconciled.selected,
            scroll: reconciled.scroll,
        });
    }

    pub(crate) fn handle_mention_picker_key(&mut self, key: KeyEvent) -> OverlayInputResult {
        if self.mention_picker.is_none() {
            return OverlayInputResult::Ignored;
        }

        if let Some(towards_next) = plain_horizontal_arrow_direction(key) {
            self.cycle_mention_search_mode(towards_next);
            return OverlayInputResult::Handled;
        }

        let visible_rows = self.mention_picker_list_visible_rows();
        let Some(state) = self.mention_picker.as_mut() else {
            return OverlayInputResult::Ignored;
        };
        let mut nav_state = ComposerInlinePickerState {
            query: std::mem::take(&mut state.query),
            items: std::mem::take(&mut state.items),
            selected: state.selected,
            scroll: state.scroll,
        };

        match handle_composer_inline_picker_input(&mut nav_state, key, visible_rows) {
            ComposerInlinePickerInputResult::Handled => {
                restore_mention_picker_nav(state, nav_state);
                OverlayInputResult::Handled
            }
            ComposerInlinePickerInputResult::Command(ComposerInlinePickerCommand::Dismiss) => {
                restore_mention_picker_nav(state, nav_state);
                self.dismiss_current_mention_token();
                self.close_mention_picker();
                OverlayInputResult::Handled
            }
            ComposerInlinePickerInputResult::Command(ComposerInlinePickerCommand::Complete) => {
                restore_mention_picker_nav(state, nav_state);
                self.complete_mention_picker_common_prefix();
                OverlayInputResult::Handled
            }
            ComposerInlinePickerInputResult::Command(ComposerInlinePickerCommand::Accept) => {
                restore_mention_picker_nav(state, nav_state);
                self.accept_mention_picker_selection()
            }
            ComposerInlinePickerInputResult::Ignored => {
                restore_mention_picker_nav(state, nav_state);
                OverlayInputResult::Ignored
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn current_mention_picker_render_result(&self) -> InlinePanelRenderResult {
        self.mention_picker_render_result(false)
    }

    pub(crate) fn mention_picker_render_result(
        &self,
        chrome_before_list: bool,
    ) -> InlinePanelRenderResult {
        let Some(state) = self.mention_picker.as_ref() else {
            return InlinePanelRenderResult::default();
        };

        let width = usize::from(self.width.max(1));
        let list_rows = self.mention_picker_list_visible_rows();
        let has_scrollbar = state.items.len() > list_rows && width > 1;
        let content_width = width.saturating_sub(usize::from(has_scrollbar));
        let mut rows = self.render_mention_picker_lines(state, content_width, list_rows);
        pad_rendered_rows_to_count(&mut rows, list_rows, content_width);

        let spacer_width = width;
        let spacer_line = Line::raw(" ".repeat(spacer_width));
        let spacer_plain = " ".repeat(spacer_width);
        let (footer_line, footer_plain) =
            render_mention_picker_footer(width, state.search_mode, self.palette);
        let footer_selectable = SelectableLineRange::blank_hit_range(0, width);
        if chrome_before_list {
            rows.lines.splice(0..0, [footer_line, spacer_line]);
            rows.plain_lines.splice(0..0, [footer_plain, spacer_plain]);
            rows.selectable
                .splice(0..0, [footer_selectable, SelectableLineRange::default()]);
        } else {
            rows.lines.push(spacer_line);
            rows.plain_lines.push(spacer_plain);
            rows.selectable.push(SelectableLineRange::default());
            rows.lines.push(footer_line);
            rows.plain_lines.push(footer_plain);
            rows.selectable.push(footer_selectable);
        }

        InlinePanelRenderResult {
            lines: rows.lines,
            plain_lines: rows.plain_lines,
            selectable: rows.selectable,
            has_content: true,
        }
    }

    pub(crate) fn mention_picker_list_visible_rows(&self) -> usize {
        usize::from(self.file_picker_popup_height.max(1))
            .saturating_sub(MENTION_PICKER_FOOTER_ROWS)
            .max(1)
    }

    pub(crate) fn file_picker_list_visible_rows(&self) -> usize {
        usize::from(self.file_picker_popup_height.max(1))
    }

    fn render_mention_picker_lines(
        &self,
        state: &MentionPickerState,
        width: usize,
        visible_rows: usize,
    ) -> ComposerInlinePickerRenderedRows {
        let width = width.max(1);
        let visible_rows = visible_rows.max(1);
        if state.items.is_empty() {
            return render_composer_inline_picker_rows(
                &mention_picker_nav_state(state),
                width,
                visible_rows,
                state.search_mode.empty_list_text(),
                tertiary_text_style(self.palette),
                |_, _, _, _| unreachable!("empty mention picker should not render items"),
                |_plain, width| SelectableLineRange::blank_hit_range(0, width),
            );
        }

        let name_column_width = attached_prompt_picker_name_column_width(
            state.items.iter().filter_map(|item| match item {
                MentionPickerItem::Skill(skill) => Some(skill_display_name(skill)),
                MentionPickerItem::File(_) => None,
            }),
            width.saturating_sub(ATTACHED_PROMPT_PICKER_INSET_WIDTH),
        );
        let mut lines = Vec::with_capacity(visible_rows);
        let mut plain_lines = Vec::with_capacity(visible_rows);
        let mut selectable = Vec::with_capacity(visible_rows);
        for row in 0..visible_rows {
            let index = state.scroll + row;
            let Some(item) = state.items.get(index) else {
                lines.push(Line::raw(""));
                plain_lines.push(String::new());
                selectable.push(SelectableLineRange::default());
                continue;
            };
            let selected = index == state.selected;
            let (line, plain_line) = match item {
                MentionPickerItem::File(file) => {
                    self.render_mention_file_line(file, selected, width, &state.query)
                }
                MentionPickerItem::Skill(skill) => self.render_mention_skill_line(
                    skill,
                    &state.query,
                    selected,
                    width,
                    name_column_width,
                ),
            };
            selectable.push(match item {
                MentionPickerItem::File(_) => file_picker_selectable_range(&plain_line, width),
                MentionPickerItem::Skill(_) => {
                    attached_prompt_picker_selectable_range(&plain_line, width)
                }
            });
            lines.push(line);
            plain_lines.push(plain_line);
        }

        ComposerInlinePickerRenderedRows {
            lines,
            plain_lines,
            selectable,
        }
    }

    fn render_mention_file_line(
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

    fn render_mention_skill_line(
        &self,
        item: &PromptAssemblyDiscoveredSkill,
        query: &str,
        selected: bool,
        width: usize,
        name_column_width: usize,
    ) -> (Line<'static>, String) {
        render_attached_prompt_picker_row(
            AttachedPromptPickerRowContent {
                display_name: skill_display_name(item),
                description: item.description.trim(),
                trailing_suffix: None,
            },
            query,
            selected,
            width,
            name_column_width,
            self.palette,
        )
    }

    fn cycle_mention_search_mode(&mut self, towards_next: bool) {
        let Some(state) = self.mention_picker.as_mut() else {
            return;
        };
        state.search_mode = state.search_mode.cycle(towards_next);
        state.selected = 0;
        state.scroll = 0;
        self.sync_mention_picker_state();
    }

    fn complete_mention_picker_common_prefix(&mut self) {
        let Some(state) = self.mention_picker.as_ref() else {
            return;
        };
        let Some(selected) = state.items.get(state.selected) else {
            return;
        };

        match selected {
            MentionPickerItem::File(_) => {
                let files = mention_file_items(&state.items);
                let prefix = common_path_completion_prefix(&files, &state.query);
                if prefix.is_empty() || state.query == prefix {
                    return;
                }
                self.replace_mention_token(format!("@{prefix}"));
            }
            MentionPickerItem::Skill(_) => {
                let prefix = common_skill_completion_prefix(
                    mention_skill_items(&state.items).as_slice(),
                    &state.query,
                );
                if prefix.is_empty() || state.query == prefix {
                    return;
                }
                self.replace_mention_token(format!("@{prefix}"));
            }
        }
    }

    fn accept_mention_picker_selection(&mut self) -> OverlayInputResult {
        if self.current_mention_query_resolves_to_file() {
            if self.insert_exact_mention_image_attachment() {
                return OverlayInputResult::Handled;
            }
            self.close_mention_picker();
            self.dismissed_mention_token = None;
            return OverlayInputResult::Ignored;
        }

        let Some(item) = self
            .mention_picker
            .as_ref()
            .and_then(|state| state.items.get(state.selected))
            .cloned()
        else {
            return OverlayInputResult::Handled;
        };

        match item {
            MentionPickerItem::File(file) => {
                if self.insert_mention_image_attachment(&file.path) {
                    return OverlayInputResult::Handled;
                }
                self.replace_mention_token(format!("@{} ", file.path));
                self.close_mention_picker();
            }
            MentionPickerItem::Skill(skill) => {
                let _ = self.insert_selected_mention_skill(&skill);
            }
        }
        OverlayInputResult::Handled
    }

    fn insert_selected_mention_skill(&mut self, skill: &PromptAssemblyDiscoveredSkill) -> bool {
        let old_value = self.composer_text().to_string();
        let old_line = self.composer.line();
        let old_column = self.composer.column();
        if !self.composer.replace_current_skill_token(
            &skill.skill_name,
            skill.skill_path.as_path(),
            skill.origin,
        ) {
            return false;
        }
        self.dismissed_mention_token = None;
        self.sync_command_panel_navigation();
        self.sync_composer_attached_picker_state();
        self.sync_external_editor_helper_after_draft_change(&old_value);
        self.sync_composer_height();
        self.sync_document_viewport_after_composer_interaction(&old_value, old_line, old_column);
        true
    }

    fn insert_exact_mention_image_attachment(&mut self) -> bool {
        let Some(path) = self
            .mention_picker
            .as_ref()
            .map(|state| state.query.clone())
        else {
            return false;
        };
        self.insert_mention_image_attachment(&path)
    }

    fn insert_mention_image_attachment(&mut self, uri: &str) -> bool {
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

        self.replace_mention_token_with_image_attachment(attachment);
        self.close_mention_picker();
        true
    }

    fn replace_mention_token(&mut self, replacement: String) {
        let old_value = self.composer_text().to_string();
        let old_line = self.composer.line();
        let old_column = self.composer.column();
        if self.composer.replace_current_at_token(&replacement) {
            self.dismissed_mention_token = None;
            self.sync_command_panel_navigation();
            self.sync_composer_attached_picker_state();
            self.sync_external_editor_helper_after_draft_change(&old_value);
            self.sync_composer_height();
            self.sync_document_viewport_after_composer_interaction(
                &old_value, old_line, old_column,
            );
        }
    }

    fn replace_mention_token_with_image_attachment(
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
            self.dismissed_mention_token = None;
            self.sync_command_panel_navigation();
            self.sync_composer_attached_picker_state();
            self.sync_external_editor_helper_after_draft_change(&old_value);
            self.sync_composer_height();
            self.sync_document_viewport_after_composer_interaction(
                &old_value, old_line, old_column,
            );
        }
    }

    fn close_mention_picker(&mut self) {
        self.mention_picker = None;
    }

    pub(crate) fn close_composer_attached_ui(&mut self) {
        self.close_mention_picker();
        self.close_custom_prompt_picker();
        self.close_context_budget();
        self.close_floating_command_menu();
        self.command_panel_selected = 0;
        self.command_panel_scroll = 0;
    }

    fn dismiss_current_mention_token(&mut self) {
        self.dismissed_mention_token = self.composer.current_at_token();
    }

    fn file_search_root(&self) -> PathBuf {
        let path = resolve_configured_current_dir(&self.current_dir);
        if path.is_dir() {
            return path;
        }

        std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
    }

    fn current_mention_query_resolves_to_file(&self) -> bool {
        let Some(state) = self.mention_picker.as_ref() else {
            return false;
        };
        if state.query.trim().is_empty() {
            return false;
        }

        let root = resolve_configured_current_dir(&self.current_dir);
        resolve_path_token(&root, &state.query).is_file()
    }

    fn mention_picker_items(
        &mut self,
        query: &str,
        search_mode: MentionSearchMode,
    ) -> Vec<MentionPickerItem> {
        let include_files = matches!(
            search_mode,
            MentionSearchMode::All | MentionSearchMode::Files
        );
        let include_skills = matches!(
            search_mode,
            MentionSearchMode::All | MentionSearchMode::Skills
        );
        let mut items = Vec::new();
        if include_files {
            let root = self.file_search_root();
            items.extend(
                self.file_search_cache
                    .search(&root, query)
                    .into_iter()
                    .map(MentionPickerItem::File),
            );
        }
        if include_skills {
            items.extend(
                filter_manual_skill_items(&self.prompt_assembly.candidates.manual_skills, query)
                    .into_iter()
                    .map(MentionPickerItem::Skill),
            );
        }
        items
    }
}

fn mention_picker_nav_state(
    state: &MentionPickerState,
) -> ComposerInlinePickerState<MentionPickerItem> {
    ComposerInlinePickerState {
        query: state.query.clone(),
        items: state.items.clone(),
        selected: state.selected,
        scroll: state.scroll,
    }
}

fn restore_mention_picker_nav(
    state: &mut MentionPickerState,
    nav_state: ComposerInlinePickerState<MentionPickerItem>,
) {
    state.query = nav_state.query;
    state.items = nav_state.items;
    state.selected = nav_state.selected;
    state.scroll = nav_state.scroll;
}

fn mention_file_items(items: &[MentionPickerItem]) -> Vec<FileSearchMatch> {
    items
        .iter()
        .filter_map(|item| match item {
            MentionPickerItem::File(file) => Some(file.clone()),
            MentionPickerItem::Skill(_) => None,
        })
        .collect()
}

fn mention_skill_items(items: &[MentionPickerItem]) -> Vec<PromptAssemblyDiscoveredSkill> {
    items
        .iter()
        .filter_map(|item| match item {
            MentionPickerItem::Skill(skill) => Some(skill.clone()),
            MentionPickerItem::File(_) => None,
        })
        .collect()
}

fn filter_manual_skill_items(
    skills: &[PromptAssemblyDiscoveredSkill],
    query: &str,
) -> Vec<PromptAssemblyDiscoveredSkill> {
    filter_composer_inline_picker_items(skills, query, |skill| ComposerInlinePickerSearchText {
        prefix_terms: vec![
            skill.skill_name.as_str().into(),
            skill_display_name(skill).into(),
        ],
        fuzzy_terms: vec![skill.description.as_str().into()],
    })
}

fn common_skill_completion_prefix(skills: &[PromptAssemblyDiscoveredSkill], query: &str) -> String {
    let prefix = common_composer_inline_picker_completion_prefix(
        skills.iter().map(|skill| skill.skill_name.as_str()),
    );

    if prefix.len() <= query.len() {
        String::new()
    } else {
        prefix
    }
}

fn skill_display_name(item: &PromptAssemblyDiscoveredSkill) -> &str {
    let trimmed_title = item.title.trim();
    if trimmed_title.is_empty() {
        item.skill_name.as_str()
    } else {
        trimmed_title
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

fn pad_rendered_rows_to_count(
    rows: &mut ComposerInlinePickerRenderedRows,
    visible_rows: usize,
    width: usize,
) {
    while rows.lines.len() < visible_rows {
        rows.lines.push(Line::raw(" ".repeat(width)));
        rows.plain_lines.push(String::new());
        rows.selectable.push(SelectableLineRange::default());
    }
}

fn plain_horizontal_arrow_direction(key: KeyEvent) -> Option<bool> {
    if !key.modifiers.is_empty() {
        return None;
    }
    match key.code {
        KeyCode::Right => Some(true),
        KeyCode::Left => Some(false),
        _ => None,
    }
}

fn render_mention_picker_footer(
    width: usize,
    search_mode: MentionSearchMode,
    palette: TerminalPalette,
) -> (Line<'static>, String) {
    let width = width.max(1);
    let modes = [
        MentionSearchMode::All,
        MentionSearchMode::Files,
        MentionSearchMode::Skills,
    ];
    let right_plain = modes
        .iter()
        .map(|mode| {
            if *mode == search_mode {
                format!("[{}]", mode.label())
            } else {
                mode.label().to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("  ");
    let right_width = display_width(&right_plain);
    let hint_plain = "Enter insert · Esc close · ←/→ switch search modes";
    let gap = 1;
    let available_left = width.saturating_sub(right_width.saturating_add(gap));
    let left_plain = if display_width(hint_plain) > available_left {
        truncate_display_width_with_ellipsis(hint_plain, available_left)
    } else {
        hint_plain.to_string()
    };
    let left_width = display_width(&left_plain);
    let padding = width.saturating_sub(left_width).saturating_sub(right_width);
    let plain_line = format!("{left_plain}{}{right_plain}", " ".repeat(padding));

    let mut spans = mention_picker_hint_spans(&left_plain, palette);
    if padding > 0 {
        spans.push(Span::raw(" ".repeat(padding)));
    }
    spans.extend(mention_picker_mode_spans(search_mode, palette));
    let rendered_width = spans
        .iter()
        .map(|span| display_width(&span.content))
        .sum::<usize>();
    if rendered_width < width {
        spans.push(Span::raw(" ".repeat(width - rendered_width)));
    }

    (Line::from(spans), plain_line)
}

fn mention_picker_hint_spans(left_plain: &str, palette: TerminalPalette) -> Vec<Span<'static>> {
    let key_style = command_accent_text_style(palette);
    let muted = muted_text_style(palette);
    let tertiary = tertiary_text_style(palette);
    if left_plain == "Enter insert · Esc close · ←/→ switch search modes" {
        return vec![
            Span::styled("Enter", key_style),
            Span::styled(" insert", muted),
            Span::styled(" · ", tertiary),
            Span::styled("Esc", key_style),
            Span::styled(" close", muted),
            Span::styled(" · ", tertiary),
            Span::styled("←/→", key_style),
            Span::styled(" switch search modes", muted),
        ];
    }

    vec![Span::styled(left_plain.to_string(), muted)]
}

fn mention_picker_mode_spans(
    search_mode: MentionSearchMode,
    palette: TerminalPalette,
) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    for (index, mode) in [
        MentionSearchMode::All,
        MentionSearchMode::Files,
        MentionSearchMode::Skills,
    ]
    .into_iter()
    .enumerate()
    {
        if index > 0 {
            spans.push(Span::raw("  "));
        }
        if mode == search_mode {
            spans.push(Span::styled(
                format!("[{}]", mode.label()),
                command_accent_text_style(palette).bold(),
            ));
        } else {
            spans.push(Span::styled(
                mode.label().to_string(),
                muted_text_style(palette),
            ));
        }
    }
    spans
}
