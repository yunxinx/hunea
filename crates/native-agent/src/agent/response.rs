use genai::chat::StreamEnd;
use mo_core::tools::{RuntimeToolCall, RuntimeToolResult};

use crate::NativeLlmPerformanceMetrics;
pub use mo_core::session::NativeAgentResponse;

/// `NativeAgentProgress` 描述 native agent loop 期间的内部进度事件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum NativeAgentProgress {
    OutputTokens {
        total_tokens: usize,
    },
    Thinking {
        is_thinking: bool,
    },
    ToolExecutionStarted {
        call: RuntimeToolCall,
    },
    ToolExecutionFinished {
        call: RuntimeToolCall,
        result: RuntimeToolResult,
    },
}

#[derive(Debug)]
pub(crate) struct NativeAgentCompletion {
    pub(crate) response: NativeAgentResponse,
    pub(crate) metrics: Option<NativeLlmPerformanceMetrics>,
    pub(crate) stream_end: Option<StreamEnd>,
}

impl NativeAgentCompletion {
    pub(crate) fn into_response(self) -> NativeAgentResponse {
        let Self {
            response,
            metrics,
            stream_end,
        } = self;
        let _ = metrics;
        let _ = stream_end;
        response
    }
}
