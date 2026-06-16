use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

use provider_protocol::{ConversationItem, PromptCompletion, ToolCall as AiToolCall};
use runtime_domain::{
    session::{
        ProviderRequestMetrics, RuntimeTerminalSnapshot, RuntimeToolActivityContent,
        RuntimeToolActivityUpdate,
    },
    token_count::StreamingTokenProgress,
};

use super::{ToolLoopCompletion, ToolLoopProgress, ToolLoopResponse};

pub(super) struct RuntimeTurnState {
    model_id: String,
    reasoning_content: String,
    // 可见进度包含工具输出；statusline metrics 只能使用 LLM 输出。
    output_progress: StreamingTokenProgress,
    llm_output_progress: StreamingTokenProgress,
    input_progress: StreamingTokenProgress,
    is_thinking: bool,
    reasoning_started_at: Option<Instant>,
    reasoning_finished_at: Option<Instant>,
    request_started_at: Option<Instant>,
    first_token_at: Option<Instant>,
    current_provider_turn_started_at: Option<Instant>,
    current_provider_generation_started_at: Option<Instant>,
    llm_generation_duration: Duration,
    current_provider_output_tokens: Option<usize>,
    llm_output_tokens_total: usize,
    terminal_output_by_id: HashMap<String, String>,
    tool_call_ids_by_index: HashMap<usize, String>,
    tool_call_argument_output_by_index: HashMap<usize, String>,
    tool_call_argument_output_by_id: HashMap<String, String>,
}

impl RuntimeTurnState {
    pub(super) fn new(model_id: String) -> Self {
        Self {
            model_id: model_id.clone(),
            reasoning_content: String::new(),
            output_progress: StreamingTokenProgress::new(model_id.clone()),
            llm_output_progress: StreamingTokenProgress::new(model_id.clone()),
            input_progress: StreamingTokenProgress::new(model_id),
            is_thinking: false,
            reasoning_started_at: None,
            reasoning_finished_at: None,
            request_started_at: None,
            first_token_at: None,
            current_provider_turn_started_at: None,
            current_provider_generation_started_at: None,
            llm_generation_duration: Duration::ZERO,
            current_provider_output_tokens: None,
            llm_output_tokens_total: 0,
            terminal_output_by_id: HashMap::new(),
            tool_call_ids_by_index: HashMap::new(),
            tool_call_argument_output_by_index: HashMap::new(),
            tool_call_argument_output_by_id: HashMap::new(),
        }
    }

    pub(super) fn mark_request_started(&mut self, now: Instant) {
        self.request_started_at.get_or_insert(now);
    }

    pub(super) fn start_provider_turn(&mut self, now: Instant) {
        self.mark_request_started(now);
        self.current_provider_turn_started_at = Some(now);
        self.current_provider_generation_started_at = None;
        self.current_provider_output_tokens = None;
        self.llm_output_progress = StreamingTokenProgress::new(self.model_id.clone());
    }

    pub(super) fn record_provider_output_usage(&mut self, output_tokens: usize) {
        self.current_provider_output_tokens = Some(
            self.current_provider_output_tokens
                .map(|current| current.max(output_tokens))
                .unwrap_or(output_tokens),
        );
    }

    pub(super) fn complete_provider_turn(
        &mut self,
        response: &PromptCompletion,
        finished_at: Instant,
    ) {
        let response_output_tokens = response
            .usage
            .as_ref()
            .and_then(|usage| usage.output_tokens)
            .map(|tokens| tokens as usize);
        let turn_output_tokens = self
            .current_provider_output_tokens
            .max(response_output_tokens);
        let _ = self.llm_output_progress.flush(finished_at);
        let estimated_output_tokens = self.llm_output_progress.total_tokens();
        self.llm_output_tokens_total =
            self.llm_output_tokens_total
                .saturating_add(match turn_output_tokens {
                    Some(output_tokens) => output_tokens,
                    None => estimated_output_tokens,
                });

        if self.provider_turn_has_output(response, turn_output_tokens) {
            self.first_token_at.get_or_insert(finished_at);
            if let Some(generation_started_at) = self.current_provider_generation_started_at {
                self.llm_generation_duration = self
                    .llm_generation_duration
                    .saturating_add(finished_at.saturating_duration_since(generation_started_at));
            }
        }

        self.current_provider_turn_started_at = None;
        self.current_provider_generation_started_at = None;
        self.current_provider_output_tokens = None;
    }

    fn provider_turn_has_output(
        &self,
        response: &PromptCompletion,
        output_tokens: Option<usize>,
    ) -> bool {
        self.current_provider_generation_started_at.is_some()
            || output_tokens.is_some_and(|tokens| tokens > 0)
            || response
                .items
                .iter()
                .any(|item| !item.text_content().is_empty())
            || response
                .items
                .iter()
                .any(|item| item.tool_calls().next().is_some())
    }

