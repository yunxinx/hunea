use super::support::*;

#[test]
fn conversation_target_must_match_running_worker() {
    let active_target = RuntimeTarget::provider("openai", "gpt-4o-mini");
    assert!(ensure_conversation_target(Some(&active_target), None).is_ok());
    assert!(ensure_conversation_target(Some(&active_target), Some(&active_target)).is_ok());

    let inactive_target = RuntimeTarget::provider("openai", "gpt-4.1-mini");
    let inactive_error = ensure_conversation_target(Some(&active_target), Some(&inactive_target))
        .expect_err("wrong conversation target should be rejected");
    assert!(inactive_error.contains("Conversation is not active"));

    let stopped_error = ensure_conversation_target(None, Some(&active_target))
        .expect_err("explicit conversation target should require a running worker");
    assert!(stopped_error.contains("Conversation is not running"));
}

#[test]
fn token_estimate_creates_render_barrier_before_permission_request() {
    let output_batch = vec![RuntimeEvent::OutputTokenEstimate {
        target: Some(RuntimeTarget::provider("local", "qwen3")),
        total_tokens: 57,
    }];
    let input_batch = vec![RuntimeEvent::InputTokenEstimate {
        target: Some(RuntimeTarget::provider("local", "qwen3")),
        total_tokens: 12,
    }];
    let permission_event = RuntimeEvent::PermissionRequested {
        target: RuntimeTarget::provider("local", "qwen3"),
        request: RuntimePermissionRequest::new(
            "permission-1",
            Some("Write temp.md".into()),
            vec![],
        ),
    };

    assert!(
        should_defer_runtime_event_for_render_barrier(&output_batch, &permission_event),
        "permission should wait for the output token estimate batch to render first"
    );
    assert!(
        should_defer_runtime_event_for_render_barrier(&input_batch, &permission_event),
        "permission should wait for the input token estimate batch to render first"
    );
    assert!(
        !should_defer_runtime_event_for_render_barrier(&[], &permission_event),
        "permission should not be deferred when there is no token estimate to render"
    );
}

#[test]
fn app_layer_persists_managed_search_tool_authorization() {
    let root = temp_test_dir("managed-search-authorization");
    let config_path = root.join("config.toml");
    let mut coordinator = runtime_coordinator(AppRuntimeOptions {
        managed_search_authorization_config_path: Some(config_path.clone()),
        ..AppRuntimeOptions::default()
    });

    let event = coordinator.persist_managed_search_tool_authorization(ManagedSearchTool::Fd, None);

    assert_eq!(event, None);
    assert_eq!(
        coordinator.options.managed_search_tools.allow_managed_fd,
        Some(true)
    );
    let content = fs::read_to_string(&config_path).expect("config should be readable");
    assert!(content.contains("allow_managed_fd = true"));
    cleanup(&root);
}

