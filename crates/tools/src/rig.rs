use std::{fmt, sync::Arc};

use rig_core::{
    completion::ToolDefinition as RigToolDefinition,
    tool::{
        ToolDyn, ToolError,
        server::{ToolServer, ToolServerError, ToolServerHandle},
    },
    wasm_compat::WasmBoxedFuture,
};
use tokio_util::sync::CancellationToken;

use crate::{Tool, ToolCall, ToolDefinition, ToolExecutor, ToolExecutorRegistry, ToolRegistry};

/// `RigToolServer` 负责把 Lumos 内部工具注册到 Rig 的 `ToolServerHandle`。
///
/// 这个类型是 Lumos 的统一工具管理层：上层只依赖 `mo-tools`，
/// 不直接接触 Rig 的适配细节。
#[derive(Clone)]
pub struct RigToolServer {
    handle: ToolServerHandle,
    executor: ToolExecutorRegistry,
    cancellation: CancellationToken,
}

impl RigToolServer {
    /// `from_executor` 使用现有工具注册表构建 Rig 工具服务器。
    pub async fn from_executor(
        executor: ToolExecutorRegistry,
        cancellation: CancellationToken,
    ) -> Result<Self, RigToolServerError> {
        let handle = ToolServer::new().run();
        let server = Self {
            handle,
            executor,
            cancellation,
        };

        for tool in server.executor.tools() {
            server.register_tool(tool).await?;
        }

        Ok(server)
    }

    /// `handle` 返回共享的 Rig 工具句柄。
    pub fn handle(&self) -> &ToolServerHandle {
        &self.handle
    }

    /// `definitions` 返回当前可见工具定义。
    pub fn definitions(&self) -> ToolRegistry {
        self.executor.definitions()
    }

    /// `add_tool` 动态添加一个工具，并同步到 Rig 句柄。
    pub async fn add_tool<T>(&mut self, tool: T) -> Result<(), RigToolServerError>
    where
        T: Tool + 'static,
    {
        let definition = tool.definition();
        let tool_name = definition.name.clone();
        self.handle
            .remove_tool(&tool_name)
            .await
            .map_err(|source| RigToolServerError::Remove {
                tool_name: tool_name.clone(),
                source,
            })?;
        self.executor.remove(&tool_name);
        self.executor.insert(tool);
        if let Err(source) = self
            .register_definition(definition.clone(), self.executor.clone())
            .await
        {
            self.executor.remove(&definition.name);
            return Err(RigToolServerError::Register { tool_name, source });
        }

        Ok(())
    }

    /// `remove_tool` 从 Rig 句柄和内部注册表中移除一个工具。
    pub async fn remove_tool(&mut self, tool_name: &str) -> Result<(), RigToolServerError> {
        self.handle
            .remove_tool(tool_name)
            .await
            .map_err(|source| RigToolServerError::Remove {
                tool_name: tool_name.to_string(),
                source,
            })?;
        self.executor.remove(tool_name);
        Ok(())
    }

    async fn register_tool(&self, tool: Arc<dyn Tool>) -> Result<(), RigToolServerError> {
        let definition = tool.definition();
        let tool_name = definition.name.clone();
        self.register_definition(definition, self.executor.clone())
            .await
            .map_err(|source| RigToolServerError::Register { tool_name, source })
    }

    async fn register_definition(
        &self,
        definition: ToolDefinition,
        executor: ToolExecutorRegistry,
    ) -> Result<(), ToolServerError> {
        self.handle
            .add_tool(RigToolAdapter {
                definition,
                executor,
                cancellation: self.cancellation.clone(),
            })
            .await
    }
}

/// `RigToolServerError` 表示 Rig 工具服务器注册或移除失败。
#[derive(Debug)]
pub enum RigToolServerError {
    Register {
        tool_name: String,
        source: ToolServerError,
    },
    Remove {
        tool_name: String,
        source: ToolServerError,
    },
}

impl fmt::Display for RigToolServerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RigToolServerError::Register { tool_name, source } => {
                write!(f, "failed to register Rig tool {tool_name}: {source}")
            }
            RigToolServerError::Remove { tool_name, source } => {
                write!(f, "failed to remove Rig tool {tool_name}: {source}")
            }
        }
    }
}

impl std::error::Error for RigToolServerError {}

struct RigToolAdapter {
    definition: ToolDefinition,
    executor: ToolExecutorRegistry,
    cancellation: CancellationToken,
}

impl ToolDyn for RigToolAdapter {
    fn name(&self) -> String {
        self.definition.name.clone()
    }

    fn definition<'a>(&'a self, _prompt: String) -> WasmBoxedFuture<'a, RigToolDefinition> {
        let definition = self.definition.clone();
        Box::pin(async move {
            RigToolDefinition {
                name: definition.name,
                description: definition.description.unwrap_or_default(),
                parameters: definition
                    .input_schema
                    .unwrap_or_else(|| serde_json::json!({ "type": "object" })),
            }
        })
    }

    fn call<'a>(&'a self, args: String) -> WasmBoxedFuture<'a, Result<String, ToolError>> {
        let executor = self.executor.clone();
        let cancellation = self.cancellation.clone();
        let tool_name = self.definition.name.clone();
        Box::pin(async move {
            if cancellation.is_cancelled() {
                return Err(ToolError::ToolCallError(Box::new(RigToolCancelled)));
            }

            let arguments = serde_json::from_str(&args).map_err(ToolError::JsonError)?;
            let call = ToolCall::new(tool_name.clone(), tool_name, arguments);
            let result = executor.execute_tool(call, &cancellation).await;
            if result.is_error {
                Err(ToolError::ToolCallError(Box::new(RigToolExecutionError(
                    result.content,
                ))))
            } else {
                Ok(result.content)
            }
        })
    }
}

#[derive(Debug)]
struct RigToolExecutionError(String);

impl fmt::Display for RigToolExecutionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for RigToolExecutionError {}

#[derive(Debug)]
struct RigToolCancelled;

impl fmt::Display for RigToolCancelled {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("tool call cancelled")
    }
}

impl std::error::Error for RigToolCancelled {}