    pub(super) fn observe_content_chunk(
        &mut self,
        content: &str,
        now: Instant,
        on_progress: &mut impl FnMut(ToolLoopProgress),
    ) {
        if content.is_empty() {
            return;
        }
        self.mark_generated_output_started(now, on_progress);
        on_progress(ToolLoopProgress::AssistantDelta {
            content: content.to_string(),
        });
        self.observe_provider_output_delta(content, now, on_progress);
    }

    pub(super) fn observe_reasoning_chunk(
        &mut self,
        content: &str,
        now: Instant,
        on_progress: &mut impl FnMut(ToolLoopProgress),
    ) {
        if content.is_empty() {
            return;
        }
        self.mark_llm_output_started(now);
        if !self.is_thinking {
            self.is_thinking = true;
            self.reasoning_started_at.get_or_insert(now);
            on_progress(ToolLoopProgress::Thinking { is_thinking: true });
        }
        self.reasoning_finished_at = Some(now);
        self.reasoning_content.push_str(content);
        on_progress(ToolLoopProgress::ReasoningDelta {
            content: content.to_string(),
        });
        self.observe_provider_output_delta(content, now, on_progress);
    }

    fn observe_provider_output_delta(
        &mut self,
        content: &str,
        now: Instant,
        on_progress: &mut impl FnMut(ToolLoopProgress),
    ) {
        self.observe_llm_output_delta(content, now);
        self.observe_visible_output_delta(content, now, on_progress);
    }

    fn observe_llm_output_delta(&mut self, content: &str, now: Instant) {
        let _ = self.llm_output_progress.observe_delta(content, now);
    }

    fn observe_visible_output_delta(
        &mut self,
        content: &str,
        now: Instant,
        on_progress: &mut impl FnMut(ToolLoopProgress),
    ) {
        if let Some(total_tokens) = self.output_progress.observe_delta(content, now) {
            on_progress(ToolLoopProgress::OutputTokens { total_tokens });
        }
    }

    pub(super) fn observe_tool_call_started(&mut self, index: usize, call_id: String) {
        self.tool_call_ids_by_index.insert(index, call_id.clone());
        if let Some(output) = self.tool_call_argument_output_by_index.get(&index) {
            self.tool_call_argument_output_by_id
                .insert(call_id, output.clone());
        }
    }

    pub(super) fn observe_tool_call_arguments_delta(
        &mut self,
        index: usize,
        delta: &str,
        now: Instant,
        on_progress: &mut impl FnMut(ToolLoopProgress),
    ) {
        if delta.is_empty() {
            return;
        }
        self.mark_generated_output_started(now, on_progress);
        let output = self
            .tool_call_argument_output_by_index
            .entry(index)
            .or_default();
        output.push_str(delta);
        if let Some(call_id) = self.tool_call_ids_by_index.get(&index) {
            self.tool_call_argument_output_by_id
                .insert(call_id.clone(), output.clone());
        }
        self.observe_provider_output_delta(delta, now, on_progress);
    }

    pub(super) fn observe_response_tool_calls_completed(
        &mut self,
        response: &PromptCompletion,
        now: Instant,
        on_progress: &mut impl FnMut(ToolLoopProgress),
    ) {
        let mut index = 0usize;
        for item in &response.items {
            for call in item.tool_calls() {
                self.observe_tool_call_completed(index, call, now, on_progress);
                index = index.saturating_add(1);
            }
        }
    }

    pub(super) fn observe_tool_call_completed(
        &mut self,
        index: usize,
        call: &AiToolCall,
        now: Instant,
        on_progress: &mut impl FnMut(ToolLoopProgress),
    ) {
        self.tool_call_ids_by_index
            .insert(index, call.call_id.clone());
        if let Some(output) = self.tool_call_argument_output_by_id.get(&call.call_id) {
            self.tool_call_argument_output_by_index
                .entry(index)
                .or_insert_with(|| output.clone());
            return;
        }
        if let Some(output) = self.tool_call_argument_output_by_index.get(&index) {
            self.tool_call_argument_output_by_id
                .insert(call.call_id.clone(), output.clone());
            return;
        }

        let output = call.arguments.to_string();
        if output.is_empty() {
            return;
        }
        self.mark_completed_generated_output_observed(now, on_progress);
        self.tool_call_argument_output_by_index
            .insert(index, output.clone());
        self.tool_call_argument_output_by_id
            .insert(call.call_id.clone(), output.clone());
        self.observe_provider_output_delta(&output, now, on_progress);
    }

