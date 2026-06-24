use color_eyre::eyre::Result;
use runtime_domain::session::{RuntimeCommand, RuntimeCommandReceipt, RuntimeTarget};

use crate::{AppEffect, Model, toast::ToastSeverity};

use super::RuntimeCoordinator;
use super::conversation::run_send_conversation_turn_effect;
use super::external_io::{
    ExternalIoRuntime, run_copy_selection_effect, run_external_editor_effect,
};
use super::model_refresh::{persist_selected_model, run_refresh_model_provider_effect};
use super::terminal::TuiTerminal;

pub(crate) fn dispatch_record_message_history(
    model: &mut Model,
    runtime_coordinator: &mut impl RuntimeCoordinator,
    text: String,
) {
    let limit = model.message_history_limit;
    if let Err(message) = runtime_coordinator
        .dispatch_runtime_command(RuntimeCommand::RecordMessageHistory { text, limit })
    {
        let revert_tail = model
            .blind_recall
            .tail_entry_text_for_persist_revert()
            .map(str::to_string);
        if let Some(tail) = revert_tail.as_deref() {
            model.blind_recall.revert_failed_persist(tail);
        }
        model.show_toast(ToastSeverity::Error, message);
    }
}

pub(super) fn apply_effect_if_needed(
    terminal: &mut TuiTerminal,
    model: &mut Model,
    runtime_coordinator: &mut impl RuntimeCoordinator,
    external_io: &mut ExternalIoRuntime,
    effect: Option<AppEffect>,
) -> Result<()> {
    let Some(effect) = effect else {
        return Ok(());
    };

    match effect {
        AppEffect::LaunchExternalEditor(launch) => {
            run_external_editor_effect(terminal, model, launch)
        }
        AppEffect::CopySelection(text) => run_copy_selection_effect(model, external_io, text),
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
        AppEffect::OpenCopyPicker => {
            run_open_copy_picker_effect(model, runtime_coordinator);
            Ok(())
        }
        AppEffect::OpenMessageHistory => {
            run_open_message_history_picker_effect(model, runtime_coordinator);
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
            let request_id = model.open_entry_tree_loading();
            run_simple_runtime_command_effect(
                model,
                runtime_coordinator,
                RuntimeCommand::LoadEntryTree { request_id },
            );
            Ok(())
        }
        AppEffect::OpenBranchTree => {
            run_open_branch_tree_effect(model, runtime_coordinator);
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
        AppEffect::OpenBranchPreview {
            request_id,
            branch_row_id,
        } => {
            run_open_branch_preview_effect(model, runtime_coordinator, request_id, branch_row_id);
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
        AppEffect::RecordMessageHistory { text } => {
            dispatch_record_message_history(model, runtime_coordinator, text);
            Ok(())
        }
        AppEffect::SendConversationTurn {
            request,
            record_message_history,
        } => {
            if let Some(text) = record_message_history {
                dispatch_record_message_history(model, runtime_coordinator, text);
            }
            run_send_conversation_turn_effect(model, runtime_coordinator, *request);
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
    let request_id = model.next_session_load_request_id();
    match runtime_coordinator.dispatch_runtime_command(RuntimeCommand::SwitchBranch {
        request_id,
        leaf_id: leaf_id.to_string(),
    }) {
        Ok(_) => model.open_entry_tree_loading_for_request(request_id),
        Err(message) => model.show_entry_tree_branch_picker_error(&message),
    }
}

pub(super) fn run_open_copy_picker_effect(
    model: &mut Model,
    runtime_coordinator: &mut impl RuntimeCoordinator,
) {
    let request_id = model.open_copy_picker_loading();
    if let Err(message) = runtime_coordinator
        .dispatch_runtime_command(RuntimeCommand::LoadCopyPickerTree { request_id })
    {
        model.show_copy_picker_error(&message);
    }
}

pub(crate) fn run_open_message_history_picker_effect(
    model: &mut Model,
    runtime_coordinator: &mut impl RuntimeCoordinator,
) {
    let request_id = model.open_message_history_picker_loading();
    if let Err(message) = runtime_coordinator
        .dispatch_runtime_command(RuntimeCommand::LoadMessageHistoryPickerRows { request_id })
    {
        model.show_message_history_picker_error(request_id, &message);
    }
}

pub(super) fn run_open_branch_tree_effect(
    model: &mut Model,
    runtime_coordinator: &mut impl RuntimeCoordinator,
) {
    let request_id = model.open_entry_tree_branch_tree_loading();
    if let Err(message) =
        runtime_coordinator.dispatch_runtime_command(RuntimeCommand::LoadBranchTree { request_id })
    {
        model.show_entry_tree_branch_tree_error(&message);
    }
}

pub(super) fn run_open_branch_preview_effect(
    model: &mut Model,
    runtime_coordinator: &mut impl RuntimeCoordinator,
    request_id: runtime_domain::session::SessionLoadRequestId,
    branch_row_id: String,
) {
    if let Err(message) =
        runtime_coordinator.dispatch_runtime_command(RuntimeCommand::LoadBranchPreview {
            request_id,
            branch_row_id,
        })
    {
        model.show_entry_tree_branch_preview_error(&message);
    }
}

fn run_simple_runtime_command_effect(
    model: &mut Model,
    runtime_coordinator: &mut impl RuntimeCoordinator,
    command: RuntimeCommand,
) {
    if let Err(message) = runtime_coordinator.dispatch_runtime_command(command) {
        model.show_toast(ToastSeverity::Error, message);
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
        model.show_toast(ToastSeverity::Error, message);
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
        model.show_toast(ToastSeverity::Error, message);
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
        Err(message) => model.show_toast(ToastSeverity::Error, message),
    }
}
