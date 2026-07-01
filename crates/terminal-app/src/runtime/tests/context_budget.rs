use super::support::*;

#[test]
fn context_budget_worker_shutdown_stops_accepting_new_commands() {
    let mut worker = super::super::context_budget_worker::ContextBudgetWorker::new()
        .expect("context budget worker should initialize");

    worker
        .shutdown()
        .expect("fresh context budget worker should shut down cleanly");

    assert!(
        !worker.has_pending_work(),
        "shutdown should clear background work tracking"
    );
    assert!(
        worker
            .load_snapshot(
                super::super::context_budget_worker::ContextBudgetSnapshotRequest {
                    request_id: request_id(77),
                    provider_kind: ProviderKind::OpenAiCompatible,
                    model_id: "qwen3".to_string(),
                    items: std::sync::Arc::from([ConversationItem::text(Role::User, "hello")]),
                    prompt_prelude: None,
                    tool_definitions: Vec::new(),
                    context_limit: runtime_domain::context_budget::ContextTokenLimit::try_from(
                        256_000,
                    )
                    .expect("fixture limit should be valid"),
                }
            )
            .is_err(),
        "shutdown worker should reject new work instead of recreating an implicit thread"
    );
}

#[test]
fn context_budget_snapshot_dispatches_to_background_worker() {
    let mut coordinator = runtime_coordinator(AppRuntimeOptions {
        loaded_models: conversation_runtime::models::LoadedModelCatalog {
            catalog: runtime_domain::model_catalog::ModelCatalog::new(vec![
                runtime_domain::model_catalog::ModelProvider::new(
                    "local",
                    ProviderKind::OpenAiCompatible,
                    "Local",
                    Some("http://127.0.0.1:1234/v1".to_string()),
                    runtime_domain::model_catalog::ModelSource::Configured,
                    vec![runtime_domain::model_catalog::ModelEntry::new(
                        "qwen3",
                        None,
                        runtime_domain::model_catalog::ModelSource::Configured,
                    )],
                ),
            ]),
            ..conversation_runtime::models::LoadedModelCatalog::default()
        },
        ..AppRuntimeOptions::default()
    });
    let selection = ModelSelection::new("local", "qwen3");
    let request_id = request_id(39);

    let receipt = coordinator
        .handle_runtime_command(RuntimeCommand::LoadContextBudgetSnapshot {
            request_id,
            selection,
        })
        .expect("context budget snapshot command should be accepted");

    assert_eq!(receipt, RuntimeCommandReceipt::Accepted);
    assert!(
        RuntimeCoordinator::has_background_runtime(&coordinator),
        "context budget snapshot should be computed through background runtime work"
    );

    let payload = wait_for_runtime_event(
        &mut coordinator,
        |event| match event {
            RuntimeEvent::ContextBudgetSnapshotLoaded {
                request_id: actual_request_id,
                payload,
            } if actual_request_id == request_id => Some(payload),
            _ => None,
        },
        "context budget snapshot payload",
    );

    assert!(
        payload.total_estimated_tokens > 0,
        "background snapshot should eventually produce the context payload"
    );
}

#[test]
fn context_budget_snapshot_includes_provider_visible_tool_definitions() {
    let mut coordinator = runtime_coordinator(AppRuntimeOptions {
        loaded_models: conversation_runtime::models::LoadedModelCatalog {
            catalog: runtime_domain::model_catalog::ModelCatalog::new(vec![
                runtime_domain::model_catalog::ModelProvider::new(
                    "local",
                    ProviderKind::OpenAiCompatible,
                    "Local",
                    Some("http://127.0.0.1:1234/v1".to_string()),
                    runtime_domain::model_catalog::ModelSource::Configured,
                    vec![runtime_domain::model_catalog::ModelEntry::new(
                        "qwen3",
                        None,
                        runtime_domain::model_catalog::ModelSource::Configured,
                    )],
                ),
            ]),
            ..conversation_runtime::models::LoadedModelCatalog::default()
        },
        ..AppRuntimeOptions::default()
    });
    let selection = ModelSelection::new("local", "qwen3");
    let request_id = request_id(40);

    coordinator
        .handle_runtime_command(RuntimeCommand::LoadContextBudgetSnapshot {
            request_id,
            selection,
        })
        .expect("context budget snapshot command should be accepted");

    let payload = wait_for_runtime_event(
        &mut coordinator,
        |event| match event {
            RuntimeEvent::ContextBudgetSnapshotLoaded {
                request_id: actual_request_id,
                payload,
            } if actual_request_id == request_id => Some(payload),
            _ => None,
        },
        "context budget snapshot payload",
    );

    assert!(
        payload.segments.iter().any(|segment| {
            segment.kind == runtime_domain::context_budget::SegmentKind::ToolDefinitions
                && segment.estimated_tokens > 0
        }),
        "context budget snapshot should include non-empty provider-visible tool definitions"
    );
}

