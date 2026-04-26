use serde::{Deserialize, Serialize};

/// `NativeChatRequest` 是 TUI 向原生 OpenAI-compatible backend 发起的一次请求。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeChatRequest {
    pub provider_id: String,
    pub model_id: String,
    pub base_url: String,
    pub api_key_env: Option<String>,
    pub messages: Vec<ChatCompletionMessage>,
}

impl NativeChatRequest {
    /// `new` 创建一次 `/v1/chat/completions` 请求。
    pub fn new(
        provider_id: impl Into<String>,
        model_id: impl Into<String>,
        base_url: impl Into<String>,
        api_key_env: Option<String>,
        messages: Vec<ChatCompletionMessage>,
    ) -> Self {
        Self {
            provider_id: provider_id.into(),
            model_id: model_id.into(),
            base_url: base_url.into(),
            api_key_env,
            messages,
        }
    }
}

/// `ChatCompletionMessage` 表示 OpenAI-compatible chat completions 消息。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatCompletionMessage {
    pub role: String,
    pub content: String,
}

impl ChatCompletionMessage {
    /// `new` 创建指定 role 的消息。
    pub fn new(role: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: role.into(),
            content: content.into(),
        }
    }

    /// `user` 创建用户消息。
    pub fn user(content: String) -> Self {
        Self::new("user", content)
    }

    /// `assistant` 创建助手消息。
    pub fn assistant(content: String) -> Self {
        Self::new("assistant", content)
    }
}

/// `ChatCompletionRequestBody` 是 `/chat/completions` 的最小请求体。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ChatCompletionRequestBody {
    model: String,
    messages: Vec<ChatCompletionMessage>,
    stream: bool,
}

impl ChatCompletionRequestBody {
    /// `new` 创建启用流式返回的 chat completions 请求体。
    pub fn new(model: impl Into<String>, messages: Vec<ChatCompletionMessage>) -> Self {
        Self {
            model: model.into(),
            messages,
            stream: true,
        }
    }
}
