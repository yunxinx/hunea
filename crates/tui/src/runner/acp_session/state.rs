use std::{
    collections::{HashMap, HashSet},
    time::{Duration, Instant},
};

use crate::RequestMetrics;
use mo_core::{
    session::{RuntimePermissionRequest, RuntimeTerminalSnapshot, RuntimeToolActivityContent},
    token_count::StreamingTokenProgress,
};

use super::{
    acp_reject_option_id_for_stale_discard, is_agent_facing_permission_rejection_notice,
    is_agent_facing_permission_rejection_notice_prefix,
};

/// `AcpSessionUiState` 保存 ACP 流式输出映射到 TUI 所需的临时状态。
#[derive(Default)]
pub(in crate::runner) struct AcpSessionUiState {
    response_buffer: String,
    reasoning_buffer: String,
    reasoning_started_at: Option<Instant>,
    pending_rejected_permission_notice_suppression: bool,
    prompt_in_flight: bool,
    discard_in_flight_prompt: Option<PromptDiscardReason>,
    token_progress: Option<StreamingTokenProgress>,
    prompt_started_at: Option<Instant>,
    first_token_at: Option<Instant>,
    tool_call_items: HashMap<String, usize>,
    tool_call_terminal_ids: HashMap<String, HashSet<String>>,
    terminal_active_states: HashMap<String, bool>,
    tool_call_token_text: HashMap<String, String>,
    rejected_permission_tool_calls: HashSet<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PromptDiscardReason {
    Cancelled,
    Stale,
}

impl AcpSessionUiState {
    pub(in crate::runner) fn reset_for_new_session(&mut self) {
        self.response_buffer.clear();
        self.reasoning_buffer.clear();
        self.reasoning_started_at = None;
        self.pending_rejected_permission_notice_suppression = false;
        self.prompt_in_flight = false;
        self.discard_in_flight_prompt = None;
        self.token_progress = None;
        self.prompt_started_at = None;
        self.first_token_at = None;
        self.tool_call_items.clear();
        self.tool_call_terminal_ids.clear();
        self.terminal_active_states.clear();
        self.tool_call_token_text.clear();
        self.rejected_permission_tool_calls.clear();
    }

    pub(in crate::runner) fn reset_response_buffer(&mut self) {
        self.response_buffer.clear();
        self.reasoning_buffer.clear();
        self.reasoning_started_at = None;
        self.pending_rejected_permission_notice_suppression = false;
    }

    pub(in crate::runner) fn push_response_chunk(&mut self, content: &str) {
        if !content.is_empty() {
            self.first_token_at.get_or_insert_with(Instant::now);
        }
        self.response_buffer.push_str(content);
    }

    pub(in crate::runner) fn push_reasoning_chunk(&mut self, content: &str) {
        if content.is_empty() {
            return;
        }
        self.first_token_at.get_or_insert_with(Instant::now);
        if self.reasoning_started_at.is_none() {
            self.reasoning_started_at = Some(Instant::now());
        }
        self.reasoning_buffer.push_str(content);
    }

    pub(in crate::runner) fn response_buffers_empty(&self) -> bool {
        self.response_buffer.is_empty() && self.reasoning_buffer.is_empty()
    }

    pub(in crate::runner) fn take_response_buffer(&mut self) -> Option<String> {
        if self.response_buffer.is_empty() {
            return None;
        }

        if self.pending_rejected_permission_notice_suppression {
            if is_agent_facing_permission_rejection_notice(&self.response_buffer) {
                self.response_buffer.clear();
                self.pending_rejected_permission_notice_suppression = false;
                return None;
            }

            if is_agent_facing_permission_rejection_notice_prefix(&self.response_buffer) {
                return None;
            }

            self.pending_rejected_permission_notice_suppression = false;
        }

        Some(std::mem::take(&mut self.response_buffer))
    }

