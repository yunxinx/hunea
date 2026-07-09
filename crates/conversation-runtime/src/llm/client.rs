use tokio_util::sync::CancellationToken;
use tool_loop_runtime::{ToolLoopOptions, run_tool_loop};
use tool_runtime::{SharedToolPermissionHandler, ToolExecutorRegistry};

use crate::{
    ConversationRequest, PreparedConversationRequest, TurnExecutionError,
    conversation::{ConversationCompletion, ConversationProgress},
    llm::{
        ConversationToolErrorFormatter, ProviderRequestError, openai_client_for_prepared_request,
        openai_client_for_request, prompt_request_from_prepared_request,
        prompt_request_from_provider_request,
    },
};

/// `execute_conversation_request` runs one conversation turn through the provider/tool runtime.
pub(crate) async fn execute_conversation_request<F>(
    request: &ConversationRequest,
    executor: ToolExecutorRegistry,
    cancellation: &CancellationToken,
    tool_max_turns: Option<usize>,
    permission_handler: Option<SharedToolPermissionHandler>,
    on_progress: &mut F,
) -> Result<ConversationCompletion, TurnExecutionError>
where
    F: FnMut(ConversationProgress) + Send,
{
    if cancellation.is_cancelled() {
        return Err(TurnExecutionError::Cancelled);
    }

    let client = openai_client_for_request(request.provider_request())?;
    let prompt_request = prompt_request_from_provider_request(request.provider_request())?;
    let completion = run_tool_loop(
        &client,
        prompt_request,
        executor,
        cancellation,
        ToolLoopOptions {
            tool_max_turns,
            permission_handler,
            error_formatter: std::sync::Arc::new(ConversationToolErrorFormatter),
            clock: Default::default(),
        },
        |progress| on_progress(conversation_progress_from_runtime_progress(progress)),
    )
    .await
    .map_err(|error| match error {
        tool_loop_runtime::ToolLoopError::Cancelled => TurnExecutionError::Cancelled,
        tool_loop_runtime::ToolLoopError::Provider(source) => {
            TurnExecutionError::from(ProviderRequestError::from(source))
        }
        tool_loop_runtime::ToolLoopError::EmptyPrompt => {
            TurnExecutionError::from(ProviderRequestError::EmptyPrompt {
                provider_id: request.provider_request().provider_id.clone(),
            })
        }
        tool_loop_runtime::ToolLoopError::ToolTurnLimit { max_turns } => TurnExecutionError::from(
            ProviderRequestError::Provider(format!("tool turn limit reached ({max_turns})")),
        ),
    })?;

    Ok(ConversationCompletion::from_runtime_completion(completion))
}

pub(crate) async fn execute_prepared_conversation_request<F>(
    request: &PreparedConversationRequest,
    executor: ToolExecutorRegistry,
    cancellation: &CancellationToken,
    tool_max_turns: Option<usize>,
    permission_handler: Option<SharedToolPermissionHandler>,
    on_progress: &mut F,
) -> Result<ConversationCompletion, TurnExecutionError>
where
    F: FnMut(ConversationProgress) + Send,
{
    if cancellation.is_cancelled() {
        return Err(TurnExecutionError::Cancelled);
    }

    let client = openai_client_for_prepared_request(request)?;
    let prompt_request = prompt_request_from_prepared_request(request)?;
    let completion = run_tool_loop(
        &client,
        prompt_request,
        executor,
        cancellation,
        ToolLoopOptions {
            tool_max_turns,
            permission_handler,
            error_formatter: std::sync::Arc::new(ConversationToolErrorFormatter),
            clock: Default::default(),
        },
        |progress| on_progress(conversation_progress_from_runtime_progress(progress)),
    )
    .await
    .map_err(|error| match error {
        tool_loop_runtime::ToolLoopError::Cancelled => TurnExecutionError::Cancelled,
        tool_loop_runtime::ToolLoopError::Provider(source) => {
            TurnExecutionError::from(ProviderRequestError::from(source))
        }
        tool_loop_runtime::ToolLoopError::EmptyPrompt => {
            TurnExecutionError::from(ProviderRequestError::EmptyPrompt {
                provider_id: request.provider_id().to_string(),
            })
        }
        tool_loop_runtime::ToolLoopError::ToolTurnLimit { max_turns } => TurnExecutionError::from(
            ProviderRequestError::Provider(format!("tool turn limit reached ({max_turns})")),
        ),
    })?;

    Ok(ConversationCompletion::from_runtime_completion(completion))
}

fn conversation_progress_from_runtime_progress(
    progress: tool_loop_runtime::ToolLoopProgress,
) -> ConversationProgress {
    match progress {
        tool_loop_runtime::ToolLoopProgress::ProviderTurnStarted => {
            ConversationProgress::ProviderTurnStarted
        }
        tool_loop_runtime::ToolLoopProgress::SystemMessage { message } => {
            ConversationProgress::SystemMessage { message }
        }
        tool_loop_runtime::ToolLoopProgress::ProviderContextItem { item } => {
            ConversationProgress::ProviderContextItem { item }
        }
        tool_loop_runtime::ToolLoopProgress::OutputTokens { total_tokens } => {
            ConversationProgress::OutputTokens { total_tokens }
        }
        tool_loop_runtime::ToolLoopProgress::InputTokens { total_tokens } => {
            ConversationProgress::InputTokens { total_tokens }
        }
        tool_loop_runtime::ToolLoopProgress::Thinking { is_thinking } => {
            ConversationProgress::Thinking { is_thinking }
        }
        tool_loop_runtime::ToolLoopProgress::AssistantDelta { content } => {
            ConversationProgress::AssistantDelta { content }
        }
        tool_loop_runtime::ToolLoopProgress::ReasoningDelta { content } => {
            ConversationProgress::ReasoningDelta { content }
        }
        tool_loop_runtime::ToolLoopProgress::ToolActivityStarted { activity } => {
            ConversationProgress::ToolActivityStarted { activity }
        }
        tool_loop_runtime::ToolLoopProgress::ToolActivityUpdated { update } => {
            ConversationProgress::ToolActivityUpdated { update }
        }
        tool_loop_runtime::ToolLoopProgress::TerminalUpdated { snapshot } => {
            ConversationProgress::TerminalUpdated { snapshot }
        }
    }
}
