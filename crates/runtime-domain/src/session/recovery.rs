use crate::model_catalog::ModelSelection;
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
    pub rewind_target_id: Option<String>,
    pub rewind_prefill: Option<String>,
    pub is_active_path: bool,
    pub is_current: bool,
}

/// `SessionTreePayload` 是当前 session 逻辑消息树的 TUI 展示数据。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionTreePayload {
    pub rows: Vec<SessionTreeRow>,
}
