mod preview;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton};
use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Clear, Paragraph, Widget},
};
use runtime_domain::prompt_assembly::persistence::PromptAssemblyScope;
use runtime_domain::prompt_assembly::{
    PromptAssemblyDiscoveredSkill, PromptAssemblyEditorTarget, PromptAssemblyExtraPromptCandidate,
    PromptAssemblyManagedSource, PromptAssemblyManagerSource, PromptAssemblyMoveDirection,
    PromptAssemblyMutation, PromptSourceKind, PromptSourceOrigin, PromptSourceStatus,
    ResolvedPromptSource, natural_sort_text_cmp, next_default_extra_prompt_title,
};

use crate::{
    AppEffect, Model,
    display_width::display_width,
    fullscreen_list_chrome::fullscreen_list_chrome_rects,
    list_selection::ListNavigationDirection,
    overlay_input_result::OverlayInputResult,
    relative_age::left_pad_display_width,
    render_frame::RenderFrame,
    status_line::truncate_display_width_with_ellipsis,
    styled_text::render_line_with_full_width_background,
    theme::{
        build_labeled_rule, command_accent_text_style, panel_block, primary_text_style,
        secondary_text_style, subtle_rule_line, surface_text_style, table_header_text_style,
        tertiary_text_style,
    },
};

#[cfg(test)]
mod tests;

