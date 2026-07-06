use super::*;

pub(in crate::prompt_assembly) fn apply_save_editor_target(
    work_dir: &Path,
    global_state: &mut PromptAssemblyScopeState,
    project_state: &mut PromptAssemblyScopeState,
    target: PromptAssemblyEditorTarget,
    content: String,
) -> Result<()> {
    match target {
        PromptAssemblyEditorTarget::CoreSystemOverride { scope } => {
            let trimmed = content.trim();
            scope_state_mut(global_state, project_state, scope)
                .set_core_system_override((!trimmed.is_empty()).then_some(content));
            Ok(())
        }
        PromptAssemblyEditorTarget::SkillDiscovery { scope } => {
            let state = prompt_source_scope_state_mut(
                global_state,
                project_state,
                scope,
                PromptSourceKind::SkillDiscovery,
            );
            state.set_skill_discovery_override(Some(content));
            ensure_skill_discovery_entry_exists(state);
            Ok(())
        }
        PromptAssemblyEditorTarget::ToolGuidelines { scope } => {
            let state = prompt_source_scope_state_mut(
                global_state,
                project_state,
                scope,
                PromptSourceKind::ToolGuidelines,
            );
            state.set_tool_guidelines_override(Some(content));
            ensure_tool_guidelines_entry_exists(state);
            Ok(())
        }
        PromptAssemblyEditorTarget::InstructionsFile { path } => {
            fs::write(&path, content)
                .wrap_err_with(|| format!("write instructions file {}", path.display()))?;
            Ok(())
        }
        PromptAssemblyEditorTarget::ExtraPrompt {
            scope,
            reference_id,
        } => {
            let state = scope_state_mut(global_state, project_state, scope);
            let title = derive_extra_prompt_title(&content, &reference_id);
            if let Some(entry) = state.entry_mut(PromptSourceKind::ExtraPrompt, &reference_id) {
                entry.title = title.clone();
            } else {
                let requested_order = next_requested_order(state.entries());
                state.upsert_entry(PersistedPromptAssemblyEntry {
                    reference_id: reference_id.clone(),
                    kind: PromptSourceKind::ExtraPrompt,
                    title: title.clone(),
                    enabled: true,
                    requested_order: Some(requested_order),
                });
            }

            if let Some(prompt) = state.extra_prompt_mut(&reference_id) {
                prompt.title = title;
                prompt.body = content;
            } else {
                state.upsert_extra_prompt(StoredPromptBody {
                    reference_id,
                    title,
                    body: content,
                });
            }
            Ok(())
        }
        PromptAssemblyEditorTarget::SkillFile { skill_name, origin } => {
            let discovered = discover_skills(work_dir, None);
            let skill = discovered
                .iter()
                .find(|skill| skill.name == skill_name && skill.origin == origin)
                .ok_or_else(|| color_eyre::eyre::eyre!("skill file `{skill_name}` is missing"))?;
            fs::write(&skill.skill_path, content)
                .wrap_err_with(|| format!("write skill file {}", skill.skill_path.display()))?;
            Ok(())
        }
    }
}

pub(in crate::prompt_assembly) fn resolved_core_system_body(
    global_state: &PromptAssemblyScopeState,
    project_state: &PromptAssemblyScopeState,
) -> String {
    project_state
        .core_system_override()
        .or(global_state.core_system_override())
        .unwrap_or(BUILTIN_CORE_SYSTEM_PROMPT)
        .trim()
        .to_string()
}

pub(in crate::prompt_assembly) fn resolve_core_system_origin(
    global_state: &PromptAssemblyScopeState,
    project_state: &PromptAssemblyScopeState,
) -> PromptSourceOrigin {
    if project_state.core_system_override().is_some() {
        PromptSourceOrigin::Project
    } else if global_state.core_system_override().is_some() {
        PromptSourceOrigin::Global
    } else {
        PromptSourceOrigin::Builtin
    }
}