#[test]
fn context_budget_snapshot_separates_skill_discovery_from_system_prompt() {
    let mut coordinator = runtime_coordinator(AppRuntimeOptions {
        loaded_models: conversation_runtime::models::LoadedModelCatalog {
            catalog: runtime_domain::model_catalog::ModelCatalog::new(vec![
                runtime_domain::model_catalog::ModelProvider::new(
                    "local",
                    ProviderKind::OpenAiCompatible,
                    "Local",
                    Some("http://127.0.0.1:1234/v1".to_string()),
                    runtime_domain::model_catalog::ModelSource::Configured,
                    vec![runtime_domain::model_catalog::ModelEntry::new(
                        "qwen3",
                        None,
                        runtime_domain::model_catalog::ModelSource::Configured,
                    )],
                ),
            ]),
            ..conversation_runtime::models::LoadedModelCatalog::default()
        },
        initial_prompt_prelude: Some(runtime_domain::prompt_assembly::PromptPreludeSnapshot {
            sections: vec![
                runtime_domain::prompt_assembly::PromptPreludeSection {
                    reference_id: "core-system".to_string(),
                    kind: runtime_domain::prompt_assembly::PromptSourceKind::CoreSystemPrompt,
                    title: "Core system prompt".to_string(),
                    origin: Some(runtime_domain::prompt_assembly::PromptSourceOrigin::Builtin),
                    body: "keep responses direct".to_string(),
                },
                runtime_domain::prompt_assembly::PromptPreludeSection {
                    reference_id: "skill-discovery".to_string(),
                    kind: runtime_domain::prompt_assembly::PromptSourceKind::SkillDiscovery,
                    title: "Skill discovery".to_string(),
                    origin: Some(runtime_domain::prompt_assembly::PromptSourceOrigin::Project),
                    body: "<available_skills>code-review</available_skills>".to_string(),
                },
            ],
        }),
        ..AppRuntimeOptions::default()
    });
    let request_id = request_id(401);

    coordinator
        .handle_runtime_command(RuntimeCommand::LoadContextBudgetSnapshot {
            request_id,
            selection: ModelSelection::new("local", "qwen3"),
        })
        .expect("context budget snapshot command should be accepted");

    let payload = wait_for_runtime_event(
        &mut coordinator,
        |event| match event {
            RuntimeEvent::ContextBudgetSnapshotLoaded {
                request_id: actual_request_id,
                payload,
            } if actual_request_id == request_id => Some(payload),
            _ => None,
        },
        "context budget snapshot payload",
    );
    let non_tool_segments = payload
        .segments
        .iter()
        .filter(|segment| {
            segment.kind != runtime_domain::context_budget::SegmentKind::ToolDefinitions
        })
        .collect::<Vec<_>>();

    assert_eq!(
        non_tool_segments
            .iter()
            .map(|segment| segment.kind)
            .collect::<Vec<_>>(),
        vec![
            runtime_domain::context_budget::SegmentKind::System,
            runtime_domain::context_budget::SegmentKind::SkillDiscovery,
        ],
        "prompt prelude should keep core system prompt and skill discovery as separate `/context` segments"
    );
    assert!(
        non_tool_segments
            .iter()
            .all(|segment| segment.estimated_tokens > 0),
        "both prompt prelude segments should keep non-zero token estimates"
    );
}

