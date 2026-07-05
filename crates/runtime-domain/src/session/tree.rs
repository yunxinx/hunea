use super::transcript_replay::TranscriptReplayItem;

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
