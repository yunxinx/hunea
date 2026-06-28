use super::support::*;

#[test]
fn context_budget_snapshot_includes_provider_visible_tool_definitions() {
    let mut coordinator = runtime_coordinator(AppRuntimeOptions::default());
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
