use super::*;

#[test]
fn disabling_default_tool_guidelines_materializes_builtin_entry_in_global_state() {
    let work_dir = temp_dir("disable-tool-guidelines");
    let store: Arc<dyn SessionStore> = Arc::new(InMemorySessionStore::new());

    apply_prompt_assembly_mutation(
        store.clone(),
        &work_dir,
        PromptAssemblyMutation::scoped(
            PromptAssemblyScope::Global,
            PromptAssemblyScopedMutationKind::SetPromptSourceEnabled {
                kind: PromptSourceKind::ToolGuidelines,
                reference_id: "tool-guidelines".to_string(),
                enabled: false,
            },
        ),
        &builtin_tool_definitions(),
    )
    .expect("disable should succeed");

    let global_state = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime should build")
        .block_on(store.load_global_prompt_assembly_state())
        .expect("global prompt assembly state should load");
    assert!(
        global_state.entries().iter().any(|entry| {
            entry.kind == PromptSourceKind::ToolGuidelines
                && entry.reference_id == "tool-guidelines"
                && !entry.enabled
        }),
        "tool guidelines should be materialized as a disabled builtin entry"
    );
}
#[test]
fn disabling_default_dynamic_environment_changes_keeps_baseline_visible() {
    let work_dir = temp_dir("disable-dynamic-changes");
    let store: Arc<dyn SessionStore> = Arc::new(InMemorySessionStore::new());

    let disabled_snapshot = apply_prompt_assembly_mutation(
        store.clone(),
        &work_dir,
        PromptAssemblyMutation::scoped(
            PromptAssemblyScope::Global,
            PromptAssemblyScopedMutationKind::SetPromptSourceEnabled {
                kind: PromptSourceKind::DynamicEnvironmentChanges,
                reference_id: "env-changes".to_string(),
                enabled: false,
            },
        ),
        &[],
    )
    .expect("disable should succeed");

    assert!(disabled_snapshot.sources.managed.iter().any(|source| {
        source.kind == PromptSourceKind::DynamicEnvironmentBaseline && source.enabled
    }));
    assert!(disabled_snapshot.sources.managed.iter().any(|source| {
        source.kind == PromptSourceKind::DynamicEnvironmentChanges && !source.enabled
    }));
    assert!(
        disabled_snapshot
            .resolution
            .assembly
            .active_sources
            .iter()
            .any(|source| {
                source.kind == PromptSourceKind::DynamicEnvironmentBaseline
                    && source.reference_id == "env-baseline"
            })
    );
    assert!(
        disabled_snapshot
            .resolution
            .assembly
            .inactive_sources
            .iter()
            .any(|source| {
                source.kind == PromptSourceKind::DynamicEnvironmentChanges
                    && source.reference_id == "env-changes"
                    && matches!(
                        source.status,
                        PromptSourceStatus::Inactive {
                            reason: PromptSourceInactiveReason::Disabled
                        }
                    )
            })
    );
}
#[test]
fn dynamic_environment_prompt_source_stays_visible_after_disable_and_can_be_restored() {
    let work_dir = temp_dir("toggle-dynamic-prompt-source");
    let store: Arc<dyn SessionStore> = Arc::new(InMemorySessionStore::new());

    let disabled_snapshot = apply_prompt_assembly_mutation(
        store.clone(),
        &work_dir,
        PromptAssemblyMutation::scoped(
            PromptAssemblyScope::Global,
            PromptAssemblyScopedMutationKind::SetPromptSourceEnabled {
                kind: PromptSourceKind::DynamicEnvironmentBaseline,
                reference_id: "env-baseline".to_string(),
                enabled: false,
            },
        ),
        &[],
    )
    .expect("disable should succeed");

    assert!(disabled_snapshot.sources.managed.iter().any(|source| {
        source.kind == PromptSourceKind::DynamicEnvironmentBaseline && !source.enabled
    }));
    assert!(disabled_snapshot.sources.managed.iter().any(|source| {
        source.kind == PromptSourceKind::DynamicEnvironmentChanges && source.enabled
    }));

    let restored_snapshot = apply_prompt_assembly_mutation(
        store,
        &work_dir,
        PromptAssemblyMutation::scoped(
            PromptAssemblyScope::Global,
            PromptAssemblyScopedMutationKind::SetPromptSourceEnabled {
                kind: PromptSourceKind::DynamicEnvironmentBaseline,
                reference_id: "env-baseline".to_string(),
                enabled: true,
            },
        ),
        &[],
    )
    .expect("re-enable should succeed");

    assert!(restored_snapshot.sources.managed.iter().any(|source| {
        source.kind == PromptSourceKind::DynamicEnvironmentBaseline && source.enabled
    }));
}
#[test]
fn moving_default_dynamic_environment_source_reorders_managed_list() {
    let work_dir = temp_dir("move-dynamic-environment");
    let store: Arc<dyn SessionStore> = Arc::new(InMemorySessionStore::new());

    let snapshot = apply_prompt_assembly_mutation(
        store,
        &work_dir,
        PromptAssemblyMutation::scoped(
            PromptAssemblyScope::Global,
            PromptAssemblyScopedMutationKind::MoveActiveSource {
                kind: PromptSourceKind::DynamicEnvironmentBaseline,
                reference_id: "env-baseline".to_string(),
                direction: PromptAssemblyMoveDirection::Down,
            },
        ),
        &[],
    )
    .expect("move should succeed");

    assert_eq!(
        snapshot
            .sources
            .managed
            .iter()
            .map(|source| source.reference_id.as_str())
            .collect::<Vec<_>>(),
        vec![
            "core-system",
            "tool-guidelines",
            "env-changes",
            "env-baseline",
            "skill-discovery",
        ]
    );
}
#[test]
fn moving_default_instruction_file_materializes_and_reorders_project_entry() {
    let work_dir = temp_dir("move-discovered-instructions");
    fs::write(work_dir.join("AGENTS.md"), "project instructions\n")
        .expect("project instructions should write");
    let store: Arc<dyn SessionStore> = Arc::new(InMemorySessionStore::new());

    let snapshot = apply_prompt_assembly_mutation(
        store,
        &work_dir,
        PromptAssemblyMutation::scoped(
            PromptAssemblyScope::Project,
            PromptAssemblyScopedMutationKind::MoveActiveSource {
                kind: PromptSourceKind::InstructionsFile,
                reference_id: "instructions:project:.".to_string(),
                direction: PromptAssemblyMoveDirection::Down,
            },
        ),
        &builtin_tool_definitions(),
    )
    .expect("move should succeed");

    assert_eq!(
        snapshot
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
            "instructions:project:."
        ]
    );

    let project_state = load_project_prompt_assembly_state(&work_dir)
        .expect("project prompt assembly state should load");
    assert!(
        project_state.entries().iter().any(|entry| {
            entry.kind == PromptSourceKind::InstructionsFile
                && entry.reference_id == "instructions:project:."
        }),
        "moving a discovered instruction file should persist an explicit entry"
    );
}
#[test]
fn save_skill_discovery_override_rebuilds_generated_block_and_preserves_appended_suffix() {
    let work_dir = temp_dir("skill-discovery-override");
    let skill_dir = work_dir.join(".agents/skills/repo-bootstrap");
    fs::create_dir_all(&skill_dir).expect("skill dir should exist");
    fs::write(
        skill_dir.join(SKILL_FILE_NAME),
        "---\nname: repo-bootstrap\ndescription: Bootstrap repo\n---\n# Repo Bootstrap\n",
    )
    .expect("skill file should exist");

    let store: Arc<dyn SessionStore> = Arc::new(InMemorySessionStore::new());
    let original = format!(
        "{SKILL_DISCOVERY_GENERATED_START}\nold generated\n{SKILL_DISCOVERY_GENERATED_END}\n\n## Notes\nkeep this suffix"
    );
    apply_prompt_assembly_mutation(
        store.clone(),
        &work_dir,
        PromptAssemblyMutation::SaveEditorTarget {
            target: PromptAssemblyEditorTarget::SkillDiscovery {
                scope: PromptAssemblyScope::Project,
            },
            content: original,
        },
        &[],
    )
    .expect("save should succeed");

    let loaded = PromptAssemblyWorkspace::new(&work_dir, &[])
        .load_manager(store)
        .expect("snapshot should load");
    let skill_discovery = loaded
        .sources
        .preview
        .iter()
        .find(|source| source.kind == PromptSourceKind::SkillDiscovery)
        .and_then(|source| source.body.as_deref())
        .expect("skill discovery body should exist");

    assert!(skill_discovery.contains(SKILL_DISCOVERY_GENERATED_START));
    assert!(skill_discovery.contains(SKILL_DISCOVERY_GENERATED_END));
    assert!(skill_discovery.contains("<available_skills>"));
    assert!(skill_discovery.contains("## Notes\nkeep this suffix"));
}
#[test]
fn save_skill_discovery_override_follows_effective_scope() {
    let work_dir = temp_dir("skill-discovery-effective-scope");
    let store: Arc<dyn SessionStore> = Arc::new(InMemorySessionStore::new());

    save_project_prompt_assembly_state(
        &work_dir,
        &scope_state! {
            scope: PromptAssemblyScope::Project,
            core_system_override: None,
            entries: vec![PersistedPromptAssemblyEntry {
                reference_id: "skill-discovery".to_string(),
                kind: PromptSourceKind::SkillDiscovery,
                title: "Skill discovery".to_string(),
                enabled: true,
                requested_order: Some(30),
            }],
            skill_discovery_override: None,
            skill_discovery_skills: Vec::new(),
            extra_prompts: Vec::new(),
            tool_guidelines_override: None,
            tool_selections: Vec::new(),
            dynamic_environment_sources: Vec::new(),
        },
    )
    .expect("project state should save");

    apply_prompt_assembly_mutation(
        store,
        &work_dir,
        PromptAssemblyMutation::SaveEditorTarget {
            target: PromptAssemblyEditorTarget::SkillDiscovery {
                scope: PromptAssemblyScope::Global,
            },
            content: "project discovery override".to_string(),
        },
        &[],
    )
    .expect("save should succeed");

    let project_state = load_project_prompt_assembly_state(&work_dir)
        .expect("project prompt assembly state should load");
    assert_eq!(
        project_state.skill_discovery_override(),
        Some("project discovery override")
    );
}
#[test]
fn moving_discovered_skill_normalizes_requested_order_to_dense_sequence() {
    let mut state = scope_state! {
        scope: PromptAssemblyScope::Project,
        core_system_override: None,
        entries: Vec::new(),
        skill_discovery_override: None,
        skill_discovery_skills: vec![
            PersistedSkillDiscoverySkillEntry {
                skill_name: "repo-bootstrap".to_string(),
                enabled: true,
                requested_order: Some(10),
            },
            PersistedSkillDiscoverySkillEntry {
                skill_name: "code-review".to_string(),
                enabled: true,
                requested_order: Some(20),
            },
        ],
        extra_prompts: Vec::new(),
        tool_guidelines_override: None,
        tool_selections: Vec::new(),
        dynamic_environment_sources: Vec::new(),
    };

    move_discovered_skill(&mut state, "code-review", PromptAssemblyMoveDirection::Up)
        .expect("move should succeed");

    assert_eq!(
        state
            .skill_discovery_skills()
            .iter()
            .map(|entry| (entry.skill_name.as_str(), entry.requested_order))
            .collect::<Vec<_>>(),
        vec![("code-review", Some(1)), ("repo-bootstrap", Some(2)),]
    );
}
#[test]
fn selecting_discovered_skill_persists_requested_order_from_one() {
    let work_dir = temp_dir("select-skill-order");
    let repo_bootstrap_dir = work_dir.join(".agents/skills/repo-bootstrap");
    fs::create_dir_all(&repo_bootstrap_dir).expect("repo-bootstrap dir should exist");
    fs::write(
        repo_bootstrap_dir.join(SKILL_FILE_NAME),
        "---\nname: repo-bootstrap\ndescription: Bootstrap repo\n---\n# Repo Bootstrap\n",
    )
    .expect("repo-bootstrap skill should write");

    let store: Arc<dyn SessionStore> = Arc::new(InMemorySessionStore::new());
    apply_prompt_assembly_mutation(
        store,
        &work_dir,
        PromptAssemblyMutation::scoped(
            PromptAssemblyScope::Project,
            PromptAssemblyScopedMutationKind::SetDiscoveredSkillSelected {
                skill_name: "repo-bootstrap".to_string(),
                selected: true,
            },
        ),
        &[],
    )
    .expect("selection should succeed");

    let project_state = load_project_prompt_assembly_state(&work_dir)
        .expect("project prompt assembly state should load");
    assert_eq!(
        project_state.skill_discovery_skills().first(),
        Some(&PersistedSkillDiscoverySkillEntry {
            skill_name: "repo-bootstrap".to_string(),
            enabled: true,
            requested_order: Some(1),
        })
    );
}
#[test]
fn moving_default_discovered_skill_materializes_dense_project_order() {
    let work_dir = temp_dir("move-default-discovered-skill");
    let repo_bootstrap_dir = work_dir.join(".agents/skills/repo-bootstrap");
    fs::create_dir_all(&repo_bootstrap_dir).expect("repo-bootstrap dir should exist");
    fs::write(
        repo_bootstrap_dir.join(SKILL_FILE_NAME),
        "---\nname: repo-bootstrap\ndescription: Bootstrap repo\n---\n# Repo Bootstrap\n",
    )
    .expect("repo-bootstrap skill should write");
    let code_review_dir = work_dir.join(".agents/skills/code-review");
    fs::create_dir_all(&code_review_dir).expect("code-review dir should exist");
    fs::write(
        code_review_dir.join(SKILL_FILE_NAME),
        "---\nname: code-review\ndescription: Review code\n---\n# Code Review\n",
    )
    .expect("code-review skill should write");
    let store: Arc<dyn SessionStore> = Arc::new(InMemorySessionStore::new());

    apply_prompt_assembly_mutation(
        store,
        &work_dir,
        PromptAssemblyMutation::scoped(
            PromptAssemblyScope::Project,
            PromptAssemblyScopedMutationKind::MoveDiscoveredSkill {
                skill_name: "code-review".to_string(),
                direction: PromptAssemblyMoveDirection::Up,
            },
        ),
        &[],
    )
    .expect("move should succeed");

    let project_state = load_project_prompt_assembly_state(&work_dir)
        .expect("project prompt assembly state should load");
    assert_eq!(
        project_state
            .skill_discovery_skills()
            .iter()
            .take(2)
            .cloned()
            .collect::<Vec<_>>(),
        vec![
            PersistedSkillDiscoverySkillEntry {
                skill_name: "code-review".to_string(),
                enabled: true,
                requested_order: Some(1),
            },
            PersistedSkillDiscoverySkillEntry {
                skill_name: "repo-bootstrap".to_string(),
                enabled: true,
                requested_order: Some(2),
            },
        ]
    );
}
#[test]
fn resetting_discovered_skill_order_restores_default_discovery_order() {
    let work_dir = temp_dir("reset-discovered-skill-order");
    let repo_bootstrap_dir = work_dir.join(".agents/skills/repo-bootstrap");
    fs::create_dir_all(&repo_bootstrap_dir).expect("repo-bootstrap dir should exist");
    fs::write(
        repo_bootstrap_dir.join(SKILL_FILE_NAME),
        "---\nname: repo-bootstrap\ndescription: Bootstrap repo\n---\n# Repo Bootstrap\n",
    )
    .expect("repo-bootstrap skill should write");
    let code_review_dir = work_dir.join(".agents/skills/code-review");
    fs::create_dir_all(&code_review_dir).expect("code-review dir should exist");
    fs::write(
        code_review_dir.join(SKILL_FILE_NAME),
        "---\nname: code-review\ndescription: Review code\n---\n# Code Review\n",
    )
    .expect("code-review skill should write");

    let default_snapshot = resolve_prompt_assembly_manager_snapshot(
        &work_dir,
        &PromptAssemblyScopeState::new(PromptAssemblyScope::Global),
        &PromptAssemblyScopeState::new(PromptAssemblyScope::Project),
        &[],
    );
    let default_order = default_snapshot
        .candidates
        .discovered_skills
        .iter()
        .map(|skill| skill.skill_name.clone())
        .collect::<Vec<_>>();
    assert!(
        default_order.len() >= 2,
        "fixture should expose at least two discovered skills"
    );

    save_project_prompt_assembly_state(
        &work_dir,
        &scope_state! {
            scope: PromptAssemblyScope::Project,
            core_system_override: None,
            entries: Vec::new(),
            skill_discovery_override: None,
            skill_discovery_skills: default_order
                .iter()
                .rev()
                .enumerate()
                .map(|(index, skill_name)| PersistedSkillDiscoverySkillEntry {
                    skill_name: skill_name.clone(),
                    enabled: index != 0,
                    requested_order: Some(u16::try_from((index + 1) * 10).unwrap_or(u16::MAX)),
                })
                .collect(),
            extra_prompts: Vec::new(),
            tool_guidelines_override: None,
            tool_selections: Vec::new(),
            dynamic_environment_sources: Vec::new(),
        },
    )
    .expect("project prompt assembly should save");

    let store: Arc<dyn SessionStore> = Arc::new(InMemorySessionStore::new());
    apply_prompt_assembly_mutation(
        store,
        &work_dir,
        PromptAssemblyMutation::scoped(
            PromptAssemblyScope::Project,
            PromptAssemblyScopedMutationKind::ResetDiscoveredSkillOrder,
        ),
        &[],
    )
    .expect("reset should succeed");

    let project_state = load_project_prompt_assembly_state(&work_dir)
        .expect("project prompt assembly state should load");
    assert_eq!(
        project_state
            .skill_discovery_skills()
            .iter()
            .map(|entry| entry.skill_name.as_str())
            .collect::<Vec<_>>(),
        default_order.iter().map(String::as_str).collect::<Vec<_>>()
    );
    assert_eq!(
        project_state
            .skill_discovery_skills()
            .iter()
            .map(|entry| entry.requested_order)
            .collect::<Vec<_>>(),
        (1..=default_order.len())
            .map(|index| Some(u16::try_from(index).unwrap_or(u16::MAX)))
            .collect::<Vec<_>>()
    );
    assert_eq!(
        project_state
            .skill_discovery_skills()
            .iter()
            .map(|entry| entry.enabled)
            .collect::<Vec<_>>(),
        default_order
            .iter()
            .map(|skill_name| {
                let reversed_index = default_order
                    .iter()
                    .rev()
                    .position(|candidate| candidate == skill_name)
                    .expect("skill should exist in reversed fixture");
                reversed_index != 0
            })
            .collect::<Vec<_>>()
    );
}
#[test]
fn moving_default_tool_materializes_dense_global_order() {
    let work_dir = temp_dir("move-default-tool");
    let store: Arc<dyn SessionStore> = Arc::new(InMemorySessionStore::new());

    apply_prompt_assembly_mutation(
        store.clone(),
        &work_dir,
        PromptAssemblyMutation::scoped(
            PromptAssemblyScope::Global,
            PromptAssemblyScopedMutationKind::MoveTool {
                tool_name: "read_file".to_string(),
                direction: PromptAssemblyMoveDirection::Up,
            },
        ),
        &builtin_tool_definitions(),
    )
    .expect("move should succeed");

    let global_state = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime should build")
        .block_on(store.load_global_prompt_assembly_state())
        .expect("global prompt assembly state should load");
    assert_eq!(
        global_state.tool_selections(),
        vec![
            PersistedToolSelectionEntry {
                tool_name: "read_file".to_string(),
                enabled: true,
                requested_order: Some(1),
            },
            PersistedToolSelectionEntry {
                tool_name: "bash".to_string(),
                enabled: true,
                requested_order: Some(2),
            },
        ]
    );
}
#[test]
fn moving_tool_ignores_unguided_registry_entries_when_materializing_order() {
    let work_dir = temp_dir("move-tool-ignore-unguided");
    let store: Arc<dyn SessionStore> = Arc::new(InMemorySessionStore::new());

    apply_prompt_assembly_mutation(
        store.clone(),
        &work_dir,
        PromptAssemblyMutation::scoped(
            PromptAssemblyScope::Global,
            PromptAssemblyScopedMutationKind::MoveTool {
                tool_name: "read_file".to_string(),
                direction: PromptAssemblyMoveDirection::Up,
            },
        ),
        &tool_definitions_with_unguided_tool(),
    )
    .expect("move should succeed");

    let global_state = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime should build")
        .block_on(store.load_global_prompt_assembly_state())
        .expect("global prompt assembly state should load");
    assert_eq!(
        global_state.tool_selections(),
        vec![
            PersistedToolSelectionEntry {
                tool_name: "read_file".to_string(),
                enabled: true,
                requested_order: Some(1),
            },
            PersistedToolSelectionEntry {
                tool_name: "bash".to_string(),
                enabled: true,
                requested_order: Some(2),
            },
        ]
    );
}
#[test]
fn disabling_skill_discovery_materializes_disabled_entry_in_selected_scope() {
    let work_dir = temp_dir("disable-skill-discovery");
    let store: Arc<dyn SessionStore> = Arc::new(InMemorySessionStore::new());

    apply_prompt_assembly_mutation(
        store,
        &work_dir,
        PromptAssemblyMutation::scoped(
            PromptAssemblyScope::Project,
            PromptAssemblyScopedMutationKind::SetPromptSourceEnabled {
                kind: PromptSourceKind::SkillDiscovery,
                reference_id: "skill-discovery".to_string(),
                enabled: false,
            },
        ),
        &[],
    )
    .expect("disable should succeed");

    let project_state = load_project_prompt_assembly_state(&work_dir)
        .expect("project prompt assembly state should load");
    assert!(
        project_state.entries().iter().any(|entry| {
            entry.kind == PromptSourceKind::SkillDiscovery
                && entry.reference_id == "skill-discovery"
                && !entry.enabled
        }),
        "skill discovery entry should be materialized as disabled"
    );
}
#[test]
fn activate_long_lived_skill_persists_reference_and_expands_in_prelude() {
    let work_dir = temp_dir("activate-skill");
    let skill_dir = work_dir.join(".agents/skills/repo-bootstrap");
    fs::create_dir_all(&skill_dir).expect("skill dir should exist");
    fs::write(
            skill_dir.join(SKILL_FILE_NAME),
            "---\nname: repo-bootstrap\ndescription: Bootstrap repo\n---\n# Repo Bootstrap\n\nUse this skill.\n",
        )
        .expect("skill file should exist");

    let store: Arc<dyn SessionStore> = Arc::new(InMemorySessionStore::new());
    let snapshot = apply_prompt_assembly_mutation(
        store.clone(),
        &work_dir,
        PromptAssemblyMutation::scoped(
            PromptAssemblyScope::Project,
            PromptAssemblyScopedMutationKind::ActivateLongLivedSkill {
                skill_name: "repo-bootstrap".to_string(),
            },
        ),
        &[],
    )
    .expect("mutation should succeed");

    assert_eq!(
        snapshot
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
            "repo-bootstrap"
        ]
    );
    assert!(
        snapshot
            .candidates
            .discovered_skills
            .iter()
            .any(|skill| skill.skill_name == "repo-bootstrap" && skill.selection.is_selected())
    );
    assert!(
        snapshot
            .resolution
            .prelude
            .effective_system_prompt()
            .expect("effective prompt should exist")
            .contains("<name>repo-bootstrap</name>")
    );

    let project_state = load_project_prompt_assembly_state(&work_dir)
        .expect("project prompt assembly state should load");
    assert_eq!(
        project_state.entries(),
        vec![PersistedPromptAssemblyEntry {
            reference_id: "repo-bootstrap".to_string(),
            kind: PromptSourceKind::LongLivedSkill,
            title: "repo-bootstrap".to_string(),
            enabled: true,
            requested_order: Some(40),
        }]
    );
}
#[test]
fn move_active_source_reorders_non_core_entries() {
    let work_dir = temp_dir("move-order");
    let store: Arc<dyn SessionStore> = Arc::new(InMemorySessionStore::new());
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime should build");
    runtime
        .block_on(store.save_global_prompt_assembly_state(&scope_state! {
            scope: PromptAssemblyScope::Global,
            core_system_override: None,
            entries: vec![PersistedPromptAssemblyEntry {
                reference_id: "shared-rules".to_string(),
                kind: PromptSourceKind::ExtraPrompt,
                title: "shared-rules".to_string(),
                enabled: true,
                requested_order: Some(10),
            }],
            skill_discovery_override: None,
            skill_discovery_skills: Vec::new(),
            extra_prompts: vec![StoredPromptBody {
                reference_id: "shared-rules".to_string(),
                title: "shared-rules".to_string(),
                body: "global rules".to_string(),
            }],
            tool_guidelines_override: None,
            tool_selections: Vec::new(),
            dynamic_environment_sources: Vec::new(),
        }))
        .expect("global state should save");
    save_project_prompt_assembly_state(
        &work_dir,
        &scope_state! {
            scope: PromptAssemblyScope::Project,
            core_system_override: None,
            entries: vec![PersistedPromptAssemblyEntry {
                reference_id: "repo-rules".to_string(),
                kind: PromptSourceKind::ExtraPrompt,
                title: "repo-rules".to_string(),
                enabled: true,
                requested_order: Some(20),
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
    )
    .expect("project state should save");

    let snapshot = apply_prompt_assembly_mutation(
        store.clone(),
        &work_dir,
        PromptAssemblyMutation::scoped(
            PromptAssemblyScope::Project,
            PromptAssemblyScopedMutationKind::MoveActiveSource {
                kind: PromptSourceKind::ExtraPrompt,
                reference_id: "repo-rules".to_string(),
                direction: PromptAssemblyMoveDirection::Up,
            },
        ),
        &[],
    )
    .expect("move should succeed");

    assert_eq!(
        snapshot
            .resolution
            .assembly
            .active_sources
            .iter()
            .map(|source| source.reference_id.as_str())
            .collect::<Vec<_>>(),
        vec![
            "core-system",
            "shared-rules",
            "tool-guidelines",
            "env-baseline",
            "repo-rules",
            "env-changes",
            "skill-discovery",
        ]
    );
}
#[test]
fn stale_prompt_entry_address_is_ignored() {
    let mut global_state = PromptAssemblyScopeState::new(PromptAssemblyScope::Global);
    let mut project_state = PromptAssemblyScopeState::new(PromptAssemblyScope::Project);
    let stale_address = PromptEntryAddress {
        scope: PromptAssemblyScope::Global,
        index: 0,
    };

    assert!(entry_ref(&global_state, &project_state, stale_address).is_none());
    assert_eq!(
        entry_requested_order(&global_state, &project_state, stale_address),
        None
    );

    set_entry_requested_order(
        &mut global_state,
        &mut project_state,
        stale_address,
        Some(10),
    );

    assert!(global_state.entries().is_empty());
    assert!(project_state.entries().is_empty());
}
#[test]
fn removing_project_active_extra_prompt_preserves_it_as_inactive_candidate() {
    let work_dir = temp_dir("remove-project-extra-prompt");
    let store: Arc<dyn SessionStore> = Arc::new(InMemorySessionStore::new());
    save_project_prompt_assembly_state(
        &work_dir,
        &scope_state! {
            scope: PromptAssemblyScope::Project,
            core_system_override: None,
            skill_discovery_override: None,
            entries: vec![PersistedPromptAssemblyEntry {
                reference_id: "review-rules".to_string(),
                kind: PromptSourceKind::ExtraPrompt,
                title: "Review rules".to_string(),
                enabled: true,
                requested_order: Some(10),
            }],
            skill_discovery_skills: Vec::new(),
            extra_prompts: vec![StoredPromptBody {
                reference_id: "review-rules".to_string(),
                title: "Review rules".to_string(),
                body: "# Review rules\nAlways verify tests.\n".to_string(),
            }],
            tool_guidelines_override: None,
            tool_selections: Vec::new(),
            dynamic_environment_sources: Vec::new(),
        },
    )
    .expect("initial project prompt assembly should save");

    let snapshot = apply_prompt_assembly_mutation(
        store,
        &work_dir,
        PromptAssemblyMutation::scoped(
            PromptAssemblyScope::Project,
            PromptAssemblyScopedMutationKind::RemovePromptSource {
                kind: PromptSourceKind::ExtraPrompt,
                reference_id: "review-rules".to_string(),
            },
        ),
        &[],
    )
    .expect("removing project extra prompt should succeed");

    assert!(
        snapshot
            .sources
            .managed
            .iter()
            .all(|source| source.reference_id != "review-rules"),
        "removed prompt should leave the active list"
    );
    assert_eq!(
        snapshot.candidates.extra_prompts,
        vec![PromptAssemblyExtraPromptCandidate {
            reference_id: "review-rules".to_string(),
            title: "Review rules".to_string(),
            origin: PromptSourceOrigin::Project,
            body: "# Review rules\nAlways verify tests.".to_string(),
            selected: false,
        }]
    );
}
#[test]
fn create_extra_prompt_keeps_supplied_legacy_default_body_verbatim() {
    let work_dir = temp_dir("create-extra-prompt-legacy-default");
    let store: Arc<dyn SessionStore> = Arc::new(InMemorySessionStore::new());

    let snapshot = apply_prompt_assembly_mutation(
        store,
        &work_dir,
        PromptAssemblyMutation::scoped(
            PromptAssemblyScope::Project,
            PromptAssemblyScopedMutationKind::CreateExtraPrompt {
                content: "# New prompt\n".to_string(),
            },
        ),
        &[],
    )
    .expect("create extra prompt should succeed");

    let created = snapshot
        .candidates
        .extra_prompts
        .iter()
        .find(|prompt| prompt.reference_id == "new-prompt")
        .expect("legacy default body should stay verbatim");

    assert_eq!(created.title, "New prompt");
    assert_eq!(created.body, "# New prompt".to_string());
}
