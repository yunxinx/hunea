use super::*;

impl Model {
    pub(in crate::prompt_overlay) fn move_selected_active_source(
        &mut self,
        direction: PromptAssemblyMoveDirection,
    ) -> Option<AppEffect> {
        if let Some(PromptOverlaySelection::DiscoveredSkill(skill)) =
            self.selected_prompt_overlay_selection()
        {
            return Some(AppEffect::MutatePromptAssembly {
                mutation: PromptAssemblyMutation::scoped(
                    skill.selection_scope,
                    PromptAssemblyScopedMutationKind::MoveDiscoveredSkill {
                        skill_name: skill.skill_name,
                        direction,
                    },
                ),
            });
        }
        if let Some(PromptOverlaySelection::ToolCandidate(tool)) =
            self.selected_prompt_overlay_selection()
        {
            return Some(AppEffect::MutatePromptAssembly {
                mutation: PromptAssemblyMutation::scoped(
                    tool.selection_scope,
                    PromptAssemblyScopedMutationKind::MoveTool {
                        tool_name: tool.name,
                        direction,
                    },
                ),
            });
        }
        let selected = self.selected_prompt_overlay_managed_source()?;
        if selected.kind == PromptSourceKind::CoreSystemPrompt {
            return None;
        }
        Some(AppEffect::MutatePromptAssembly {
            mutation: PromptAssemblyMutation::scoped(
                selected.scope?,
                PromptAssemblyScopedMutationKind::MoveActiveSource {
                    kind: selected.kind,
                    reference_id: selected.reference_id,
                    direction,
                },
            ),
        })
    }

    pub(in crate::prompt_overlay) fn toggle_selected_prompt_source_enabled(
        &mut self,
    ) -> Option<AppEffect> {
        if let Some(selected) = self.selected_prompt_overlay_managed_source() {
            if selected.kind == PromptSourceKind::CoreSystemPrompt {
                return None;
            }
            return Some(AppEffect::MutatePromptAssembly {
                mutation: PromptAssemblyMutation::scoped(
                    selected.scope?,
                    PromptAssemblyScopedMutationKind::SetPromptSourceEnabled {
                        kind: selected.kind,
                        reference_id: selected.reference_id,
                        enabled: !selected.enabled,
                    },
                ),
            });
        }

        match self.selected_prompt_overlay_selection()? {
            PromptOverlaySelection::ManagedSource(_) => None,
            PromptOverlaySelection::ResolvedSource(selected) => {
                if selected.kind == PromptSourceKind::CoreSystemPrompt {
                    return None;
                }
                Some(AppEffect::MutatePromptAssembly {
                    mutation: PromptAssemblyMutation::scoped(
                        prompt_scope_from_origin(selected.origin?)?,
                        PromptAssemblyScopedMutationKind::SetPromptSourceEnabled {
                            kind: selected.kind,
                            reference_id: selected.reference_id,
                            enabled: false,
                        },
                    ),
                })
            }
            PromptOverlaySelection::ExtraPromptCandidate(candidate) => {
                Some(AppEffect::MutatePromptAssembly {
                    mutation: PromptAssemblyMutation::scoped(
                        prompt_scope_from_origin(candidate.origin)?,
                        PromptAssemblyScopedMutationKind::SetExtraPromptSelected {
                            reference_id: candidate.reference_id,
                            selected: !candidate.selected,
                        },
                    ),
                })
            }
            PromptOverlaySelection::DiscoveredSkill(skill) => {
                if !skill.selection.can_select() {
                    return None;
                }
                Some(AppEffect::MutatePromptAssembly {
                    mutation: PromptAssemblyMutation::scoped(
                        skill.selection_scope,
                        PromptAssemblyScopedMutationKind::SetDiscoveredSkillSelected {
                            skill_name: skill.skill_name,
                            selected: !skill.selection.is_selected(),
                        },
                    ),
                })
            }
            PromptOverlaySelection::ToolCandidate(tool) => {
                if !tool.selection.can_select() {
                    return None;
                }
                Some(AppEffect::MutatePromptAssembly {
                    mutation: PromptAssemblyMutation::scoped(
                        tool.selection_scope,
                        PromptAssemblyScopedMutationKind::SetToolSelected {
                            tool_name: tool.name,
                            selected: !tool.selection.is_selected(),
                        },
                    ),
                })
            }
            PromptOverlaySelection::DynamicEnvironmentCandidate(source) => {
                let snapshot_kind = self.prompt_overlay_dynamic_selected_snapshot_kind();
                Some(AppEffect::MutatePromptAssembly {
                    mutation: PromptAssemblyMutation::SetDynamicEnvironmentSourceSelected {
                        snapshot_kind,
                        source_kind: source.source_kind,
                        selected: !prompt_overlay_dynamic_source_selected(&source, snapshot_kind),
                    },
                })
            }
        }
    }

    pub(in crate::prompt_overlay) fn create_extra_prompt_from_overlay(
        &self,
        scope: PromptAssemblyScope,
    ) -> Option<AppEffect> {
        let state = self.prompt_overlay.as_ref()?;
        if state.focus != PromptOverlayFocus::Inactive
            || state.inactive_tab != PromptOverlayInactiveTab::ExtraPrompts
        {
            return None;
        }
        let content = self.default_extra_prompt_body_for_scope(scope);
        Some(AppEffect::MutatePromptAssembly {
            mutation: PromptAssemblyMutation::scoped(
                scope,
                PromptAssemblyScopedMutationKind::CreateExtraPrompt { content },
            ),
        })
    }

    pub(in crate::prompt_overlay) fn default_extra_prompt_body_for_scope(
        &self,
        scope: PromptAssemblyScope,
    ) -> String {
        let title = next_default_extra_prompt_title(
            self.prompt_assembly
                .sources
                .managed
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
                        .candidates
                        .extra_prompts
                        .iter()
                        .filter(|candidate| {
                            prompt_scope_from_origin(candidate.origin)
                                .is_some_and(|origin_scope| origin_scope == scope)
                        })
                        .map(|candidate| candidate.title.as_str()),
                ),
        );
        default_extra_prompt_body(&title)
    }

    pub(in crate::prompt_overlay) fn open_delete_extra_prompt_confirmation(
        &mut self,
        scope: PromptAssemblyScope,
        reference_id: String,
        title: String,
    ) {
        let Some(state) = self.prompt_overlay.as_mut() else {
            return;
        };
        state.dialog = Some(PromptOverlayDialog::ConfirmDeleteExtraPrompt {
            scope,
            reference_id,
            title,
        });
    }

    pub(in crate::prompt_overlay) fn delete_extra_prompt_effect(
        &self,
        scope: PromptAssemblyScope,
        reference_id: String,
    ) -> AppEffect {
        AppEffect::MutatePromptAssembly {
            mutation: PromptAssemblyMutation::scoped(
                scope,
                PromptAssemblyScopedMutationKind::DeleteExtraPrompt { reference_id },
            ),
        }
    }
}
