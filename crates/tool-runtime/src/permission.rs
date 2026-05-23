use std::{future::Future, pin::Pin, sync::Arc};

use tokio_util::sync::CancellationToken;

use super::{ToolCall, ToolDefinition};

/// `ToolPermissionPolicy` 描述工具调用的默认许可策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ToolPermissionPolicy {
    #[default]
    Never,
    Ask,
    Always,
}

/// `ToolPermissionRequest` 描述一次需要用户确认的工具调用。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolPermissionRequest {
    pub call: ToolCall,
    pub definition: ToolDefinition,
}

impl ToolPermissionRequest {
    /// `new` 创建一次工具权限确认请求。
    pub fn new(call: ToolCall, definition: ToolDefinition) -> Self {
        Self { call, definition }
    }
}

/// `ToolPermissionDecision` 表示权限确认结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolPermissionDecision {
    Allow,
    Deny { message: String },
}

/// `ToolPermissionFuture` 是权限确认异步任务。
pub type ToolPermissionFuture<'a> =
    Pin<Box<dyn Future<Output = ToolPermissionDecision> + Send + 'a>>;

/// `ToolPermissionHandler` 负责在 Ask 工具真正执行前获取用户许可。
pub trait ToolPermissionHandler: Send + Sync {
    fn request_permission<'a>(
        &'a self,
        request: ToolPermissionRequest,
        cancellation: &'a CancellationToken,
    ) -> ToolPermissionFuture<'a>;
}

/// `SharedToolPermissionHandler` 是跨工具服务器共享的权限处理器。
pub type SharedToolPermissionHandler = Arc<dyn ToolPermissionHandler>;
