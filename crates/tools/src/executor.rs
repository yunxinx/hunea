use std::{
    collections::BTreeMap,
    future::Future,
    pin::Pin,
    sync::{Arc, RwLock},
};

use tokio_util::sync::CancellationToken;

use super::{ToolCall, ToolDefinition, ToolRegistry, ToolResult, schema::validate_tool_arguments};

/// `ToolExecutionFuture` 是工具执行返回结果的异步任务。
pub type ToolExecutionFuture<'a> = Pin<Box<dyn Future<Output = ToolResult> + Send + 'a>>;

/// `Tool` 描述一个可执行的 runtime tool。
pub trait Tool: Send + Sync {
    /// `definition` 返回暴露给模型与 UI 的工具定义。
    fn definition(&self) -> ToolDefinition;

    /// `execute` 执行一次模型发起的工具调用。
    fn execute<'a>(
        &'a self,
        call: ToolCall,
        cancellation: &'a CancellationToken,
    ) -> ToolExecutionFuture<'a>;
}

/// `ToolExecutor` 是 runtime/agent 调用工具时依赖的最小执行边界。
pub trait ToolExecutor: Send + Sync {
    /// `execute_tool` 执行一次工具调用，并返回可回传给模型的结果。
    fn execute_tool<'a>(
        &'a self,
        call: ToolCall,
        cancellation: &'a CancellationToken,
    ) -> ToolExecutionFuture<'a>;
}

/// `ToolExecutorRegistry` 保存可执行工具，并按名称稳定导出定义。
#[derive(Clone, Default)]
pub struct ToolExecutorRegistry {
    tools: Arc<RwLock<BTreeMap<String, Arc<dyn Tool>>>>,
}

impl ToolExecutorRegistry {
    /// `new` 创建空的可执行工具注册表。
    pub fn new() -> Self {
        Self::default()
    }

    /// `insert` 注册或替换一个可执行工具。
    pub fn insert<T>(&mut self, tool: T)
    where
        T: Tool + 'static,
    {
        let definition = tool.definition();
        self.tools
            .write()
            .expect("tool registry lock should not be poisoned")
            .insert(definition.name, Arc::new(tool));
    }

    /// `remove` 删除一个可执行工具；若不存在则返回 `None`。
    pub fn remove(&mut self, tool_name: &str) -> Option<Arc<dyn Tool>> {
        self.tools
            .write()
            .expect("tool registry lock should not be poisoned")
            .remove(tool_name)
    }

    /// `definitions` 返回当前可执行工具的模型可见定义。
    pub fn definitions(&self) -> ToolRegistry {
        let mut registry = ToolRegistry::new();
        for tool in self
            .tools
            .read()
            .expect("tool registry lock should not be poisoned")
            .values()
        {
            registry.insert(tool.definition());
        }
        registry
    }

    /// `tools` 返回当前注册的工具，供 Rig 适配层按名称注册。
    pub fn tools(&self) -> Vec<Arc<dyn Tool>> {
        self.tools
            .read()
            .expect("tool registry lock should not be poisoned")
            .values()
            .cloned()
            .collect()
    }
}

impl ToolExecutor for ToolExecutorRegistry {
    fn execute_tool<'a>(
        &'a self,
        call: ToolCall,
        cancellation: &'a CancellationToken,
    ) -> ToolExecutionFuture<'a> {
        let tool = {
            self.tools
                .read()
                .expect("tool registry lock should not be poisoned")
                .get(&call.name)
                .cloned()
        };

        let Some(tool) = tool else {
            return Box::pin(async move {
                ToolResult::error(
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
                ToolResult::error(
                    call_id,
                    format!("Tool {tool_name} arguments do not match schema: {error}"),
                )
            });
        }

        Box::pin(async move { tool.execute(call, cancellation).await })
    }
}
