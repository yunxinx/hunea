use std::{
    collections::VecDeque,
    fmt,
    sync::{Arc, Mutex},
};

use rig_core::{
    completion::ToolDefinition as RigToolDefinition,
    tool::{
        ToolDyn, ToolError,
        server::{ToolServer, ToolServerError, ToolServerHandle},
    },
    wasm_compat::WasmBoxedFuture,
};
use tokio_util::sync::CancellationToken;

use crate::{
    SharedToolErrorFormatter, Tool, ToolCall, ToolDefinition, ToolExecutor, ToolExecutorRegistry,
    ToolRegistry, tool_error::default_tool_error_formatter,
};

const TOOL_RESULT_DETAILS_RECORD_LIMIT: usize = 128;

/// `RigToolServer` 负责把 Lumos 内部工具注册到 Rig 的 `ToolServerHandle`。
///
/// 这个类型是 Lumos 的统一工具管理层：上层只依赖 `mo-tools`，
/// 不直接接触 Rig 的适配细节。
#[derive(Clone)]
pub struct RigToolServer {
    handle: ToolServerHandle,
    executor: ToolExecutorRegistry,
    cancellation: CancellationToken,
    error_formatter: SharedToolErrorFormatter,
    result_details: SharedRigToolResultDetails,
}

impl RigToolServer {
    /// `from_executor` 使用现有工具注册表构建 Rig 工具服务器。
    pub async fn from_executor(
        executor: ToolExecutorRegistry,
        cancellation: CancellationToken,
    ) -> Result<Self, RigToolServerError> {
        Self::from_executor_with_error_formatter(
            executor,
            cancellation,
            default_tool_error_formatter(),
        )
        .await
    }

    /// `from_executor_with_error_formatter` 使用自定义错误 formatter 构建 Rig 工具服务器。
    pub async fn from_executor_with_error_formatter(
        executor: ToolExecutorRegistry,
        cancellation: CancellationToken,
        error_formatter: SharedToolErrorFormatter,
    ) -> Result<Self, RigToolServerError> {
        let handle = ToolServer::new().run();
        let result_details = SharedRigToolResultDetails::default();
        let server = Self {
            handle,
            executor,
            cancellation,
            error_formatter,
            result_details,
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

    /// `take_tool_result_details` 取出一次成功工具结果的内部 metadata。
    pub fn take_tool_result_details(
        &self,
        tool_name: &str,
        arguments: &serde_json::Value,
        content: &str,
    ) -> Option<serde_json::Value> {
        self.result_details.take(tool_name, arguments, content)
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
                error_formatter: Arc::clone(&self.error_formatter),
                result_details: self.result_details.clone(),
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
    error_formatter: SharedToolErrorFormatter,
    result_details: SharedRigToolResultDetails,
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
        let error_formatter = Arc::clone(&self.error_formatter);
        let result_details = self.result_details.clone();
        Box::pin(async move {
            if cancellation.is_cancelled() {
                return Err(ToolError::ToolCallError(Box::new(RigToolCancelled)));
            }

            let arguments: serde_json::Value =
                serde_json::from_str(&args).map_err(ToolError::JsonError)?;
            let call = ToolCall::new(tool_name.clone(), tool_name.clone(), arguments.clone());
            let result = executor.execute_tool(call, &cancellation).await;
            if result.is_error {
                let processed = error_formatter.format_tool_error(&tool_name, &result.content);
                Ok(processed.assistant_message)
            } else {
                let content = result.content;
                if let Some(details) = result.details {
                    result_details.insert(tool_name, arguments, content.clone(), details);
                }
                Ok(content)
            }
        })
    }
}

#[derive(Clone, Default)]
struct SharedRigToolResultDetails {
    records: Arc<Mutex<VecDeque<RigToolResultDetailsRecord>>>,
}

impl SharedRigToolResultDetails {
    fn insert(
        &self,
        tool_name: String,
        arguments: serde_json::Value,
        content: String,
        details: serde_json::Value,
    ) {
        let mut records = self
            .records
            .lock()
            .expect("tool result details lock should not be poisoned");
        records.push_back(RigToolResultDetailsRecord {
            tool_name,
            arguments,
            content,
            details,
        });
        while records.len() > TOOL_RESULT_DETAILS_RECORD_LIMIT {
            records.pop_front();
        }
    }

    fn take(
        &self,
        tool_name: &str,
        arguments: &serde_json::Value,
        content: &str,
    ) -> Option<serde_json::Value> {
        let mut records = self
            .records
            .lock()
            .expect("tool result details lock should not be poisoned");
        let index = records.iter().position(|record| {
            record.tool_name == tool_name
                && record.arguments == *arguments
                && record.content.as_str() == content
        })?;

        records.remove(index).map(|record| record.details)
    }
}

struct RigToolResultDetailsRecord {
    tool_name: String,
    arguments: serde_json::Value,
    content: String,
    details: serde_json::Value,
}

#[derive(Debug)]
struct RigToolCancelled;

impl fmt::Display for RigToolCancelled {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("tool call cancelled")
    }
}

impl std::error::Error for RigToolCancelled {}
