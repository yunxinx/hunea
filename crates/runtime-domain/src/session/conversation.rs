use provider_protocol::{ContentBlock, ConversationItem, Role};

use std::{fmt, time::Duration};

use super::{
    RuntimePermissionRequest, RuntimeTarget, RuntimeTerminalSnapshot, RuntimeToolActivity,
    RuntimeToolActivityUpdate, TranscriptUserMessage,
};

/// `ConversationRequest` 描述一次完整的对话执行请求。
#[derive(Clone, PartialEq, Eq)]
pub struct ConversationRequest {
    provider_request: ProviderRequest,
}

impl fmt::Debug for ConversationRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ConversationRequest")
            .field("target", &self.target())
            .field("item_count", &self.provider_request.items.len())
            .finish()
    }
}

impl ConversationRequest {
    /// `new` 创建一个还未附加工具的对话请求。
    pub fn new(
        provider_id: impl Into<String>,
        model_id: impl Into<String>,
        items: Vec<ConversationItem>,
    ) -> Self {
        Self {
            provider_request: ProviderRequest::new(provider_id, model_id, items),
        }
    }

    /// `target` 返回该请求对应的统一 runtime 目标。
    pub fn target(&self) -> RuntimeTarget {
        RuntimeTarget::provider(
            self.provider_request.provider_id.clone(),
            self.provider_request.model_id.clone(),
        )
    }

    /// `provider_request` 返回底层 provider 请求参数。
    pub fn provider_request(&self) -> &ProviderRequest {
        &self.provider_request
    }
}

/// `ConversationTurnRequest` 描述 TUI 向 provider-visible 对话提交的一次用户 turn。
#[derive(Clone, PartialEq, Eq)]
pub struct ConversationTurnRequest {
    provider_id: String,
    model_id: String,
    message: ConversationItem,
    transcript_user_message: Option<TranscriptUserMessage>,
}

impl fmt::Debug for ConversationTurnRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ConversationTurnRequest")
            .field("target", &self.target())
            .field("message_role", &self.message.role())
            .field(
                "has_transcript_user_message",
                &self.transcript_user_message.is_some(),
            )
            .finish()
    }
}

impl ConversationTurnRequest {
    /// `new` 创建一次对话轮次提交请求。
    pub fn new(
        provider_id: impl Into<String>,
        model_id: impl Into<String>,
        message: ConversationItem,
    ) -> Self {
        Self {
            provider_id: provider_id.into(),
            model_id: model_id.into(),
            message,
            transcript_user_message: None,
        }
    }

    /// `new_user_text` 从 UI 原始用户输入创建一次对话轮次提交请求。
    pub fn new_user_text(
        provider_id: impl Into<String>,
        model_id: impl Into<String>,
        text: impl Into<String>,
    ) -> Self {
        let text = text.into();
        let content = if text.is_empty() {
            Vec::new()
        } else {
            vec![ContentBlock::Text(text)]
        };
        Self::new_user_content(provider_id, model_id, content)
    }

    /// `new_user_content` 从结构化用户内容创建一次对话轮次提交请求。
    pub fn new_user_content(
        provider_id: impl Into<String>,
        model_id: impl Into<String>,
        content: Vec<ContentBlock>,
    ) -> Self {
        Self::new(provider_id, model_id, ConversationItem::user(content))
    }

    /// `new_user_source_message` 从 transcript-visible 用户消息创建一次对话轮次提交请求。
    pub fn new_user_source_message(
        provider_id: impl Into<String>,
        model_id: impl Into<String>,
        message: TranscriptUserMessage,
    ) -> Self {
        let mut request = Self::new(provider_id, model_id, message.provider_message());
        request.transcript_user_message = Some(message);
        request
    }

    /// `target` 返回该 turn 对应的统一 runtime 目标。
    pub fn target(&self) -> RuntimeTarget {
        RuntimeTarget::provider(self.provider_id.clone(), self.model_id.clone())
    }

