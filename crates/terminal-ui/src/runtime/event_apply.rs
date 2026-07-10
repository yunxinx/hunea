use runtime_domain::prompt_assembly::{
    PromptAssemblyDiscoveredSkill, PromptAssemblyExtraPromptCandidate,
};
use runtime_domain::session::{
    RuntimeEvent, RuntimePermissionOptionKind, RuntimePermissionRequest, RuntimeTarget,
    RuntimeTerminalSnapshot, RuntimeToolActivity, RuntimeToolActivityStatus,
    RuntimeToolActivityUpdate, RuntimeToolKind, SessionResumePayload,
    TranscriptCustomPromptBinding, TranscriptReplayItem, TranscriptReplayRole,
    TranscriptSkillBinding, TranscriptUserMessage,
};

use super::super::{
    Model,
    model::RequestMetrics,
    runtime::tool_activity_preview::ToolApprovalPreview,
    session_tree_preview_replay::SessionTreePreviewReplay,
    toast::ToastSeverity,
    tool_approval_panel::{ToolApprovalDetail, ToolApprovalSource},
    tool_result::ToolActivityRenderMode,
};
use serde_json::Value;

const FALLBACK_CHAT_FAILURE_MESSAGE: &str = "Unknown error";

pub(crate) trait RuntimeEventApply {
    fn apply_runtime_event(&mut self, event: RuntimeEvent);
}

