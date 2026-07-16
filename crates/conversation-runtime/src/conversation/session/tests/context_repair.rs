use super::support::*;

#[test]
fn provider_context_repair_items_fill_unresolved_tool_calls() {
    let mut ledger = ProviderContextRepairLedger::default();

    ledger.observe(&ConversationItem::assistant_with_tool_calls(
        String::new(),
        vec![
            ToolCall::new("call-1", "read", "{}"),
            ToolCall::new("call-2", "search", "{}"),
        ],
    ));
    ledger.observe(&ConversationItem::tool_result(
        "call-1",
        vec![ContentBlock::Text("done".into())],
        false,
    ));

    let repair_items = ledger.take_repair_items(TOOL_EXECUTION_INTERRUPTED);

    assert_eq!(repair_items.len(), 1);
    let repair_item = &repair_items[0];
    match repair_item {
        ConversationItem::ToolResult {
            call_id,
            is_error,
            content,
        } => {
            assert_eq!(call_id, "call-2");
            assert!(*is_error);
            assert_eq!(content[0].as_text(), Some(TOOL_EXECUTION_INTERRUPTED));
        }
        _ => panic!("expected ToolResult item"),
    }
    assert!(
        ledger
            .take_repair_items(TOOL_EXECUTION_INTERRUPTED)
            .is_empty()
    );
}
