use super::*;

/// 测试辅助：加载初始 prompt prelude。
///
/// `work_dir` 与 `config_dir` 必须分开传入——前者是项目目录，后者是数据目录
///（全局 `~/.config/hunea/` 或便携 `.hunea/`）。二者不可互换，否则全局
/// AGENTS.md 会去项目目录下找。
#[cfg(test)]
pub(crate) fn load_initial_prompt_prelude(
    store: Arc<dyn SessionStore>,
    work_dir: &Path,
    config_dir: &Path,
) -> Result<PromptPreludeSnapshot> {
    Ok(PromptAssemblyWorkspace::new(work_dir, config_dir, &[])
        .load_manager(store)?
        .resolution
        .prelude)
}

pub(crate) fn dynamic_environment_session_config_from_manager(
    manager: &PromptAssemblyManagerSnapshot,
) -> DynamicEnvironmentSessionConfig {
    let mut source_selections = manager
        .candidates
        .dynamic_environment
        .iter()
        .flat_map(|candidate| {
            [
                DynamicEnvironmentSourceSelection {
                    snapshot_kind: DynamicEnvironmentSnapshotKind::Baseline,
                    source_kind: candidate.source_kind,
                    enabled: candidate.baseline_selected,
                },
                DynamicEnvironmentSourceSelection {
                    snapshot_kind: DynamicEnvironmentSnapshotKind::Changes,
                    source_kind: candidate.source_kind,
                    enabled: candidate.changes_selected,
                },
            ]
        })
        .collect::<Vec<_>>();
    source_selections.sort_by_key(|selection| (selection.snapshot_kind, selection.source_kind));

    DynamicEnvironmentSessionConfig {
        baseline_enabled: manager
            .resolution
            .assembly
            .active_sources
            .iter()
            .any(|source| source.kind == PromptSourceKind::DynamicEnvironmentBaseline),
        changes_enabled: manager
            .resolution
            .assembly
            .active_sources
            .iter()
            .any(|source| source.kind == PromptSourceKind::DynamicEnvironmentChanges),
        source_selections,
        static_baseline_observations: static_dynamic_environment_baseline_observations(manager),
    }
}

fn static_dynamic_environment_baseline_observations(
    manager: &PromptAssemblyManagerSnapshot,
) -> Vec<DynamicEnvironmentObservation> {
    let baseline_active = manager
        .resolution
        .assembly
        .active_sources
        .iter()
        .any(|source| source.kind == PromptSourceKind::DynamicEnvironmentBaseline);
    if !baseline_active {
        return Vec::new();
    }

    let selected_sources = manager
        .candidates
        .dynamic_environment
        .iter()
        .filter(|candidate| candidate.baseline_selected)
        .map(|candidate| candidate.source_kind)
        .collect::<Vec<_>>();
    manager
        .dynamic_environment_observations
        .iter()
        .filter(|observation| selected_sources.contains(&observation.source_kind))
        .cloned()
        .collect()
}

#[cfg(test)]
pub(super) fn resolve_initial_prompt_prelude_with_overrides(
    work_dir: &Path,
    config_dir: &Path,
    global_state: &PromptAssemblyScopeState,
    project_state: &PromptAssemblyScopeState,
    global_skill_root_override: Option<&Path>,
    global_instructions_path_override: Option<&Path>,
) -> PromptPreludeSnapshot {
    resolve_prompt_assembly_manager_snapshot_with_overrides(
        work_dir,
        config_dir,
        global_state,
        project_state,
        global_skill_root_override,
        global_instructions_path_override,
        &[],
    )
    .resolution
    .prelude
}

pub(super) fn resolve_prompt_assembly_manager_snapshot(
    work_dir: &Path,
    config_dir: &Path,
    global_state: &PromptAssemblyScopeState,
    project_state: &PromptAssemblyScopeState,
    tool_definitions: &[ToolDefinition],
) -> PromptAssemblyManagerSnapshot {
    resolve_prompt_assembly_manager_snapshot_with_overrides(
        work_dir,
        config_dir,
        global_state,
        project_state,
        None,
        None,
        tool_definitions,
    )
}

