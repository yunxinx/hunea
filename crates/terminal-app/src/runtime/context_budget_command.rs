//! Builds context budget snapshot for the `/context` overlay.

use conversation_runtime::context_budget::{
    context_budget_from_items, context_budget_tool_definitions_text,
};
use runtime_domain::{
    context_budget::{ContextBudgetSnapshot, ContextLimitDisplay},
    model_catalog::ModelSelection,
    session::{
        ContextBudgetDisplayPayload, ContextBudgetSegmentPayload, ContextBudgetSnapshotPayload,
        RuntimeCommandReceipt, RuntimeEvent,
    },
};

use super::AppRuntimeCoordinator;

impl AppRuntimeCoordinator {
    pub(super) fn load_context_budget_snapshot_command(
        &mut self,
        selection: &ModelSelection,
    ) -> Result<RuntimeCommandReceipt, String> {
        let model_id = selection.model_id.clone();
        let context_limit = self
            .options
            .model_catalog
            .context_limit_for(&self.options.context_limits, selection);
        let items = self
            .provider_conversation
            .provider_items_for_context_budget_probe();
        let tool_definitions_text = context_budget_tool_definitions_text(&self.workspace_tools)
            .map_err(|error| format!("Serialize provider-visible tool definitions: {error}"))?;
        let snapshot = context_budget_from_items(
            &model_id,
            &items,
            tool_definitions_text.as_deref(),
            context_limit,
        );
        self.pending_runtime_events
            .push(RuntimeEvent::ContextBudgetSnapshotLoaded {
                payload: snapshot_to_payload(snapshot),
            });
        Ok(RuntimeCommandReceipt::Accepted)
    }
}

fn snapshot_to_payload(snapshot: ContextBudgetSnapshot) -> ContextBudgetSnapshotPayload {
    ContextBudgetSnapshotPayload {
        model_id: snapshot.model_id,
        total_estimated_tokens: snapshot.total_estimated_tokens,
        context_limit: snapshot.context_limit,
        display: display_to_payload(snapshot.display),
        segments: snapshot
            .segments
            .into_iter()
            .map(|segment| ContextBudgetSegmentPayload {
                kind: segment.kind,
                stack_order: segment.stack_order,
                estimated_tokens: segment.estimated_tokens,
                label: segment.label,
            })
            .collect(),
    }
}

fn display_to_payload(display: ContextLimitDisplay) -> ContextBudgetDisplayPayload {
    match display {
        ContextLimitDisplay::Relative { used } => ContextBudgetDisplayPayload::Relative { used },
        ContextLimitDisplay::Absolute {
            limit,
            used,
            percent,
        } => ContextBudgetDisplayPayload::Absolute {
            limit,
            used,
            percent,
        },
    }
}
