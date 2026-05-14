use std::{
    collections::{HashMap, VecDeque},
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

use mo_core::tools::{
    RuntimeToolCall, RuntimeToolDefinition, RuntimeToolExecutor, RuntimeToolResult,
};
use rig_core::{
    agent::{HookAction, PromptHook, ToolCallHookAction},
    completion::{CompletionModel, ToolDefinition},
    message::{ToolCall as RigToolCall, ToolResultContent},
    tool::{ToolDyn, ToolError},
    wasm_compat::WasmBoxedFuture,
};
use tokio_util::sync::CancellationToken;

use crate::NativeAgentRequest;

pub(crate) fn build_rig_tools_for_request(
    request: &NativeAgentRequest,
    executor: Arc<dyn RuntimeToolExecutor>,
    cancellation: CancellationToken,
    state: Arc<RigToolExecutionState>,
) -> Vec<Box<dyn ToolDyn>> {
    request
        .tools()
        .definitions()
        .cloned()
        .map(|definition| {
            Box::new(RigRuntimeTool {
                definition,
                executor: Arc::clone(&executor),
                cancellation: cancellation.clone(),
                state: Arc::clone(&state),
            }) as Box<dyn ToolDyn>
        })
        .collect()
}

pub(crate) fn runtime_tool_call_from_rig(tool_call: RigToolCall) -> RuntimeToolCall {
    RuntimeToolCall::new(
        tool_call
            .call_id
            .clone()
            .unwrap_or_else(|| tool_call.id.clone()),
        tool_call.function.name,
        tool_call.function.arguments,
    )
}

pub(crate) fn tool_result_text(content: &rig_core::OneOrMany<ToolResultContent>) -> String {
    content
        .iter()
        .map(|content| match content {
            ToolResultContent::Text(text) => text.text.as_str(),
            ToolResultContent::Image(_) => "[image]",
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[derive(Default)]
pub(crate) struct RigToolExecutionState {
    streamed_calls: Mutex<HashMap<String, RuntimeToolCall>>,
    pending_calls: Mutex<VecDeque<PendingRigToolCall>>,
    completed_results: Mutex<HashMap<String, RuntimeToolResult>>,
    fallback_call_counter: AtomicUsize,
}

impl RigToolExecutionState {
    pub(crate) fn register_streamed_tool_call(
        &self,
        internal_call_id: String,
        call: RuntimeToolCall,
    ) {
        self.streamed_calls
            .lock()
            .expect("rig tool state lock should not be poisoned")
            .insert(internal_call_id, call);
    }

    fn queue_pending_tool_call(
        &self,
        internal_call_id: &str,
        tool_name: &str,
        tool_call_id: Option<String>,
        args: &str,
    ) {
        let call = self
            .streamed_calls
            .lock()
            .expect("rig tool state lock should not be poisoned")
            .get(internal_call_id)
            .cloned()
            .unwrap_or_else(|| {
                RuntimeToolCall::new(
                    tool_call_id.unwrap_or_else(|| internal_call_id.to_string()),
                    tool_name.to_string(),
                    serde_json::from_str(args).unwrap_or_else(|_| serde_json::json!({})),
                )
            });
        self.pending_calls
            .lock()
            .expect("rig tool state lock should not be poisoned")
            .push_back(PendingRigToolCall {
                internal_call_id: internal_call_id.to_string(),
                arguments: args.to_string(),
                call,
            });
    }

    fn take_pending_tool_call(&self, tool_name: &str, args: &str) -> PendingRigToolCall {
        let mut pending_calls = self
            .pending_calls
            .lock()
            .expect("rig tool state lock should not be poisoned");
        if let Some(index) = pending_calls
            .iter()
            .position(|pending| pending.call.name == tool_name && pending.arguments == args)
        {
            return pending_calls
                .remove(index)
                .expect("pending tool call index should exist");
        }
        if let Some(index) = pending_calls
            .iter()
            .position(|pending| pending.call.name == tool_name)
        {
            return pending_calls
                .remove(index)
                .expect("pending tool call index should exist");
        }

        let sequence = self.fallback_call_counter.fetch_add(1, Ordering::Relaxed);
        let internal_call_id = format!("rig-tool-{sequence}");
        PendingRigToolCall {
            internal_call_id: internal_call_id.clone(),
            arguments: args.to_string(),
            call: RuntimeToolCall::new(
                internal_call_id,
                tool_name.to_string(),
                serde_json::from_str(args).unwrap_or_else(|_| serde_json::json!({})),
            ),
        }
    }

    fn complete_tool_result(&self, internal_call_id: String, result: RuntimeToolResult) {
        self.completed_results
            .lock()
            .expect("rig tool state lock should not be poisoned")
            .insert(internal_call_id, result);
    }

    pub(crate) fn take_completed_tool_result(
        &self,
        internal_call_id: &str,
    ) -> Option<RuntimeToolResult> {
        self.completed_results
            .lock()
            .expect("rig tool state lock should not be poisoned")
            .remove(internal_call_id)
    }

    pub(crate) fn take_streamed_tool_call(
        &self,
        internal_call_id: &str,
    ) -> Option<RuntimeToolCall> {
        self.streamed_calls
            .lock()
            .expect("rig tool state lock should not be poisoned")
            .remove(internal_call_id)
    }
}

struct PendingRigToolCall {
    internal_call_id: String,
    arguments: String,
    call: RuntimeToolCall,
}

struct RigRuntimeTool {
    definition: RuntimeToolDefinition,
    executor: Arc<dyn RuntimeToolExecutor>,
    cancellation: CancellationToken,
    state: Arc<RigToolExecutionState>,
}

impl ToolDyn for RigRuntimeTool {
    fn name(&self) -> String {
        self.definition.name.clone()
    }

    fn definition<'a>(&'a self, _prompt: String) -> WasmBoxedFuture<'a, ToolDefinition> {
        Box::pin(async move {
            ToolDefinition {
                name: self.definition.name.clone(),
                description: self.definition.description.clone().unwrap_or_default(),
                parameters: self
                    .definition
                    .input_schema
                    .clone()
                    .unwrap_or_else(|| serde_json::json!({ "type": "object" })),
            }
        })
    }

    fn call<'a>(&'a self, args: String) -> WasmBoxedFuture<'a, Result<String, ToolError>> {
        Box::pin(async move {
            if self.cancellation.is_cancelled() {
                return Err(ToolError::ToolCallError(Box::new(RigToolCancelled)));
            }

            let pending = self
                .state
                .take_pending_tool_call(&self.definition.name, &args);
            let mut call = pending.call;
            call.arguments = serde_json::from_str(&args).unwrap_or_else(|_| serde_json::json!({}));
            let mut result = self
                .executor
                .execute_tool(call.clone(), &self.cancellation)
                .await;
            result.call_id = call.call_id;
            let content = result.content.clone();
            self.state
                .complete_tool_result(pending.internal_call_id, result);
            Ok(content)
        })
    }
}

