use crate::provider::{ProviderApiKey, ProviderKind};

use std::time::Duration;

use super::{
    RuntimePermissionRequest, RuntimeTarget, RuntimeToolActivity, RuntimeToolActivityUpdate,
};

/// `NativeAgentRequest` 描述一次内置 native agent turn。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeAgentRequest {
    llm_request: NativeLlmRequest,
}

impl NativeAgentRequest {
    /// `new` 创建一个还未附加工具的 native agent 请求。
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
            llm_request: NativeLlmRequest::new(
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
        RuntimeTarget::native_agent(
            self.llm_request.provider_id.clone(),
            self.llm_request.model_id.clone(),
        )
    }

    /// `llm_request` 返回底层模型请求参数。
    pub fn llm_request(&self) -> &NativeLlmRequest {
        &self.llm_request
    }
}

/// `NativeLlmRequest` 保存 native agent 调用 LLM backend 所需的模型参数。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeLlmRequest {
    pub provider_id: String,
    pub provider_kind: ProviderKind,
    pub model_id: String,
    pub base_url: Option<String>,
    pub api_key: Option<ProviderApiKey>,
    pub api_key_env: Option<String>,
    pub messages: Vec<ChatMessage>,
}

impl NativeLlmRequest {
    /// `new` 创建一次原生 LLM backend 请求参数。
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

/// `ChatMessage` 是 Lumos transcript 到 native LLM 请求之间的稳定消息形状。
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

/// `NativeAgentResponse` 保存 native agent 单轮输出。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NativeAgentResponse {
    pub content: String,
    pub reasoning_content: Option<String>,
    pub reasoning_duration: Option<Duration>,
}

/// `NativeLlmPerformanceMetrics` 记录一次成功请求的输出性能指标。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NativeLlmPerformanceMetrics {
    pub latency: Duration,
    pub output_tokens: usize,
    pub duration: Duration,
}

/// `NativeAgentEvent` 是 native agent worker 暴露给消费层的事件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NativeAgentEvent {
    Retrying {
        message: String,
    },
    OutputTokenEstimate {
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
    PermissionRequested {
        request: RuntimePermissionRequest,
    },
    Finished {
        response: NativeAgentResponse,
        metrics: Option<NativeLlmPerformanceMetrics>,
    },
    Failed {
        message: String,
    },
    Interrupted,
}

impl NativeAgentEvent {
    /// `is_terminal` 判断事件是否结束当前 native turn。
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Finished { .. } | Self::Failed { .. } | Self::Interrupted
        )
    }
}
