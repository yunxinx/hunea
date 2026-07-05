mod dialog;
mod editor;
mod input;
mod preview;
mod render;
mod render_cells;
mod render_rows;
mod render_support;
mod selection;
mod state;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton};
use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Clear, Paragraph, Widget},
};
use runtime_domain::dynamic_environment::DynamicEnvironmentSnapshotKind;
use runtime_domain::prompt_assembly::persistence::PromptAssemblyScope;
use runtime_domain::prompt_assembly::{
    PromptAssemblyDiscoveredSkill, PromptAssemblyDynamicEnvironmentCandidate,
    PromptAssemblyExtraPromptCandidate, PromptAssemblyManagedSource, PromptAssemblyManagerSource,
    PromptAssemblyMoveDirection, PromptAssemblyMutation, PromptAssemblyToolCandidate,
    PromptSourceKind, PromptSourceOrigin, PromptSourceStatus, ResolvedPromptSource,
    SKILL_DISCOVERY_GENERATED_END, SKILL_DISCOVERY_GENERATED_START, TOOL_GUIDELINES_GENERATED_END,
    TOOL_GUIDELINES_GENERATED_START, default_extra_prompt_body, next_default_extra_prompt_title,
};
use runtime_domain::text::natural_sort_text_cmp;

use crate::{
    AppEffect, Model,
    display_width::display_width,
    fullscreen_list_chrome::fullscreen_list_chrome_rects,
    list_selection::{ListNavigationDirection, VisibleWindowSelection},
    overlay_input_result::OverlayInputResult,
    relative_age::left_pad_display_width,
    render_frame::RenderFrame,
    shortcut_help_popover::{ShortcutHelpEntry, ShortcutHelpPopover, aligned_shortcut_help_lines},
    status_line::truncate_display_width_with_ellipsis,
    styled_text::render_line_with_full_width_background,
    theme::{
        build_labeled_rule, command_accent_text_style, panel_block, primary_text_style,
        secondary_text_style, subtle_rule_line, surface_text_style, table_header_text_style,
        tertiary_text_style,
    },
};
use dialog::PromptOverlayDialog;
use render_cells::*;
use render_rows::*;
use render_support::*;
use state::PromptOverlayExpandedRow;
#[cfg(test)]
pub(crate) use state::PromptOverlayPendingEditor;
pub(crate) use state::{PromptOverlayFocus, PromptOverlayInactiveTab, PromptOverlayState};

#[cfg(test)]
mod tests;

