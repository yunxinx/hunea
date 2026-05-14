use std::sync::Arc;

use super::{
    NativeAgentError, NativeAgentRequest,
    response::{NativeAgentCompletion, NativeAgentProgress},
};
use crate::{NativeLlmProgress, execute_rig_agent_for_request};
use mo_core::tools::RuntimeToolExecutor;

pub(crate) async fn send_agent_loop_with_cancellation_and_token_progress<F>(
    request: &NativeAgentRequest,
    executor: Arc<dyn RuntimeToolExecutor>,
    cancellation: &tokio_util::sync::CancellationToken,
    mut on_progress: F,
) -> Result<NativeAgentCompletion, NativeAgentError>
where
    F: FnMut(NativeLlmProgress),
{
    send_agent_loop_with_cancellation_and_progress(request, executor, cancellation, |progress| {
        match progress {
            NativeAgentProgress::OutputTokens { total_tokens } => {
                on_progress(NativeLlmProgress::OutputTokens { total_tokens });
            }
            NativeAgentProgress::Thinking { is_thinking } => {
                on_progress(NativeLlmProgress::Thinking { is_thinking });
            }
            NativeAgentProgress::ToolExecutionStarted { .. }
            | NativeAgentProgress::ToolExecutionFinished { .. } => {}
        }
    })
    .await
}

pub(crate) async fn send_agent_loop_with_cancellation_and_progress<F>(
    request: &NativeAgentRequest,
    executor: Arc<dyn RuntimeToolExecutor>,
    cancellation: &tokio_util::sync::CancellationToken,
    mut on_progress: F,
) -> Result<NativeAgentCompletion, NativeAgentError>
where
    F: FnMut(NativeAgentProgress),
{
    execute_rig_agent_for_request(request, executor, cancellation, &mut on_progress).await
}
