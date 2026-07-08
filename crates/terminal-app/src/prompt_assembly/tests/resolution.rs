use super::*;

#[test]
fn resolve_initial_prompt_prelude_orders_core_extra_discovery_and_long_lived_skill() {
    let work_dir = temp_dir("resolve");
    let project_skill_dir = work_dir.join(".agents/skills/repo-bootstrap");
    let missing_global_instructions_path = work_dir.join("missing-global-AGENTS.md");
    fs::create_dir_all(&project_skill_dir).expect("skill dir should exist");
    fs::write(
            project_skill_dir.join(SKILL_FILE_NAME),
            "---\nname: repo-bootstrap\ndescription: Bootstrap repo\ndisable-model-invocation: false\n---\n# Repo Bootstrap\n\nUse this skill.\n",
        )
        .expect("skill file should exist");

    let global_state = scope_state! {
        scope: PromptAssemblyScope::Global,
        core_system_override: Some("global core".to_string()),
        skill_discovery_override: None,
        entries: vec![
            PersistedPromptAssemblyEntry {
                reference_id: "skill-discovery".to_string(),
                kind: PromptSourceKind::SkillDiscovery,
                title: "Skill discovery source".to_string(),
                enabled: true,
                requested_order: Some(20),
            },
            PersistedPromptAssemblyEntry {
                reference_id: "repo-bootstrap".to_string(),
                kind: PromptSourceKind::LongLivedSkill,
                title: "repo-bootstrap".to_string(),
                enabled: true,
                requested_order: Some(30),
            },
        ],
        skill_discovery_skills: Vec::new(),
        extra_prompts: Vec::new(),
        tool_guidelines_override: None,
        tool_selections: Vec::new(),
        dynamic_environment_sources: Vec::new(),
    };
    let project_state = scope_state! {
        scope: PromptAssemblyScope::Project,
        core_system_override: None,
        skill_discovery_override: None,
        entries: vec![PersistedPromptAssemblyEntry {
            reference_id: "repo-rules".to_string(),
            kind: PromptSourceKind::ExtraPrompt,
            title: "repo-rules".to_string(),
            enabled: true,
            requested_order: Some(10),
        }],
        skill_discovery_skills: Vec::new(),
        extra_prompts: vec![StoredPromptBody {
            reference_id: "repo-rules".to_string(),
            title: "repo-rules".to_string(),
            body: "project rules".to_string(),
        }],
        tool_guidelines_override: None,
        tool_selections: Vec::new(),
        dynamic_environment_sources: Vec::new(),
    };

    let prelude = resolve_initial_prompt_prelude_with_overrides(
        &work_dir,
        &global_state,
        &project_state,
        None,
        Some(&missing_global_instructions_path),
    );

    assert_eq!(prelude.sections.len(), 5);
    assert_eq!(prelude.sections[0].kind, PromptSourceKind::CoreSystemPrompt);
    assert_eq!(prelude.sections[1].reference_id, "repo-rules");
    assert_eq!(
        prelude.sections[2].kind,
        PromptSourceKind::DynamicEnvironmentBaseline
    );
    assert_eq!(prelude.sections[3].reference_id, "skill-discovery");
    assert_eq!(prelude.sections[4].reference_id, "repo-bootstrap");
    let effective = prelude
        .effective_system_prompt()
        .expect("effective prompt should exist");
    assert!(effective.starts_with("global core\n\nproject rules\n\n"));
    assert!(effective.contains("Environment baseline for this session:"));
    assert!(effective.contains("<available_skills>"));
    assert!(effective.contains("<name>repo-bootstrap</name>"));
    assert!(effective.contains("<skill>\n<name>repo-bootstrap</name>"));
}
#[test]
fn resolve_initial_prompt_prelude_places_instruction_files_between_core_and_extra_and_stops_at_git_root()
 {
    let global_instructions_dir = temp_dir("instructions-global");
    let global_instructions_path = global_instructions_dir.join("AGENTS.md");
    fs::write(&global_instructions_path, "global instructions\n")
        .expect("global instructions should write");

    let outside_root = temp_dir("instructions-outside-root");
    fs::write(outside_root.join("AGENTS.md"), "outside instructions\n")
        .expect("outside instructions should write");

    let project_root = outside_root.join("repo");
    let nested_dir = project_root.join("workspace").join("crate");
    fs::create_dir_all(&nested_dir).expect("nested dir should exist");
    write_project_skill(&nested_dir, "repo-bootstrap");
    fs::write(project_root.join(".git"), "gitdir: mock\n").expect("git marker should write");
    fs::write(project_root.join("AGENTS.md"), "root instructions\n")
        .expect("root instructions should write");
    fs::write(
        project_root.join("workspace").join("CLAUDE.md"),
        "workspace claude\n",
    )
    .expect("workspace claude should write");
    fs::write(nested_dir.join("AGENTS.md"), "crate agents\n").expect("crate AGENTS should write");
    fs::write(nested_dir.join("CLAUDE.md"), "crate claude\n").expect("crate CLAUDE should write");

    let prelude = resolve_initial_prompt_prelude_with_overrides(
        &nested_dir,
        &scope_state! {
            scope: PromptAssemblyScope::Global,
            core_system_override: Some("global core".to_string()),
            entries: vec![PersistedPromptAssemblyEntry {
                reference_id: "skill-discovery".to_string(),
                kind: PromptSourceKind::SkillDiscovery,
                title: "Skill discovery source".to_string(),
                enabled: true,
                requested_order: Some(20),
            }],
            skill_discovery_override: None,
            skill_discovery_skills: Vec::new(),
            extra_prompts: Vec::new(),
            tool_guidelines_override: None,
            tool_selections: Vec::new(),
            dynamic_environment_sources: Vec::new(),
        },
        &scope_state! {
            scope: PromptAssemblyScope::Project,
            core_system_override: None,
            entries: vec![PersistedPromptAssemblyEntry {
                reference_id: "repo-rules".to_string(),
                kind: PromptSourceKind::ExtraPrompt,
                title: "repo-rules".to_string(),
                enabled: true,
                requested_order: Some(10),
            }],
            skill_discovery_override: None,
            skill_discovery_skills: Vec::new(),
            extra_prompts: vec![StoredPromptBody {
                reference_id: "repo-rules".to_string(),
                title: "repo-rules".to_string(),
                body: "project rules".to_string(),
            }],
            tool_guidelines_override: None,
            tool_selections: Vec::new(),
            dynamic_environment_sources: Vec::new(),
        },
        None,
        Some(&global_instructions_path),
    );

    assert_eq!(
        prelude
            .sections
            .iter()
            .map(|section| section.kind)
            .collect::<Vec<_>>(),
        vec![
            PromptSourceKind::CoreSystemPrompt,
            PromptSourceKind::ExtraPrompt,
            PromptSourceKind::DynamicEnvironmentBaseline,
            PromptSourceKind::InstructionsFile,
            PromptSourceKind::SkillDiscovery,
            PromptSourceKind::InstructionsFile,
            PromptSourceKind::InstructionsFile,
            PromptSourceKind::InstructionsFile,
        ]
    );
    let effective = prelude
        .effective_system_prompt()
        .expect("effective prompt should exist");
    assert!(
        effective.starts_with("global core\n\nproject rules\n\n"),
        "explicitly ordered project prompt should stay ahead of discovered instructions: {effective}"
    );
    assert!(
        effective.contains("Environment baseline for this session:"),
        "static dynamic environment baseline should participate in prompt prelude ordering: {effective}"
    );
    assert!(
        !effective.contains("outside instructions"),
        "project discovery should stop at git root: {effective}"
    );
    assert!(
        !effective.contains("crate claude"),
        "AGENTS.md should win over CLAUDE.md in the same directory: {effective}"
    );
}
#[test]
fn resolve_initial_prompt_assembly_keeps_inactive_sources_for_manager_view() {
    let work_dir = temp_dir("snapshot");
    let resolved = resolve_prompt_assembly_manager_snapshot(
        &work_dir,
        &scope_state! {
            scope: PromptAssemblyScope::Global,
            core_system_override: None,
            entries: vec![
                PersistedPromptAssemblyEntry {
                    reference_id: "disabled".to_string(),
                    kind: PromptSourceKind::ExtraPrompt,
                    title: "disabled".to_string(),
                    enabled: false,
                    requested_order: Some(10),
                },
                PersistedPromptAssemblyEntry {
                    reference_id: "missing".to_string(),
                    kind: PromptSourceKind::LongLivedSkill,
                    title: "missing".to_string(),
                    enabled: true,
                    requested_order: Some(20),
                },
            ],
            skill_discovery_override: None,
            skill_discovery_skills: Vec::new(),
            extra_prompts: Vec::new(),
            tool_guidelines_override: None,
            tool_selections: Vec::new(),
            dynamic_environment_sources: Vec::new(),
        },
        &PromptAssemblyScopeState::new(PromptAssemblyScope::Project),
        &[],
    );

    assert_eq!(resolved.resolution.assembly.active_sources.len(), 5);
    assert_eq!(
        resolved
            .resolution
            .assembly
            .active_sources
            .iter()
            .map(|source| source.reference_id.as_str())
            .collect::<Vec<_>>(),
        vec![
            "core-system",
            "tool-guidelines",
            "env-baseline",
            "env-changes",
            "skill-discovery",
        ]
    );
    assert_eq!(
        resolved
            .resolution
            .assembly
            .inactive_sources
            .iter()
            .map(|source| source.reference_id.as_str())
            .collect::<Vec<_>>(),
        vec!["disabled", "missing"]
    );
}

