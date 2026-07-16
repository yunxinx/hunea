use super::*;

impl Model {
    pub(in crate::prompt_overlay) fn prompt_overlay_action_availability(
        &self,
    ) -> PromptOverlayActionAvailability {
        match self.selected_prompt_overlay_selection() {
            Some(PromptOverlaySelection::ManagedSource(source)) => {
                PromptOverlayActionAvailability::PromptSource {
                    can_edit: !matches!(
                        source.kind,
                        PromptSourceKind::LongLivedSkill
                            | PromptSourceKind::DynamicEnvironmentBaseline
                            | PromptSourceKind::DynamicEnvironmentChanges
                    ),
                    can_remove: prompt_overlay_source_kind_can_remove(source.kind),
                    can_toggle_selection: source.kind != PromptSourceKind::CoreSystemPrompt,
                    can_reorder_active: source.kind != PromptSourceKind::CoreSystemPrompt,
                }
            }
            Some(PromptOverlaySelection::ResolvedSource(source)) => {
                PromptOverlayActionAvailability::PromptSource {
                    can_edit: source.kind == PromptSourceKind::ExtraPrompt,
                    can_remove: prompt_overlay_source_kind_can_remove(source.kind),
                    can_toggle_selection: source.kind != PromptSourceKind::CoreSystemPrompt,
                    can_reorder_active: false,
                }
            }
            Some(PromptOverlaySelection::ExtraPromptCandidate(_)) => {
                PromptOverlayActionAvailability::ExtraPromptCandidate {
                    can_add_custom: self.prompt_overlay_can_add_custom(),
                }
            }
            Some(PromptOverlaySelection::DiscoveredSkill(_)) => {
                PromptOverlayActionAvailability::SelectableCandidate {
                    can_reorder_active: true,
                }
            }
            Some(PromptOverlaySelection::ToolCandidate(tool)) => {
                PromptOverlayActionAvailability::SelectableCandidate {
                    // 排序只作用于 guidelines 注入顺序；禁用的工具 guidelines 不注入，
                    // 与 Guide 列不可操作的语义保持一致，同样不可排序。
                    can_reorder_active: tool.tool_enabled && tool.selection.can_select(),
                }
            }
            Some(PromptOverlaySelection::DynamicEnvironmentCandidate(_)) => {
                PromptOverlayActionAvailability::DynamicEnvironmentCandidate
            }
            None => PromptOverlayActionAvailability::Empty {
                can_add_custom: self.prompt_overlay_can_add_custom(),
            },
        }
    }

    pub(in crate::prompt_overlay) fn prompt_overlay_can_add_custom(&self) -> bool {
        self.prompt_overlay.as_ref().is_some_and(|state| {
            state.focus == PromptOverlayFocus::Inactive
                && state.inactive_tab == PromptOverlayInactiveTab::ExtraPrompts
        })
    }
}
