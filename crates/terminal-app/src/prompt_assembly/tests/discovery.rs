use super::*;

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
fn discover_skills_from_root_uses_stable_natural_directory_order() {
    let root = temp_dir("skill-discovery-stable-root-order");
    for (dir_name, skill_name) in [
        ("zzz", "zzz"),
        ("aaa", "aaa"),
        ("mmm", "mmm"),
        ("bbb", "bbb"),
        ("ccc", "ccc"),
    ] {
        let skill_dir = root.join(dir_name);
        fs::create_dir_all(&skill_dir).expect("skill dir should exist");
        fs::write(
            skill_dir.join(SKILL_FILE_NAME),
            format!("---\nname: {skill_name}\ndescription: Test skill.\n---\n# {skill_name}\n"),
        )
        .expect("skill file should exist");
    }

    let mut discovered = Vec::new();
    let mut diagnostics = Vec::new();
    let mut seen_names = HashMap::new();
    discover_skills_from_root(
        &root,
        PromptSourceOrigin::Project,
        &mut discovered,
        &mut seen_names,
        &mut diagnostics,
    );

    assert_eq!(diagnostics, Vec::new());
    assert_eq!(
        discovered
            .iter()
            .map(|skill| skill.name.as_str())
            .collect::<Vec<_>>(),
        vec!["aaa", "bbb", "ccc", "mmm", "zzz"]
    );
    let _ = fs::remove_dir_all(root);
}
#[test]
fn discover_nested_skill_dirs_use_stable_natural_directory_order() {
    let root = temp_dir("skill-discovery-stable-nested-order");
    let nested_root = root.join("nested");
    for (dir_name, skill_name) in [
        ("zzz", "zzz"),
        ("aaa", "aaa"),
        ("mmm", "mmm"),
        ("bbb", "bbb"),
        ("ccc", "ccc"),
    ] {
        let skill_dir = nested_root.join(dir_name);
        fs::create_dir_all(&skill_dir).expect("nested skill dir should exist");
        fs::write(
            skill_dir.join(SKILL_FILE_NAME),
            format!("---\nname: {skill_name}\ndescription: Test skill.\n---\n# {skill_name}\n"),
        )
        .expect("skill file should exist");
    }

    let mut discovered = Vec::new();
    let mut diagnostics = Vec::new();
    let mut seen_names = HashMap::new();
    discover_skill_dir(
        &nested_root,
        PromptSourceOrigin::Project,
        &mut discovered,
        &mut seen_names,
        &mut diagnostics,
    );

    assert_eq!(diagnostics, Vec::new());
    assert_eq!(
        discovered
            .iter()
            .map(|skill| skill.name.as_str())
            .collect::<Vec<_>>(),
        vec!["aaa", "bbb", "ccc", "mmm", "zzz"]
    );
    let _ = fs::remove_dir_all(root);
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
    let global_skill_root = work_dir.join("global-skills");
    fs::create_dir_all(&global_skill_root).expect("global skill root should exist");
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

    let snapshot = resolve_prompt_assembly_manager_snapshot_with_global_skill_root(
        &work_dir,
        &PromptAssemblyScopeState::new(PromptAssemblyScope::Global),
        &PromptAssemblyScopeState::new(PromptAssemblyScope::Project),
        Some(&global_skill_root),
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
    assert_eq!(
        snapshot
            .candidates
            .discovered_skills
            .iter()
            .map(|skill| (skill.skill_name.as_str(), skill.selection.can_select()))
            .collect::<Vec<_>>(),
        vec![("aaa-discovery", true), ("zzz-manual", false)],
        "manual-only skills should remain visible after discovery-eligible skills"
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
