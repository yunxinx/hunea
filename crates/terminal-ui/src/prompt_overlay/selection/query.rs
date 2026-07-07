use super::*;

impl Model {
    pub(in crate::prompt_overlay) fn selected_prompt_overlay_source(
        &self,
    ) -> Option<ResolvedPromptSource> {
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

    pub(in crate::prompt_overlay) fn selected_prompt_overlay_managed_source(
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

    pub(in crate::prompt_overlay) fn selected_prompt_overlay_left_row(
        &self,
    ) -> Option<PromptOverlayLeftRow> {
        let state = self.prompt_overlay.as_ref()?;
        if state.focus != PromptOverlayFocus::Active {
            return None;
        }
        self.prompt_overlay_left_rows()
            .get(state.active_selected)
            .cloned()
    }

    pub(in crate::prompt_overlay) fn selected_prompt_overlay_inactive_row(
        &self,
    ) -> Option<PromptOverlayInactiveRow> {
        let state = self.prompt_overlay.as_ref()?;
        if state.focus != PromptOverlayFocus::Inactive {
            return None;
        }
        self.prompt_overlay_inactive_rows(state.inactive_tab)
            .get(state.inactive_selected)
            .cloned()
    }

    pub(in crate::prompt_overlay) fn selected_prompt_overlay_selection(
        &self,
    ) -> Option<PromptOverlaySelection> {
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

    pub(in crate::prompt_overlay) fn manager_source_for_resolved_source(
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
}
