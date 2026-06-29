//! Builds context budget snapshot for the `/context` overlay.

use conversation_runtime::context_budget::{
    ContextBudgetProbe, build_context_budget_snapshot, context_budget_tool_definitions,
};
use runtime_domain::{
    context_budget::{ContextBudgetSnapshot, ContextLimitDisplay},
    model_catalog::ModelSelection,
    session::{
        ContextBudgetDisplayPayload, ContextBudgetLoadErrorPayload, ContextBudgetSegmentPayload,
        ContextBudgetSnapshotPayload, RuntimeCommandReceipt, RuntimeEvent, SessionLoadRequestId,
    },
};

use super::AppRuntimeCoordinator;

impl AppRuntimeCoordinator {
    pub(super) fn load_context_budget_snapshot_command(
        &mut self,
        request_id: SessionLoadRequestId,
        selection: &ModelSelection,
    ) -> Result<RuntimeCommandReceipt, String> {
        let Some(provider) = self
            .options
            .loaded_models
            .catalog
            .enabled_provider_by_id(&selection.provider_id)
        else {
            self.pending_runtime_events
                .push(RuntimeEvent::ContextBudgetSnapshotLoadFailed {
                    request_id,
                    error: ContextBudgetLoadErrorPayload::UnknownProvider {
                        provider_id: selection.provider_id.clone(),
                    },
                });
            return Ok(RuntimeCommandReceipt::Accepted);
        };
        let model_id = selection.model_id.clone();
        let context_limit = self.options.loaded_models.context_limit_for(selection);
        let items = self.provider_conversation.context_budget_probe_items();
        let tool_definitions = context_budget_tool_definitions(&self.workspace_tools);
        let snapshot = match build_context_budget_snapshot(ContextBudgetProbe::new(
            provider.connection().kind,
            &model_id,
            items,
            &tool_definitions,
            context_limit,
        )) {
            Ok(snapshot) => snapshot,
            Err(error) => {
                self.pending_runtime_events
                    .push(RuntimeEvent::ContextBudgetSnapshotLoadFailed {
                        request_id,
                        error: context_budget_load_error_payload(error),
                    });
                return Ok(RuntimeCommandReceipt::Accepted);
            }
        };
        self.pending_runtime_events
            .push(RuntimeEvent::ContextBudgetSnapshotLoaded {
                request_id,
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
            })
            .collect(),
    }
}

fn display_to_payload(display: ContextLimitDisplay) -> ContextBudgetDisplayPayload {
    match display {
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

fn context_budget_load_error_payload(
    error: conversation_runtime::ContextBudgetError,
) -> ContextBudgetLoadErrorPayload {
    match error {
        conversation_runtime::ContextBudgetError::UnsupportedProvider { provider_kind } => {
            ContextBudgetLoadErrorPayload::UnsupportedProvider { provider_kind }
        }
        conversation_runtime::ContextBudgetError::Projection { failure, .. } => {
            ContextBudgetLoadErrorPayload::ProjectionFailed {
                kind: failure.kind,
                status: failure.status,
                detail: failure.detail,
            }
        }
    }
}
