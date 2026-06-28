use super::support::*;

#[test]
fn context_budget_snapshot_includes_provider_visible_tool_definitions() {
    let mut coordinator = runtime_coordinator(AppRuntimeOptions {
        model_catalog: runtime_domain::model_catalog::ModelCatalog::new(vec![
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
        ..AppRuntimeOptions::default()
    });
    let selection = ModelSelection::new("local", "qwen3");

    coordinator
        .handle_runtime_command(RuntimeCommand::LoadContextBudgetSnapshot { selection })
        .expect("context budget snapshot command should be accepted");

    let payload = wait_for_runtime_event(
        &mut coordinator,
        |event| match event {
            RuntimeEvent::ContextBudgetSnapshotLoaded { payload } => Some(payload),
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
