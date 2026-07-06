use super::*;

impl Model {
    pub(in crate::prompt_overlay) fn prompt_overlay_left_rows(&self) -> Vec<PromptOverlayLeftRow> {
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

    pub(in crate::prompt_overlay) fn prompt_overlay_inactive_rows(
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

    pub(in crate::prompt_overlay) fn prompt_overlay_extra_rows(
        &self,
    ) -> Vec<PromptOverlayInactiveRow> {
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

    pub(in crate::prompt_overlay) fn prompt_overlay_skill_rows(
        &self,
    ) -> Vec<PromptOverlayInactiveRow> {
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
                (Some(left_winner), Some(right_winner)) => (!left_winner.selection.can_select())
                    .cmp(&!right_winner.selection.can_select())
                    .then_with(|| {
                        left_winner
                            .selection
                            .selected_order()
                            .unwrap_or(usize::MAX)
                            .cmp(
                                &right_winner
                                    .selection
                                    .selected_order()
                                    .unwrap_or(usize::MAX),
                            )
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

    pub(in crate::prompt_overlay) fn prompt_overlay_tool_rows(
        &self,
    ) -> Vec<PromptOverlayInactiveRow> {
        self.prompt_assembly
            .candidates
            .tools
            .iter()
            .cloned()
            .map(|tool| PromptOverlayInactiveRow::ToolCandidate { tool })
            .collect()
    }

    pub(in crate::prompt_overlay) fn prompt_overlay_dynamic_rows(
        &self,
    ) -> Vec<PromptOverlayInactiveRow> {
        self.prompt_assembly
            .candidates
            .dynamic_environment
            .iter()
            .cloned()
            .map(|source| PromptOverlayInactiveRow::DynamicEnvironmentCandidate { source })
            .collect()
    }

    pub(in crate::prompt_overlay) fn prompt_overlay_managed_status_for(
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

    pub(in crate::prompt_overlay) fn prompt_overlay_shadowed_sources_for(
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
}