#[test]
fn context_budget_snapshot_failure_is_reported_as_runtime_event() {
    let mut coordinator = runtime_coordinator(AppRuntimeOptions {
        loaded_models: conversation_runtime::models::LoadedModelCatalog {
            catalog: runtime_domain::model_catalog::ModelCatalog::new(vec![
                runtime_domain::model_catalog::ModelProvider::new(
                    "anthropic",
                    ProviderKind::Anthropic,
                    "Anthropic",
                    None,
                    runtime_domain::model_catalog::ModelSource::Configured,
                    vec![runtime_domain::model_catalog::ModelEntry::new(
                        "claude-sonnet-4",
                        None,
                        runtime_domain::model_catalog::ModelSource::Configured,
                    )],
                ),
            ]),
            ..conversation_runtime::models::LoadedModelCatalog::default()
        },
        ..AppRuntimeOptions::default()
    });
    let request_id = request_id(41);
    let selection = ModelSelection::new("anthropic", "claude-sonnet-4");

    let receipt = coordinator
        .handle_runtime_command(RuntimeCommand::LoadContextBudgetSnapshot {
            request_id,
            selection,
        })
        .expect("context budget load command should stay accepted and fail via runtime event");

    assert_eq!(receipt, RuntimeCommandReceipt::Accepted);

    let message = wait_for_runtime_event(
        &mut coordinator,
        |event| match event {
            RuntimeEvent::ContextBudgetSnapshotLoadFailed {
                request_id: actual_request_id,
                error:
                    runtime_domain::session::ContextBudgetLoadErrorPayload::UnsupportedProvider {
                        provider_kind,
                    },
            } if actual_request_id == request_id => Some(provider_kind),
            _ => None,
        },
        "context budget snapshot failure event",
    );

    assert!(
        matches!(message, ProviderKind::Anthropic),
        "unsupported provider failures should keep their structured provider kind"
    );
}

#[test]
fn context_budget_projection_failure_keeps_structured_error_kind() {
    let mut coordinator = runtime_coordinator(AppRuntimeOptions {
        loaded_models: conversation_runtime::models::LoadedModelCatalog {
            catalog: runtime_domain::model_catalog::ModelCatalog::new(vec![
                runtime_domain::model_catalog::ModelProvider::new(
                    "local",
                    ProviderKind::OpenAiCompatible,
                    "Local",
                    Some("http://127.0.0.1:1234/v1".to_string()),
                    runtime_domain::model_catalog::ModelSource::Configured,
                    vec![runtime_domain::model_catalog::ModelEntry::new(
                        "qwen3",
                        None,
                        runtime_domain::model_catalog::ModelSource::Configured,
                    )],
                ),
            ]),
            ..conversation_runtime::models::LoadedModelCatalog::default()
        },
        ..AppRuntimeOptions::default()
    });
    coordinator
        .provider_conversation
        .append_items(vec![ConversationItem::tool_result(
            "missing-call",
            vec![ContentBlock::Text("tool output".to_string())],
            false,
        )])
        .expect("fixture items should append");

    let request_id = request_id(42);
    let selection = ModelSelection::new("local", "qwen3");

    let receipt = coordinator
        .handle_runtime_command(RuntimeCommand::LoadContextBudgetSnapshot {
            request_id,
            selection,
        })
        .expect("projection failures should be reported via runtime event");

    assert_eq!(receipt, RuntimeCommandReceipt::Accepted);

    let error_kind = wait_for_runtime_event(
        &mut coordinator,
        |event| match event {
            RuntimeEvent::ContextBudgetSnapshotLoadFailed {
                request_id: actual_request_id,
                error:
                    runtime_domain::session::ContextBudgetLoadErrorPayload::ProjectionFailed {
                        kind,
                        ..
                    },
            } if actual_request_id == request_id => Some(kind),
            _ => None,
        },
        "context budget projection failure event",
    );

    assert_eq!(
        error_kind,
        runtime_domain::session::ContextBudgetProjectionErrorKind::Protocol
    );
}

