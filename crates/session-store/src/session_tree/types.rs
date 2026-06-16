use std::collections::HashSet;

use runtime_domain::session::{
    SessionBranchTreeNode as DomainSessionBranchTreeNode,
    SessionTreeBranchChoice as DomainSessionTreeBranchChoice,
    SessionTreeRowKind as DomainSessionTreeRowKind, TranscriptReplayItem,
};

/// `SessionTreeSnapshot` 是 `/tree` 所需的逻辑消息行投影。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SessionTreeSnapshot {
    pub rows: Vec<SessionTreeSnapshotRow>,
    pub current_row_id: Option<String>,
    pub active_row_ids: HashSet<String>,
}

/// `SessionBranchTreeSnapshot` 是完整 branch 拓扑视图所需的数据投影。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SessionBranchTreeSnapshot {
    pub nodes: Vec<SessionBranchTreeSnapshotNode>,
    pub current_branch_row_id: Option<String>,
    pub total_message_count: usize,
}

/// `SessionBranchTreeSnapshotNode` 描述 branch tree 中一个可切换的 branch root。
pub type SessionBranchTreeSnapshotNode = DomainSessionBranchTreeNode;

/// `SessionTreeSnapshotRow` 描述单条用户可见逻辑消息的树展示与回溯语义。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionTreeSnapshotRow {
    pub id: String,
    pub parent_id: Option<String>,
    pub display_depth: usize,
    pub kind: SessionTreeSnapshotRowKind,
    pub display_text: String,
    pub summary: String,
    pub preview_content: String,
    pub preview_replay_items: Vec<TranscriptReplayItem>,
    pub rewind_target_id: Option<String>,
    pub rewind_prefill: Option<String>,
    pub branch_choices: Vec<SessionTreeSnapshotBranchChoice>,
}

/// `SessionTreeSnapshotBranchChoice` 描述 fork parent 下可切换的 sibling branch。
pub type SessionTreeSnapshotBranchChoice = DomainSessionTreeBranchChoice;

/// `SessionTreeSnapshotRowKind` 是 session-store 层的逻辑消息行类型分类。
pub type SessionTreeSnapshotRowKind = DomainSessionTreeRowKind;
