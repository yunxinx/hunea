use super::*;

pub(in crate::prompt_assembly) fn apply_mutation_to_scope_states(
    work_dir: &Path,
    global_state: &mut PromptAssemblyScopeState,
    project_state: &mut PromptAssemblyScopeState,
    mutation: PromptAssemblyMutation,
    tool_definitions: &[ToolDefinition],
) -> Result<()> {
    match mutation {
        PromptAssemblyMutation::SaveEditorTarget { target, content } => {
            apply_save_editor_target(work_dir, global_state, project_state, target, content)
        }
        PromptAssemblyMutation::SetDynamicEnvironmentSourceSelected {
            snapshot_kind,
            source_kind,
            selected,
        } => {
            ensure_dynamic_environment_selection_state_materialized(
                global_state,
                project_state,
                PromptAssemblyScope::Global,
            );
            set_dynamic_environment_source_selected(
                scope_state_mut(global_state, project_state, PromptAssemblyScope::Global),
                snapshot_kind,
                source_kind,
                selected,
            );
            Ok(())
        }
        PromptAssemblyMutation::Scoped(scoped) => apply_scoped_mutation_to_scope_states(
            work_dir,
            global_state,
            project_state,
            scoped,
            tool_definitions,
        ),
    }
}

fn apply_scoped_mutation_to_scope_states(
    work_dir: &Path,
    global_state: &mut PromptAssemblyScopeState,
    project_state: &mut PromptAssemblyScopeState,
    mutation: PromptAssemblyScopedMutation,
    tool_definitions: &[ToolDefinition],
) -> Result<()> {
    let PromptAssemblyScopedMutation { scope, kind } = mutation;
    match kind {
        PromptAssemblyScopedMutationKind::SetExtraPromptSelected {
            reference_id,
            selected,
        } => {
            set_extra_prompt_selected(global_state, project_state, scope, &reference_id, selected);
            Ok(())
        }
        PromptAssemblyScopedMutationKind::SetPromptSourceEnabled {
            kind,
            reference_id,
            enabled,
        } => {
            ensure_prompt_source_entry_materialized(
                work_dir,
                global_state,
                project_state,
                scope,
                kind,
                &reference_id,
            );
            let state = prompt_source_scope_state_mut(global_state, project_state, scope, kind);
            set_prompt_source_enabled(state, kind, &reference_id, enabled);
            Ok(())
        }
        PromptAssemblyScopedMutationKind::SetDiscoveredSkillSelected {
            skill_name,
            selected,
        } => {
            ensure_skill_discovery_selection_state_materialized(
                work_dir,
                global_state,
                project_state,
                scope,
            );
            set_discovered_skill_selected(
                scope_state_mut(global_state, project_state, scope),
                &skill_name,
                selected,
            );
            Ok(())
        }
        PromptAssemblyScopedMutationKind::MoveDiscoveredSkill {
            skill_name,
            direction,
        } => {
            ensure_skill_discovery_selection_state_materialized(
                work_dir,
                global_state,
                project_state,
                scope,
            );
            move_discovered_skill(
                scope_state_mut(global_state, project_state, scope),
                &skill_name,
                direction,
            )
        }
        PromptAssemblyScopedMutationKind::ResetDiscoveredSkillOrder => {
            ensure_skill_discovery_selection_state_materialized(
                work_dir,
                global_state,
                project_state,
                scope,
            );
            let discovered_skills = effective_discovered_skills(&discover_skills(work_dir, None));
            reset_discovered_skill_order(
                scope_state_mut(global_state, project_state, scope),
                &discovered_skills,
            );
            Ok(())
        }
        PromptAssemblyScopedMutationKind::SetToolSelected {
            tool_name,
            selected,
        } => {
            if !tool_has_prompt_guidelines(tool_definitions, &tool_name) {
                return Ok(());
            }
            let scope = tool_guidelines_scope(global_state, project_state, scope);
            ensure_tool_selection_state_materialized(
                global_state,
                project_state,
                scope,
                tool_definitions,
            );
            set_tool_selected(
                scope_state_mut(global_state, project_state, scope),
                &tool_name,
                selected,
            );
            Ok(())
        }
        PromptAssemblyScopedMutationKind::MoveTool {
            tool_name,
            direction,
        } => {
            if !tool_has_prompt_guidelines(tool_definitions, &tool_name) {
                return Ok(());
            }
            let scope = tool_guidelines_scope(global_state, project_state, scope);
            ensure_tool_selection_state_materialized(
                global_state,
                project_state,
                scope,
                tool_definitions,
            );
            move_tool(
                scope_state_mut(global_state, project_state, scope),
                &tool_name,
                direction,
            )
        }
        PromptAssemblyScopedMutationKind::ActivateLongLivedSkill { skill_name } => {
            activate_long_lived_skill(global_state, project_state, scope, &skill_name);
            Ok(())
        }
        PromptAssemblyScopedMutationKind::CreateExtraPrompt { content } => {
            let state = scope_state_mut(global_state, project_state, scope);
            let title = derive_extra_prompt_title(&content, "New prompt");
            let reference_id = generate_unique_extra_prompt_reference_id(state, &title);
            let requested_order = next_requested_order(state.entries());
            state.upsert_entry(PersistedPromptAssemblyEntry {
                reference_id: reference_id.clone(),
                kind: PromptSourceKind::ExtraPrompt,
                title: title.clone(),
                enabled: true,
                requested_order: Some(requested_order),
            });
            state.upsert_extra_prompt(StoredPromptBody {
                reference_id,
                title,
                body: content,
            });
            Ok(())
        }
        PromptAssemblyScopedMutationKind::DeleteExtraPrompt { reference_id } => {
            let state = scope_state_mut(global_state, project_state, scope);
            state.remove_entry(PromptSourceKind::ExtraPrompt, &reference_id);
            state.remove_extra_prompt(&reference_id);
            Ok(())
        }
        PromptAssemblyScopedMutationKind::RemovePromptSource { kind, reference_id } => {
            ensure_prompt_source_entry_materialized(
                work_dir,
                global_state,
                project_state,
                scope,
                kind,
                &reference_id,
            );
            remove_prompt_source(
                prompt_source_scope_state_mut(global_state, project_state, scope, kind),
                kind,
                &reference_id,
            );
            Ok(())
        }
        PromptAssemblyScopedMutationKind::MoveActiveSource {
            kind,
            reference_id,
            direction,
        } => {
            ensure_active_prompt_source_ordering_materialized(
                work_dir,
                global_state,
                project_state,
                scope,
                kind,
                &reference_id,
            );
            move_active_source(
                global_state,
                project_state,
                scope,
                kind,
                &reference_id,
                direction,
            )
        }
        PromptAssemblyScopedMutationKind::RestoreCoreSystemOverride => {
            scope_state_mut(global_state, project_state, scope).set_core_system_override(None);
            Ok(())
        }
    }
}
