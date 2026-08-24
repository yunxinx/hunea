use runtime_domain::session::{ConversationTurnRequest, RuntimeCommandReceipt, RuntimeTarget};

#[cfg(test)]
use runtime_domain::session::TranscriptUserMessage;

use super::{
    AppRuntimeCoordinator,
    agent::{AgentCommand, AgentCommandReceipt, AgentId, AgentTurnId, AgentTurnRequest},
};
#[cfg(test)]
use crate::prompt_assembly::AttachedPromptMessageAssembly;

impl AppRuntimeCoordinator {
    pub(super) fn truncate_conversation(
        &mut self,
        retained_user_turns: usize,
    ) -> Result<RuntimeCommandReceipt, String> {
        self.ensure_session_mutation_available("truncate conversation")?;
        if let Some((session_id, leaf_id)) = self
            .components
            .agent_port_mut()
            .truncate_after_user_turns(retained_user_turns)?
        {
            let views = self.session_views()?;
            self.components
                .session_store_worker
                .set_leaf(views, session_id, leaf_id)?;
        }
        Ok(RuntimeCommandReceipt::Accepted)
    }

    pub(super) fn respond_permission(
        &mut self,
        target: Option<&RuntimeTarget>,
        request_id: &str,
        option_id: Option<String>,
    ) -> Result<(), String> {
        self.components
            .agent_port_mut()
            .dispatch(AgentCommand::RespondPermission {
                agent_id: AgentId::MAIN,
                target: target.cloned(),
                request_id: request_id.to_string(),
                option_id,
            })
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    pub(super) fn start_conversation_turn(
        &mut self,
        target: RuntimeTarget,
        request: ConversationTurnRequest,
    ) -> Result<RuntimeCommandReceipt, String> {
        let request = AgentTurnRequest::from_conversation_request(request);
        if target != request.target() {
            return Err(format!(
                "Conversation target does not match request: {}",
                target.display_label()
            ));
        }
        let turn_id = AgentTurnId::new(self.next_agent_turn_id);
        let receipt = self
            .components
            .agent_port_mut()
            .dispatch(AgentCommand::SubmitTurn {
                agent_id: AgentId::MAIN,
                turn_id,
                request: Box::new(request),
            })
            .map_err(|error| error.to_string())?;
        self.next_agent_turn_id = self.next_agent_turn_id.saturating_add(1);
        Ok(runtime_receipt(receipt))
    }

    pub(super) fn interrupt_runtime(
        &mut self,
        target: Option<RuntimeTarget>,
    ) -> Result<RuntimeCommandReceipt, String> {
        self.components
            .agent_port_mut()
            .dispatch(AgentCommand::Interrupt {
                agent_id: AgentId::MAIN,
                target,
            })
            .map(runtime_receipt)
            .map_err(|error| error.to_string())
    }

    #[cfg(test)]
    pub(super) fn dynamic_environment_injection(
        &mut self,
    ) -> Result<super::dynamic_environment_worker::DynamicEnvironmentInjection, String> {
        self.components
            .agent_test_harness()
            .dynamic_environment_injection(self.options.dynamic_environment_observer.clone())
    }

    #[cfg(test)]
    pub(super) fn attached_prompt_message_assembly(
        &self,
        user_message: &TranscriptUserMessage,
    ) -> Result<AttachedPromptMessageAssembly, String> {
        self.components
            .agent_test_harness_ref()
            .attached_prompt_message_assembly(user_message)
    }
}

fn runtime_receipt(receipt: AgentCommandReceipt) -> RuntimeCommandReceipt {
    match receipt {
        AgentCommandReceipt::Accepted => RuntimeCommandReceipt::Accepted,
        AgentCommandReceipt::TurnStarted { activity_label, .. } => {
            RuntimeCommandReceipt::ConversationStarted { activity_label }
        }
        AgentCommandReceipt::Interrupted { target } => {
            RuntimeCommandReceipt::Interrupted { target }
        }
    }
}

#[cfg(test)]
pub(super) fn ensure_conversation_target(
    active_target: Option<&RuntimeTarget>,
    command_target: Option<&RuntimeTarget>,
) -> Result<(), String> {
    match command_target {
        Some(target @ RuntimeTarget::Provider(_)) => match active_target {
            Some(active_target) if active_target == target => Ok(()),
            Some(_) => Err(format!(
                "Conversation is not active: {}",
                target.display_label()
            )),
            None => Err(format!(
                "Conversation is not running: {}",
                target.display_label()
            )),
        },
        None => Ok(()),
    }
}
