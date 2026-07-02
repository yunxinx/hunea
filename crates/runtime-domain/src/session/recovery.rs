use crate::model_catalog::ModelSelection;
use crate::prompt_assembly::PromptSourceOrigin;
use serde::{Deserialize, Serialize};

use super::activity::{RuntimeTerminalSnapshot, RuntimeToolActivity};

/// `SessionPickerRow` 是 TUI session picker 展示与选择所需的 session 摘要。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionPickerRow {
    pub session_id: String,
    pub title: String,
    pub first_user_message: String,
    pub last_assistant_message: String,
    pub updated_at_ms: i64,
    pub work_dir: String,
    pub size_bytes: Option<u64>,
    pub model: Option<String>,
}

/// `TranscriptReplayItem` 表示从 canonical session history 重建 TUI transcript 的语义项。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum TranscriptReplayItem {
    Message {
        role: TranscriptReplayRole,
        content: String,
    },
    BoundUserMessage {
        message: TranscriptUserMessage,
    },
    Reasoning {
        content: String,
    },
    ToolActivity {
        activity: RuntimeToolActivity,
    },
    TerminalSnapshot {
        snapshot: RuntimeTerminalSnapshot,
    },
    ToolResult {
        content: String,
    },
    System {
        content: String,
    },
}

impl TranscriptReplayItem {
    /// `content_text` 返回该 replay 项适合测试和搜索使用的主文本。
    pub fn content_text(&self) -> &str {
        match self {
            Self::Message { content, .. }
            | Self::BoundUserMessage {
                message: TranscriptUserMessage { content, .. },
            }
            | Self::Reasoning { content }
            | Self::ToolResult { content }
            | Self::System { content } => content,
            Self::ToolActivity { activity } => &activity.title,
            Self::TerminalSnapshot { snapshot } => snapshot
                .command
                .as_deref()
                .filter(|command| !command.is_empty())
                .or_else(|| (!snapshot.output.is_empty()).then_some(snapshot.output.as_str()))
                .unwrap_or(snapshot.terminal_id.as_str()),
        }
    }
}

/// `TranscriptSkillBinding` 表示一次 user transcript 中仍可恢复的 `$skill` 结构化绑定。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TranscriptSkillBinding {
    pub skill_name: String,
    pub origin: PromptSourceOrigin,
    pub skill_path: String,
    pub start_char: usize,
    pub end_char: usize,
}

impl TranscriptSkillBinding {
    /// `visible_token_text` 返回 transcript 中应当出现的 `$skill` 可见 token。
    #[must_use]
    pub fn visible_token_text(&self) -> String {
        format!("${}", self.skill_name)
    }
}

/// `TranscriptCustomPromptBinding` 表示一次 user transcript 中仍可恢复的 `#prompt` 结构化绑定。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TranscriptCustomPromptBinding {
    pub reference_id: String,
    pub origin: PromptSourceOrigin,
    pub start_char: usize,
    pub end_char: usize,
}

impl TranscriptCustomPromptBinding {
    /// `visible_token_text` 返回 transcript 中应当出现的 `#prompt` 可见 token。
    #[must_use]
    pub fn visible_token_text(&self) -> String {
        format!("#{}", self.reference_id)
    }
}

/// `TranscriptUserMessage` 表示 transcript-visible 的用户消息及其可选结构化绑定。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct TranscriptUserMessage {
    pub content: String,
    #[serde(default)]
    pub skill_bindings: Vec<TranscriptSkillBinding>,
    #[serde(default)]
    pub custom_prompt_bindings: Vec<TranscriptCustomPromptBinding>,
}

/// `TranscriptReplayRole` 是恢复普通消息时可见消息的角色。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TranscriptReplayRole {
    User,
    Assistant,
}

/// `SessionResumePayload` 是 runtime 恢复 session 后返回给 TUI 的完整可见状态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionResumePayload {
    pub session_id: String,
    pub transcript: Vec<TranscriptReplayItem>,
    pub restored_model: Option<ModelSelection>,
}

/// `SessionPreviewPayload` 是 resume picker 预览 session 所需的完整可见 transcript。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionPreviewPayload {
    pub session_id: String,
    pub transcript: Vec<TranscriptReplayItem>,
}

/// `SessionTreeRowKind` 描述 `/tree` 逻辑消息行的类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionTreeRowKind {
    User,
    Assistant,
    Tool,
    Reasoning,
}

/// `SessionTreeRow` 是 `/tree` 的扁平逻辑展示节点。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionTreeRow {
    pub row_id: String,
    pub parent_id: Option<String>,
    pub display_depth: usize,
    pub kind: SessionTreeRowKind,
    pub display_text: String,
    pub summary: String,
    pub preview_content: String,
    pub preview_replay_items: Vec<TranscriptReplayItem>,
    pub rewind_target_id: Option<String>,
    pub rewind_prefill: Option<String>,
    pub is_active_path: bool,
    pub is_current: bool,
    pub branch_choices: Vec<SessionTreeBranchChoice>,
}

/// `SessionBranchSummary` 是 branch picker 与 branch tree 共享的 branch 摘要。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionBranchSummary {
    pub branch_row_id: String,
    pub subtree_leaf_id: String,
    pub latest_row_id: String,
    pub kind: SessionTreeRowKind,
    pub display_summary: String,
    pub preview_content: String,
    pub is_current: bool,
    pub message_count: usize,
    pub branch_created_at_ms: i64,
    pub latest_updated_at_ms: i64,
}

/// `SessionTreeBranchChoice` 是 branch picker 展示与 switch branch 所需的 sibling 摘要。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionTreeBranchChoice {
    pub branch: SessionBranchSummary,
}

/// `SessionBranchTreeNode` 是 branch tree 视图中的一个 branch root 节点。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionBranchTreeNode {
    pub parent_branch_row_id: Option<String>,
    pub branch: SessionBranchSummary,
}

/// `SessionBranchTreePayload` 是完整 branch 拓扑视图的 TUI 展示数据。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionBranchTreePayload {
    pub nodes: Vec<SessionBranchTreeNode>,
    pub current_branch_row_id: Option<String>,
    pub total_message_count: usize,
}

/// `SessionTreePayload` 是当前 session 逻辑消息树的 TUI 展示数据。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionTreePayload {
    pub rows: Vec<SessionTreeRow>,
    pub current_row_id: Option<String>,
}
