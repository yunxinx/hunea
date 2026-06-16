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
        AppEffect::OpenResumePicker => {
            model.open_session_picker_loading();
            run_simple_runtime_command_effect(
                model,
                runtime_coordinator,
                RuntimeCommand::ListSessions,
            );
            Ok(())
        }
        AppEffect::OpenSessionPreview { session_id } => {
            run_simple_runtime_command_effect(
                model,
                runtime_coordinator,
                RuntimeCommand::LoadSessionPreview { session_id },
            );
            Ok(())
        }
        AppEffect::ResumeSession { session_id } => {
            run_simple_runtime_command_effect(
                model,
                runtime_coordinator,
                RuntimeCommand::ResumeSession { session_id },
            );
            Ok(())
        }
        AppEffect::OpenEntryRewind => {
            model.open_entry_tree_loading();
            run_simple_runtime_command_effect(
                model,
                runtime_coordinator,
                RuntimeCommand::LoadEntryTree,
            );
            Ok(())
        }
        AppEffect::OpenBranchTree => {
            model.open_entry_tree_branch_tree_loading();
            run_simple_runtime_command_effect(
                model,
                runtime_coordinator,
                RuntimeCommand::LoadBranchTree,
            );
            Ok(())
        }
        AppEffect::SelectEntryRewind { entry_id, prefill } => {
            if let Some(prefill) = prefill {
                model.composer_mut().reset_text_and_move_to_end(prefill);
            }
            run_simple_runtime_command_effect(
                model,
                runtime_coordinator,
                RuntimeCommand::SelectEntryRewind { entry_id },
            );
            Ok(())
        }
        AppEffect::OpenBranchPreview { branch_row_id } => {
            run_simple_runtime_command_effect(
                model,
                runtime_coordinator,
                RuntimeCommand::LoadBranchPreview { branch_row_id },
            );
            Ok(())
        }
        AppEffect::SwitchBranch { leaf_id } => {
            run_switch_branch_effect(model, runtime_coordinator, &leaf_id);
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

pub(super) fn run_switch_branch_effect(
    model: &mut Model,
    runtime_coordinator: &mut impl RuntimeCoordinator,
    leaf_id: &str,
) {
    match runtime_coordinator.dispatch_runtime_command(RuntimeCommand::SwitchBranch {
        leaf_id: leaf_id.to_string(),
    }) {
        Ok(_) => model.open_entry_tree_loading(),
        Err(message) => model.show_entry_tree_branch_picker_error(&message),
    }
}

fn run_simple_runtime_command_effect(
    model: &mut Model,
    runtime_coordinator: &mut impl RuntimeCoordinator,
    command: RuntimeCommand,
) {
    if let Err(message) = runtime_coordinator.dispatch_runtime_command(command) {
        model.show_transient_status_notice(&message);
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
