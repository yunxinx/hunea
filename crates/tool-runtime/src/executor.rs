use std::{
    collections::BTreeMap,
    future::Future,
    pin::Pin,
    sync::{Arc, RwLock},
};

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::{
    SharedToolPermissionHandler, ToolCall, ToolDefinition, ToolPermissionDecision,
    ToolPermissionFileSnapshot, ToolPermissionPreview, ToolPermissionRequest, ToolRegistry,
    ToolResult, schema::validate_tool_arguments,
};

/// `ToolExecutionFuture` 是工具执行返回结果的异步任务。
pub type ToolExecutionFuture<'a> = Pin<Box<dyn Future<Output = ToolResult> + Send + 'a>>;

/// `ToolProgress` 描述工具执行期间可向 runtime/TUI 流式更新的事件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolProgress {
    SystemMessage { message: String },
    TerminalUpdated { snapshot: ToolTerminalSnapshot },
}

/// `ToolTerminalSnapshot` 描述执行类工具的 terminal 输出快照。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolTerminalSnapshot {
    pub terminal_id: String,
    pub command: Option<String>,
    pub cwd: Option<String>,
    pub output: String,
    pub truncated: bool,
    pub exit_status: Option<ToolTerminalExitStatus>,
    pub released: bool,
}

/// `ToolTerminalExitStatus` 描述执行类工具的退出状态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolTerminalExitStatus {
    pub exit_code: Option<u32>,
    pub signal: Option<String>,
}

/// `ToolProgressSink` 是工具执行期间可选的进度事件出口。
#[derive(Debug, Clone, Default)]
pub struct ToolProgressSink {
    sender: Option<mpsc::UnboundedSender<ToolProgress>>,
}

impl ToolProgressSink {
    /// `none` 创建不发送任何事件的进度出口。
    pub const fn none() -> Self {
        Self { sender: None }
    }

    /// `from_sender` 从 channel sender 创建进度出口。
    pub fn from_sender(sender: mpsc::UnboundedSender<ToolProgress>) -> Self {
        Self {
            sender: Some(sender),
        }
    }

    /// `emit` 尝试发送工具进度；消费端已释放时静默丢弃。
    pub fn emit(&self, progress: ToolProgress) {
        if let Some(sender) = &self.sender {
            let _ = sender.send(progress);
        }
    }
}

/// `ToolExecutionContext` 保存一次工具调用的取消与进度上报上下文。
#[derive(Clone)]
pub struct ToolExecutionContext<'a> {
    cancellation: &'a CancellationToken,
    progress_sink: ToolProgressSink,
    permission_snapshot: Option<ToolPermissionFileSnapshot>,
    permission_handler: Option<SharedToolPermissionHandler>,
}

impl<'a> ToolExecutionContext<'a> {
    /// `new` 创建不带进度出口的工具执行上下文。
    pub fn new(cancellation: &'a CancellationToken) -> Self {
        Self {
            cancellation,
            progress_sink: ToolProgressSink::none(),
            permission_snapshot: None,
            permission_handler: None,
        }
    }

    /// `with_progress_sink` 设置工具执行期间的进度出口。
    pub fn with_progress_sink(mut self, progress_sink: ToolProgressSink) -> Self {
        self.progress_sink = progress_sink;
        self
    }

    /// `with_permission_snapshot` 附带本次审批预览读取到的文件指纹。
    pub fn with_permission_snapshot(
        mut self,
        snapshot: Option<ToolPermissionFileSnapshot>,
    ) -> Self {
        self.permission_snapshot = snapshot;
        self
    }

    /// `with_permission_handler` 允许工具执行中发起 runtime-owned 确认请求。
    pub fn with_permission_handler(
        mut self,
        permission_handler: Option<SharedToolPermissionHandler>,
    ) -> Self {
        self.permission_handler = permission_handler;
        self
    }

    /// `cancellation` 返回本次工具调用的取消 token。
    pub const fn cancellation(&self) -> &'a CancellationToken {
        self.cancellation
    }

    /// `permission_snapshot` 返回用户审批时看到的文件指纹。
    pub const fn permission_snapshot(&self) -> Option<&ToolPermissionFileSnapshot> {
        self.permission_snapshot.as_ref()
    }

    /// `emit` 向 runtime 发送一次工具进度事件。
    pub fn emit(&self, progress: ToolProgress) {
        self.progress_sink.emit(progress);
    }

    /// `request_permission` 复用 runtime 权限通道，避免工具内部私建审批流程。
    pub async fn request_permission(
        &self,
        request: ToolPermissionRequest,
    ) -> ToolPermissionDecision {
        let Some(permission_handler) = self.permission_handler.as_ref() else {
            return ToolPermissionDecision::Deny {
                message: format!(
                    "Tool permission denied: {} requires approval",
                    request.definition.name
                ),
            };
        };
        permission_handler
            .request_permission(request, self.cancellation)
            .await
    }
}

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

    /// `execute_with_context` 执行工具并允许工具上报进度事件。
    fn execute_with_context<'a>(
        &'a self,
        call: ToolCall,
        context: ToolExecutionContext<'a>,
    ) -> ToolExecutionFuture<'a> {
        self.execute(call, context.cancellation())
    }

    /// `permission_preview` 返回执行前可展示的结构化变更预览。
    fn permission_preview(
        &self,
        _call: &ToolCall,
        _cancellation: &CancellationToken,
    ) -> Option<ToolPermissionPreview> {
        None
    }
}

/// `ToolExecutor` 是 runtime/agent 调用工具时依赖的最小执行边界。
pub trait ToolExecutor: Send + Sync {
    /// `execute_tool` 执行一次工具调用，并返回可回传给模型的结果。
    fn execute_tool<'a>(
        &'a self,
        call: ToolCall,
        cancellation: &'a CancellationToken,
    ) -> ToolExecutionFuture<'a> {
        self.execute_tool_with_context(call, ToolExecutionContext::new(cancellation))
    }

    /// `execute_tool_with_context` 执行工具并允许工具向 runtime 上报进度事件。
    fn execute_tool_with_context<'a>(
        &'a self,
        call: ToolCall,
        context: ToolExecutionContext<'a>,
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

    /// `tools` 返回当前注册的工具，供 runtime 按名称注册或检查。
    pub fn tools(&self) -> Vec<Arc<dyn Tool>> {
        self.tools
            .read()
            .expect("tool registry lock should not be poisoned")
            .values()
            .cloned()
            .collect()
    }

    /// `permission_preview` 读取指定工具的审批前结构化变更预览。
    pub fn permission_preview(
        &self,
        call: &ToolCall,
        cancellation: &CancellationToken,
    ) -> Option<ToolPermissionPreview> {
        let tool = {
            self.tools
                .read()
                .expect("tool registry lock should not be poisoned")
                .get(&call.name)
                .cloned()
        }?;

        tool.permission_preview(call, cancellation)
    }
}

impl ToolExecutor for ToolExecutorRegistry {
    fn execute_tool_with_context<'a>(
        &'a self,
        call: ToolCall,
        context: ToolExecutionContext<'a>,
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

        Box::pin(async move { tool.execute_with_context(call, context).await })
    }
}
