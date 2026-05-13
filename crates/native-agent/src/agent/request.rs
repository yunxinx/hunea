use mo_core::{session::RuntimeTarget, tools::RuntimeToolRegistry};

use crate::{ChatMessage, NativeLlmRequest, ProviderApiKey, ProviderKind};

/// `NativeAgentRequest` 描述一次内置 native agent turn。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeAgentRequest {
    llm_request: NativeLlmRequest,
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
            llm_request: NativeLlmRequest::new(
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

    /// `with_tools` 附加可供 agent 使用的工具注册表。
    pub fn with_tools(mut self, tools: RuntimeToolRegistry) -> Self {
        self.tools = tools;
        self
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

    /// `tools` 返回 agent 可见的工具定义。
    pub fn tools(&self) -> &RuntimeToolRegistry {
        &self.tools
    }
}