const PROMPT_OVERLAY_HEADER_INSET: usize = 2;
const PROMPT_OVERLAY_FOOTER_TABS_SUFFIX: &str = " · Tab tabs";
const SKILL_DISCOVERY_GENERATED_START: &str = "<!-- hunea:skill-discovery generated:start -->";
const SKILL_DISCOVERY_GENERATED_END: &str = "<!-- hunea:skill-discovery generated:end -->";
const PROMPT_OVERLAY_HEADER_TRAILING_PADDING: usize = 2;
const PROMPT_OVERLAY_ROW_PREFIX_WIDTH: usize = 1;
const PROMPT_OVERLAY_COLUMN_GAP: usize = 2;
const PROMPT_OVERLAY_OUTER_PADDING: usize = 2;
const PROMPT_OVERLAY_LEFT_SEL_WIDTH: usize = 3;
const PROMPT_OVERLAY_LEFT_ORD_WIDTH: usize = 3;
const PROMPT_OVERLAY_LEFT_KIND_WIDTH: usize = "discovery".len();
const PROMPT_OVERLAY_LEFT_SCOPE_WIDTH: usize = 7;
const PROMPT_OVERLAY_RIGHT_SCOPE_WIDTH: usize = 7;
const PROMPT_OVERLAY_SCOPE_TRAILING_PADDING: usize = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PromptOverlayFocus {
    Active,
    Inactive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PromptOverlayInactiveTab {
    LongLivedSkills,
    ExtraPrompts,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum PromptOverlayDialog {
    CreateExtraPromptScope { selected_scope: PromptAssemblyScope },
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum PromptOverlayExpandedRow {
    ActiveSource {
        reference_id: String,
        kind: PromptSourceKind,
    },
    InactiveExtraPrompt {
        reference_id: String,
    },
    InactiveDiscoveredSkill {
        skill_name: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PromptOverlayManagedStatus {
    Active,
    Disabled,
    Missing,
    Shadowed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum PromptOverlayLeftRow {
    ManagedSource {
        source: PromptAssemblyManagedSource,
        status: PromptOverlayManagedStatus,
        shadowed_count: usize,
    },
    ShadowedDetail {
        source: ResolvedPromptSource,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum PromptOverlayInactiveRow {
    ExtraPromptCandidate {
        source: PromptAssemblyExtraPromptCandidate,
        shadowed_count: usize,
    },
    ExtraPromptShadowedDetail {
        source: PromptAssemblyExtraPromptCandidate,
    },
    DiscoveredSkill {
        skill: PromptAssemblyDiscoveredSkill,
        shadowed_count: usize,
    },
    DiscoveredSkillShadowedDetail {
        skill: PromptAssemblyDiscoveredSkill,
    },
}

impl PromptOverlayInactiveTab {
    const ALL: [Self; 2] = [Self::LongLivedSkills, Self::ExtraPrompts];

    fn next(self) -> Self {
        match self {
            Self::LongLivedSkills => Self::ExtraPrompts,
            Self::ExtraPrompts => Self::LongLivedSkills,
        }
    }

    fn previous(self) -> Self {
        match self {
            Self::LongLivedSkills => Self::ExtraPrompts,
            Self::ExtraPrompts => Self::LongLivedSkills,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::LongLivedSkills => "Skill",
            Self::ExtraPrompts => "Custom Prompts",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PromptOverlayState {
    pub(crate) focus: PromptOverlayFocus,
    pub(crate) active_selected: usize,
    pub(crate) active_scroll: usize,
    pub(crate) active_selected_row_id: Option<String>,
    pub(crate) inactive_tab: PromptOverlayInactiveTab,
    pub(crate) inactive_selected: usize,
    pub(crate) inactive_scroll: usize,
    pub(crate) inactive_selected_row_id: Option<String>,
    expanded_row: Option<PromptOverlayExpandedRow>,
    dialog: Option<PromptOverlayDialog>,
    pub(crate) preview: Option<preview::PromptOverlayPreviewState>,
    pub(crate) draft_scope: PromptAssemblyScope,
    pub(crate) pending_editor: Option<PromptOverlayPendingEditor>,
}

impl Default for PromptOverlayState {
    fn default() -> Self {
        Self {
            focus: PromptOverlayFocus::Active,
            active_selected: 0,
            active_scroll: 0,
            active_selected_row_id: None,
            inactive_tab: PromptOverlayInactiveTab::LongLivedSkills,
            inactive_selected: 0,
            inactive_scroll: 0,
            inactive_selected_row_id: None,
            expanded_row: None,
            dialog: None,
            preview: None,
            draft_scope: PromptAssemblyScope::Project,
            pending_editor: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PromptOverlayPendingEditor {
    pub(crate) target: PromptAssemblyEditorTarget,
    pub(crate) original_draft: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum PromptOverlaySelection {
    ManagedSource(PromptAssemblyManagedSource),
    ResolvedSource(ResolvedPromptSource),
    ExtraPromptCandidate(PromptAssemblyExtraPromptCandidate),
    DiscoveredSkill(PromptAssemblyDiscoveredSkill),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PromptOverlayActionAvailability {
    can_edit: bool,
    can_add_custom: bool,
    can_remove: bool,
    can_toggle_selection: bool,
    can_reorder_active: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PromptOverlayLayoutRects {
    chrome: crate::fullscreen_list_chrome::FullscreenListChromeRects,
    left_pane: Rect,
    left_body: Rect,
    right_pane: Rect,
    right_body: Rect,
}

impl Model {
    pub(crate) fn prompt_overlay_active(&self) -> bool {
        self.prompt_overlay.is_some()
    }

    pub(crate) fn open_prompt_overlay(&mut self) {
        if self.prompt_overlay_active() {
            return;
        }

        self.close_model_panel();
        self.close_tool_approval_panel();
        self.close_composer_attached_ui();
        self.sync_composer_height();
        self.prompt_overlay = Some(PromptOverlayState::default());
        self.sync_prompt_overlay_state();
    }

    pub(crate) fn close_prompt_overlay(&mut self) {
        self.prompt_overlay = None;
        self.sync_composer_height();
    }

    pub(crate) fn handle_prompt_overlay_key(&mut self, key: KeyEvent) -> OverlayInputResult {
        if !self.prompt_overlay_active() {
            return OverlayInputResult::Ignored;
        }
        if self.prompt_overlay_preview_active() {
            return self.handle_prompt_overlay_preview_key(key);
        }
        if self.prompt_overlay_dialog_active() {
            return self.handle_prompt_overlay_dialog_key(key);
        }

        match key.code {
            KeyCode::Esc if key.modifiers.is_empty() => {
                self.close_prompt_overlay();
                OverlayInputResult::Handled
            }
            KeyCode::Left | KeyCode::Char('h') if key.modifiers.is_empty() => {
                self.set_prompt_overlay_focus(PromptOverlayFocus::Active);
                OverlayInputResult::Handled
            }
            KeyCode::Right | KeyCode::Char('l') if key.modifiers.is_empty() => {
                self.set_prompt_overlay_focus(PromptOverlayFocus::Inactive);
                OverlayInputResult::Handled
            }
            KeyCode::Tab if key.modifiers.is_empty() => {
                if self
                    .prompt_overlay
                    .as_ref()
                    .is_some_and(|state| state.focus == PromptOverlayFocus::Inactive)
                {
                    self.cycle_prompt_overlay_inactive_tab(1);
                }
                OverlayInputResult::Handled
            }
            KeyCode::BackTab => {
                if self
                    .prompt_overlay
                    .as_ref()
                    .is_some_and(|state| state.focus == PromptOverlayFocus::Inactive)
                {
                    self.cycle_prompt_overlay_inactive_tab(-1);
                }
                OverlayInputResult::Handled
            }
            KeyCode::Up | KeyCode::Char('k') if key.modifiers.is_empty() => {
                self.move_prompt_overlay_selection(ListNavigationDirection::Previous);
                OverlayInputResult::Handled
            }
            KeyCode::Down | KeyCode::Char('j') if key.modifiers.is_empty() => {
                self.move_prompt_overlay_selection(ListNavigationDirection::Next);
                OverlayInputResult::Handled
            }
            KeyCode::PageUp if key.modifiers.is_empty() => {
                self.move_prompt_overlay_page(ListNavigationDirection::Previous);
                OverlayInputResult::Handled
            }
            KeyCode::PageDown if key.modifiers.is_empty() => {
                self.move_prompt_overlay_page(ListNavigationDirection::Next);
                OverlayInputResult::Handled
            }
            KeyCode::Home if key.modifiers.is_empty() => {
                self.jump_prompt_overlay_selection_to_edge(true);
                OverlayInputResult::Handled
            }
            KeyCode::End if key.modifiers.is_empty() => {
                self.jump_prompt_overlay_selection_to_edge(false);
                OverlayInputResult::Handled
            }
            KeyCode::Char(' ') if key.modifiers.is_empty() => {
                self.open_selected_prompt_overlay_preview();
                OverlayInputResult::Handled
            }
            KeyCode::Char('p') if key.modifiers.is_empty() => {
                self.open_prompt_overlay_assembled_preview();
                OverlayInputResult::Handled
            }
            KeyCode::Char('e') if key.modifiers == KeyModifiers::CONTROL => {
                self.toggle_prompt_overlay_expanded_row();
                OverlayInputResult::Handled
            }
            KeyCode::Char('\u{0005}') if key.modifiers.is_empty() => {
                self.toggle_prompt_overlay_expanded_row();
                OverlayInputResult::Handled
            }
            KeyCode::Char('e') if key.modifiers.is_empty() => {
                OverlayInputResult::from_effect(self.open_prompt_overlay_editor_for_selection())
            }
            KeyCode::Char('a') if key.modifiers.is_empty() => {
                self.open_create_extra_prompt_scope_picker();
                OverlayInputResult::Handled
            }
            KeyCode::Char('i') | KeyCode::Char('I') if key.modifiers.is_empty() => {
                OverlayInputResult::Handled
            }
            KeyCode::Char('d') if key.modifiers.is_empty() => {
                OverlayInputResult::from_effect(self.remove_selected_prompt_source())
            }
            KeyCode::Char('x') if key.modifiers.is_empty() => {
                OverlayInputResult::from_effect(self.toggle_selected_prompt_source_enabled())
            }
            KeyCode::Char('K') if allows_shift_only_modifier(key.modifiers) => {
                OverlayInputResult::from_effect(
                    self.move_selected_active_source(PromptAssemblyMoveDirection::Up),
                )
            }
            KeyCode::Char('J') if allows_shift_only_modifier(key.modifiers) => {
                OverlayInputResult::from_effect(
                    self.move_selected_active_source(PromptAssemblyMoveDirection::Down),
                )
            }
            KeyCode::Char('r') if key.modifiers.is_empty() => {
                OverlayInputResult::from_effect(self.restore_selected_core_system_override())
            }
            _ => OverlayInputResult::Handled,
        }
    }

    pub(crate) fn render_prompt_overlay(&mut self, frame: &mut RenderFrame<'_>, area: Rect) {
        let Some(state) = self.prompt_overlay.as_ref() else {
            return;
        };
        if state.preview.is_some() {
            self.render_prompt_overlay_preview(frame, area);
            return;
        }

        frame.render_widget(Clear, area);
        let Some(layout) = prompt_overlay_layout_rects(area) else {
            return;
        };

        frame.render_widget(
            Paragraph::new(
                self.prompt_overlay_header_line(usize::from(area.width), state.inactive_tab),
            ),
            layout.chrome.header,
        );
        frame.render_widget(
            Paragraph::new(subtle_rule_line(usize::from(area.width), self.palette)),
            layout.chrome.header_rule,
        );
        let gutter_x = layout.left_pane.x.saturating_add(layout.left_pane.width);
        let gutter = Rect::new(gutter_x, layout.left_pane.y, 1, layout.left_pane.height);

        if gutter.width > 0 {
            frame.render_widget(
                Paragraph::new(vertical_rule_lines(
                    usize::from(gutter.height),
                    self.palette,
                )),
                gutter,
            );
        }

        self.render_prompt_overlay_active_pane(frame, layout.left_pane, state);
        self.render_prompt_overlay_inactive_pane(frame, layout.right_pane, state);

        let focused_page = self.prompt_overlay_focused_page_label(state, area.height);
        frame.render_widget(
            Paragraph::new(build_labeled_rule(area.width, focused_page, self.palette)),
            layout.chrome.page_rule,
        );
        frame.render_widget(
            Paragraph::new(Line::styled(
                self.prompt_overlay_footer_hint(area.width),
                tertiary_text_style(self.palette).add_modifier(Modifier::ITALIC),
            )),
            layout.chrome.footer,
        );

        if let Some(dialog) = state.dialog.as_ref() {
            self.render_prompt_overlay_dialog(frame, layout.right_pane, dialog);
        }
    }

    pub(crate) fn apply_prompt_overlay_external_editor_finished(
        &mut self,
        draft_path: &std::path::Path,
        failed: bool,
    ) -> Option<Option<AppEffect>> {
        let pending_editor = self
            .prompt_overlay
            .as_ref()
            .and_then(|state| state.pending_editor.as_ref())
            .cloned()?;
        let state = self.prompt_overlay.as_mut()?;
        state.pending_editor = None;

        if failed {
            let _ = std::fs::remove_file(draft_path);
            self.show_toast(crate::toast::ToastSeverity::Error, "External editor failed");
            return Some(None);
        }
        let content = match std::fs::read_to_string(draft_path) {
            Ok(content) => content,
            Err(_) => {
                let _ = std::fs::remove_file(draft_path);
                self.show_toast(
                    crate::toast::ToastSeverity::Error,
                    "Failed to read external editor draft",
                );
                return Some(None);
            }
        };
        let _ = std::fs::remove_file(draft_path);
        let normalized_content = normalize_prompt_overlay_external_editor_draft(&content);
        if normalized_content == pending_editor.original_draft {
            return Some(None);
        }
        Some(Some(AppEffect::MutatePromptAssembly {
            mutation: PromptAssemblyMutation::SaveEditorTarget {
                target: pending_editor.target,
                content: normalized_content,
            },
        }))
    }

    fn open_selected_prompt_overlay_preview(&mut self) {
        let Some(selection) = self.selected_prompt_overlay_selection() else {
            return;
        };
        match selection {
            PromptOverlaySelection::ManagedSource(source) => {
                let source = ResolvedPromptSource {
                    reference_id: source.reference_id,
                    kind: source.kind,
                    title: source.title,
                    origin: source.origin,
                    status: PromptSourceStatus::Active {
                        order: source.order,
                    },
                };
                let Some(manager_source) = self.manager_source_for_resolved_source(&source) else {
                    return;
                };
                self.open_prompt_overlay_source_preview(manager_source);
            }
            PromptOverlaySelection::ResolvedSource(source) => {
                let Some(manager_source) = self.manager_source_for_resolved_source(&source) else {
                    return;
                };
                self.open_prompt_overlay_source_preview(manager_source);
            }
            PromptOverlaySelection::ExtraPromptCandidate(candidate) => {
                self.open_prompt_overlay_plain_text_preview(candidate.title, &candidate.body, None);
            }
            PromptOverlaySelection::DiscoveredSkill(skill) => {
                self.open_prompt_overlay_skill_preview(skill);
            }
        }
    }

    fn open_prompt_overlay_skill_preview(&mut self, skill: PromptAssemblyDiscoveredSkill) {
        let preview_notice = (!skill.can_select_for_discovery).then(|| {
            "Manual-only skill: `disable-model-invocation: true` keeps this skill out of skill discovery."
                .to_string()
        });
        self.open_prompt_overlay_plain_text_preview(
            skill.title.clone(),
            &skill.body,
            preview_notice,
        );
    }

    fn prompt_overlay_dialog_active(&self) -> bool {
        self.prompt_overlay
            .as_ref()
            .and_then(|state| state.dialog.as_ref())
            .is_some()
    }

    fn handle_prompt_overlay_dialog_key(&mut self, key: KeyEvent) -> OverlayInputResult {
        match key.code {
            KeyCode::Esc if key.modifiers.is_empty() => {
                if let Some(state) = self.prompt_overlay.as_mut() {
                    state.dialog = None;
                }
                OverlayInputResult::Handled
            }
            KeyCode::Left | KeyCode::Char('h') | KeyCode::Up | KeyCode::Char('k')
                if key.modifiers.is_empty() =>
            {
                self.toggle_prompt_overlay_dialog_scope();
                OverlayInputResult::Handled
            }
            KeyCode::Right | KeyCode::Char('l') | KeyCode::Down | KeyCode::Char('j')
                if key.modifiers.is_empty() =>
            {
                self.toggle_prompt_overlay_dialog_scope();
                OverlayInputResult::Handled
            }
            KeyCode::Enter if key.modifiers.is_empty() => {
                OverlayInputResult::from_effect(self.confirm_prompt_overlay_dialog())
            }
            _ => OverlayInputResult::Handled,
        }
    }

    fn toggle_prompt_overlay_dialog_scope(&mut self) {
        let Some(state) = self.prompt_overlay.as_mut() else {
            return;
        };
        let Some(PromptOverlayDialog::CreateExtraPromptScope { selected_scope }) =
            state.dialog.as_mut()
        else {
            return;
        };
        *selected_scope = match selected_scope {
            PromptAssemblyScope::Global => PromptAssemblyScope::Project,
            PromptAssemblyScope::Project => PromptAssemblyScope::Global,
        };
    }

    fn confirm_prompt_overlay_dialog(&mut self) -> Option<AppEffect> {
        let dialog = self
            .prompt_overlay
            .as_ref()
            .and_then(|state| state.dialog.clone())?;
        if let Some(state) = self.prompt_overlay.as_mut() {
            state.dialog = None;
        }
        match dialog {
            PromptOverlayDialog::CreateExtraPromptScope { selected_scope } => {
                self.create_extra_prompt_from_overlay(selected_scope)
            }
        }
    }

    fn toggle_prompt_overlay_expanded_row(&mut self) {
        let Some(focus) = self.prompt_overlay.as_ref().map(|state| state.focus) else {
            return;
        };
        let current_expanded_row = self
            .prompt_overlay
            .as_ref()
            .and_then(|state| state.expanded_row.clone());
        let next_expanded_row = match focus {
            PromptOverlayFocus::Active => match self.selected_prompt_overlay_left_row() {
                Some(PromptOverlayLeftRow::ManagedSource {
                    source,
                    shadowed_count,
                    ..
                }) if shadowed_count > 0 => {
                    let row = PromptOverlayExpandedRow::ActiveSource {
                        reference_id: source.reference_id,
                        kind: source.kind,
                    };
                    if current_expanded_row.as_ref() == Some(&row) {
                        None
                    } else {
                        Some(row)
                    }
                }
                Some(PromptOverlayLeftRow::ShadowedDetail { .. }) => None,
                _ => return,
            },
            PromptOverlayFocus::Inactive => match self.selected_prompt_overlay_inactive_row() {
                Some(PromptOverlayInactiveRow::ExtraPromptCandidate {
                    source,
                    shadowed_count,
                }) if shadowed_count > 0 => {
                    let row = PromptOverlayExpandedRow::InactiveExtraPrompt {
                        reference_id: source.reference_id,
                    };
                    if current_expanded_row.as_ref() == Some(&row) {
                        None
                    } else {
                        Some(row)
                    }
                }
                Some(PromptOverlayInactiveRow::DiscoveredSkill {
                    skill,
                    shadowed_count,
                }) if shadowed_count > 0 => {
                    let row = PromptOverlayExpandedRow::InactiveDiscoveredSkill {
                        skill_name: skill.skill_name,
                    };
                    if current_expanded_row.as_ref() == Some(&row) {
                        None
                    } else {
                        Some(row)
                    }
                }
                Some(
                    PromptOverlayInactiveRow::ExtraPromptShadowedDetail { .. }
                    | PromptOverlayInactiveRow::DiscoveredSkillShadowedDetail { .. },
                ) => None,
                _ => return,
            },
        };
        let Some(state) = self.prompt_overlay.as_mut() else {
            return;
        };
        state.expanded_row = next_expanded_row;
        self.sync_prompt_overlay_state();
    }

    fn open_create_extra_prompt_scope_picker(&mut self) {
        let Some(state) = self.prompt_overlay.as_mut() else {
            return;
        };
        if state.focus != PromptOverlayFocus::Inactive
            || state.inactive_tab != PromptOverlayInactiveTab::ExtraPrompts
        {
            return;
        }
        state.dialog = Some(PromptOverlayDialog::CreateExtraPromptScope {
            selected_scope: PromptAssemblyScope::Project,
        });
    }

    pub(crate) fn open_prompt_overlay_editor_for_selection(&mut self) -> Option<AppEffect> {
        let scope = self
            .prompt_overlay
            .as_ref()
            .map(|state| state.draft_scope)
            .unwrap_or(PromptAssemblyScope::Project);
        let (target, initial_content) = match self.selected_prompt_overlay_selection()? {
            PromptOverlaySelection::ManagedSource(source) => {
                let selected = ResolvedPromptSource {
                    reference_id: source.reference_id.clone(),
                    kind: source.kind,
                    title: source.title.clone(),
                    origin: source.origin,
                    status: PromptSourceStatus::Active {
                        order: source.order,
                    },
                };
                let manager_source = self.manager_source_for_resolved_source(&selected)?;
                match selected.kind {
                    PromptSourceKind::CoreSystemPrompt => (
                        PromptAssemblyEditorTarget::CoreSystemOverride { scope },
                        self.core_system_editor_body_for_scope(scope),
                    ),
                    PromptSourceKind::SkillDiscovery => (
                        PromptAssemblyEditorTarget::SkillDiscovery { scope },
                        self.skill_discovery_editor_body_for_scope(scope),
                    ),
                    PromptSourceKind::ExtraPrompt => {
                        let origin = selected.origin?;
                        (
                            PromptAssemblyEditorTarget::ExtraPrompt {
                                scope: prompt_scope_from_origin(origin)?,
                                reference_id: selected.reference_id.clone(),
                            },
                            manager_source.body.unwrap_or_default(),
                        )
                    }
                    PromptSourceKind::LongLivedSkill => return None,
                }
            }
            PromptOverlaySelection::ExtraPromptCandidate(candidate) => (
                PromptAssemblyEditorTarget::ExtraPrompt {
                    scope: prompt_scope_from_origin(candidate.origin)?,
                    reference_id: candidate.reference_id.clone(),
                },
                candidate.body,
            ),
            PromptOverlaySelection::ResolvedSource(selected) => {
                let manager_source = self.manager_source_for_resolved_source(&selected)?;
                match selected.kind {
                    PromptSourceKind::ExtraPrompt => (
                        PromptAssemblyEditorTarget::ExtraPrompt {
                            scope: prompt_scope_from_origin(selected.origin?)?,
                            reference_id: selected.reference_id.clone(),
                        },
                        manager_source.body.unwrap_or_default(),
                    ),
                    PromptSourceKind::CoreSystemPrompt
                    | PromptSourceKind::SkillDiscovery
                    | PromptSourceKind::LongLivedSkill => return None,
                }
            }
            PromptOverlaySelection::DiscoveredSkill(_) => return None,
        };

        let launch = self.prepare_external_editor_launch_for_content(&initial_content)?;
        if let Some(state) = self.prompt_overlay.as_mut() {
            state.pending_editor = Some(PromptOverlayPendingEditor {
                target,
                original_draft: initial_content,
            });
        }
        Some(AppEffect::LaunchExternalEditor(launch))
    }

    fn remove_selected_prompt_source(&mut self) -> Option<AppEffect> {
        match self.selected_prompt_overlay_selection()? {
            PromptOverlaySelection::ManagedSource(selected) => {
                if selected.kind == PromptSourceKind::CoreSystemPrompt {
                    return None;
                }
                Some(AppEffect::MutatePromptAssembly {
                    mutation: PromptAssemblyMutation::RemovePromptSource {
                        scope: prompt_scope_from_origin(selected.origin?)?,
                        kind: selected.kind,
                        reference_id: selected.reference_id,
                    },
                })
            }
            PromptOverlaySelection::ExtraPromptCandidate(candidate) => {
                Some(AppEffect::MutatePromptAssembly {
                    mutation: PromptAssemblyMutation::DeleteExtraPrompt {
                        scope: prompt_scope_from_origin(candidate.origin)?,
                        reference_id: candidate.reference_id,
                    },
                })
            }
            PromptOverlaySelection::ResolvedSource(selected) => {
                if selected.kind == PromptSourceKind::CoreSystemPrompt {
                    return None;
                }
                Some(AppEffect::MutatePromptAssembly {
                    mutation: PromptAssemblyMutation::RemovePromptSource {
                        scope: prompt_scope_from_origin(selected.origin?)?,
                        kind: selected.kind,
                        reference_id: selected.reference_id,
                    },
                })
            }
            PromptOverlaySelection::DiscoveredSkill(_) => None,
        }
    }

    fn restore_selected_core_system_override(&mut self) -> Option<AppEffect> {
        let selected = self.selected_prompt_overlay_source()?;
        if selected.kind != PromptSourceKind::CoreSystemPrompt {
            return None;
        }
        let scope = self
            .prompt_overlay
            .as_ref()
            .map(|state| state.draft_scope)
            .unwrap_or(PromptAssemblyScope::Project);
        Some(AppEffect::MutatePromptAssembly {
            mutation: PromptAssemblyMutation::RestoreCoreSystemOverride { scope },
        })
    }

    fn selected_prompt_overlay_source(&self) -> Option<ResolvedPromptSource> {
        match self.selected_prompt_overlay_selection()? {
            PromptOverlaySelection::ManagedSource(selected) => Some(ResolvedPromptSource {
                reference_id: selected.reference_id,
                kind: selected.kind,
                title: selected.title,
                origin: selected.origin,
                status: PromptSourceStatus::Active {
                    order: selected.order,
                },
            }),
            PromptOverlaySelection::ResolvedSource(source) => Some(source),
            PromptOverlaySelection::ExtraPromptCandidate(_)
            | PromptOverlaySelection::DiscoveredSkill(_) => None,
        }
    }

    fn selected_prompt_overlay_managed_source(&self) -> Option<PromptAssemblyManagedSource> {
        let state = self.prompt_overlay.as_ref()?;
        match state.focus {
            PromptOverlayFocus::Active => match self
                .prompt_overlay_left_rows()
                .get(state.active_selected)?
                .clone()
            {
                PromptOverlayLeftRow::ManagedSource { source, .. } => Some(source),
                PromptOverlayLeftRow::ShadowedDetail { .. } => None,
            },
            PromptOverlayFocus::Inactive => None,
        }
    }

    fn selected_prompt_overlay_left_row(&self) -> Option<PromptOverlayLeftRow> {
        let state = self.prompt_overlay.as_ref()?;
        if state.focus != PromptOverlayFocus::Active {
            return None;
        }
        self.prompt_overlay_left_rows()
            .get(state.active_selected)
            .cloned()
    }

    fn selected_prompt_overlay_inactive_row(&self) -> Option<PromptOverlayInactiveRow> {
        let state = self.prompt_overlay.as_ref()?;
        if state.focus != PromptOverlayFocus::Inactive {
            return None;
        }
        self.prompt_overlay_inactive_rows(state.inactive_tab)
            .get(state.inactive_selected)
            .cloned()
    }

    fn selected_prompt_overlay_selection(&self) -> Option<PromptOverlaySelection> {
        let state = self.prompt_overlay.as_ref()?;
        match state.focus {
            PromptOverlayFocus::Active => match self
                .prompt_overlay_left_rows()
                .get(state.active_selected)?
                .clone()
            {
                PromptOverlayLeftRow::ManagedSource { source, .. } => {
                    Some(PromptOverlaySelection::ManagedSource(source))
                }
                PromptOverlayLeftRow::ShadowedDetail { source } => {
                    Some(PromptOverlaySelection::ResolvedSource(source))
                }
            },
            PromptOverlayFocus::Inactive => match self
                .prompt_overlay_inactive_rows(state.inactive_tab)
                .get(state.inactive_selected)?
                .clone()
            {
                PromptOverlayInactiveRow::ExtraPromptCandidate { source, .. }
                | PromptOverlayInactiveRow::ExtraPromptShadowedDetail { source } => {
                    Some(PromptOverlaySelection::ExtraPromptCandidate(source))
                }
                PromptOverlayInactiveRow::DiscoveredSkill { skill, .. }
                | PromptOverlayInactiveRow::DiscoveredSkillShadowedDetail { skill } => {
                    Some(PromptOverlaySelection::DiscoveredSkill(skill))
                }
            },
        }
    }

    fn manager_source_for_resolved_source(
        &self,
        selected: &ResolvedPromptSource,
    ) -> Option<PromptAssemblyManagerSource> {
        self.prompt_assembly
            .sources
            .iter()
            .find(|source| {
                source.reference_id == selected.reference_id
                    && source.kind == selected.kind
                    && source.origin == selected.origin
            })
            .cloned()
    }

    fn prompt_overlay_left_rows(&self) -> Vec<PromptOverlayLeftRow> {
        let expanded_row = self
            .prompt_overlay
            .as_ref()
            .and_then(|state| state.expanded_row.clone());

        let mut rows = Vec::new();
        for source in &self.prompt_assembly.managed_sources {
            let status = self.prompt_overlay_managed_status_for(source);
            if matches!(status, PromptOverlayManagedStatus::Shadowed) {
                continue;
            }
            let shadowed_sources =
                self.prompt_overlay_shadowed_sources_for(source.kind, &source.reference_id);
            let shadowed_count = shadowed_sources.len();
            rows.push(PromptOverlayLeftRow::ManagedSource {
                source: source.clone(),
                status,
                shadowed_count,
            });
            let should_show_shadowed = expanded_row.as_ref().is_some_and(|expanded| {
                matches!(
                    expanded,
                    PromptOverlayExpandedRow::ActiveSource { reference_id, kind }
                        if reference_id == &source.reference_id && kind == &source.kind
                )
            });
            if should_show_shadowed {
                rows.extend(
                    shadowed_sources
                        .into_iter()
                        .map(|source| PromptOverlayLeftRow::ShadowedDetail { source }),
                );
            }
        }
        rows
    }

    fn prompt_overlay_inactive_rows(
        &self,
        tab: PromptOverlayInactiveTab,
    ) -> Vec<PromptOverlayInactiveRow> {
        match tab {
            PromptOverlayInactiveTab::ExtraPrompts => self.prompt_overlay_extra_rows(),
            PromptOverlayInactiveTab::LongLivedSkills => self.prompt_overlay_skill_rows(),
        }
    }

    fn prompt_overlay_extra_rows(&self) -> Vec<PromptOverlayInactiveRow> {
        let expanded_row = self
            .prompt_overlay
            .as_ref()
            .and_then(|state| state.expanded_row.clone());
        let mut groups = self
            .prompt_assembly
            .extra_prompt_candidates
            .iter()
            .cloned()
            .fold(
                std::collections::HashMap::<String, Vec<PromptAssemblyExtraPromptCandidate>>::new(),
                |mut groups, source| {
                    groups
                        .entry(source.reference_id.clone())
                        .or_default()
                        .push(source);
                    groups
                },
            )
            .into_values()
            .collect::<Vec<_>>();
        groups.sort_by(|left, right| {
            let left_winner = prompt_overlay_extra_candidate_winner(left);
            let right_winner = prompt_overlay_extra_candidate_winner(right);
            natural_sort_text_cmp(&left_winner.title, &right_winner.title)
                .then_with(|| left_winner.reference_id.cmp(&right_winner.reference_id))
        });

        let mut rows = Vec::new();
        for group in groups {
            let (winner, shadowed) = prompt_overlay_partition_extra_candidates(group);
            let shadowed_count = shadowed.len();
            rows.push(PromptOverlayInactiveRow::ExtraPromptCandidate {
                source: winner.clone(),
                shadowed_count,
            });
            let should_show_shadowed = expanded_row.as_ref().is_some_and(|expanded| {
                matches!(
                    expanded,
                    PromptOverlayExpandedRow::InactiveExtraPrompt { reference_id }
                        if reference_id == &winner.reference_id
                )
            });
            if should_show_shadowed {
                rows.extend(
                    shadowed.into_iter().map(|source| {
                        PromptOverlayInactiveRow::ExtraPromptShadowedDetail { source }
                    }),
                );
            }
        }
        rows
    }

    fn prompt_overlay_skill_rows(&self) -> Vec<PromptOverlayInactiveRow> {
        let expanded_row = self
            .prompt_overlay
            .as_ref()
            .and_then(|state| state.expanded_row.clone());
        let mut groups = self
            .prompt_assembly
            .discovered_skills
            .iter()
            .cloned()
            .fold(
                std::collections::HashMap::<String, Vec<PromptAssemblyDiscoveredSkill>>::new(),
                |mut groups, skill| {
                    groups
                        .entry(skill.skill_name.clone())
                        .or_default()
                        .push(skill);
                    groups
                },
            )
            .into_values()
            .collect::<Vec<_>>();
        groups.sort_by(|left, right| {
            let left_winner = prompt_overlay_discovered_skill_winner(left);
            let right_winner = prompt_overlay_discovered_skill_winner(right);
            (!left_winner.can_select_for_discovery)
                .cmp(&!right_winner.can_select_for_discovery)
                .then_with(|| natural_sort_text_cmp(&left_winner.title, &right_winner.title))
                .then_with(|| {
                    natural_sort_text_cmp(&left_winner.skill_name, &right_winner.skill_name)
                })
        });

        let mut rows = Vec::new();
        for group in groups {
            let (winner, shadowed) = prompt_overlay_partition_discovered_skills(group);
            let shadowed_count = shadowed.len();
            rows.push(PromptOverlayInactiveRow::DiscoveredSkill {
                skill: winner.clone(),
                shadowed_count,
            });
            let should_show_shadowed = expanded_row.as_ref().is_some_and(|expanded| {
                matches!(
                    expanded,
                    PromptOverlayExpandedRow::InactiveDiscoveredSkill { skill_name }
                        if skill_name == &winner.skill_name
                )
            });
            if should_show_shadowed {
                rows.extend(shadowed.into_iter().map(|skill| {
                    PromptOverlayInactiveRow::DiscoveredSkillShadowedDetail { skill }
                }));
            }
        }
        rows
    }

    fn prompt_overlay_managed_status_for(
        &self,
        source: &PromptAssemblyManagedSource,
    ) -> PromptOverlayManagedStatus {
        if self
            .prompt_assembly
            .snapshot
            .active_sources
            .iter()
            .any(|resolved| {
                prompt_overlay_matches_resolved_source(
                    resolved,
                    source.kind,
                    &source.reference_id,
                    source.origin,
                )
            })
        {
            return PromptOverlayManagedStatus::Active;
        }

        self.prompt_assembly
            .snapshot
            .inactive_sources
            .iter()
            .find(|resolved| {
                prompt_overlay_matches_resolved_source(
                    resolved,
                    source.kind,
                    &source.reference_id,
                    source.origin,
                )
            })
            .map_or(
                PromptOverlayManagedStatus::Disabled,
                |resolved| match resolved.status {
                    PromptSourceStatus::Inactive { reason } => match reason {
                        runtime_domain::prompt_assembly::PromptSourceInactiveReason::Disabled => {
                            PromptOverlayManagedStatus::Disabled
                        }
                        runtime_domain::prompt_assembly::PromptSourceInactiveReason::Missing => {
                            PromptOverlayManagedStatus::Missing
                        }
                        runtime_domain::prompt_assembly::PromptSourceInactiveReason::Shadowed => {
                            PromptOverlayManagedStatus::Shadowed
                        }
                    },
                    PromptSourceStatus::Active { .. } => PromptOverlayManagedStatus::Active,
                },
            )
    }

    fn prompt_overlay_shadowed_sources_for(
        &self,
        kind: PromptSourceKind,
        reference_id: &str,
    ) -> Vec<ResolvedPromptSource> {
        self.prompt_assembly
            .snapshot
            .inactive_sources
            .iter()
            .filter(|source| {
                source.kind == kind
                    && source.reference_id == reference_id
                    && matches!(
                        source.status,
                        PromptSourceStatus::Inactive {
                            reason:
                                runtime_domain::prompt_assembly::PromptSourceInactiveReason::Shadowed
                        }
                    )
            })
            .cloned()
            .collect()
    }

    fn move_selected_active_source(
        &mut self,
        direction: PromptAssemblyMoveDirection,
    ) -> Option<AppEffect> {
        let selected = self.selected_prompt_overlay_managed_source()?;
        if selected.kind == PromptSourceKind::CoreSystemPrompt {
            return None;
        }
        Some(AppEffect::MutatePromptAssembly {
            mutation: PromptAssemblyMutation::MoveActiveSource {
                scope: prompt_scope_from_origin(selected.origin?)?,
                kind: selected.kind,
                reference_id: selected.reference_id,
                direction,
            },
        })
    }

    fn toggle_selected_prompt_source_enabled(&mut self) -> Option<AppEffect> {
        if let Some(selected) = self.selected_prompt_overlay_managed_source() {
            if selected.kind == PromptSourceKind::CoreSystemPrompt {
                return None;
            }
            let scope = if selected.kind == PromptSourceKind::SkillDiscovery {
                self.prompt_overlay
                    .as_ref()
                    .map(|state| state.draft_scope)
                    .unwrap_or(PromptAssemblyScope::Project)
            } else {
                prompt_scope_from_origin(selected.origin?)?
            };
            return Some(AppEffect::MutatePromptAssembly {
                mutation: PromptAssemblyMutation::SetPromptSourceEnabled {
                    scope,
                    kind: selected.kind,
                    reference_id: selected.reference_id,
                    enabled: !selected.enabled,
                },
            });
        }

        match self.selected_prompt_overlay_selection()? {
            PromptOverlaySelection::ManagedSource(_) => None,
            PromptOverlaySelection::ResolvedSource(selected) => {
                if selected.kind == PromptSourceKind::CoreSystemPrompt {
                    return None;
                }
                Some(AppEffect::MutatePromptAssembly {
                    mutation: PromptAssemblyMutation::SetPromptSourceEnabled {
                        scope: prompt_scope_from_origin(selected.origin?)?,
                        kind: selected.kind,
                        reference_id: selected.reference_id,
                        enabled: false,
                    },
                })
            }
            PromptOverlaySelection::ExtraPromptCandidate(candidate) => {
                Some(AppEffect::MutatePromptAssembly {
                    mutation: PromptAssemblyMutation::SetExtraPromptSelected {
                        scope: prompt_scope_from_origin(candidate.origin)?,
                        reference_id: candidate.reference_id,
                        selected: !candidate.selected,
                    },
                })
            }
            PromptOverlaySelection::DiscoveredSkill(skill) => {
                if !skill.can_select_for_discovery {
                    return None;
                }
                Some(AppEffect::MutatePromptAssembly {
                    mutation: PromptAssemblyMutation::SetDiscoveredSkillSelected {
                        scope: prompt_scope_from_origin(skill.origin)?,
                        skill_name: skill.skill_name,
                        selected: !skill.selected,
                    },
                })
            }
        }
    }

    fn create_extra_prompt_from_overlay(&self, scope: PromptAssemblyScope) -> Option<AppEffect> {
        let state = self.prompt_overlay.as_ref()?;
        if state.focus != PromptOverlayFocus::Inactive
            || state.inactive_tab != PromptOverlayInactiveTab::ExtraPrompts
        {
            return None;
        }
        let content = self.default_extra_prompt_body_for_scope(scope);
        Some(AppEffect::MutatePromptAssembly {
            mutation: PromptAssemblyMutation::CreateExtraPrompt { scope, content },
        })
    }

    fn default_extra_prompt_body_for_scope(&self, scope: PromptAssemblyScope) -> String {
        let title = next_default_extra_prompt_title(
            self.prompt_assembly
                .managed_sources
                .iter()
                .filter(|source| {
                    source.kind == PromptSourceKind::ExtraPrompt
                        && source
                            .origin
                            .and_then(prompt_scope_from_origin)
                            .is_some_and(|origin_scope| origin_scope == scope)
                })
                .map(|source| source.title.as_str())
                .chain(
                    self.prompt_assembly
                        .extra_prompt_candidates
                        .iter()
                        .filter(|candidate| {
                            prompt_scope_from_origin(candidate.origin)
                                .is_some_and(|origin_scope| origin_scope == scope)
                        })
                        .map(|candidate| candidate.title.as_str()),
                ),
        );
        format!("# {title}\n")
    }

    fn prompt_overlay_action_availability(&self) -> PromptOverlayActionAvailability {
        match self.selected_prompt_overlay_selection() {
            Some(PromptOverlaySelection::ManagedSource(source)) => {
                let can_remove = !matches!(
                    source.kind,
                    PromptSourceKind::CoreSystemPrompt | PromptSourceKind::SkillDiscovery
                );
                PromptOverlayActionAvailability {
                    can_edit: source.kind != PromptSourceKind::LongLivedSkill,
                    can_add_custom: false,
                    can_remove,
                    can_toggle_selection: true,
                    can_reorder_active: source.kind != PromptSourceKind::CoreSystemPrompt,
                }
            }
            Some(PromptOverlaySelection::ResolvedSource(source)) => {
                let can_remove = !matches!(
                    source.kind,
                    PromptSourceKind::CoreSystemPrompt | PromptSourceKind::SkillDiscovery
                );
                PromptOverlayActionAvailability {
                    can_edit: source.kind == PromptSourceKind::ExtraPrompt,
                    can_add_custom: false,
                    can_remove,
                    can_toggle_selection: source.kind != PromptSourceKind::CoreSystemPrompt,
                    can_reorder_active: false,
                }
            }
            Some(PromptOverlaySelection::ExtraPromptCandidate(_)) => {
                PromptOverlayActionAvailability {
                    can_edit: true,
                    can_add_custom: self.prompt_overlay_can_add_custom(),
                    can_remove: true,
                    can_toggle_selection: true,
                    can_reorder_active: false,
                }
            }
            Some(PromptOverlaySelection::DiscoveredSkill(_)) => PromptOverlayActionAvailability {
                can_edit: false,
                can_add_custom: false,
                can_remove: false,
                can_toggle_selection: true,
                can_reorder_active: false,
            },
            None => PromptOverlayActionAvailability {
                can_edit: false,
                can_add_custom: self.prompt_overlay_can_add_custom(),
                can_remove: false,
                can_toggle_selection: false,
                can_reorder_active: false,
            },
        }
    }

    fn prompt_overlay_can_add_custom(&self) -> bool {
        self.prompt_overlay.as_ref().is_some_and(|state| {
            state.focus == PromptOverlayFocus::Inactive
                && state.inactive_tab == PromptOverlayInactiveTab::ExtraPrompts
        })
    }

    fn core_system_editor_body_for_scope(&self, scope: PromptAssemblyScope) -> String {
        match scope {
            PromptAssemblyScope::Global => self
                .prompt_assembly
                .global_core_system_override
                .clone()
                .unwrap_or_else(|| self.prompt_assembly.builtin_core_system_body.clone()),
            PromptAssemblyScope::Project => self
                .prompt_assembly
                .project_core_system_override
                .clone()
                .or_else(|| self.prompt_assembly.global_core_system_override.clone())
                .unwrap_or_else(|| self.prompt_assembly.builtin_core_system_body.clone()),
        }
    }

    fn skill_discovery_editor_body_for_scope(&self, scope: PromptAssemblyScope) -> String {
        let origin = Some(match scope {
            PromptAssemblyScope::Global => PromptSourceOrigin::Global,
            PromptAssemblyScope::Project => PromptSourceOrigin::Project,
        });
        let body = self
            .prompt_assembly
            .sources
            .iter()
            .find(|source| {
                source.reference_id == "skill-discovery"
                    && source.kind == PromptSourceKind::SkillDiscovery
                    && source.origin == origin
            })
            .and_then(|source| source.body.clone())
            .unwrap_or_default();
        if body.is_empty() {
            return format!("{SKILL_DISCOVERY_GENERATED_START}\n{SKILL_DISCOVERY_GENERATED_END}\n");
        }
        if body.contains(SKILL_DISCOVERY_GENERATED_START)
            && body.contains(SKILL_DISCOVERY_GENERATED_END)
        {
            return body;
        }
        format!("{SKILL_DISCOVERY_GENERATED_START}\n{body}\n{SKILL_DISCOVERY_GENERATED_END}\n")
    }

    pub(crate) fn move_prompt_overlay_selection_by_delta(&mut self, delta: isize) {
        let Some(direction) = ListNavigationDirection::from_delta(delta) else {
            return;
        };
        self.move_prompt_overlay_selection(direction);
    }

    fn set_prompt_overlay_focus(&mut self, focus: PromptOverlayFocus) {
        let Some(state) = self.prompt_overlay.as_mut() else {
            return;
        };
        state.focus = focus;
        self.sync_prompt_overlay_state();
    }

    pub(crate) fn handle_prompt_overlay_mouse_down(
        &mut self,
        button: MouseButton,
        column: u16,
        row: u16,
    ) -> OverlayInputResult {
        if !self.prompt_overlay_active() {
            return OverlayInputResult::Ignored;
        }
        if button != MouseButton::Left || self.prompt_overlay_preview_active() {
            return OverlayInputResult::Handled;
        }
        if self.prompt_overlay_dialog_active() {
            return self.handle_prompt_overlay_dialog_mouse_down(column, row);
        }

        let Some((active_tab, _focus)) = self
            .prompt_overlay
            .as_ref()
            .map(|state| (state.inactive_tab, state.focus))
        else {
            return OverlayInputResult::Handled;
        };
        let Some(layout) = prompt_overlay_layout_rects(Rect::new(0, 0, self.width, self.height))
        else {
            return OverlayInputResult::Handled;
        };

        if let Some(tab) =
            self.prompt_overlay_header_tab_at(column, row, layout.chrome.header, active_tab)
        {
            self.set_prompt_overlay_focus(PromptOverlayFocus::Inactive);
            if active_tab != tab {
                self.set_prompt_overlay_inactive_tab(tab);
            }
            return OverlayInputResult::Handled;
        }

        if prompt_overlay_rect_contains(layout.left_pane, column, row) {
            self.set_prompt_overlay_focus(PromptOverlayFocus::Active);
            if let Some(visible_offset) =
                prompt_overlay_visible_offset_for_row(layout.left_body, row)
            {
                self.select_prompt_overlay_active_row(visible_offset);
            }
            return OverlayInputResult::Handled;
        }

        if prompt_overlay_rect_contains(layout.right_pane, column, row) {
            self.set_prompt_overlay_focus(PromptOverlayFocus::Inactive);
            if let Some(visible_offset) =
                prompt_overlay_visible_offset_for_row(layout.right_body, row)
            {
                self.select_prompt_overlay_inactive_row(visible_offset);
            }
            return OverlayInputResult::Handled;
        }

        OverlayInputResult::Handled
    }

    fn cycle_prompt_overlay_inactive_tab(&mut self, delta: isize) {
        let Some(state) = self.prompt_overlay.as_mut() else {
            return;
        };
        state.inactive_tab = if delta.is_negative() {
            state.inactive_tab.previous()
        } else {
            state.inactive_tab.next()
        };
        self.sync_prompt_overlay_state();
    }

    fn set_prompt_overlay_inactive_tab(&mut self, tab: PromptOverlayInactiveTab) {
        let Some(state) = self.prompt_overlay.as_mut() else {
            return;
        };
        state.inactive_tab = tab;
        self.sync_prompt_overlay_state();
    }

    fn move_prompt_overlay_selection(&mut self, direction: ListNavigationDirection) {
        let focus = match self.prompt_overlay.as_ref() {
            Some(state) => state.focus,
            None => return,
        };

        match focus {
            PromptOverlayFocus::Active => self.move_prompt_overlay_active_selection(direction),
            PromptOverlayFocus::Inactive => self.move_prompt_overlay_inactive_selection(direction),
        }
    }

    fn move_prompt_overlay_page(&mut self, direction: ListNavigationDirection) {
        let focus = match self.prompt_overlay.as_ref() {
            Some(state) => state.focus,
            None => return,
        };

        let page_size = match focus {
            PromptOverlayFocus::Active => prompt_overlay_active_visible_rows(self.height),
            PromptOverlayFocus::Inactive => prompt_overlay_inactive_visible_rows(self.height),
        };
        for _ in 0..page_size.max(1) {
            self.move_prompt_overlay_selection(direction);
        }
    }

    fn jump_prompt_overlay_selection_to_edge(&mut self, first: bool) {
        let (focus, inactive_tab) = match self.prompt_overlay.as_ref() {
            Some(state) => (state.focus, state.inactive_tab),
            None => return,
        };
        let active_rows = self.prompt_overlay_left_rows();
        let active_count = active_rows.len();
        let active_row_id = if matches!(focus, PromptOverlayFocus::Active) {
            active_rows
                .get(if first {
                    0
                } else {
                    active_count.saturating_sub(1)
                })
                .map(prompt_overlay_left_row_id)
        } else {
            None
        };
        let inactive_reference_id = if matches!(focus, PromptOverlayFocus::Inactive) {
            let rows = self.prompt_overlay_inactive_rows(inactive_tab);
            let source_count = rows.len();
            rows.get(if first {
                0
            } else {
                source_count.saturating_sub(1)
            })
            .map(prompt_overlay_inactive_row_id)
        } else {
            None
        };
        let inactive_count = if matches!(focus, PromptOverlayFocus::Inactive) {
            self.prompt_overlay_inactive_source_count(inactive_tab)
        } else {
            0
        };

        let Some(state) = self.prompt_overlay.as_mut() else {
            return;
        };

        match focus {
            PromptOverlayFocus::Active => {
                let last_index = active_count.saturating_sub(1);
                state.active_selected = if first { 0 } else { last_index };
                state.active_selected_row_id = active_row_id;
            }
            PromptOverlayFocus::Inactive => {
                let last_index = inactive_count.saturating_sub(1);
                state.inactive_selected = if first { 0 } else { last_index };
                state.inactive_selected_row_id = inactive_reference_id;
            }
        }
        self.sync_prompt_overlay_state();
    }

    fn move_prompt_overlay_active_selection(&mut self, direction: ListNavigationDirection) {
        let rows = self.prompt_overlay_left_rows();
        let count = rows.len();
        let Some(state) = self.prompt_overlay.as_mut() else {
            return;
        };
        if count == 0 {
            state.active_selected = 0;
            state.active_scroll = 0;
            state.active_selected_row_id = None;
            return;
        }

        let next = match direction {
            ListNavigationDirection::Previous => state.active_selected.saturating_sub(1),
            ListNavigationDirection::Next => state
                .active_selected
                .saturating_add(1)
                .min(count.saturating_sub(1)),
        };
        state.active_selected = next;
        state.active_selected_row_id = rows.get(next).map(prompt_overlay_left_row_id);
        self.sync_prompt_overlay_state();
    }

    fn select_prompt_overlay_active_row(&mut self, visible_offset: usize) {
        let rows = self.prompt_overlay_left_rows();
        let total = rows.len();
        let current_scroll = self
            .prompt_overlay
            .as_ref()
            .map(|state| state.active_scroll)
            .unwrap_or_default();
        let selected = current_scroll.saturating_add(visible_offset);
        let Some(state) = self.prompt_overlay.as_mut() else {
            return;
        };
        if total == 0 {
            state.active_selected = 0;
            state.active_scroll = 0;
            state.active_selected_row_id = None;
            return;
        }
        let next_selected = selected.min(total.saturating_sub(1));
        state.active_selected = next_selected;
        state.active_selected_row_id = rows.get(next_selected).map(prompt_overlay_left_row_id);
        self.sync_prompt_overlay_state();
    }

    fn move_prompt_overlay_inactive_selection(&mut self, direction: ListNavigationDirection) {
        let inactive_tab = match self.prompt_overlay.as_ref() {
            Some(state) => state.inactive_tab,
            None => return,
        };
        let source_count = self.prompt_overlay_inactive_source_count(inactive_tab);

        if source_count == 0 {
            let Some(state) = self.prompt_overlay.as_mut() else {
                return;
            };
            state.inactive_selected = 0;
            state.inactive_scroll = 0;
            state.inactive_selected_row_id = None;
            return;
        }

        let current_selected = match self.prompt_overlay.as_ref() {
            Some(state) => state.inactive_selected,
            None => return,
        };
        let next = match direction {
            ListNavigationDirection::Previous => current_selected.saturating_sub(1),
            ListNavigationDirection::Next => current_selected
                .saturating_add(1)
                .min(source_count.saturating_sub(1)),
        };
        let next_reference_id = self
            .prompt_overlay_inactive_rows(inactive_tab)
            .get(next)
            .map(prompt_overlay_inactive_row_id);

        let Some(state) = self.prompt_overlay.as_mut() else {
            return;
        };
        state.inactive_selected = next;
        state.inactive_selected_row_id = next_reference_id;
        self.sync_prompt_overlay_state();
    }

    fn select_prompt_overlay_inactive_row(&mut self, visible_offset: usize) {
        let (inactive_tab, current_scroll, rows) = match self.prompt_overlay.as_ref() {
            Some(state) => (
                state.inactive_tab,
                state.inactive_scroll,
                self.prompt_overlay_inactive_rows(state.inactive_tab),
            ),
            None => return,
        };
        if rows.is_empty() {
            let Some(state) = self.prompt_overlay.as_mut() else {
                return;
            };
            state.inactive_selected = 0;
            state.inactive_scroll = 0;
            state.inactive_selected_row_id = None;
            return;
        }

        let selected = current_scroll
            .saturating_add(visible_offset)
            .min(rows.len().saturating_sub(1));
        let row_id = rows.get(selected).map(prompt_overlay_inactive_row_id);
        let Some(state) = self.prompt_overlay.as_mut() else {
            return;
        };
        if state.inactive_tab != inactive_tab {
            return;
        }
        state.inactive_selected = selected;
        state.inactive_selected_row_id = row_id;
        self.sync_prompt_overlay_state();
    }

    pub(crate) fn sync_prompt_overlay_state(&mut self) {
        let inactive_tab = match self.prompt_overlay.as_ref() {
            Some(state) => state.inactive_tab,
            None => return,
        };
        let inactive_source_count = self.prompt_overlay_inactive_source_count(inactive_tab);

        let active_count = self.prompt_overlay_left_rows().len();
        let (
            current_active_selected,
            current_active_scroll,
            current_active_selected_row_id,
            current_inactive_selected,
            current_inactive_scroll,
            current_inactive_reference_id,
        ) = match self.prompt_overlay.as_ref() {
            Some(state) => (
                state.active_selected,
                state.active_scroll,
                state.active_selected_row_id.clone(),
                state.inactive_selected,
                state.inactive_scroll,
                state.inactive_selected_row_id.clone(),
            ),
            None => return,
        };

        let active_rows = self.prompt_overlay_left_rows();
        let mut next_active_selected = current_active_selected;
        if let Some(row_id) = current_active_selected_row_id.as_deref()
            && let Some(index) = active_rows
                .iter()
                .position(|row| prompt_overlay_left_row_id(row) == row_id)
        {
            next_active_selected = index;
        }
        next_active_selected = next_active_selected.min(active_count.saturating_sub(1));
        let next_active_selected_row_id = active_rows
            .get(next_active_selected)
            .map(prompt_overlay_left_row_id);
        let next_active_scroll = clamp_scroll(
            current_active_scroll,
            next_active_selected,
            active_count,
            prompt_overlay_active_visible_rows(self.height),
        );

        let mut next_inactive_selected = current_inactive_selected;
        let inactive_rows = self.prompt_overlay_inactive_rows(inactive_tab);
        if let Some(reference_id) = current_inactive_reference_id.as_deref() {
            let matched_index = inactive_rows
                .iter()
                .position(|row| prompt_overlay_inactive_row_id(row) == reference_id);
            if let Some(index) = matched_index {
                next_inactive_selected = index;
            }
        }
        next_inactive_selected =
            next_inactive_selected.min(inactive_source_count.saturating_sub(1));
        let next_inactive_reference_id = inactive_rows
            .get(next_inactive_selected)
            .map(prompt_overlay_inactive_row_id);

        let selected_row = next_inactive_selected;
        let next_inactive_scroll = clamp_scroll(
            current_inactive_scroll,
            selected_row,
            inactive_source_count,
            prompt_overlay_inactive_visible_rows(self.height),
        );

        let Some(state) = self.prompt_overlay.as_mut() else {
            return;
        };
        state.active_selected = next_active_selected;
        state.active_scroll = next_active_scroll;
        state.active_selected_row_id = next_active_selected_row_id;
        state.inactive_selected = next_inactive_selected;
        state.inactive_selected_row_id = next_inactive_reference_id;
        state.inactive_scroll = next_inactive_scroll;
    }

    fn prompt_overlay_header_line(
        &self,
        width: usize,
        active_tab: PromptOverlayInactiveTab,
    ) -> Line<'static> {
        let title = "Prompt Assembly";
        let tabs = self.prompt_overlay_tabs_plain(active_tab);
        let available_width = width.saturating_sub(PROMPT_OVERLAY_HEADER_INSET);
        let tabs_width = display_width(&tabs) + PROMPT_OVERLAY_HEADER_TRAILING_PADDING;
        let title_width = available_width
            .saturating_sub(tabs_width)
            .saturating_sub(1)
            .max(1);
        let title = truncate_display_width_with_ellipsis(title, title_width);
        let padding = available_width
            .saturating_sub(display_width(&title))
            .saturating_sub(tabs_width)
            .max(1);

        let mut spans = vec![
            Span::raw(" ".repeat(PROMPT_OVERLAY_HEADER_INSET)),
            Span::styled(title, primary_text_style(self.palette).bold()),
            Span::raw(" ".repeat(padding)),
        ];
        spans.extend(self.prompt_overlay_tabs_spans(active_tab));
        spans.push(Span::raw(
            " ".repeat(PROMPT_OVERLAY_HEADER_TRAILING_PADDING),
        ));

        Line::from(spans)
    }

    fn prompt_overlay_footer_hint(&self, width: u16) -> String {
        let actions = self.prompt_overlay_action_availability();
        let mut parts = if width < 120 {
            vec!["Esc close", "Space preview"]
        } else {
            vec![
                "Esc close",
                "←/→/h/l focus panes",
                "↑/↓/j/k move",
                "Space source",
                "p assembled",
            ]
        };
        let show_shadowed_toggle = matches!(
            self.selected_prompt_overlay_left_row(),
            Some(PromptOverlayLeftRow::ManagedSource { shadowed_count, .. }) if shadowed_count > 0
        ) || matches!(
            self.selected_prompt_overlay_left_row(),
            Some(PromptOverlayLeftRow::ShadowedDetail { .. })
        ) || matches!(
            self.selected_prompt_overlay_inactive_row(),
            Some(PromptOverlayInactiveRow::ExtraPromptCandidate { shadowed_count, .. })
                if shadowed_count > 0
        ) || matches!(
            self.selected_prompt_overlay_inactive_row(),
            Some(PromptOverlayInactiveRow::DiscoveredSkill { shadowed_count, .. })
                if shadowed_count > 0
        ) || matches!(
            self.selected_prompt_overlay_inactive_row(),
            Some(
                PromptOverlayInactiveRow::ExtraPromptShadowedDetail { .. }
                    | PromptOverlayInactiveRow::DiscoveredSkillShadowedDetail { .. }
            )
        );
        let selected_core = self
            .selected_prompt_overlay_source()
            .is_some_and(|source| source.kind == PromptSourceKind::CoreSystemPrompt);
        if actions.can_edit {
            parts.push("e/ctrl+g edit");
        }
        if actions.can_add_custom {
            parts.push("a create prompt");
        }
        if actions.can_remove {
            parts.push("d remove");
        }
        if actions.can_toggle_selection {
            parts.push("x disable");
        }
        if selected_core {
            parts.push("r restore");
        }
        if actions.can_reorder_active && width >= 120 {
            parts.push("J/K reorder");
        }
        if show_shadowed_toggle {
            parts.push("ctrl+e shadowed");
        }
        let mut text = format!("  {}", parts.join(" · "));
        let focus_right = self
            .prompt_overlay
            .as_ref()
            .is_some_and(|state| state.focus == PromptOverlayFocus::Inactive);
        if focus_right {
            text.push_str(PROMPT_OVERLAY_FOOTER_TABS_SUFFIX);
        }
        text
    }

    fn render_prompt_overlay_active_pane(
        &self,
        frame: &mut RenderFrame<'_>,
        area: Rect,
        state: &PromptOverlayState,
    ) {
        if area.is_empty() {
            return;
        }
        let [header_area, body_area] =
            Layout::vertical([Constraint::Length(1), Constraint::Fill(1)]).areas(area);
        frame.render_widget(
            Paragraph::new(self.prompt_overlay_active_header_line(usize::from(header_area.width))),
            header_area,
        );

        let sources = self.prompt_overlay_left_rows();
        let lines = prompt_overlay_active_lines(
            &sources,
            state.active_selected,
            state.active_scroll,
            state.focus == PromptOverlayFocus::Active,
            usize::from(body_area.width),
            usize::from(body_area.height),
            self.palette,
        );
        frame.render_widget(PromptOverlayLineListWidget { lines: &lines }, body_area);
    }

    fn render_prompt_overlay_inactive_pane(
        &self,
        frame: &mut RenderFrame<'_>,
        area: Rect,
        state: &PromptOverlayState,
    ) {
        if area.is_empty() {
            return;
        }
        let [header_area, body_area] =
            Layout::vertical([Constraint::Length(1), Constraint::Fill(1)]).areas(area);
        frame.render_widget(
            Paragraph::new(self.prompt_overlay_inactive_header_line(
                state.inactive_tab,
                usize::from(header_area.width),
            )),
            header_area,
        );

        let lines = if state.inactive_tab == PromptOverlayInactiveTab::LongLivedSkills {
            prompt_overlay_discovered_skill_lines(
                &self.prompt_overlay_inactive_rows(PromptOverlayInactiveTab::LongLivedSkills),
                state.inactive_selected_row_id.as_deref(),
                state.inactive_scroll,
                state.focus == PromptOverlayFocus::Inactive,
                usize::from(body_area.width),
                usize::from(body_area.height),
                self.palette,
            )
        } else {
            prompt_overlay_inactive_lines(
                &self.prompt_overlay_inactive_rows(PromptOverlayInactiveTab::ExtraPrompts),
                state.inactive_selected_row_id.as_deref(),
                state.inactive_scroll,
                state.focus == PromptOverlayFocus::Inactive,
                usize::from(body_area.width),
                usize::from(body_area.height),
                self.palette,
            )
        };
        frame.render_widget(PromptOverlayLineListWidget { lines: &lines }, body_area);
    }

    fn prompt_overlay_active_header_line(&self, width: usize) -> Line<'static> {
        let content_width = width.saturating_sub(PROMPT_OVERLAY_ROW_PREFIX_WIDTH);
        let text = format!(
            "{}{}",
            " ".repeat(PROMPT_OVERLAY_ROW_PREFIX_WIDTH),
            truncate_display_width_with_ellipsis(
                &prompt_overlay_active_header_text(content_width),
                content_width.max(1),
            )
        );
        Line::styled(
            truncate_display_width_with_ellipsis(&text, width.max(1)),
            table_header_text_style(self.palette),
        )
    }

    fn prompt_overlay_inactive_header_line(
        &self,
        active_tab: PromptOverlayInactiveTab,
        width: usize,
    ) -> Line<'static> {
        let content_width = width.saturating_sub(PROMPT_OVERLAY_ROW_PREFIX_WIDTH);
        let label = match active_tab {
            PromptOverlayInactiveTab::ExtraPrompts => {
                prompt_overlay_extra_header_text(content_width)
            }
            PromptOverlayInactiveTab::LongLivedSkills => {
                prompt_overlay_skill_header_text(content_width)
            }
        };
        let text = format!(
            "{}{}",
            " ".repeat(PROMPT_OVERLAY_ROW_PREFIX_WIDTH),
            truncate_display_width_with_ellipsis(&label, content_width.max(1))
        );
        Line::styled(
            truncate_display_width_with_ellipsis(&text, width.max(1)),
            table_header_text_style(self.palette),
        )
    }

    fn prompt_overlay_tabs_plain(&self, active_tab: PromptOverlayInactiveTab) -> String {
        PromptOverlayInactiveTab::ALL
            .iter()
            .copied()
            .map(|tab| {
                if tab == active_tab {
                    format!("[{}]", tab.label())
                } else {
                    tab.label().to_string()
                }
            })
            .collect::<Vec<_>>()
            .join(" ")
    }

    fn prompt_overlay_tabs_spans(
        &self,
        active_tab: PromptOverlayInactiveTab,
    ) -> Vec<Span<'static>> {
        let mut spans = Vec::new();

        for (index, tab) in PromptOverlayInactiveTab::ALL.iter().copied().enumerate() {
            if index > 0 {
                spans.push(Span::raw(" "));
            }
            let is_active = tab == active_tab;
            let label = if is_active {
                format!("[{}]", tab.label())
            } else {
                tab.label().to_string()
            };
            let style = if is_active {
                surface_text_style(self.palette).bold()
            } else {
                tertiary_text_style(self.palette)
            };
            spans.push(Span::styled(label, style));
        }

        spans
    }

    fn prompt_overlay_focused_page_label(
        &self,
        state: &PromptOverlayState,
        _height: u16,
    ) -> String {
        match state.focus {
            PromptOverlayFocus::Active => selection_label(
                Some("Active"),
                state.active_selected,
                self.prompt_overlay_left_rows().len(),
            ),
            PromptOverlayFocus::Inactive => selection_label(
                None,
                state.inactive_selected,
                self.prompt_overlay_inactive_source_count(state.inactive_tab),
            ),
        }
    }

    pub(crate) fn prompt_overlay_inactive_source_count(
        &self,
        tab: PromptOverlayInactiveTab,
    ) -> usize {
        self.prompt_overlay_inactive_rows(tab).len()
    }

    fn render_prompt_overlay_dialog(
        &self,
        frame: &mut RenderFrame<'_>,
        anchor_area: Rect,
        dialog: &PromptOverlayDialog,
    ) {
        let dialog_area = prompt_overlay_dialog_area(anchor_area);
        frame.render_widget(Clear, dialog_area);

        let lines = match dialog {
            PromptOverlayDialog::CreateExtraPromptScope { selected_scope } => vec![
                Line::styled(
                    "Create custom prompt in",
                    primary_text_style(self.palette).bold(),
                ),
                Line::raw(""),
                prompt_overlay_scope_picker_line(*selected_scope, self.palette),
                Line::raw(""),
                Line::styled(
                    "←/→/h/l select · Enter confirm · Esc cancel",
                    tertiary_text_style(self.palette),
                ),
            ],
        };

        let block = panel_block(self.palette);
        let inner_area = block.inner(dialog_area);
        frame.render_widget(block, dialog_area);
        frame.render_widget(Paragraph::new(lines), inner_area);
    }

    fn handle_prompt_overlay_dialog_mouse_down(
        &mut self,
        column: u16,
        row: u16,
    ) -> OverlayInputResult {
        let Some(state) = self.prompt_overlay.as_mut() else {
            return OverlayInputResult::Handled;
        };
        let Some(layout) = prompt_overlay_layout_rects(Rect::new(0, 0, self.width, self.height))
        else {
            return OverlayInputResult::Handled;
        };
        let dialog_area = prompt_overlay_dialog_area(layout.right_pane);
        if !prompt_overlay_rect_contains(dialog_area, column, row) {
            return OverlayInputResult::Handled;
        }

        match state.dialog.as_mut() {
            Some(PromptOverlayDialog::CreateExtraPromptScope { selected_scope }) => {
                let inner_area = panel_block(self.palette).inner(dialog_area);
                let scope_row = inner_area.y.saturating_add(2);
                if row != scope_row {
                    return OverlayInputResult::Handled;
                }

                let project_label = if *selected_scope == PromptAssemblyScope::Project {
                    "[Project]"
                } else {
                    "Project"
                };
                let global_label = if *selected_scope == PromptAssemblyScope::Global {
                    "[Global]"
                } else {
                    "Global"
                };
                let project_end = inner_area.x.saturating_add(
                    u16::try_from(display_width(project_label)).unwrap_or(u16::MAX),
                );
                let global_start = project_end.saturating_add(1);
                let global_end = global_start
                    .saturating_add(u16::try_from(display_width(global_label)).unwrap_or(u16::MAX));

                if column >= inner_area.x && column < project_end {
                    *selected_scope = PromptAssemblyScope::Project;
                } else if column >= global_start && column < global_end {
                    *selected_scope = PromptAssemblyScope::Global;
                }
                OverlayInputResult::Handled
            }
            None => OverlayInputResult::Handled,
        }
    }

    fn prompt_overlay_header_tab_at(
        &self,
        column: u16,
        row: u16,
        header_area: Rect,
        active_tab: PromptOverlayInactiveTab,
    ) -> Option<PromptOverlayInactiveTab> {
        if row != header_area.y
            || column < header_area.x
            || column >= header_area.x.saturating_add(header_area.width)
        {
            return None;
        }

        let width = usize::from(header_area.width);
        let title = "Prompt Assembly";
        let tabs = self.prompt_overlay_tabs_plain(active_tab);
        let available_width = width.saturating_sub(PROMPT_OVERLAY_HEADER_INSET);
        let tabs_width = display_width(&tabs) + PROMPT_OVERLAY_HEADER_TRAILING_PADDING;
        let title_width = available_width
            .saturating_sub(tabs_width)
            .saturating_sub(1)
            .max(1);
        let title = truncate_display_width_with_ellipsis(title, title_width);
        let padding = available_width
            .saturating_sub(display_width(&title))
            .saturating_sub(tabs_width)
            .max(1);
        let mut current_column = usize::from(header_area.x)
            .saturating_add(PROMPT_OVERLAY_HEADER_INSET)
            .saturating_add(display_width(&title))
            .saturating_add(padding);
        let clicked_column = usize::from(column);

        for (index, tab) in PromptOverlayInactiveTab::ALL.iter().copied().enumerate() {
            if index > 0 {
                current_column = current_column.saturating_add(1);
            }
            let label = if tab == active_tab {
                format!("[{}]", tab.label())
            } else {
                tab.label().to_string()
            };
            let label_end = current_column.saturating_add(display_width(&label));
            if clicked_column >= current_column && clicked_column < label_end {
                return Some(tab);
            }
            current_column = label_end;
        }

        None
    }
}

fn prompt_overlay_matches_resolved_source(
    resolved: &ResolvedPromptSource,
    kind: PromptSourceKind,
    reference_id: &str,
    origin: Option<PromptSourceOrigin>,
) -> bool {
    resolved.kind == kind && resolved.reference_id == reference_id && resolved.origin == origin
}

fn prompt_overlay_scope_picker_line(
    scope: PromptAssemblyScope,
    palette: crate::theme::TerminalPalette,
) -> Line<'static> {
    let (project_style, global_style) = match scope {
        PromptAssemblyScope::Project => (
            surface_text_style(palette).bold(),
            secondary_text_style(palette),
        ),
        PromptAssemblyScope::Global => (
            secondary_text_style(palette),
            surface_text_style(palette).bold(),
        ),
    };

    match scope {
        PromptAssemblyScope::Project => Line::from(vec![
            Span::styled("[Project]", project_style),
            Span::raw(" "),
            Span::styled("Global", global_style),
        ]),
        PromptAssemblyScope::Global => Line::from(vec![
            Span::styled("Project", project_style),
            Span::raw(" "),
            Span::styled("[Global]", global_style),
        ]),
    }
}

fn prompt_overlay_inactive_row_id(row: &PromptOverlayInactiveRow) -> String {
    match row {
        PromptOverlayInactiveRow::ExtraPromptCandidate { source, .. }
        | PromptOverlayInactiveRow::ExtraPromptShadowedDetail { source } => {
            format!(
                "extra:{}:{}",
                source.reference_id,
                prompt_overlay_origin_label(source.origin)
            )
        }
        PromptOverlayInactiveRow::DiscoveredSkill { skill, .. }
        | PromptOverlayInactiveRow::DiscoveredSkillShadowedDetail { skill } => {
            format!(
                "skill:{}:{}",
                skill.skill_name,
                prompt_overlay_origin_label(skill.origin)
            )
        }
    }
}

fn prompt_overlay_left_row_id(row: &PromptOverlayLeftRow) -> String {
    match row {
        PromptOverlayLeftRow::ManagedSource { source, .. } => format!(
            "managed:{}:{}:{}",
            prompt_overlay_kind_label(source.kind),
            source.reference_id,
            source.origin.map_or("none", prompt_overlay_origin_label),
        ),
        PromptOverlayLeftRow::ShadowedDetail { source } => format!(
            "shadowed:{}:{}:{}",
            prompt_overlay_kind_label(source.kind),
            source.reference_id,
            source.origin.map_or("none", prompt_overlay_origin_label),
        ),
    }
}

fn prompt_overlay_partition_extra_candidates(
    mut candidates: Vec<PromptAssemblyExtraPromptCandidate>,
) -> (
    PromptAssemblyExtraPromptCandidate,
    Vec<PromptAssemblyExtraPromptCandidate>,
) {
    candidates.sort_by_key(|candidate| prompt_overlay_origin_sort_key(candidate.origin));
    let winner = candidates.remove(0);
    (winner, candidates)
}

fn prompt_overlay_extra_candidate_winner(
    candidates: &[PromptAssemblyExtraPromptCandidate],
) -> &PromptAssemblyExtraPromptCandidate {
    candidates
        .iter()
        .min_by_key(|candidate| prompt_overlay_origin_sort_key(candidate.origin))
        .expect("extra prompt group should not be empty")
}

fn prompt_overlay_partition_discovered_skills(
    mut skills: Vec<PromptAssemblyDiscoveredSkill>,
) -> (
    PromptAssemblyDiscoveredSkill,
    Vec<PromptAssemblyDiscoveredSkill>,
) {
    skills.sort_by_key(|skill| prompt_overlay_origin_sort_key(skill.origin));
    let winner = skills.remove(0);
    (winner, skills)
}

fn prompt_overlay_discovered_skill_winner(
    skills: &[PromptAssemblyDiscoveredSkill],
) -> &PromptAssemblyDiscoveredSkill {
    skills
        .iter()
        .min_by_key(|skill| prompt_overlay_origin_sort_key(skill.origin))
        .expect("discovered skill group should not be empty")
}

fn prompt_overlay_origin_sort_key(origin: PromptSourceOrigin) -> u8 {
    match origin {
        PromptSourceOrigin::Project => 0,
        PromptSourceOrigin::Global => 1,
        PromptSourceOrigin::Builtin => 2,
    }
}

fn prompt_overlay_layout_rects(area: Rect) -> Option<PromptOverlayLayoutRects> {
    let chrome = fullscreen_list_chrome_rects(area)?;
    let [left_pane, _gutter, right_pane] = Layout::horizontal([
        Constraint::Percentage(50),
        Constraint::Length(1),
        Constraint::Percentage(50),
    ])
    .areas(chrome.body);
    let [_left_header, left_body] =
        Layout::vertical([Constraint::Length(1), Constraint::Fill(1)]).areas(left_pane);
    let [_right_header, right_body] =
        Layout::vertical([Constraint::Length(1), Constraint::Fill(1)]).areas(right_pane);
    Some(PromptOverlayLayoutRects {
        chrome,
        left_pane,
        left_body,
        right_pane,
        right_body,
    })
}

fn prompt_overlay_dialog_area(anchor_area: Rect) -> Rect {
    let dialog_width = anchor_area.width.min(52);
    let dialog_height = 7u16.min(anchor_area.height);
    let dialog_x = anchor_area
        .x
        .saturating_add(anchor_area.width.saturating_sub(dialog_width) / 2);
    let dialog_y = anchor_area
        .y
        .saturating_add(anchor_area.height.saturating_sub(dialog_height) / 2);
    Rect::new(dialog_x, dialog_y, dialog_width, dialog_height)
}

fn prompt_overlay_rect_contains(area: Rect, column: u16, row: u16) -> bool {
    column >= area.x
        && column < area.x.saturating_add(area.width)
        && row >= area.y
        && row < area.y.saturating_add(area.height)
}

fn prompt_overlay_visible_offset_for_row(body_area: Rect, row: u16) -> Option<usize> {
    (row >= body_area.y && row < body_area.y.saturating_add(body_area.height))
        .then(|| usize::from(row.saturating_sub(body_area.y)))
}

fn prompt_overlay_selection_styles(
    selected: bool,
    focused: bool,
    palette: crate::theme::TerminalPalette,
) -> (Style, Style, &'static str) {
    let visually_selected = selected && focused;
    let item_style = if visually_selected {
        primary_text_style(palette).bold()
    } else {
        secondary_text_style(palette)
    };
    let marker_style = if visually_selected {
        command_accent_text_style(palette)
    } else {
        tertiary_text_style(palette)
    };
    let marker = if visually_selected { "█" } else { " " };
    (item_style, marker_style, marker)
}

struct PromptOverlayLineListWidget<'a> {
    lines: &'a [Line<'static>],
}

impl Widget for PromptOverlayLineListWidget<'_> {
    fn render(self, area: Rect, buf: &mut ratatui::buffer::Buffer) {
        for (row, line) in self.lines.iter().take(usize::from(area.height)).enumerate() {
            let y = area.y + u16::try_from(row).unwrap_or(u16::MAX);
            render_line_with_full_width_background(line, Rect::new(area.x, y, area.width, 1), buf);
        }
    }
}

fn prompt_overlay_active_lines(
    sources: &[PromptOverlayLeftRow],
    selected: usize,
    scroll: usize,
    focused: bool,
    width: usize,
    body_height: usize,
    palette: crate::theme::TerminalPalette,
) -> Vec<Line<'static>> {
    if body_height == 0 {
        return Vec::new();
    }
    if sources.is_empty() {
        return vec![Line::styled(
            truncate_display_width_with_ellipsis("  No active sources", width.max(1)),
            tertiary_text_style(palette),
        )];
    }

    let mut lines = Vec::new();
    for (index, source) in sources.iter().enumerate().skip(scroll).take(body_height) {
        lines.push(prompt_overlay_left_row_line(
            source,
            index == selected,
            focused,
            width,
            palette,
        ));
    }
    lines
}

fn prompt_overlay_inactive_lines(
    rows: &[PromptOverlayInactiveRow],
    selected_row_id: Option<&str>,
    scroll: usize,
    focused: bool,
    width: usize,
    body_height: usize,
    palette: crate::theme::TerminalPalette,
) -> Vec<Line<'static>> {
    if body_height == 0 {
        return Vec::new();
    }
    if rows.is_empty() {
        return vec![prompt_overlay_empty_inactive_line(
            "No candidates",
            width,
            palette,
        )];
    }

    let mut lines = Vec::new();
    for row in rows.iter().skip(scroll).take(body_height) {
        lines.push(prompt_overlay_inactive_row_line(
            row,
            selected_row_id == Some(prompt_overlay_inactive_row_id(row).as_str()),
            focused,
            width,
            palette,
        ));
    }
    lines
}

fn prompt_overlay_left_row_line(
    row: &PromptOverlayLeftRow,
    selected: bool,
    focused: bool,
    width: usize,
    palette: crate::theme::TerminalPalette,
) -> Line<'static> {
    let content_width = width.saturating_sub(PROMPT_OVERLAY_ROW_PREFIX_WIDTH).max(1);
    let label = match row {
        PromptOverlayLeftRow::ManagedSource {
            source,
            status,
            shadowed_count,
        } => prompt_overlay_active_row_text(source, *status, *shadowed_count, content_width),
        PromptOverlayLeftRow::ShadowedDetail { source } => {
            prompt_overlay_shadowed_detail_row_text(source, content_width)
        }
    };
    let (item_style, marker_style, marker) =
        prompt_overlay_selection_styles(selected, focused, palette);
    prompt_overlay_list_line(
        marker,
        marker_style,
        truncate_display_width_with_ellipsis(&label, content_width),
        item_style,
    )
}

fn prompt_overlay_discovered_skill_lines(
    skills: &[PromptOverlayInactiveRow],
    selected_row_id: Option<&str>,
    scroll: usize,
    focused: bool,
    width: usize,
    body_height: usize,
    palette: crate::theme::TerminalPalette,
) -> Vec<Line<'static>> {
    if body_height == 0 {
        return Vec::new();
    }
    if skills.is_empty() {
        return vec![prompt_overlay_empty_inactive_line(
            "No discovered skills",
            width,
            palette,
        )];
    }

    skills
        .iter()
        .map(|row| {
            let selected = selected_row_id == Some(prompt_overlay_inactive_row_id(row).as_str());
            let content_width = width.saturating_sub(PROMPT_OVERLAY_ROW_PREFIX_WIDTH).max(1);
            let (item_style, marker_style, marker) =
                prompt_overlay_selection_styles(selected, focused, palette);
            let label = prompt_overlay_skill_row_text(row, content_width);
            prompt_overlay_list_line(
                marker,
                marker_style,
                truncate_display_width_with_ellipsis(&label, content_width),
                item_style,
            )
        })
        .skip(scroll)
        .take(body_height)
        .collect()
}

fn prompt_overlay_inactive_row_line(
    row: &PromptOverlayInactiveRow,
    selected: bool,
    focused: bool,
    width: usize,
    palette: crate::theme::TerminalPalette,
) -> Line<'static> {
    let content_width = width.saturating_sub(PROMPT_OVERLAY_ROW_PREFIX_WIDTH).max(1);
    let (item_style, marker_style, marker) =
        prompt_overlay_selection_styles(selected, focused, palette);
    let label = match row {
        PromptOverlayInactiveRow::ExtraPromptCandidate {
            source,
            shadowed_count,
        } => prompt_overlay_extra_row_text(source, *shadowed_count, content_width),
        PromptOverlayInactiveRow::ExtraPromptShadowedDetail { source } => {
            prompt_overlay_extra_shadowed_detail_row_text(source, content_width)
        }
        PromptOverlayInactiveRow::DiscoveredSkill {
            shadowed_count: _, ..
        } => prompt_overlay_skill_row_text(row, content_width),
        PromptOverlayInactiveRow::DiscoveredSkillShadowedDetail { .. } => {
            prompt_overlay_skill_row_text(row, content_width)
        }
    };
    prompt_overlay_list_line(
        marker,
        marker_style,
        truncate_display_width_with_ellipsis(&label, content_width),
        item_style,
    )
}

fn prompt_overlay_origin_label(origin: PromptSourceOrigin) -> &'static str {
    match origin {
        PromptSourceOrigin::Builtin => "builtin",
        PromptSourceOrigin::Global => "global",
        PromptSourceOrigin::Project => "project",
    }
}

fn prompt_overlay_kind_label(kind: PromptSourceKind) -> &'static str {
    match kind {
        PromptSourceKind::CoreSystemPrompt => "system",
        PromptSourceKind::ExtraPrompt => "custom",
        PromptSourceKind::SkillDiscovery => "discovery",
        PromptSourceKind::LongLivedSkill => "skill",
    }
}

fn prompt_overlay_active_header_text(width: usize) -> String {
    let source_width = prompt_overlay_left_source_width(width);
    let left_pad = " ".repeat(PROMPT_OVERLAY_OUTER_PADDING);
    let sel = prompt_overlay_center_text("Sel", PROMPT_OVERLAY_LEFT_SEL_WIDTH);
    let ord = left_pad_display_width("Ord", PROMPT_OVERLAY_LEFT_ORD_WIDTH);
    let source = format!(
        "{:<width$}",
        truncate_display_width_with_ellipsis("Source", source_width),
        width = source_width
    );
    let kind = format!("{:<width$}", "Type", width = PROMPT_OVERLAY_LEFT_KIND_WIDTH);
    let scope = format!(
        "{:<width$}",
        "Scope",
        width = PROMPT_OVERLAY_LEFT_SCOPE_WIDTH
    );
    let gap = " ".repeat(PROMPT_OVERLAY_COLUMN_GAP);
    let trailing = " ".repeat(PROMPT_OVERLAY_SCOPE_TRAILING_PADDING);
    format!("{left_pad}{sel}{gap}{ord}{gap}{source}{gap}{kind}{gap}{scope}{trailing}")
}

fn prompt_overlay_extra_header_text(width: usize) -> String {
    let name_width = prompt_overlay_right_extra_name_width(width);
    let left_pad = " ".repeat(PROMPT_OVERLAY_OUTER_PADDING);
    let sel = prompt_overlay_center_text("Sel", PROMPT_OVERLAY_LEFT_SEL_WIDTH);
    let name = format!(
        "{:<width$}",
        truncate_display_width_with_ellipsis("Name", name_width),
        width = name_width
    );
    let scope = format!(
        "{:<width$}",
        "Scope",
        width = PROMPT_OVERLAY_RIGHT_SCOPE_WIDTH
    );
    let gap = " ".repeat(PROMPT_OVERLAY_COLUMN_GAP);
    let trailing = " ".repeat(PROMPT_OVERLAY_SCOPE_TRAILING_PADDING);
    format!("{left_pad}{sel}{gap}{name}{gap}{scope}{trailing}")
}

fn prompt_overlay_skill_header_text(width: usize) -> String {
    let name_width = prompt_overlay_right_skill_name_width(width);
    let left_pad = " ".repeat(PROMPT_OVERLAY_OUTER_PADDING);
    let sel = prompt_overlay_center_text("Sel", PROMPT_OVERLAY_LEFT_SEL_WIDTH);
    let name = format!(
        "{:<width$}",
        truncate_display_width_with_ellipsis("Name", name_width),
        width = name_width
    );
    let scope = format!(
        "{:<width$}",
        "Scope",
        width = PROMPT_OVERLAY_RIGHT_SCOPE_WIDTH
    );
    let gap = " ".repeat(PROMPT_OVERLAY_COLUMN_GAP);
    let trailing = " ".repeat(PROMPT_OVERLAY_SCOPE_TRAILING_PADDING);
    format!("{left_pad}{sel}{gap}{name}{gap}{scope}{trailing}")
}

fn prompt_overlay_active_row_text(
    source: &PromptAssemblyManagedSource,
    status: PromptOverlayManagedStatus,
    shadowed_count: usize,
    width: usize,
) -> String {
    let source_width = prompt_overlay_left_source_width(width);
    let left_pad = " ".repeat(PROMPT_OVERLAY_OUTER_PADDING);
    let sel = prompt_overlay_center_text(
        if source.enabled { "●" } else { "○" },
        PROMPT_OVERLAY_LEFT_SEL_WIDTH,
    );
    let ord = left_pad_display_width(&source.order.to_string(), PROMPT_OVERLAY_LEFT_ORD_WIDTH);
    let status_marker = prompt_overlay_managed_status_marker(status, shadowed_count);
    let source_name = prompt_overlay_cell_with_trailing_marker(
        &source.title,
        status_marker.as_deref(),
        source_width,
    );
    let kind = format!(
        "{:<width$}",
        prompt_overlay_kind_label(source.kind),
        width = PROMPT_OVERLAY_LEFT_KIND_WIDTH
    );
    let scope = format!(
        "{:<width$}",
        source
            .origin
            .map(prompt_overlay_origin_label)
            .unwrap_or("-"),
        width = PROMPT_OVERLAY_LEFT_SCOPE_WIDTH
    );
    let gap = " ".repeat(PROMPT_OVERLAY_COLUMN_GAP);
    let trailing = " ".repeat(PROMPT_OVERLAY_SCOPE_TRAILING_PADDING);
    format!("{left_pad}{sel}{gap}{ord}{gap}{source_name}{gap}{kind}{gap}{scope}{trailing}")
}

fn prompt_overlay_shadowed_detail_row_text(source: &ResolvedPromptSource, width: usize) -> String {
    let source_width = prompt_overlay_left_source_width(width);
    let left_pad = " ".repeat(PROMPT_OVERLAY_OUTER_PADDING);
    let sel = prompt_overlay_center_text("↳", PROMPT_OVERLAY_LEFT_SEL_WIDTH);
    let ord = left_pad_display_width("", PROMPT_OVERLAY_LEFT_ORD_WIDTH);
    let source_name = prompt_overlay_cell_with_trailing_marker(
        &format!(
            "shadowed {}",
            source
                .origin
                .map(prompt_overlay_origin_label)
                .unwrap_or("-")
        ),
        None,
        source_width,
    );
    let kind = format!(
        "{:<width$}",
        prompt_overlay_kind_label(source.kind),
        width = PROMPT_OVERLAY_LEFT_KIND_WIDTH
    );
    let scope = format!(
        "{:<width$}",
        source
            .origin
            .map(prompt_overlay_origin_label)
            .unwrap_or("-"),
        width = PROMPT_OVERLAY_LEFT_SCOPE_WIDTH
    );
    let gap = " ".repeat(PROMPT_OVERLAY_COLUMN_GAP);
    let trailing = " ".repeat(PROMPT_OVERLAY_SCOPE_TRAILING_PADDING);
    format!("{left_pad}{sel}{gap}{ord}{gap}{source_name}{gap}{kind}{gap}{scope}{trailing}")
}

fn prompt_overlay_extra_row_text(
    source: &PromptAssemblyExtraPromptCandidate,
    shadowed_count: usize,
    width: usize,
) -> String {
    let name_width = prompt_overlay_right_extra_name_width(width);
    let left_pad = " ".repeat(PROMPT_OVERLAY_OUTER_PADDING);
    let sel = prompt_overlay_center_text(
        if source.selected { "●" } else { "○" },
        PROMPT_OVERLAY_LEFT_SEL_WIDTH,
    );
    let name = prompt_overlay_cell_with_trailing_marker(
        &source.title,
        prompt_overlay_shadowed_count_marker(shadowed_count).as_deref(),
        name_width,
    );
    let scope = format!(
        "{:<width$}",
        prompt_overlay_origin_label(source.origin),
        width = PROMPT_OVERLAY_RIGHT_SCOPE_WIDTH
    );
    let gap = " ".repeat(PROMPT_OVERLAY_COLUMN_GAP);
    let trailing = " ".repeat(PROMPT_OVERLAY_SCOPE_TRAILING_PADDING);
    format!("{left_pad}{sel}{gap}{name}{gap}{scope}{trailing}")
}

fn prompt_overlay_skill_row_text(row: &PromptOverlayInactiveRow, width: usize) -> String {
    let name_width = prompt_overlay_right_skill_name_width(width);
    let left_pad = " ".repeat(PROMPT_OVERLAY_OUTER_PADDING);
    let sel = prompt_overlay_center_text(
        &prompt_overlay_skill_sel_label(row),
        PROMPT_OVERLAY_LEFT_SEL_WIDTH,
    );
    let name = prompt_overlay_skill_name_cell(row, name_width);
    let scope = format!(
        "{:<width$}",
        prompt_overlay_origin_label(prompt_overlay_inactive_skill_origin(row)),
        width = PROMPT_OVERLAY_RIGHT_SCOPE_WIDTH
    );
    let gap = " ".repeat(PROMPT_OVERLAY_COLUMN_GAP);
    let trailing = " ".repeat(PROMPT_OVERLAY_SCOPE_TRAILING_PADDING);
    format!("{left_pad}{sel}{gap}{name}{gap}{scope}{trailing}")
}

fn prompt_overlay_extra_shadowed_detail_row_text(
    source: &PromptAssemblyExtraPromptCandidate,
    width: usize,
) -> String {
    let name_width = prompt_overlay_right_extra_name_width(width);
    let left_pad = " ".repeat(PROMPT_OVERLAY_OUTER_PADDING);
    let sel = prompt_overlay_center_text(
        if source.selected { "●" } else { "○" },
        PROMPT_OVERLAY_LEFT_SEL_WIDTH,
    );
    let name = prompt_overlay_cell_with_trailing_marker(
        &format!("shadowed {}", prompt_overlay_origin_label(source.origin)),
        None,
        name_width,
    );
    let scope = format!(
        "{:<width$}",
        prompt_overlay_origin_label(source.origin),
        width = PROMPT_OVERLAY_RIGHT_SCOPE_WIDTH
    );
    let gap = " ".repeat(PROMPT_OVERLAY_COLUMN_GAP);
    let trailing = " ".repeat(PROMPT_OVERLAY_SCOPE_TRAILING_PADDING);
    format!("{left_pad}{sel}{gap}{name}{gap}{scope}{trailing}")
}

fn prompt_overlay_skill_sel_label(row: &PromptOverlayInactiveRow) -> String {
    match row {
        PromptOverlayInactiveRow::DiscoveredSkill { skill, .. }
        | PromptOverlayInactiveRow::DiscoveredSkillShadowedDetail { skill } => {
            if !skill.can_select_for_discovery {
                "-".to_string()
            } else if skill.selected {
                "●".to_string()
            } else {
                "○".to_string()
            }
        }
        _ => "-".to_string(),
    }
}

fn prompt_overlay_skill_name_cell(row: &PromptOverlayInactiveRow, width: usize) -> String {
    match row {
        PromptOverlayInactiveRow::DiscoveredSkill {
            skill,
            shadowed_count,
        } => {
            let trailing = if *shadowed_count > 0 {
                prompt_overlay_shadowed_count_marker(*shadowed_count)
            } else if !skill.can_select_for_discovery {
                Some("(manual)".to_string())
            } else {
                None
            };
            prompt_overlay_cell_with_trailing_marker(&skill.title, trailing.as_deref(), width)
        }
        PromptOverlayInactiveRow::DiscoveredSkillShadowedDetail { skill } => {
            prompt_overlay_cell_with_trailing_marker(
                &format!("shadowed {}", prompt_overlay_origin_label(skill.origin)),
                None,
                width,
            )
        }
        _ => prompt_overlay_fill_cell("", width),
    }
}

fn prompt_overlay_inactive_skill_origin(row: &PromptOverlayInactiveRow) -> PromptSourceOrigin {
    match row {
        PromptOverlayInactiveRow::DiscoveredSkill { skill, .. }
        | PromptOverlayInactiveRow::DiscoveredSkillShadowedDetail { skill } => skill.origin,
        _ => PromptSourceOrigin::Project,
    }
}

fn prompt_overlay_shadowed_count_marker(shadowed_count: usize) -> Option<String> {
    (shadowed_count > 0).then(|| format!("+{shadowed_count} shadowed"))
}

fn prompt_overlay_managed_status_marker(
    status: PromptOverlayManagedStatus,
    shadowed_count: usize,
) -> Option<String> {
    match status {
        PromptOverlayManagedStatus::Active => prompt_overlay_shadowed_count_marker(shadowed_count),
        PromptOverlayManagedStatus::Missing => Some("missing".to_string()),
        PromptOverlayManagedStatus::Shadowed => Some("shadowed".to_string()),
        PromptOverlayManagedStatus::Disabled => None,
    }
}

fn prompt_overlay_cell_with_trailing_marker(
    text: &str,
    trailing_marker: Option<&str>,
    width: usize,
) -> String {
    let width = width.max(1);
    let Some(trailing_marker) = trailing_marker.filter(|marker| !marker.is_empty()) else {
        return prompt_overlay_fill_cell(text, width);
    };

    let trailing_width = display_width(trailing_marker);
    let reserved_width = trailing_width.saturating_add(1);
    if width <= reserved_width {
        return truncate_display_width_with_ellipsis(trailing_marker, width);
    }

    let text_width = width.saturating_sub(reserved_width);
    let visible_text = truncate_display_width_with_ellipsis(text, text_width);
    let padding = text_width.saturating_sub(display_width(&visible_text));
    format!("{visible_text}{} {trailing_marker}", " ".repeat(padding))
}

fn prompt_overlay_fill_cell(text: &str, width: usize) -> String {
    let visible_text = truncate_display_width_with_ellipsis(text, width);
    let padding = width.saturating_sub(display_width(&visible_text));
    format!("{visible_text}{}", " ".repeat(padding))
}

fn prompt_overlay_left_source_width(width: usize) -> usize {
    width
        .saturating_sub(PROMPT_OVERLAY_OUTER_PADDING)
        .saturating_sub(PROMPT_OVERLAY_LEFT_SEL_WIDTH)
        .saturating_sub(PROMPT_OVERLAY_LEFT_ORD_WIDTH)
        .saturating_sub(PROMPT_OVERLAY_LEFT_KIND_WIDTH)
        .saturating_sub(PROMPT_OVERLAY_LEFT_SCOPE_WIDTH)
        .saturating_sub(PROMPT_OVERLAY_SCOPE_TRAILING_PADDING)
        .saturating_sub(PROMPT_OVERLAY_COLUMN_GAP * 4)
        .max(12)
}

fn prompt_overlay_right_extra_name_width(width: usize) -> usize {
    width
        .saturating_sub(PROMPT_OVERLAY_OUTER_PADDING)
        .saturating_sub(PROMPT_OVERLAY_LEFT_SEL_WIDTH)
        .saturating_sub(PROMPT_OVERLAY_RIGHT_SCOPE_WIDTH)
        .saturating_sub(PROMPT_OVERLAY_SCOPE_TRAILING_PADDING)
        .saturating_sub(PROMPT_OVERLAY_COLUMN_GAP * 2)
        .max(12)
}

fn prompt_overlay_right_skill_name_width(width: usize) -> usize {
    width
        .saturating_sub(PROMPT_OVERLAY_OUTER_PADDING)
        .saturating_sub(PROMPT_OVERLAY_LEFT_SEL_WIDTH)
        .saturating_sub(PROMPT_OVERLAY_RIGHT_SCOPE_WIDTH)
        .saturating_sub(PROMPT_OVERLAY_SCOPE_TRAILING_PADDING)
        .saturating_sub(PROMPT_OVERLAY_COLUMN_GAP * 2)
        .max(12)
}

fn prompt_overlay_list_line(
    marker: &str,
    marker_style: Style,
    content: String,
    content_style: Style,
) -> Line<'static> {
    Line::from(vec![
        Span::styled(marker.to_string(), marker_style),
        Span::styled(content, content_style),
    ])
}

fn prompt_overlay_empty_inactive_line(
    message: &str,
    width: usize,
    palette: crate::theme::TerminalPalette,
) -> Line<'static> {
    let content_width = width.saturating_sub(PROMPT_OVERLAY_ROW_PREFIX_WIDTH).max(1);
    let right_prefix = " ".repeat(PROMPT_OVERLAY_OUTER_PADDING);
    let content = format!(
        "{}{}",
        right_prefix,
        truncate_display_width_with_ellipsis(
            message,
            content_width
                .saturating_sub(PROMPT_OVERLAY_OUTER_PADDING)
                .max(1)
        ),
    );

    prompt_overlay_list_line(
        " ",
        tertiary_text_style(palette),
        truncate_display_width_with_ellipsis(&content, content_width),
        tertiary_text_style(palette),
    )
}

