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

/// `TranscriptReplayItem` 表示从 canonical session history 重建 TUI transcript 的一项。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscriptReplayItem {
    pub role: TranscriptReplayRole,
    pub content: String,
}

/// `TranscriptReplayRole` 是恢复 transcript 时可见消息的角色。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TranscriptReplayRole {
    User,
    Assistant,
    System,
    Tool,
}

/// `SessionResumePayload` 是 runtime 恢复 session 后返回给 TUI 的完整可见状态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionResumePayload {
    pub session_id: String,
    pub transcript: Vec<TranscriptReplayItem>,
    pub restored_model: Option<String>,
    pub missing_model: Option<String>,
}

/// `SessionPreviewPayload` 是 resume picker 预览 session 所需的完整可见 transcript。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionPreviewPayload {
    pub session_id: String,
    pub transcript: Vec<TranscriptReplayItem>,
}

/// `SessionTreeEntryKind` 描述 entry rewind tree 中一行 entry 的类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionTreeEntryKind {
    Header,
    User,
    Assistant,
    Tool,
    Reasoning,
    Config,
    Leaf,
    Other,
}

/// `SessionTreeEntry` 是 entry rewind tree 的扁平展示节点。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionTreeEntry {
    pub entry_id: String,
    pub parent_id: Option<String>,
    pub depth: usize,
    pub kind: SessionTreeEntryKind,
    pub label: String,
    pub content: String,
    pub rewind_target_id: Option<String>,
    pub rewind_prefill: Option<String>,
    pub is_active_path: bool,
    pub is_current_leaf: bool,
}

/// `SessionTreePayload` 是当前 session entry tree 的 TUI 展示数据。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionTreePayload {
    pub entries: Vec<SessionTreeEntry>,
}