    /// `provider_id` 返回当前 provider 标识。
    pub fn provider_id(&self) -> &str {
        &self.provider_id
    }

    /// `model_id` 返回当前模型标识。
    pub fn model_id(&self) -> &str {
        &self.model_id
    }

    /// `message` 返回本轮提交的用户消息。
    pub fn message(&self) -> &ConversationItem {
        &self.message
    }

    /// `is_user_message` 返回本轮消息是否为用户输入。
    pub fn is_user_message(&self) -> bool {
        self.message.role() == Some(Role::User)
    }

    /// `message_text` 返回本轮消息中的可见文本。
    pub fn message_text(&self) -> String {
        self.message.text_content()
    }

    /// `transcript_user_message` 返回 transcript-visible 用户消息。
    pub fn transcript_user_message(&self) -> Option<&TranscriptUserMessage> {
        self.transcript_user_message.as_ref()
    }
}

/// `ProviderRequest` 保存向上游 provider 发起请求所需的模型参数。
#[derive(Clone, PartialEq, Eq)]
pub struct ProviderRequest {
    pub provider_id: String,
    pub model_id: String,
    pub items: Vec<ConversationItem>,
}

impl fmt::Debug for ProviderRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProviderRequest")
            .field("provider_id", &self.provider_id)
            .field("model_id", &self.model_id)
            .field("item_count", &self.items.len())
            .finish()
    }
}

impl ProviderRequest {
    /// `new` 创建一次 provider backend 请求参数。
    pub fn new(
        provider_id: impl Into<String>,
        model_id: impl Into<String>,
        items: Vec<ConversationItem>,
    ) -> Self {
        Self {
            provider_id: provider_id.into(),
            model_id: model_id.into(),
            items,
        }
    }
}

#[cfg(test)]
mod tests {
    use provider_protocol::ContentBlock;

    use crate::session::TranscriptUserAttachment;

    use super::{ConversationTurnRequest, TranscriptUserMessage};

    #[test]
    fn user_source_message_builds_structured_provider_content() {
        let request = ConversationTurnRequest::new_user_source_message(
            "openai",
            "gpt-4o",
            TranscriptUserMessage {
                content: "inspect this".to_string(),
                attachments: vec![TranscriptUserAttachment::Image {
                    data_base64: "iVBORw==".to_string(),
                    mime_type: "image/png".to_string(),
                    uri: Some("assets/a.png".to_string()),
                    detail: None,
                }],
                skill_bindings: Vec::new(),
                custom_prompt_bindings: Vec::new(),
            },
        );

        let provider_protocol::ConversationItem::Message { content, .. } = request.message() else {
            panic!("turn request should carry a user message");
        };

        assert!(matches!(
            &content[0],
            ContentBlock::Text(text) if text == "inspect this"
        ));
        assert!(matches!(
            &content[1],
            ContentBlock::Image { data_base64, mime_type, uri, detail }
                if data_base64 == "iVBORw=="
                    && mime_type == "image/png"
                    && uri.as_deref() == Some("assets/a.png")
                    && detail.is_none()
        ));
    }

    #[test]
    fn request_debug_does_not_include_provider_visible_content() {
        let sentinel = "delivery-sentinel-that-must-not-leak";
        let turn = ConversationTurnRequest::new_user_text("openai", "gpt-4o", sentinel);
        let request = super::ConversationRequest::new(
            "openai",
            "gpt-4o",
            vec![provider_protocol::ConversationItem::text(
                provider_protocol::Role::User,
                sentinel,
            )],
        );

        assert!(!format!("{turn:?}").contains(sentinel));
        assert!(!format!("{request:?}").contains(sentinel));
        assert!(!format!("{:?}", request.provider_request()).contains(sentinel));
    }
}

/// `ConversationResponse` 保存单轮对话输出的完整 provider-visible items。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConversationResponse {
    pub items: Vec<ConversationItem>,
    pub reasoning_duration: Option<Duration>,
}