    fn mark_generated_output_started(
        &mut self,
        now: Instant,
        on_progress: &mut impl FnMut(ToolLoopProgress),
    ) {
        self.mark_llm_output_started(now);
        self.stop_reasoning_for_generated_output(now, on_progress);
    }

    fn mark_completed_generated_output_observed(
        &mut self,
        now: Instant,
        on_progress: &mut impl FnMut(ToolLoopProgress),
    ) {
        self.first_token_at.get_or_insert(now);
        self.stop_reasoning_for_generated_output(now, on_progress);
    }

    fn mark_llm_output_started(&mut self, now: Instant) {
        self.first_token_at.get_or_insert(now);
        self.current_provider_generation_started_at
            .get_or_insert(now);
    }

    fn stop_reasoning_for_generated_output(
        &mut self,
        now: Instant,
        on_progress: &mut impl FnMut(ToolLoopProgress),
    ) {
        if self.is_thinking {
            self.is_thinking = false;
            self.reasoning_finished_at = Some(now);
            on_progress(ToolLoopProgress::Thinking { is_thinking: false });
        }
    }

    pub(super) fn observe_tool_result_input(
        &mut self,
        content: &str,
        now: Instant,
        on_progress: &mut impl FnMut(ToolLoopProgress),
    ) {
        let Some(total_tokens) =
            observe_complete_token_total(&mut self.input_progress, content, now)
        else {
            return;
        };

        on_progress(ToolLoopProgress::InputTokens { total_tokens });
    }

    pub(super) fn observe_tool_activity_output(
        &mut self,
        activity_id: &str,
        content: Option<&str>,
        suppress_counted_arguments: bool,
        now: Instant,
        on_progress: &mut impl FnMut(ToolLoopProgress),
    ) {
        let Some(content) = content else {
            return;
        };
        if suppress_counted_arguments
            && self
                .tool_call_argument_output_by_id
                .contains_key(activity_id)
        {
            return;
        }
        let token_text = self.visible_tool_output_delta(activity_id, content);
        let token_text = token_text.as_deref().unwrap_or(content);
        let Some(total_tokens) =
            observe_complete_token_total(&mut self.output_progress, token_text, now)
        else {
            return;
        };

        on_progress(ToolLoopProgress::OutputTokens { total_tokens });
    }

    pub(super) fn observe_terminal_snapshot_output(
        &mut self,
        snapshot: &RuntimeTerminalSnapshot,
        now: Instant,
        on_progress: &mut impl FnMut(ToolLoopProgress),
    ) {
        let token_text = self.visible_tool_output_delta(&snapshot.terminal_id, &snapshot.output);
        self.terminal_output_by_id
            .insert(snapshot.terminal_id.clone(), snapshot.output.clone());
        let token_text = token_text.as_deref().unwrap_or(&snapshot.output);
        let Some(total_tokens) =
            observe_complete_token_total(&mut self.output_progress, token_text, now)
        else {
            return;
        };

        on_progress(ToolLoopProgress::OutputTokens { total_tokens });
    }

    fn visible_tool_output_delta(&self, activity_id: &str, content: &str) -> Option<String> {
        self.terminal_output_by_id
            .get(activity_id)
            .map(|previous| terminal_output_delta(previous, content).to_string())
    }

    pub(super) fn finish_at(
        mut self,
        finished_at: Instant,
        appended_items: Vec<ConversationItem>,
    ) -> ToolLoopCompletion {
        if self.is_thinking {
            self.is_thinking = false;
        }
        let _ = self.output_progress.flush(finished_at);
        let _ = self.llm_output_progress.flush(finished_at);
        let metrics = self.performance_metrics();
        let reasoning_duration = self.reasoning_duration();
        ToolLoopCompletion {
            response: ToolLoopResponse::new(appended_items, reasoning_duration),
            metrics,
        }
    }

    fn performance_metrics(&self) -> Option<ProviderRequestMetrics> {
        let request_started_at = self.request_started_at?;
        let first_token_at = self.first_token_at?;
        Some(ProviderRequestMetrics {
            latency: first_token_at.saturating_duration_since(request_started_at),
            output_tokens: self.llm_output_tokens_total,
            duration: self.llm_generation_duration,
        })
    }

    fn reasoning_duration(&self) -> Option<Duration> {
        if self.reasoning_content.trim().is_empty() {
            return None;
        }

        let started_at = self.reasoning_started_at?;
        let finished_at = self.reasoning_finished_at.unwrap_or(started_at);
        Some(finished_at.saturating_duration_since(started_at))
    }
}

fn observe_complete_token_total(
    progress: &mut StreamingTokenProgress,
    content: &str,
    now: Instant,
) -> Option<usize> {
    if content.is_empty() {
        return None;
    }

    progress
        .observe_delta(content, now)
        .or_else(|| progress.flush(now))
}

