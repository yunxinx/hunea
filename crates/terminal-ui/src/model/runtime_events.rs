use std::time::Duration;

use runtime_domain::session::{
    RuntimeTerminalSnapshot, RuntimeToolActivity, RuntimeToolActivityUpdate,
};

use super::{
    Model,
    runtime_response::{BufferedRuntimeResponse, strip_displayed_reasoning_prefix},
};
use crate::{ReasoningDisplayMode, Sender};

impl Model {
    pub(crate) fn append_assistant_message_from_runtime(&mut self, content: impl Into<String>) {
        let content = content.into();
        if content.is_empty() {
            return;
        }

        let preserved_viewport_state = self.preserved_viewport_state_for_transcript_refresh();
        let style_mode = self.style_mode;
        self.transcript_mut().append_message_with_style_mode(
            Sender::Assistant,
            content,
            style_mode,
        );
        self.refresh_status_line_after_transcript_change();
        self.sync_transcript_render();
        self.document_runtime.follow_bottom = true;
        self.sync_document_viewport_after_transcript_refresh(preserved_viewport_state);
    }

    pub(crate) fn append_runtime_response_from_runtime(
        &mut self,
        content: impl Into<String>,
        reasoning_content: Option<String>,
        reasoning_duration: Option<Duration>,
    ) {
        let content = content.into();
        let reasoning_content = reasoning_content
            .filter(|content| !content.trim().is_empty())
            .filter(|_| self.show_reasoning_content);

        if content.is_empty() && reasoning_content.is_none() {
            return;
        }

        if let Some(reasoning_content) = reasoning_content {
            self.append_assistant_message_with_reasoning_from_runtime(
                content,
                reasoning_content,
                reasoning_duration,
            );
            return;
        }

        self.append_assistant_message_from_runtime(content);
    }

    pub(crate) fn push_runtime_assistant_delta(&mut self, content: &str) {
        if content.is_empty() {
            return;
        }
        if self.runtime_response_buffer.is_empty() {
            let _ = self.mark_exploration_tool_activities_complete_from_runtime();
        }
        self.flush_runtime_reasoning_for_expanded_family_display();
        self.runtime_response_buffer.push_content(content);
    }

    pub(crate) fn push_runtime_reasoning_delta(&mut self, content: &str) {
        if content.is_empty() {
            return;
        }
        if self.runtime_response_buffer.is_empty() {
            let _ = self.mark_exploration_tool_activities_complete_from_runtime();
        }
        self.runtime_response_buffer.push_reasoning_content(content);
    }

    pub(crate) fn flush_runtime_response_buffer(&mut self) {
        if let Some(response) = self.runtime_response_buffer.take() {
            self.append_buffered_runtime_response_from_runtime(
                response,
                self.streams_reasoning_into_transcript_during_response(),
            );
        }
    }

    pub(crate) fn flush_runtime_response_buffer_with_final(
        &mut self,
        final_content: String,
        final_reasoning_content: Option<String>,
        final_reasoning_duration: Option<Duration>,
    ) {
        // Expanded reasoning that already crossed a text/tool boundary is
        // committed display state. The final provider `reasoning_content`
        // should only supplement the still-buffered tail that has not been
        // rendered yet.
        let displayed_reasoning_content = self.streamed_runtime_reasoning.displayed_content.clone();
        let buffered_response = if displayed_reasoning_content.is_empty() {
            self.runtime_response_buffer.take_with_final(
                final_content,
                final_reasoning_content,
                final_reasoning_duration,
            )
        } else if self.runtime_response_buffer.has_reasoning_content() {
            self.runtime_response_buffer.take_with_final(
                final_content,
                strip_displayed_reasoning_prefix(
                    final_reasoning_content,
                    &displayed_reasoning_content,
                ),
                final_reasoning_duration,
            )
        } else {
            self.runtime_response_buffer
                .take_with_final(final_content, None, None)
        };

        if let Some(response) = buffered_response {
            self.append_final_buffered_runtime_response_from_runtime(response);
        }
        self.accept_streamed_runtime_reasoning_from_runtime();
    }

    pub(crate) fn clear_runtime_response_buffer(&mut self) {
        self.runtime_response_buffer.clear();
        self.discard_streamed_runtime_reasoning_from_runtime();
    }

