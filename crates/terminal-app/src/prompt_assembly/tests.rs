use super::*;
use runtime_domain::prompt_assembly::persistence::{
    PersistedPromptAssemblyEntry, PersistedSkillDiscoverySkillEntry, PersistedToolSelectionEntry,
    PromptAssemblyScope, PromptAssemblyScopeState, StoredPromptBody,
    save_project_prompt_assembly_state,
};
use session_store::InMemorySessionStore;

macro_rules! scope_state {
    (scope: $scope:expr, $($field:ident $(: $value:expr)?),* $(,)?) => {{
        let mut state = PromptAssemblyScopeState::new($scope);
        $(scope_state!(@assign state, $field $(: $value)?);)*
        state
    }};
    (@assign $state:ident, core_system_override : $value:expr) => {
        $state.set_core_system_override($value);
    };
    (@assign $state:ident, skill_discovery_override : $value:expr) => {
        $state.set_skill_discovery_override($value);
    };
    (@assign $state:ident, tool_guidelines_override : $value:expr) => {
        $state.set_tool_guidelines_override($value);
    };
    (@assign $state:ident, entries : $value:expr) => {
        $state.set_entries($value);
    };
    (@assign $state:ident, skill_discovery_skills : $value:expr) => {
        $state.set_skill_discovery_skills($value);
    };
    (@assign $state:ident, tool_selections : $value:expr) => {
        $state.set_tool_selections($value);
    };
    (@assign $state:ident, dynamic_environment_sources : $value:expr) => {
        $state.set_dynamic_environment_sources($value);
    };
    (@assign $state:ident, extra_prompts : $value:expr) => {
        $state.set_extra_prompts($value);
    };
    (@assign $state:ident, $field:ident) => {
        scope_state!(@assign $state, $field : $field);
    };
}

fn temp_dir(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "hunea-terminal-app-prompt-assembly-{label}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock should be after epoch")
            .as_nanos()
    ));
    fs::create_dir_all(&dir).expect("temp dir should exist");
    dir
}

fn builtin_tool_definitions() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition::new("bash")
            .with_label("Bash")
            .with_description("Run shell commands")
            .with_prompt_guidelines("Prefer rg over grep."),
        ToolDefinition::new("read_file")
            .with_label("Read file")
            .with_description("Read workspace files")
            .with_prompt_guidelines("Use for direct file reads."),
    ]
}

fn tool_definitions_with_unguided_tool() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition::new("authorize_search_download")
            .with_label("Authorize search download")
            .with_description("Install a managed search helper"),
        ToolDefinition::new("bash")
            .with_label("Bash")
            .with_description("Run shell commands")
            .with_prompt_guidelines("Prefer rg over grep."),
        ToolDefinition::new("read_file")
            .with_label("Read file")
            .with_description("Read workspace files")
            .with_prompt_guidelines("Use for direct file reads."),
    ]
}

