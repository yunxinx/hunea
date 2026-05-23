use mo_agent_runtime::AgentRuntimeCompletion;
use mo_ai_core::Message;

use crate::NativeLlmPerformanceMetrics;
pub use mo_core::session::NativeAgentResponse;

/// `NativeAgentProgress` 描述 native agent loop 期间的内部进度事件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum NativeAgentProgress {
    ProviderTurnStarted,
    ProviderContextMessage {
        message: Message,
    },
    OutputTokens {
        total_tokens: usize,
    },
    InputTokens {
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
        activity: mo_core::session::RuntimeToolActivity,
    },
    ToolActivityUpdated {
        update: mo_core::session::RuntimeToolActivityUpdate,
    },
}

#[derive(Debug)]
pub(crate) struct NativeAgentCompletion {
    pub(crate) response: NativeAgentResponse,
    pub(crate) metrics: Option<NativeLlmPerformanceMetrics>,
}

impl NativeAgentCompletion {
    pub(crate) fn from_runtime_completion(completion: AgentRuntimeCompletion) -> Self {
        Self {
            response: NativeAgentResponse {
                content: completion.response.content,
                reasoning_content: completion.response.reasoning_content,
                reasoning_duration: completion.response.reasoning_duration,
            },
            metrics: completion.metrics,
        }
    }

    pub(crate) fn into_response(self) -> NativeAgentResponse {
        let Self { response, metrics } = self;
        let _ = metrics;
        response
    }
}
