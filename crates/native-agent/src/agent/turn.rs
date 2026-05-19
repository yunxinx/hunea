use super::{
    NativeAgentError, NativeAgentRequest,
    response::{NativeAgentCompletion, NativeAgentProgress},
};
use crate::{NativeLlmProgress, execute_rig_agent_for_request};
use mo_tools::ToolExecutorRegistry;

pub(crate) async fn send_agent_loop_with_cancellation_and_token_progress<F>(
    request: &NativeAgentRequest,
    executor: ToolExecutorRegistry,
    cancellation: &tokio_util::sync::CancellationToken,
    tool_max_turns: Option<usize>,
    mut on_progress: F,
) -> Result<NativeAgentCompletion, NativeAgentError>
where
    F: FnMut(NativeLlmProgress),
{
    send_agent_loop_with_cancellation_and_progress(
        request,
        executor,
        cancellation,
        tool_max_turns,
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
    mut on_progress: F,
) -> Result<NativeAgentCompletion, NativeAgentError>
where
    F: FnMut(NativeAgentProgress),
{
    execute_rig_agent_for_request(
        request,
        executor,
        cancellation,
        tool_max_turns,
        &mut on_progress,
    )
    .await
}
