use crate::provider::{ProviderApiKey, ProviderKind};

use provider_protocol::{ConversationItem, Role};

use std::time::Duration;

use super::{
    RuntimePermissionRequest, RuntimeTarget, RuntimeTerminalSnapshot, RuntimeToolActivity,
    RuntimeToolActivityUpdate, TranscriptUserMessage,
};

/// `ConversationRequest` 描述一次完整的对话执行请求。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversationRequest {
    provider_request: ProviderRequest,
}

impl ConversationRequest {
    /// `new` 创建一个还未附加工具的对话请求。
    pub fn new(
        provider_id: impl Into<String>,
        provider_kind: ProviderKind,
        model_id: impl Into<String>,
        base_url: Option<String>,
        api_key: Option<ProviderApiKey>,
        api_key_env: Option<String>,
        items: Vec<ConversationItem>,
    ) -> Self {
        Self {
            provider_request: ProviderRequest::new(
                provider_id,
                provider_kind,
                model_id,
                base_url,
                api_key,
                api_key_env,
                items,
            ),
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversationTurnRequest {
    provider_id: String,
    provider_kind: ProviderKind,
    model_id: String,
    base_url: Option<String>,
    api_key: Option<ProviderApiKey>,
    api_key_env: Option<String>,
    message: ConversationItem,
    transcript_user_message: Option<TranscriptUserMessage>,
}

impl ConversationTurnRequest {
    /// `new` 创建一次对话轮次提交请求。
    pub fn new(
        provider_id: impl Into<String>,
        provider_kind: ProviderKind,
        model_id: impl Into<String>,
        base_url: Option<String>,
        api_key: Option<ProviderApiKey>,
        api_key_env: Option<String>,
        message: ConversationItem,
    ) -> Self {
        Self {
            provider_id: provider_id.into(),
            provider_kind,
            model_id: model_id.into(),
            base_url,
            api_key,
            api_key_env,
            message,
            transcript_user_message: None,
        }
    }

    /// `new_user_text` 从 UI 原始用户输入创建一次对话轮次提交请求。
    pub fn new_user_text(
        provider_id: impl Into<String>,
        provider_kind: ProviderKind,
        model_id: impl Into<String>,
        base_url: Option<String>,
        api_key: Option<ProviderApiKey>,
        api_key_env: Option<String>,
        text: impl Into<String>,
    ) -> Self {
        Self::new(
            provider_id,
            provider_kind,
            model_id,
            base_url,
            api_key,
            api_key_env,
            ConversationItem::text(Role::User, text),
        )
    }

    /// `new_user_source_message` 从 transcript-visible 用户消息创建一次对话轮次提交请求。
    pub fn new_user_source_message(
        provider_id: impl Into<String>,
        provider_kind: ProviderKind,
        model_id: impl Into<String>,
        base_url: Option<String>,
        api_key: Option<ProviderApiKey>,
        api_key_env: Option<String>,
        message: TranscriptUserMessage,
    ) -> Self {
        let mut request = Self::new_user_text(
            provider_id,
            provider_kind,
            model_id,
            base_url,
            api_key,
            api_key_env,
            message.content.clone(),
        );
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

    /// `provider_kind` 返回 provider 类型。
    pub const fn provider_kind(&self) -> ProviderKind {
        self.provider_kind
    }

    /// `model_id` 返回当前模型标识。
    pub fn model_id(&self) -> &str {
        &self.model_id
    }

    /// `base_url` 返回当前 provider base_url。
    pub fn base_url(&self) -> Option<&str> {
        self.base_url.as_deref()
    }

    /// `api_key` 返回直接配置的 API key。
    pub fn api_key(&self) -> Option<&ProviderApiKey> {
        self.api_key.as_ref()
    }

    /// `api_key_env` 返回 API key 环境变量名。
    pub fn api_key_env(&self) -> Option<&str> {
        self.api_key_env.as_deref()
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderRequest {
    pub provider_id: String,
    pub provider_kind: ProviderKind,
    pub model_id: String,
    pub base_url: Option<String>,
    pub api_key: Option<ProviderApiKey>,
    pub api_key_env: Option<String>,
    pub items: Vec<ConversationItem>,
}

impl ProviderRequest {
    /// `new` 创建一次 provider backend 请求参数。
    pub fn new(
        provider_id: impl Into<String>,
        provider_kind: ProviderKind,
        model_id: impl Into<String>,
        base_url: Option<String>,
        api_key: Option<ProviderApiKey>,
        api_key_env: Option<String>,
        items: Vec<ConversationItem>,
    ) -> Self {
        Self {
            provider_id: provider_id.into(),
            provider_kind,
            model_id: model_id.into(),
            base_url,
            api_key,
            api_key_env,
            items,
        }
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
    let end = lines
        .iter()
        .rposition(|line| !line.trim().is_empty())
        .expect("start exists when at least one non-blank line exists");

    lines[start..=end].join("\n")
}

/// `ProviderRequestMetrics` 记录一次成功请求中的 LLM 输出性能指标。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProviderRequestMetrics {
    pub latency: Duration,
    pub output_tokens: usize,
    pub duration: Duration,
}

/// `ManagedSearchTool` 标识可由 app 层持久化授权的受管搜索工具。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManagedSearchTool {
    Ripgrep,
    Fd,
}

impl ManagedSearchTool {
    /// `from_binary_name` 从外部工具二进制名解析受管搜索工具。
    pub fn from_binary_name(name: &str) -> Option<Self> {
        match name {
            "rg" => Some(Self::Ripgrep),
            "fd" => Some(Self::Fd),
            _ => None,
        }
    }

    /// `binary_name` 返回工具的标准二进制名。
    pub const fn binary_name(self) -> &'static str {
        match self {
            Self::Ripgrep => "rg",
            Self::Fd => "fd",
        }
    }
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
    ManagedSearchToolAuthorization {
        tool: ManagedSearchTool,
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
