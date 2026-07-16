use runtime_domain::prompt_assembly::{
    PromptAssemblyMutation, PromptAssemblyScopedMutationKind, PromptPreludeSection,
    PromptPreludeSnapshot, PromptSourceKind, PromptSourceOrigin,
    persistence::{
        PersistedPromptAssemblyEntry, PromptAssemblyScope, StoredPromptBody,
        load_project_prompt_assembly_state, project_custom_prompts_dir,
        save_project_prompt_assembly_state,
    },
};
use runtime_domain::session::PromptAssemblyUpdateNotice;

use super::support::*;

macro_rules! scope_state {
    (scope: $scope:expr, $($field:ident $(: $value:expr)?),* $(,)?) => {{
        let mut state =
            runtime_domain::prompt_assembly::persistence::PromptAssemblyScopeState::new($scope);
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

#[test]
fn begin_prompt_assembly_edit_reads_latest_filesystem_state() {
    let root = temp_test_dir("reload-prompt-assembly");
    let work_dir = root.join("repo");
    let skill_dir = work_dir.join(".agents/skills/repo-bootstrap");
    fs::create_dir_all(&skill_dir).expect("skill dir should exist");
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: repo-bootstrap\ndescription: Bootstrap repo\n---\n# Repo Bootstrap\n\nUse this skill.\n",
    )
    .expect("skill file should exist");

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
                enabled: false,
                requested_order: None,
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
    .expect("project prompt assembly should save");

    let store: Arc<dyn SessionStore> = Arc::new(InMemorySessionStore::new());
    let mut coordinator = runtime_coordinator(AppRuntimeOptions {
        session_store: Some(store),
        session_header_template: Some(SessionHeader {
            session_id: SessionId::new(),
            work_dir: work_dir.clone(),
            session_name: None,
            initial_model: "qwen3".to_string(),
            git_head: None,
            cli_version: None,
        }),
        ..AppRuntimeOptions::default()
    });

    coordinator
        .begin_prompt_assembly_edit()
        .expect("begin prompt assembly edit should load working copy");
    let initial_manager = coordinator
        .peek_prompt_assembly_edit_snapshot()
        .expect("edit session should be active after begin");

    assert!(
        initial_manager
            .candidates
            .extra_prompts
            .iter()
            .any(|prompt| prompt.reference_id == "review-rules")
    );
    assert!(
        initial_manager
            .candidates
            .discovered_skills
            .iter()
            .any(|skill| skill.skill_name == "repo-bootstrap")
    );
    assert!(
        initial_manager
            .candidates
            .manual_skills
            .iter()
            .any(|skill| skill.skill_name == "repo-bootstrap")
    );

    fs::remove_file(project_custom_prompts_dir(&work_dir).join("review-rules.md"))
        .expect("custom prompt file should be removable");
    fs::remove_file(skill_dir.join("SKILL.md")).expect("skill file should be removable");

    coordinator
        .commit_prompt_assembly_edit()
        .expect("commit with no mutation should be a no-op");
    coordinator
        .begin_prompt_assembly_edit()
        .expect("re-entering edit session should re-read filesystem state");
    let reloaded_manager = coordinator
        .peek_prompt_assembly_edit_snapshot()
        .expect("edit session should be active after second begin");

    assert!(
        reloaded_manager
            .candidates
            .extra_prompts
            .iter()
            .all(|prompt| prompt.reference_id != "review-rules")
    );
    assert!(
        reloaded_manager
            .candidates
            .discovered_skills
            .iter()
            .all(|skill| !(skill.origin
                == runtime_domain::prompt_assembly::PromptSourceOrigin::Project
                && skill.skill_name == "repo-bootstrap"))
    );
    assert!(
        reloaded_manager
            .candidates
            .manual_skills
            .iter()
            .all(|skill| !(skill.origin
                == runtime_domain::prompt_assembly::PromptSourceOrigin::Project
                && skill.skill_name == "repo-bootstrap"))
    );
    cleanup(&root);
}

#[test]
fn begin_prompt_assembly_edit_reports_load_failure() {
    let root = temp_test_dir("reload-prompt-assembly-load-failure");
    let work_dir = root.join("repo");
    fs::create_dir_all(&work_dir).expect("work dir should exist");
    let store: Arc<dyn SessionStore> = Arc::new(FailingSessionStore::new(
        Arc::new(InMemorySessionStore::new()),
        FailingSessionStoreLoad::PromptAssemblyLoad,
    ));
    let mut coordinator = runtime_coordinator(AppRuntimeOptions {
        session_store: Some(store),
        session_header_template: Some(SessionHeader {
            session_id: SessionId::new(),
            work_dir,
            session_name: None,
            initial_model: "qwen3".to_string(),
            git_head: None,
            cli_version: None,
        }),
        ..AppRuntimeOptions::default()
    });

    let error = coordinator
        .begin_prompt_assembly_edit()
        .expect_err("load failure should be reported via Err");

    assert!(
        error.contains("injected prompt assembly load failure"),
        "failure message should retain store context: {error}"
    );
    assert!(
        coordinator.peek_prompt_assembly_edit_snapshot().is_none(),
        "edit session should not be active after begin failure"
    );
    cleanup(&root);
}

#[test]
fn project_prompt_assembly_mutation_does_not_touch_global_save_path() {
    let root = temp_test_dir("mutate-project-prompt-assembly-with-failing-global-save");
    let work_dir = root.join("repo");
    fs::create_dir_all(&work_dir).expect("work dir should exist");
    let store: Arc<dyn SessionStore> = Arc::new(FailingSessionStore::new(
        Arc::new(InMemorySessionStore::new()),
        FailingSessionStoreLoad::PromptAssemblySave,
    ));
    let mut coordinator = runtime_coordinator(AppRuntimeOptions {
        session_store: Some(store),
        session_header_template: Some(SessionHeader {
            session_id: SessionId::new(),
            work_dir: work_dir.clone(),
            session_name: None,
            initial_model: "qwen3".to_string(),
            git_head: None,
            cli_version: None,
        }),
        ..AppRuntimeOptions::default()
    });

    coordinator
        .begin_prompt_assembly_edit()
        .expect("begin prompt assembly edit should load working copy");
    let snapshot = coordinator
        .apply_prompt_assembly_edit_mutation(PromptAssemblyMutation::scoped(
            PromptAssemblyScope::Project,
            PromptAssemblyScopedMutationKind::CreateExtraPrompt {
                content: "# Project-only prompt\nThis must not be half-saved.\n".to_string(),
            },
        ))
        .expect("project-scoped mutation should apply to working copy");
    assert!(
        snapshot
            .candidates
            .extra_prompts
            .iter()
            .any(|prompt| prompt.title == "Project-only prompt")
    );

    coordinator
        .commit_prompt_assembly_edit()
        .expect("commit should save project state without touching global save path");

    let project_state = load_project_prompt_assembly_state(&work_dir)
        .expect("project prompt assembly state should remain readable");
    assert!(
        project_state
            .extra_prompts()
            .iter()
            .any(|prompt| prompt.title == "Project-only prompt"),
        "project custom prompt should be written without touching global state"
    );
    assert!(
        project_state.entries().iter().any(|entry| {
            entry.kind == PromptSourceKind::ExtraPrompt && entry.title == "Project-only prompt"
        }),
        "project entry should be written without touching global state"
    );
    cleanup(&root);
}

#[test]
fn commit_prompt_assembly_edit_reports_save_failure() {
    let root = temp_test_dir("mutate-prompt-assembly-save-failure");
    let work_dir = root.join("repo");
    fs::create_dir_all(&work_dir).expect("work dir should exist");
    let store: Arc<dyn SessionStore> = Arc::new(FailingSessionStore::new(
        Arc::new(InMemorySessionStore::new()),
        FailingSessionStoreLoad::PromptAssemblySave,
    ));
    let mut coordinator = runtime_coordinator(AppRuntimeOptions {
        session_store: Some(store),
        session_header_template: Some(SessionHeader {
            session_id: SessionId::new(),
            work_dir,
            session_name: None,
            initial_model: "qwen3".to_string(),
            git_head: None,
            cli_version: None,
        }),
        ..AppRuntimeOptions::default()
    });

    coordinator
        .begin_prompt_assembly_edit()
        .expect("begin prompt assembly edit should load working copy");
    coordinator
        .apply_prompt_assembly_edit_mutation(PromptAssemblyMutation::scoped(
            PromptAssemblyScope::Global,
            PromptAssemblyScopedMutationKind::CreateExtraPrompt {
                content: "# Saved search\nUse ripgrep first.\n".to_string(),
            },
        ))
        .expect("global-scoped mutation should apply to working copy");

    let error = coordinator
        .commit_prompt_assembly_edit()
        .expect_err("global save failure should be reported via Err");

    assert!(
        error.contains("injected prompt assembly save failure"),
        "failure message should retain store context: {error}"
    );

    // 失败后 edit session 必须保留 working copy，让用户可重试或继续编辑。
    let preserved_snapshot = coordinator
        .peek_prompt_assembly_edit_snapshot()
        .expect("edit session should remain active after commit failure");
    assert!(
        preserved_snapshot
            .candidates
            .extra_prompts
            .iter()
            .any(|prompt| prompt.title == "Saved search"),
        "working copy should retain the unsaved mutation after commit failure"
    );

    // session 仍在 active 状态，应能继续应用新的 mutation。
    coordinator
        .apply_prompt_assembly_edit_mutation(PromptAssemblyMutation::scoped(
            PromptAssemblyScope::Global,
            PromptAssemblyScopedMutationKind::CreateExtraPrompt {
                content: "# Another prompt\n".to_string(),
            },
        ))
        .expect("further mutation should apply to preserved working copy");
    cleanup(&root);
}

#[test]
fn begin_after_commit_failure_preserves_unsaved_working_copy() {
    let root = temp_test_dir("begin-after-commit-failure-preserve");
    let work_dir = root.join("repo");
    fs::create_dir_all(&work_dir).expect("work dir should exist");
    let store: Arc<dyn SessionStore> = Arc::new(FailingSessionStore::new(
        Arc::new(InMemorySessionStore::new()),
        FailingSessionStoreLoad::PromptAssemblySave,
    ));
    let mut coordinator = runtime_coordinator(AppRuntimeOptions {
        session_store: Some(store),
        session_header_template: Some(SessionHeader {
            session_id: SessionId::new(),
            work_dir,
            session_name: None,
            initial_model: "qwen3".to_string(),
            git_head: None,
            cli_version: None,
        }),
        ..AppRuntimeOptions::default()
    });

    coordinator
        .begin_prompt_assembly_edit()
        .expect("begin prompt assembly edit should load working copy");
    coordinator
        .apply_prompt_assembly_edit_mutation(PromptAssemblyMutation::scoped(
            PromptAssemblyScope::Global,
            PromptAssemblyScopedMutationKind::CreateExtraPrompt {
                content: "# Unsaved edit\n".to_string(),
            },
        ))
        .expect("global mutation should apply to working copy");
    coordinator
        .commit_prompt_assembly_edit()
        .expect_err("global save failure should retain edit session");

    // 再次 begin 必须复用未提交的 session，而非从磁盘重新 load 覆盖未落盘的编辑。
    let snapshot = coordinator
        .begin_prompt_assembly_edit()
        .expect("re-begin should reuse preserved edit session");
    assert!(
        snapshot
            .candidates
            .extra_prompts
            .iter()
            .any(|prompt| prompt.title == "Unsaved edit"),
        "re-begin after commit failure should preserve unsaved working copy, not reload from disk"
    );
    cleanup(&root);
}

#[test]
fn commit_prompt_assembly_edit_releases_session_on_noop_commit() {
    let root = temp_test_dir("commit-prompt-assembly-noop-release");
    let work_dir = root.join("repo");
    fs::create_dir_all(&work_dir).expect("work dir should exist");
    let store: Arc<dyn SessionStore> = Arc::new(InMemorySessionStore::new());
    let mut coordinator = runtime_coordinator(AppRuntimeOptions {
        session_store: Some(store),
        session_header_template: Some(SessionHeader {
            session_id: SessionId::new(),
            work_dir,
            session_name: None,
            initial_model: "qwen3".to_string(),
            git_head: None,
            cli_version: None,
        }),
        ..AppRuntimeOptions::default()
    });

    coordinator
        .begin_prompt_assembly_edit()
        .expect("begin prompt assembly edit should load working copy");
    assert!(
        coordinator.peek_prompt_assembly_edit_snapshot().is_some(),
        "edit session should be active after begin"
    );

    // not dirty commit：不应落盘，但必须释放 edit session，避免 working copy 长期挂在 coordinator 上。
    coordinator
        .commit_prompt_assembly_edit()
        .expect("noop commit should succeed without persistence");
    assert!(
        coordinator.peek_prompt_assembly_edit_snapshot().is_none(),
        "edit session should be released after successful noop commit"
    );
    cleanup(&root);
}

#[test]
fn prompt_assembly_changes_sync_current_empty_session_prelude_immediately() {
    let root = temp_test_dir("prompt-assembly-next-new-session");
    let work_dir = root.join("repo");
    fs::create_dir_all(&work_dir).expect("work dir should exist");
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
                enabled: false,
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
    .expect("project prompt assembly should save");

    let store: Arc<dyn SessionStore> = Arc::new(InMemorySessionStore::new());
    let initial_prelude = PromptPreludeSnapshot {
        sections: vec![PromptPreludeSection {
            reference_id: "core-system".to_string(),
            kind: PromptSourceKind::CoreSystemPrompt,
            title: "Core system prompt".to_string(),
            origin: Some(PromptSourceOrigin::Builtin),
            body: "historical core".to_string(),
        }],
    };
    let mut coordinator = runtime_coordinator(AppRuntimeOptions {
        session_store: Some(store),
        session_header_template: Some(SessionHeader {
            session_id: SessionId::new(),
            work_dir: work_dir.clone(),
            session_name: None,
            initial_model: "qwen3".to_string(),
            git_head: None,
            cli_version: None,
        }),
        initial_prompt_prelude: Some(initial_prelude.clone()),
        ..AppRuntimeOptions::default()
    });

    assert_eq!(
        coordinator.provider_conversation.prompt_prelude(),
        Some(&initial_prelude)
    );

    coordinator
        .begin_prompt_assembly_edit()
        .expect("begin prompt assembly edit should load working copy");
    coordinator
        .apply_prompt_assembly_edit_mutation(PromptAssemblyMutation::scoped(
            PromptAssemblyScope::Project,
            PromptAssemblyScopedMutationKind::SetPromptSourceEnabled {
                kind: PromptSourceKind::ExtraPrompt,
                reference_id: "review-rules".to_string(),
                enabled: true,
            },
        ))
        .expect("prompt assembly mutation should apply to working copy");
    coordinator
        .commit_prompt_assembly_edit()
        .expect("commit should persist and emit updated event");

    let (updated_manager, notice) = wait_for_runtime_event(
        &mut coordinator,
        |event| match event {
            RuntimeEvent::PromptAssemblyUpdated { manager, notice } => Some((manager, notice)),
            _ => None,
        },
        "prompt assembly updated event",
    );

    assert_eq!(
        coordinator.provider_conversation.prompt_prelude(),
        Some(&updated_manager.resolution.prelude)
    );
    assert_eq!(
        coordinator.options.initial_prompt_prelude.as_ref(),
        Some(&updated_manager.resolution.prelude)
    );
    assert_eq!(
        notice,
        Some(PromptAssemblyUpdateNotice::CurrentEmptySessionUpdated)
    );
    assert_ne!(
        Some(&updated_manager.resolution.prelude),
        Some(&initial_prelude)
    );
    cleanup(&root);
}

#[test]
fn prompt_assembly_changes_on_started_session_apply_only_after_next_new_session_reset() {
    let root = temp_test_dir("prompt-assembly-started-session");
    let work_dir = root.join("repo");
    fs::create_dir_all(&work_dir).expect("work dir should exist");
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
                enabled: false,
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
    .expect("project prompt assembly should save");

    let store: Arc<dyn SessionStore> = Arc::new(InMemorySessionStore::new());
    let initial_prelude = PromptPreludeSnapshot {
        sections: vec![PromptPreludeSection {
            reference_id: "core-system".to_string(),
            kind: PromptSourceKind::CoreSystemPrompt,
            title: "Core system prompt".to_string(),
            origin: Some(PromptSourceOrigin::Builtin),
            body: "historical core".to_string(),
        }],
    };
    let mut coordinator = runtime_coordinator(AppRuntimeOptions {
        session_store: Some(store),
        session_header_template: Some(SessionHeader {
            session_id: SessionId::new(),
            work_dir: work_dir.clone(),
            session_name: None,
            initial_model: "qwen3".to_string(),
            git_head: None,
            cli_version: None,
        }),
        initial_prompt_prelude: Some(initial_prelude.clone()),
        ..AppRuntimeOptions::default()
    });
    coordinator
        .provider_conversation
        .append_items(vec![ConversationItem::text(Role::User, "already started")])
        .expect("seed history should succeed");

    coordinator
        .begin_prompt_assembly_edit()
        .expect("begin prompt assembly edit should load working copy");
    coordinator
        .apply_prompt_assembly_edit_mutation(PromptAssemblyMutation::scoped(
            PromptAssemblyScope::Project,
            PromptAssemblyScopedMutationKind::SetPromptSourceEnabled {
                kind: PromptSourceKind::ExtraPrompt,
                reference_id: "review-rules".to_string(),
                enabled: true,
            },
        ))
        .expect("prompt assembly mutation should apply to working copy");
    coordinator
        .commit_prompt_assembly_edit()
        .expect("commit should persist and emit updated event");
    let (updated_manager, notice) = wait_for_runtime_event(
        &mut coordinator,
        |event| match event {
            RuntimeEvent::PromptAssemblyUpdated { manager, notice } => Some((manager, notice)),
            _ => None,
        },
        "prompt assembly updated event",
    );

    assert_eq!(
        coordinator.provider_conversation.prompt_prelude(),
        Some(&initial_prelude)
    );
    assert_eq!(
        coordinator.options.initial_prompt_prelude.as_ref(),
        Some(&updated_manager.resolution.prelude)
    );
    assert_eq!(
        notice,
        Some(PromptAssemblyUpdateNotice::NextNewSessionUpdated)
    );

    coordinator
        .handle_runtime_command(RuntimeCommand::Reset)
        .expect("reset should succeed");

    assert_eq!(
        coordinator.provider_conversation.prompt_prelude(),
        coordinator.options.initial_prompt_prelude.as_ref()
    );
    cleanup(&root);
}

#[test]
fn dynamic_environment_selection_change_on_empty_session_emits_current_session_notice() {
    let root = temp_test_dir("dynamic-environment-notice-empty-session");
    let work_dir = root.join("repo");
    fs::create_dir_all(&work_dir).expect("work dir should exist");
    let store: Arc<dyn SessionStore> = Arc::new(InMemorySessionStore::new());
    let store_runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime should build");
    store_runtime
        .block_on(store.save_global_prompt_assembly_state(&scope_state! {
            scope: PromptAssemblyScope::Global,
            core_system_override: None,
            skill_discovery_override: None,
            entries: Vec::new(),
            skill_discovery_skills: Vec::new(),
            extra_prompts: Vec::new(),
            tool_guidelines_override: None,
            tool_selections: Vec::new(),
            dynamic_environment_sources:
                runtime_domain::dynamic_environment::default_dynamic_environment_selections(),
        }))
        .expect("global prompt state should save");
    let mut coordinator = runtime_coordinator(AppRuntimeOptions {
        session_store: Some(store),
        session_header_template: Some(SessionHeader {
            session_id: SessionId::new(),
            work_dir: work_dir.clone(),
            session_name: None,
            initial_model: "qwen3".to_string(),
            git_head: None,
            cli_version: None,
        }),
        ..AppRuntimeOptions::default()
    });

    coordinator
        .begin_prompt_assembly_edit()
        .expect("begin prompt assembly edit should load working copy");
    coordinator
        .apply_prompt_assembly_edit_mutation(
            runtime_domain::prompt_assembly::PromptAssemblyMutation::SetDynamicEnvironmentSourceSelected {
                snapshot_kind:
                    runtime_domain::dynamic_environment::DynamicEnvironmentSnapshotKind::Baseline,
                source_kind:
                    runtime_domain::dynamic_environment::DynamicEnvironmentSourceKind::Date,
                selected: false,
            },
        )
        .expect("dynamic environment mutation should apply to working copy");
    coordinator
        .commit_prompt_assembly_edit()
        .expect("commit should persist and emit updated event");

    let notice = wait_for_runtime_event(
        &mut coordinator,
        |event| match event {
            RuntimeEvent::PromptAssemblyUpdated { notice, .. } => Some(notice),
            _ => None,
        },
        "prompt assembly updated notice",
    );

    assert_eq!(
        notice,
        Some(PromptAssemblyUpdateNotice::CurrentEmptySessionUpdated)
    );
    cleanup(&root);
}

#[test]
fn disabling_dynamic_environment_changes_waits_for_next_new_session() {
    let root = temp_test_dir("dynamic-environment-next-new-session");
    let work_dir = root.join("repo");
    fs::create_dir_all(&work_dir).expect("work dir should exist");
    let store: Arc<dyn SessionStore> = Arc::new(InMemorySessionStore::new());
    let store_runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime should build");
    let mut dynamic_environment_sources =
        runtime_domain::dynamic_environment::default_dynamic_environment_selections();
    for source in &mut dynamic_environment_sources {
        source.enabled = source.snapshot_kind
            == runtime_domain::dynamic_environment::DynamicEnvironmentSnapshotKind::Changes
            && source.source_kind
                == runtime_domain::dynamic_environment::DynamicEnvironmentSourceKind::Date;
    }
    store_runtime
        .block_on(store.save_global_prompt_assembly_state(&scope_state! {
            scope: PromptAssemblyScope::Global,
            core_system_override: None,
            skill_discovery_override: None,
            entries: Vec::new(),
            skill_discovery_skills: Vec::new(),
            extra_prompts: Vec::new(),
            tool_guidelines_override: None,
            tool_selections: Vec::new(),
            dynamic_environment_sources,
        }))
        .expect("global prompt state should save");
    let mut coordinator = runtime_coordinator(AppRuntimeOptions {
        session_store: Some(store),
        session_header_template: Some(SessionHeader {
            session_id: SessionId::new(),
            work_dir: work_dir.clone(),
            session_name: None,
            initial_model: "qwen3".to_string(),
            git_head: None,
            cli_version: None,
        }),
        initial_dynamic_environment_session_config: Some(
            runtime_domain::dynamic_environment::DynamicEnvironmentSessionConfig {
                baseline_enabled: false,
                changes_enabled: true,
                source_selections: vec![
                    runtime_domain::dynamic_environment::DynamicEnvironmentSourceSelection {
                        snapshot_kind:
                            runtime_domain::dynamic_environment::DynamicEnvironmentSnapshotKind::Changes,
                        source_kind:
                            runtime_domain::dynamic_environment::DynamicEnvironmentSourceKind::Date,
                        enabled: true,
                    },
                ],
                static_baseline_observations: Vec::new(),
            },
        ),
        ..AppRuntimeOptions::default()
    });
    coordinator
        .provider_conversation
        .append_items(vec![ConversationItem::text(Role::User, "already started")])
        .expect("seed history should succeed");

    coordinator
        .begin_prompt_assembly_edit()
        .expect("begin prompt assembly edit should load working copy");
    coordinator
        .apply_prompt_assembly_edit_mutation(PromptAssemblyMutation::scoped(
            PromptAssemblyScope::Global,
            PromptAssemblyScopedMutationKind::SetPromptSourceEnabled {
                kind: PromptSourceKind::DynamicEnvironmentChanges,
                reference_id: "env-changes".to_string(),
                enabled: false,
            },
        ))
        .expect("dynamic environment source mutation should apply to working copy");
    coordinator
        .commit_prompt_assembly_edit()
        .expect("commit should persist and emit updated event");
    let notice = wait_for_runtime_event(
        &mut coordinator,
        |event| match event {
            RuntimeEvent::PromptAssemblyUpdated { notice, .. } => Some(notice),
            _ => None,
        },
        "prompt assembly updated notice",
    );
    assert_eq!(
        notice,
        Some(PromptAssemblyUpdateNotice::NextNewSessionUpdated)
    );

    let current_session_injection = coordinator
        .dynamic_environment_injection()
        .expect("current session dynamic environment should resolve");
    assert!(
        !current_session_injection.appended_user_texts.is_empty(),
        "current started session should keep the old dynamic environment config until reset"
    );

    coordinator
        .handle_runtime_command(RuntimeCommand::Reset)
        .expect("reset should succeed");
    coordinator
        .provider_conversation
        .append_items(vec![ConversationItem::text(Role::User, "fresh session")])
        .expect("fresh session history should seed");
    let next_session_injection = coordinator
        .dynamic_environment_injection()
        .expect("next new session dynamic environment should resolve");
    assert!(
        next_session_injection.appended_user_texts.is_empty(),
        "after reset the disabled dynamic environment source should stop injecting changes"
    );
    cleanup(&root);
}
#[test]
fn session_tools_for_manager_filters_disabled_tools_and_keeps_full_registry() {
    use runtime_domain::prompt_assembly::persistence::PromptAssemblyScope;
    use runtime_domain::prompt_assembly::{
        PromptAssemblyManagerSnapshot, PromptAssemblySelectionState, PromptAssemblyToolCandidate,
        PromptSourceOrigin,
    };
    use tool_runtime::{Tool, ToolCall, ToolDefinition, ToolExecutionFuture, ToolResult};

    struct StubTool {
        name: &'static str,
    }

    impl Tool for StubTool {
        fn definition(&self) -> ToolDefinition {
            ToolDefinition::new(self.name)
        }

        fn execute<'a>(
            &'a self,
            call: ToolCall,
            _cancellation: &'a tokio_util::sync::CancellationToken,
        ) -> ToolExecutionFuture<'a> {
            Box::pin(async move { ToolResult::success(call.call_id, String::new()) })
        }
    }

    fn tool_candidate(name: &str, tool_enabled: bool) -> PromptAssemblyToolCandidate {
        PromptAssemblyToolCandidate {
            name: name.to_string(),
            label: None,
            description: None,
            prompt_guidelines: None,
            origin: PromptSourceOrigin::Builtin,
            selection_scope: PromptAssemblyScope::Global,
            tool_enabled,
            selection: PromptAssemblySelectionState::Unselectable,
        }
    }

    let mut workspace_tools = tool_runtime::ToolExecutorRegistry::new();
    workspace_tools.insert(StubTool { name: "bash" });
    workspace_tools.insert(StubTool { name: "read" });

    let mut manager = PromptAssemblyManagerSnapshot::default();
    manager.candidates.tools = vec![tool_candidate("bash", false), tool_candidate("read", true)];

    let session_tools = super::super::session_tools_for_manager(&workspace_tools, Some(&manager));
    assert_eq!(
        session_tools
            .definitions()
            .definitions()
            .map(|definition| definition.name.clone())
            .collect::<Vec<_>>(),
        vec!["read".to_string()],
        "disabled tools should be excluded from the session registry"
    );
    assert_eq!(
        workspace_tools.definitions().definitions().count(),
        2,
        "full registry should stay untouched for /prompt inventory"
    );

    let unfiltered = super::super::session_tools_for_manager(&workspace_tools, None);
    assert_eq!(
        unfiltered.definitions().definitions().count(),
        2,
        "missing manager snapshot should keep every tool enabled"
    );
}
#[test]
fn disabling_tool_on_empty_session_updates_session_tools_immediately() {
    let root = temp_test_dir("tool-enablement-empty-session");
    let work_dir = root.join("repo");
    fs::create_dir_all(&work_dir).expect("work dir should exist");
    let store: Arc<dyn SessionStore> = Arc::new(InMemorySessionStore::new());
    let mut coordinator = runtime_coordinator(AppRuntimeOptions {
        session_store: Some(store),
        session_header_template: Some(SessionHeader {
            session_id: SessionId::new(),
            work_dir: work_dir.clone(),
            session_name: None,
            initial_model: "qwen3".to_string(),
            git_head: None,
            cli_version: None,
        }),
        ..AppRuntimeOptions::default()
    });
    let disabled_tool_name = coordinator.prompt_assembly_tool_definitions()[0]
        .name
        .clone();
    assert!(
        coordinator
            .session_workspace_tools
            .definitions()
            .definitions()
            .any(|definition| definition.name == disabled_tool_name),
        "tool should start enabled in the session registry"
    );

    coordinator
        .begin_prompt_assembly_edit()
        .expect("begin prompt assembly edit should load working copy");
    coordinator
        .apply_prompt_assembly_edit_mutation(PromptAssemblyMutation::scoped(
            PromptAssemblyScope::Global,
            PromptAssemblyScopedMutationKind::SetToolEnabled {
                tool_name: disabled_tool_name.clone(),
                enabled: false,
            },
        ))
        .expect("tool enablement mutation should apply to working copy");
    coordinator
        .commit_prompt_assembly_edit()
        .expect("commit should persist and emit updated event");
    let (_, notice) = wait_for_runtime_event(
        &mut coordinator,
        |event| match event {
            RuntimeEvent::PromptAssemblyUpdated { manager, notice } => Some((manager, notice)),
            _ => None,
        },
        "prompt assembly updated event",
    );

    // 空会话：即使 prelude 未变化（该工具可能无 guidelines），启停变化也应触发
    // 当前会话通知，并立即从 session 工具集移除该工具。
    assert_eq!(
        notice,
        Some(PromptAssemblyUpdateNotice::CurrentEmptySessionUpdated)
    );
    assert!(
        !coordinator
            .session_workspace_tools
            .definitions()
            .definitions()
            .any(|definition| definition.name == disabled_tool_name),
        "disabled tool should leave the session registry immediately on an empty session"
    );
    assert!(
        coordinator
            .workspace_tools
            .definitions()
            .definitions()
            .any(|definition| definition.name == disabled_tool_name),
        "full registry should keep the tool for /prompt inventory"
    );
    cleanup(&root);
}
#[test]
fn disabling_tool_on_started_session_waits_for_next_new_session_reset() {
    let root = temp_test_dir("tool-enablement-started-session");
    let work_dir = root.join("repo");
    fs::create_dir_all(&work_dir).expect("work dir should exist");
    let store: Arc<dyn SessionStore> = Arc::new(InMemorySessionStore::new());
    let mut coordinator = runtime_coordinator(AppRuntimeOptions {
        session_store: Some(store),
        session_header_template: Some(SessionHeader {
            session_id: SessionId::new(),
            work_dir: work_dir.clone(),
            session_name: None,
            initial_model: "qwen3".to_string(),
            git_head: None,
            cli_version: None,
        }),
        ..AppRuntimeOptions::default()
    });
    coordinator
        .provider_conversation
        .append_items(vec![ConversationItem::text(Role::User, "already started")])
        .expect("seed history should succeed");
    let disabled_tool_name = coordinator.prompt_assembly_tool_definitions()[0]
        .name
        .clone();

    coordinator
        .begin_prompt_assembly_edit()
        .expect("begin prompt assembly edit should load working copy");
    coordinator
        .apply_prompt_assembly_edit_mutation(PromptAssemblyMutation::scoped(
            PromptAssemblyScope::Global,
            PromptAssemblyScopedMutationKind::SetToolEnabled {
                tool_name: disabled_tool_name.clone(),
                enabled: false,
            },
        ))
        .expect("tool enablement mutation should apply to working copy");
    coordinator
        .commit_prompt_assembly_edit()
        .expect("commit should persist and emit updated event");
    let (_, notice) = wait_for_runtime_event(
        &mut coordinator,
        |event| match event {
            RuntimeEvent::PromptAssemblyUpdated { manager, notice } => Some((manager, notice)),
            _ => None,
        },
        "prompt assembly updated event",
    );

    assert_eq!(
        notice,
        Some(PromptAssemblyUpdateNotice::NextNewSessionUpdated)
    );
    assert!(
        coordinator
            .session_workspace_tools
            .definitions()
            .definitions()
            .any(|definition| definition.name == disabled_tool_name),
        "started session should keep its tool set until the next new session"
    );

    coordinator
        .handle_runtime_command(RuntimeCommand::Reset)
        .expect("reset should succeed");

    assert!(
        !coordinator
            .session_workspace_tools
            .definitions()
            .definitions()
            .any(|definition| definition.name == disabled_tool_name),
        "disabled tool should leave the session registry after reset"
    );
    cleanup(&root);
}
