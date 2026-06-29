#[cfg(test)]
mod tests {
    use runtime_domain::{context_budget::SegmentKind, session::ContextBudgetSegmentPayload};

    #[test]
    fn segment_kind_is_carried_as_a_strong_type() {
        let payload = ContextBudgetSegmentPayload {
            kind: SegmentKind::ToolDefinitions,
            stack_order: 3,
            estimated_tokens: 42,
        };

        assert_eq!(payload.kind, SegmentKind::ToolDefinitions);
    }
}
