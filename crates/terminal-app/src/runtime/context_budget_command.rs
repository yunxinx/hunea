//! Builds context budget snapshot for the `/context` overlay.

use runtime_domain::{
    model_catalog::ModelSelection,
    session::{
        ContextBudgetLoadErrorPayload, RuntimeCommandReceipt, RuntimeEvent, SessionLoadRequestId,
    },
};

use super::{
    AppRuntimeCoordinator, context_budget_worker::context_budget_tool_definitions_for_worker,
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
        let items = self
            .provider_conversation
            .context_budget_probe_items()
            .into_iter()
            // 后台线程不能借用协调器里的会话历史，这里显式转成 owned 输入。
            // `/context` 是按需命令，接受一次探测时的 clone，换取主循环不被同步重算阻塞。
            .map(|item| item.into_owned())
            .collect();
        let tool_definitions = context_budget_tool_definitions_for_worker(&self.workspace_tools);
        self.context_budget_worker.load_snapshot(
            request_id,
            provider.connection().kind,
            selection.model_id.clone(),
            items,
            tool_definitions,
            self.options.loaded_models.context_limit_for(selection),
        )?;
        Ok(RuntimeCommandReceipt::Accepted)
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
