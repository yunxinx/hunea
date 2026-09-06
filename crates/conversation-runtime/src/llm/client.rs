use extension_hook_runtime::ExtensionHookRegistry;
use tokio_util::sync::CancellationToken;
use tool_loop_runtime::{ToolLoopOptions, run_tool_loop};
use tool_runtime::{SharedToolPermissionHandler, ToolExecutorRegistry, ToolInvocationIdentity};

use crate::{
    ConversationRequest, PreparedConversationRequest, ProviderClientLease, TurnExecutionError,
    conversation::{ConversationCompletion, ConversationProgress},
    llm::{
        ConversationToolErrorFormatter, ProviderRequestError, prompt_request_from_prepared_request,
        prompt_request_from_provider_request,
    },
};

pub(crate) struct PreparedRequestExecutionOptions {
    pub(crate) tool_max_turns: Option<usize>,
    pub(crate) permission_handler: Option<SharedToolPermissionHandler>,
    pub(crate) extension_hooks: ExtensionHookRegistry,
    pub(crate) invocation_identity: Option<ToolInvocationIdentity>,
}

/// `execute_conversation_request` runs one conversation turn through the provider/tool runtime.
pub(crate) async fn execute_conversation_request<F>(
    lease: &ProviderClientLease,
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

    let prompt_request = prompt_request_from_provider_request(request.provider_request())?;
    let completion = run_tool_loop(
        lease.client(),
        prompt_request,
        executor,
        cancellation,
        ToolLoopOptions {
            tool_max_turns,
            permission_handler,
            error_formatter: std::sync::Arc::new(ConversationToolErrorFormatter),
            clock: Default::default(),
            extension_hooks: ExtensionHookRegistry::new(),
            invocation_identity: None,
        },
        |progress| on_progress(conversation_progress_from_runtime_progress(progress)),
    )
    .await
    .map_err(|error| {
        turn_execution_error_from_tool_loop(error, request.provider_request().provider_id.as_str())
    })?;

    Ok(ConversationCompletion::from_runtime_completion(completion))
}

pub(crate) async fn execute_prepared_conversation_request<F>(
    lease: &ProviderClientLease,
    request: &PreparedConversationRequest,
    executor: ToolExecutorRegistry,
    cancellation: &CancellationToken,
    options: PreparedRequestExecutionOptions,
    on_progress: &mut F,
) -> Result<ConversationCompletion, TurnExecutionError>
where
    F: FnMut(ConversationProgress) + Send,
{
    if cancellation.is_cancelled() {
        return Err(TurnExecutionError::Cancelled);
    }

    let prompt_request = prompt_request_from_prepared_request(lease, request)?;
    let completion = run_tool_loop(
        lease.client(),
        prompt_request,
        executor,
        cancellation,
        ToolLoopOptions {
            tool_max_turns: options.tool_max_turns,
            permission_handler: options.permission_handler,
            error_formatter: std::sync::Arc::new(ConversationToolErrorFormatter),
            clock: Default::default(),
            extension_hooks: options.extension_hooks,
            invocation_identity: options.invocation_identity,
        },
        |progress| on_progress(conversation_progress_from_runtime_progress(progress)),
    )
    .await
    .map_err(|error| turn_execution_error_from_tool_loop(error, request.provider_id()))?;

    Ok(ConversationCompletion::from_runtime_completion(completion))
}