#[test]
fn load_instructions_file_reports_non_utf8_read_errors() {
    let root = temp_dir("instructions-invalid-utf8");
    let path = root.join("AGENTS.md");
    fs::write(&path, [0xff, 0xfe]).expect("invalid utf8 fixture should be writable");

    let diagnostic = load_instructions_file(
        "instructions:project:.",
        "AGENTS.md".to_string(),
        &path,
        PromptSourceOrigin::Project,
    )
    .expect_err("invalid UTF-8 should be diagnostic instead of silent absence");

    assert_eq!(diagnostic.origin, Some(PromptSourceOrigin::Project));
    assert_eq!(diagnostic.path, Some(path));
    assert!(diagnostic.message.contains("read instructions file"));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn parse_skill_file_preserves_yaml_decode_source() {
    let root = temp_dir("skill-invalid-yaml");
    let path = root.join("SKILL.md");
    fs::write(
        &path,
        "---\nname: [unterminated\ndescription: broken\n---\nbody\n",
    )
    .expect("skill fixture should be writable");

    let error = parse_skill_file(&path, PromptSourceOrigin::Project)
        .expect_err("invalid YAML should preserve decode source");

    assert!(error.to_string().contains("decode skill frontmatter"));
    assert!(std::error::Error::source(&error).is_some());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn manager_snapshot_reports_invalid_skill_files_as_diagnostics() {
    let work_dir = temp_dir("invalid-skill-diagnostic");
    let skill_dir = work_dir.join(".agents/skills/broken-skill");
    fs::create_dir_all(&skill_dir).expect("skill dir should exist");
    let skill_path = skill_dir.join(SKILL_FILE_NAME);
    fs::write(
        &skill_path,
        "---\nname: broken-skill\n---\n# Missing description\n",
    )
    .expect("invalid skill fixture should write");

    let snapshot = resolve_prompt_assembly_manager_snapshot(
        &work_dir,
        &PromptAssemblyScopeState::new(PromptAssemblyScope::Global),
        &PromptAssemblyScopeState::new(PromptAssemblyScope::Project),
        &[],
    );

    assert!(
        snapshot.diagnostics.iter().any(|diagnostic| {
            diagnostic.path.as_deref() == Some(skill_path.as_path())
                && diagnostic.message.contains("missing required description")
        }),
        "invalid skill file should be surfaced as a prompt assembly diagnostic: {:?}",
        snapshot.diagnostics
    );
}

#[test]
fn manager_snapshot_includes_default_dynamic_environment_sources() {
    let work_dir = temp_dir("dynamic-defaults");
    let snapshot = resolve_prompt_assembly_manager_snapshot(
        &work_dir,
        &PromptAssemblyScopeState::new(PromptAssemblyScope::Global),
        &PromptAssemblyScopeState::new(PromptAssemblyScope::Project),
        &[],
    );

    assert!(snapshot.sources.managed.iter().any(|source| {
        source.kind == PromptSourceKind::DynamicEnvironmentBaseline
            && source.title == "Env baseline"
            && source.enabled
    }));
    assert!(snapshot.sources.managed.iter().any(|source| {
        source.kind == PromptSourceKind::DynamicEnvironmentChanges
            && source.title == "Env changes"
            && source.enabled
    }));
    assert_eq!(snapshot.candidates.dynamic_environment.len(), 4);
    assert!(
        snapshot
            .candidates
            .dynamic_environment
            .iter()
            .any(|candidate| {
                candidate.source_kind == DynamicEnvironmentSourceKind::GitWorkingTree
                    && candidate.origin == PromptSourceOrigin::Builtin
                    && candidate.baseline_selected
                    && candidate.changes_selected
                    && candidate
                        .baseline_preview_body
                        .contains("Environment baseline for this session:")
                    && candidate
                        .changes_preview_body
                        .contains("Environment changed since the last turn:")
            })
    );
    assert!(
        snapshot
            .candidates
            .dynamic_environment
            .iter()
            .any(|candidate| {
                candidate.source_kind == DynamicEnvironmentSourceKind::Workdir
                    && candidate.origin == PromptSourceOrigin::Builtin
                    && !candidate.baseline_selected
                    && !candidate.changes_selected
            })
    );
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
        ]
    );
}

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

    assert_eq!(prelude.sections.len(), 4);
    assert_eq!(prelude.sections[0].kind, PromptSourceKind::CoreSystemPrompt);
    assert_eq!(prelude.sections[1].reference_id, "repo-rules");
    assert_eq!(prelude.sections[2].reference_id, "skill-discovery");
    assert_eq!(prelude.sections[3].reference_id, "repo-bootstrap");
    let effective = prelude
        .effective_system_prompt()
        .expect("effective prompt should exist");
    assert!(effective.starts_with("global core\n\nproject rules\n\n"));
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
fn discover_skills_parses_multiline_yaml_frontmatter() {
    let work_dir = temp_dir("multiline-frontmatter");
    let skill_dir = work_dir.join(".agents/skills/caveman");
    fs::create_dir_all(&skill_dir).expect("skill dir should exist");
    fs::write(
            skill_dir.join(SKILL_FILE_NAME),
            "---\nname: caveman\ndescription: >\n  Ultra-compressed communication mode.\n  Cuts token usage without losing technical accuracy.\n---\n# Caveman\n",
        )
        .expect("skill file should exist");

    let discovered = discover_skills(&work_dir, None);
    let skill = discovered
        .iter()
        .find(|skill| skill.name == "caveman")
        .expect("multiline frontmatter skill should be discovered");

    assert_eq!(
        skill.description,
        "Ultra-compressed communication mode. Cuts token usage without losing technical accuracy."
    );
}