#[test]
fn conversation_failure_before_provider_request_rolls_back_pending_user() {
    let mut coordinator = runtime_coordinator(AppRuntimeOptions {
        runtime_request_policy: runtime_domain::request_policy::RuntimeRequestPolicy::new(
            0,
            Vec::new(),
            1,
        ),
        ..AppRuntimeOptions::default()
    });
    let request = ConversationTurnRequest::new(
        "openai",
        ProviderKind::OpenAi,
        "gpt-4o-mini",
        None,
        None,
        None,
        ConversationItem::text(Role::User, "hello"),
    );
    let target = request.target();

    coordinator
        .handle_runtime_command(RuntimeCommand::SubmitConversationTurn { target, request })
        .expect("conversation request should start");

    let mut events = Vec::new();
    for _ in 0..50 {
        events.extend(RuntimeCoordinator::drain_runtime_events(&mut coordinator));
        if events
            .iter()
            .any(|event| matches!(event, RuntimeEvent::Failed { .. }))
        {
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }

    assert!(
        events
            .iter()
            .any(|event| matches!(event, RuntimeEvent::Failed { .. })),
        "preflight failure should be reported"
    );
    assert!(coordinator.provider_conversation.is_history_empty());

    let next_request = ConversationTurnRequest::new(
        "local",
        ProviderKind::OpenAiCompatible,
        "qwen3",
        Some("http://127.0.0.1:1234/v1".to_string()),
        None,
        None,
        ConversationItem::text(Role::User, "next"),
    );
    coordinator
        .provider_conversation
        .prepare_turn(&next_request)
        .expect("failed preflight turn should not leave stale pending state");
}

#[test]
fn startup_prompt_missing_source_check_emits_aggregated_runtime_event() {
    let root = temp_test_dir("prompt-missing-check");
    let work_dir = root.join("repo");
    fs::create_dir_all(&work_dir).expect("work dir should exist");
    let store: Arc<dyn SessionStore> = Arc::new(InMemorySessionStore::new());
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime should build");
    runtime
        .block_on(store.save_global_prompt_assembly_state(
            &runtime_domain::prompt_assembly::persistence::PromptAssemblyScopeState {
                scope: runtime_domain::prompt_assembly::persistence::PromptAssemblyScope::Global,
                core_system_override: None,
                entries: vec![
                    runtime_domain::prompt_assembly::persistence::PersistedPromptAssemblyEntry {
                        reference_id: "missing-skill".to_string(),
                        kind: runtime_domain::prompt_assembly::PromptSourceKind::LongLivedSkill,
                        title: "missing-skill".to_string(),
                        enabled: true,
                        requested_order: Some(10),
                    },
                ],
                extra_prompts: Vec::new(),
            },
        ))
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
        .handle_runtime_command(RuntimeCommand::CheckPromptAssemblyMissingSources)
        .expect("startup prompt check should be accepted");

    let missing_count = wait_for_runtime_event(
        &mut coordinator,
        |event| match event {
            RuntimeEvent::PromptAssemblyMissingSourcesChecked { missing_count } => {
                Some(missing_count)
            }
            _ => None,
        },
        "prompt missing check result",
    );

    assert_eq!(missing_count, 1);
    cleanup(&root);
}

#[test]
fn manual_skill_mentions_emit_synthetic_skill_usage_events_before_worker_failure() {
    let root = temp_test_dir("manual-skill-events");
    let work_dir = root.join("repo");
    let skill_dir = work_dir.join(".agents/skills/code-review");
    fs::create_dir_all(&skill_dir).expect("skill dir should exist");
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: code-review\ndescription: Review code\n---\n# Code Review\n\nReview carefully.\n",
    )
    .expect("skill file should exist");

    let store: Arc<dyn SessionStore> = Arc::new(InMemorySessionStore::new());
    let mut coordinator = runtime_coordinator(AppRuntimeOptions {
        runtime_request_policy: runtime_domain::request_policy::RuntimeRequestPolicy::new(
            0,
            Vec::new(),
            1,
        ),
        session_store: Some(store),
        session_header_template: Some(SessionHeader {
            session_id: SessionId::new(),
            work_dir: work_dir.clone(),
            session_name: None,
            initial_model: "gpt-4o-mini".to_string(),
            git_head: None,
            cli_version: None,
        }),
        ..AppRuntimeOptions::default()
    });
    let request = ConversationTurnRequest::new(
        "openai",
        ProviderKind::OpenAi,
        "gpt-4o-mini",
        None,
        None,
        None,
        ConversationItem::text(Role::User, "Please audit this diff with $code-review"),
    );
    let target = request.target();

    coordinator
        .handle_runtime_command(RuntimeCommand::SubmitConversationTurn { target, request })
        .expect("conversation should start");

    let events = RuntimeCoordinator::drain_runtime_events(&mut coordinator);
    assert!(matches!(
        events.as_slice(),
        [RuntimeEvent::ToolActivityStarted { activity, .. }]
            if activity.title.ends_with(".agents/skills/code-review/SKILL.md")
                && activity.raw_input.as_ref().and_then(|raw| raw.string_field(&["hunea_skill_name"]))
                    == Some("code-review".to_string())
    ));

    let failure = wait_for_runtime_event(
        &mut coordinator,
        |event| match event {
            RuntimeEvent::Failed { message, .. } => Some(message),
            _ => None,
        },
        "worker failure after synthetic skill usage event",
    );
    assert!(
        failure.contains("requires API key"),
        "worker should still fail through the normal runtime path: {failure}"
    );
    cleanup(&root);
}
