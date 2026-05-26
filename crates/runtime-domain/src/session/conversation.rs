use crate::provider::{ProviderApiKey, ProviderKind};

use std::time::Duration;

use super::{
    RuntimePermissionRequest, RuntimeTarget, RuntimeTerminalSnapshot, RuntimeToolActivity,
    RuntimeToolActivityUpdate,
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
        messages: Vec<ChatMessage>,
    ) -> Self {
        Self {
            provider_request: ProviderRequest::new(
                provider_id,
                provider_kind,
                model_id,
                base_url,
                api_key,
                api_key_env,
                messages,
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
    message: ChatMessage,
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
        message: ChatMessage,
    ) -> Self {
        Self {
            provider_id: provider_id.into(),
            provider_kind,
            model_id: model_id.into(),
            base_url,
            api_key,
            api_key_env,
            message,
        }
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
    pub fn message(&self) -> &ChatMessage {
        &self.message
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
    pub messages: Vec<ChatMessage>,
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
        messages: Vec<ChatMessage>,
    ) -> Self {
        Self {
            provider_id: provider_id.into(),
            provider_kind,
            model_id: model_id.into(),
            base_url,
            api_key,
            api_key_env,
            messages,
        }
    }
}

/// `ChatMessageBlock` 描述一条用户消息中的结构化输入块。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChatMessageBlock {
    Text(String),
    Image {
        data_base64: String,
        mime_type: String,
        uri: Option<String>,
    },
    Audio {
        data_base64: String,
        mime_type: String,
        uri: Option<String>,
    },
    Document {
        data_base64: String,
        mime_type: String,
        filename: Option<String>,
        uri: Option<String>,
    },
}

/// `ChatMessage` 是 transcript 到 provider 请求之间的稳定消息形状。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatMessage {
    pub role: ChatRole,
    pub content: String,
    pub blocks: Option<Vec<ChatMessageBlock>>,
}

impl ChatMessage {
    /// `user` 创建用户消息。
    pub fn user(content: String) -> Self {
        Self::user_with_blocks(content, None)
    }

    /// `user_with_blocks` 创建带结构化内容块的用户消息。
    pub fn user_with_blocks(content: String, blocks: Option<Vec<ChatMessageBlock>>) -> Self {
        Self {
            role: ChatRole::User,
            content,
            blocks,
        }
    }

    /// `assistant` 创建助手消息。
    pub fn assistant(content: String) -> Self {
        Self {
            role: ChatRole::Assistant,
            content,
            blocks: None,
        }
    }
}

/// `ChatRole` 描述 Lumos 当前会发送给上游的 transcript role。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatRole {
    User,
    Assistant,
}

impl ChatRole {
    /// `as_str` 返回上游协议常用的 role 名称。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Assistant => "assistant",
        }
    }
}

/// `ConversationResponse` 保存单轮对话输出。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConversationResponse {
    pub content: String,
    pub reasoning_content: Option<String>,
    pub reasoning_duration: Option<Duration>,
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
