use mo_agent_runtime::{AgentRuntimeOptions, run_agent_runtime};
use mo_tools::{SharedToolPermissionHandler, ToolExecutorRegistry};
use tokio_util::sync::CancellationToken;

use crate::{
    NativeAgentError, NativeAgentExecutionRequest, NativeAgentRequest,
    agent::{NativeAgentCompletion, NativeAgentProgress},
    llm::{
        NativeAgentToolErrorFormatter, NativeLlmError, openai_client_for_execution_request,
        openai_client_for_request, prompt_request_from_execution_request,
        prompt_request_from_native_llm_request,
    },
};

/// `execute_native_agent_for_request` runs one native turn through Lumos AI runtime.
pub(crate) async fn execute_native_agent_for_request<F>(
    request: &NativeAgentRequest,
    executor: ToolExecutorRegistry,
    cancellation: &CancellationToken,
    tool_max_turns: Option<usize>,
    permission_handler: Option<SharedToolPermissionHandler>,
    on_progress: &mut F,
) -> Result<NativeAgentCompletion, NativeAgentError>
where
    F: FnMut(NativeAgentProgress) + Send,
{
    if cancellation.is_cancelled() {
        return Err(NativeAgentError::Cancelled);
    }

    let client = openai_client_for_request(request.llm_request())?;
    let prompt_request = prompt_request_from_native_llm_request(request.llm_request())?;
    let completion = run_agent_runtime(
        &client,
        prompt_request,
        executor,
        cancellation,
        AgentRuntimeOptions {
            tool_max_turns,
            permission_handler,
            error_formatter: std::sync::Arc::new(NativeAgentToolErrorFormatter),
        },
        |progress| on_progress(native_progress_from_runtime_progress(progress)),
    )
    .await
    .map_err(|error| match error {
        mo_agent_runtime::AgentRuntimeError::Cancelled => NativeAgentError::Cancelled,
        mo_agent_runtime::AgentRuntimeError::Provider(source) => {
            NativeAgentError::from(NativeLlmError::from(source))
        }
        mo_agent_runtime::AgentRuntimeError::EmptyPrompt => {
            NativeAgentError::from(NativeLlmError::EmptyPrompt {
                provider_id: request.llm_request().provider_id.clone(),
            })
        }
        mo_agent_runtime::AgentRuntimeError::ToolTurnLimit { max_turns } => NativeAgentError::from(
            NativeLlmError::Provider(format!("agent reached tool turn limit ({max_turns})")),
        ),
    })?;

    Ok(NativeAgentCompletion::from_runtime_completion(completion))
}

pub(crate) async fn execute_native_agent_for_execution_request<F>(
    request: &NativeAgentExecutionRequest,
    executor: ToolExecutorRegistry,
    cancellation: &CancellationToken,
    tool_max_turns: Option<usize>,
    permission_handler: Option<SharedToolPermissionHandler>,
    on_progress: &mut F,
) -> Result<NativeAgentCompletion, NativeAgentError>
where
    F: FnMut(NativeAgentProgress) + Send,
{
    if cancellation.is_cancelled() {
        return Err(NativeAgentError::Cancelled);
    }

    let client = openai_client_for_execution_request(request)?;
    let prompt_request = prompt_request_from_execution_request(request)?;
    let completion = run_agent_runtime(
        &client,
        prompt_request,
        executor,
        cancellation,
        AgentRuntimeOptions {
            tool_max_turns,
            permission_handler,
            error_formatter: std::sync::Arc::new(NativeAgentToolErrorFormatter),
        },
        |progress| on_progress(native_progress_from_runtime_progress(progress)),
    )
    .await
    .map_err(|error| match error {
        mo_agent_runtime::AgentRuntimeError::Cancelled => NativeAgentError::Cancelled,
        mo_agent_runtime::AgentRuntimeError::Provider(source) => {
            NativeAgentError::from(NativeLlmError::from(source))
        }
        mo_agent_runtime::AgentRuntimeError::EmptyPrompt => {
            NativeAgentError::from(NativeLlmError::EmptyPrompt {
                provider_id: request.provider_id().to_string(),
            })
        }
        mo_agent_runtime::AgentRuntimeError::ToolTurnLimit { max_turns } => NativeAgentError::from(
            NativeLlmError::Provider(format!("agent reached tool turn limit ({max_turns})")),
        ),
    })?;

    Ok(NativeAgentCompletion::from_runtime_completion(completion))
}

fn native_progress_from_runtime_progress(
    progress: mo_agent_runtime::AgentRuntimeProgress,
) -> NativeAgentProgress {
    match progress {
        mo_agent_runtime::AgentRuntimeProgress::ProviderTurnStarted => {
            NativeAgentProgress::ProviderTurnStarted
        }
        mo_agent_runtime::AgentRuntimeProgress::ProviderContextMessage { message } => {
            NativeAgentProgress::ProviderContextMessage { message }
        }
        mo_agent_runtime::AgentRuntimeProgress::OutputTokens { total_tokens } => {
            NativeAgentProgress::OutputTokens { total_tokens }
        }
        mo_agent_runtime::AgentRuntimeProgress::InputTokens { total_tokens } => {
            NativeAgentProgress::InputTokens { total_tokens }
        }
        mo_agent_runtime::AgentRuntimeProgress::Thinking { is_thinking } => {
            NativeAgentProgress::Thinking { is_thinking }
        }
        mo_agent_runtime::AgentRuntimeProgress::AssistantDelta { content } => {
            NativeAgentProgress::AssistantDelta { content }
        }
        mo_agent_runtime::AgentRuntimeProgress::ReasoningDelta { content } => {
            NativeAgentProgress::ReasoningDelta { content }
        }
        mo_agent_runtime::AgentRuntimeProgress::ToolActivityStarted { activity } => {
            NativeAgentProgress::ToolActivityStarted { activity }
        }
        mo_agent_runtime::AgentRuntimeProgress::ToolActivityUpdated { update } => {
            NativeAgentProgress::ToolActivityUpdated { update }
        }
    }
}
