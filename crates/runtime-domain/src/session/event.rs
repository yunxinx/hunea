use crate::context_budget::SegmentKind;

use super::{
    ConversationResponse, MessageHistoryEntry, MessageHistoryEntryId, MessageHistoryRow,
    RuntimeIdentity, RuntimePermissionRequest, RuntimeRequestMetrics, RuntimeTarget,
    RuntimeTerminalSnapshot, RuntimeToolActivity, RuntimeToolActivityUpdate,
    SessionBranchTreePayload, SessionLoadRequestId, SessionPickerRow, SessionPreviewPayload,
    SessionResumePayload, SessionTreePayload,
};

/// Context budget snapshot payload for the `/context` overlay.
#[derive(Debug, Clone, PartialEq)]
pub struct ContextBudgetSnapshotPayload {
    pub model_id: String,
    pub segments: Vec<ContextBudgetSegmentPayload>,
    pub total_estimated_tokens: usize,
    pub context_limit: Option<u32>,
    pub display: ContextBudgetDisplayPayload,
}

/// One segment in a context budget snapshot event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextBudgetSegmentPayload {
    pub kind: SegmentKind,
    pub stack_order: u16,
    pub estimated_tokens: usize,
    pub label: String,
}

/// Display mode for context budget header and legend.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ContextBudgetDisplayPayload {
    Relative { used: u32 },
    Absolute { limit: u32, used: u32, percent: f32 },
}

/// `RuntimeEvent` 描述交互式 runtime 返回给 TUI 的统一事件。
#[derive(Debug, Clone, PartialEq)]
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
    InputTokenEstimate {
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
    ToolActivityStarted {
        target: RuntimeTarget,
        activity: RuntimeToolActivity,
    },
    ToolActivityUpdated {
        target: RuntimeTarget,
        update: RuntimeToolActivityUpdate,
    },
    TerminalUpdated {
        target: RuntimeTarget,
        snapshot: RuntimeTerminalSnapshot,
    },
    PermissionRequested {
        target: RuntimeTarget,
        request: RuntimePermissionRequest,
    },
    PermissionCancelled {
        target: RuntimeTarget,
        request_id: Option<String>,
    },
    SessionListLoaded {
        rows: Vec<SessionPickerRow>,
    },
    SessionPreviewLoaded {
        payload: SessionPreviewPayload,
    },
    SessionResumed {
        payload: SessionResumePayload,
    },
    SessionTreeLoaded {
        request_id: SessionLoadRequestId,
        payload: SessionTreePayload,
    },
    SessionTreeLoadFailed {
        request_id: SessionLoadRequestId,
        message: String,
    },
    CopyPickerTreeLoaded {
        request_id: SessionLoadRequestId,
        payload: SessionTreePayload,
    },
    CopyPickerTreeLoadFailed {
        request_id: SessionLoadRequestId,
        message: String,
    },
    ContextBudgetSnapshotLoaded {
        request_id: SessionLoadRequestId,
        payload: ContextBudgetSnapshotPayload,
    },
    ContextBudgetSnapshotLoadFailed {
        request_id: SessionLoadRequestId,
        message: String,
    },
    SessionBranchTreeLoaded {
        request_id: SessionLoadRequestId,
        payload: SessionBranchTreePayload,
    },
    SessionBranchTreeLoadFailed {
        request_id: SessionLoadRequestId,
        message: String,
    },
    SessionBranchPreviewLoaded {
        request_id: SessionLoadRequestId,
        payload: SessionTreePayload,
    },
    SessionBranchPreviewLoadFailed {
        request_id: SessionLoadRequestId,
        message: String,
    },
    SessionBranchSwitchFailed {
        request_id: SessionLoadRequestId,
        message: String,
    },
    MessageHistoryStartupCacheLoaded {
        entries: Vec<MessageHistoryEntry>,
    },
    MessageHistoryStartupCacheLoadFailed {
        message: String,
    },
    MessageHistoryPickerRowsLoaded {
        request_id: SessionLoadRequestId,
        rows: Vec<MessageHistoryRow>,
    },
    MessageHistoryPickerRowsLoadFailed {
        request_id: SessionLoadRequestId,
        message: String,
    },
    MessageHistoryRecorded {
        entry_id: MessageHistoryEntryId,
    },
    MessageHistoryRecordFailed {
        entry_id: MessageHistoryEntryId,
        message: String,
    },
    MessageFinished {
        target: Option<RuntimeTarget>,
        response: ConversationResponse,
        finish_reason: Option<String>,
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
            | Self::ToolActivityStarted { target, .. }
            | Self::ToolActivityUpdated { target, .. }
            | Self::TerminalUpdated { target, .. }
            | Self::PermissionRequested { target, .. }
            | Self::PermissionCancelled { target, .. }
            | Self::Stopped { target, .. } => Some(target),
            Self::MessageFinished { target, .. } => target.as_ref(),
            Self::StartFailed { target, .. }
            | Self::SystemMessage { target, .. }
            | Self::OutputTokenEstimate { target, .. }
            | Self::InputTokenEstimate { target, .. }
            | Self::Thinking { target, .. }
            | Self::Retrying { target, .. }
            | Self::Failed { target, .. }
            | Self::Interrupted { target, .. } => target.as_ref(),
            Self::SessionListLoaded { .. }
            | Self::SessionPreviewLoaded { .. }
            | Self::SessionResumed { .. }
            | Self::SessionTreeLoaded { .. }
            | Self::SessionTreeLoadFailed { .. }
            | Self::CopyPickerTreeLoaded { .. }
            | Self::CopyPickerTreeLoadFailed { .. }
            | Self::SessionBranchTreeLoaded { .. }
            | Self::SessionBranchTreeLoadFailed { .. }
            | Self::SessionBranchPreviewLoaded { .. }
            | Self::SessionBranchPreviewLoadFailed { .. }
            | Self::SessionBranchSwitchFailed { .. }
            | Self::MessageHistoryStartupCacheLoaded { .. }
            | Self::MessageHistoryStartupCacheLoadFailed { .. }
            | Self::MessageHistoryPickerRowsLoaded { .. }
            | Self::MessageHistoryPickerRowsLoadFailed { .. }
            | Self::MessageHistoryRecorded { .. }
            | Self::MessageHistoryRecordFailed { .. }
            | Self::ContextBudgetSnapshotLoaded { .. }
            | Self::ContextBudgetSnapshotLoadFailed { .. } => None,
        }
    }
}
