use std::{collections::BTreeMap, future::Future, pin::Pin, sync::Arc};

use tokio_util::sync::CancellationToken;

use super::{
    RuntimeToolCall, RuntimeToolDefinition, RuntimeToolRegistry, RuntimeToolResult,
    schema::validate_tool_arguments,
};

/// `RuntimeToolExecutionFuture` 是工具执行返回结果的异步任务。
pub type RuntimeToolExecutionFuture<'a> =
    Pin<Box<dyn Future<Output = RuntimeToolResult> + Send + 'a>>;

/// `RuntimeTool` 描述一个可执行的 runtime tool。
pub trait RuntimeTool: Send + Sync {
    /// `definition` 返回暴露给模型与 UI 的工具定义。
    fn definition(&self) -> RuntimeToolDefinition;

    /// `execute` 执行一次模型发起的工具调用。
    fn execute<'a>(
        &'a self,
        call: RuntimeToolCall,
        cancellation: &'a CancellationToken,
    ) -> RuntimeToolExecutionFuture<'a>;
}

/// `RuntimeToolExecutor` 是 runtime/agent 调用工具时依赖的最小执行边界。
pub trait RuntimeToolExecutor: Send + Sync {
    /// `execute_tool` 执行一次工具调用，并返回可回传给模型的结果。
    fn execute_tool<'a>(
        &'a self,
        call: RuntimeToolCall,
        cancellation: &'a CancellationToken,
    ) -> RuntimeToolExecutionFuture<'a>;
}

/// `RuntimeToolExecutorRegistry` 保存可执行工具，并按名称稳定导出定义。
#[derive(Default)]
pub struct RuntimeToolExecutorRegistry {
    tools: BTreeMap<String, Arc<dyn RuntimeTool>>,
}

impl RuntimeToolExecutorRegistry {
    /// `new` 创建空的可执行工具注册表。
    pub fn new() -> Self {
        Self::default()
    }

    /// `insert` 注册或替换一个可执行工具。
    pub fn insert<T>(&mut self, tool: T)
    where
        T: RuntimeTool + 'static,
    {
        let definition = tool.definition();
        self.tools.insert(definition.name, Arc::new(tool));
    }

    /// `definitions` 返回当前可执行工具的模型可见定义。
    pub fn definitions(&self) -> RuntimeToolRegistry {
        let mut registry = RuntimeToolRegistry::new();
        for tool in self.tools.values() {
            registry.insert(tool.definition());
        }
        registry
    }
}

impl RuntimeToolExecutor for RuntimeToolExecutorRegistry {
    fn execute_tool<'a>(
        &'a self,
        call: RuntimeToolCall,
        cancellation: &'a CancellationToken,
    ) -> RuntimeToolExecutionFuture<'a> {
        let Some(tool) = self.tools.get(&call.name).cloned() else {
            return Box::pin(async move {
                RuntimeToolResult::error(
                    call.call_id,
                    format!("Tool {} is not registered", call.name),
                )
            });
        };
        let definition = tool.definition();
        if let Some(schema) = definition.input_schema.as_ref()
            && let Err(error) = validate_tool_arguments(schema, &call.arguments)
        {
            let call_id = call.call_id;
            let tool_name = call.name;
            return Box::pin(async move {
                RuntimeToolResult::error(
                    call_id,
                    format!("Tool {tool_name} arguments do not match schema: {error}"),
                )
            });
        }

        Box::pin(async move { tool.execute(call, cancellation).await })
    }
}