impl RuntimeEventApply for Model {
    fn apply_runtime_event(&mut self, event: RuntimeEvent) {
        if !matches!(&event, RuntimeEvent::Retrying { .. }) {
            self.clear_stream_activity_retry_header();
        }

        match event {
            RuntimeEvent::Started { identity, .. } => {
                self.show_toast(
                    ToastSeverity::Info,
                    format!("Runtime ready: {}", identity.label),
                );
            }
            RuntimeEvent::StartFailed { message, .. } => {
                self.show_toast(
                    ToastSeverity::Error,
                    format!("Runtime start failed: {message}"),
                );
            }
            RuntimeEvent::SystemMessage { message, .. } => {
                self.flush_runtime_response_buffer();
                self.append_system_message_from_runtime(message);
            }
            RuntimeEvent::TurnStarted { label, .. } => {
                self.reset_runtime_final_body_divider_state();
                self.clear_runtime_response_buffer();
                if self.stream_activity.is_none() {
                    self.show_stream_activity(label);
                }
            }
            RuntimeEvent::AssistantDelta { content, .. } => {
                self.push_runtime_assistant_delta(&content);
                self.set_stream_activity_thinking(false);
            }
            RuntimeEvent::ReasoningDelta { content, .. } => {
                self.push_runtime_reasoning_delta(&content);
                self.set_stream_activity_thinking(true);
            }
            RuntimeEvent::OutputTokenEstimate {
                target,
                total_tokens,
            } => {
                self.ensure_stream_activity_for_runtime_token_progress(target.as_ref());
                self.set_stream_activity_output_tokens(total_tokens);
            }
            RuntimeEvent::InputTokenEstimate {
                target,
                total_tokens,
            } => {
                self.ensure_stream_activity_for_runtime_token_progress(target.as_ref());
                self.set_stream_activity_input_tokens(total_tokens);
            }
            RuntimeEvent::Thinking { is_thinking, .. } => {
                self.set_stream_activity_thinking(is_thinking);
            }
            RuntimeEvent::Retrying { message, .. } => {
                self.close_runtime_permission_approval_panel();
                self.clear_runtime_response_buffer();
                self.show_stream_activity_retry_header(message);
            }
            RuntimeEvent::ToolActivityStarted { activity, .. } => {
                self.flush_runtime_response_buffer();
                self.append_runtime_tool_activity_from_runtime(activity);
                self.record_runtime_tool_activity_started_for_final_body_divider();
                self.set_stream_activity_thinking(false);
            }
            RuntimeEvent::ToolActivityUpdated { update, .. } => {
                self.flush_runtime_response_buffer();
                upsert_runtime_tool_activity(self, update);
                self.set_stream_activity_thinking(false);
            }
            RuntimeEvent::TerminalUpdated { snapshot, .. } => {
                let _ = self.apply_runtime_terminal_snapshot_from_runtime(snapshot);
            }
            RuntimeEvent::PermissionRequested { target, request } => {
                self.flush_runtime_response_buffer();
                show_runtime_permission_request(self, target, request);
            }
            RuntimeEvent::PermissionCancelled { .. } => {
                self.close_tool_approval_panel();
                self.show_toast(ToastSeverity::Info, "Runtime permission request cancelled");
            }
            RuntimeEvent::SessionListLoaded { rows } => {
                self.apply_session_picker_rows(rows);
            }
            RuntimeEvent::SessionPreviewLoaded { payload } => {
                self.apply_session_preview_payload(payload);
            }
            RuntimeEvent::SessionTreeLoaded {
                request_id,
                payload,
            } => {
                if self.entry_tree_load_request_matches(request_id) {
                    self.apply_entry_tree_payload(payload);
                }
            }
            RuntimeEvent::SessionTreeLoadFailed {
                request_id,
                message,
            } => {
                if self.entry_tree_load_request_matches(request_id) {
                    self.show_entry_tree_error(&message);
                }
            }
            RuntimeEvent::CopyPickerTreeLoaded {
                request_id,
                payload,
            } => {
                if self.copy_picker_load_request_matches(request_id) {
                    self.apply_copy_picker_payload(payload);
                }
            }
            RuntimeEvent::CopyPickerTreeLoadFailed {
                request_id,
                message,
            } => {
                if self.copy_picker_load_request_matches(request_id) {
                    self.show_copy_picker_error(&message);
                }
            }
            RuntimeEvent::ContextBudgetSnapshotLoaded {
                request_id,
                payload,
            } => {
                if self.context_budget_load_request_matches(request_id) {
                    self.apply_context_budget_snapshot(request_id, payload);
                }
            }
            RuntimeEvent::ContextBudgetSnapshotLoadFailed { request_id, error } => {
                if self.context_budget_load_request_matches(request_id) {
                    self.show_context_budget_error(request_id, error);
                }
            }
            RuntimeEvent::SessionBranchTreeLoaded {
                request_id,
                payload,
            } => {
                if self.entry_tree_branch_tree_load_request_matches(request_id) {
                    self.apply_entry_tree_branch_tree_payload(payload);
                }
            }
            RuntimeEvent::SessionBranchTreeLoadFailed {
                request_id,
                message,
            } => {
                if self.entry_tree_branch_tree_load_request_matches(request_id) {
                    self.show_entry_tree_branch_tree_error(&message);
                }
            }
            RuntimeEvent::SessionBranchPreviewLoaded {
                request_id,
                payload,
            } => {
                if self.entry_tree_branch_preview_load_request_matches(request_id) {
                    self.apply_entry_tree_branch_preview_payload(payload);
                }
            }
            RuntimeEvent::SessionBranchPreviewLoadFailed {
                request_id,
                message,
            } => {
                if self.entry_tree_branch_preview_load_request_matches(request_id) {
                    self.show_entry_tree_branch_preview_error(&message);
                }
            }
            RuntimeEvent::SessionBranchSwitchFailed {
                request_id,
                message,
            } => {
                if self.entry_tree_load_request_matches(request_id) {
                    self.show_entry_tree_error(&message);
                }
            }
            RuntimeEvent::MessageHistoryStartupCacheLoaded { entries } => {
                self.blind_recall.apply_startup_cache(entries);
            }
            RuntimeEvent::MessageHistoryStartupCacheLoadFailed { message } => {
                self.show_toast(ToastSeverity::Error, message);
            }
            RuntimeEvent::PromptAssemblyMissingSourcesChecked { missing_count } => {
                if missing_count > 0 {
                    let label = if missing_count == 1 {
                        "1 prompt source is missing; open /prompt to repair it".to_string()
                    } else {
                        format!(
                            "{missing_count} prompt sources are missing; open /prompt to repair them"
                        )
                    };
                    self.show_toast(ToastSeverity::Error, label);
                }
            }
            RuntimeEvent::MessageHistoryPickerRowsLoaded { request_id, rows } => {
                if self.message_history_picker_load_request_matches(request_id) {
                    self.apply_message_history_picker_rows(request_id, rows);
                }
            }
            RuntimeEvent::MessageHistoryPickerRowsLoadFailed {
                request_id,
                message,
            } => {
                if self.message_history_picker_load_request_matches(request_id) {
                    self.show_message_history_picker_error(request_id, &message);
                }
            }
            RuntimeEvent::MessageHistoryRecorded { entry_id } => {
                self.blind_recall.confirm_persisted(entry_id);
            }
            RuntimeEvent::MessageHistoryRecordFailed { entry_id, message } => {
                self.blind_recall.revert_failed_persist(entry_id);
                self.show_toast(
                    ToastSeverity::Error,
                    format!("Message history not saved: {message}"),
                );
            }
            RuntimeEvent::PromptAssemblyUpdated { manager, notice } => {
                self.prompt_assembly = manager;
                self.sync_prompt_overlay_state();
                if let Some(notice) = notice {
                    self.pending_prompt_assembly_notice = Some(notice);
                    self.present_pending_prompt_assembly_notice_if_ready();
                }
            }
            RuntimeEvent::PromptAssemblyUpdateFailed { message, .. } => {
                self.show_toast(
                    ToastSeverity::Error,
                    format!("Prompt assembly not updated: {message}"),
                );
            }
            RuntimeEvent::SessionResumed { payload } => {
                self.apply_session_resume_payload(payload);
            }
            RuntimeEvent::MessageFinished {
                response, metrics, ..
            } => {
                self.close_runtime_permission_approval_panel();
                if let Some(metrics) = metrics {
                    self.set_last_request_metrics(Some(RequestMetrics::new(
                        metrics.latency,
                        metrics.output_tokens,
                        metrics.duration,
                    )));
                }
                self.set_stream_activity_thinking(false);
                self.flush_runtime_response_buffer_with_final(
                    response.text_content(),
                    response.reasoning_content(),
                    response.reasoning_duration,
                );
                self.finish_stream_activity_with_work_summary();
                self.reset_runtime_final_body_divider_state();
            }
            RuntimeEvent::Failed { message, .. } => {
                self.close_runtime_permission_approval_panel();
                self.flush_runtime_response_buffer();
                self.accept_streamed_runtime_reasoning_from_runtime();
                self.append_system_message_from_runtime(normalize_chat_failure_message(&message));
                self.finish_stream_activity_with_work_summary();
                self.reset_runtime_final_body_divider_state();
            }
            RuntimeEvent::Interrupted { .. } => {
                self.close_runtime_permission_approval_panel();
                self.flush_runtime_response_buffer();
                self.accept_streamed_runtime_reasoning_from_runtime();
                self.append_system_message_from_runtime("Chat interrupted");
                self.clear_stream_activity();
                self.reset_runtime_final_body_divider_state();
            }
            RuntimeEvent::Stopped { message, .. } => {
                self.close_runtime_permission_approval_panel();
                self.flush_runtime_response_buffer();
                self.accept_streamed_runtime_reasoning_from_runtime();
                self.finish_stream_activity_with_work_summary();
                self.reset_runtime_final_body_divider_state();
                if let Some(message) = message {
                    self.show_toast(ToastSeverity::Error, format!("Runtime stopped: {message}"));
                }
            }
        }
    }
}