#[cfg(test)]
pub(super) fn resolve_prompt_assembly_manager_snapshot_with_global_skill_root(
    work_dir: &Path,
    config_dir: &Path,
    global_state: &PromptAssemblyScopeState,
    project_state: &PromptAssemblyScopeState,
    global_skill_root_override: Option<&Path>,
    tool_definitions: &[ToolDefinition],
) -> PromptAssemblyManagerSnapshot {
    resolve_prompt_assembly_manager_snapshot_with_overrides(
        work_dir,
        config_dir,
        global_state,
        project_state,
        global_skill_root_override,
        None,
        tool_definitions,
    )
}

pub(super) fn resolve_prompt_assembly_manager_snapshot_with_overrides(
    work_dir: &Path,
    config_dir: &Path,
    global_state: &PromptAssemblyScopeState,
    project_state: &PromptAssemblyScopeState,
    global_skill_root_override: Option<&Path>,
    global_instructions_path_override: Option<&Path>,
    tool_definitions: &[ToolDefinition],
) -> PromptAssemblyManagerSnapshot {
    let mut effective_global_state = global_state.clone();
    let mut effective_project_state = project_state.clone();
    ensure_default_skill_discovery_source(
        &mut effective_global_state,
        &mut effective_project_state,
    );
    ensure_default_tool_guidelines_source(
        &mut effective_global_state,
        &mut effective_project_state,
    );
    ensure_default_dynamic_environment_sources(&mut effective_global_state);
    let (discovered_skills, mut diagnostics) =
        discover_skills_with_diagnostics(work_dir, global_skill_root_override);
    let (discovered_instruction_files, instruction_diagnostics) =
        discover_instruction_files(work_dir, config_dir, global_instructions_path_override);
    diagnostics.extend(instruction_diagnostics);
    ensure_discovered_instruction_entries(
        &mut effective_global_state,
        &mut effective_project_state,
        &discovered_instruction_files,
    );
    let global_state = &effective_global_state;
    let project_state = &effective_project_state;
    let effective_discovered_skills = effective_discovered_skills(&discovered_skills);
    let extra_prompt_bodies = indexed_extra_prompt_bodies(global_state, project_state);
    let instructions_by_reference_id = discovered_instruction_files
        .iter()
        .map(|file| (file.reference_id.clone(), file.clone()))
        .collect::<HashMap<_, _>>();
    let skill_discovery_skill_state = merged_skill_discovery_skill_state(
        global_state,
        project_state,
        &effective_discovered_skills,
    );
    let tool_selection_state =
        merged_tool_selection_state(global_state, project_state, tool_definitions);
    let tool_enablement_state =
        merged_tool_enablement_state(global_state, project_state, tool_definitions);
    let dynamic_environment_selection_state =
        merged_dynamic_environment_selection_state(global_state, project_state);
    let dynamic_environment_observations = observe_dynamic_environment_inventory(work_dir);
    let skills_by_name = effective_discovered_skills
        .iter()
        .map(|skill| (skill.name.clone(), skill.clone()))
        .collect::<HashMap<_, _>>();
    let resolution_context = PromptAssemblyResolutionContext {
        extra_prompt_bodies: &extra_prompt_bodies,
        skills_by_name: &skills_by_name,
        instructions_by_reference_id: &instructions_by_reference_id,
        skill_discovery_skill_state: &skill_discovery_skill_state,
        tool_definitions,
        tool_selection_state: &tool_selection_state,
        tool_enablement_state: &tool_enablement_state,
        dynamic_environment_selection_state: &dynamic_environment_selection_state,
        dynamic_environment_observations: &dynamic_environment_observations,
        global_state,
        project_state,
    };

    let mut candidate_bodies = HashMap::new();
    let mut candidates = Vec::new();
    extend_candidates(
        &mut candidates,
        &mut candidate_bodies,
        global_state,
        &resolution_context,
    );
    extend_candidates(
        &mut candidates,
        &mut candidate_bodies,
        project_state,
        &resolution_context,
    );

    let snapshot = resolve_prompt_assembly(&PromptAssemblyInput {
        core_system: CoreSystemPromptInput {
            global_override_present: global_state.core_system_override().is_some(),
            project_override_present: project_state.core_system_override().is_some(),
        },
        candidates,
    });
    let mut sources = vec![PromptAssemblyManagerSource {
        reference_id: "core-system".to_string(),
        kind: PromptSourceKind::CoreSystemPrompt,
        title: "Core system prompt".to_string(),
        origin: Some(resolve_core_system_origin(global_state, project_state)),
        resolved_body_origin: Some(resolve_core_system_origin(global_state, project_state)),
        backing_file_path: None,
        body: Some(resolved_core_system_body(global_state, project_state)),
    }];
    sources.extend(materialized_sources_for_state(
        global_state,
        &resolution_context,
    ));
    sources.extend(materialized_sources_for_state(
        project_state,
        &resolution_context,
    ));

    let mut sections = Vec::new();
    for source in &snapshot.active_sources {
        if !matches!(source.status, PromptSourceStatus::Active { .. }) {
            continue;
        }
        if source.kind == PromptSourceKind::DynamicEnvironmentChanges {
            continue;
        }

        let body = match source.kind {
            PromptSourceKind::CoreSystemPrompt => {
                resolved_core_system_body(global_state, project_state)
            }
            _ => candidate_bodies
                .get(&candidate_body_key(
                    source.origin,
                    source.kind,
                    &source.reference_id,
                ))
                .cloned()
                .unwrap_or_default(),
        };
        if body.trim().is_empty() {
            continue;
        }

        sections.push(PromptPreludeSection {
            reference_id: source.reference_id.clone(),
            kind: source.kind,
            title: source.title.clone(),
            origin: source.origin,
            body,
        });
    }

    PromptAssemblyManagerSnapshot {
        resolution: PromptAssemblyResolvedSnapshot {
            assembly: snapshot,
            prelude: PromptPreludeSnapshot { sections },
        },
        sources: PromptAssemblySourceInventorySnapshot {
            managed: managed_sources(global_state, project_state),
            preview: sources,
        },
        candidates: PromptAssemblyCandidateInventorySnapshot {
            extra_prompts: extra_prompt_candidates(
                global_state,
                project_state,
                &extra_prompt_bodies,
            ),
            discovered_skills: discovered_skill_inventory(
                &discovered_skills,
                global_state,
                project_state,
                &skill_discovery_skill_state,
            ),
            manual_skills: manual_skill_inventory(&effective_discovered_skills),
            tools: tool_candidate_inventory(
                tool_definitions,
                &tool_selection_state,
                &tool_enablement_state,
                global_state,
                project_state,
            ),
            dynamic_environment: dynamic_environment_candidate_inventory(
                &dynamic_environment_observations,
                &dynamic_environment_selection_state,
                global_state,
                project_state,
            ),
        },
        dynamic_environment_observations: {
            let mut observations = dynamic_environment_observations
                .into_values()
                .collect::<Vec<_>>();
            observations.sort_by_key(|observation| observation.source_kind);
            observations
        },
        diagnostics,
        core_system: PromptAssemblyCoreSystemSnapshot {
            builtin_body: BUILTIN_CORE_SYSTEM_PROMPT.to_string(),
            global_override: global_state.core_system_override().map(str::to_string),
            project_override: project_state.core_system_override().map(str::to_string),
        },
    }
}