#[test]
fn manager_snapshot_keeps_project_and_global_skill_duplicates_for_overlay() {
    let work_dir = temp_dir("skill-duplicates-visible");
    let project_skill_dir = work_dir.join(".agents/skills/repo-bootstrap");
    fs::create_dir_all(&project_skill_dir).expect("project skill dir should exist");
    fs::write(
        project_skill_dir.join(SKILL_FILE_NAME),
        "---\nname: repo-bootstrap\ndescription: Project bootstrap\n---\n# Project Bootstrap\n",
    )
    .expect("project skill file should exist");
    let global_skill_root = temp_dir("skill-duplicates-visible-global");
    let global_skill_dir = global_skill_root.join("repo-bootstrap");
    fs::create_dir_all(&global_skill_dir).expect("global skill dir should exist");
    fs::write(
        global_skill_dir.join(SKILL_FILE_NAME),
        "---\nname: repo-bootstrap\ndescription: Global bootstrap\n---\n# Global Bootstrap\n",
    )
    .expect("global skill file should exist");

    let snapshot = resolve_prompt_assembly_manager_snapshot_with_global_skill_root(
        &work_dir,
        &PromptAssemblyScopeState::new(PromptAssemblyScope::Global),
        &PromptAssemblyScopeState::new(PromptAssemblyScope::Project),
        Some(global_skill_root.as_path()),
        &[],
    );

    let visible_origins = snapshot
        .candidates
        .discovered_skills
        .iter()
        .filter(|skill| skill.skill_name == "repo-bootstrap")
        .map(|skill| skill.origin)
        .collect::<Vec<_>>();
    let manual_origins = snapshot
        .candidates
        .manual_skills
        .iter()
        .filter(|skill| skill.skill_name == "repo-bootstrap")
        .map(|skill| skill.origin)
        .collect::<Vec<_>>();

    assert_eq!(
        visible_origins,
        vec![PromptSourceOrigin::Project, PromptSourceOrigin::Global]
    );
    assert_eq!(manual_origins, vec![PromptSourceOrigin::Project]);
}

