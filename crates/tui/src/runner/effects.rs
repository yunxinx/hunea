use color_eyre::eyre::Result;
use mo_core::session::{RuntimeCommand, RuntimeCommandReceipt, RuntimeTarget};

use crate::{AppEffect, Model};

use super::RuntimeCoordinator;
use super::acp_session::{
    AcpSessionUiState, run_interrupt_acp_prompt_effect, run_respond_acp_permission_effect,
    run_send_acp_prompt_effect, run_set_acp_model_effect, run_start_acp_session_effect,
    run_stop_acp_background_terminals_effect,
};
use super::external_io::{run_copy_selection_effect, run_external_editor_effect};
use super::model_refresh::{persist_selected_model, run_refresh_model_provider_effect};
use super::native_agent::run_send_native_agent_effect;
use super::terminal::TuiTerminal;

pub(super) fn apply_effect_if_needed(
    terminal: &mut TuiTerminal,
    model: &mut Model,
    acp_ui_state: &mut AcpSessionUiState,
    runtime_coordinator: &mut impl RuntimeCoordinator,
    effect: Option<AppEffect>,
) -> Result<()> {
    let Some(effect) = effect else {
        return Ok(());
    };

    match effect {
        AppEffect::LaunchExternalEditor(launch) => {
            run_external_editor_effect(terminal, model, launch)
        }
        AppEffect::CopySelection(text) => run_copy_selection_effect(terminal, model, &text),
        AppEffect::ResetRuntimeSession => {
            reset_runtime_session_after_clear(acp_ui_state, runtime_coordinator);
            Ok(())
        }
        AppEffect::StartAcpSession { agent_id } => {
            run_start_acp_session_effect(model, acp_ui_state, runtime_coordinator, &agent_id);
            Ok(())
        }
        AppEffect::SubmitAcpPrompt(submission) => {
            run_send_acp_prompt_effect(model, acp_ui_state, runtime_coordinator, submission);
            Ok(())
        }
        AppEffect::RespondAcpPermission {
            request_id,
            option_id,
            is_rejection,
            rejected_tool_call_id,
        } => {
            run_respond_acp_permission_effect(
                model,
                acp_ui_state,
                runtime_coordinator,
                &request_id,
                option_id,
                is_rejection,
                rejected_tool_call_id,
            );
            Ok(())
        }
        AppEffect::RespondRuntimePermission {
            target,
            request_id,
            option_id,
        } => {
            run_respond_runtime_permission_effect(
                model,
                runtime_coordinator,
                target,
                &request_id,
                option_id,
            );
            Ok(())
        }
        AppEffect::SetAcpModel { config_id, value } => {
            run_set_acp_model_effect(model, runtime_coordinator, config_id, value);
            Ok(())
        }
        AppEffect::StopAcpBackgroundTerminals => {
            run_stop_acp_background_terminals_effect(model, runtime_coordinator);
            Ok(())
        }
        AppEffect::TruncateNativeAgentSession {
            retained_user_turns,
        } => {
            run_truncate_native_agent_session_effect(
                model,
                runtime_coordinator,
                retained_user_turns,
            );
            Ok(())
        }
        AppEffect::PersistSelectedModel { selection } => {
            persist_selected_model(model, runtime_coordinator, &selection);
            Ok(())
        }
        AppEffect::RefreshModelProvider { request } => {
            run_refresh_model_provider_effect(model, runtime_coordinator, request);
            Ok(())
        }
        AppEffect::SendNativeAgent { request } => {
            run_send_native_agent_effect(model, runtime_coordinator, request);
            Ok(())
        }
        AppEffect::InterruptCurrentTurn => {
            run_interrupt_current_turn_effect(model, acp_ui_state, runtime_coordinator);
            Ok(())
        }
    }
}

fn run_truncate_native_agent_session_effect(
    model: &mut Model,
    runtime_coordinator: &mut impl RuntimeCoordinator,
    retained_user_turns: usize,
) {
    if let Err(message) = runtime_coordinator.dispatch_runtime_command(
        RuntimeCommand::truncate_native_agent_session(retained_user_turns),
    ) {
        model.show_transient_status_notice(&message);
    }
}

fn run_respond_runtime_permission_effect(
    model: &mut Model,
    runtime_coordinator: &mut impl RuntimeCoordinator,
    target: RuntimeTarget,
    request_id: &str,
    option_id: Option<String>,
) {
    if let Err(message) =
        runtime_coordinator.dispatch_runtime_command(RuntimeCommand::RespondPermission {
            target: Some(target),
            request_id: request_id.to_string(),
            option_id,
        })
    {
        model.show_transient_status_notice(&message);
    }
}

pub(super) fn reset_runtime_session_after_clear(
    acp_ui_state: &mut AcpSessionUiState,
    runtime_coordinator: &mut impl RuntimeCoordinator,
) {
    acp_ui_state.reset_after_clear();
    let _ = runtime_coordinator.dispatch_runtime_command(RuntimeCommand::Reset);
}

pub(super) fn run_interrupt_current_turn_effect(
    model: &mut Model,
    acp_ui_state: &mut AcpSessionUiState,
    runtime_coordinator: &mut impl RuntimeCoordinator,
) {
    match runtime_coordinator.dispatch_runtime_command(RuntimeCommand::interrupt_current()) {
        Ok(RuntimeCommandReceipt::Interrupted {
            target: Some(RuntimeTarget::NativeAgent(_)),
        }) => {
            model.append_system_message_from_runtime("Chat interrupted");
            model.finish_stream_activity_with_work_summary();
        }
        Ok(RuntimeCommandReceipt::Interrupted { .. }) => {
            run_interrupt_acp_prompt_effect(model, acp_ui_state, runtime_coordinator, false);
        }
        Ok(_) => {}
        Err(message) => model.show_transient_status_notice(&message),
    }
}
