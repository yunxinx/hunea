use crossterm::event::KeyEvent;
use ratatui::text::Line;
use runtime_domain::prompt_assembly::{PromptAssemblyExtraPromptCandidate, PromptSourceOrigin};

use super::{
    Model,
    attached_prompt_picker_row::{
        ATTACHED_PROMPT_PICKER_INSET_WIDTH, AttachedPromptPickerRowContent,
        attached_prompt_picker_name_column_width, attached_prompt_picker_selectable_range,
        render_attached_prompt_picker_row,
    },
    composer_inline_picker::{
        ComposerInlinePickerKey, ComposerInlinePickerState, classify_composer_inline_picker_key,
        move_composer_inline_picker_selection, reconcile_composer_inline_picker_state,
        render_composer_inline_picker_rows,
    },
    inline_panel::InlinePanelRenderResult,
    overlay_input_result::OverlayInputResult,
    selection::SelectableLineRange,
    theme::tertiary_text_style,
};

/// `CustomPromptPickerState` 保存 `#prompt` 选择器的当前查询、结果和导航位置。
pub(crate) type CustomPromptPickerState =
    ComposerInlinePickerState<PromptAssemblyExtraPromptCandidate>;

impl Model {
    pub(crate) fn custom_prompt_picker_active(&self) -> bool {
        self.custom_prompt_picker.is_some()
    }

    pub(crate) fn sync_custom_prompt_picker_state(&mut self) {
        if self.blocks_composer_input() || self.command_panel_active() {
            self.close_custom_prompt_picker();
            return;
        }

        let Some(query) = self.composer.current_custom_prompt_token() else {
            self.close_custom_prompt_picker();
            self.dismissed_custom_prompt_picker_token = None;
            return;
        };

        if self.dismissed_custom_prompt_picker_token.as_ref() == Some(&query) {
            self.close_custom_prompt_picker();
            return;
        }

        let items =
            filter_custom_prompt_items(&self.prompt_assembly.extra_prompt_candidates, &query);
        let visible_rows = self.file_picker_list_visible_rows();
        let previous = self.custom_prompt_picker.as_ref();
        let bound_prompt = self.composer.current_custom_prompt_binding();
        let initial_selected = bound_prompt
            .as_ref()
            .and_then(|binding| {
                items.iter().position(|item| {
                    item.reference_id == binding.reference_id && item.origin == binding.origin
                })
            })
            .unwrap_or(0);

        self.custom_prompt_picker = Some(reconcile_composer_inline_picker_state(
            query,
            items,
            previous,
            visible_rows,
            initial_selected,
        ));
    }

    pub(crate) fn handle_custom_prompt_picker_key(&mut self, key: KeyEvent) -> OverlayInputResult {
        if !self.custom_prompt_picker_active() {
            return OverlayInputResult::Ignored;
        }

        match classify_composer_inline_picker_key(key) {
            Some(ComposerInlinePickerKey::MovePrevious) => {
                self.move_custom_prompt_picker_selection(-1);
                OverlayInputResult::Handled
            }
            Some(ComposerInlinePickerKey::MoveNext) => {
                self.move_custom_prompt_picker_selection(1);
                OverlayInputResult::Handled
            }
            Some(ComposerInlinePickerKey::Dismiss) => {
                self.dismiss_current_custom_prompt_picker_token();
                self.close_custom_prompt_picker();
                OverlayInputResult::Handled
            }
            Some(ComposerInlinePickerKey::Complete) => {
                self.complete_custom_prompt_picker_common_prefix();
                OverlayInputResult::Handled
            }
            Some(ComposerInlinePickerKey::Accept) => {
                let _ = self.insert_selected_custom_prompt_picker_item();
                OverlayInputResult::Handled
            }
            None => OverlayInputResult::Ignored,
        }
    }