fn prompt_overlay_center_text(value: &str, width: usize) -> String {
    let value_width = display_width(value).min(width);
    let left = width.saturating_sub(value_width) / 2;
    let right = width.saturating_sub(value_width).saturating_sub(left);
    format!("{}{}{}", " ".repeat(left), value, " ".repeat(right))
}

fn selection_label(label: Option<&str>, selected: usize, total: usize) -> String {
    let position = if total == 0 {
        0
    } else {
        selected.min(total.saturating_sub(1)) + 1
    };

    match label {
        Some(label) => format!(" {label} {position}/{total} "),
        None => format!(" {position}/{total} "),
    }
}

fn clamp_scroll(
    current_scroll: usize,
    selected: usize,
    total: usize,
    visible_rows: usize,
) -> usize {
    if total == 0 {
        return 0;
    }
    let visible_rows = visible_rows.max(1);
    let max_scroll = total.saturating_sub(visible_rows);
    let mut scroll = current_scroll.min(max_scroll);
    if selected < scroll {
        scroll = selected;
    }
    if selected >= scroll.saturating_add(visible_rows) {
        scroll = selected + 1 - visible_rows;
    }
    scroll.min(max_scroll)
}

fn prompt_overlay_active_visible_rows(height: u16) -> usize {
    let chrome = fullscreen_list_chrome_rects(Rect::new(0, 0, 1, height));
    let body_height = chrome.map(|rects| rects.body.height).unwrap_or_default();
    usize::from(body_height.saturating_sub(1)).max(1)
}

fn prompt_overlay_inactive_visible_rows(height: u16) -> usize {
    let chrome = fullscreen_list_chrome_rects(Rect::new(0, 0, 1, height));
    let body_height = chrome.map(|rects| rects.body.height).unwrap_or_default();
    usize::from(body_height.saturating_sub(1)).max(1)
}

fn vertical_rule_lines(
    height: usize,
    palette: crate::theme::TerminalPalette,
) -> Vec<Line<'static>> {
    (0..height)
        .map(|_| Line::styled("│", tertiary_text_style(palette)))
        .collect()
}

fn prompt_scope_from_origin(origin: PromptSourceOrigin) -> Option<PromptAssemblyScope> {
    match origin {
        PromptSourceOrigin::Builtin => None,
        PromptSourceOrigin::Global => Some(PromptAssemblyScope::Global),
        PromptSourceOrigin::Project => Some(PromptAssemblyScope::Project),
    }
}

fn allows_shift_only_modifier(modifiers: KeyModifiers) -> bool {
    modifiers.is_empty() || modifiers == KeyModifiers::SHIFT
}

fn normalize_prompt_overlay_external_editor_draft(content: &str) -> String {
    content.replace("\r\n", "\n").replace('\r', "\n")
}
