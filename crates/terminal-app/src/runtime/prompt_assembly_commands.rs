use runtime_domain::session::{RuntimeCommandReceipt, RuntimeEvent};

use super::AppRuntimeCoordinator;
use crate::prompt_assembly::apply_prompt_assembly_mutation;

impl AppRuntimeCoordinator {
    pub(super) fn mutate_prompt_assembly(
        &mut self,
        mutation: runtime_domain::prompt_assembly::PromptAssemblyMutation,
    ) -> Result<RuntimeCommandReceipt, String> {
        let store = self.session_store()?;
        let header = self.session_header()?;
        let manager = apply_prompt_assembly_mutation(store, &header.work_dir, mutation)
            .map_err(|error| error.to_string())?;
        self.options.initial_prompt_prelude = Some(manager.prelude.clone());
        self.pending_runtime_events
            .push(RuntimeEvent::PromptAssemblyUpdated { manager });
        Ok(RuntimeCommandReceipt::Accepted)
    }
}
