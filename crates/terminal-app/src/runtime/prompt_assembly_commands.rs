use runtime_domain::session::{RuntimeCommandReceipt, RuntimeEvent};
use tool_runtime::ToolDefinition;

use super::AppRuntimeCoordinator;
use crate::prompt_assembly::{
    apply_prompt_assembly_mutation, load_prompt_assembly_manager_snapshot,
};

impl AppRuntimeCoordinator {
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
            .push(RuntimeEvent::PromptAssemblyUpdated { manager });
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
        self.options.initial_prompt_prelude = Some(manager.prelude.clone());
        self.pending_runtime_events
            .push(RuntimeEvent::PromptAssemblyUpdated { manager });
        Ok(RuntimeCommandReceipt::Accepted)
    }
}
