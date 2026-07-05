use super::*;

impl Model {
    pub(super) fn toggle_prompt_overlay_expanded_row(&mut self) {
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

    pub(super) fn remove_selected_prompt_source(&mut self) -> Option<AppEffect> {
        match self.selected_prompt_overlay_selection()? {
            PromptOverlaySelection::ManagedSource(selected) => {
                if prompt_overlay_source_kind_is_protected_from_remove(selected.kind) {
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
                if prompt_overlay_source_kind_is_protected_from_remove(selected.kind) {
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
            PromptOverlaySelection::DiscoveredSkill(_)
            | PromptOverlaySelection::ToolCandidate(_)
            | PromptOverlaySelection::DynamicEnvironmentCandidate(_) => None,
        }
    }

    pub(super) fn restore_selected_core_system_override(&mut self) -> Option<AppEffect> {
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

    pub(super) fn reset_selected_discovered_skill_order(&mut self) -> Option<AppEffect> {
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
}

fn prompt_overlay_source_kind_is_protected_from_remove(kind: PromptSourceKind) -> bool {
    matches!(
        kind,
        PromptSourceKind::CoreSystemPrompt
            | PromptSourceKind::InstructionsFile
            | PromptSourceKind::DynamicEnvironmentBaseline
            | PromptSourceKind::DynamicEnvironmentChanges
    )
}
