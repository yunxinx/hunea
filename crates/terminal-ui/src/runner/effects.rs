use color_eyre::eyre::Result;
use runtime_domain::session::{RuntimeCommand, RuntimeCommandReceipt, RuntimeTarget};

use crate::{AppEffect, Model};

use super::RuntimeCoordinator;
use super::conversation::run_send_conversation_turn_effect;
use super::external_io::{run_copy_selection_effect, run_external_editor_effect};
use super::model_refresh::{persist_selected_model, run_refresh_model_provider_effect};
use super::terminal::TuiTerminal;

pub(super) fn apply_effect_if_needed(
    terminal: &mut TuiTerminal,
    model: &mut Model,
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
            reset_runtime_session_after_clear(runtime_coordinator);
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
        AppEffect::TruncateConversation {
            retained_user_turns,
        } => {
            run_truncate_conversation_effect(model, runtime_coordinator, retained_user_turns);
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
        AppEffect::SendConversationTurn { request } => {
            run_send_conversation_turn_effect(model, runtime_coordinator, request);
            Ok(())
        }
        AppEffect::InterruptCurrentTurn => {
            run_interrupt_current_turn_effect(model, runtime_coordinator);
            Ok(())
        }
    }
}

fn run_truncate_conversation_effect(
    model: &mut Model,
    runtime_coordinator: &mut impl RuntimeCoordinator,
    retained_user_turns: usize,
) {
    if let Err(message) = runtime_coordinator
        .dispatch_runtime_command(RuntimeCommand::truncate_conversation(retained_user_turns))
    {
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

pub(super) fn reset_runtime_session_after_clear(runtime_coordinator: &mut impl RuntimeCoordinator) {
    let _ = runtime_coordinator.dispatch_runtime_command(RuntimeCommand::Reset);
}

pub(super) fn run_interrupt_current_turn_effect(
    model: &mut Model,
    runtime_coordinator: &mut impl RuntimeCoordinator,
) {
    match runtime_coordinator.dispatch_runtime_command(RuntimeCommand::interrupt_current()) {
        Ok(RuntimeCommandReceipt::Interrupted {
            target: Some(RuntimeTarget::Provider(_)),
        }) => {
            model.finish_stream_activity_with_work_summary();
        }
        Ok(RuntimeCommandReceipt::Interrupted { .. }) => {}
        Ok(_) => {}
        Err(message) => model.show_transient_status_notice(&message),
    }
}
