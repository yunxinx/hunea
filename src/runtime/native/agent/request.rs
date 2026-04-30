use crate::runtime::{
    native::{ChatMessage, NativeChatRequest, ProviderApiKey, ProviderKind},
    session::RuntimeTarget,
    tools::RuntimeToolRegistry,
};

/// `NativeAgentRequest` 描述一次内置 native agent turn。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeAgentRequest {
    chat_request: NativeChatRequest,
    tools: RuntimeToolRegistry,
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
            chat_request: NativeChatRequest::new(
                provider_id,
                provider_kind,
                model_id,
                base_url,
                api_key,
                api_key_env,
                messages,
            ),
            tools: RuntimeToolRegistry::new(),
        }
    }

    /// `from_chat_request` 从现有 native chat 请求提升为 native agent 请求。
    pub fn from_chat_request(chat_request: NativeChatRequest) -> Self {
        Self {
            chat_request,
            tools: RuntimeToolRegistry::new(),
        }
    }

    /// `with_tools` 附加可供 agent 使用的工具注册表。
    pub fn with_tools(mut self, tools: RuntimeToolRegistry) -> Self {
        self.tools = tools;
        self
    }

    /// `target` 返回该请求对应的统一 runtime 目标。
    pub fn target(&self) -> RuntimeTarget {
        RuntimeTarget::native_agent(
            self.chat_request.provider_id.clone(),
            self.chat_request.model_id.clone(),
        )
    }

    /// `chat_request` 返回底层模型请求参数。
    pub fn chat_request(&self) -> &NativeChatRequest {
        &self.chat_request
    }

    /// `tools` 返回 agent 可见的工具定义。
    pub fn tools(&self) -> &RuntimeToolRegistry {
        &self.tools
    }

    pub(crate) fn has_tools(&self) -> bool {
        !self.tools.is_empty()
    }
}