    pub(crate) fn current_custom_prompt_picker_render_result(&self) -> InlinePanelRenderResult {
        let Some(state) = self.custom_prompt_picker.as_ref() else {
            return InlinePanelRenderResult::default();
        };

        let visible_rows = self.file_picker_list_visible_rows();
        let width = usize::from(self.width.max(1));
        let has_scrollbar = state.items.len() > visible_rows;
        let content_width = width.saturating_sub(usize::from(has_scrollbar && width > 1));
        let (lines, plain_lines, selectable) =
            self.render_custom_prompt_picker_lines(state, content_width, visible_rows);

        InlinePanelRenderResult {
            lines,
            plain_lines,
            selectable,
            has_content: true,
        }
    }

    fn render_custom_prompt_picker_lines(
        &self,
        state: &CustomPromptPickerState,
        width: usize,
        visible_rows: usize,
    ) -> (Vec<Line<'static>>, Vec<String>, Vec<SelectableLineRange>) {
        let name_column_width = attached_prompt_picker_name_column_width(
            state.items.iter().map(custom_prompt_picker_display_name),
            width.saturating_sub(ATTACHED_PROMPT_PICKER_INSET_WIDTH),
        );

        let rows = render_composer_inline_picker_rows(
            state,
            width,
            visible_rows,
            "  No custom prompts",
            tertiary_text_style(self.palette),
            |item, query, selected, width| {
                self.render_custom_prompt_picker_line(
                    item,
                    query,
                    selected,
                    width,
                    name_column_width,
                )
            },
            custom_prompt_picker_selectable_range,
        );
        (rows.lines, rows.plain_lines, rows.selectable)
    }

    fn render_custom_prompt_picker_line(
        &self,
        item: &PromptAssemblyExtraPromptCandidate,
        query: &str,
        selected: bool,
        width: usize,
        name_column_width: usize,
    ) -> (Line<'static>, String) {
        render_attached_prompt_picker_row(
            AttachedPromptPickerRowContent {
                display_name: custom_prompt_picker_display_name(item),
                description: custom_prompt_picker_body_summary(&item.body)
                    .as_deref()
                    .unwrap_or_default(),
                trailing_suffix: Some(custom_prompt_picker_origin_suffix(item.origin)),
            },
            query,
            selected,
            width,
            name_column_width,
            self.palette,
        )
    }

    fn move_custom_prompt_picker_selection(&mut self, delta: isize) {
        let visible_rows = self.file_picker_list_visible_rows();
        let Some(state) = self.custom_prompt_picker.as_mut() else {
            return;
        };
        move_composer_inline_picker_selection(state, delta, visible_rows);
    }

    fn complete_custom_prompt_picker_common_prefix(&mut self) {
        let Some(state) = self.custom_prompt_picker.as_ref() else {
            return;
        };
        let prefix = common_custom_prompt_completion_prefix(&state.items, &state.query);
        if prefix.is_empty() || state.query == prefix {
            return;
        }

        self.replace_custom_prompt_picker_token(format!("#{prefix}"));
    }

    fn insert_selected_custom_prompt_picker_item(&mut self) -> bool {
        let Some(prompt) = self
            .custom_prompt_picker
            .as_ref()
            .and_then(|state| state.items.get(state.selected))
            .cloned()
        else {
            return false;
        };

        let old_value = self.composer_text().to_string();
        let old_line = self.composer.line();
        let old_column = self.composer.column();
        if !self
            .composer
            .replace_current_custom_prompt_token(&prompt.reference_id, prompt.origin)
        {
            return false;
        }
        self.dismissed_custom_prompt_picker_token = None;
        self.sync_command_panel_navigation();
        self.sync_composer_attached_picker_state();
        self.sync_external_editor_helper_after_draft_change(&old_value);
        self.sync_composer_height();
        self.sync_document_viewport_after_composer_interaction(&old_value, old_line, old_column);
        true
    }

    fn replace_custom_prompt_picker_token(&mut self, replacement: String) {
        let old_value = self.composer_text().to_string();
        let old_line = self.composer.line();
        let old_column = self.composer.column();
        if self
            .composer
            .replace_current_prefixed_token('#', &replacement)
        {
            self.dismissed_custom_prompt_picker_token = None;
            self.sync_command_panel_navigation();
            self.sync_composer_attached_picker_state();
            self.sync_external_editor_helper_after_draft_change(&old_value);
            self.sync_composer_height();
            self.sync_document_viewport_after_composer_interaction(
                &old_value, old_line, old_column,
            );
        }
    }

