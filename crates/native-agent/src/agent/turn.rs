use super::{
    NativeAgentError, NativeAgentRequest,
    response::{NativeAgentCompletion, NativeAgentProgress},
};
use crate::{NativeLlmProgress, execute_native_agent_for_request};
use mo_tools::{SharedToolPermissionHandler, ToolExecutorRegistry};

pub(crate) async fn send_agent_loop_with_cancellation_and_token_progress<F>(
    request: &NativeAgentRequest,
    executor: ToolExecutorRegistry,
    cancellation: &tokio_util::sync::CancellationToken,
    tool_max_turns: Option<usize>,
    permission_handler: Option<SharedToolPermissionHandler>,
    mut on_progress: F,
) -> Result<NativeAgentCompletion, NativeAgentError>
where
    F: FnMut(NativeLlmProgress) + Send,
{
    send_agent_loop_with_cancellation_and_progress(
        request,
        executor,
        cancellation,
        tool_max_turns,
        permission_handler,
        |progress| match progress {
            NativeAgentProgress::OutputTokens { total_tokens } => {
                on_progress(NativeLlmProgress::OutputTokens { total_tokens });
            }
            NativeAgentProgress::Thinking { is_thinking } => {
                on_progress(NativeLlmProgress::Thinking { is_thinking });
            }
            NativeAgentProgress::AssistantDelta { .. }
            | NativeAgentProgress::ReasoningDelta { .. }
            | NativeAgentProgress::ToolActivityStarted { .. }
            | NativeAgentProgress::ToolActivityUpdated { .. } => {}
        },
    )
    .await
}

pub(crate) async fn send_agent_loop_with_cancellation_and_progress<F>(
    request: &NativeAgentRequest,
    executor: ToolExecutorRegistry,
    cancellation: &tokio_util::sync::CancellationToken,
    tool_max_turns: Option<usize>,
    permission_handler: Option<SharedToolPermissionHandler>,
    mut on_progress: F,
) -> Result<NativeAgentCompletion, NativeAgentError>
where
    F: FnMut(NativeAgentProgress) + Send,
{
    execute_native_agent_for_request(
        request,
        executor,
        cancellation,
        tool_max_turns,
        permission_handler,
        &mut on_progress,
    )
    .await
}
