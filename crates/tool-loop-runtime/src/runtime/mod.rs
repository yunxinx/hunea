//! Tool loop runtime 的 provider streaming 与工具轮次编排入口。

use provider_protocol::{ConversationItem, PromptRequest, ProviderClient};
use tokio_util::sync::CancellationToken;
use tool_runtime::ToolExecutorRegistry;

use crate::{
    activity::{runtime_tool_activity_from_call, runtime_tool_activity_update_from_result},
    error::ToolLoopError,
};

mod execution;
mod state;
mod streaming;
mod types;

use execution::{ToolCallExecutionContext, execute_tool_call, interrupted_tool_execution};
use state::{
    RuntimeTurnState, runtime_tool_activity_update_duplicates_tool_arguments,
    runtime_tool_activity_update_token_text,
};
use streaming::{
    append_provider_context_item, append_provider_context_items, stream_provider_turn,
};

pub use execution::provider_tool_definitions_from_registry;
pub use types::{
    ToolLoopClock, ToolLoopCompletion, ToolLoopOptions, ToolLoopProgress, ToolLoopResponse,
};

/// `run_tool_loop` 负责执行 provider turn 与工具循环，直到本轮请求完成。
pub async fn run_tool_loop<C, F>(
    client: &C,
    mut request: PromptRequest,
    executor: ToolExecutorRegistry,
    cancellation: &CancellationToken,
    options: ToolLoopOptions,
    mut on_progress: F,
) -> Result<ToolLoopCompletion, ToolLoopError>
where
    C: ProviderClient + ?Sized,
    F: FnMut(ToolLoopProgress) + Send,
{
    if request.items.is_empty() {
        return Err(ToolLoopError::EmptyPrompt);
    }
    if cancellation.is_cancelled() {
        return Err(ToolLoopError::Cancelled);
    }

    let tool_definitions = executor.definitions();
    request.tools = provider_tool_definitions_from_registry(&tool_definitions);
    let mut state = RuntimeTurnState::new(request.model.clone());
    let clock = options.clock.clone();
    let mut tool_turns = 0usize;
    let mut appended_items = Vec::new();

    loop {
        let provider_completion = stream_provider_turn(
            client,
            &request,
            cancellation,
            &clock,
            &mut state,
            &mut on_progress,
        )
        .await?;
        let tool_calls = extract_tool_calls(&provider_completion.items);
        if !provider_completion.finish_reason.is_tool_call() || tool_calls.is_empty() {
            append_provider_context_items(
                &provider_completion.items,
                &mut appended_items,
                &mut on_progress,
            );
            return Ok(state.finish_at(clock.now(), appended_items));
        }

        if let Some(max_turns) = options.tool_max_turns
            && tool_turns >= max_turns
        {
            return Err(ToolLoopError::ToolTurnLimit { max_turns });
        }
        tool_turns = tool_turns.saturating_add(1);
        request
            .items
            .extend(provider_completion.items.iter().cloned());
        append_provider_context_items(
            &provider_completion.items,
            &mut appended_items,
            &mut on_progress,
        );

        let mut tool_result_batch = Vec::new();
        for call in &tool_calls {
            let activity = runtime_tool_activity_from_call(call, &tool_definitions);
            on_progress(ToolLoopProgress::ToolActivityStarted { activity });
            let mut tool_call_context = ToolCallExecutionContext {
                executor: &executor,
                tool_definitions: &tool_definitions,
                cancellation,
                clock: &clock,
                permission_handler: options.permission_handler.as_ref(),
                error_formatter: &options.error_formatter,
                state: &mut state,
            };
            let execution = if cancellation.is_cancelled() {
                interrupted_tool_execution(call)
            } else {
                execute_tool_call(call, &mut tool_call_context, &mut on_progress).await
            };
            let update = runtime_tool_activity_update_from_result(
                call,
                &execution.raw_result,
                execution.processed_error.as_ref(),
                &tool_definitions,
            );
            let visible_tool_output = runtime_tool_activity_update_token_text(&update);
            let suppress_counted_arguments =
                runtime_tool_activity_update_duplicates_tool_arguments(&update);
            let activity_id = update.activity_id.clone();
            on_progress(ToolLoopProgress::ToolActivityUpdated { update });
            state.observe_tool_activity_output(
                &activity_id,
                visible_tool_output.as_deref(),
                suppress_counted_arguments,
                clock.now(),
                &mut on_progress,
            );
            let tool_result_item = ConversationItem::tool_result(
                execution.provider_result.call_id.clone(),
                execution.provider_result.content.clone(),
                execution.provider_result.is_error,
            );
            tool_result_batch.push((tool_result_item, execution.raw_result.terminate));
        }

        let should_terminate_after_batch = tool_result_batch
            .iter()
            .all(|(_, should_terminate)| *should_terminate);
        for (tool_result_item, _) in tool_result_batch {
            if should_terminate_after_batch {
                append_provider_context_item(
                    tool_result_item,
                    &mut appended_items,
                    &mut on_progress,
                );
                continue;
            }
            state.observe_tool_result_input(&tool_result_item, clock.now(), &mut on_progress);
            request.items.push(tool_result_item.clone());
            append_provider_context_item(tool_result_item, &mut appended_items, &mut on_progress);
        }
        if cancellation.is_cancelled() {
            return Err(ToolLoopError::Cancelled);
        }
        if should_terminate_after_batch {
            return Ok(state.finish_at(clock.now(), appended_items));
        }
    }
}

fn extract_tool_calls(items: &[ConversationItem]) -> Vec<provider_protocol::ToolCall> {
    items
        .iter()
        .flat_map(|item| item.tool_calls().cloned())
        .collect()
}

#[cfg(test)]
mod tests;
