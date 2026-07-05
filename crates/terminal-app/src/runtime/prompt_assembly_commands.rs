use runtime_domain::session::{
    PromptAssemblyCommandFailureKind, PromptAssemblyUpdateNotice, RuntimeCommandReceipt,
    RuntimeEvent,
};

use super::AppRuntimeCoordinator;
use crate::prompt_assembly::dynamic_environment_session_config_from_manager;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PromptSessionConfigRefreshTarget {
    CurrentEmptySession,
    NextNewSession,
}

impl AppRuntimeCoordinator {
    fn prompt_session_config_refresh_target(&self) -> PromptSessionConfigRefreshTarget {
        if !self.conversation_worker.is_running()
            && self.pending_conversation_turn.is_none()
            && self.provider_conversation.is_history_empty()
            && self.provider_conversation.session_id().is_none()
        {
            PromptSessionConfigRefreshTarget::CurrentEmptySession
        } else {
            PromptSessionConfigRefreshTarget::NextNewSession
        }
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
        match self.prompt_session_config_refresh_target() {
            PromptSessionConfigRefreshTarget::CurrentEmptySession => {
                self.provider_conversation
                    .set_prompt_prelude(Some(manager.resolution.prelude.clone()));
                self.provider_conversation
                    .set_dynamic_environment_session_config(Some(
                        dynamic_environment_session_config.clone(),
                    ));
                Some(PromptAssemblyUpdateNotice::CurrentEmptySessionUpdated)
            }
            PromptSessionConfigRefreshTarget::NextNewSession => {
                Some(PromptAssemblyUpdateNotice::NextNewSessionUpdated)
            }
        }
    }

    pub(super) fn reload_prompt_assembly(&mut self) -> RuntimeCommandReceipt {
        match self.reload_prompt_assembly_result() {
            Ok(receipt) => receipt,
            Err(message) => {
                self.pending_runtime_events
                    .push(RuntimeEvent::PromptAssemblyUpdateFailed {
                        kind: PromptAssemblyCommandFailureKind::RuntimeState,
                        message,
                    });
                RuntimeCommandReceipt::Accepted
            }
        }
    }

    fn reload_prompt_assembly_result(&mut self) -> Result<RuntimeCommandReceipt, String> {
        let store = self.session_store()?;
        let header = self.session_header()?;
        self.session_store_worker.load_prompt_assembly(
            store,
            header.work_dir,
            self.prompt_assembly_tool_definitions().to_vec(),
        )?;
        Ok(RuntimeCommandReceipt::Accepted)
    }

    pub(super) fn prompt_assembly_reloaded_event(
        &mut self,
        manager: runtime_domain::prompt_assembly::PromptAssemblyManagerSnapshot,
    ) -> RuntimeEvent {
        self.options.prompt_assembly_manager = Some(manager.clone());
        self.options.initial_prompt_prelude = Some(manager.resolution.prelude.clone());
        let dynamic_environment_session_config =
            dynamic_environment_session_config_from_manager(&manager);
        self.options.initial_dynamic_environment_session_config =
            Some(dynamic_environment_session_config.clone());
        if self.prompt_session_config_refresh_target()
            == PromptSessionConfigRefreshTarget::CurrentEmptySession
        {
            self.provider_conversation
                .set_prompt_prelude(Some(manager.resolution.prelude.clone()));
            self.provider_conversation
                .set_dynamic_environment_session_config(Some(dynamic_environment_session_config));
        }
        RuntimeEvent::PromptAssemblyUpdated {
            manager,
            notice: None,
        }
    }

    pub(super) fn mutate_prompt_assembly(
        &mut self,
        mutation: runtime_domain::prompt_assembly::PromptAssemblyMutation,
    ) -> RuntimeCommandReceipt {
        match self.mutate_prompt_assembly_result(mutation) {
            Ok(receipt) => receipt,
            Err(message) => {
                self.pending_runtime_events
                    .push(RuntimeEvent::PromptAssemblyUpdateFailed {
                        kind: PromptAssemblyCommandFailureKind::RuntimeState,
                        message,
                    });
                RuntimeCommandReceipt::Accepted
            }
        }
    }

    fn mutate_prompt_assembly_result(
        &mut self,
        mutation: runtime_domain::prompt_assembly::PromptAssemblyMutation,
    ) -> Result<RuntimeCommandReceipt, String> {
        let store = self.session_store()?;
        let header = self.session_header()?;
        self.ensure_session_mutation_available("mutate prompt assembly")?;
        self.session_store_worker.apply_prompt_assembly_mutation(
            store,
            header.work_dir,
            mutation,
            self.prompt_assembly_tool_definitions().to_vec(),
        )?;
        Ok(RuntimeCommandReceipt::Accepted)
    }

    pub(super) fn prompt_assembly_mutated_event(
        &mut self,
        manager: runtime_domain::prompt_assembly::PromptAssemblyManagerSnapshot,
    ) -> RuntimeEvent {
        let dynamic_environment_session_config =
            dynamic_environment_session_config_from_manager(&manager);
        let prelude_changed =
            self.options.initial_prompt_prelude.as_ref() != Some(&manager.resolution.prelude);
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
        self.options.prompt_assembly_manager = Some(manager.clone());
        self.options.initial_prompt_prelude = Some(manager.resolution.prelude.clone());
        self.options.initial_dynamic_environment_session_config =
            Some(dynamic_environment_session_config);
        RuntimeEvent::PromptAssemblyUpdated { manager, notice }
    }
}
