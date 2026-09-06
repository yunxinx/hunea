use color_eyre::eyre::Result;
use runtime_domain::session::{RuntimeCommand, RuntimeCommandReceipt, RuntimeTarget};

use crate::{AppEffect, Model, toast::ToastSeverity};

use super::conversation::run_send_conversation_turn_effect;
use super::external_io::{
    ExternalIoRuntime, run_copy_selection_effect, run_external_editor_effect,
};
use super::loop_event_pump::LoopEventPump;
use super::model_refresh::{persist_selected_model, run_refresh_model_provider_effect};
use super::runtime_port::{ModelRuntimePort, PromptRuntimePort, RuntimeCommandPort};
use super::terminal::{TerminalSession, TuiTerminal};

pub(crate) fn dispatch_record_message_history(
    model: &mut Model,
    runtime_coordinator: &mut impl RuntimeCommandPort,
    entry_id: runtime_domain::session::MessageHistoryEntryId,
    text: String,
) {
    let limit = model.message_history_limit;
    if let Err(message) =
        runtime_coordinator.dispatch_runtime_command(RuntimeCommand::RecordMessageHistory {
            entry_id,
            text,
            limit,
        })
    {
        model.blind_recall.revert_failed_persist(entry_id);
        model.show_toast(ToastSeverity::Error, message);
    }
}

