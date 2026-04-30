use std::time::Duration;

use crate::runtime::native::chat::{
    ChatPerformanceMetrics, NativeChatProgress, NativeChatResponse,
    send_chat_with_cancellation_and_token_progress,
};

use super::{NativeAgentError, NativeAgentRequest};

/// `NativeAgentResponse` 保存 native agent 单轮输出。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NativeAgentResponse {
    pub content: String,
    pub reasoning_content: Option<String>,
    pub reasoning_duration: Option<Duration>,
}

impl From<NativeChatResponse> for NativeAgentResponse {
    fn from(response: NativeChatResponse) -> Self {
        Self {
            content: response.content,
            reasoning_content: response.reasoning_content,
            reasoning_duration: response.reasoning_duration,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NativeAgentCompletion {
    pub(crate) response: NativeAgentResponse,
    pub(crate) metrics: Option<ChatPerformanceMetrics>,
}

/// `send_agent_turn_with_cancellation` 执行不含工具调用的 native agent 单轮请求。
pub async fn send_agent_turn_with_cancellation(
    request: &NativeAgentRequest,
    cancellation: &tokio_util::sync::CancellationToken,
) -> Result<NativeAgentResponse, NativeAgentError> {
    send_agent_turn_with_cancellation_and_token_progress(request, cancellation, |_| {})
        .await
        .map(|completion| completion.response)
}

pub(crate) async fn send_agent_turn_with_cancellation_and_token_progress<F>(
    request: &NativeAgentRequest,
    cancellation: &tokio_util::sync::CancellationToken,
    on_progress: F,
) -> Result<NativeAgentCompletion, NativeAgentError>
where
    F: FnMut(NativeChatProgress),
{
    if cancellation.is_cancelled() {
        return Err(NativeAgentError::Cancelled);
    }
    if request.has_tools() {
        return Err(NativeAgentError::ToolsRequireExecutor);
    }

    let completion = send_chat_with_cancellation_and_token_progress(
        request.chat_request(),
        cancellation,
        on_progress,
    )
    .await?;

    Ok(NativeAgentCompletion {
        response: completion.response.into(),
        metrics: completion.metrics,
    })
}