    pub(in crate::runner) fn take_reasoning_buffer(
        &mut self,
    ) -> (Option<String>, Option<Duration>) {
        if self.reasoning_buffer.is_empty() {
            self.reasoning_started_at = None;
            return (None, None);
        }

        let duration = self
            .reasoning_started_at
            .take()
            .map(|started_at| Instant::now().saturating_duration_since(started_at));
        (Some(std::mem::take(&mut self.reasoning_buffer)), duration)
    }

    pub(in crate::runner) fn mark_prompt_submitted(&mut self) {
        self.prompt_in_flight = true;
    }

    pub(in crate::runner) fn mark_prompt_started(&mut self) {
        self.prompt_in_flight = true;
        self.prompt_started_at = Some(Instant::now());
        self.first_token_at = None;
        self.tool_call_items.clear();
        self.tool_call_terminal_ids.clear();
        self.tool_call_token_text.clear();
    }

    pub(in crate::runner) fn start_token_progress(&mut self, model_id: impl Into<String>) {
        self.token_progress = Some(StreamingTokenProgress::new(model_id));
    }

    pub(in crate::runner) fn observe_output_tokens(&mut self, content: &str) -> Option<usize> {
        if !content.is_empty() {
            self.first_token_at.get_or_insert_with(Instant::now);
        }
        self.token_progress
            .as_mut()
            .and_then(|progress| progress.observe_delta(content, Instant::now()))
    }

    pub(in crate::runner) fn observe_tool_call_tokens(
        &mut self,
        tool_call_id: &str,
        projected_text: Option<String>,
    ) -> Option<usize> {
        let projected_text = projected_text?;
        if projected_text.is_empty() {
            return None;
        }

        let previous = self
            .tool_call_token_text
            .entry(tool_call_id.to_string())
            .or_default();
        let delta = if projected_text.starts_with(previous.as_str()) {
            projected_text[previous.len()..].to_string()
        } else {
            projected_text.clone()
        };
        *previous = projected_text;

        self.observe_output_tokens(&delta)
    }

    pub(in crate::runner) fn flush_output_tokens(&mut self) -> Option<usize> {
        self.token_progress
            .as_mut()
            .and_then(|progress| progress.flush(Instant::now()))
    }

    pub(in crate::runner) fn total_output_tokens(&self) -> usize {
        self.token_progress
            .as_ref()
            .map(StreamingTokenProgress::total_tokens)
            .unwrap_or(0)
    }

    pub(in crate::runner) fn request_metrics(
        &self,
        finished_at: Instant,
    ) -> Option<RequestMetrics> {
        let prompt_started_at = self.prompt_started_at?;
        let first_token_at = self.first_token_at?;
        Some(RequestMetrics::new(
            first_token_at.saturating_duration_since(prompt_started_at),
            self.total_output_tokens(),
            finished_at.saturating_duration_since(prompt_started_at),
        ))
    }

    pub(in crate::runner) fn mark_prompt_finished(&mut self) {
        self.prompt_in_flight = false;
        self.discard_in_flight_prompt = None;
        self.response_buffer.clear();
        self.reasoning_buffer.clear();
        self.reasoning_started_at = None;
        self.pending_rejected_permission_notice_suppression = false;
        self.token_progress = None;
        self.prompt_started_at = None;
        self.first_token_at = None;
        self.tool_call_items.clear();
        self.tool_call_terminal_ids.clear();
        self.tool_call_token_text.clear();
    }

    pub(in crate::runner) fn should_discard_prompt_output(&self) -> bool {
        self.discard_in_flight_prompt.is_some()
    }

    pub(in crate::runner) fn permission_option_id_for_discarded_prompt(
        &self,
        request: &RuntimePermissionRequest,
    ) -> Option<String> {
        match self.discard_in_flight_prompt {
            Some(PromptDiscardReason::Cancelled) => None,
            Some(PromptDiscardReason::Stale) | None => {
                acp_reject_option_id_for_stale_discard(request)
            }
        }
    }

    pub(in crate::runner) fn suppress_rejected_permission_notice_for_tool_call(
        &mut self,
        tool_call_id: Option<String>,
    ) {
        self.pending_rejected_permission_notice_suppression = true;
        if let Some(tool_call_id) = tool_call_id {
            self.rejected_permission_tool_calls.insert(tool_call_id);
        }
    }