pub(super) fn apply_effect_if_needed<R>(
    terminal: &mut TuiTerminal,
    terminal_session: &mut TerminalSession,
    model: &mut Model,
    runtime_coordinator: &mut R,
    external_io: &mut ExternalIoRuntime,
    loop_events: &mut LoopEventPump,
    effect: Option<AppEffect>,
) -> Result<()>
where
    R: RuntimeCommandPort + ModelRuntimePort + PromptRuntimePort,
{
    dispatch_context_budget_cancellation_if_needed(model, runtime_coordinator);
    dispatch_prompt_assembly_commit_if_needed(model, runtime_coordinator);
    dispatch_agents_observation_stops_if_needed(model, runtime_coordinator);
    dispatch_pending_agent_view_observes_if_needed(model, runtime_coordinator);

    let Some(effect) = effect else {
        return Ok(());
    };

    match effect {
        AppEffect::LaunchExternalEditor(launch) => {
            let follow_up =
                run_external_editor_effect(terminal, terminal_session, loop_events, model, launch)?;
            apply_effect_if_needed(
                terminal,
                terminal_session,
                model,
                runtime_coordinator,
                external_io,
                loop_events,
                follow_up,
            )
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
        AppEffect::RespondAgentPermission { target, option_id } => {
            run_respond_agent_permission_effect(model, runtime_coordinator, target, option_id);
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
        AppEffect::OpenContextBudget => {
            run_open_context_budget_effect(model, runtime_coordinator);
            Ok(())
        }
        AppEffect::OpenMessageHistory => {
            run_open_message_history_picker_effect(model, runtime_coordinator);
            Ok(())
        }
        AppEffect::OpenAgentsPanel => {
            run_open_agents_panel_effect(model, runtime_coordinator);
            Ok(())
        }
        AppEffect::ObserveAgentTranscript {
            request_id,
            agent_id,
        } => {
            run_observe_agent_transcript_effect(model, runtime_coordinator, request_id, agent_id);
            Ok(())
        }
        AppEffect::StopAgent {
            agent_id,
            generation,
        } => {
            run_stop_agent_effect(model, runtime_coordinator, agent_id, generation);
            Ok(())
        }
        AppEffect::BeginPromptAssemblyEdit => {
            match runtime_coordinator.begin_prompt_assembly_edit() {
                Ok(snapshot) => {
                    model.prompt_assembly = snapshot;
                    model.sync_prompt_overlay_state();
                }
                Err(message) => {
                    model.show_toast(ToastSeverity::Error, message);
                    // begin 失败：从未成功进入 edit session，关 overlay 但不触发 commit。
                    model.dismiss_prompt_overlay();
                }
            }
            Ok(())
        }
        AppEffect::ApplyPromptAssemblyEditMutation { mutation } => {
            match runtime_coordinator.apply_prompt_assembly_edit_mutation(mutation) {
                Ok(snapshot) => {
                    model.prompt_assembly = snapshot;
                    model.sync_prompt_overlay_state();
                }
                Err(message) => model.show_toast(ToastSeverity::Error, message),
            }
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
        AppEffect::RecordMessageHistory { entry_id, text } => {
            dispatch_record_message_history(model, runtime_coordinator, entry_id, text);
            Ok(())
        }
        AppEffect::SendConversationTurn {
            request,
            record_message_history,
        } => {
            if let Some(entry) = record_message_history {
                dispatch_record_message_history(model, runtime_coordinator, entry.id, entry.text);
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

fn dispatch_context_budget_cancellation_if_needed(
    model: &mut Model,
    runtime_coordinator: &mut impl RuntimeCommandPort,
) {
    if !model.take_context_budget_cancellation_request() {
        return;
    }

    if let Err(message) =
        runtime_coordinator.dispatch_runtime_command(RuntimeCommand::CancelContextBudgetSnapshot)
    {
        model.show_toast(ToastSeverity::Error, message);
    }
}

fn dispatch_prompt_assembly_commit_if_needed(
    model: &mut Model,
    runtime_coordinator: &mut impl PromptRuntimePort,
) {
    if !model.take_prompt_assembly_commit_request() {
        return;
    }

    if let Err(message) = runtime_coordinator.commit_prompt_assembly_edit() {
        model.show_toast(ToastSeverity::Error, message);
    }
}

/// 消费 `/agents` panel 关闭路径置位的 observation 注销标志。
///
/// 注销命令幂等（runtime 侧 id/generation mismatch 静默），Err 只意味着 runtime
/// 不可达——此时 observation 已随 runtime 消亡，不再打扰用户。
pub(crate) fn dispatch_agents_observation_stops_if_needed(
    model: &mut Model,
    runtime_coordinator: &mut (impl RuntimeCommandPort + ?Sized),
) {
    let Some(stops) = model.take_pending_agent_observation_stops() else {
        return;
    };

    if let Some((observation_id, generation)) = stops.overview {
        let _ = runtime_coordinator.dispatch_runtime_command(RuntimeCommand::StopObservingAgents {
            observation_id,
            generation,
        });
    }
    for (observation_id, generation) in stops.agent_views {
        let _ = runtime_coordinator.dispatch_runtime_command(
            RuntimeCommand::StopObservingAgentTranscript {
                observation_id,
                generation,
            },
        );
    }
}

pub(super) fn run_switch_branch_effect(
    model: &mut Model,
    runtime_coordinator: &mut impl RuntimeCommandPort,
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
    runtime_coordinator: &mut impl RuntimeCommandPort,
) {
    let request_id = model.open_copy_picker_loading();
    if let Err(message) = runtime_coordinator
        .dispatch_runtime_command(RuntimeCommand::LoadCopyPickerTree { request_id })
    {
        model.show_copy_picker_error(&message);
    }
}

pub(super) fn run_open_context_budget_effect(
    model: &mut Model,
    runtime_coordinator: &mut impl RuntimeCommandPort,
) {
    let Some(selection) = model.selected_model.selection().cloned() else {
        model.show_toast(
            crate::toast::ToastSeverity::Error,
            "Select a model before opening context budget",
        );
        return;
    };
    let request_id = model.open_context_budget_loading();
    if let Err(message) =
        runtime_coordinator.dispatch_runtime_command(RuntimeCommand::LoadContextBudgetSnapshot {
            request_id,
            selection,
        })
    {
        model.show_context_budget_error(
            request_id,
            runtime_domain::session::ContextBudgetLoadErrorPayload::RuntimeInternal {
                detail: Some(message),
            },
        );
    }
}

pub(crate) fn run_open_message_history_picker_effect(
    model: &mut Model,
    runtime_coordinator: &mut impl RuntimeCommandPort,
) {
    let request_id = model.open_message_history_picker_loading();
    if let Err(message) = runtime_coordinator
        .dispatch_runtime_command(RuntimeCommand::LoadMessageHistoryPickerRows { request_id })
    {
        model.show_message_history_picker_error(request_id, &message);
    }
}

/// 消费事件应用点暂存的 per-agent view observe 请求（pill 导航打开 preview）。
///
/// 事件应用发生在 `Model::apply_runtime_event` 内，没有 Effect 通道；
/// 这里按 pending-flag 模式统一补派发，Err 收敛与直接点击路径一致。
pub(crate) fn dispatch_pending_agent_view_observes_if_needed(
    model: &mut Model,
    runtime_coordinator: &mut (impl RuntimeCommandPort + ?Sized),
) {
    let requests = model.take_pending_agent_view_observe_requests();
    for (request_id, agent_id) in requests {
        run_observe_agent_transcript_effect(model, runtime_coordinator, request_id, agent_id);
    }
}

/// `/agents` 打开：Model 先进 loading 态并分配 request_id，再派发 `ObserveAgents`。
pub(crate) fn run_open_agents_panel_effect(
    model: &mut Model,
    runtime_coordinator: &mut impl RuntimeCommandPort,
) {
    let request_id = model.open_agents_panel_loading();
    if let Err(message) =
        runtime_coordinator.dispatch_runtime_command(RuntimeCommand::ObserveAgents { request_id })
    {
        model.show_agents_panel_error(request_id, &message);
    }
}

/// Enter/Space 进入 per-agent surface：派发共享的 `ObserveAgentTranscript`。
pub(crate) fn run_observe_agent_transcript_effect(
    model: &mut Model,
    runtime_coordinator: &mut (impl RuntimeCommandPort + ?Sized),
    request_id: runtime_domain::agent::AgentObservationRequestId,
    agent_id: runtime_domain::agent::AgentId,
) {
    if let Err(message) =
        runtime_coordinator.dispatch_runtime_command(RuntimeCommand::ObserveAgentTranscript {
            request_id,
            agent_id,
        })
    {
        model.show_agents_panel_agent_view_error(request_id, &message);
    }
}

/// `x` 二次确认后的 stop：失败走 toast，panel 仍由 overview delta 反映真实状态。
pub(crate) fn run_stop_agent_effect(
    model: &mut Model,
    runtime_coordinator: &mut impl RuntimeCommandPort,
    agent_id: runtime_domain::agent::AgentId,
    generation: runtime_domain::agent::AgentRuntimeGeneration,
) {
    if let Err(message) = runtime_coordinator.dispatch_runtime_command(RuntimeCommand::StopAgent {
        agent_id,
        generation,
    }) {
        model.show_toast(ToastSeverity::Error, message);
    }
}

pub(super) fn run_open_branch_tree_effect(
    model: &mut Model,
    runtime_coordinator: &mut impl RuntimeCommandPort,
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
    runtime_coordinator: &mut impl RuntimeCommandPort,
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
    runtime_coordinator: &mut impl RuntimeCommandPort,
    command: RuntimeCommand,
) {
    if let Err(message) = runtime_coordinator.dispatch_runtime_command(command) {
        model.show_toast(ToastSeverity::Error, message);
    }
}

fn run_truncate_conversation_effect(
    model: &mut Model,
    runtime_coordinator: &mut impl RuntimeCommandPort,
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
    runtime_coordinator: &mut impl RuntimeCommandPort,
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

/// child Agent permission 提交：target 从 FIFO head 原样携带，由 runtime 校验
/// （generation / request / option 全链 closed 拒绝）。
///
/// Err → Error toast + 主动 reconcile：runtime 拒绝后投影不会变化，preview 的
/// 本地 Submitted 锁定需要在 snapshot 仍 Pending 时解除，允许重试。
pub(crate) fn run_respond_agent_permission_effect(
    model: &mut Model,
    runtime_coordinator: &mut impl RuntimeCommandPort,
    target: runtime_domain::agent::AgentPermissionTarget,
    option_id: String,
) {
    if let Err(message) =
        runtime_coordinator.dispatch_runtime_command(RuntimeCommand::RespondAgentPermission {
            target: target.clone(),
            option_id: Some(option_id),
        })
    {
        model.show_toast(ToastSeverity::Error, message);
        model.sync_agents_panel_preview_permission(target.agent_id);
    }
}

pub(super) fn reset_runtime_session_after_clear(runtime_coordinator: &mut impl RuntimeCommandPort) {
    let _ = runtime_coordinator.dispatch_runtime_command(RuntimeCommand::Reset);
}

pub(super) fn run_interrupt_current_turn_effect(
    model: &mut Model,
    runtime_coordinator: &mut impl RuntimeCommandPort,
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