fn normalize_chat_failure_message(message: &str) -> String {
    let normalized_message = message.trim();
    if normalized_message.is_empty() {
        return FALLBACK_CHAT_FAILURE_MESSAGE.to_string();
    }

    let (description, json_body) = split_error_description_and_json_body(normalized_message);
    let description = normalize_error_description(&description);

    match (description.is_empty(), json_body) {
        (true, Some(body)) => format!("{FALLBACK_CHAT_FAILURE_MESSAGE}\nBody: {body}"),
        (true, None) => FALLBACK_CHAT_FAILURE_MESSAGE.to_string(),
        (false, Some(body)) => format!("{description}\nBody: {body}"),
        (false, None) => description,
    }
}

impl Model {
    fn ensure_stream_activity_for_runtime_token_progress(
        &mut self,
        target: Option<&RuntimeTarget>,
    ) {
        if self.stream_activity.is_some() {
            return;
        }

        self.show_stream_activity(
            target
                .map(RuntimeTarget::display_label)
                .unwrap_or("Working"),
        );
    }

    fn apply_session_resume_payload(&mut self, payload: SessionResumePayload) {
        self.close_runtime_permission_approval_panel();
        self.clear_runtime_response_buffer();
        self.accept_streamed_runtime_reasoning_from_runtime();
        self.clear_stream_activity();
        self.reset_runtime_final_body_divider_state();

        let restored_model = payload.restored_model.clone();
        self.rebuild_transcript_from_replay(payload.transcript);
        self.apply_resumed_model(restored_model);
        self.show_toast(
            ToastSeverity::Info,
            format!("Resumed session {}", payload.session_id),
        );
    }

