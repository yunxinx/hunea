use genai::chat::{ChatMessage as GenAiChatMessage, ChatRole as GenAiChatRole};

use super::ProviderKind;

/// `ProviderApiKey` 保存配置文件中直接写入的 provider API key。
#[derive(Clone, PartialEq, Eq)]
pub struct ProviderApiKey(String);

impl ProviderApiKey {
    /// `new` 创建一个直接可用的 API key。
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub(crate) fn from_optional_config(value: Option<String>) -> Option<Self> {
        value
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .map(Self)
    }

    /// `as_str` 返回 API key 原文。
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for ProviderApiKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ProviderApiKey(REDACTED)")
    }
}

/// `NativeChatRequest` 是 TUI 向原生 LLM backend 发起的一次对话请求。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeChatRequest {
    pub provider_id: String,
    pub provider_kind: ProviderKind,
    pub model_id: String,
    pub base_url: Option<String>,
    pub api_key: Option<ProviderApiKey>,
    pub api_key_env: Option<String>,
    pub messages: Vec<ChatMessage>,
}

impl NativeChatRequest {
    /// `new` 创建一次原生 LLM 请求。
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

/// `ChatMessage` 是 Lumos transcript 到 LLM 请求之间的稳定消息形状。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatMessage {
    pub role: ChatRole,
    pub content: String,
}

impl ChatMessage {
    /// `user` 创建用户消息。
    pub fn user(content: String) -> Self {
        Self {
            role: ChatRole::User,
            content,
        }
    }

    /// `assistant` 创建助手消息。
    pub fn assistant(content: String) -> Self {
        Self {
            role: ChatRole::Assistant,
            content,
        }
    }

    pub(crate) fn into_genai(self) -> GenAiChatMessage {
        match self.role {
            ChatRole::User => GenAiChatMessage::user(self.content),
            ChatRole::Assistant => GenAiChatMessage::assistant(self.content),
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

impl From<ChatRole> for GenAiChatRole {
    fn from(role: ChatRole) -> Self {
        match role {
            ChatRole::User => Self::User,
            ChatRole::Assistant => Self::Assistant,
        }
    }
}