#[test]
fn dynamic_environment_session_config_uses_static_baseline_observations_from_manager() {
    let work_dir = temp_dir("dynamic-environment-baseline-config");
    let manager = resolve_prompt_assembly_manager_snapshot(
        &work_dir,
        &scope_state! {
            scope: PromptAssemblyScope::Global,
            core_system_override: None,
            entries: vec![],
            skill_discovery_override: None,
            skill_discovery_skills: Vec::new(),
            extra_prompts: Vec::new(),
            tool_guidelines_override: None,
            tool_selections: Vec::new(),
            dynamic_environment_sources: vec![
                runtime_domain::dynamic_environment::DynamicEnvironmentSourceSelection {
                    snapshot_kind: runtime_domain::dynamic_environment::DynamicEnvironmentSnapshotKind::Baseline,
                    source_kind: runtime_domain::dynamic_environment::DynamicEnvironmentSourceKind::Date,
                    enabled: true,
                },
                runtime_domain::dynamic_environment::DynamicEnvironmentSourceSelection {
                    snapshot_kind: runtime_domain::dynamic_environment::DynamicEnvironmentSnapshotKind::Changes,
                    source_kind: runtime_domain::dynamic_environment::DynamicEnvironmentSourceKind::Date,
                    enabled: false,
                },
            ],
        },
        &PromptAssemblyScopeState::new(PromptAssemblyScope::Project),
        &[],
    );

    let session_config = dynamic_environment_session_config_from_manager(&manager);

    assert!(session_config.baseline_enabled);
    assert!(
        session_config
            .static_baseline_observations
            .iter()
            .any(|observation| {
                observation.source_kind
                    == runtime_domain::dynamic_environment::DynamicEnvironmentSourceKind::Date
            })
    );
}
#[test]
fn resolve_manager_snapshot_injects_default_skill_discovery_source_with_generated_body() {
    let work_dir = temp_dir("default-skill-discovery");
    let project_skill_dir = work_dir.join(".agents/skills/repo-bootstrap");
    fs::create_dir_all(&project_skill_dir).expect("skill dir should exist");
    fs::write(
            project_skill_dir.join(SKILL_FILE_NAME),
            "---\nname: repo-bootstrap\ndescription: Bootstrap repo\ndisable-model-invocation: false\n---\n# Repo Bootstrap\n\nUse this skill.\n",
        )
        .expect("skill file should exist");

    let resolved = resolve_prompt_assembly_manager_snapshot(
        &work_dir,
        &PromptAssemblyScopeState::new(PromptAssemblyScope::Global),
        &PromptAssemblyScopeState::new(PromptAssemblyScope::Project),
        &[],
    );

    let managed_skill_discovery = resolved
        .sources
        .managed
        .iter()
        .find(|source| source.kind == PromptSourceKind::SkillDiscovery)
        .expect("default skill discovery source should exist");
    assert_eq!(managed_skill_discovery.reference_id, "skill-discovery");

    let materialized_skill_discovery = resolved
        .sources
        .preview
        .iter()
        .find(|source| {
            source.kind == PromptSourceKind::SkillDiscovery
                && source.reference_id == "skill-discovery"
        })
        .expect("materialized skill discovery source should exist");
    assert!(
        materialized_skill_discovery
            .body
            .as_deref()
            .expect("skill discovery body should exist")
            .contains("<available_skills>")
    );
    assert!(
        materialized_skill_discovery
            .body
            .as_deref()
            .expect("skill discovery body should exist")
            .contains("<name>repo-bootstrap</name>")
    );
}
#[test]
fn resolve_manager_snapshot_places_tool_guidelines_after_core_and_marks_it_builtin() {
    let work_dir = temp_dir("default-tool-guidelines");

    let resolved = resolve_prompt_assembly_manager_snapshot(
        &work_dir,
        &PromptAssemblyScopeState::new(PromptAssemblyScope::Global),
        &PromptAssemblyScopeState::new(PromptAssemblyScope::Project),
        &builtin_tool_definitions(),
    );

    assert_eq!(
        resolved
            .resolution
            .assembly
            .active_sources
            .iter()
            .map(|source| (source.reference_id.as_str(), source.origin))
            .collect::<Vec<_>>(),
        vec![
            ("core-system", Some(PromptSourceOrigin::Builtin)),
            ("tool-guidelines", Some(PromptSourceOrigin::Builtin)),
            ("env-baseline", Some(PromptSourceOrigin::Builtin)),
            ("env-changes", Some(PromptSourceOrigin::Builtin)),
            ("skill-discovery", Some(PromptSourceOrigin::Project)),
        ]
    );
    let managed_tool_guidelines = resolved
        .sources
        .managed
        .iter()
        .find(|source| source.reference_id == "tool-guidelines")
        .expect("tool guidelines should be visible in manager list");
    assert_eq!(
        managed_tool_guidelines.origin,
        Some(PromptSourceOrigin::Builtin)
    );
    assert_eq!(
        managed_tool_guidelines.scope,
        Some(PromptAssemblyScope::Global)
    );
    assert_eq!(managed_tool_guidelines.order, 2);
}
#[test]
fn active_long_lived_skill_prefers_project_skill_when_names_collide() {
    let work_dir = temp_dir("skill-precedence");
    let project_skill_dir = work_dir.join(".agents/skills/repo-bootstrap");
    fs::create_dir_all(&project_skill_dir).expect("project skill dir should exist");
    fs::write(
            project_skill_dir.join(SKILL_FILE_NAME),
            "---\nname: repo-bootstrap\ndescription: Project bootstrap\n---\n# Project Bootstrap\n\nproject body\n",
        )
        .expect("project skill file should exist");

    let home_dir = temp_dir("skill-precedence-home");
    let global_skill_dir = home_dir.join(".agents/skills/repo-bootstrap");
    fs::create_dir_all(&global_skill_dir).expect("global skill dir should exist");
    fs::write(
            global_skill_dir.join(SKILL_FILE_NAME),
            "---\nname: repo-bootstrap\ndescription: Global bootstrap\n---\n# Global Bootstrap\n\nglobal body\n",
        )
        .expect("global skill file should exist");
    let snapshot = resolve_prompt_assembly_manager_snapshot_with_global_skill_root(
        &work_dir,
        &scope_state! {
            scope: PromptAssemblyScope::Global,
            core_system_override: None,
            entries: vec![PersistedPromptAssemblyEntry {
                reference_id: "repo-bootstrap".to_string(),
                kind: PromptSourceKind::LongLivedSkill,
                title: "repo-bootstrap".to_string(),
                enabled: true,
                requested_order: Some(10),
            }],
            skill_discovery_override: None,
            skill_discovery_skills: Vec::new(),
            extra_prompts: Vec::new(),
            tool_guidelines_override: None,
            tool_selections: Vec::new(),
            dynamic_environment_sources: Vec::new(),
        },
        &PromptAssemblyScopeState::new(PromptAssemblyScope::Project),
        Some(&home_dir.join(".agents").join("skills")),
        &[],
    );

    let skill_source = snapshot
        .sources
        .preview
        .iter()
        .find(|source| {
            source.kind == PromptSourceKind::LongLivedSkill
                && source.reference_id == "repo-bootstrap"
        })
        .expect("long-lived skill source should exist");
    assert_eq!(skill_source.origin, Some(PromptSourceOrigin::Global));
    assert_eq!(
        skill_source.resolved_body_origin,
        Some(PromptSourceOrigin::Project)
    );
    assert!(
        skill_source
            .body
            .as_deref()
            .expect("skill body should exist")
            .contains("project body")
    );
}
#[test]
fn missing_source_check_counts_missing_entries_without_blocking_snapshot_resolution() {
    let manager = resolve_prompt_assembly_manager_snapshot(
        &temp_dir("missing-check"),
        &scope_state! {
            scope: PromptAssemblyScope::Global,
            core_system_override: None,
            entries: vec![
                PersistedPromptAssemblyEntry {
                    reference_id: "missing-skill".to_string(),
                    kind: PromptSourceKind::LongLivedSkill,
                    title: "missing-skill".to_string(),
                    enabled: true,
                    requested_order: Some(10),
                },
                PersistedPromptAssemblyEntry {
                    reference_id: "disabled-extra".to_string(),
                    kind: PromptSourceKind::ExtraPrompt,
                    title: "disabled-extra".to_string(),
                    enabled: false,
                    requested_order: Some(20),
                },
            ],
            skill_discovery_override: None,
            skill_discovery_skills: Vec::new(),
            extra_prompts: Vec::new(),
            tool_guidelines_override: None,
            tool_selections: Vec::new(),
            dynamic_environment_sources: Vec::new(),
        },
        &PromptAssemblyScopeState::new(PromptAssemblyScope::Project),
        &[],
    );

    let check = PromptAssemblyMissingSourcesCheck::from_manager(&manager);

    assert_eq!(check.missing_count, 1);
    assert!(
        manager
            .resolution
            .assembly
            .inactive_sources
            .iter()
            .any(|source| {
                source.reference_id == "missing-skill"
                    && matches!(
                        source.status,
                        PromptSourceStatus::Inactive {
                            reason:
                                runtime_domain::prompt_assembly::PromptSourceInactiveReason::Missing
                        }
                    )
            })
    );
}