#[derive(Clone)]
pub(crate) struct RigToolProgressHook {
    state: Arc<RigToolExecutionState>,
}

impl RigToolProgressHook {
    pub(crate) fn new(state: Arc<RigToolExecutionState>) -> Self {
        Self { state }
    }
}

impl<M> PromptHook<M> for RigToolProgressHook
where
    M: CompletionModel,
{
    async fn on_tool_call(
        &self,
        tool_name: &str,
        tool_call_id: Option<String>,
        internal_call_id: &str,
        args: &str,
    ) -> ToolCallHookAction {
        self.state
            .queue_pending_tool_call(internal_call_id, tool_name, tool_call_id, args);
        ToolCallHookAction::cont()
    }

    async fn on_tool_result(
        &self,
        _tool_name: &str,
        _tool_call_id: Option<String>,
        internal_call_id: &str,
        _args: &str,
        _result: &str,
    ) -> HookAction {
        let _ = internal_call_id;
        HookAction::cont()
    }
}

#[derive(Debug)]
struct RigToolCancelled;

impl std::fmt::Display for RigToolCancelled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("tool call cancelled")
    }
}

impl std::error::Error for RigToolCancelled {}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use mo_core::{
        provider::ProviderKind,
        tools::{RuntimeToolDefinition, RuntimeToolExecutorRegistry, RuntimeToolRegistry},
    };
    use tokio_util::sync::CancellationToken;

    use super::{RigToolExecutionState, build_rig_tools_for_request};
    use crate::{ChatMessage, NativeAgentRequest};

    #[test]
    fn rig_tools_for_request_preserve_runtime_tool_schema_order() {
        let mut registry = RuntimeToolRegistry::new();
        registry.insert(RuntimeToolDefinition::new("write_file").with_description("Write"));
        registry.insert(
            RuntimeToolDefinition::new("read_file")
                .with_description("Read")
                .with_input_schema(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "path": { "type": "string" }
                    },
                    "required": ["path"]
                })),
        );
        let request = NativeAgentRequest::new(
            "local",
            ProviderKind::OpenAiCompatible,
            "qwen3",
            Some("http://127.0.0.1:1234/v1".to_string()),
            None,
            None,
            vec![ChatMessage::user("read Cargo.toml".to_string())],
        )
        .with_tools(registry);

        let tools = build_rig_tools_for_request(
            &request,
            Arc::new(RuntimeToolExecutorRegistry::new()),
            CancellationToken::new(),
            Arc::new(RigToolExecutionState::default()),
        );

        assert_eq!(tools.len(), 2);
        assert_eq!(tools[0].name(), "read_file");
        assert_eq!(tools[1].name(), "write_file");
    }
}
