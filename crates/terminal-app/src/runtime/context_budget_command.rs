//! Builds context budget snapshot for the `/context` overlay.

use runtime_domain::{
    model_catalog::ModelSelection,
    session::{
        ContextBudgetLoadErrorPayload, RuntimeCommandReceipt, RuntimeEvent, SessionLoadRequestId,
    },
};

use super::{
    AppRuntimeCoordinator,
    context_budget_worker::{
        ContextBudgetSnapshotRequest, context_budget_tool_definitions_for_worker,
    },
};

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
            self.context_budget_unknown_provider_event(request_id, selection.provider_id.clone());
            return Ok(RuntimeCommandReceipt::Accepted);
        };
        let items = self.provider_conversation.context_budget_probe_items();
        let tool_definitions = context_budget_tool_definitions_for_worker(&self.workspace_tools);
        if let Err(error) = self
            .context_budget_worker
            .load_snapshot(ContextBudgetSnapshotRequest {
                request_id,
                provider_kind: provider.connection().kind,
                model_id: selection.model_id.clone(),
                items,
                prompt_prelude: self.provider_conversation.prompt_prelude().cloned(),
                tool_definitions,
                context_limit: self.options.loaded_models.context_limit_for(selection),
                upstream_context_tokens: self.provider_conversation.upstream_context_tokens(),
            })
        {
            self.pending_runtime_events
                .push(RuntimeEvent::ContextBudgetSnapshotLoadFailed {
                    request_id,
                    error: error.into_payload(),
                });
        }
        Ok(RuntimeCommandReceipt::Accepted)
    }

    pub(super) fn cancel_context_budget_snapshot_command(&mut self) -> RuntimeCommandReceipt {
        self.context_budget_worker.cancel_pending();
        RuntimeCommandReceipt::Accepted
    }
}

impl AppRuntimeCoordinator {
    pub(super) fn context_budget_unknown_provider_event(
        &mut self,
        request_id: SessionLoadRequestId,
        provider_id: String,
    ) {
        self.pending_runtime_events
            .push(RuntimeEvent::ContextBudgetSnapshotLoadFailed {
                request_id,
                error: ContextBudgetLoadErrorPayload::UnknownProvider { provider_id },
            });
    }
}
