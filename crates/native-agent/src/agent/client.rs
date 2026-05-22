use super::{
    NativeAgentError, NativeAgentRequest, response::NativeAgentResponse,
    turn::send_agent_loop_with_cancellation_and_token_progress,
};
use mo_tools::ToolExecutorRegistry;

/// `send_agent_loop_with_cancellation` 执行带工具回灌的 native agent loop。
pub async fn send_agent_loop_with_cancellation(
    request: &NativeAgentRequest,
    executor: ToolExecutorRegistry,
    cancellation: &tokio_util::sync::CancellationToken,
) -> Result<NativeAgentResponse, NativeAgentError> {
    let completion = send_agent_loop_with_cancellation_and_token_progress(
        request,
        executor,
        cancellation,
        None,
        None,
        |_| {},
    )
    .await?;
    Ok(completion.into_response())
}