#[test]
fn cancel_context_budget_snapshot_stops_background_tracking_and_drops_stale_events() {
    let mut coordinator = runtime_coordinator(AppRuntimeOptions {
        loaded_models: conversation_runtime::models::LoadedModelCatalog {
            catalog: runtime_domain::model_catalog::ModelCatalog::new(vec![
                runtime_domain::model_catalog::ModelProvider::new(
                    "local",
                    ProviderKind::OpenAiCompatible,
                    "Local",
                    Some("http://127.0.0.1:1234/v1".to_string()),
                    runtime_domain::model_catalog::ModelSource::Configured,
                    vec![runtime_domain::model_catalog::ModelEntry::new(
                        "qwen3",
                        None,
                        runtime_domain::model_catalog::ModelSource::Configured,
                    )],
                ),
            ]),
            ..conversation_runtime::models::LoadedModelCatalog::default()
        },
        ..AppRuntimeOptions::default()
    });
    coordinator
        .provider_conversation
        .append_items(vec![ConversationItem::text(
            Role::User,
            "context budget ".repeat(250_000),
        )])
        .expect("fixture items should append");

    coordinator
        .handle_runtime_command(RuntimeCommand::LoadContextBudgetSnapshot {
            request_id: request_id(43),
            selection: ModelSelection::new("local", "qwen3"),
        })
        .expect("context budget snapshot command should be accepted");
    assert!(RuntimeCoordinator::has_background_runtime(&coordinator));

    coordinator
        .handle_runtime_command(RuntimeCommand::CancelContextBudgetSnapshot)
        .expect("cancel context budget command should be accepted");

    assert!(
        !RuntimeCoordinator::has_background_runtime(&coordinator),
        "cancel should clear `/context` background tracking immediately"
    );
    wait_for_runtime_idle(&mut coordinator);
}

#[test]
fn latest_context_budget_request_supersedes_stale_work() {
    let mut coordinator = runtime_coordinator(AppRuntimeOptions {
        loaded_models: conversation_runtime::models::LoadedModelCatalog {
            catalog: runtime_domain::model_catalog::ModelCatalog::new(vec![
                runtime_domain::model_catalog::ModelProvider::new(
                    "local",
                    ProviderKind::OpenAiCompatible,
                    "Local",
                    Some("http://127.0.0.1:1234/v1".to_string()),
                    runtime_domain::model_catalog::ModelSource::Configured,
                    vec![runtime_domain::model_catalog::ModelEntry::new(
                        "qwen3",
                        None,
                        runtime_domain::model_catalog::ModelSource::Configured,
                    )],
                ),
            ]),
            ..conversation_runtime::models::LoadedModelCatalog::default()
        },
        ..AppRuntimeOptions::default()
    });
    coordinator
        .provider_conversation
        .append_items(vec![ConversationItem::text(
            Role::User,
            "context budget ".repeat(250_000),
        )])
        .expect("fixture items should append");

    let first_request_id = request_id(44);
    let second_request_id = request_id(45);
    let selection = ModelSelection::new("local", "qwen3");

    coordinator
        .handle_runtime_command(RuntimeCommand::LoadContextBudgetSnapshot {
            request_id: first_request_id,
            selection: selection.clone(),
        })
        .expect("first context budget snapshot command should be accepted");
    coordinator
        .handle_runtime_command(RuntimeCommand::LoadContextBudgetSnapshot {
            request_id: second_request_id,
            selection,
        })
        .expect("second context budget snapshot command should be accepted");

    let mut loaded_request_ids = Vec::new();
    for _ in 0..100 {
        for event in RuntimeCoordinator::drain_runtime_events(&mut coordinator) {
            if let RuntimeEvent::ContextBudgetSnapshotLoaded { request_id, .. } = event {
                loaded_request_ids.push(request_id);
            }
        }
        if !RuntimeCoordinator::has_background_runtime(&coordinator) {
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }

    assert_eq!(
        loaded_request_ids,
        vec![second_request_id],
        "only the latest `/context` request should complete after stale work is superseded"
    );
}