fn turn_execution_error_from_tool_loop(
    error: tool_loop_runtime::ToolLoopError,
    provider_id: &str,
) -> TurnExecutionError {
    match error {
        tool_loop_runtime::ToolLoopError::Cancelled => TurnExecutionError::Cancelled,
        tool_loop_runtime::ToolLoopError::Provider(source) => {
            TurnExecutionError::from(ProviderRequestError::from(source))
        }
        tool_loop_runtime::ToolLoopError::ExtensionHook { source } => {
            TurnExecutionError::from(ProviderRequestError::ExtensionHook { source })
        }
        tool_loop_runtime::ToolLoopError::EmptyPrompt => {
            TurnExecutionError::from(ProviderRequestError::EmptyPrompt {
                provider_id: provider_id.to_string(),
            })
        }
        tool_loop_runtime::ToolLoopError::ToolTurnLimit { max_turns } => {
            TurnExecutionError::from(ProviderRequestError::ToolTurnLimit { max_turns })
        }
    }
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

#[cfg(test)]
mod tests {
    use std::{error::Error as _, sync::Arc, time::Duration};

    use extension_hook_runtime::{
        BeforeToolExecutePayload, ExtensionHookRegistry, HookFailureKind, HookId, HookOwnerId,
        HookPriority, HookRegistrationOptions,
    };

    use super::*;

    #[tokio::test]
    async fn hook_error_mapping_keeps_tool_loop_and_provider_diagnostics_closed() {
        const PRIVATE_VALUES: &[&str] = &[
            "private-instruction-body",
            "private-user-content",
            "private-tool-arguments",
            "private-tool-schema",
            "private-tool-result",
            "/private/workspace/file",
            "private-credential",
            "https://private.example/v1",
            "private-session-id",
            "private-request-id",
            "private-call-id",
            "private-raw-hook-error",
        ];
        let hooks = ExtensionHookRegistry::new();
        let private_raw_error = Arc::<str>::from(PRIVATE_VALUES[11]);
        let private_raw_error_for_hook = Arc::clone(&private_raw_error);
        let _registration = hooks
            .register_before_tool_execute(
                HookOwnerId::try_new("safe-owner").expect("owner id should validate"),
                HookId::try_new("safe-hook").expect("hook id should validate"),
                HookRegistrationOptions::try_new(HookPriority::default(), Duration::from_secs(1))
                    .expect("hook options should validate"),
                Arc::new(move |_: BeforeToolExecutePayload, _| {
                    let private_raw_error = Arc::clone(&private_raw_error_for_hook);
                    async move {
                        assert!(!private_raw_error.is_empty());
                        Err(HookFailureKind::Internal)
                    }
                }),
            )
            .expect("hook should register");
        let source = hooks
            .dispatch_before_tool_execute(
                BeforeToolExecutePayload::new(tool_runtime::ToolCall::new(
                    PRIVATE_VALUES[10],
                    "safe-tool",
                    serde_json::json!({
                        "instruction": PRIVATE_VALUES[0],
                        "user": PRIVATE_VALUES[1],
                        "arguments": PRIVATE_VALUES[2],
                        "schema": PRIVATE_VALUES[3],
                        "result": PRIVATE_VALUES[4],
                        "path": PRIVATE_VALUES[5],
                        "credential": PRIVATE_VALUES[6],
                        "endpoint": PRIVATE_VALUES[7],
                        "session_id": PRIVATE_VALUES[8],
                        "request_id": PRIVATE_VALUES[9],
                    }),
                )),
                &CancellationToken::new(),
            )
            .await
            .expect_err("hook should fail closed");
        let tool_loop_error = tool_loop_runtime::ToolLoopError::ExtensionHook {
            source: source.clone(),
        };
        assert_error_chain_is_closed(&tool_loop_error, PRIVATE_VALUES);
        let mapped = turn_execution_error_from_tool_loop(
            tool_loop_runtime::ToolLoopError::ExtensionHook { source },
            PRIVATE_VALUES[7],
        );
        let TurnExecutionError::Llm(provider_error) = mapped else {
            panic!("hook failure must remain a provider request error")
        };
        assert!(provider_error.source().is_some());
        assert_error_chain_is_closed(&provider_error, PRIVATE_VALUES);
        drop(private_raw_error);
    }

    fn assert_error_chain_is_closed(
        error: &(dyn std::error::Error + 'static),
        private_values: &[&str],
    ) {
        let mut current = Some(error);
        while let Some(error) = current {
            let diagnostic = format!("{error:?} {error}");
            for private in private_values {
                assert!(!diagnostic.contains(private), "diagnostic leaked {private}");
            }
            current = error.source();
        }
    }
}