    fn rebuild_transcript_from_replay(&mut self, items: Vec<TranscriptReplayItem>) {
        let preserved_viewport_state = self.preserved_viewport_state_for_transcript_refresh();
        let (transcript, terminal_snapshots) =
            self.transcript_from_replay_items_with_terminal_snapshots(items, None);
        self.transcript = transcript;
        self.runtime_terminal_snapshots = terminal_snapshots;
        self.refresh_status_line_after_transcript_change();
        self.sync_transcript_render();
        self.document_runtime.follow_bottom = true;
        self.sync_document_viewport_after_transcript_refresh(preserved_viewport_state);
    }

    pub(crate) fn transcript_from_replay_items(
        &self,
        items: impl IntoIterator<Item = TranscriptReplayItem>,
    ) -> crate::transcript::Transcript {
        self.transcript_from_replay_items_with_terminal_snapshots(items, None)
            .0
    }

    pub(crate) fn transcript_from_replay_items_with_tool_activity_render_mode(
        &self,
        items: impl IntoIterator<Item = TranscriptReplayItem>,
        tool_activity_render_mode: ToolActivityRenderMode,
    ) -> crate::transcript::Transcript {
        self.transcript_from_replay_items_with_terminal_snapshots(
            items,
            Some(tool_activity_render_mode),
        )
        .0
    }

    pub(crate) fn transcript_from_session_tree_preview_replay_with_tool_activity_render_mode(
        &self,
        replay: SessionTreePreviewReplay<'_>,
        tool_activity_render_mode: ToolActivityRenderMode,
    ) -> crate::transcript::Transcript {
        match replay {
            SessionTreePreviewReplay::Borrowed(items) => self
                .transcript_from_replay_item_refs_with_tool_activity_render_mode(
                    items,
                    tool_activity_render_mode,
                ),
            SessionTreePreviewReplay::Fallback(item) => self
                .transcript_from_replay_items_with_tool_activity_render_mode(
                    std::iter::once(item),
                    tool_activity_render_mode,
                ),
        }
    }

    pub(crate) fn message_preview_transcript_from_session_tree_preview_replay(
        &self,
        replay: SessionTreePreviewReplay<'_>,
    ) -> crate::transcript::Transcript {
        self.transcript_from_session_tree_preview_replay_with_tool_activity_render_mode(
            replay,
            ToolActivityRenderMode::DebugDetailed,
        )
    }

    fn transcript_from_replay_item_refs_with_tool_activity_render_mode(
        &self,
        items: &[TranscriptReplayItem],
        tool_activity_render_mode: ToolActivityRenderMode,
    ) -> crate::transcript::Transcript {
        self.transcript_from_replay_items_with_terminal_snapshots(
            items.iter(),
            Some(tool_activity_render_mode),
        )
        .0
    }

    fn transcript_from_replay_items_with_terminal_snapshots<I, T>(
        &self,
        items: I,
        tool_activity_render_mode: Option<ToolActivityRenderMode>,
    ) -> (crate::transcript::Transcript, Vec<RuntimeTerminalSnapshot>)
    where
        I: IntoIterator<Item = T>,
        T: TranscriptReplayItemSource,
    {
        let mut transcript =
            crate::transcript::Transcript::new(self.palette, self.working_dir.clone());
        transcript.set_gap(1);
        if self.has_window {
            transcript.set_width(self.width);
        }
        if let Some(tool_activity_render_mode) = tool_activity_render_mode {
            transcript.set_tool_activity_render_mode(tool_activity_render_mode);
        }
        let mut terminal_snapshots = Vec::new();
        for item in items {
            if let Some(snapshot) = item.terminal_snapshot() {
                upsert_replay_terminal_snapshot(&mut terminal_snapshots, snapshot.clone());
            }
            item.append_to_transcript(
                &mut transcript,
                self.style_mode,
                self.reasoning_display_mode,
                &self.prompt_assembly.candidates.manual_skills,
                &self.prompt_assembly.candidates.extra_prompts,
            );
        }
        for snapshot in terminal_snapshots.iter().cloned() {
            let _ = transcript.set_runtime_terminal_snapshot(snapshot);
        }
        (transcript, terminal_snapshots)
    }