    pub(crate) fn streams_reasoning_into_transcript_during_response(&self) -> bool {
        // expanded-simplified 只是 expanded 的主界面 compact 渲染策略；
        // reasoning 进入 transcript 的时机必须和 expanded 保持一致。
        self.show_reasoning_content
            && matches!(
                self.reasoning_display_mode,
                ReasoningDisplayMode::Expanded | ReasoningDisplayMode::ExpandedSimplified
            )
    }

    pub(crate) fn flush_runtime_reasoning_for_expanded_family_display(&mut self) {
        if !self.streams_reasoning_into_transcript_during_response() {
            return;
        }
        if self.runtime_final_body_divider_pending {
            return;
        }

        let Some(reasoning) = self
            .runtime_response_buffer
            .take_reasoning_for_expanded_display()
        else {
            return;
        };
        self.append_buffered_runtime_response_from_runtime(
            BufferedRuntimeResponse {
                content: String::new(),
                reasoning_content: Some(reasoning.content),
                reasoning_duration: reasoning.duration,
            },
            true,
        );
    }

    pub(crate) fn accept_streamed_runtime_reasoning_from_runtime(&mut self) {
        self.streamed_runtime_reasoning.item_indices.clear();
        self.streamed_runtime_reasoning.displayed_content.clear();
    }

    fn append_buffered_runtime_response_from_runtime(
        &mut self,
        response: BufferedRuntimeResponse,
        track_streamed_reasoning: bool,
    ) {
        let tracked_reasoning_content = track_streamed_reasoning
            .then_some(response.reasoning_content.as_deref())
            .flatten()
            .map(str::to_owned);
        let reasoning_item_index = track_streamed_reasoning
            .then_some(response.reasoning_content.as_ref())
            .flatten()
            .map(|_| self.transcript.len());

        self.append_runtime_response_from_runtime(
            response.content,
            response.reasoning_content,
            response.reasoning_duration,
        );

        if let Some(item_index) = reasoning_item_index
            && self.transcript.len() > item_index
        {
            self.streamed_runtime_reasoning
                .item_indices
                .push(item_index);
        }

        if let Some(reasoning_content) = tracked_reasoning_content {
            self.streamed_runtime_reasoning
                .displayed_content
                .push_str(&reasoning_content);
        }
    }

    fn append_final_buffered_runtime_response_from_runtime(
        &mut self,
        response: BufferedRuntimeResponse,
    ) {
        let should_insert_divider =
            self.runtime_final_body_divider_pending && !response.content.is_empty();
        if !should_insert_divider {
            self.append_runtime_response_from_runtime(
                response.content,
                response.reasoning_content,
                response.reasoning_duration,
            );
            return;
        }

        let visible_reasoning_content = response
            .reasoning_content
            .filter(|content| !content.trim().is_empty())
            .filter(|_| self.show_reasoning_content);
        if let Some(reasoning_content) = visible_reasoning_content {
            self.append_assistant_message_with_reasoning_from_runtime(
                String::new(),
                reasoning_content,
                response.reasoning_duration,
            );
        }
        self.append_runtime_final_body_divider_if_pending();
        self.append_assistant_message_from_runtime(response.content);
    }

    fn discard_streamed_runtime_reasoning_from_runtime(&mut self) {
        let item_indices = std::mem::take(&mut self.streamed_runtime_reasoning.item_indices);
        if item_indices.is_empty() {
            return;
        }

        let preserved_viewport_state = self.preserved_viewport_state_for_transcript_refresh();
        if !self.transcript_mut().remove_items(&item_indices) {
            return;
        }
        self.refresh_status_line_after_transcript_change();
        self.sync_transcript_render();
        self.document_runtime.follow_bottom = true;
        self.sync_document_viewport_after_transcript_refresh(preserved_viewport_state);
    }

    fn append_assistant_message_with_reasoning_from_runtime(
        &mut self,
        content: impl Into<String>,
        reasoning_content: impl Into<String>,
        reasoning_duration: Option<Duration>,
    ) {
        let content = content.into();
        let reasoning_content = reasoning_content.into();
        if content.is_empty() && reasoning_content.is_empty() {
            return;
        }

        let preserved_viewport_state = self.preserved_viewport_state_for_transcript_refresh();
        let style_mode = self.style_mode;
        let reasoning_display_mode = self.reasoning_display_mode;
        self.transcript_mut()
            .append_assistant_message_with_reasoning(
                content,
                reasoning_content,
                reasoning_display_mode,
                reasoning_duration,
                style_mode,
            );
        self.refresh_status_line_after_transcript_change();
        self.sync_transcript_render();
        self.document_runtime.follow_bottom = true;
        self.sync_document_viewport_after_transcript_refresh(preserved_viewport_state);
    }

