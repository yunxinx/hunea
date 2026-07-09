use super::*;

pub(in crate::prompt_assembly) fn materialized_sources_for_state(
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

pub(in crate::prompt_assembly) fn extend_candidates(
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

pub(in crate::prompt_assembly) fn resolvable_for_entry(
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

pub(in crate::prompt_assembly) fn body_for_entry(
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

pub(in crate::prompt_assembly) fn resolved_body_origin_for_entry(
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

pub(in crate::prompt_assembly) fn backing_file_path_for_entry(
    entry: &PersistedPromptAssemblyEntry,
    instructions_by_reference_id: &HashMap<String, DiscoveredInstructionsFile>,
) -> Option<PathBuf> {
    (entry.kind == PromptSourceKind::InstructionsFile).then(|| {
        instructions_by_reference_id
            .get(&entry.reference_id)
            .map(|file| file.path.clone())
    })?
}

pub(in crate::prompt_assembly) fn collision_key_for_entry(
    entry: &PersistedPromptAssemblyEntry,
) -> Option<String> {
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

pub(in crate::prompt_assembly) fn indexed_extra_prompt_bodies(
    global_state: &PromptAssemblyScopeState,
    project_state: &PromptAssemblyScopeState,
) -> HashMap<String, String> {
    let mut bodies = HashMap::new();
    insert_extra_prompt_bodies(&mut bodies, global_state);
    insert_extra_prompt_bodies(&mut bodies, project_state);
    bodies
}

pub(in crate::prompt_assembly) fn insert_extra_prompt_bodies(
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

pub(in crate::prompt_assembly) fn candidate_body_key(
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

pub(in crate::prompt_assembly) fn ensure_prompt_source_entry_materialized(
    work_dir: &Path,
    config_dir: &Path,
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
            ensure_default_dynamic_environment_sources(global_state);
        }
        PromptSourceKind::InstructionsFile => {
            let (discovered_instruction_files, _) =
                discover_instruction_files(work_dir, config_dir, None);
            ensure_discovered_instruction_entries(
                global_state,
                project_state,
                &discovered_instruction_files,
            );
        }
        _ => {}
    }
}

pub(in crate::prompt_assembly) fn ensure_active_prompt_source_ordering_materialized(
    work_dir: &Path,
    config_dir: &Path,
    global_state: &mut PromptAssemblyScopeState,
    project_state: &mut PromptAssemblyScopeState,
    scope: PromptAssemblyScope,
    kind: PromptSourceKind,
    reference_id: &str,
) {
    ensure_default_skill_discovery_source(global_state, project_state);
    ensure_default_tool_guidelines_source(global_state, project_state);
    ensure_default_dynamic_environment_sources(global_state);
    let (discovered_instruction_files, _) = discover_instruction_files(work_dir, config_dir, None);
    ensure_discovered_instruction_entries(
        global_state,
        project_state,
        &discovered_instruction_files,
    );
    ensure_prompt_source_entry_materialized(
        work_dir,
        config_dir,
        global_state,
        project_state,
        scope,
        kind,
        reference_id,
    );
}

pub(in crate::prompt_assembly) fn ensure_skill_discovery_selection_state_materialized(
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

pub(in crate::prompt_assembly) fn ensure_tool_selection_state_materialized(
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

pub(in crate::prompt_assembly) fn ensure_dynamic_environment_selection_state_materialized(
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
