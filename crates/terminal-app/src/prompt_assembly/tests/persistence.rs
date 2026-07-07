use super::*;

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
