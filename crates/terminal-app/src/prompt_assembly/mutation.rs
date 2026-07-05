use super::*;

pub(super) fn apply_mutation_to_scope_states(
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
            state.entries_mut().push(PersistedPromptAssemblyEntry {
                reference_id: reference_id.clone(),
                kind: PromptSourceKind::ExtraPrompt,
                title: title.clone(),
                enabled: true,
                requested_order: Some(requested_order),
            });
            state
                .extra_prompts_mut()
                .retain(|prompt| prompt.reference_id != reference_id);
            state.extra_prompts_mut().push(StoredPromptBody {
                reference_id,
                title,
                body: content,
            });
            Ok(())
        }
        PromptAssemblyScopedMutationKind::DeleteExtraPrompt { reference_id } => {
            let state = scope_state_mut(global_state, project_state, scope);
            state.entries_mut().retain(|entry| {
                !(entry.kind == PromptSourceKind::ExtraPrompt && entry.reference_id == reference_id)
            });
            state
                .extra_prompts_mut()
                .retain(|prompt| prompt.reference_id != reference_id);
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

pub(super) fn apply_save_editor_target(
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
            if let Some(entry) = state.entries_mut().iter_mut().find(|entry| {
                entry.kind == PromptSourceKind::ExtraPrompt && entry.reference_id == reference_id
            }) {
                entry.title = title.clone();
            } else {
                let requested_order = next_requested_order(state.entries());
                state.entries_mut().push(PersistedPromptAssemblyEntry {
                    reference_id: reference_id.clone(),
                    kind: PromptSourceKind::ExtraPrompt,
                    title: title.clone(),
                    enabled: true,
                    requested_order: Some(requested_order),
                });
            }

            if let Some(prompt) = state
                .extra_prompts_mut()
                .iter_mut()
                .find(|prompt| prompt.reference_id == reference_id)
            {
                prompt.title = title;
                prompt.body = content;
            } else {
                state.extra_prompts_mut().push(StoredPromptBody {
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

pub(super) fn resolved_core_system_body(
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

pub(super) fn resolve_core_system_origin(
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

pub(super) fn materialized_sources_for_state(
    state: &PromptAssemblyScopeState,
    context: &PromptAssemblyResolutionContext<'_>,
) -> Vec<PromptAssemblyManagerSource> {
    state
        .entries()
        .iter()
        .map(|entry| PromptAssemblyManagerSource {
            reference_id: entry.reference_id.clone(),
            kind: entry.kind,
            title: entry.title.clone(),
            origin: Some(entry_origin(state.scope(), entry.kind)),
            resolved_body_origin: resolved_body_origin_for_entry(
                entry,
                context.skills_by_name,
                context.instructions_by_reference_id,
            ),
            backing_file_path: backing_file_path_for_entry(
                entry,
                context.instructions_by_reference_id,
            ),
            body: body_for_entry(entry, state.scope(), context),
        })
        .collect()
}

pub(super) fn extend_candidates(
    candidates: &mut Vec<PromptSourceCandidate>,
    candidate_bodies: &mut HashMap<String, String>,
    state: &PromptAssemblyScopeState,
    context: &PromptAssemblyResolutionContext<'_>,
) {
    for entry in state.entries() {
        let reference_id = entry.reference_id.clone();
        let origin = Some(entry_origin(state.scope(), entry.kind));
        let resolvable = resolvable_for_entry(
            entry,
            state.scope(),
            context.extra_prompt_bodies,
            context.skills_by_name,
            context.instructions_by_reference_id,
        );
        let candidate = PromptSourceCandidate {
            reference_id: reference_id.clone(),
            kind: entry.kind,
            title: entry.title.clone(),
            origin,
            collision_key: collision_key_for_entry(entry),
            state: PromptSourceCandidateState::from_materialized_source(entry.enabled, resolvable),
            requested_order: entry.requested_order,
        };
        if let Some(body) = body_for_entry(entry, state.scope(), context) {
            candidate_bodies.insert(candidate_body_key(origin, entry.kind, &reference_id), body);
        }
        candidates.push(candidate);
    }
}

pub(super) fn resolvable_for_entry(
    entry: &PersistedPromptAssemblyEntry,
    scope: PromptAssemblyScope,
    extra_prompt_bodies: &HashMap<String, String>,
    skills_by_name: &HashMap<String, DiscoveredSkill>,
    instructions_by_reference_id: &HashMap<String, DiscoveredInstructionsFile>,
) -> bool {
    match entry.kind {
        PromptSourceKind::ExtraPrompt => {
            extra_prompt_bodies.contains_key(&scope_reference_key(scope, &entry.reference_id))
        }
        PromptSourceKind::InstructionsFile => {
            instructions_by_reference_id.contains_key(&entry.reference_id)
        }
        PromptSourceKind::SkillDiscovery => true,
        PromptSourceKind::ToolGuidelines => true,
        PromptSourceKind::DynamicEnvironmentBaseline
        | PromptSourceKind::DynamicEnvironmentChanges => true,
        PromptSourceKind::LongLivedSkill => skills_by_name.contains_key(&entry.reference_id),
        PromptSourceKind::CoreSystemPrompt => true,
    }
}

pub(super) fn body_for_entry(
    entry: &PersistedPromptAssemblyEntry,
    scope: PromptAssemblyScope,
    context: &PromptAssemblyResolutionContext<'_>,
) -> Option<String> {
    match entry.kind {
        PromptSourceKind::InstructionsFile => context
            .instructions_by_reference_id
            .get(&entry.reference_id)
            .map(|file| file.body.clone()),
        PromptSourceKind::ExtraPrompt => context
            .extra_prompt_bodies
            .get(&scope_reference_key(scope, &entry.reference_id))
            .cloned(),
        PromptSourceKind::SkillDiscovery => Some(resolve_skill_discovery_body(
            scope,
            context.skill_discovery_skill_state,
            context.skills_by_name,
            context.global_state,
            context.project_state,
        )),
        PromptSourceKind::LongLivedSkill => context
            .skills_by_name
            .get(&entry.reference_id)
            .map(format_long_lived_skill_body),
        PromptSourceKind::ToolGuidelines => Some(resolve_tool_guidelines_body(
            scope,
            context.tool_selection_state,
            context.tool_definitions,
            context.global_state,
            context.project_state,
        )),
        PromptSourceKind::DynamicEnvironmentBaseline => dynamic_environment_preview_body(
            context.dynamic_environment_observations,
            DynamicEnvironmentSnapshotKind::Baseline,
            context.dynamic_environment_selection_state,
        ),
        PromptSourceKind::DynamicEnvironmentChanges => dynamic_environment_preview_body(
            context.dynamic_environment_observations,
            DynamicEnvironmentSnapshotKind::Changes,
            context.dynamic_environment_selection_state,
        ),
        PromptSourceKind::CoreSystemPrompt => None,
    }
}

pub(super) fn resolved_body_origin_for_entry(
    entry: &PersistedPromptAssemblyEntry,
    skills_by_name: &HashMap<String, DiscoveredSkill>,
    instructions_by_reference_id: &HashMap<String, DiscoveredInstructionsFile>,
) -> Option<PromptSourceOrigin> {
    match entry.kind {
        PromptSourceKind::InstructionsFile => instructions_by_reference_id
            .get(&entry.reference_id)
            .map(|file| file.origin),
        PromptSourceKind::LongLivedSkill => skills_by_name
            .get(&entry.reference_id)
            .map(|skill| skill.origin),
        _ => None,
    }
}

pub(super) fn backing_file_path_for_entry(
    entry: &PersistedPromptAssemblyEntry,
    instructions_by_reference_id: &HashMap<String, DiscoveredInstructionsFile>,
) -> Option<PathBuf> {
    (entry.kind == PromptSourceKind::InstructionsFile).then(|| {
        instructions_by_reference_id
            .get(&entry.reference_id)
            .map(|file| file.path.clone())
    })?
}

pub(super) fn collision_key_for_entry(entry: &PersistedPromptAssemblyEntry) -> Option<String> {
    match entry.kind {
        PromptSourceKind::InstructionsFile
        | PromptSourceKind::SkillDiscovery
        | PromptSourceKind::ToolGuidelines
        | PromptSourceKind::DynamicEnvironmentBaseline
        | PromptSourceKind::DynamicEnvironmentChanges => None,
        PromptSourceKind::ExtraPrompt | PromptSourceKind::LongLivedSkill => {
            Some(entry.reference_id.clone())
        }
        PromptSourceKind::CoreSystemPrompt => None,
    }
}

pub(super) fn indexed_extra_prompt_bodies(
    global_state: &PromptAssemblyScopeState,
    project_state: &PromptAssemblyScopeState,
) -> HashMap<String, String> {
    let mut bodies = HashMap::new();
    insert_extra_prompt_bodies(&mut bodies, global_state);
    insert_extra_prompt_bodies(&mut bodies, project_state);
    bodies
}

pub(super) fn insert_extra_prompt_bodies(
    indexed: &mut HashMap<String, String>,
    state: &PromptAssemblyScopeState,
) {
    for StoredPromptBody {
        reference_id, body, ..
    } in state.extra_prompts()
    {
        indexed.insert(
            scope_reference_key(state.scope(), reference_id),
            body.trim().to_string(),
        );
    }
}

pub(super) fn candidate_body_key(
    origin: Option<PromptSourceOrigin>,
    kind: PromptSourceKind,
    reference_id: &str,
) -> String {
    format!(
        "{}::{:?}::{reference_id}",
        origin.map_or("none", PromptSourceOrigin::as_str),
        kind
    )
}

pub(super) fn scope_origin(scope: PromptAssemblyScope) -> PromptSourceOrigin {
    match scope {
        PromptAssemblyScope::Global => PromptSourceOrigin::Global,
        PromptAssemblyScope::Project => PromptSourceOrigin::Project,
    }
}

pub(super) fn entry_origin(
    scope: PromptAssemblyScope,
    kind: PromptSourceKind,
) -> PromptSourceOrigin {
    if matches!(
        kind,
        PromptSourceKind::ToolGuidelines
            | PromptSourceKind::DynamicEnvironmentBaseline
            | PromptSourceKind::DynamicEnvironmentChanges
    ) {
        PromptSourceOrigin::Builtin
    } else {
        scope_origin(scope)
    }
}

pub(super) fn scope_state_mut<'a>(
    global_state: &'a mut PromptAssemblyScopeState,
    project_state: &'a mut PromptAssemblyScopeState,
    scope: PromptAssemblyScope,
) -> &'a mut PromptAssemblyScopeState {
    match scope {
        PromptAssemblyScope::Global => global_state,
        PromptAssemblyScope::Project => project_state,
    }
}

pub(super) fn prompt_source_scope_state_mut<'a>(
    global_state: &'a mut PromptAssemblyScopeState,
    project_state: &'a mut PromptAssemblyScopeState,
    scope: PromptAssemblyScope,
    kind: PromptSourceKind,
) -> &'a mut PromptAssemblyScopeState {
    let scope = prompt_source_effective_scope(kind, global_state, project_state, scope);
    scope_state_mut(global_state, project_state, scope)
}

pub(super) fn prompt_source_effective_scope(
    kind: PromptSourceKind,
    global_state: &PromptAssemblyScopeState,
    project_state: &PromptAssemblyScopeState,
    fallback: PromptAssemblyScope,
) -> PromptAssemblyScope {
    match kind {
        PromptSourceKind::SkillDiscovery => {
            skill_discovery_scope(global_state, project_state, fallback)
        }
        PromptSourceKind::ToolGuidelines => {
            tool_guidelines_scope(global_state, project_state, fallback)
        }
        PromptSourceKind::DynamicEnvironmentBaseline
        | PromptSourceKind::DynamicEnvironmentChanges => {
            let _ = (global_state, project_state, fallback);
            PromptAssemblyScope::Global
        }
        PromptSourceKind::CoreSystemPrompt
        | PromptSourceKind::InstructionsFile
        | PromptSourceKind::ExtraPrompt
        | PromptSourceKind::LongLivedSkill => fallback,
    }
}

pub(super) fn skill_discovery_scope(
    global_state: &PromptAssemblyScopeState,
    project_state: &PromptAssemblyScopeState,
    fallback: PromptAssemblyScope,
) -> PromptAssemblyScope {
    if project_state.skill_discovery_override().is_some()
        || !project_state.skill_discovery_skills().is_empty()
        || project_state
            .entries()
            .iter()
            .any(|entry| entry.kind == PromptSourceKind::SkillDiscovery)
    {
        PromptAssemblyScope::Project
    } else if global_state.skill_discovery_override().is_some()
        || !global_state.skill_discovery_skills().is_empty()
        || global_state
            .entries()
            .iter()
            .any(|entry| entry.kind == PromptSourceKind::SkillDiscovery)
    {
        PromptAssemblyScope::Global
    } else {
        fallback
    }
}

pub(super) fn tool_guidelines_scope(
    global_state: &PromptAssemblyScopeState,
    project_state: &PromptAssemblyScopeState,
    fallback: PromptAssemblyScope,
) -> PromptAssemblyScope {
    if project_state.tool_guidelines_override().is_some()
        || !project_state.tool_selections().is_empty()
        || project_state
            .entries()
            .iter()
            .any(|entry| entry.kind == PromptSourceKind::ToolGuidelines)
    {
        PromptAssemblyScope::Project
    } else if global_state.tool_guidelines_override().is_some()
        || !global_state.tool_selections().is_empty()
        || global_state
            .entries()
            .iter()
            .any(|entry| entry.kind == PromptSourceKind::ToolGuidelines)
    {
        PromptAssemblyScope::Global
    } else {
        fallback
    }
}

pub(super) fn ensure_prompt_source_entry_materialized(
    work_dir: &Path,
    global_state: &mut PromptAssemblyScopeState,
    project_state: &mut PromptAssemblyScopeState,
    scope: PromptAssemblyScope,
    kind: PromptSourceKind,
    _reference_id: &str,
) {
    match kind {
        PromptSourceKind::SkillDiscovery => {
            ensure_skill_discovery_entry_exists(scope_state_mut(
                global_state,
                project_state,
                scope,
            ));
        }
        PromptSourceKind::ToolGuidelines => {
            ensure_tool_guidelines_entry_exists(prompt_source_scope_state_mut(
                global_state,
                project_state,
                scope,
                kind,
            ));
        }
        PromptSourceKind::DynamicEnvironmentBaseline
        | PromptSourceKind::DynamicEnvironmentChanges => {
            ensure_default_dynamic_environment_sources(global_state, project_state);
        }
        PromptSourceKind::InstructionsFile => {
            let (discovered_instruction_files, _) = discover_instruction_files(work_dir, None);
            ensure_discovered_instruction_entries(
                global_state,
                project_state,
                &discovered_instruction_files,
            );
        }
        _ => {}
    }
}

pub(super) fn ensure_active_prompt_source_ordering_materialized(
    work_dir: &Path,
    global_state: &mut PromptAssemblyScopeState,
    project_state: &mut PromptAssemblyScopeState,
    scope: PromptAssemblyScope,
    kind: PromptSourceKind,
    reference_id: &str,
) {
    ensure_default_skill_discovery_source(global_state, project_state);
    ensure_default_tool_guidelines_source(global_state, project_state);
    ensure_default_dynamic_environment_sources(global_state, project_state);
    let (discovered_instruction_files, _) = discover_instruction_files(work_dir, None);
    ensure_discovered_instruction_entries(
        global_state,
        project_state,
        &discovered_instruction_files,
    );
    ensure_prompt_source_entry_materialized(
        work_dir,
        global_state,
        project_state,
        scope,
        kind,
        reference_id,
    );
}

pub(super) fn ensure_skill_discovery_selection_state_materialized(
    work_dir: &Path,
    global_state: &mut PromptAssemblyScopeState,
    project_state: &mut PromptAssemblyScopeState,
    scope: PromptAssemblyScope,
) {
    let discovered_skills = effective_discovered_skills(&discover_skills(work_dir, None));
    let merged_state =
        merged_skill_discovery_skill_state(global_state, project_state, &discovered_skills);
    let state = scope_state_mut(global_state, project_state, scope);
    if state.skill_discovery_skills() != merged_state.as_slice() {
        state.set_skill_discovery_skills(merged_state);
    }
}

pub(super) fn ensure_tool_selection_state_materialized(
    global_state: &mut PromptAssemblyScopeState,
    project_state: &mut PromptAssemblyScopeState,
    scope: PromptAssemblyScope,
    tool_definitions: &[ToolDefinition],
) {
    let merged_state = merged_tool_selection_state(global_state, project_state, tool_definitions);
    let state = scope_state_mut(global_state, project_state, scope);
    if state.tool_selections() != merged_state.as_slice() {
        state.set_tool_selections(merged_state);
    }
}

pub(super) fn ensure_dynamic_environment_selection_state_materialized(
    global_state: &mut PromptAssemblyScopeState,
    project_state: &mut PromptAssemblyScopeState,
    scope: PromptAssemblyScope,
) {
    let merged_state = merged_dynamic_environment_selection_state(global_state, project_state);
    let state = scope_state_mut(global_state, project_state, scope);
    if state.dynamic_environment_sources() != merged_state.as_slice() {
        state.set_dynamic_environment_sources(merged_state);
    }
}

pub(super) fn generate_extra_prompt_reference_id_slug(title: &str) -> String {
    let slug = title
        .chars()
        .flat_map(char::to_lowercase)
        .map(|character| match character {
            'a'..='z' | '0'..='9' => character,
            _ => '-',
        })
        .collect::<String>()
        .split('-')
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    let slug = if slug.is_empty() {
        "prompt"
    } else {
        slug.as_str()
    };
    slug.to_string()
}

pub(super) fn generate_unique_extra_prompt_reference_id(
    state: &PromptAssemblyScopeState,
    title: &str,
) -> String {
    let base = generate_extra_prompt_reference_id_slug(title);
    let existing_reference_ids = state
        .entries()
        .iter()
        .filter(|entry| entry.kind == PromptSourceKind::ExtraPrompt)
        .map(|entry| entry.reference_id.as_str())
        .chain(
            state
                .extra_prompts()
                .iter()
                .map(|prompt| prompt.reference_id.as_str()),
        )
        .collect::<std::collections::HashSet<_>>();
    if !existing_reference_ids.contains(base.as_str()) {
        return base;
    }

    let mut suffix = 2usize;
    loop {
        let candidate = format!("{base}-{suffix}");
        if !existing_reference_ids.contains(candidate.as_str()) {
            return candidate;
        }
        suffix = suffix.saturating_add(1);
    }
}

pub(super) fn next_requested_order(entries: &[PersistedPromptAssemblyEntry]) -> u16 {
    entries
        .iter()
        .filter_map(|entry| entry.requested_order)
        .max()
        .unwrap_or(DEFAULT_SKILL_DISCOVERY_REQUESTED_ORDER)
        .saturating_add(10)
}

pub(super) fn activate_long_lived_skill(
    global_state: &mut PromptAssemblyScopeState,
    project_state: &mut PromptAssemblyScopeState,
    scope: PromptAssemblyScope,
    skill_name: &str,
) {
    let state = scope_state_mut(global_state, project_state, scope);
    let next_order = next_requested_order(state.entries());
    if let Some(entry) = state.entries_mut().iter_mut().find(|entry| {
        entry.kind == PromptSourceKind::LongLivedSkill && entry.reference_id == skill_name
    }) {
        entry.enabled = true;
        if entry.requested_order.is_none() {
            entry.requested_order = Some(next_order);
        }
        return;
    }

    state.entries_mut().push(PersistedPromptAssemblyEntry {
        reference_id: skill_name.to_string(),
        kind: PromptSourceKind::LongLivedSkill,
        title: skill_name.to_string(),
        enabled: true,
        requested_order: Some(next_order),
    });
}

pub(super) fn ensure_skill_discovery_entry_exists(state: &mut PromptAssemblyScopeState) {
    if state
        .entries()
        .iter()
        .any(|entry| entry.kind == PromptSourceKind::SkillDiscovery)
    {
        return;
    }
    let requested_order = default_skill_discovery_requested_order(state.entries());
    state.entries_mut().push(PersistedPromptAssemblyEntry {
        reference_id: "skill-discovery".to_string(),
        kind: PromptSourceKind::SkillDiscovery,
        title: "Skill discovery".to_string(),
        enabled: true,
        requested_order: Some(requested_order),
    });
}

pub(super) fn ensure_default_skill_discovery_source(
    global_state: &mut PromptAssemblyScopeState,
    project_state: &mut PromptAssemblyScopeState,
) {
    if global_state
        .entries()
        .iter()
        .chain(project_state.entries().iter())
        .any(|entry| entry.kind == PromptSourceKind::SkillDiscovery)
    {
        return;
    }

    let target = if scope_state_has_prompt_content(project_state)
        || !scope_state_has_prompt_content(global_state)
    {
        project_state
    } else {
        global_state
    };
    ensure_skill_discovery_entry_exists(target);
}

pub(super) fn scope_state_has_prompt_content(state: &PromptAssemblyScopeState) -> bool {
    state.core_system_override().is_some()
        || state.skill_discovery_override().is_some()
        || state.tool_guidelines_override().is_some()
        || !state.entries().is_empty()
        || !state.skill_discovery_skills().is_empty()
        || !state.tool_selections().is_empty()
        || !state.extra_prompts().is_empty()
}

pub(super) fn default_skill_discovery_requested_order(
    entries: &[PersistedPromptAssemblyEntry],
) -> u16 {
    entries
        .iter()
        .find(|entry| entry.kind == PromptSourceKind::SkillDiscovery)
        .and_then(|entry| entry.requested_order)
        .unwrap_or(DEFAULT_SKILL_DISCOVERY_REQUESTED_ORDER)
}

pub(super) fn set_prompt_source_enabled(
    state: &mut PromptAssemblyScopeState,
    kind: PromptSourceKind,
    reference_id: &str,
    enabled: bool,
) {
    if let Some(entry) = state
        .entries_mut()
        .iter_mut()
        .find(|entry| entry.kind == kind && entry.reference_id == reference_id)
    {
        entry.enabled = enabled;
    }
}

pub(super) fn set_extra_prompt_selected(
    global_state: &mut PromptAssemblyScopeState,
    project_state: &mut PromptAssemblyScopeState,
    scope: PromptAssemblyScope,
    reference_id: &str,
    selected: bool,
) {
    let state = scope_state_mut(global_state, project_state, scope);
    let Some(prompt) = state
        .extra_prompts()
        .iter()
        .find(|prompt| prompt.reference_id == reference_id)
        .cloned()
    else {
        return;
    };

    if selected {
        if state.entries().iter().any(|entry| {
            entry.kind == PromptSourceKind::ExtraPrompt && entry.reference_id == reference_id
        }) {
            return;
        }
        let requested_order = next_requested_order(state.entries());
        state.entries_mut().push(PersistedPromptAssemblyEntry {
            reference_id: prompt.reference_id,
            kind: PromptSourceKind::ExtraPrompt,
            title: prompt.title,
            enabled: true,
            requested_order: Some(requested_order),
        });
        return;
    }

    state.entries_mut().retain(|entry| {
        !(entry.kind == PromptSourceKind::ExtraPrompt && entry.reference_id == reference_id)
    });
}

pub(super) fn set_discovered_skill_selected(
    state: &mut PromptAssemblyScopeState,
    skill_name: &str,
    selected: bool,
) {
    let next_order = next_skill_discovery_requested_order(state.skill_discovery_skills());
    if let Some(entry) = state
        .skill_discovery_skills_mut()
        .iter_mut()
        .find(|entry| entry.skill_name == skill_name)
    {
        entry.enabled = selected;
        if selected && entry.requested_order.is_none() {
            entry.requested_order = Some(next_order);
        }
        return;
    }

    state
        .skill_discovery_skills_mut()
        .push(PersistedSkillDiscoverySkillEntry {
            skill_name: skill_name.to_string(),
            enabled: selected,
            requested_order: Some(next_order),
        });
}

pub(super) fn move_discovered_skill(
    state: &mut PromptAssemblyScopeState,
    skill_name: &str,
    direction: PromptAssemblyMoveDirection,
) -> Result<()> {
    let Some(position) = state
        .skill_discovery_skills()
        .iter()
        .position(|entry| entry.skill_name == skill_name)
    else {
        return Ok(());
    };
    let Some(neighbor) = (match direction {
        PromptAssemblyMoveDirection::Up => position.checked_sub(1),
        PromptAssemblyMoveDirection::Down => {
            (position + 1 < state.skill_discovery_skills().len()).then_some(position + 1)
        }
    }) else {
        return Ok(());
    };
    state.skill_discovery_skills_mut().swap(position, neighbor);
    normalize_skill_discovery_requested_orders(state);
    Ok(())
}

pub(super) fn reset_discovered_skill_order(
    state: &mut PromptAssemblyScopeState,
    discovered_skills: &[DiscoveredSkill],
) {
    let enabled_by_name = state
        .skill_discovery_skills()
        .iter()
        .map(|entry| (entry.skill_name.as_str(), entry.enabled))
        .collect::<HashMap<_, _>>();
    state.set_skill_discovery_skills(
        discovered_skills
            .iter()
            .filter(|skill| skill.can_select_for_discovery())
            .enumerate()
            .map(|(index, skill)| PersistedSkillDiscoverySkillEntry {
                skill_name: skill.name.clone(),
                enabled: enabled_by_name
                    .get(skill.name.as_str())
                    .copied()
                    .unwrap_or(true),
                requested_order: Some(u16::try_from(index + 1).unwrap_or(u16::MAX)),
            })
            .collect(),
    );
}

pub(super) fn next_skill_discovery_requested_order(
    entries: &[PersistedSkillDiscoverySkillEntry],
) -> u16 {
    entries
        .iter()
        .filter_map(|entry| entry.requested_order)
        .max()
        .unwrap_or(0)
        .saturating_add(1)
}

pub(super) fn normalize_skill_discovery_requested_orders(state: &mut PromptAssemblyScopeState) {
    for (index, entry) in state.skill_discovery_skills_mut().iter_mut().enumerate() {
        entry.requested_order = Some(u16::try_from(index + 1).unwrap_or(u16::MAX));
    }
}

pub(super) fn remove_prompt_source(
    state: &mut PromptAssemblyScopeState,
    kind: PromptSourceKind,
    reference_id: &str,
) {
    if matches!(
        kind,
        PromptSourceKind::InstructionsFile
            | PromptSourceKind::SkillDiscovery
            | PromptSourceKind::ToolGuidelines
            | PromptSourceKind::DynamicEnvironmentBaseline
            | PromptSourceKind::DynamicEnvironmentChanges
    ) {
        return;
    }
    state
        .entries_mut()
        .retain(|entry| !(entry.kind == kind && entry.reference_id == reference_id));
}

pub(super) fn move_active_source(
    global_state: &mut PromptAssemblyScopeState,
    project_state: &mut PromptAssemblyScopeState,
    scope: PromptAssemblyScope,
    kind: PromptSourceKind,
    reference_id: &str,
    direction: PromptAssemblyMoveDirection,
) -> Result<()> {
    let Some(current) = find_entry_address(global_state, project_state, scope, kind, reference_id)
    else {
        return Ok(());
    };
    let ordered = ordered_non_core_entry_addresses(global_state, project_state);
    let Some(position) = ordered.iter().position(|address| *address == current) else {
        return Ok(());
    };
    let neighbor_position = match direction {
        PromptAssemblyMoveDirection::Up => position.checked_sub(1),
        PromptAssemblyMoveDirection::Down => (position + 1 < ordered.len()).then_some(position + 1),
    };
    let Some(neighbor_position) = neighbor_position else {
        return Ok(());
    };
    let neighbor = ordered[neighbor_position];

    let current_order = entry_requested_order(global_state, project_state, current);
    let neighbor_order = entry_requested_order(global_state, project_state, neighbor);
    set_entry_requested_order(global_state, project_state, current, neighbor_order);
    set_entry_requested_order(global_state, project_state, neighbor, current_order);
    normalize_requested_orders(global_state, project_state);
    Ok(())
}

pub(super) fn find_entry_address(
    global_state: &PromptAssemblyScopeState,
    project_state: &PromptAssemblyScopeState,
    scope: PromptAssemblyScope,
    kind: PromptSourceKind,
    reference_id: &str,
) -> Option<PromptEntryAddress> {
    let state = match scope {
        PromptAssemblyScope::Global => global_state,
        PromptAssemblyScope::Project => project_state,
    };
    state
        .entries()
        .iter()
        .position(|entry| entry.kind == kind && entry.reference_id == reference_id)
        .map(|index| PromptEntryAddress { scope, index })
}

pub(super) fn ordered_non_core_entry_addresses(
    global_state: &PromptAssemblyScopeState,
    project_state: &PromptAssemblyScopeState,
) -> Vec<PromptEntryAddress> {
    let mut addresses = Vec::new();
    addresses.extend(state_entry_addresses(global_state));
    addresses.extend(state_entry_addresses(project_state));
    addresses.sort_by(|left, right| {
        let left_entry = entry_ref(global_state, project_state, *left);
        let right_entry = entry_ref(global_state, project_state, *right);
        requested_order_sort_key(left_entry.requested_order)
            .cmp(&requested_order_sort_key(right_entry.requested_order))
            .then_with(|| natural_sort_text_cmp(&left_entry.title, &right_entry.title))
            .then_with(|| left_entry.reference_id.cmp(&right_entry.reference_id))
            .then_with(|| {
                left.scope
                    .as_stored_value()
                    .cmp(right.scope.as_stored_value())
            })
    });
    addresses
}

pub(super) fn state_entry_addresses(state: &PromptAssemblyScopeState) -> Vec<PromptEntryAddress> {
    state
        .entries()
        .iter()
        .enumerate()
        .map(|(index, _)| PromptEntryAddress {
            scope: state.scope(),
            index,
        })
        .collect()
}

pub(super) fn entry_ref<'a>(
    global_state: &'a PromptAssemblyScopeState,
    project_state: &'a PromptAssemblyScopeState,
    address: PromptEntryAddress,
) -> &'a PersistedPromptAssemblyEntry {
    match address.scope {
        PromptAssemblyScope::Global => &global_state.entries()[address.index],
        PromptAssemblyScope::Project => &project_state.entries()[address.index],
    }
}

pub(super) fn entry_mut<'a>(
    global_state: &'a mut PromptAssemblyScopeState,
    project_state: &'a mut PromptAssemblyScopeState,
    address: PromptEntryAddress,
) -> &'a mut PersistedPromptAssemblyEntry {
    match address.scope {
        PromptAssemblyScope::Global => &mut global_state.entries_mut()[address.index],
        PromptAssemblyScope::Project => &mut project_state.entries_mut()[address.index],
    }
}

pub(super) fn entry_requested_order(
    global_state: &PromptAssemblyScopeState,
    project_state: &PromptAssemblyScopeState,
    address: PromptEntryAddress,
) -> Option<u16> {
    entry_ref(global_state, project_state, address).requested_order
}

pub(super) fn set_entry_requested_order(
    global_state: &mut PromptAssemblyScopeState,
    project_state: &mut PromptAssemblyScopeState,
    address: PromptEntryAddress,
    requested_order: Option<u16>,
) {
    entry_mut(global_state, project_state, address).requested_order = requested_order;
}

pub(super) fn normalize_requested_orders(
    global_state: &mut PromptAssemblyScopeState,
    project_state: &mut PromptAssemblyScopeState,
) {
    let ordered = ordered_non_core_entry_addresses(global_state, project_state);
    for (index, address) in ordered.into_iter().enumerate() {
        let normalized = u16::try_from((index + 1) * 10).unwrap_or(u16::MAX);
        set_entry_requested_order(global_state, project_state, address, Some(normalized));
    }
}
