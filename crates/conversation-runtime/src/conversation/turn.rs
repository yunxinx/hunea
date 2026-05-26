use super::{
    ConversationRequest, TurnExecutionError,
    response::{ConversationCompletion, ConversationProgress},
};
use crate::{
    PreparedConversationRequest, ProviderProgress,
    llm::{execute_conversation_request, execute_prepared_conversation_request},
};
use tool_runtime::{SharedToolPermissionHandler, ToolExecutorRegistry};

pub(crate) async fn run_conversation_turn_with_cancellation_and_token_progress<F>(
    request: &ConversationRequest,
    executor: ToolExecutorRegistry,
    cancellation: &tokio_util::sync::CancellationToken,
    tool_max_turns: Option<usize>,
    permission_handler: Option<SharedToolPermissionHandler>,
    mut on_progress: F,
) -> Result<ConversationCompletion, TurnExecutionError>
where
    F: FnMut(ProviderProgress) + Send,
{
    run_conversation_turn_with_cancellation_and_progress(
        request,
        executor,
        cancellation,
        tool_max_turns,
        permission_handler,
        |progress| match progress {
            ConversationProgress::OutputTokens { total_tokens } => {
                on_progress(ProviderProgress::OutputTokens { total_tokens });
            }
            ConversationProgress::InputTokens { .. } => {}
            ConversationProgress::Thinking { is_thinking } => {
                on_progress(ProviderProgress::Thinking { is_thinking });
            }
            ConversationProgress::AssistantDelta { .. }
            | ConversationProgress::ReasoningDelta { .. }
            | ConversationProgress::SystemMessage { .. }
            | ConversationProgress::ProviderTurnStarted
            | ConversationProgress::ProviderContextMessage { .. }
            | ConversationProgress::ToolActivityStarted { .. }
            | ConversationProgress::ToolActivityUpdated { .. }
            | ConversationProgress::TerminalUpdated { .. }
            | ConversationProgress::ManagedSearchToolAuthorization { .. } => {}
        },
    )
    .await
}

pub(crate) async fn run_conversation_turn_with_cancellation_and_progress<F>(
    request: &ConversationRequest,
    executor: ToolExecutorRegistry,
    cancellation: &tokio_util::sync::CancellationToken,
    tool_max_turns: Option<usize>,
    permission_handler: Option<SharedToolPermissionHandler>,
    mut on_progress: F,
) -> Result<ConversationCompletion, TurnExecutionError>
where
    F: FnMut(ConversationProgress) + Send,
{
    execute_conversation_request(
        request,
        executor,
        cancellation,
        tool_max_turns,
        permission_handler,
        &mut on_progress,
    )
    .await
}

pub(crate) async fn run_prepared_conversation_with_progress<F>(
    request: &PreparedConversationRequest,
    executor: ToolExecutorRegistry,
    cancellation: &tokio_util::sync::CancellationToken,
    tool_max_turns: Option<usize>,
    permission_handler: Option<SharedToolPermissionHandler>,
    mut on_progress: F,
) -> Result<ConversationCompletion, TurnExecutionError>
where
    F: FnMut(ConversationProgress) + Send,
{
    execute_prepared_conversation_request(
        request,
        executor,
        cancellation,
        tool_max_turns,
        permission_handler,
        &mut on_progress,
    )
    .await
}
