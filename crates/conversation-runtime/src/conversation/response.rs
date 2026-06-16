use provider_protocol::ConversationItem;
use tool_loop_runtime::ToolLoopCompletion;

use crate::ProviderRequestMetrics;
pub use runtime_domain::session::ConversationResponse;

/// `ConversationProgress` 描述对话工具循环期间的内部进度事件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ConversationProgress {
    SystemMessage {
        message: String,
    },
    ProviderTurnStarted,
    ProviderContextItem {
        item: ConversationItem,
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
    ManagedSearchToolAuthorization {
        tool: runtime_domain::session::ManagedSearchTool,
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
            response: ConversationResponse::new(
                completion.response.items,
                completion.response.reasoning_duration,
            ),
            metrics: completion.metrics,
        }
    }

    pub(crate) fn into_response(self) -> ConversationResponse {
        let Self { response, metrics } = self;
        let _ = metrics;
        response
    }
}
