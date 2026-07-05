use super::*;

impl Model {
    pub(super) fn selected_prompt_overlay_source(&self) -> Option<ResolvedPromptSource> {
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
            | PromptOverlaySelection::DiscoveredSkill(_)
            | PromptOverlaySelection::ToolCandidate(_)
            | PromptOverlaySelection::DynamicEnvironmentCandidate(_) => None,
        }
    }

    pub(super) fn selected_prompt_overlay_managed_source(
        &self,
    ) -> Option<PromptAssemblyManagedSource> {
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

    pub(super) fn selected_prompt_overlay_left_row(&self) -> Option<PromptOverlayLeftRow> {
        let state = self.prompt_overlay.as_ref()?;
        if state.focus != PromptOverlayFocus::Active {
            return None;
        }
        self.prompt_overlay_left_rows()
            .get(state.active_selected)
            .cloned()
    }

    pub(super) fn selected_prompt_overlay_inactive_row(&self) -> Option<PromptOverlayInactiveRow> {
        let state = self.prompt_overlay.as_ref()?;
        if state.focus != PromptOverlayFocus::Inactive {
            return None;
        }
        self.prompt_overlay_inactive_rows(state.inactive_tab)
            .get(state.inactive_selected)
            .cloned()
    }

    pub(super) fn selected_prompt_overlay_selection(&self) -> Option<PromptOverlaySelection> {
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
                PromptOverlayInactiveRow::ToolCandidate { tool } => {
                    Some(PromptOverlaySelection::ToolCandidate(tool))
                }
                PromptOverlayInactiveRow::DynamicEnvironmentCandidate { source } => {
                    Some(PromptOverlaySelection::DynamicEnvironmentCandidate(source))
                }
            },
        }
    }

    pub(super) fn manager_source_for_resolved_source(
        &self,
        selected: &ResolvedPromptSource,
    ) -> Option<PromptAssemblyManagerSource> {
        self.prompt_assembly
            .sources
            .preview
            .iter()
            .find(|source| {
                source.reference_id == selected.reference_id
                    && source.kind == selected.kind
                    && source.origin == selected.origin
            })
            .cloned()
    }

    pub(super) fn prompt_overlay_left_rows(&self) -> Vec<PromptOverlayLeftRow> {
        let expanded_row = self
            .prompt_overlay
            .as_ref()
            .and_then(|state| state.expanded_row.clone());

        let mut rows = Vec::new();
        for source in &self.prompt_assembly.sources.managed {
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

    pub(super) fn prompt_overlay_inactive_rows(
        &self,
        tab: PromptOverlayInactiveTab,
    ) -> Vec<PromptOverlayInactiveRow> {
        match tab {
            PromptOverlayInactiveTab::ExtraPrompts => self.prompt_overlay_extra_rows(),
            PromptOverlayInactiveTab::LongLivedSkills => self.prompt_overlay_skill_rows(),
            PromptOverlayInactiveTab::Tools => self.prompt_overlay_tool_rows(),
            PromptOverlayInactiveTab::Dynamic => self.prompt_overlay_dynamic_rows(),
        }
    }

    pub(super) fn prompt_overlay_extra_rows(&self) -> Vec<PromptOverlayInactiveRow> {
        let expanded_row = self
            .prompt_overlay
            .as_ref()
            .and_then(|state| state.expanded_row.clone());
        let mut groups = self
            .prompt_assembly
            .candidates
            .extra_prompts
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
            match (
                prompt_overlay_extra_candidate_winner(left),
                prompt_overlay_extra_candidate_winner(right),
            ) {
                (Some(left_winner), Some(right_winner)) => {
                    natural_sort_text_cmp(&left_winner.title, &right_winner.title)
                        .then_with(|| left_winner.reference_id.cmp(&right_winner.reference_id))
                }
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => std::cmp::Ordering::Equal,
            }
        });

        let mut rows = Vec::new();
        for group in groups {
            let Some((winner, shadowed)) = prompt_overlay_partition_extra_candidates(group) else {
                continue;
            };
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

    pub(super) fn prompt_overlay_skill_rows(&self) -> Vec<PromptOverlayInactiveRow> {
        let expanded_row = self
            .prompt_overlay
            .as_ref()
            .and_then(|state| state.expanded_row.clone());
        let mut groups = self
            .prompt_assembly
            .candidates
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
            match (
                prompt_overlay_discovered_skill_winner(left),
                prompt_overlay_discovered_skill_winner(right),
            ) {
                (Some(left_winner), Some(right_winner)) => (!left_winner.can_select_for_discovery)
                    .cmp(&!right_winner.can_select_for_discovery)
                    .then_with(|| {
                        left_winner
                            .selected_order
                            .unwrap_or(usize::MAX)
                            .cmp(&right_winner.selected_order.unwrap_or(usize::MAX))
                    })
                    .then_with(|| natural_sort_text_cmp(&left_winner.title, &right_winner.title))
                    .then_with(|| {
                        natural_sort_text_cmp(&left_winner.skill_name, &right_winner.skill_name)
                    }),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => std::cmp::Ordering::Equal,
            }
        });

        let mut rows = Vec::new();
        for group in groups {
            let Some((winner, shadowed)) = prompt_overlay_partition_discovered_skills(group) else {
                continue;
            };
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

    pub(super) fn prompt_overlay_tool_rows(&self) -> Vec<PromptOverlayInactiveRow> {
        self.prompt_assembly
            .candidates
            .tools
            .iter()
            .cloned()
            .map(|tool| PromptOverlayInactiveRow::ToolCandidate { tool })
            .collect()
    }

    pub(super) fn prompt_overlay_dynamic_rows(&self) -> Vec<PromptOverlayInactiveRow> {
        self.prompt_assembly
            .candidates
            .dynamic_environment
            .iter()
            .cloned()
            .map(|source| PromptOverlayInactiveRow::DynamicEnvironmentCandidate { source })
            .collect()
    }

    pub(super) fn prompt_overlay_managed_status_for(
        &self,
        source: &PromptAssemblyManagedSource,
    ) -> PromptOverlayManagedStatus {
        if self
            .prompt_assembly
            .resolution
            .assembly
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
            .resolution
            .assembly
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

    pub(super) fn prompt_overlay_shadowed_sources_for(
        &self,
        kind: PromptSourceKind,
        reference_id: &str,
    ) -> Vec<ResolvedPromptSource> {
        self.prompt_assembly
            .resolution
            .assembly
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

    pub(super) fn move_selected_active_source(
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

    pub(super) fn toggle_selected_prompt_source_enabled(&mut self) -> Option<AppEffect> {
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
                if !skill.can_select_for_discovery {
                    return None;
                }
                Some(AppEffect::MutatePromptAssembly {
                    mutation: PromptAssemblyMutation::scoped(
                        skill.selection_scope,
                        PromptAssemblyScopedMutationKind::SetDiscoveredSkillSelected {
                            skill_name: skill.skill_name,
                            selected: !skill.selected,
                        },
                    ),
                })
            }
            PromptOverlaySelection::ToolCandidate(tool) => {
                if !tool.can_select {
                    return None;
                }
                Some(AppEffect::MutatePromptAssembly {
                    mutation: PromptAssemblyMutation::scoped(
                        tool.selection_scope,
                        PromptAssemblyScopedMutationKind::SetToolSelected {
                            tool_name: tool.name,
                            selected: !tool.selected,
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

    pub(super) fn create_extra_prompt_from_overlay(
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

    pub(super) fn default_extra_prompt_body_for_scope(&self, scope: PromptAssemblyScope) -> String {
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

    pub(super) fn open_delete_extra_prompt_confirmation(
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

    pub(super) fn delete_extra_prompt_effect(
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

    pub(super) fn prompt_overlay_action_availability(&self) -> PromptOverlayActionAvailability {
        match self.selected_prompt_overlay_selection() {
            Some(PromptOverlaySelection::ManagedSource(source)) => {
                PromptOverlayActionAvailability {
                    can_edit: !matches!(
                        source.kind,
                        PromptSourceKind::LongLivedSkill
                            | PromptSourceKind::DynamicEnvironmentBaseline
                            | PromptSourceKind::DynamicEnvironmentChanges
                    ),
                    can_add_custom: false,
                    can_remove: prompt_overlay_source_kind_can_remove(source.kind),
                    can_toggle_selection: source.kind != PromptSourceKind::CoreSystemPrompt,
                    can_reorder_active: source.kind != PromptSourceKind::CoreSystemPrompt,
                }
            }
            Some(PromptOverlaySelection::ResolvedSource(source)) => {
                PromptOverlayActionAvailability {
                    can_edit: source.kind == PromptSourceKind::ExtraPrompt,
                    can_add_custom: false,
                    can_remove: prompt_overlay_source_kind_can_remove(source.kind),
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
                can_reorder_active: true,
            },
            Some(PromptOverlaySelection::ToolCandidate(_)) => PromptOverlayActionAvailability {
                can_edit: false,
                can_add_custom: false,
                can_remove: false,
                can_toggle_selection: true,
                can_reorder_active: true,
            },
            Some(PromptOverlaySelection::DynamicEnvironmentCandidate(_)) => {
                PromptOverlayActionAvailability {
                    can_edit: false,
                    can_add_custom: false,
                    can_remove: false,
                    can_toggle_selection: true,
                    can_reorder_active: false,
                }
            }
            None => PromptOverlayActionAvailability {
                can_edit: false,
                can_add_custom: self.prompt_overlay_can_add_custom(),
                can_remove: false,
                can_toggle_selection: false,
                can_reorder_active: false,
            },
        }
    }

    pub(super) fn prompt_overlay_can_add_custom(&self) -> bool {
        self.prompt_overlay.as_ref().is_some_and(|state| {
            state.focus == PromptOverlayFocus::Inactive
                && state.inactive_tab == PromptOverlayInactiveTab::ExtraPrompts
        })
    }

    pub(super) fn core_system_editor_body_for_scope(&self, scope: PromptAssemblyScope) -> String {
        match scope {
            PromptAssemblyScope::Global => self
                .prompt_assembly
                .core_system
                .global_override
                .clone()
                .unwrap_or_else(|| self.prompt_assembly.core_system.builtin_body.clone()),
            PromptAssemblyScope::Project => self
                .prompt_assembly
                .core_system
                .project_override
                .clone()
                .or_else(|| self.prompt_assembly.core_system.global_override.clone())
                .unwrap_or_else(|| self.prompt_assembly.core_system.builtin_body.clone()),
        }
    }

    pub(super) fn skill_discovery_editor_body_for_scope(
        &self,
        scope: PromptAssemblyScope,
    ) -> String {
        let origin = Some(match scope {
            PromptAssemblyScope::Global => PromptSourceOrigin::Global,
            PromptAssemblyScope::Project => PromptSourceOrigin::Project,
        });
        let body = self
            .prompt_assembly
            .sources
            .preview
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

    pub(super) fn tool_guidelines_editor_body(&self) -> String {
        let body = self
            .prompt_assembly
            .sources
            .preview
            .iter()
            .find(|source| {
                source.reference_id == "tool-guidelines"
                    && source.kind == PromptSourceKind::ToolGuidelines
            })
            .and_then(|source| source.body.clone())
            .unwrap_or_default();
        if body.is_empty() {
            return format!("{TOOL_GUIDELINES_GENERATED_START}\n{TOOL_GUIDELINES_GENERATED_END}\n");
        }
        if body.contains(TOOL_GUIDELINES_GENERATED_START)
            && body.contains(TOOL_GUIDELINES_GENERATED_END)
        {
            return body;
        }
        format!("{TOOL_GUIDELINES_GENERATED_START}\n{body}\n{TOOL_GUIDELINES_GENERATED_END}\n")
    }
}