    fn apply_resumed_model(
        &mut self,
        restored_model: Option<runtime_domain::model_catalog::ModelSelection>,
    ) {
        let Some(selection) = restored_model else {
            return;
        };

        self.selected_model = Some(selection);
        self.requires_model_selection = true;
        self.bump_status_line_revision();
    }
}

trait TranscriptReplayItemSource {
    fn terminal_snapshot(&self) -> Option<&RuntimeTerminalSnapshot>;

    fn append_to_transcript(
        self,
        transcript: &mut crate::transcript::Transcript,
        style_mode: crate::style_mode::StyleMode,
        reasoning_display_mode: crate::ReasoningDisplayMode,
        manual_skills: &[PromptAssemblyDiscoveredSkill],
        extra_prompt_candidates: &[PromptAssemblyExtraPromptCandidate],
    );
}

impl TranscriptReplayItemSource for TranscriptReplayItem {
    fn terminal_snapshot(&self) -> Option<&RuntimeTerminalSnapshot> {
        transcript_replay_item_terminal_snapshot(self)
    }

    fn append_to_transcript(
        self,
        transcript: &mut crate::transcript::Transcript,
        style_mode: crate::style_mode::StyleMode,
        reasoning_display_mode: crate::ReasoningDisplayMode,
        manual_skills: &[PromptAssemblyDiscoveredSkill],
        extra_prompt_candidates: &[PromptAssemblyExtraPromptCandidate],
    ) {
        append_transcript_replay_item(
            transcript,
            self,
            style_mode,
            reasoning_display_mode,
            manual_skills,
            extra_prompt_candidates,
        );
    }
}

impl TranscriptReplayItemSource for &TranscriptReplayItem {
    fn terminal_snapshot(&self) -> Option<&RuntimeTerminalSnapshot> {
        transcript_replay_item_terminal_snapshot(self)
    }

    fn append_to_transcript(
        self,
        transcript: &mut crate::transcript::Transcript,
        style_mode: crate::style_mode::StyleMode,
        reasoning_display_mode: crate::ReasoningDisplayMode,
        manual_skills: &[PromptAssemblyDiscoveredSkill],
        extra_prompt_candidates: &[PromptAssemblyExtraPromptCandidate],
    ) {
        append_transcript_replay_item(
            transcript,
            (*self).clone(),
            style_mode,
            reasoning_display_mode,
            manual_skills,
            extra_prompt_candidates,
        );
    }
}

fn transcript_replay_item_terminal_snapshot(
    item: &TranscriptReplayItem,
) -> Option<&RuntimeTerminalSnapshot> {
    match item {
        TranscriptReplayItem::TerminalSnapshot { snapshot } => Some(snapshot),
        TranscriptReplayItem::Message { .. }
        | TranscriptReplayItem::BoundUserMessage { .. }
        | TranscriptReplayItem::Reasoning { .. }
        | TranscriptReplayItem::ToolActivity { .. }
        | TranscriptReplayItem::ToolResult { .. }
        | TranscriptReplayItem::System { .. } => None,
    }
}