#[test]
fn discovered_skill_inventory_keeps_manual_only_skills_visible() {
    let work_dir = temp_dir("manual-only-skill-visible");
    let manual_skill_dir = work_dir.join(".agents/skills/zzz-manual");
    fs::create_dir_all(&manual_skill_dir).expect("skill dir should exist");
    fs::write(
            manual_skill_dir.join(SKILL_FILE_NAME),
            "---\nname: zzz-manual\ndescription: Ask which skill fits.\ndisable-model-invocation: true\n---\n# Ask Matt\n",
        )
        .expect("skill file should exist");
    let discovery_skill_dir = work_dir.join(".agents/skills/aaa-discovery");
    fs::create_dir_all(&discovery_skill_dir).expect("skill dir should exist");
    fs::write(
        discovery_skill_dir.join(SKILL_FILE_NAME),
        "---\nname: aaa-discovery\ndescription: Discovery skill.\n---\n# Discovery\n",
    )
    .expect("skill file should exist");

    let snapshot = resolve_prompt_assembly_manager_snapshot(
        &work_dir,
        &PromptAssemblyScopeState::new(PromptAssemblyScope::Global),
        &PromptAssemblyScopeState::new(PromptAssemblyScope::Project),
        &[],
    );

    let skill = snapshot
        .candidates
        .discovered_skills
        .iter()
        .find(|skill| skill.skill_name == "zzz-manual")
        .expect("manual-only skill should remain visible in discovered inventory");
    assert!(!skill.selection.can_select());
    assert!(!skill.selection.is_selected());
    assert_eq!(skill.selection.selected_order(), None);
    let manual_index = snapshot
        .candidates
        .discovered_skills
        .iter()
        .position(|skill| skill.skill_name == "zzz-manual")
        .expect("manual-only skill should stay in inventory");
    assert!(
        snapshot.candidates.discovered_skills[..manual_index]
            .windows(2)
            .all(|pair| pair[0].title <= pair[1].title),
        "discovery-eligible ordering should stay intact before manual-only suffix"
    );
    assert!(
        snapshot.candidates.discovered_skills[..manual_index]
            .iter()
            .all(|skill| skill.selection.can_select()),
        "manual-only skills should sort after discovery-eligible skills"
    );

    let generated = snapshot
        .sources
        .preview
        .iter()
        .find(|source| source.kind == PromptSourceKind::SkillDiscovery)
        .and_then(|source| source.body.as_deref())
        .expect("skill discovery body should exist");
    assert!(
        !generated.contains("<name>zzz-manual</name>"),
        "manual-only skill should stay out of skill discovery prompt body"
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
fn manager_snapshot_skill_inventory_uses_dense_selected_order() {
    let work_dir = temp_dir("skill-order-dense");
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
    let global_skill_root = temp_dir("skill-order-dense-global");

    let snapshot = resolve_prompt_assembly_manager_snapshot_with_global_skill_root(
        &work_dir,
        &scope_state! {
            scope: PromptAssemblyScope::Global,
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
        },
        &PromptAssemblyScopeState::new(PromptAssemblyScope::Project),
        Some(global_skill_root.as_path()),
        &[],
    );

    let mut selected_orders = snapshot
        .candidates
        .discovered_skills
        .iter()
        .map(|skill| (skill.skill_name.clone(), skill.selection.selected_order()))
        .collect::<Vec<_>>();
    selected_orders.sort_by(|left, right| left.0.cmp(&right.0));

    assert_eq!(
        selected_orders,
        vec![
            ("code-review".to_string(), Some(2)),
            ("repo-bootstrap".to_string(), Some(1)),
        ]
    );
}

#[test]
fn manager_snapshot_tool_inventory_filters_unguided_tools_and_uses_dense_selected_order() {
    let work_dir = temp_dir("tool-order-dense");

    let snapshot = resolve_prompt_assembly_manager_snapshot(
        &work_dir,
        &scope_state! {
            scope: PromptAssemblyScope::Global,
            core_system_override: None,
            entries: Vec::new(),
            skill_discovery_override: None,
            skill_discovery_skills: Vec::new(),
            extra_prompts: Vec::new(),
            tool_guidelines_override: None,
            tool_selections: vec![
                PersistedToolSelectionEntry {
                    tool_name: "authorize_search_download".to_string(),
                    enabled: true,
                    requested_order: Some(10),
                },
                PersistedToolSelectionEntry {
                    tool_name: "bash".to_string(),
                    enabled: true,
                    requested_order: Some(20),
                },
                PersistedToolSelectionEntry {
                    tool_name: "read_file".to_string(),
                    enabled: true,
                    requested_order: Some(30),
                },
            ],
            dynamic_environment_sources: Vec::new(),
        },
        &PromptAssemblyScopeState::new(PromptAssemblyScope::Project),
        &tool_definitions_with_unguided_tool(),
    );

    assert_eq!(
        snapshot
            .candidates
            .tools
            .iter()
            .map(|tool| (tool.name.as_str(), tool.selection.selected_order()))
            .collect::<Vec<_>>(),
        vec![("bash", Some(1)), ("read_file", Some(2))]
    );
    assert!(
        snapshot
            .candidates
            .tools
            .iter()
            .all(|tool| tool.selection_scope == PromptAssemblyScope::Global)
    );
}

#[test]
fn manager_snapshot_discovered_skills_carry_effective_selection_scope() {
    let work_dir = temp_dir("skill-selection-scope");
    let global_skill_root = temp_dir("skill-selection-scope-global");
    let global_skill_dir = global_skill_root.join("code-review");
    fs::create_dir_all(&global_skill_dir).expect("global skill dir should exist");
    fs::write(
        global_skill_dir.join(SKILL_FILE_NAME),
        "---\nname: code-review\ndescription: Review code\n---\n# Code Review\n",
    )
    .expect("global skill file should write");

    let snapshot = resolve_prompt_assembly_manager_snapshot_with_global_skill_root(
        &work_dir,
        &PromptAssemblyScopeState::new(PromptAssemblyScope::Global),
        &PromptAssemblyScopeState::new(PromptAssemblyScope::Project),
        Some(global_skill_root.as_path()),
        &[],
    );

    let skill = snapshot
        .candidates
        .discovered_skills
        .iter()
        .find(|skill| skill.skill_name == "code-review")
        .expect("global discovered skill should exist");
    assert_eq!(skill.origin, PromptSourceOrigin::Global);
    assert_eq!(skill.selection_scope, PromptAssemblyScope::Project);
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
fn load_initial_prompt_prelude_reads_global_and_project_state() {
    let work_dir = temp_dir("load");
    let global_store: Arc<dyn SessionStore> = Arc::new(InMemorySessionStore::new());
    let project_state = scope_state! {
        scope: PromptAssemblyScope::Project,
        core_system_override: Some("project core".to_string()),
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
    };
    save_project_prompt_assembly_state(&work_dir, &project_state)
        .expect("project state should save");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime should build");
    runtime
        .block_on(
            global_store.save_global_prompt_assembly_state(&scope_state! {
                scope: PromptAssemblyScope::Global,
                core_system_override: Some("global core".to_string()),
                entries: Vec::new(),
                skill_discovery_override: None,
                skill_discovery_skills: Vec::new(),
                extra_prompts: Vec::new(),
                tool_guidelines_override: None,
                tool_selections: Vec::new(),
                dynamic_environment_sources: Vec::new(),
            }),
        )
        .expect("global state should save");

    let prelude =
        load_initial_prompt_prelude(global_store, &work_dir).expect("prelude should load");

    let effective = prelude
        .effective_system_prompt()
        .expect("effective prompt should exist");
    assert!(effective.starts_with("project core\n\n"));
    assert!(effective.contains("<available_skills>"));
    assert!(effective.contains("project rules"));
}

#[test]
fn prompt_assembly_workspace_reads_snapshot_and_prelude() {
    let work_dir = temp_dir("load-snapshot");
    let global_store: Arc<dyn SessionStore> = Arc::new(InMemorySessionStore::new());
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime should build");
    runtime
        .block_on(
            global_store.save_global_prompt_assembly_state(&scope_state! {
                scope: PromptAssemblyScope::Global,
                core_system_override: Some("global core".to_string()),
                entries: vec![PersistedPromptAssemblyEntry {
                    reference_id: "disabled".to_string(),
                    kind: PromptSourceKind::ExtraPrompt,
                    title: "disabled".to_string(),
                    enabled: false,
                    requested_order: Some(10),
                }],
                skill_discovery_override: None,
                skill_discovery_skills: Vec::new(),
                extra_prompts: Vec::new(),
                tool_guidelines_override: None,
                tool_selections: Vec::new(),
                dynamic_environment_sources: Vec::new(),
            }),
        )
        .expect("global state should save");

    let loaded = PromptAssemblyWorkspace::new(&work_dir, &[])
        .load_manager(global_store)
        .expect("snapshot should load");

    let effective = loaded
        .resolution
        .prelude
        .effective_system_prompt()
        .expect("effective prompt should exist");
    assert!(effective.starts_with("global core\n\n"));
    assert!(effective.contains("<available_skills>"));
    assert_eq!(
        loaded
            .resolution
            .assembly
            .inactive_sources
            .iter()
            .map(|source| source.reference_id.as_str())
            .collect::<Vec<_>>(),
        vec!["disabled"]
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

#[test]
fn assemble_attached_prompt_message_expands_unique_skill_mentions_in_first_use_order() {
    let work_dir = temp_dir("manual-skill-assembly");
    let repo_bootstrap_dir = work_dir.join(".agents/skills/repo-bootstrap");
    fs::create_dir_all(&repo_bootstrap_dir).expect("repo-bootstrap dir should exist");
    fs::write(
            repo_bootstrap_dir.join(SKILL_FILE_NAME),
            "---\nname: repo-bootstrap\ndescription: Bootstrap repo\n---\n# Repo Bootstrap\n\nBootstrap steps.\n",
        )
        .expect("repo-bootstrap skill should write");
    let code_review_dir = work_dir.join(".agents/skills/code-review");
    fs::create_dir_all(&code_review_dir).expect("code-review dir should exist");
    fs::write(
            code_review_dir.join(SKILL_FILE_NAME),
            "---\nname: code-review\ndescription: Review code\n---\n# Code Review\n\nReview carefully.\n",
        )
        .expect("code-review skill should write");

    let assembled = assemble_attached_prompt_message(
        None,
        &work_dir,
        &TranscriptUserMessage {
            content: "Please use $repo-bootstrap before $code-review and repeat $repo-bootstrap"
                .to_string(),
            attachments: Vec::new(),
            skill_bindings: vec![
                runtime_domain::session::TranscriptSkillBinding {
                    skill_name: "repo-bootstrap".to_string(),
                    origin: PromptSourceOrigin::Project,
                    skill_path: repo_bootstrap_dir
                        .join(SKILL_FILE_NAME)
                        .display()
                        .to_string(),
                    start_char: 11,
                    end_char: 26,
                },
                runtime_domain::session::TranscriptSkillBinding {
                    skill_name: "code-review".to_string(),
                    origin: PromptSourceOrigin::Project,
                    skill_path: code_review_dir.join(SKILL_FILE_NAME).display().to_string(),
                    start_char: 34,
                    end_char: 46,
                },
                runtime_domain::session::TranscriptSkillBinding {
                    skill_name: "repo-bootstrap".to_string(),
                    origin: PromptSourceOrigin::Project,
                    skill_path: repo_bootstrap_dir
                        .join(SKILL_FILE_NAME)
                        .display()
                        .to_string(),
                    start_char: 58,
                    end_char: 73,
                },
            ],
            custom_prompt_bindings: Vec::new(),
        },
    );

    assert_eq!(
        assembled
            .manual_skill_uses
            .iter()
            .map(|skill| skill.skill_name.as_str())
            .collect::<Vec<_>>(),
        vec!["repo-bootstrap", "code-review"]
    );
    assert_eq!(
        assembled.provider_visible_user_text,
        format!(
            "{}\n\n{}\n\nPlease use $repo-bootstrap before $code-review and repeat $repo-bootstrap",
            format_long_lived_skill_body(&DiscoveredSkill {
                name: "repo-bootstrap".to_string(),
                description: "Bootstrap repo".to_string(),
                skill_path: repo_bootstrap_dir.join(SKILL_FILE_NAME),
                body: "# Repo Bootstrap\n\nBootstrap steps.".to_string(),
                origin: PromptSourceOrigin::Project,
                disable_model_invocation: false,
            }),
            format_long_lived_skill_body(&DiscoveredSkill {
                name: "code-review".to_string(),
                description: "Review code".to_string(),
                skill_path: code_review_dir.join(SKILL_FILE_NAME),
                body: "# Code Review\n\nReview carefully.".to_string(),
                origin: PromptSourceOrigin::Project,
                disable_model_invocation: false,
            }),
        )
    );
}

#[test]
fn assemble_attached_prompt_message_ignores_plain_text_tokens_without_bindings() {
    let work_dir = temp_dir("manual-skill-without-bindings");
    let code_review_dir = work_dir.join(".agents/skills/code-review");
    fs::create_dir_all(&code_review_dir).expect("code-review dir should exist");
    fs::write(
            code_review_dir.join(SKILL_FILE_NAME),
            "---\nname: code-review\ndescription: Review code\n---\n# Code Review\n\nReview carefully.\n",
        )
        .expect("code-review skill should write");

    let assembled = assemble_attached_prompt_message(
        None,
        &work_dir,
        &TranscriptUserMessage {
            content: "Please use $code-review".to_string(),
            attachments: Vec::new(),
            skill_bindings: Vec::new(),
            custom_prompt_bindings: Vec::new(),
        },
    );

    assert!(assembled.manual_skill_uses.is_empty());
    assert!(assembled.custom_prompt_uses.is_empty());
    assert_eq!(
        assembled.provider_visible_user_text,
        "Please use $code-review"
    );
}

#[test]
fn assemble_attached_prompt_message_includes_custom_prompt_bodies_in_first_use_order() {
    let work_dir = temp_dir("custom-prompt-attachment");
    let store: Arc<dyn SessionStore> = Arc::new(InMemorySessionStore::new());
    save_project_prompt_assembly_state(
        &work_dir,
        &scope_state! {
            scope: PromptAssemblyScope::Project,
            core_system_override: None,
            entries: vec![PersistedPromptAssemblyEntry {
                reference_id: "review-rules".to_string(),
                kind: PromptSourceKind::ExtraPrompt,
                title: "Review Rules".to_string(),
                enabled: false,
                requested_order: None,
            }],
            skill_discovery_override: None,
            skill_discovery_skills: Vec::new(),
            extra_prompts: vec![StoredPromptBody {
                reference_id: "review-rules".to_string(),
                title: "Review Rules".to_string(),
                body: "# Review Rules\nCheck regressions before approving.".to_string(),
            }],
            tool_guidelines_override: None,
            tool_selections: Vec::new(),
            dynamic_environment_sources: Vec::new(),
        },
    )
    .expect("project prompt state should save");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime should build");
    runtime
        .block_on(
            store.save_global_prompt_assembly_state(&PromptAssemblyScopeState::new(
                PromptAssemblyScope::Global,
            )),
        )
        .expect("global prompt state should save");

    let manager = PromptAssemblyWorkspace::new(&work_dir, &[])
        .load_manager(store)
        .expect("prompt assembly manager should load");

    let assembled = assemble_attached_prompt_message(
        Some(&manager),
        &work_dir,
        &TranscriptUserMessage {
            content: "Before\n#review-rules\nAfter".to_string(),
            attachments: Vec::new(),
            skill_bindings: Vec::new(),
            custom_prompt_bindings: vec![runtime_domain::session::TranscriptCustomPromptBinding {
                reference_id: "review-rules".to_string(),
                origin: PromptSourceOrigin::Project,
                start_char: 7,
                end_char: 20,
            }],
        },
    );

    assert!(assembled.manual_skill_uses.is_empty());
    assert_eq!(
        assembled
            .custom_prompt_uses
            .iter()
            .map(|prompt| prompt.reference_id.as_str())
            .collect::<Vec<_>>(),
        vec!["review-rules"]
    );
    assert_eq!(
        assembled.provider_visible_user_text,
        "Before\n\n# Review Rules\nCheck regressions before approving.\n\nAfter"
    );
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
