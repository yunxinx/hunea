use runtime_domain::session::{PromptAssemblyUpdateNotice, RuntimeCommandReceipt, RuntimeEvent};
use tool_runtime::ToolDefinition;

use super::AppRuntimeCoordinator;
use crate::prompt_assembly::{
    apply_prompt_assembly_mutation, load_prompt_assembly_manager_snapshot,
};

impl AppRuntimeCoordinator {
    fn current_session_accepts_prompt_prelude_refresh(&self) -> bool {
        !self.conversation_worker.is_running()
            && self.provider_conversation.is_history_empty()
            && self.provider_conversation.session_id().is_none()
    }

    fn prompt_assembly_update_notice(
        &mut self,
        prelude_changed: bool,
        manager: &runtime_domain::prompt_assembly::PromptAssemblyManagerSnapshot,
    ) -> Option<PromptAssemblyUpdateNotice> {
        if !prelude_changed {
            return None;
        }
        if !self.current_session_accepts_prompt_prelude_refresh() {
            return Some(PromptAssemblyUpdateNotice::NextNewSessionUpdated);
        }

        self.provider_conversation
            .set_prompt_prelude(Some(manager.prelude.clone()));
        Some(PromptAssemblyUpdateNotice::CurrentEmptySessionUpdated)
    }

    pub(super) fn tool_definitions(&self) -> Vec<ToolDefinition> {
        self.workspace_tools
            .definitions()
            .definitions()
            .cloned()
            .collect()
    }

    pub(super) fn reload_prompt_assembly(&mut self) -> Result<RuntimeCommandReceipt, String> {
        let store = self.session_store()?;
        let header = self.session_header()?;
        let tool_defs = self.tool_definitions();
        let manager = load_prompt_assembly_manager_snapshot(store, &header.work_dir, &tool_defs)
            .map_err(|error| error.to_string())?;
        self.options.initial_prompt_prelude = Some(manager.prelude.clone());
        self.pending_runtime_events
            .push(RuntimeEvent::PromptAssemblyUpdated {
                manager,
                notice: None,
            });
        Ok(RuntimeCommandReceipt::Accepted)
    }

    pub(super) fn mutate_prompt_assembly(
        &mut self,
        mutation: runtime_domain::prompt_assembly::PromptAssemblyMutation,
    ) -> Result<RuntimeCommandReceipt, String> {
        let store = self.session_store()?;
        let header = self.session_header()?;
        let tool_defs = self.tool_definitions();
        let manager = apply_prompt_assembly_mutation(store, &header.work_dir, mutation, &tool_defs)
            .map_err(|error| error.to_string())?;
        let prelude_changed =
            self.options.initial_prompt_prelude.as_ref() != Some(&manager.prelude);
        let notice = self.prompt_assembly_update_notice(prelude_changed, &manager);
        self.options.initial_prompt_prelude = Some(manager.prelude.clone());
        self.pending_runtime_events
            .push(RuntimeEvent::PromptAssemblyUpdated { manager, notice });
        Ok(RuntimeCommandReceipt::Accepted)
    }
}
