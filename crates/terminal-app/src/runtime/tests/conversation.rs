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