    pub(crate) fn close_custom_prompt_picker(&mut self) {
        self.custom_prompt_picker = None;
    }

    fn dismiss_current_custom_prompt_picker_token(&mut self) {
        self.dismissed_custom_prompt_picker_token = self.composer.current_custom_prompt_token();
    }
}

fn filter_custom_prompt_items(
    prompts: &[PromptAssemblyExtraPromptCandidate],
    query: &str,
) -> Vec<PromptAssemblyExtraPromptCandidate> {
    let trimmed_query = query.trim().to_ascii_lowercase();
    if trimmed_query.is_empty() {
        return prompts.to_vec();
    }

    let mut prefix_matches = Vec::new();
    let mut fuzzy_matches = Vec::new();
    for prompt in prompts {
        let reference_id = prompt.reference_id.to_ascii_lowercase();
        let title = custom_prompt_picker_display_name(prompt).to_ascii_lowercase();
        let description = custom_prompt_picker_description(prompt).to_ascii_lowercase();
        let body = prompt.body.to_ascii_lowercase();
        if reference_id.starts_with(&trimmed_query) || title.starts_with(&trimmed_query) {
            prefix_matches.push(prompt.clone());
        } else if reference_id.contains(&trimmed_query)
            || title.contains(&trimmed_query)
            || description.contains(&trimmed_query)
            || body.contains(&trimmed_query)
        {
            fuzzy_matches.push(prompt.clone());
        }
    }
    prefix_matches.extend(fuzzy_matches);
    prefix_matches
}

fn common_custom_prompt_completion_prefix(
    prompts: &[PromptAssemblyExtraPromptCandidate],
    query: &str,
) -> String {
    let mut matches = prompts
        .iter()
        .filter(|prompt| {
            prompt
                .reference_id
                .to_ascii_lowercase()
                .starts_with(&query.to_ascii_lowercase())
        })
        .map(|prompt| prompt.reference_id.as_str());
    let Some(first) = matches.next() else {
        return String::new();
    };
    let mut prefix = first.to_string();
    for reference_id in matches {
        let common_len = prefix
            .chars()
            .zip(reference_id.chars())
            .take_while(|(left, right)| left.eq_ignore_ascii_case(right))
            .count();
        prefix = prefix.chars().take(common_len).collect();
        if prefix.is_empty() {
            break;
        }
    }
    prefix
}

fn custom_prompt_picker_selectable_range(plain_line: &str, width: usize) -> SelectableLineRange {
    attached_prompt_picker_selectable_range(plain_line, width)
}

fn custom_prompt_picker_display_name(item: &PromptAssemblyExtraPromptCandidate) -> &str {
    if item.title.trim().is_empty() {
        item.reference_id.as_str()
    } else {
        item.title.trim()
    }
}

fn custom_prompt_picker_description(item: &PromptAssemblyExtraPromptCandidate) -> String {
    let scope = custom_prompt_picker_origin_suffix(item.origin);
    match custom_prompt_picker_body_summary(&item.body) {
        Some(content) => format!("{content} {scope}"),
        None => scope.to_string(),
    }
}

fn custom_prompt_picker_body_summary(body: &str) -> Option<String> {
    body.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .find_map(|line| {
            if let Some(heading) = line.strip_prefix('#') {
                let heading = heading.trim_start_matches('#').trim();
                if !heading.is_empty() {
                    return None;
                }
            }

            Some(line.split_whitespace().collect::<Vec<_>>().join(" "))
        })
        .filter(|summary| !summary.is_empty())
}

fn custom_prompt_picker_origin_suffix(origin: PromptSourceOrigin) -> &'static str {
    match origin {
        PromptSourceOrigin::Project => "(project)",
        PromptSourceOrigin::Global => "(global)",
        PromptSourceOrigin::Builtin => "(builtin)",
    }
}