impl ConversationResponse {
    /// `new` 从完整 provider-visible items 创建对话响应。
    pub fn new(items: Vec<ConversationItem>, reasoning_duration: Option<Duration>) -> Self {
        Self {
            items,
            reasoning_duration,
        }
    }

    /// `assistant_text` 创建仅包含最终 assistant 文本的响应。
    pub fn assistant_text(content: impl Into<String>) -> Self {
        let content = content.into();
        let items = if content.is_empty() {
            Vec::new()
        } else {
            vec![ConversationItem::text(Role::Assistant, content)]
        };
        Self::new(items, None)
    }

    /// `with_reasoning` 创建带 reasoning 与最终 assistant 文本的响应。
    pub fn with_reasoning(
        content: impl Into<String>,
        reasoning_content: impl Into<String>,
        reasoning_duration: Option<Duration>,
    ) -> Self {
        let content = content.into();
        let reasoning_content = reasoning_content.into();
        let mut items = Vec::new();
        if !reasoning_content.trim().is_empty() {
            items.push(ConversationItem::Reasoning {
                content: reasoning_content,
                summary: None,
                encrypted: None,
            });
        }
        if !content.is_empty() {
            items.push(ConversationItem::text(Role::Assistant, content));
        }
        Self::new(items, reasoning_duration)
    }

    /// `text_content` 返回最终 assistant 消息的可见文本。
    pub fn text_content(&self) -> String {
        self.items
            .iter()
            .rev()
            .find(|item| item.role() == Some(Role::Assistant))
            .map(ConversationItem::text_content)
            .unwrap_or_default()
            .trim_end()
            .to_string()
    }

    /// `reasoning_content` 返回所有 reasoning item 的拼接内容。
    pub fn reasoning_content(&self) -> Option<String> {
        let content = self
            .items
            .iter()
            .filter_map(|item| match item {
                ConversationItem::Reasoning { content, .. } => Some(content.as_str()),
                _ => None,
            })
            .collect::<String>();
        let content = trim_outer_blank_lines(&content);
        (!content.is_empty()).then_some(content)
    }
}

fn trim_outer_blank_lines(content: &str) -> String {
    let lines = content.lines().collect::<Vec<_>>();
    let Some(start) = lines.iter().position(|line| !line.trim().is_empty()) else {
        return String::new();
    };
    let Some(end) = lines.iter().rposition(|line| !line.trim().is_empty()) else {
        return String::new();
    };

    lines[start..=end].join("\n")
}

/// `ProviderRequestMetrics` 记录一次成功请求中的 LLM 输出性能指标。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProviderRequestMetrics {
    pub latency: Duration,
    pub output_tokens: usize,
    pub duration: Duration,
}

/// `ConversationEvent` 是对话 worker 暴露给消费层的事件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConversationEvent {
    SystemMessage {
        message: String,
    },
    Retrying {
        message: String,
    },
    OutputTokenEstimate {
        total_tokens: usize,
    },
    InputTokenEstimate {
        total_tokens: usize,
    },
    Thinking {
        is_thinking: bool,
    },
    AssistantDelta {
        content: String,
    },
    ReasoningDelta {
        content: String,
    },
    ToolActivityStarted {
        activity: RuntimeToolActivity,
    },
    ToolActivityUpdated {
        update: RuntimeToolActivityUpdate,
    },
    TerminalUpdated {
        snapshot: RuntimeTerminalSnapshot,
    },
    PermissionRequested {
        request: RuntimePermissionRequest,
    },
    Finished {
        response: ConversationResponse,
        metrics: Option<ProviderRequestMetrics>,
    },
    Failed {
        message: String,
    },
    Interrupted,
}

impl ConversationEvent {
    /// `is_terminal` 判断事件是否结束当前对话轮次。
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Finished { .. } | Self::Failed { .. } | Self::Interrupted
        )
    }
}
