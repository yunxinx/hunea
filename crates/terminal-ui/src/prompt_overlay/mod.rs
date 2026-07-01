mod preview;

use crossterm::event::{KeyCode, KeyEvent};
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
    ResolvedPromptSource,
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
        build_labeled_rule, command_accent_text_style, primary_text_style, secondary_text_style,
        subtle_rule_line, surface_text_style, table_header_text_style, tertiary_text_style,
    },
};

#[cfg(test)]
mod tests;

const PROMPT_OVERLAY_HEADER_INSET: usize = 2;
const PROMPT_OVERLAY_FOOTER_FULL_BASE: &str = "  Esc close · ←/→/h/l focus panes · ↑/↓/j/k move · Space source · p assembled · e/ctrl+g edit · s scope · a/A add extra · i/I add skill · d remove · x disable · J/K reorder";
const PROMPT_OVERLAY_FOOTER_TABS_SUFFIX: &str = " · Tab tabs";
const PROMPT_OVERLAY_FOOTER_RESTORE_SUFFIX: &str = " · r restore";
const SKILL_DISCOVERY_GENERATED_START: &str = "<!-- hunea:skill-discovery generated:start -->";
const SKILL_DISCOVERY_GENERATED_END: &str = "<!-- hunea:skill-discovery generated:end -->";
const PROMPT_OVERLAY_HEADER_TRAILING_PADDING: usize = 2;
const PROMPT_OVERLAY_ROW_PREFIX_WIDTH: usize = 1;
const PROMPT_OVERLAY_COLUMN_GAP: usize = 2;
const PROMPT_OVERLAY_OUTER_PADDING: usize = 2;
const PROMPT_OVERLAY_LEFT_SEL_WIDTH: usize = 3;
const PROMPT_OVERLAY_LEFT_ORD_WIDTH: usize = 3;
const PROMPT_OVERLAY_LEFT_KIND_WIDTH: usize = 4;
const PROMPT_OVERLAY_LEFT_SCOPE_WIDTH: usize = 7;
const PROMPT_OVERLAY_RIGHT_SKILL_ORD_WIDTH: usize = 3;
const PROMPT_OVERLAY_RIGHT_SCOPE_WIDTH: usize = 7;
const PROMPT_OVERLAY_SCOPE_TRAILING_PADDING: usize = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PromptOverlayFocus {
    Active,
    Inactive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PromptOverlayInactiveTab {
    ExtraPrompts,
    LongLivedSkills,
}

impl PromptOverlayInactiveTab {
    const ALL: [Self; 2] = [Self::ExtraPrompts, Self::LongLivedSkills];

    fn next(self) -> Self {
        match self {
            Self::ExtraPrompts => Self::LongLivedSkills,
            Self::LongLivedSkills => Self::ExtraPrompts,
        }
    }

    fn previous(self) -> Self {
        match self {
            Self::ExtraPrompts => Self::LongLivedSkills,
            Self::LongLivedSkills => Self::ExtraPrompts,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::ExtraPrompts => "Extra",
            Self::LongLivedSkills => "Skill",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PromptOverlayState {
    pub(crate) focus: PromptOverlayFocus,
    pub(crate) active_selected: usize,
    pub(crate) active_scroll: usize,
    pub(crate) inactive_tab: PromptOverlayInactiveTab,
    pub(crate) inactive_selected: usize,
    pub(crate) inactive_scroll: usize,
    pub(crate) inactive_selected_reference_id: Option<String>,
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
            inactive_tab: PromptOverlayInactiveTab::ExtraPrompts,
            inactive_selected: 0,
            inactive_scroll: 0,
            inactive_selected_reference_id: None,
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
    ExtraPromptCandidate(PromptAssemblyExtraPromptCandidate),
    DiscoveredSkill(PromptAssemblyDiscoveredSkill),
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
            KeyCode::Char('s') if key.modifiers.is_empty() => {
                self.toggle_prompt_overlay_draft_scope();
                OverlayInputResult::Handled
            }
            KeyCode::Char('e') if key.modifiers.is_empty() => {
                OverlayInputResult::from_effect(self.open_prompt_overlay_editor_for_selection())
            }
            KeyCode::Char('a') if key.modifiers.is_empty() => {
                OverlayInputResult::Effect(AppEffect::MutatePromptAssembly {
                    mutation: PromptAssemblyMutation::CreateExtraPrompt {
                        scope: PromptAssemblyScope::Project,
                        content: "# New prompt\n".to_string(),
                    },
                })
            }
            KeyCode::Char('A') if key.modifiers.is_empty() => {
                OverlayInputResult::Effect(AppEffect::MutatePromptAssembly {
                    mutation: PromptAssemblyMutation::CreateExtraPrompt {
                        scope: PromptAssemblyScope::Global,
                        content: "# New prompt\n".to_string(),
                    },
                })
            }
            KeyCode::Char('i') if key.modifiers.is_empty() => OverlayInputResult::from_effect(
                self.activate_selected_discovered_skill(PromptAssemblyScope::Project),
            ),
            KeyCode::Char('I') if key.modifiers.is_empty() => OverlayInputResult::from_effect(
                self.activate_selected_discovered_skill(PromptAssemblyScope::Global),
            ),
            KeyCode::Char('d') if key.modifiers.is_empty() => {
                OverlayInputResult::from_effect(self.remove_selected_prompt_source())
            }
            KeyCode::Char('x') if key.modifiers.is_empty() => {
                OverlayInputResult::from_effect(self.toggle_selected_prompt_source_enabled())
            }
            KeyCode::Char('K') if key.modifiers.is_empty() => OverlayInputResult::from_effect(
                self.move_selected_active_source(PromptAssemblyMoveDirection::Up),
            ),
            KeyCode::Char('J') if key.modifiers.is_empty() => OverlayInputResult::from_effect(
                self.move_selected_active_source(PromptAssemblyMoveDirection::Down),
            ),
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
        let Some(chrome) = fullscreen_list_chrome_rects(area) else {
            return;
        };
        let scope = self
            .prompt_overlay
            .as_ref()
            .map(|state| state.draft_scope)
            .unwrap_or(PromptAssemblyScope::Project);

        frame.render_widget(
            Paragraph::new(self.prompt_overlay_header_line(
                usize::from(area.width),
                scope,
                state.inactive_tab,
            )),
            chrome.header,
        );
        frame.render_widget(
            Paragraph::new(subtle_rule_line(usize::from(area.width), self.palette)),
            chrome.header_rule,
        );

        let [left_pane, gutter, right_pane] = Layout::horizontal([
            Constraint::Fill(1),
            Constraint::Length(1),
            Constraint::Fill(1),
        ])
        .areas(chrome.body);

        if gutter.width > 0 {
            frame.render_widget(
                Paragraph::new(vertical_rule_lines(
                    usize::from(gutter.height),
                    self.palette,
                )),
                gutter,
            );
        }

        self.render_prompt_overlay_active_pane(frame, left_pane, state);
        self.render_prompt_overlay_inactive_pane(frame, right_pane, state);

        let focused_page = self.prompt_overlay_focused_page_label(state, area.height);
        frame.render_widget(
            Paragraph::new(build_labeled_rule(area.width, focused_page, self.palette)),
            chrome.page_rule,
        );
        frame.render_widget(
            Paragraph::new(Line::styled(
                self.prompt_overlay_footer_hint(area.width),
                tertiary_text_style(self.palette).add_modifier(Modifier::ITALIC),
            )),
            chrome.footer,
        );
    }

    pub(crate) fn apply_prompt_overlay_external_editor_finished(
        &mut self,
        draft_path: &std::path::Path,
        original_draft: &str,
        failed: bool,
    ) -> Option<AppEffect> {
        let target = self
            .prompt_overlay
            .as_ref()
            .and_then(|state| state.pending_editor.as_ref())
            .map(|pending| pending.target.clone())?;
        let state = self.prompt_overlay.as_mut()?;
        state.pending_editor = None;

        if failed {
            let _ = std::fs::remove_file(draft_path);
            self.show_toast(crate::toast::ToastSeverity::Error, "External editor failed");
            return None;
        }
        let content = match std::fs::read_to_string(draft_path) {
            Ok(content) => content,
            Err(_) => {
                let _ = std::fs::remove_file(draft_path);
                self.show_toast(
                    crate::toast::ToastSeverity::Error,
                    "Failed to read external editor draft",
                );
                return None;
            }
        };
        let _ = std::fs::remove_file(draft_path);
        if content == original_draft {
            return None;
        }
        Some(AppEffect::MutatePromptAssembly {
            mutation: PromptAssemblyMutation::SaveEditorTarget { target, content },
        })
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
            PromptOverlaySelection::ExtraPromptCandidate(candidate) => {
                self.open_prompt_overlay_plain_text_preview(candidate.title, &candidate.body);
            }
            PromptOverlaySelection::DiscoveredSkill(skill) => {
                self.open_prompt_overlay_plain_text_preview(skill.title.clone(), &skill.body);
            }
        }
    }

    fn toggle_prompt_overlay_draft_scope(&mut self) {
        let Some(state) = self.prompt_overlay.as_mut() else {
            return;
        };
        state.draft_scope = match state.draft_scope {
            PromptAssemblyScope::Global => PromptAssemblyScope::Project,
            PromptAssemblyScope::Project => PromptAssemblyScope::Global,
        };
    }

    pub(crate) fn open_prompt_overlay_editor_for_selection(&mut self) -> Option<AppEffect> {
        let selected = self.selected_prompt_overlay_source()?;
        let scope = self
            .prompt_overlay
            .as_ref()
            .map(|state| state.draft_scope)
            .unwrap_or(PromptAssemblyScope::Project);
        let manager_source = self.manager_source_for_resolved_source(&selected)?;

        let (target, initial_content) = match selected.kind {
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
            PromptSourceKind::LongLivedSkill => (
                PromptAssemblyEditorTarget::SkillFile {
                    skill_name: selected.reference_id.clone(),
                    origin: manager_source.resolved_body_origin.or(selected.origin)?,
                },
                manager_source.body.unwrap_or_default(),
            ),
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
        let selected = self.selected_prompt_overlay_managed_source()?;
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
        let selected = self.selected_prompt_overlay_managed_source()?;
        Some(ResolvedPromptSource {
            reference_id: selected.reference_id,
            kind: selected.kind,
            title: selected.title,
            origin: selected.origin,
            status: PromptSourceStatus::Active {
                order: selected.order,
            },
        })
    }

    fn selected_prompt_overlay_managed_source(&self) -> Option<PromptAssemblyManagedSource> {
        let state = self.prompt_overlay.as_ref()?;
        match state.focus {
            PromptOverlayFocus::Active => self
                .prompt_assembly
                .managed_sources
                .get(state.active_selected)
                .cloned(),
            PromptOverlayFocus::Inactive => None,
        }
    }

    fn selected_prompt_overlay_selection(&self) -> Option<PromptOverlaySelection> {
        let state = self.prompt_overlay.as_ref()?;
        match state.focus {
            PromptOverlayFocus::Active => self
                .prompt_assembly
                .managed_sources
                .get(state.active_selected)
                .cloned()
                .map(PromptOverlaySelection::ManagedSource),
            PromptOverlayFocus::Inactive => match state.inactive_tab {
                PromptOverlayInactiveTab::LongLivedSkills => self
                    .prompt_assembly
                    .discovered_skills
                    .get(state.inactive_selected)
                    .cloned()
                    .map(PromptOverlaySelection::DiscoveredSkill),
                PromptOverlayInactiveTab::ExtraPrompts => self
                    .prompt_assembly
                    .extra_prompt_candidates
                    .get(state.inactive_selected)
                    .cloned()
                    .map(PromptOverlaySelection::ExtraPromptCandidate),
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

    fn activate_selected_discovered_skill(
        &mut self,
        scope: PromptAssemblyScope,
    ) -> Option<AppEffect> {
        let state = self.prompt_overlay.as_ref()?;
        if state.focus != PromptOverlayFocus::Inactive
            || state.inactive_tab != PromptOverlayInactiveTab::LongLivedSkills
        {
            return None;
        }
        let selected = self
            .prompt_assembly
            .discovered_skills
            .get(state.inactive_selected)?;
        Some(AppEffect::MutatePromptAssembly {
            mutation: PromptAssemblyMutation::SetDiscoveredSkillSelected {
                scope,
                skill_name: selected.skill_name.clone(),
                selected: !selected.selected,
            },
        })
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
        let selected = self.selected_prompt_overlay_managed_source()?;
        if selected.kind == PromptSourceKind::CoreSystemPrompt {
            return None;
        }
        Some(AppEffect::MutatePromptAssembly {
            mutation: PromptAssemblyMutation::SetPromptSourceEnabled {
                scope: prompt_scope_from_origin(selected.origin?)?,
                kind: selected.kind,
                reference_id: selected.reference_id,
                enabled: !selected.enabled,
            },
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
        let inactive_reference_id = if matches!(focus, PromptOverlayFocus::Inactive) {
            let source_count = self.prompt_overlay_inactive_source_count(inactive_tab);
            let index = if first {
                0
            } else {
                source_count.saturating_sub(1)
            };
            self.prompt_overlay_inactive_reference_id_at(inactive_tab, index)
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
                let last_index = self.prompt_assembly.managed_sources.len().saturating_sub(1);
                state.active_selected = if first { 0 } else { last_index };
            }
            PromptOverlayFocus::Inactive => {
                let last_index = inactive_count.saturating_sub(1);
                state.inactive_selected = if first { 0 } else { last_index };
                state.inactive_selected_reference_id = inactive_reference_id;
            }
        }
        self.sync_prompt_overlay_state();
    }

    fn move_prompt_overlay_active_selection(&mut self, direction: ListNavigationDirection) {
        let Some(state) = self.prompt_overlay.as_mut() else {
            return;
        };
        let count = self.prompt_assembly.managed_sources.len();
        if count == 0 {
            state.active_selected = 0;
            state.active_scroll = 0;
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
            state.inactive_selected_reference_id = None;
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
        let next_reference_id = self.prompt_overlay_inactive_reference_id_at(inactive_tab, next);

        let Some(state) = self.prompt_overlay.as_mut() else {
            return;
        };
        state.inactive_selected = next;
        state.inactive_selected_reference_id = next_reference_id;
        self.sync_prompt_overlay_state();
    }

    pub(crate) fn sync_prompt_overlay_state(&mut self) {
        let inactive_tab = match self.prompt_overlay.as_ref() {
            Some(state) => state.inactive_tab,
            None => return,
        };
        let inactive_source_count = self.prompt_overlay_inactive_source_count(inactive_tab);

        let active_count = self.prompt_assembly.managed_sources.len();
        let (
            current_active_selected,
            current_active_scroll,
            current_inactive_selected,
            current_inactive_scroll,
            current_inactive_reference_id,
        ) = match self.prompt_overlay.as_ref() {
            Some(state) => (
                state.active_selected,
                state.active_scroll,
                state.inactive_selected,
                state.inactive_scroll,
                state.inactive_selected_reference_id.clone(),
            ),
            None => return,
        };

        let next_active_selected = current_active_selected.min(active_count.saturating_sub(1));
        let next_active_scroll = clamp_scroll(
            current_active_scroll,
            next_active_selected,
            active_count,
            prompt_overlay_active_visible_rows(self.height),
        );

        let mut next_inactive_selected = current_inactive_selected;
        if let Some(reference_id) = current_inactive_reference_id.as_deref() {
            let matched_index = if inactive_tab == PromptOverlayInactiveTab::LongLivedSkills {
                self.prompt_assembly
                    .discovered_skills
                    .iter()
                    .position(|skill| skill.skill_name == reference_id)
            } else {
                self.prompt_assembly
                    .extra_prompt_candidates
                    .iter()
                    .position(|source| source.reference_id == reference_id)
            };
            if let Some(index) = matched_index {
                next_inactive_selected = index;
            }
        }
        next_inactive_selected =
            next_inactive_selected.min(inactive_source_count.saturating_sub(1));
        let next_inactive_reference_id =
            self.prompt_overlay_inactive_reference_id_at(inactive_tab, next_inactive_selected);

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
        state.inactive_selected = next_inactive_selected;
        state.inactive_selected_reference_id = next_inactive_reference_id;
        state.inactive_scroll = next_inactive_scroll;
    }

    fn prompt_overlay_header_line(
        &self,
        width: usize,
        scope: PromptAssemblyScope,
        active_tab: PromptOverlayInactiveTab,
    ) -> Line<'static> {
        let title = format!(
            "Prompt Assembly · scope={} · {} active · {} candidates",
            match scope {
                PromptAssemblyScope::Global => "global",
                PromptAssemblyScope::Project => "project",
            },
            self.prompt_assembly
                .managed_sources
                .iter()
                .filter(|source| source.enabled)
                .count(),
            self.prompt_assembly.extra_prompt_candidates.len()
                + self.prompt_assembly.discovered_skills.len()
        );
        let tabs = self.prompt_overlay_tabs_plain(active_tab);
        let available_width = width.saturating_sub(PROMPT_OVERLAY_HEADER_INSET);
        let tabs_width = display_width(&tabs) + PROMPT_OVERLAY_HEADER_TRAILING_PADDING;
        let title_width = available_width
            .saturating_sub(tabs_width)
            .saturating_sub(1)
            .max(1);
        let title = truncate_display_width_with_ellipsis(&title, title_width);
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
        let mut text = if width < 120 {
            "  Esc close · Space preview · e edit · d remove · x disable".to_string()
        } else {
            PROMPT_OVERLAY_FOOTER_FULL_BASE.to_string()
        };
        let focus_right = self
            .prompt_overlay
            .as_ref()
            .is_some_and(|state| state.focus == PromptOverlayFocus::Inactive);
        if focus_right {
            text.push_str(PROMPT_OVERLAY_FOOTER_TABS_SUFFIX);
        }
        let selected_core = self
            .selected_prompt_overlay_source()
            .is_some_and(|source| source.kind == PromptSourceKind::CoreSystemPrompt);
        if selected_core {
            text.push_str(PROMPT_OVERLAY_FOOTER_RESTORE_SUFFIX);
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

        let sources = self.prompt_assembly.managed_sources.as_slice();
        let lines = prompt_overlay_active_lines(
            sources,
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
                &self.prompt_assembly.discovered_skills,
                state.inactive_selected_reference_id.as_deref(),
                state.inactive_scroll,
                state.focus == PromptOverlayFocus::Inactive,
                usize::from(body_area.width),
                usize::from(body_area.height),
                self.palette,
            )
        } else {
            prompt_overlay_inactive_lines(
                &self.prompt_assembly.extra_prompt_candidates,
                state.inactive_selected_reference_id.as_deref(),
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

    fn prompt_overlay_focused_page_label(&self, state: &PromptOverlayState, height: u16) -> String {
        match state.focus {
            PromptOverlayFocus::Active => page_label(
                "Active",
                state.active_selected,
                self.prompt_assembly.managed_sources.len(),
                prompt_overlay_active_visible_rows(height),
            ),
            PromptOverlayFocus::Inactive => page_label(
                "Inactive",
                state.inactive_selected,
                self.prompt_overlay_inactive_source_count(state.inactive_tab),
                prompt_overlay_inactive_visible_rows(height),
            ),
        }
    }

    pub(crate) fn prompt_overlay_inactive_source_count(
        &self,
        tab: PromptOverlayInactiveTab,
    ) -> usize {
        if tab == PromptOverlayInactiveTab::LongLivedSkills {
            return self.prompt_assembly.discovered_skills.len();
        }
        self.prompt_assembly.extra_prompt_candidates.len()
    }

    fn prompt_overlay_inactive_reference_id_at(
        &self,
        tab: PromptOverlayInactiveTab,
        index: usize,
    ) -> Option<String> {
        if tab == PromptOverlayInactiveTab::LongLivedSkills {
            return self
                .prompt_assembly
                .discovered_skills
                .get(index)
                .map(|skill| skill.skill_name.clone());
        }
        self.prompt_assembly
            .extra_prompt_candidates
            .get(index)
            .map(|source| source.reference_id.clone())
    }
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
    sources: &[PromptAssemblyManagedSource],
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
        lines.push(prompt_overlay_managed_source_line(
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
    rows: &[PromptAssemblyExtraPromptCandidate],
    selected_reference_id: Option<&str>,
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
        lines.push(prompt_overlay_extra_candidate_line(
            row,
            selected_reference_id == Some(row.reference_id.as_str()),
            focused,
            width,
            palette,
        ));
    }
    lines
}

fn prompt_overlay_managed_source_line(
    source: &PromptAssemblyManagedSource,
    selected: bool,
    focused: bool,
    width: usize,
    palette: crate::theme::TerminalPalette,
) -> Line<'static> {
    let content_width = width.saturating_sub(PROMPT_OVERLAY_ROW_PREFIX_WIDTH).max(1);
    let label = prompt_overlay_active_row_text(source, content_width);
    let item_style = if selected {
        primary_text_style(palette).bold()
    } else {
        secondary_text_style(palette)
    };
    let marker_style = if selected {
        command_accent_text_style(palette)
    } else {
        tertiary_text_style(palette)
    };
    let marker = if selected && focused { "█" } else { " " };
    prompt_overlay_list_line(
        marker,
        marker_style,
        truncate_display_width_with_ellipsis(&label, content_width),
        item_style,
    )
}

fn prompt_overlay_discovered_skill_lines(
    skills: &[PromptAssemblyDiscoveredSkill],
    selected_reference_id: Option<&str>,
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
        .skip(scroll)
        .take(body_height)
        .map(|skill| {
            let selected = selected_reference_id == Some(skill.skill_name.as_str());
            let content_width = width.saturating_sub(PROMPT_OVERLAY_ROW_PREFIX_WIDTH).max(1);
            let item_style = if selected {
                primary_text_style(palette).bold()
            } else {
                secondary_text_style(palette)
            };
            let marker_style = if selected {
                command_accent_text_style(palette)
            } else {
                tertiary_text_style(palette)
            };
            let marker = if selected && focused { "█" } else { " " };
            let label = prompt_overlay_skill_row_text(skill, content_width);
            prompt_overlay_list_line(
                marker,
                marker_style,
                truncate_display_width_with_ellipsis(&label, content_width),
                item_style,
            )
        })
        .collect()
}

fn prompt_overlay_extra_candidate_line(
    source: &PromptAssemblyExtraPromptCandidate,
    selected: bool,
    focused: bool,
    width: usize,
    palette: crate::theme::TerminalPalette,
) -> Line<'static> {
    let content_width = width.saturating_sub(PROMPT_OVERLAY_ROW_PREFIX_WIDTH).max(1);
    let item_style = if selected {
        primary_text_style(palette).bold()
    } else {
        secondary_text_style(palette)
    };
    let marker_style = if selected {
        command_accent_text_style(palette)
    } else {
        tertiary_text_style(palette)
    };
    let marker = if selected && focused { "█" } else { " " };
    let label = prompt_overlay_extra_row_text(source, content_width);
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

fn prompt_overlay_kind_short_label(kind: PromptSourceKind) -> &'static str {
    match kind {
        PromptSourceKind::CoreSystemPrompt => "sys",
        PromptSourceKind::ExtraPrompt => "ext",
        PromptSourceKind::SkillDiscovery => "disc",
        PromptSourceKind::LongLivedSkill => "sk",
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
    format!("{left_pad}{name}{gap}{scope}{trailing}")
}

fn prompt_overlay_skill_header_text(width: usize) -> String {
    let name_width = prompt_overlay_right_skill_name_width(width);
    let left_pad = " ".repeat(PROMPT_OVERLAY_OUTER_PADDING);
    let ord = left_pad_display_width("Num", PROMPT_OVERLAY_RIGHT_SKILL_ORD_WIDTH);
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
    format!("{left_pad}{ord}{gap}{name}{gap}{scope}{trailing}")
}

fn prompt_overlay_active_row_text(source: &PromptAssemblyManagedSource, width: usize) -> String {
    let source_width = prompt_overlay_left_source_width(width);
    let left_pad = " ".repeat(PROMPT_OVERLAY_OUTER_PADDING);
    let sel = prompt_overlay_center_text(
        if source.enabled { "●" } else { "○" },
        PROMPT_OVERLAY_LEFT_SEL_WIDTH,
    );
    let ord = left_pad_display_width(&source.order.to_string(), PROMPT_OVERLAY_LEFT_ORD_WIDTH);
    let source_name = format!(
        "{:<width$}",
        truncate_display_width_with_ellipsis(&source.title, source_width),
        width = source_width
    );
    let kind = format!(
        "{:<width$}",
        prompt_overlay_kind_short_label(source.kind),
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
    width: usize,
) -> String {
    let name_width = prompt_overlay_right_extra_name_width(width);
    let left_pad = " ".repeat(PROMPT_OVERLAY_OUTER_PADDING);
    let name = format!(
        "{:<width$}",
        truncate_display_width_with_ellipsis(&source.title, name_width),
        width = name_width
    );
    let scope = format!(
        "{:<width$}",
        prompt_overlay_origin_label(source.origin),
        width = PROMPT_OVERLAY_RIGHT_SCOPE_WIDTH
    );
    let gap = " ".repeat(PROMPT_OVERLAY_COLUMN_GAP);
    let trailing = " ".repeat(PROMPT_OVERLAY_SCOPE_TRAILING_PADDING);
    format!("{left_pad}{name}{gap}{scope}{trailing}")
}

fn prompt_overlay_skill_row_text(skill: &PromptAssemblyDiscoveredSkill, width: usize) -> String {
    let name_width = prompt_overlay_right_skill_name_width(width);
    let left_pad = " ".repeat(PROMPT_OVERLAY_OUTER_PADDING);
    let ord = left_pad_display_width(
        &skill
            .selected_order
            .map(|order| order.to_string())
            .unwrap_or_else(|| "-".to_string()),
        PROMPT_OVERLAY_RIGHT_SKILL_ORD_WIDTH,
    );
    let name = format!(
        "{:<width$}",
        truncate_display_width_with_ellipsis(&skill.title, name_width),
        width = name_width
    );
    let scope = format!(
        "{:<width$}",
        prompt_overlay_origin_label(skill.origin),
        width = PROMPT_OVERLAY_RIGHT_SCOPE_WIDTH
    );
    let gap = " ".repeat(PROMPT_OVERLAY_COLUMN_GAP);
    let trailing = " ".repeat(PROMPT_OVERLAY_SCOPE_TRAILING_PADDING);
    format!("{left_pad}{ord}{gap}{name}{gap}{scope}{trailing}")
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
        .saturating_sub(PROMPT_OVERLAY_RIGHT_SCOPE_WIDTH)
        .saturating_sub(PROMPT_OVERLAY_SCOPE_TRAILING_PADDING)
        .saturating_sub(PROMPT_OVERLAY_COLUMN_GAP)
        .max(12)
}

fn prompt_overlay_right_skill_name_width(width: usize) -> usize {
    width
        .saturating_sub(PROMPT_OVERLAY_OUTER_PADDING)
        .saturating_sub(PROMPT_OVERLAY_RIGHT_SKILL_ORD_WIDTH)
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
    let content = format!(
        "{}{}",
        " ".repeat(PROMPT_OVERLAY_OUTER_PADDING),
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

fn page_label(label: &str, selected: usize, total: usize, visible_rows: usize) -> String {
    let page_size = visible_rows.max(1);
    let page_count = total.max(1).div_ceil(page_size);
    let page_number = if total == 0 {
        1
    } else {
        selected / page_size + 1
    };
    format!(" {label} {page_number}/{page_count} ")
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
