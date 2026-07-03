use runtime_domain::prompt_assembly::{
    PromptPreludeSection, PromptPreludeSnapshot, PromptSourceKind, PromptSourceOrigin,
    persistence::{
        PersistedPromptAssemblyEntry, PromptAssemblyScope, PromptAssemblyScopeState,
        StoredPromptBody, project_custom_prompts_dir, save_project_prompt_assembly_state,
    },
};
use runtime_domain::session::PromptAssemblyUpdateNotice;

use super::support::*;

#[test]
fn reload_prompt_assembly_reads_latest_filesystem_state() {
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
        &PromptAssemblyScopeState {
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
        .handle_runtime_command(RuntimeCommand::ReloadPromptAssembly)
        .expect("reload prompt assembly should be accepted");

    let initial_manager = wait_for_runtime_event(
        &mut coordinator,
        |event| match event {
            RuntimeEvent::PromptAssemblyUpdated { manager, .. } => Some(manager),
            _ => None,
        },
        "initial prompt assembly snapshot",
    );

    assert!(
        initial_manager
            .extra_prompt_candidates
            .iter()
            .any(|prompt| prompt.reference_id == "review-rules")
    );
    assert!(
        initial_manager
            .discovered_skills
            .iter()
            .any(|skill| skill.skill_name == "repo-bootstrap")
    );
    assert!(
        initial_manager
            .manual_skills
            .iter()
            .any(|skill| skill.skill_name == "repo-bootstrap")
    );

    fs::remove_file(project_custom_prompts_dir(&work_dir).join("review-rules.md"))
        .expect("custom prompt file should be removable");
    fs::remove_file(skill_dir.join("SKILL.md")).expect("skill file should be removable");

    coordinator
        .handle_runtime_command(RuntimeCommand::ReloadPromptAssembly)
        .expect("reload prompt assembly should be accepted after filesystem changes");

    let reloaded_manager = wait_for_runtime_event(
        &mut coordinator,
        |event| match event {
            RuntimeEvent::PromptAssemblyUpdated { manager, .. } => Some(manager),
            _ => None,
        },
        "reloaded prompt assembly snapshot",
    );

    assert!(
        reloaded_manager
            .extra_prompt_candidates
            .iter()
            .all(|prompt| prompt.reference_id != "review-rules")
    );
    assert!(
        reloaded_manager
            .discovered_skills
            .iter()
            .all(|skill| !(skill.origin
                == runtime_domain::prompt_assembly::PromptSourceOrigin::Project
                && skill.skill_name == "repo-bootstrap"))
    );
    assert!(
        reloaded_manager
            .manual_skills
            .iter()
            .all(|skill| !(skill.origin
                == runtime_domain::prompt_assembly::PromptSourceOrigin::Project
                && skill.skill_name == "repo-bootstrap"))
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
        &PromptAssemblyScopeState {
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
        .handle_runtime_command(RuntimeCommand::MutatePromptAssembly {
            mutation:
                runtime_domain::prompt_assembly::PromptAssemblyMutation::SetPromptSourceEnabled {
                    scope: PromptAssemblyScope::Project,
                    kind: PromptSourceKind::ExtraPrompt,
                    reference_id: "review-rules".to_string(),
                    enabled: true,
                },
        })
        .expect("prompt assembly mutation should be accepted");
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
        Some(&updated_manager.prelude)
    );
    assert_eq!(
        coordinator.options.initial_prompt_prelude.as_ref(),
        Some(&updated_manager.prelude)
    );
    assert_eq!(
        notice,
        Some(PromptAssemblyUpdateNotice::CurrentEmptySessionUpdated)
    );
    assert_ne!(Some(&updated_manager.prelude), Some(&initial_prelude));
    cleanup(&root);
}

#[test]
fn prompt_assembly_changes_on_started_session_apply_only_after_next_new_session_reset() {
    let root = temp_test_dir("prompt-assembly-started-session");
    let work_dir = root.join("repo");
    fs::create_dir_all(&work_dir).expect("work dir should exist");
    save_project_prompt_assembly_state(
        &work_dir,
        &PromptAssemblyScopeState {
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
        .handle_runtime_command(RuntimeCommand::MutatePromptAssembly {
            mutation:
                runtime_domain::prompt_assembly::PromptAssemblyMutation::SetPromptSourceEnabled {
                    scope: PromptAssemblyScope::Project,
                    kind: PromptSourceKind::ExtraPrompt,
                    reference_id: "review-rules".to_string(),
                    enabled: true,
                },
        })
        .expect("prompt assembly mutation should be accepted");
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
        Some(&updated_manager.prelude)
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
