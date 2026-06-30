mod preview;

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::Modifier,
    text::{Line, Span},
    widgets::{Clear, Paragraph, Widget},
};
use runtime_domain::prompt_assembly::persistence::PromptAssemblyScope;
use runtime_domain::prompt_assembly::{
    PromptAssemblyEditorTarget, PromptAssemblyLifecycle, PromptAssemblyMutation,
    PromptSourceInactiveReason, PromptSourceKind, PromptSourceOrigin, PromptSourceStatus,
    ResolvedPromptSource,
};

use crate::{
    AppEffect, Model,
    fullscreen_list_chrome::fullscreen_list_chrome_rects,
    list_selection::ListNavigationDirection,
    overlay_input_result::OverlayInputResult,
    render_frame::RenderFrame,
    status_line::truncate_display_width_with_ellipsis,
    styled_text::render_line_with_full_width_background,
    theme::{
        build_labeled_rule, command_accent_text_style, primary_text_style, secondary_text_style,
        subtle_rule_line, surface_text_style, tertiary_text_style,
    },
};

#[cfg(test)]
mod tests;

const PROMPT_OVERLAY_HEADER_INSET: usize = 2;
const PROMPT_OVERLAY_PANE_TITLE_ROWS: u16 = 1;
const PROMPT_OVERLAY_RIGHT_TAB_ROWS: u16 = 1;
const PROMPT_OVERLAY_FOOTER_COMPACT: &str =
    "  Esc close · Space source · p assembled · e/ctrl+g edit · a/A add extra";