pub(super) fn runtime_tool_activity_update_token_text(
    update: &RuntimeToolActivityUpdate,
) -> Option<String> {
    if let Some(content) = update.content.as_ref() {
        let text = content
            .iter()
            .filter_map(runtime_tool_activity_content_token_text)
            .collect::<Vec<_>>()
            .join("\n");
        if !text.is_empty() {
            return Some(text);
        }
    }

    update.raw_output.as_ref().and_then(|raw| raw.token_text())
}

pub(super) fn runtime_tool_activity_update_duplicates_tool_arguments(
    update: &RuntimeToolActivityUpdate,
) -> bool {
    update.content.as_ref().is_some_and(|content| {
        content
            .iter()
            .any(|content| matches!(content, RuntimeToolActivityContent::Diff { .. }))
    })
}

fn runtime_tool_activity_content_token_text(
    content: &RuntimeToolActivityContent,
) -> Option<String> {
    match content {
        RuntimeToolActivityContent::Text(text) | RuntimeToolActivityContent::Unknown(text) => {
            non_empty_token_text(text)
        }
        RuntimeToolActivityContent::Image { mime_type, uri } => {
            let text = uri
                .as_deref()
                .map(|uri| format!("image {mime_type} {uri}"))
                .unwrap_or_else(|| format!("image {mime_type}"));
            non_empty_token_text(&text)
        }
        RuntimeToolActivityContent::Audio { mime_type } => {
            non_empty_token_text(&format!("audio {mime_type}"))
        }
        RuntimeToolActivityContent::ResourceLink { uri, name, title } => {
            let mut text = format!("{name} {uri}");
            if let Some(title) = title.as_deref() {
                text.push('\n');
                text.push_str(title);
            }
            non_empty_token_text(&text)
        }
        RuntimeToolActivityContent::Resource {
            uri,
            mime_type,
            text,
        } => {
            let mut parts = vec![uri.clone()];
            if let Some(mime_type) = mime_type.as_deref() {
                parts.push(mime_type.to_string());
            }
            if let Some(text) = text.as_deref() {
                parts.push(text.to_string());
            }
            non_empty_token_text(&parts.join("\n"))
        }
        RuntimeToolActivityContent::Diff {
            path,
            old_text,
            new_text,
            ..
        } => {
            let mut parts = vec![path.clone()];
            if let Some(old_text) = old_text.as_deref() {
                parts.push(old_text.to_string());
            }
            parts.push(new_text.clone());
            non_empty_token_text(&parts.join("\n"))
        }
        RuntimeToolActivityContent::Terminal { .. } => None,
    }
}

fn non_empty_token_text(text: &str) -> Option<String> {
    (!text.is_empty()).then(|| text.to_string())
}

fn terminal_output_delta<'a>(previous: &str, current: &'a str) -> &'a str {
    if previous.is_empty() {
        return current;
    }
    if let Some(delta) = current.strip_prefix(previous) {
        return delta;
    }
    if previous.ends_with(current) {
        return "";
    }

    let mut overlap_len = 0usize;
    for (index, _) in current.char_indices().skip(1) {
        if previous.ends_with(&current[..index]) {
            overlap_len = index;
        }
    }

    &current[overlap_len..]
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use provider_protocol::{ConversationItem, FinishReason, PromptCompletion, ToolCall};

    use super::RuntimeTurnState;

    #[test]
    fn completed_response_tool_calls_use_flat_tool_call_indices() {
        let mut state = RuntimeTurnState::new("qwen3".to_string());
        let completion = PromptCompletion::new(
            vec![ConversationItem::assistant_with_tool_calls(
                String::new(),
                vec![
                    ToolCall::new("call-1", "read", r#"{"path":"one"}"#),
                    ToolCall::new("call-2", "read", r#"{"path":"two"}"#),
                ],
            )],
            FinishReason::ToolCalls,
            None,
        );
        let mut events = Vec::new();

        state.observe_response_tool_calls_completed(&completion, Instant::now(), &mut |event| {
            events.push(event)
        });

        assert_eq!(
            state.tool_call_ids_by_index.get(&0).map(String::as_str),
            Some("call-1")
        );
        assert_eq!(
            state.tool_call_ids_by_index.get(&1).map(String::as_str),
            Some("call-2")
        );
        assert_eq!(
            state
                .tool_call_argument_output_by_index
                .get(&0)
                .map(String::as_str),
            Some(r#"{"path":"one"}"#)
        );
        assert_eq!(
            state
                .tool_call_argument_output_by_index
                .get(&1)
                .map(String::as_str),
            Some(r#"{"path":"two"}"#)
        );
    }
}