fn managed_sources(
    global_state: &PromptAssemblyScopeState,
    project_state: &PromptAssemblyScopeState,
) -> Vec<PromptAssemblyManagedSource> {
    let mut sources = vec![PromptAssemblyManagedSource {
        reference_id: "core-system".to_string(),
        kind: PromptSourceKind::CoreSystemPrompt,
        title: "Core system prompt".to_string(),
        origin: Some(resolve_core_system_origin(global_state, project_state)),
        scope: None,
        enabled: true,
        order: 1,
    }];

    let mut entries = global_state
        .entries()
        .iter()
        .map(|entry| (PromptAssemblyScope::Global, entry))
        .chain(
            project_state
                .entries()
                .iter()
                .map(|entry| (PromptAssemblyScope::Project, entry)),
        )
        .collect::<Vec<_>>();
    entries.sort_by(|(left_scope, left), (right_scope, right)| {
        requested_order_sort_key(left.requested_order)
            .cmp(&requested_order_sort_key(right.requested_order))
            .then_with(|| natural_sort_text_cmp(&left.title, &right.title))
            .then_with(|| left.reference_id.cmp(&right.reference_id))
            .then_with(|| {
                left_scope
                    .as_stored_value()
                    .cmp(right_scope.as_stored_value())
            })
    });

    for (index, (scope, entry)) in entries.into_iter().enumerate() {
        sources.push(PromptAssemblyManagedSource {
            reference_id: entry.reference_id.clone(),
            kind: entry.kind,
            title: entry.title.clone(),
            origin: Some(entry_origin(scope, entry.kind)),
            scope: Some(scope),
            enabled: entry.enabled,
            order: index + 2,
        });
    }

    sources
}