fn append_transcript_replay_item(
    transcript: &mut crate::transcript::Transcript,
    item: TranscriptReplayItem,
    style_mode: crate::style_mode::StyleMode,
    reasoning_display_mode: crate::ReasoningDisplayMode,
    manual_skills: &[PromptAssemblyDiscoveredSkill],
    extra_prompt_candidates: &[PromptAssemblyExtraPromptCandidate],
) {
    match item {
        TranscriptReplayItem::Message {
            role: TranscriptReplayRole::User,
            content,
        } => {
            transcript.append_message_with_style_mode(crate::Sender::User, content, style_mode);
        }
        TranscriptReplayItem::Message {
            role: TranscriptReplayRole::Assistant,
            content,
        } => {
            transcript.append_message_with_style_mode(
                crate::Sender::Assistant,
                content,
                style_mode,
            );
        }
        TranscriptReplayItem::BoundUserMessage { message } => {
            let source_message = validated_transcript_user_source_message(
                message,
                manual_skills,
                extra_prompt_candidates,
            );
            transcript.append_message_with_style_mode_and_source(
                crate::Sender::User,
                source_message
                    .as_transcript_user_message()
                    .display_content(),
                style_mode,
                Some(source_message),
            );
        }
        TranscriptReplayItem::Reasoning { content } => {
            transcript.append_reasoning_message(content, reasoning_display_mode, None);
        }
        TranscriptReplayItem::ToolActivity { activity } => {
            transcript.append_runtime_tool_activity(activity);
        }
        TranscriptReplayItem::TerminalSnapshot { snapshot } => {
            let _ = transcript.set_runtime_terminal_snapshot(snapshot);
        }
        TranscriptReplayItem::ToolResult { content } => {
            transcript.append_tool_result(content, crate::tool_result::ToolResultKind::Ran);
        }
        TranscriptReplayItem::System { content } => {
            transcript.append_system_message(content);
        }
    }
}

fn upsert_replay_terminal_snapshot(
    snapshots: &mut Vec<RuntimeTerminalSnapshot>,
    snapshot: RuntimeTerminalSnapshot,
) {
    if let Some(existing) = snapshots
        .iter_mut()
        .find(|existing| existing.terminal_id == snapshot.terminal_id)
    {
        *existing = snapshot;
        return;
    }

    snapshots.push(snapshot);
}

fn validated_transcript_user_source_message(
    message: TranscriptUserMessage,
    manual_skills: &[PromptAssemblyDiscoveredSkill],
    extra_prompt_candidates: &[PromptAssemblyExtraPromptCandidate],
) -> crate::composer::ComposerSourceMessage {
    let valid_skill_bindings = message
        .skill_bindings
        .into_iter()
        .filter(|binding| {
            is_visible_binding_range_valid(binding, &message.content)
                && manual_skills.iter().any(|skill| {
                    skill.skill_name == binding.skill_name
                        && skill.origin == binding.origin
                        && skill.skill_path == std::path::Path::new(binding.skill_path.as_str())
                })
        })
        .collect();
    let valid_custom_prompt_bindings = message
        .custom_prompt_bindings
        .into_iter()
        .filter(|binding| {
            is_visible_custom_prompt_binding_range_valid(binding, &message.content)
                && extra_prompt_candidates.iter().any(|prompt| {
                    prompt.reference_id == binding.reference_id && prompt.origin == binding.origin
                })
        })
        .collect();
    crate::composer::ComposerSourceMessage::from_transcript_user_message(TranscriptUserMessage {
        content: message.content,
        attachments: message.attachments,
        skill_bindings: valid_skill_bindings,
        custom_prompt_bindings: valid_custom_prompt_bindings,
    })
}

fn is_visible_binding_range_valid(binding: &TranscriptSkillBinding, content: &str) -> bool {
    let total_chars = content.chars().count();
    if binding.start_char >= binding.end_char || binding.end_char > total_chars {
        return false;
    }

    let visible_text = content
        .chars()
        .skip(binding.start_char)
        .take(binding.end_char - binding.start_char)
        .collect::<String>();
    visible_text == binding.visible_token_text()
}

fn is_visible_custom_prompt_binding_range_valid(
    binding: &TranscriptCustomPromptBinding,
    content: &str,
) -> bool {
    let total_chars = content.chars().count();
    if binding.start_char >= binding.end_char || binding.end_char > total_chars {
        return false;
    }

    let visible_text = content
        .chars()
        .skip(binding.start_char)
        .take(binding.end_char - binding.start_char)
        .collect::<String>();
    visible_text == binding.visible_token_text()
}