    pub(crate) fn toggle_reasoning_item(&mut self, item_index: usize) -> bool {
        let preserved_viewport_state = self.preserved_viewport_state_for_transcript_refresh();
        if !self.transcript_mut().toggle_reasoning_item(item_index) {
            return false;
        }

        self.sync_transcript_render();
        self.sync_document_viewport_after_transcript_refresh(preserved_viewport_state);
        true
    }

    pub(crate) fn append_system_message_from_runtime(&mut self, content: impl Into<String>) {
        self.append_local_system_message(content);
    }

    pub(crate) fn append_local_system_message(&mut self, content: impl Into<String>) {
        let content = content.into();
        if content.is_empty() {
            return;
        }

        let preserved_viewport_state = self.preserved_viewport_state_for_transcript_refresh();
        self.transcript_mut().append_system_message(content);
        self.refresh_status_line_after_transcript_change();
        self.sync_transcript_render();
        self.document_runtime.follow_bottom = true;
        self.sync_document_viewport_after_transcript_refresh(preserved_viewport_state);
    }

    pub(crate) fn present_pending_prompt_assembly_notice_if_ready(&mut self) {
        if self.prompt_overlay_active() {
            return;
        }
        let Some(notice) = self.pending_prompt_assembly_notice.take() else {
            return;
        };
        match notice {
            runtime_domain::session::PromptAssemblyUpdateNotice::CurrentEmptySessionUpdated => {
                self.append_local_system_message("Prompt updated for current empty session.");
            }
            runtime_domain::session::PromptAssemblyUpdateNotice::NextNewSessionUpdated => {
                self.show_toast(
                    crate::toast::ToastSeverity::Info,
                    "Prompt updated. Applies to next new session.",
                );
            }
        }
    }

    pub(crate) fn append_work_duration_from_runtime(&mut self, duration: Duration) {
        let preserved_viewport_state = self.preserved_viewport_state_for_transcript_refresh();
        self.transcript_mut().append_work_duration_message(duration);
        self.refresh_status_line_after_transcript_change();
        self.sync_transcript_render();
        self.document_runtime.follow_bottom = true;
        self.sync_document_viewport_after_transcript_refresh(preserved_viewport_state);
    }

    pub(crate) fn reset_runtime_final_body_divider_state(&mut self) {
        self.runtime_turn_tool_call_count = 0;
        self.runtime_final_body_divider_pending = false;
        self.runtime_final_body_divider_inserted = false;
    }

    pub(crate) fn record_runtime_tool_activity_started_for_final_body_divider(&mut self) {
        self.runtime_turn_tool_call_count = self.runtime_turn_tool_call_count.saturating_add(1);
        if self.runtime_turn_tool_call_count > 3 && !self.runtime_final_body_divider_inserted {
            self.runtime_final_body_divider_pending = true;
        }
    }

    fn append_runtime_final_body_divider_if_pending(&mut self) {
        if !self.runtime_final_body_divider_pending || self.runtime_final_body_divider_inserted {
            return;
        }

        let preserved_viewport_state = self.preserved_viewport_state_for_transcript_refresh();
        self.transcript_mut().append_final_body_divider();
        self.runtime_final_body_divider_pending = false;
        self.runtime_final_body_divider_inserted = true;
        self.refresh_status_line_after_transcript_change();
        self.sync_transcript_render();
        self.document_runtime.follow_bottom = true;
        self.sync_document_viewport_after_transcript_refresh(preserved_viewport_state);
    }

    pub(crate) fn append_tool_result_from_runtime(
        &mut self,
        content: impl Into<String>,
        kind: crate::tool_result::ToolResultKind,
    ) {
        let content = content.into();
        if content.is_empty() {
            return;
        }

        let preserved_viewport_state = self.preserved_viewport_state_for_transcript_refresh();
        self.transcript_mut().append_tool_result(content, kind);
        self.refresh_status_line_after_transcript_change();
        self.sync_transcript_render();
        self.document_runtime.follow_bottom = true;
        self.sync_document_viewport_after_transcript_refresh(preserved_viewport_state);
    }

