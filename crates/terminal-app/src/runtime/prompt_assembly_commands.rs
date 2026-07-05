use runtime_domain::session::{
    PromptAssemblyCommandFailureKind, PromptAssemblyUpdateNotice, RuntimeCommandReceipt,
    RuntimeEvent,
};

use super::AppRuntimeCoordinator;
use crate::prompt_assembly::{
    PromptAssemblyWorkspace, dynamic_environment_session_config_from_manager,
};

#[derive(Debug, thiserror::Error)]
enum PromptAssemblyCommandError {
    #[error("{0}")]
    RuntimeState(String),
    #[error("load prompt assembly manager snapshot: {source}")]
    LoadManager { source: color_eyre::Report },
    #[error("apply prompt assembly mutation: {source}")]
    ApplyMutation { source: color_eyre::Report },
}

impl PromptAssemblyCommandError {
    fn failure_kind(&self) -> PromptAssemblyCommandFailureKind {
        match self {
            Self::RuntimeState(_) => PromptAssemblyCommandFailureKind::RuntimeState,
            Self::LoadManager { .. } => PromptAssemblyCommandFailureKind::LoadManager,
            Self::ApplyMutation { .. } => PromptAssemblyCommandFailureKind::ApplyMutation,
        }
    }

    fn display_message(&self) -> String {
        let mut message = self.to_string();
        let mut source = std::error::Error::source(self);
        while let Some(error) = source {
            let source_message = error.to_string();
            if !source_message.is_empty() && !message.contains(&source_message) {
                message.push_str(": ");
                message.push_str(&source_message);
            }
            source = error.source();
        }
        message
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PromptSessionConfigRefreshTarget {
    CurrentEmptySession,
    NextNewSession,
}

impl AppRuntimeCoordinator {
    fn prompt_session_config_refresh_target(&self) -> PromptSessionConfigRefreshTarget {
        if !self.conversation_worker.is_running()
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
                    .set_prompt_prelude(Some(manager.prelude.clone()));
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

    pub(super) fn reload_prompt_assembly(&mut self) -> Result<RuntimeCommandReceipt, String> {
        match self.reload_prompt_assembly_result() {
            Ok(receipt) => Ok(receipt),
            Err(error) => {
                self.pending_runtime_events
                    .push(RuntimeEvent::PromptAssemblyUpdateFailed {
                        kind: error.failure_kind(),
                        message: error.display_message(),
                    });
                Ok(RuntimeCommandReceipt::Accepted)
            }
        }
    }

    fn reload_prompt_assembly_result(
        &mut self,
    ) -> Result<RuntimeCommandReceipt, PromptAssemblyCommandError> {
        let store = self
            .session_store()
            .map_err(PromptAssemblyCommandError::RuntimeState)?;
        let header = self
            .session_header()
            .map_err(PromptAssemblyCommandError::RuntimeState)?;
        let manager =
            PromptAssemblyWorkspace::new(&header.work_dir, self.prompt_assembly_tool_definitions())
                .load_manager(store)
                .map_err(|source| PromptAssemblyCommandError::LoadManager { source })?;
        self.options.initial_prompt_prelude = Some(manager.prelude.clone());
        let dynamic_environment_session_config =
            dynamic_environment_session_config_from_manager(&manager);
        self.options.initial_dynamic_environment_session_config =
            Some(dynamic_environment_session_config.clone());
        if self.prompt_session_config_refresh_target()
            == PromptSessionConfigRefreshTarget::CurrentEmptySession
        {
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
        match self.mutate_prompt_assembly_result(mutation) {
            Ok(receipt) => Ok(receipt),
            Err(error) => {
                self.pending_runtime_events
                    .push(RuntimeEvent::PromptAssemblyUpdateFailed {
                        kind: error.failure_kind(),
                        message: error.display_message(),
                    });
                Ok(RuntimeCommandReceipt::Accepted)
            }
        }
    }

    fn mutate_prompt_assembly_result(
        &mut self,
        mutation: runtime_domain::prompt_assembly::PromptAssemblyMutation,
    ) -> Result<RuntimeCommandReceipt, PromptAssemblyCommandError> {
        let store = self
            .session_store()
            .map_err(PromptAssemblyCommandError::RuntimeState)?;
        let header = self
            .session_header()
            .map_err(PromptAssemblyCommandError::RuntimeState)?;
        let manager =
            PromptAssemblyWorkspace::new(&header.work_dir, self.prompt_assembly_tool_definitions())
                .apply_mutation(store, mutation)
                .map_err(|source| PromptAssemblyCommandError::ApplyMutation { source })?;
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