fn split_error_description_and_json_body(message: &str) -> (String, Option<String>) {
    let message_lines = message.lines().collect::<Vec<_>>();
    let Some(last_non_empty_line_index) = message_lines
        .iter()
        .rposition(|line| !line.trim().is_empty())
    else {
        return (String::new(), None);
    };

    let last_non_empty_line = message_lines[last_non_empty_line_index].trim();
    if let Some(body) = last_non_empty_line
        .strip_prefix("Body:")
        .map(str::trim)
        .filter(|body| is_json_body(body))
    {
        let description = message_lines[..last_non_empty_line_index].join("\n");
        return (description, Some(body.to_string()));
    }

    if is_json_body(last_non_empty_line) {
        let description = message_lines[..last_non_empty_line_index].join("\n");
        return (description, Some(last_non_empty_line.to_string()));
    }

    if let Some((description_suffix, body)) = split_inline_json_body(last_non_empty_line) {
        let mut description_lines = message_lines[..last_non_empty_line_index]
            .iter()
            .map(|line| (*line).to_string())
            .collect::<Vec<_>>();
        description_lines.push(description_suffix.to_string());
        return (description_lines.join("\n"), Some(body.to_string()));
    }

    (message.to_string(), None)
}

fn split_inline_json_body(line: &str) -> Option<(&str, &str)> {
    for (index, character) in line.char_indices() {
        if !matches!(character, '{' | '[') {
            continue;
        }

        let body = line[index..].trim();
        if is_json_body(body) {
            return Some((line[..index].trim_end(), body));
        }
    }

    None
}

fn is_json_body(body: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(body).is_ok()
}

fn normalize_error_description(description: &str) -> String {
    let mut lines = description.lines();
    let Some(first_line) = lines.next() else {
        return String::new();
    };

    let mut normalized_lines = vec![first_line.trim().to_string()];
    normalized_lines.extend(
        lines
            .map(str::trim_end)
            .filter(|line| !line.trim().is_empty())
            .map(str::to_string),
    );
    normalized_lines.join("\n")
}

fn upsert_runtime_tool_activity(model: &mut Model, update: RuntimeToolActivityUpdate) {
    let activity_id = update.activity_id.clone();
    match model.runtime_tool_activity_item_index_from_runtime(&activity_id) {
        Some(item_index) => {
            model.update_runtime_tool_activity_from_runtime(item_index, update);
        }
        None => {
            model.append_runtime_tool_activity_from_runtime(runtime_tool_activity_from_update(
                update,
            ));
        }
    }
}

fn runtime_tool_activity_from_update(update: RuntimeToolActivityUpdate) -> RuntimeToolActivity {
    let activity_id = update.activity_id;
    let title = update
        .title
        .unwrap_or_else(|| format!("Tool activity {activity_id}"));
    RuntimeToolActivity {
        activity_id,
        title,
        kind: update.kind.unwrap_or(RuntimeToolKind::Other),
        status: update.status.unwrap_or(RuntimeToolActivityStatus::Pending),
        content: update.content.unwrap_or_default(),
        locations: update.locations.unwrap_or_default(),
        raw_input: update.raw_input,
        raw_output: update.raw_output,
    }
}

fn show_runtime_permission_request(
    model: &mut Model,
    target: runtime_domain::session::RuntimeTarget,
    request: RuntimePermissionRequest,
) {
    let preview = request
        .tool_activity
        .as_ref()
        .and_then(ToolApprovalPreview::from_runtime_tool_activity_update);
    if let Some(activity_id) = request
        .tool_activity
        .as_ref()
        .map(|tool_activity| tool_activity.activity_id.as_str())
    {
        model.suspend_runtime_tool_activity_approval_from_runtime(activity_id);
    }
    let title = runtime_permission_title(&request);
    let details = runtime_permission_details(&request);
    model.clear_status_notice();
    model.open_tool_approval_panel_with_preview(
        ToolApprovalSource::RuntimePermission {
            target,
            request_id: request.request_id,
            allow_option_id: option_id_for(
                &request.options,
                RuntimePermissionOptionKind::AllowOnce,
            ),
            allow_always_option_id: option_id_for(
                &request.options,
                RuntimePermissionOptionKind::AllowAlways,
            ),
            reject_option_id: option_id_for(
                &request.options,
                RuntimePermissionOptionKind::RejectOnce,
            ),
            reject_always_option_id: option_id_for(
                &request.options,
                RuntimePermissionOptionKind::RejectAlways,
            ),
        },
        title,
        details,
        preview,
    );
}

