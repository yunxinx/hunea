use color_eyre::eyre::Result;

use crate::{AppEffect, Model};

use super::RuntimeDriver;
use super::acp_session::{
    AcpRuntimeState, run_interrupt_acp_prompt_effect, run_respond_acp_permission_effect,
    run_send_acp_prompt_effect, run_set_acp_model_effect, run_start_acp_session_effect,
    run_stop_acp_background_terminals_effect,
};
use super::external_io::{run_copy_selection_effect, run_external_editor_effect};
use super::model_refresh::{persist_selected_model, run_refresh_model_provider_effect};
use super::native_agent::{run_interrupt_native_agent_effect, run_send_native_agent_effect};
use super::terminal::TuiTerminal;

pub(super) fn apply_effect_if_needed(
    terminal: &mut TuiTerminal,
    model: &mut Model,
    acp_runtime: &mut AcpRuntimeState,
    runtime_driver: &mut impl RuntimeDriver,
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
            reset_runtime_session_after_clear(acp_runtime, runtime_driver);
            Ok(())
        }
        AppEffect::StartAcpSession { agent_id } => {
            run_start_acp_session_effect(model, acp_runtime, runtime_driver, &agent_id);
            Ok(())
        }
        AppEffect::SubmitAcpPrompt(submission) => {
            run_send_acp_prompt_effect(model, acp_runtime, runtime_driver, submission);
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
                acp_runtime,
                runtime_driver,
                &request_id,
                option_id,
                is_rejection,
                rejected_tool_call_id,
            );
            Ok(())
        }
        AppEffect::SetAcpModel { config_id, value } => {
            run_set_acp_model_effect(model, runtime_driver, config_id, value);
            Ok(())
        }
        AppEffect::StopAcpBackgroundTerminals => {
            run_stop_acp_background_terminals_effect(model, runtime_driver);
            Ok(())
        }
        AppEffect::PersistSelectedModel { selection } => {
            persist_selected_model(model, runtime_driver, &selection);
            Ok(())
        }
        AppEffect::RefreshModelProvider { request } => {
            run_refresh_model_provider_effect(model, runtime_driver, request);
            Ok(())
        }
        AppEffect::SendNativeAgent { request } => {
            run_send_native_agent_effect(model, runtime_driver, request);
            Ok(())
        }
        AppEffect::InterruptCurrentTurn => {
            run_interrupt_current_turn_effect(model, acp_runtime, runtime_driver);
            Ok(())
        }
    }
}

pub(super) fn reset_runtime_session_after_clear(
    acp_runtime: &mut AcpRuntimeState,
    runtime_driver: &mut impl RuntimeDriver,
) {
    acp_runtime.reset_after_clear();
    runtime_driver.reset_runtime_session();
}

pub(super) fn run_interrupt_current_turn_effect(
    model: &mut Model,
    acp_runtime: &mut AcpRuntimeState,
    runtime_driver: &mut impl RuntimeDriver,
) {
    if run_interrupt_native_agent_effect(model, runtime_driver) {
        return;
    }

    run_interrupt_acp_prompt_effect(model, acp_runtime, runtime_driver);
}
