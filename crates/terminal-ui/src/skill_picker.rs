use crossterm::event::KeyEvent;
use ratatui::text::Line;
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
        reconcile_composer_inline_picker_state, render_composer_inline_picker_panel,
        render_composer_inline_picker_rows,
    },
    inline_panel::InlinePanelRenderResult,
    overlay_input_result::OverlayInputResult,
    selection::SelectableLineRange,
    theme::tertiary_text_style,
};

/// `SkillPickerState` 保存 `$skill` 选择器的当前查询、结果和导航位置。
pub(crate) type SkillPickerState = ComposerInlinePickerState<PromptAssemblyDiscoveredSkill>;

impl Model {
    pub(crate) fn skill_picker_active(&self) -> bool {
        self.skill_picker.is_some()
    }

    pub(crate) fn sync_composer_attached_picker_state(&mut self) {
        self.sync_file_picker_state();
        self.sync_skill_picker_state();
        self.sync_custom_prompt_picker_state();
    }

    pub(crate) fn sync_skill_picker_state(&mut self) {
        if self.blocks_composer_input() || self.command_panel_active() {
            self.close_skill_picker();
            return;
        }

        let Some(query) = self.composer.current_skill_token() else {
            self.close_skill_picker();
            self.dismissed_skill_picker_token = None;
            return;
        };

        if self.dismissed_skill_picker_token.as_ref() == Some(&query) {
            self.close_skill_picker();
            return;
        }

        let items =
            filter_manual_skill_items(&self.prompt_assembly.candidates.manual_skills, &query);
        let visible_rows = self.file_picker_list_visible_rows();
        let previous = self.skill_picker.as_ref();
        let bound_skill_name = self
            .composer
            .current_skill_binding()
            .map(|binding| binding.skill_name);
        let initial_selected = bound_skill_name
            .as_deref()
            .and_then(|skill_name| {
                items
                    .iter()
                    .position(|item| item.skill_name.as_str() == skill_name)
            })
            .unwrap_or(0);

        self.skill_picker = Some(reconcile_composer_inline_picker_state(
            query,
            items,
            previous,
            visible_rows,
            initial_selected,
        ));
    }

    pub(crate) fn handle_skill_picker_key(&mut self, key: KeyEvent) -> OverlayInputResult {
        let visible_rows = self.file_picker_list_visible_rows();
        let Some(state) = self.skill_picker.as_mut() else {
            return OverlayInputResult::Ignored;
        };

        match handle_composer_inline_picker_input(state, key, visible_rows) {
            ComposerInlinePickerInputResult::Handled => OverlayInputResult::Handled,
            ComposerInlinePickerInputResult::Command(ComposerInlinePickerCommand::Dismiss) => {
                self.dismiss_current_skill_picker_token();
                self.close_skill_picker();
                OverlayInputResult::Handled
            }
            ComposerInlinePickerInputResult::Command(ComposerInlinePickerCommand::Complete) => {
                self.complete_skill_picker_common_prefix();
                OverlayInputResult::Handled
            }
            ComposerInlinePickerInputResult::Command(ComposerInlinePickerCommand::Accept) => {
                let _ = self.insert_selected_skill_picker_skill();
                OverlayInputResult::Handled
            }
            ComposerInlinePickerInputResult::Ignored => OverlayInputResult::Ignored,
        }
    }

    pub(crate) fn current_skill_picker_render_result(&self) -> InlinePanelRenderResult {
        render_composer_inline_picker_panel(
            self.skill_picker.as_ref(),
            self.width,
            self.file_picker_list_visible_rows(),
            |state, width, visible_rows| self.render_skill_picker_lines(state, width, visible_rows),
        )
    }

    fn render_skill_picker_lines(
        &self,
        state: &SkillPickerState,
        width: usize,
        visible_rows: usize,
    ) -> ComposerInlinePickerRenderedRows {
        let name_column_width = attached_prompt_picker_name_column_width(
            state.items.iter().map(skill_picker_display_name),
            width.saturating_sub(ATTACHED_PROMPT_PICKER_INSET_WIDTH),
        );

        render_composer_inline_picker_rows(
            state,
            width,
            visible_rows,
            "  No skills",
            tertiary_text_style(self.palette),
            |item, query, selected, width| {
                self.render_skill_picker_line(item, query, selected, width, name_column_width)
            },
            skill_picker_selectable_range,
        )
    }

    fn render_skill_picker_line(
        &self,
        item: &PromptAssemblyDiscoveredSkill,
        query: &str,
        selected: bool,
        width: usize,
        name_column_width: usize,
    ) -> (Line<'static>, String) {
        render_attached_prompt_picker_row(
            AttachedPromptPickerRowContent {
                display_name: skill_picker_display_name(item),
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

    fn complete_skill_picker_common_prefix(&mut self) {
        let Some(state) = self.skill_picker.as_ref() else {
            return;
        };
        let prefix = common_skill_completion_prefix(&state.items, &state.query);
        if prefix.is_empty() || state.query == prefix {
            return;
        }

        self.replace_skill_picker_token(format!("${prefix}"));
    }

    fn insert_selected_skill_picker_skill(&mut self) -> bool {
        let Some(skill) = self
            .skill_picker
            .as_ref()
            .and_then(|state| state.items.get(state.selected))
            .cloned()
        else {
            return false;
        };

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
        self.dismissed_skill_picker_token = None;
        self.sync_command_panel_navigation();
        self.sync_composer_attached_picker_state();
        self.sync_external_editor_helper_after_draft_change(&old_value);
        self.sync_composer_height();
        self.sync_document_viewport_after_composer_interaction(&old_value, old_line, old_column);
        true
    }

    fn replace_skill_picker_token(&mut self, replacement: String) {
        let old_value = self.composer_text().to_string();
        let old_line = self.composer.line();
        let old_column = self.composer.column();
        if self
            .composer
            .replace_current_prefixed_token('$', &replacement)
        {
            self.dismissed_skill_picker_token = None;
            self.sync_command_panel_navigation();
            self.sync_composer_attached_picker_state();
            self.sync_external_editor_helper_after_draft_change(&old_value);
            self.sync_composer_height();
            self.sync_document_viewport_after_composer_interaction(
                &old_value, old_line, old_column,
            );
        }
    }

    pub(crate) fn close_skill_picker(&mut self) {
        self.skill_picker = None;
    }

    fn dismiss_current_skill_picker_token(&mut self) {
        self.dismissed_skill_picker_token = self.composer.current_skill_token();
    }
}

fn filter_manual_skill_items(
    skills: &[PromptAssemblyDiscoveredSkill],
    query: &str,
) -> Vec<PromptAssemblyDiscoveredSkill> {
    filter_composer_inline_picker_items(skills, query, |skill| ComposerInlinePickerSearchText {
        prefix_terms: vec![
            skill.skill_name.as_str().into(),
            skill_picker_display_name(skill).into(),
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

fn skill_picker_selectable_range(plain_line: &str, width: usize) -> SelectableLineRange {
    attached_prompt_picker_selectable_range(plain_line, width)
}

fn skill_picker_display_name(item: &PromptAssemblyDiscoveredSkill) -> &str {
    let trimmed_title = item.title.trim();
    if trimmed_title.is_empty() {
        item.skill_name.as_str()
    } else {
        trimmed_title
    }
}