fn runtime_permission_title(request: &RuntimePermissionRequest) -> String {
    runtime_permission_raw_input(request)
        .and_then(|raw_input| raw_input.string_field(&["command", "cmd"]))
        .map(|command| command.trim().to_string())
        .filter(|command| !command.is_empty())
        .or_else(|| request.title.clone())
        .unwrap_or_default()
}

fn runtime_permission_details(request: &RuntimePermissionRequest) -> Vec<ToolApprovalDetail> {
    let Some(raw_input) = runtime_permission_raw_input(request) else {
        return Vec::new();
    };
    let is_command_request = raw_input
        .string_field(&["command", "cmd"])
        .map(|command| !command.trim().is_empty())
        .unwrap_or(false);
    if !is_command_request {
        return Vec::new();
    }

    let mut details = Vec::new();
    if let Some(description) = raw_input
        .string_field(&["description"])
        .map(|description| description.trim().to_string())
        .filter(|description| !description.is_empty())
    {
        details.push(ToolApprovalDetail {
            // UI 文案保持历史用语：字段已迁移为 `description`，但审批面板仍显示 "Reason"。
            label: "Reason".to_string(),
            value: description,
        });
    }

    if let Some(workdir) = raw_input_string_field(raw_input.as_json(), &["workdir", "cwd"]) {
        details.push(ToolApprovalDetail {
            label: "Workdir".to_string(),
            value: workdir,
        });
    }
    if let Some(timeout) = raw_input_display_field(raw_input.as_json(), &["timeout", "timeout_ms"])
    {
        details.push(ToolApprovalDetail {
            label: "Timeout".to_string(),
            value: timeout,
        });
    }

    details
}

fn runtime_permission_raw_input(
    request: &RuntimePermissionRequest,
) -> Option<&runtime_domain::session::RuntimeToolActivityRawValue> {
    request.tool_activity.as_ref()?.raw_input.as_ref()
}

fn raw_input_string_field(value: &Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(Value::as_str))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn raw_input_display_field(value: &Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| match value.get(*key)? {
        Value::String(text) => {
            let text = text.trim();
            (!text.is_empty()).then(|| text.to_string())
        }
        Value::Number(number) => Some(number.to_string()),
        Value::Bool(boolean) => Some(boolean.to_string()),
        _ => None,
    })
}

fn option_id_for(
    options: &[runtime_domain::session::RuntimePermissionOption],
    kind: RuntimePermissionOptionKind,
) -> Option<String> {
    options
        .iter()
        .find(|option| option.kind == kind)
        .map(|option| option.option_id.clone())
}

#[cfg(test)]
mod tests {
    use super::normalize_chat_failure_message;

    #[test]
    fn chat_failure_message_marks_json_body() {
        let message = "provider error HTTP 401: Invalid status code 401 Unauthorized with message:\n{\"type\":\"error\",\"error\":{\"type\":\"CreditsError\",\"message\":\"Insufficient balance...\"}}";

        assert_eq!(
            normalize_chat_failure_message(message),
            "provider error HTTP 401: Invalid status code 401 Unauthorized with message:\nBody: {\"type\":\"error\",\"error\":{\"type\":\"CreditsError\",\"message\":\"Insufficient balance...\"}}"
        );
    }

    #[test]
    fn chat_failure_message_extracts_inline_json_body() {
        let message = "provider error HTTP 401: Invalid status code 401 Unauthorized with message: {\"type\":\"error\",\"error\":{\"type\":\"CreditsError\",\"message\":\"Insufficient balance...\"}}";

        assert_eq!(
            normalize_chat_failure_message(message),
            "provider error HTTP 401: Invalid status code 401 Unauthorized with message:\nBody: {\"type\":\"error\",\"error\":{\"type\":\"CreditsError\",\"message\":\"Insufficient balance...\"}}"
        );
    }

    #[test]
    fn chat_failure_message_preserves_non_json_details() {
        let message =
            "provider error HTTP 400: HTTP error.\nStatus: 400 Bad Request\nCause: bad request";

        assert_eq!(
            normalize_chat_failure_message(message),
            "provider error HTTP 400: HTTP error.\nStatus: 400 Bad Request\nCause: bad request"
        );
    }
}
