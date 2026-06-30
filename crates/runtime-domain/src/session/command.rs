use super::{ConversationTurnRequest, MessageHistoryEntryId, RuntimeTarget, SessionLoadRequestId};
use crate::prompt_assembly::PromptAssemblyMutation;

/// `RuntimeCommand` 描述 TUI 向交互式 runtime 发出的统一命令。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeCommand {
    SubmitConversationTurn {
        target: RuntimeTarget,
        request: ConversationTurnRequest,
    },
    TruncateConversation {
        retained_user_turns: usize,
    },
    Interrupt {
        target: Option<RuntimeTarget>,
    },
    RespondPermission {
        target: Option<RuntimeTarget>,
        request_id: String,
        option_id: Option<String>,
    },
    ListSessions,
    LoadSessionPreview {
        session_id: String,
    },
    ResumeSession {
        session_id: String,
    },
    LoadEntryTree {
        request_id: SessionLoadRequestId,
    },
    LoadCopyPickerTree {
        request_id: SessionLoadRequestId,
    },
    LoadContextBudgetSnapshot {
        request_id: SessionLoadRequestId,
        selection: crate::model_catalog::ModelSelection,
    },
    CancelContextBudgetSnapshot,
    LoadBranchTree {
        request_id: SessionLoadRequestId,
    },
    LoadBranchPreview {
        request_id: SessionLoadRequestId,
        branch_row_id: String,
    },
    SwitchBranch {
        request_id: SessionLoadRequestId,
        leaf_id: String,
    },
    SelectEntryRewind {
        entry_id: String,
    },
    LoadMessageHistoryStartupCache,
    LoadMessageHistoryPickerRows {
        request_id: SessionLoadRequestId,
    },
    RecordMessageHistory {
        entry_id: MessageHistoryEntryId,
        text: String,
        limit: usize,
    },
    MutatePromptAssembly {
        mutation: PromptAssemblyMutation,
    },
    Reset,
}

impl RuntimeCommand {
    /// `submit_conversation_turn` 创建对话轮次提交命令。
    pub fn submit_conversation_turn(request: ConversationTurnRequest) -> Self {
        Self::SubmitConversationTurn {
            target: request.target(),
            request,
        }
    }

    /// `truncate_conversation` 创建 provider-visible 对话历史截断命令。
    pub fn truncate_conversation(retained_user_turns: usize) -> Self {
        Self::TruncateConversation {
            retained_user_turns,
        }
    }

    /// `interrupt_current` 创建打断当前 turn 的命令。
    pub fn interrupt_current() -> Self {
        Self::Interrupt { target: None }
    }

    /// `respond_permission` 创建权限确认响应命令。
    pub fn respond_permission(
        target: RuntimeTarget,
        request_id: impl Into<String>,
        option_id: Option<String>,
    ) -> Self {
        Self::RespondPermission {
            target: Some(target),
            request_id: request_id.into(),
            option_id,
        }
    }

    /// `target` 返回命令关联的 runtime 目标。
    pub fn target(&self) -> Option<&RuntimeTarget> {
        match self {
            Self::SubmitConversationTurn { target, .. }
            | Self::Interrupt {
                target: Some(target),
            }
            | Self::RespondPermission {
                target: Some(target),
                ..
            } => Some(target),
            Self::Interrupt { target: None }
            | Self::RespondPermission { target: None, .. }
            | Self::TruncateConversation { .. }
            | Self::ListSessions
            | Self::LoadSessionPreview { .. }
            | Self::ResumeSession { .. }
            | Self::LoadEntryTree { .. }
            | Self::LoadCopyPickerTree { .. }
            | Self::LoadContextBudgetSnapshot { .. }
            | Self::CancelContextBudgetSnapshot
            | Self::LoadBranchTree { .. }
            | Self::LoadBranchPreview { .. }
            | Self::SwitchBranch { .. }
            | Self::SelectEntryRewind { .. }
            | Self::LoadMessageHistoryStartupCache
            | Self::LoadMessageHistoryPickerRows { .. }
            | Self::RecordMessageHistory { .. }
            | Self::MutatePromptAssembly { .. }
            | Self::Reset => None,
        }
    }
}

/// `RuntimeCommandReceipt` 描述 runtime coordinator 接受命令后的同步结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeCommandReceipt {
    Accepted,
    ConversationStarted { activity_label: String },
    Interrupted { target: Option<RuntimeTarget> },
}