    pub(crate) fn append_runtime_tool_activity_from_runtime(
        &mut self,
        call: impl Into<RuntimeToolActivity>,
    ) -> usize {
        let call = call.into();
        let preserved_viewport_state = self.preserved_viewport_state_for_transcript_refresh();
        let item_index = self.transcript_mut().append_runtime_tool_activity(call);
        let snapshots = self.runtime_terminal_snapshots.clone();
        for snapshot in snapshots {
            let _ = self
                .transcript_mut()
                .set_runtime_terminal_snapshot(snapshot);
        }
        self.refresh_status_line_after_transcript_change();
        self.sync_transcript_render();
        self.document_runtime.follow_bottom = true;
        self.sync_document_viewport_after_transcript_refresh(preserved_viewport_state);
        item_index
    }

    pub(crate) fn mark_exploration_tool_activities_complete_from_runtime(&mut self) -> bool {
        let preserved_viewport_state = self.preserved_viewport_state_for_transcript_refresh();
        if !self
            .transcript_mut()
            .mark_exploration_tool_activities_complete()
        {
            return false;
        }
        self.refresh_status_line_after_transcript_change();
        self.sync_transcript_render();
        self.document_runtime.follow_bottom = true;
        self.sync_document_viewport_after_transcript_refresh(preserved_viewport_state);
        true
    }

    pub(crate) fn runtime_tool_activity_item_index_from_runtime(
        &self,
        tool_call_id: &str,
    ) -> Option<usize> {
        self.transcript.runtime_tool_activity_index(tool_call_id)
    }

    pub(crate) fn update_runtime_tool_activity_from_runtime(
        &mut self,
        item_index: usize,
        update: impl Into<RuntimeToolActivityUpdate>,
    ) -> bool {
        let update = update.into();
        let preserved_viewport_state = self.preserved_viewport_state_for_transcript_refresh();
        if !self
            .transcript_mut()
            .update_runtime_tool_activity(item_index, update)
        {
            return false;
        }
        let snapshots = self.runtime_terminal_snapshots.clone();
        for snapshot in snapshots {
            let _ = self
                .transcript_mut()
                .set_runtime_terminal_snapshot(snapshot);
        }
        self.refresh_status_line_after_transcript_change();
        self.sync_transcript_render();
        if preserved_viewport_state.is_none() {
            self.document_runtime.follow_bottom = true;
        }
        self.sync_document_viewport_after_transcript_refresh(preserved_viewport_state);
        true
    }

    /// `suspend_runtime_tool_activity_approval_from_runtime` 在审批面板打开期间隐藏重复的等待行。
    pub(crate) fn suspend_runtime_tool_activity_approval_from_runtime(
        &mut self,
        activity_id: &str,
    ) -> bool {
        let preserved_viewport_state = self.preserved_viewport_state_for_transcript_refresh();
        if !self
            .transcript_mut()
            .set_runtime_tool_activity_approval_suspended(activity_id, true)
        {
            return false;
        }
        self.refresh_status_line_after_transcript_change();
        self.sync_transcript_render();
        self.document_runtime.follow_bottom = true;
        self.sync_document_viewport_after_transcript_refresh(preserved_viewport_state);
        true
    }

    /// `clear_runtime_tool_activity_approval_suspensions_from_runtime` 恢复被审批面板隐藏的工具行。
    pub(crate) fn clear_runtime_tool_activity_approval_suspensions_from_runtime(&mut self) -> bool {
        let preserved_viewport_state = self.preserved_viewport_state_for_transcript_refresh();
        if !self
            .transcript_mut()
            .clear_runtime_tool_activity_approval_suspensions()
        {
            return false;
        }
        self.refresh_status_line_after_transcript_change();
        self.sync_transcript_render();
        self.document_runtime.follow_bottom = true;
        self.sync_document_viewport_after_transcript_refresh(preserved_viewport_state);
        true
    }

    pub(crate) fn apply_runtime_terminal_snapshot_from_runtime(
        &mut self,
        snapshot: impl Into<RuntimeTerminalSnapshot>,
    ) -> bool {
        let snapshot = snapshot.into();
        let preserved_viewport_state = self.preserved_viewport_state_for_transcript_refresh();
        self.runtime_terminal_snapshots
            .retain(|stored| stored.terminal_id != snapshot.terminal_id);
        self.runtime_terminal_snapshots.push(snapshot.clone());
        if !self
            .transcript_mut()
            .set_runtime_terminal_snapshot(snapshot)
        {
            return false;
        }
        self.refresh_status_line_after_transcript_change();
        self.sync_transcript_render();
        if preserved_viewport_state.is_none() {
            self.document_runtime.follow_bottom = true;
        }
        self.sync_document_viewport_after_transcript_refresh(preserved_viewport_state);
        true
    }
}
