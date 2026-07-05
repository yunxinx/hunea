use std::{fs, path::Path};

use runtime_domain::prompt_assembly::persistence::PromptAssemblyScope;
use runtime_domain::prompt_assembly::{
    PromptAssemblyEditorTarget, PromptAssemblyMutation, PromptSourceKind, PromptSourceStatus,
    ResolvedPromptSource,
};

use crate::{AppEffect, Model, toast::ToastSeverity};

use super::{
    PromptOverlaySelection, render_cells::prompt_scope_from_origin,
    state::PromptOverlayPendingEditor,
};

impl Model {
    pub(crate) fn apply_prompt_overlay_external_editor_finished(
        &mut self,
        draft_path: &Path,
        failed: bool,
    ) -> Option<Option<AppEffect>> {
        let pending_editor = self.take_prompt_overlay_pending_editor()?;

        if failed {
            cleanup_prompt_overlay_editor_draft(draft_path, &pending_editor);
            self.show_toast(ToastSeverity::Error, "External editor failed");
            return Some(None);
        }

        let content = match fs::read_to_string(draft_path) {
            Ok(content) => content,
            Err(_) => {
                cleanup_prompt_overlay_editor_draft(draft_path, &pending_editor);
                self.show_toast(ToastSeverity::Error, "Failed to read external editor draft");
                return Some(None);
            }
        };
        cleanup_prompt_overlay_editor_draft(draft_path, &pending_editor);

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
                    PromptSourceKind::InstructionsFile => {
                        let backing_file_path = manager_source.backing_file_path?;
                        let initial_content = manager_source.body.unwrap_or_default();
                        let launch = self.prepare_external_editor_launch_for_path(
                            backing_file_path.clone(),
                            &initial_content,
                        )?;
                        self.set_prompt_overlay_pending_editor(PromptOverlayPendingEditor {
                            target: PromptAssemblyEditorTarget::InstructionsFile {
                                path: backing_file_path,
                            },
                            original_draft: initial_content,
                            cleanup_path_after_finish: false,
                        });
                        return Some(AppEffect::LaunchExternalEditor(launch));
                    }
                    PromptSourceKind::SkillDiscovery => {
                        let scope = source.scope?;
                        (
                            PromptAssemblyEditorTarget::SkillDiscovery { scope },
                            self.skill_discovery_editor_body_for_scope(scope),
                        )
                    }
                    PromptSourceKind::ToolGuidelines => {
                        let scope = source.scope?;
                        (
                            PromptAssemblyEditorTarget::ToolGuidelines { scope },
                            self.tool_guidelines_editor_body(),
                        )
                    }
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
                    PromptSourceKind::LongLivedSkill
                    | PromptSourceKind::DynamicEnvironmentBaseline
                    | PromptSourceKind::DynamicEnvironmentChanges => return None,
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
                    | PromptSourceKind::InstructionsFile
                    | PromptSourceKind::SkillDiscovery
                    | PromptSourceKind::LongLivedSkill
                    | PromptSourceKind::ToolGuidelines
                    | PromptSourceKind::DynamicEnvironmentBaseline
                    | PromptSourceKind::DynamicEnvironmentChanges => return None,
                }
            }
            PromptOverlaySelection::DiscoveredSkill(_)
            | PromptOverlaySelection::ToolCandidate(_)
            | PromptOverlaySelection::DynamicEnvironmentCandidate(_) => return None,
        };

        let launch = self.prepare_external_editor_launch_for_content(&initial_content)?;
        self.set_prompt_overlay_pending_editor(PromptOverlayPendingEditor {
            target,
            original_draft: initial_content,
            cleanup_path_after_finish: true,
        });
        Some(AppEffect::LaunchExternalEditor(launch))
    }

    fn set_prompt_overlay_pending_editor(&mut self, pending_editor: PromptOverlayPendingEditor) {
        if let Some(state) = self.prompt_overlay.as_mut() {
            state.pending_editor = Some(pending_editor);
        }
    }

    fn take_prompt_overlay_pending_editor(&mut self) -> Option<PromptOverlayPendingEditor> {
        self.prompt_overlay
            .as_mut()
            .and_then(|state| state.pending_editor.take())
    }
}

fn cleanup_prompt_overlay_editor_draft(
    draft_path: &Path,
    pending_editor: &PromptOverlayPendingEditor,
) {
    if pending_editor.cleanup_path_after_finish {
        let _ = fs::remove_file(draft_path);
    }
}

fn normalize_prompt_overlay_external_editor_draft(content: &str) -> String {
    content.replace("\r\n", "\n").replace('\r', "\n")
}