const PROMPT_OVERLAY_FOOTER_FULL: &str = "  Esc close · ←/→/h/l focus panes · ↑/↓/j/k move · Tab tabs · Space source · p assembled · e/ctrl+g edit · s scope · d delete · r restore";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PromptOverlayFocus {
    Active,
    Inactive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PromptOverlayInactiveTab {
    All,
    ExtraPrompts,
    SkillDiscovery,
    LongLivedSkills,
}

impl PromptOverlayInactiveTab {
    const ALL: [Self; 4] = [
        Self::All,
        Self::ExtraPrompts,
        Self::SkillDiscovery,
        Self::LongLivedSkills,
    ];

    fn next(self) -> Self {
        match self {
            Self::All => Self::ExtraPrompts,
            Self::ExtraPrompts => Self::SkillDiscovery,
            Self::SkillDiscovery => Self::LongLivedSkills,
            Self::LongLivedSkills => Self::All,
        }
    }

    fn previous(self) -> Self {
        match self {
            Self::All => Self::LongLivedSkills,
            Self::ExtraPrompts => Self::All,
            Self::SkillDiscovery => Self::ExtraPrompts,
            Self::LongLivedSkills => Self::SkillDiscovery,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::ExtraPrompts => "Extra",
            Self::SkillDiscovery => "Discovery",
            Self::LongLivedSkills => "Skills",
        }
    }

    fn matches_kind(self, kind: PromptSourceKind) -> bool {
        match self {
            Self::All => true,
            Self::ExtraPrompts => kind == PromptSourceKind::ExtraPrompt,
            Self::SkillDiscovery => kind == PromptSourceKind::SkillDiscovery,
            Self::LongLivedSkills => kind == PromptSourceKind::LongLivedSkill,
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
            inactive_tab: PromptOverlayInactiveTab::All,
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

#[derive(Debug, Clone, Copy)]
enum PromptOverlayRenderedRow<'a> {
    GroupHeader(PromptSourceInactiveReason),
    Source(&'a ResolvedPromptSource),
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
                self.cycle_prompt_overlay_inactive_tab(1);
                OverlayInputResult::Handled
            }
            KeyCode::BackTab => {
                self.cycle_prompt_overlay_inactive_tab(-1);
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
            KeyCode::Char('d') if key.modifiers.is_empty() => {
                OverlayInputResult::from_effect(self.delete_selected_extra_prompt())
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
        let Some(chrome) = fullscreen_list_chrome_rects(area) else {
            return;
        };
        let scope = self
            .prompt_overlay
            .as_ref()
            .map(|state| state.draft_scope)
            .unwrap_or(PromptAssemblyScope::Project);

        frame.render_widget(
            Paragraph::new(self.prompt_overlay_header_line(usize::from(area.width), scope)),
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
                prompt_overlay_footer_hint(area.width),
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
        let Some(source) = self.selected_prompt_overlay_manager_source() else {
            return;
        };
        self.open_prompt_overlay_source_preview(source);
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
                    origin: selected.origin?,
                },
                manager_source.body.unwrap_or_default(),
            ),
            PromptSourceKind::SkillDiscovery => return None,
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

    fn delete_selected_extra_prompt(&mut self) -> Option<AppEffect> {
        let selected = self.selected_prompt_overlay_source()?;
        if selected.kind != PromptSourceKind::ExtraPrompt {
            return None;
        }
        Some(AppEffect::MutatePromptAssembly {
            mutation: PromptAssemblyMutation::DeleteExtraPrompt {
                scope: prompt_scope_from_origin(selected.origin?)?,
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
        let state = self.prompt_overlay.as_ref()?;
        match state.focus {
            PromptOverlayFocus::Active => self
                .prompt_assembly
                .snapshot
                .active_sources
                .get(state.active_selected)
                .cloned(),
            PromptOverlayFocus::Inactive => self
                .prompt_overlay_inactive_sources_for_tab(state.inactive_tab)
                .get(state.inactive_selected)
                .cloned(),
        }
    }

    fn selected_prompt_overlay_manager_source(
        &self,
    ) -> Option<runtime_domain::prompt_assembly::PromptAssemblyManagerSource> {
        let selected = self.selected_prompt_overlay_source()?;
        self.manager_source_for_resolved_source(&selected)
    }

    fn manager_source_for_resolved_source(
        &self,
        selected: &ResolvedPromptSource,
    ) -> Option<runtime_domain::prompt_assembly::PromptAssemblyManagerSource> {
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
            let sources = self.prompt_overlay_inactive_sources_for_tab(inactive_tab);
            let index = if first {
                0
            } else {
                sources.len().saturating_sub(1)
            };
            sources.get(index).map(|source| source.reference_id.clone())
        } else {
            None
        };
        let inactive_count = if matches!(focus, PromptOverlayFocus::Inactive) {
            self.prompt_overlay_inactive_sources_for_tab(inactive_tab)
                .len()
        } else {
            0
        };

        let Some(state) = self.prompt_overlay.as_mut() else {
            return;
        };

        match focus {
            PromptOverlayFocus::Active => {
                let last_index = self
                    .prompt_assembly
                    .snapshot
                    .active_sources
                    .len()
                    .saturating_sub(1);
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
        let count = self.prompt_assembly.snapshot.active_sources.len();
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
        let sources = self.prompt_overlay_inactive_sources_for_tab(inactive_tab);

        let Some(state) = self.prompt_overlay.as_mut() else {
            return;
        };
        if sources.is_empty() {
            state.inactive_selected = 0;
            state.inactive_scroll = 0;
            state.inactive_selected_reference_id = None;
            return;
        }

        let next = match direction {
            ListNavigationDirection::Previous => state.inactive_selected.saturating_sub(1),
            ListNavigationDirection::Next => state
                .inactive_selected
                .saturating_add(1)
                .min(sources.len().saturating_sub(1)),
        };
        state.inactive_selected = next;
        state.inactive_selected_reference_id = Some(sources[next].reference_id.clone());
        self.sync_prompt_overlay_state();
    }

    pub(crate) fn sync_prompt_overlay_state(&mut self) {
        let inactive_tab = match self.prompt_overlay.as_ref() {
            Some(state) => state.inactive_tab,
            None => return,
        };
        let inactive_sources = self.prompt_overlay_inactive_sources_for_tab(inactive_tab);
        let rendered_rows = prompt_overlay_inactive_rendered_rows(&inactive_sources);

        let Some(state) = self.prompt_overlay.as_mut() else {
            return;
        };

        let active_count = self.prompt_assembly.snapshot.active_sources.len();
        state.active_selected = state.active_selected.min(active_count.saturating_sub(1));
        state.active_scroll = clamp_scroll(
            state.active_scroll,
            state.active_selected,
            active_count,
            prompt_overlay_active_visible_rows(self.height),
        );

        if let Some(reference_id) = state.inactive_selected_reference_id.as_deref()
            && let Some(index) = inactive_sources
                .iter()
                .position(|source| source.reference_id == reference_id)
        {
            state.inactive_selected = index;
        }
        state.inactive_selected = state
            .inactive_selected
            .min(inactive_sources.len().saturating_sub(1));
        state.inactive_selected_reference_id = inactive_sources
            .get(state.inactive_selected)
            .map(|source| source.reference_id.clone());

        let selected_row = prompt_overlay_inactive_selected_row_index(
            &rendered_rows,
            state.inactive_selected_reference_id.as_deref(),
        )
        .unwrap_or_default();
        state.inactive_scroll = clamp_scroll(
            state.inactive_scroll,
            selected_row,
            rendered_rows.len(),
            prompt_overlay_inactive_visible_rows(self.height),
        );
    }

    fn prompt_overlay_header_line(
        &self,
        width: usize,
        scope: PromptAssemblyScope,
    ) -> Line<'static> {
        let lifecycle = match self.prompt_assembly.snapshot.lifecycle {
            PromptAssemblyLifecycle::NextNewSession => "Next New Session",
        };
        let title = format!(
            "Prompt Assembly · {lifecycle} · scope={} · {} active · {} inactive",
            match scope {
                PromptAssemblyScope::Global => "global",
                PromptAssemblyScope::Project => "project",
            },
            self.prompt_assembly.snapshot.active_sources.len(),
            self.prompt_assembly.snapshot.inactive_sources.len()
        );

        Line::from(vec![
            Span::raw(" ".repeat(PROMPT_OVERLAY_HEADER_INSET)),
            Span::styled(
                truncate_display_width_with_ellipsis(
                    &title,
                    width.saturating_sub(PROMPT_OVERLAY_HEADER_INSET).max(1),
                ),
                primary_text_style(self.palette).bold(),
            ),
        ])
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
        let [title_area, body_area] = Layout::vertical([
            Constraint::Length(PROMPT_OVERLAY_PANE_TITLE_ROWS),
            Constraint::Fill(1),
        ])
        .areas(area);

        frame.render_widget(
            Paragraph::new(self.prompt_overlay_pane_title_line(
                "Active Sources",
                self.prompt_assembly.snapshot.active_sources.len(),
                state.focus == PromptOverlayFocus::Active,
                usize::from(title_area.width),
            )),
            title_area,
        );

        let sources = self.prompt_assembly.snapshot.active_sources.as_slice();
        let lines = prompt_overlay_active_lines(
            sources,
            state.active_selected,
            state.active_scroll,
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
        let [title_area, tabs_area, body_area] = Layout::vertical([
            Constraint::Length(PROMPT_OVERLAY_PANE_TITLE_ROWS),
            Constraint::Length(PROMPT_OVERLAY_RIGHT_TAB_ROWS),
            Constraint::Fill(1),
        ])
        .areas(area);

        let filtered_sources = self.prompt_overlay_inactive_sources_for_tab(state.inactive_tab);
        let rendered_rows = prompt_overlay_inactive_rendered_rows(&filtered_sources);

        frame.render_widget(
            Paragraph::new(self.prompt_overlay_pane_title_line(
                "Inactive Sources",
                filtered_sources.len(),
                state.focus == PromptOverlayFocus::Inactive,
                usize::from(title_area.width),
            )),
            title_area,
        );
        frame.render_widget(
            Paragraph::new(self.prompt_overlay_tabs_line(
                state.inactive_tab,
                state.focus == PromptOverlayFocus::Inactive,
            )),
            tabs_area,
        );

        let lines = prompt_overlay_inactive_lines(
            &rendered_rows,
            state.inactive_selected_reference_id.as_deref(),
            state.inactive_scroll,
            usize::from(body_area.width),
            usize::from(body_area.height),
            self.palette,
        );
        frame.render_widget(PromptOverlayLineListWidget { lines: &lines }, body_area);
    }

    fn prompt_overlay_pane_title_line(
        &self,
        title: &str,
        count: usize,
        focused: bool,
        width: usize,
    ) -> Line<'static> {
        let label = format!("{title} ({count})");
        let style = if focused {
            command_accent_text_style(self.palette).bold()
        } else {
            secondary_text_style(self.palette).bold()
        };
        Line::from(vec![
            Span::raw("  "),
            Span::styled(
                truncate_display_width_with_ellipsis(&label, width.saturating_sub(2).max(1)),
                style,
            ),
        ])
    }

    fn prompt_overlay_tabs_line(
        &self,
        active_tab: PromptOverlayInactiveTab,
        focused: bool,
    ) -> Line<'static> {
        let mut spans = vec![
            Span::raw("  "),
            Span::styled("Tabs: ", secondary_text_style(self.palette)),
        ];

        for (index, tab) in PromptOverlayInactiveTab::ALL.iter().copied().enumerate() {
            if index > 0 {
                spans.push(Span::raw("  "));
            }
            let is_active = tab == active_tab;
            let label = if is_active {
                format!("[{}]", tab.label())
            } else {
                tab.label().to_string()
            };
            let style = if is_active {
                if focused {
                    surface_text_style(self.palette).bold()
                } else {
                    secondary_text_style(self.palette).bold()
                }
            } else {
                tertiary_text_style(self.palette)
            };
            spans.push(Span::styled(label, style));
        }

        Line::from(spans)
    }

    fn prompt_overlay_focused_page_label(&self, state: &PromptOverlayState, height: u16) -> String {
        match state.focus {
            PromptOverlayFocus::Active => page_label(
                "Active",
                state.active_selected,
                self.prompt_assembly.snapshot.active_sources.len(),
                prompt_overlay_active_visible_rows(height),
            ),
            PromptOverlayFocus::Inactive => page_label(
                "Inactive",
                state.inactive_selected,
                self.prompt_overlay_inactive_sources_for_tab(state.inactive_tab)
                    .len(),
                prompt_overlay_inactive_visible_rows(height),
            ),
        }
    }

    fn prompt_overlay_inactive_sources_for_tab(
        &self,
        tab: PromptOverlayInactiveTab,
    ) -> Vec<ResolvedPromptSource> {
        self.prompt_assembly
            .snapshot
            .inactive_sources
            .iter()
            .filter(|source| tab.matches_kind(source.kind))
            .cloned()
            .collect()
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
    sources: &[ResolvedPromptSource],
    selected: usize,
    scroll: usize,
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
        lines.push(prompt_overlay_source_line(
            source,
            index == selected,
            width,
            palette,
        ));
    }
    lines
}

fn prompt_overlay_inactive_lines(
    rows: &[PromptOverlayRenderedRow<'_>],
    selected_reference_id: Option<&str>,
    scroll: usize,
    width: usize,
    body_height: usize,
    palette: crate::theme::TerminalPalette,
) -> Vec<Line<'static>> {
    if body_height == 0 {
        return Vec::new();
    }
    if rows.is_empty() {
        return vec![Line::styled(
            truncate_display_width_with_ellipsis(
                "  No inactive sources in this filter",
                width.max(1),
            ),
            tertiary_text_style(palette),
        )];
    }

    let mut lines = Vec::new();
    for row in rows.iter().skip(scroll).take(body_height) {
        match row {
            PromptOverlayRenderedRow::GroupHeader(reason) => {
                lines.push(Line::from(vec![
                    Span::raw("  "),
                    Span::styled(
                        truncate_display_width_with_ellipsis(
                            prompt_overlay_group_label(*reason),
                            width.saturating_sub(2).max(1),
                        ),
                        secondary_text_style(palette).bold(),
                    ),
                ]));
            }
            PromptOverlayRenderedRow::Source(source) => lines.push(prompt_overlay_source_line(
                source,
                selected_reference_id == Some(source.reference_id.as_str()),
                width,
                palette,
            )),
        }
    }
    lines
}

fn prompt_overlay_source_line(
    source: &ResolvedPromptSource,
    selected: bool,
    width: usize,
    palette: crate::theme::TerminalPalette,
) -> Line<'static> {
    let label = prompt_overlay_source_label(source);
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
    let marker = if selected { "█" } else { " " };

    Line::from(vec![
        Span::styled(marker, marker_style),
        Span::raw(" "),
        Span::styled(
            truncate_display_width_with_ellipsis(&label, width.saturating_sub(2).max(1)),
            item_style,
        ),
    ])
}

fn prompt_overlay_source_label(source: &ResolvedPromptSource) -> String {
    let mut parts = vec![source.title.clone()];
    if let Some(origin) = source.origin {
        parts.push(prompt_overlay_origin_label(origin).to_string());
    }
    parts.push(prompt_overlay_kind_label(source.kind).to_string());
    parts.join(" · ")
}

fn prompt_overlay_kind_label(kind: PromptSourceKind) -> &'static str {
    match kind {
        PromptSourceKind::CoreSystemPrompt => "system",
        PromptSourceKind::ExtraPrompt => "extra",
        PromptSourceKind::SkillDiscovery => "discovery",
        PromptSourceKind::LongLivedSkill => "skill",
    }
}

fn prompt_overlay_origin_label(origin: PromptSourceOrigin) -> &'static str {
    match origin {
        PromptSourceOrigin::Builtin => "builtin",
        PromptSourceOrigin::Global => "global",
        PromptSourceOrigin::Project => "project",
    }
}

fn prompt_overlay_group_label(reason: PromptSourceInactiveReason) -> &'static str {
    match reason {
        PromptSourceInactiveReason::Disabled => "Disabled",
        PromptSourceInactiveReason::Missing => "Missing",
        PromptSourceInactiveReason::Shadowed => "Shadowed",
    }
}

fn prompt_overlay_inactive_rendered_rows(
    sources: &[ResolvedPromptSource],
) -> Vec<PromptOverlayRenderedRow<'_>> {
    let mut rows = Vec::new();
    let mut previous_reason = None;
    for source in sources {
        let PromptSourceStatus::Inactive { reason } = source.status else {
            continue;
        };
        if previous_reason != Some(reason) {
            rows.push(PromptOverlayRenderedRow::GroupHeader(reason));
            previous_reason = Some(reason);
        }
        rows.push(PromptOverlayRenderedRow::Source(source));
    }
    rows
}

fn prompt_overlay_inactive_selected_row_index(
    rows: &[PromptOverlayRenderedRow<'_>],
    selected_reference_id: Option<&str>,
) -> Option<usize> {
    rows.iter().position(|row| match row {
        PromptOverlayRenderedRow::GroupHeader(_) => false,
        PromptOverlayRenderedRow::Source(source) => {
            selected_reference_id == Some(source.reference_id.as_str())
        }
    })
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
    usize::from(body_height.saturating_sub(PROMPT_OVERLAY_PANE_TITLE_ROWS)).max(1)
}

fn prompt_overlay_inactive_visible_rows(height: u16) -> usize {
    let chrome = fullscreen_list_chrome_rects(Rect::new(0, 0, 1, height));
    let body_height = chrome.map(|rects| rects.body.height).unwrap_or_default();
    usize::from(
        body_height
            .saturating_sub(PROMPT_OVERLAY_PANE_TITLE_ROWS)
            .saturating_sub(PROMPT_OVERLAY_RIGHT_TAB_ROWS),
    )
    .max(1)
}

fn vertical_rule_lines(
    height: usize,
    palette: crate::theme::TerminalPalette,
) -> Vec<Line<'static>> {
    (0..height)
        .map(|_| Line::styled("│", tertiary_text_style(palette)))
        .collect()
}

fn prompt_overlay_footer_hint(width: u16) -> &'static str {
    if width < 88 {
        PROMPT_OVERLAY_FOOTER_COMPACT
    } else {
        PROMPT_OVERLAY_FOOTER_FULL
    }
}

fn prompt_scope_from_origin(origin: PromptSourceOrigin) -> Option<PromptAssemblyScope> {
    match origin {
        PromptSourceOrigin::Builtin => None,
        PromptSourceOrigin::Global => Some(PromptAssemblyScope::Global),
        PromptSourceOrigin::Project => Some(PromptAssemblyScope::Project),
    }
}