    pub(in crate::runner) fn should_sanitize_rejected_permission_tool_update(
        &self,
        tool_call_id: &str,
    ) -> bool {
        self.pending_rejected_permission_notice_suppression
            || self.rejected_permission_tool_calls.contains(tool_call_id)
    }

    pub(in crate::runner) fn interrupt_prompt(&mut self) -> bool {
        if !self.prompt_in_flight {
            return false;
        }
        self.response_buffer.clear();
        self.reasoning_buffer.clear();
        self.reasoning_started_at = None;
        self.pending_rejected_permission_notice_suppression = false;
        self.token_progress = None;
        self.prompt_started_at = None;
        self.first_token_at = None;
        self.discard_in_flight_prompt = Some(PromptDiscardReason::Cancelled);
        self.tool_call_token_text.clear();
        true
    }

    pub(in crate::runner) fn reset_after_clear(&mut self) {
        self.response_buffer.clear();
        self.reasoning_buffer.clear();
        self.reasoning_started_at = None;
        self.pending_rejected_permission_notice_suppression = false;
        self.token_progress = None;
        self.prompt_started_at = None;
        self.first_token_at = None;
        self.tool_call_items.clear();
        self.tool_call_terminal_ids.clear();
        self.tool_call_token_text.clear();
        self.rejected_permission_tool_calls.clear();
        if self.prompt_in_flight && self.discard_in_flight_prompt.is_none() {
            self.discard_in_flight_prompt = Some(PromptDiscardReason::Stale);
        }
    }

    pub(in crate::runner) fn track_tool_call(&mut self, tool_call_id: String, item_index: usize) {
        self.tool_call_items.insert(tool_call_id, item_index);
    }

    pub(in crate::runner) fn track_tool_call_terminal_content(
        &mut self,
        tool_call_id: &str,
        content: Option<&[RuntimeToolActivityContent]>,
    ) {
        let Some(content) = content else {
            return;
        };
        let terminal_ids = content
            .iter()
            .filter_map(|content| match content {
                RuntimeToolActivityContent::Terminal { terminal_id } => Some(terminal_id.clone()),
                _ => None,
            })
            .collect::<HashSet<_>>();
        if terminal_ids.is_empty() {
            self.tool_call_terminal_ids.remove(tool_call_id);
        } else {
            self.tool_call_terminal_ids
                .insert(tool_call_id.to_string(), terminal_ids);
        }
    }

    pub(in crate::runner) fn observe_terminal_snapshot(
        &mut self,
        snapshot: &RuntimeTerminalSnapshot,
    ) {
        self.terminal_active_states.insert(
            snapshot.terminal_id.clone(),
            snapshot.exit_status.is_none() && !snapshot.released,
        );
    }

    pub(in crate::runner) fn tool_call_item_index(&self, tool_call_id: &str) -> Option<usize> {
        self.tool_call_items.get(tool_call_id).copied()
    }

    pub(in crate::runner) fn tracked_non_background_tool_call_indices(&self) -> Vec<usize> {
        self.tool_call_items
            .iter()
            .filter_map(|(tool_call_id, item_index)| {
                (!self.tool_call_has_running_or_pending_terminal(tool_call_id))
                    .then_some(*item_index)
            })
            .collect()
    }

    pub(in crate::runner) fn tool_call_has_running_or_pending_terminal(
        &self,
        tool_call_id: &str,
    ) -> bool {
        self.tool_call_terminal_ids
            .get(tool_call_id)
            .is_some_and(|terminal_ids| {
                terminal_ids.iter().any(|terminal_id| {
                    self.terminal_active_states
                        .get(terminal_id)
                        .copied()
                        .unwrap_or(true)
                })
            })
    }

    pub(in crate::runner) fn clear_tool_call_tracking(&mut self) {
        self.tool_call_items.clear();
        self.tool_call_terminal_ids.clear();
        self.tool_call_token_text.clear();
    }
}
