use std::time::{Duration, Instant};

/// `RuntimeResponseBuffer` 暂存 runtime 的流式文本，直到工具调用等语义边界出现。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct RuntimeResponseBuffer {
    content: String,
    pub(super) reasoning_content: String,
    reasoning_started_at: Option<Instant>,
}

impl RuntimeResponseBuffer {
    pub(super) fn is_empty(&self) -> bool {
        self.content.is_empty() && self.reasoning_content.is_empty()
    }

    pub(super) fn has_reasoning_content(&self) -> bool {
        !self.reasoning_content.is_empty()
    }

    pub(super) fn push_content(&mut self, content: &str) {
        self.content.push_str(content);
    }

    pub(super) fn push_reasoning_content(&mut self, content: &str) {
        if content.is_empty() {
            return;
        }
        self.reasoning_started_at.get_or_insert_with(Instant::now);
        self.reasoning_content.push_str(content);
    }

    pub(super) fn clear(&mut self) {
        self.content.clear();
        self.reasoning_content.clear();
        self.reasoning_started_at = None;
    }

    pub(super) fn take(&mut self) -> Option<BufferedRuntimeResponse> {
        let content = std::mem::take(&mut self.content);
        let reasoning_content = if self.reasoning_content.is_empty() {
            self.reasoning_started_at = None;
            None
        } else {
            Some(std::mem::take(&mut self.reasoning_content))
        };
        let reasoning_duration = reasoning_content
            .as_ref()
            .and_then(|_| self.reasoning_started_at.take())
            .map(|started_at| Instant::now().saturating_duration_since(started_at));

        if content.is_empty() && reasoning_content.is_none() {
            return None;
        }

        Some(BufferedRuntimeResponse {
            content,
            reasoning_content,
            reasoning_duration,
        })
    }

    pub(super) fn take_reasoning_for_expanded_display(
        &mut self,
    ) -> Option<BufferedReasoningFragment> {
        if self.reasoning_content.is_empty() {
            self.reasoning_started_at = None;
            return None;
        }

        let content = std::mem::take(&mut self.reasoning_content);
        let duration = self
            .reasoning_started_at
            .take()
            .map(|started_at| Instant::now().saturating_duration_since(started_at));

        Some(BufferedReasoningFragment { content, duration })
    }

    pub(super) fn take_with_final(
        &mut self,
        final_content: String,
        final_reasoning_content: Option<String>,
        final_reasoning_duration: Option<Duration>,
    ) -> Option<BufferedRuntimeResponse> {
        let final_reasoning_content =
            final_reasoning_content.filter(|content| !content.trim().is_empty());
        let mut response = self.take().unwrap_or_default();

        response.content = reconcile_buffered_text_with_final(response.content, final_content);
        let (reasoning_content, reasoning_duration) = reconcile_buffered_reasoning_with_final(
            response.reasoning_content,
            response.reasoning_duration,
            final_reasoning_content,
            final_reasoning_duration,
        );
        response.reasoning_content = reasoning_content;
        response.reasoning_duration = reasoning_duration;

        if response.content.is_empty() && response.reasoning_content.is_none() {
            return None;
        }

        Some(response)
    }
}

fn reconcile_buffered_text_with_final(buffered: String, final_content: String) -> String {
    if buffered.is_empty() {
        return final_content;
    }
    if final_content.is_empty() {
        return buffered;
    }
    if final_content.starts_with(&buffered) {
        return final_content;
    }

    buffered
}

fn reconcile_buffered_reasoning_with_final(
    buffered: Option<String>,
    buffered_duration: Option<Duration>,
    final_content: Option<String>,
    final_duration: Option<Duration>,
) -> (Option<String>, Option<Duration>) {
    match final_content {
        Some(content) => (Some(content), final_duration.or(buffered_duration)),
        None => (buffered, buffered_duration),
    }
}

pub(super) fn strip_displayed_reasoning_prefix(
    final_content: Option<String>,
    displayed_content: &str,
) -> Option<String> {
    let final_content = final_content?;
    if displayed_content.is_empty() {
        return Some(final_content);
    }

    final_content
        .strip_prefix(displayed_content)
        .and_then(|tail| (!tail.is_empty()).then(|| tail.to_string()))
}

#[derive(Default)]
pub(super) struct BufferedRuntimeResponse {
    pub(super) content: String,
    pub(super) reasoning_content: Option<String>,
    pub(super) reasoning_duration: Option<Duration>,
}

pub(super) struct BufferedReasoningFragment {
    pub(super) content: String,
    pub(super) duration: Option<Duration>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct StreamedRuntimeReasoning {
    pub(super) item_indices: Vec<usize>,
    pub(super) displayed_content: String,
}