const PROMPT_OVERLAY_HEADER_INSET: usize = 2;
const PROMPT_OVERLAY_HEADER_TRAILING_PADDING: usize = 2;
const PROMPT_OVERLAY_ROW_PREFIX_WIDTH: usize = 1;
const PROMPT_OVERLAY_COLUMN_GAP: usize = 2;
const PROMPT_OVERLAY_OUTER_PADDING: usize = 2;
const PROMPT_OVERLAY_LEFT_SEL_WIDTH: usize = 3;
const PROMPT_OVERLAY_LEFT_ORD_WIDTH: usize = 3;
const PROMPT_OVERLAY_RIGHT_ORD_WIDTH: usize = 3;
const PROMPT_OVERLAY_DYNAMIC_CHECKBOX_WIDTH: usize = "Change".len();
const PROMPT_OVERLAY_LEFT_KIND_WIDTH: usize = "instructions".len();
const PROMPT_OVERLAY_LEFT_SCOPE_WIDTH: usize = 7;
const PROMPT_OVERLAY_RIGHT_SCOPE_WIDTH: usize = 7;
const PROMPT_OVERLAY_SCOPE_TRAILING_PADDING: usize = 2;
const PROMPT_OVERLAY_LEFT_PANE_RATIO_NUMERATOR: u32 = 9;
const PROMPT_OVERLAY_RIGHT_PANE_RATIO_NUMERATOR: u32 = 11;
const PROMPT_OVERLAY_PANE_RATIO_DENOMINATOR: u32 = 20;
const PROMPT_OVERLAY_FOOTER_MORE_LABEL: &str = "? more";

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
    ToolCandidate {
        tool: PromptAssemblyToolCandidate,
    },
    DynamicEnvironmentCandidate {
        source: PromptAssemblyDynamicEnvironmentCandidate,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum PromptOverlaySelection {
    ManagedSource(PromptAssemblyManagedSource),
    ResolvedSource(ResolvedPromptSource),
    ExtraPromptCandidate(PromptAssemblyExtraPromptCandidate),
    DiscoveredSkill(PromptAssemblyDiscoveredSkill),
    ToolCandidate(PromptAssemblyToolCandidate),
    DynamicEnvironmentCandidate(PromptAssemblyDynamicEnvironmentCandidate),
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
        self.present_pending_prompt_assembly_notice_if_ready();
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
            PromptOverlaySelection::ToolCandidate(tool) => {
                let body = tool.prompt_guidelines.unwrap_or_default();
                self.open_prompt_overlay_plain_text_preview(
                    tool.label.unwrap_or(tool.name),
                    &body,
                    None,
                );
            }
            PromptOverlaySelection::DynamicEnvironmentCandidate(source) => {
                self.open_prompt_overlay_dynamic_environment_preview(source);
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

    fn open_prompt_overlay_dynamic_environment_preview(
        &mut self,
        source: PromptAssemblyDynamicEnvironmentCandidate,
    ) {
        let snapshot_kind = self.prompt_overlay_dynamic_selected_snapshot_kind();
        let title = match snapshot_kind {
            DynamicEnvironmentSnapshotKind::Baseline => {
                format!("{} · Baseline", source.label)
            }
            DynamicEnvironmentSnapshotKind::Changes => {
                format!("{} · Changes", source.label)
            }
        };
        let content = match snapshot_kind {
            DynamicEnvironmentSnapshotKind::Baseline => source.baseline_preview_body,
            DynamicEnvironmentSnapshotKind::Changes => source.changes_preview_body,
        };
        self.open_prompt_overlay_plain_text_preview(title, &content, None);
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

    fn remove_selected_prompt_source(&mut self) -> Option<AppEffect> {
        match self.selected_prompt_overlay_selection()? {
            PromptOverlaySelection::ManagedSource(selected) => {
                if matches!(
                    selected.kind,
                    PromptSourceKind::CoreSystemPrompt
                        | PromptSourceKind::InstructionsFile
                        | PromptSourceKind::DynamicEnvironmentBaseline
                        | PromptSourceKind::DynamicEnvironmentChanges
                ) {
                    return None;
                }
                Some(AppEffect::MutatePromptAssembly {
                    mutation: PromptAssemblyMutation::RemovePromptSource {
                        scope: selected.scope?,
                        kind: selected.kind,
                        reference_id: selected.reference_id,
                    },
                })
            }
            PromptOverlaySelection::ExtraPromptCandidate(candidate) => {
                let scope = prompt_scope_from_origin(candidate.origin)?;
                self.open_delete_extra_prompt_confirmation(
                    scope,
                    candidate.reference_id,
                    candidate.title,
                );
                None
            }
            PromptOverlaySelection::ResolvedSource(selected) => {
                if matches!(
                    selected.kind,
                    PromptSourceKind::CoreSystemPrompt
                        | PromptSourceKind::InstructionsFile
                        | PromptSourceKind::DynamicEnvironmentBaseline
                        | PromptSourceKind::DynamicEnvironmentChanges
                ) {
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
            PromptOverlaySelection::ToolCandidate(_) => None,
            PromptOverlaySelection::DynamicEnvironmentCandidate(_) => None,
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

    fn reset_selected_discovered_skill_order(&mut self) -> Option<AppEffect> {
        let PromptOverlaySelection::DiscoveredSkill(skill) =
            self.selected_prompt_overlay_selection()?
        else {
            return None;
        };
        Some(AppEffect::MutatePromptAssembly {
            mutation: PromptAssemblyMutation::ResetDiscoveredSkillOrder {
                scope: skill.selection_scope,
            },
        })
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
}
