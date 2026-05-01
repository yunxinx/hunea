use color_eyre::eyre::Result;

use crate::frontend::tui::{AppEffect, Model};
use crate::runtime::native::{ModelProviderRefreshRuntimeState, NativeAgentRuntimeState};

use super::RuntimeOptions;
use super::acp_session::{
    AcpRuntimeState, run_interrupt_acp_prompt_effect, run_respond_acp_permission_effect,
    run_send_acp_prompt_effect, run_set_acp_model_effect, run_start_acp_session_effect,
};
use super::external_io::{run_copy_selection_effect, run_external_editor_effect};
use super::model_refresh::{persist_selected_model, run_refresh_model_provider_effect};
use super::native_agent::{run_interrupt_native_agent_effect, run_send_native_agent_effect};
use super::terminal::TuiTerminal;

pub(super) fn apply_effect_if_needed(
    terminal: &mut TuiTerminal,
    model: &mut Model,
    runtime_options: &RuntimeOptions,
    acp_runtime: &mut AcpRuntimeState,
    native_agent_runtime: &mut NativeAgentRuntimeState,
    model_refresh_runtime: &mut ModelProviderRefreshRuntimeState,
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
            reset_runtime_session_after_clear(
                acp_runtime,
                native_agent_runtime,
                model_refresh_runtime,
            );
            Ok(())
        }
        AppEffect::StartAcpSession { agent_id } => {
            run_start_acp_session_effect(model, runtime_options, acp_runtime, &agent_id)
        }
        AppEffect::SendAcpPrompt { agent_id, prompt } => {
            run_send_acp_prompt_effect(model, acp_runtime, &agent_id, prompt);
            Ok(())
        }
        AppEffect::RespondAcpPermission {
            request_id,
            option_id,
        } => {
            run_respond_acp_permission_effect(model, acp_runtime, &request_id, option_id);
            Ok(())
        }
        AppEffect::SetAcpModel { config_id, value } => {
            run_set_acp_model_effect(model, acp_runtime, config_id, value);
            Ok(())
        }
        AppEffect::PersistSelectedModel { selection } => {
            persist_selected_model(
                model,
                runtime_options.model_config_path.as_deref(),
                &selection,
            );
            Ok(())
        }
        AppEffect::RefreshModelProvider { request } => {
            run_refresh_model_provider_effect(model, model_refresh_runtime, request);
            Ok(())
        }
        AppEffect::SendNativeAgent { request } => {
            run_send_native_agent_effect(
                model,
                native_agent_runtime,
                request,
                runtime_options.runtime_request_policy.clone(),
            );
            Ok(())
        }
        AppEffect::InterruptCurrentTurn => {
            run_interrupt_current_turn_effect(model, acp_runtime, native_agent_runtime);
            Ok(())
        }
    }
}

pub(super) fn reset_runtime_session_after_clear(
    acp_runtime: &mut AcpRuntimeState,
    native_agent_runtime: &mut NativeAgentRuntimeState,
    model_refresh_runtime: &mut ModelProviderRefreshRuntimeState,
) {
    acp_runtime.reset_after_clear();
    native_agent_runtime.reset_after_clear();
    model_refresh_runtime.reset_after_clear();
}

pub(super) fn run_interrupt_current_turn_effect(
    model: &mut Model,
    acp_runtime: &mut AcpRuntimeState,
    native_agent_runtime: &mut NativeAgentRuntimeState,
) {
    if run_interrupt_native_agent_effect(model, native_agent_runtime) {
        return;
    }

    run_interrupt_acp_prompt_effect(model, acp_runtime);
}
