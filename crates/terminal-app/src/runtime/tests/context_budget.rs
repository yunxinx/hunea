use super::support::*;

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