fn extra_prompt_candidates(
    global_state: &PromptAssemblyScopeState,
    project_state: &PromptAssemblyScopeState,
    extra_prompt_bodies: &HashMap<String, String>,
) -> Vec<PromptAssemblyExtraPromptCandidate> {
    let mut candidates = Vec::new();
    push_extra_prompt_candidates(&mut candidates, global_state, extra_prompt_bodies);
    push_extra_prompt_candidates(&mut candidates, project_state, extra_prompt_bodies);
    candidates.sort_by(|left, right| {
        natural_sort_text_cmp(&left.title, &right.title)
            .then_with(|| left.reference_id.cmp(&right.reference_id))
    });
    candidates
}

pub(super) fn effective_discovered_skills(
    discovered_skills: &[DiscoveredSkill],
) -> Vec<DiscoveredSkill> {
    let mut seen_names = std::collections::HashSet::<&str>::new();
    discovered_skills
        .iter()
        .filter(|skill| seen_names.insert(skill.name.as_str()))
        .cloned()
        .collect()
}

fn push_extra_prompt_candidates(
    candidates: &mut Vec<PromptAssemblyExtraPromptCandidate>,
    state: &PromptAssemblyScopeState,
    extra_prompt_bodies: &HashMap<String, String>,
) {
    let selected_ids = state
        .entries()
        .iter()
        .filter(|entry| entry.kind == PromptSourceKind::ExtraPrompt)
        .map(|entry| entry.reference_id.as_str())
        .collect::<std::collections::HashSet<_>>();
    for prompt in state.extra_prompts() {
        let body = extra_prompt_bodies
            .get(&scope_reference_key(state.scope(), &prompt.reference_id))
            .cloned()
            .unwrap_or_else(|| prompt.body.trim().to_string());
        candidates.push(PromptAssemblyExtraPromptCandidate {
            reference_id: prompt.reference_id.clone(),
            title: prompt.title.clone(),
            origin: scope_origin(state.scope()),
            body,
            selected: selected_ids.contains(prompt.reference_id.as_str()),
        });
    }
}

pub(super) fn merged_skill_discovery_skill_state(
    global_state: &PromptAssemblyScopeState,
    project_state: &PromptAssemblyScopeState,
    discovered_skills: &[DiscoveredSkill],
) -> Vec<PersistedSkillDiscoverySkillEntry> {
    let mut state_by_name = HashMap::<String, PersistedSkillDiscoverySkillEntry>::new();
    for entry in global_state.skill_discovery_skills() {
        state_by_name.insert(entry.skill_name.clone(), entry.clone());
    }
    for entry in project_state.skill_discovery_skills() {
        state_by_name.insert(entry.skill_name.clone(), entry.clone());
    }

    let mut state = discovered_skills
        .iter()
        .filter(|skill| skill.can_select_for_discovery())
        .enumerate()
        .map(|(index, skill)| {
            state_by_name
                .get(&skill.name)
                .cloned()
                .unwrap_or(PersistedSkillDiscoverySkillEntry {
                    skill_name: skill.name.clone(),
                    enabled: true,
                    requested_order: Some(u16::try_from(index + 1).unwrap_or(u16::MAX)),
                })
        })
        .collect::<Vec<_>>();
    state.sort_by(|left, right| {
        requested_order_sort_key(left.requested_order)
            .cmp(&requested_order_sort_key(right.requested_order))
            .then_with(|| natural_sort_text_cmp(&left.skill_name, &right.skill_name))
    });
    state
}
