use crate::tools::{RuntimeToolCall, RuntimeToolResult};

use super::{RuntimeIdentity, RuntimePermissionRequest, RuntimeRequestMetrics, RuntimeTarget};

/// `RuntimeEvent` 描述交互式 runtime 返回给 TUI 的统一事件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeEvent {
    Started {
        target: RuntimeTarget,
        identity: RuntimeIdentity,
    },
    StartFailed {
        target: Option<RuntimeTarget>,
        message: String,
    },
    SystemMessage {
        target: Option<RuntimeTarget>,
        message: String,
    },
    TurnStarted {
        target: RuntimeTarget,
        label: String,
    },
    AssistantDelta {
        target: RuntimeTarget,
        content: String,
    },
    ReasoningDelta {
        target: RuntimeTarget,
        content: String,
    },
    OutputTokenEstimate {
        target: Option<RuntimeTarget>,
        total_tokens: usize,
    },
    Thinking {
        target: Option<RuntimeTarget>,
        is_thinking: bool,
    },
    Retrying {
        target: Option<RuntimeTarget>,
        message: String,
    },
    ToolExecutionStarted {
        target: Option<RuntimeTarget>,
        call: RuntimeToolCall,
    },
    ToolExecutionFinished {
        target: Option<RuntimeTarget>,
        call: RuntimeToolCall,
        result: RuntimeToolResult,
    },
    PermissionRequested {
        target: RuntimeTarget,
        request: RuntimePermissionRequest,
    },
    PermissionCancelled {
        target: RuntimeTarget,
        request_id: Option<String>,
    },
    MessageFinished {
        target: Option<RuntimeTarget>,
        content: String,
        reasoning_content: Option<String>,
        reasoning_duration: Option<std::time::Duration>,
        metrics: Option<RuntimeRequestMetrics>,
    },
    Failed {
        target: Option<RuntimeTarget>,
        message: String,
    },
    Interrupted {
        target: Option<RuntimeTarget>,
    },
    Stopped {
        target: RuntimeTarget,
        message: Option<String>,
    },
}

impl RuntimeEvent {
    /// `target` 返回事件关联的 runtime 目标。
    pub fn target(&self) -> Option<&RuntimeTarget> {
        match self {
            Self::Started { target, .. }
            | Self::TurnStarted { target, .. }
            | Self::AssistantDelta { target, .. }
            | Self::ReasoningDelta { target, .. }
            | Self::PermissionRequested { target, .. }
            | Self::PermissionCancelled { target, .. }
            | Self::Stopped { target, .. } => Some(target),
            Self::MessageFinished { target, .. } => target.as_ref(),
            Self::StartFailed { target, .. }
            | Self::SystemMessage { target, .. }
            | Self::OutputTokenEstimate { target, .. }
            | Self::Thinking { target, .. }
            | Self::Retrying { target, .. }
            | Self::ToolExecutionStarted { target, .. }
            | Self::ToolExecutionFinished { target, .. }
            | Self::Failed { target, .. }
            | Self::Interrupted { target, .. } => target.as_ref(),
        }
    }
}
