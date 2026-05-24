use provider_protocol::Message;
use tool_loop_runtime::ToolLoopCompletion;

use crate::ProviderRequestMetrics;
pub use runtime_domain::session::ConversationResponse;

/// `ConversationProgress` 描述对话工具循环期间的内部进度事件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ConversationProgress {
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
        activity: runtime_domain::session::RuntimeToolActivity,
    },
    ToolActivityUpdated {
        update: runtime_domain::session::RuntimeToolActivityUpdate,
    },
    TerminalUpdated {
        snapshot: runtime_domain::session::RuntimeTerminalSnapshot,
    },
}

#[derive(Debug)]
pub(crate) struct ConversationCompletion {
    pub(crate) response: ConversationResponse,
    pub(crate) metrics: Option<ProviderRequestMetrics>,
}

impl ConversationCompletion {
    pub(crate) fn from_runtime_completion(completion: ToolLoopCompletion) -> Self {
        Self {
            response: ConversationResponse {
                content: completion.response.content,
                reasoning_content: completion.response.reasoning_content,
                reasoning_duration: completion.response.reasoning_duration,
            },
            metrics: completion.metrics,
        }
    }

    pub(crate) fn into_response(self) -> ConversationResponse {
        let Self { response, metrics } = self;
        let _ = metrics;
        response
    }
}
