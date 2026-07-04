use runtime_domain::session::{PromptAssemblyUpdateNotice, RuntimeCommandReceipt, RuntimeEvent};
use tool_runtime::ToolDefinition;

use super::AppRuntimeCoordinator;
use crate::prompt_assembly::{
    apply_prompt_assembly_mutation, dynamic_environment_session_config_from_manager,
    load_prompt_assembly_manager_snapshot,
};

impl AppRuntimeCoordinator {
    fn current_session_accepts_prompt_session_config_refresh(&self) -> bool {
        !self.conversation_worker.is_running()
            && self.provider_conversation.is_history_empty()
            && self.provider_conversation.session_id().is_none()
    }

    fn prompt_assembly_update_notice(
        &mut self,
        session_prompt_config_changed: bool,
        manager: &runtime_domain::prompt_assembly::PromptAssemblyManagerSnapshot,
        dynamic_environment_session_config: &runtime_domain::dynamic_environment::DynamicEnvironmentSessionConfig,
    ) -> Option<PromptAssemblyUpdateNotice> {
        if !session_prompt_config_changed {
            return None;
        }
        if !self.current_session_accepts_prompt_session_config_refresh() {
            return Some(PromptAssemblyUpdateNotice::NextNewSessionUpdated);
        }

        self.provider_conversation
            .set_prompt_prelude(Some(manager.prelude.clone()));
        self.provider_conversation
            .set_dynamic_environment_session_config(Some(
                dynamic_environment_session_config.clone(),
            ));
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
        let dynamic_environment_session_config =
            dynamic_environment_session_config_from_manager(&manager);
        self.options.initial_dynamic_environment_session_config =
            Some(dynamic_environment_session_config.clone());
        if self.current_session_accepts_prompt_session_config_refresh() {
            self.provider_conversation
                .set_prompt_prelude(Some(manager.prelude.clone()));
            self.provider_conversation
                .set_dynamic_environment_session_config(Some(dynamic_environment_session_config));
        }
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
        let dynamic_environment_session_config =
            dynamic_environment_session_config_from_manager(&manager);
        let prelude_changed =
            self.options.initial_prompt_prelude.as_ref() != Some(&manager.prelude);
        let dynamic_environment_config_changed = self
            .options
            .initial_dynamic_environment_session_config
            .as_ref()
            != Some(&dynamic_environment_session_config);
        let notice = self.prompt_assembly_update_notice(
            prelude_changed || dynamic_environment_config_changed,
            &manager,
            &dynamic_environment_session_config,
        );
        self.options.initial_prompt_prelude = Some(manager.prelude.clone());
        self.options.initial_dynamic_environment_session_config =
            Some(dynamic_environment_session_config);
        self.pending_runtime_events
            .push(RuntimeEvent::PromptAssemblyUpdated { manager, notice });
        Ok(RuntimeCommandReceipt::Accepted)
    }
}
