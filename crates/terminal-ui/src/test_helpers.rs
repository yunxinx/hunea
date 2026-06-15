use ratatui::{buffer::Buffer, layout::Rect};

use runtime_domain::session::{
    SessionBranchSummary, SessionBranchTreeNode, SessionBranchTreePayload, SessionTreeBranchChoice,
    SessionTreeRow, SessionTreeRowKind, TranscriptReplayItem,
};

use crate::Model;

pub(crate) fn render_model_buffer(model: &mut Model, width: u16, height: u16) -> Buffer {
    let area = Rect::new(0, 0, width, height);
    let mut buffer = Buffer::empty(area);
    let _ = model.render_to_buffer(area, &mut buffer);
    buffer
}

pub(crate) fn rendered_rows(buffer: &Buffer) -> Vec<String> {
    (0..buffer.area.height)
        .map(|row| {
            let mut line = String::new();
            for column in 0..buffer.area.width {
                line.push_str(buffer[(column, row)].symbol());
            }
            line
        })
        .collect()
}

pub(crate) fn numbered_tree_row(index: usize) -> SessionTreeRow {
    let kind = if index.is_multiple_of(2) {
        SessionTreeRowKind::User
    } else {
        SessionTreeRowKind::Assistant
    };
    tree_row(
        &format!("row-{index}"),
        kind,
        &format!("message {index}"),
        (kind == SessionTreeRowKind::User).then(|| format!("message {index}")),
        Some(&format!("target-{index}")),
    )
}

pub(crate) fn tree_row_with_parent_at_depth(
    row_id: &str,
    parent_id: Option<&str>,
    kind: SessionTreeRowKind,
    content: &str,
    display_depth: usize,
    is_active_path: bool,
    is_current: bool,
) -> SessionTreeRow {
    SessionTreeRow {
        parent_id: parent_id.map(str::to_string),
        display_depth,
        is_active_path,
        is_current,
        ..tree_row(row_id, kind, content, None, Some(row_id))
    }
}

pub(crate) fn tree_row_with_branch_choices(
    row_id: &str,
    kind: SessionTreeRowKind,
    content: &str,
    branch_choices: Vec<SessionTreeBranchChoice>,
) -> SessionTreeRow {
    SessionTreeRow {
        branch_choices,
        ..tree_row(row_id, kind, content, None, Some(row_id))
    }
}

pub(crate) fn tree_row_with_preview_replay_items(
    row_id: &str,
    kind: SessionTreeRowKind,
    content: &str,
    preview_replay_items: Vec<TranscriptReplayItem>,
) -> SessionTreeRow {
    SessionTreeRow {
        preview_replay_items,
        ..tree_row(row_id, kind, content, None, Some(row_id))
    }
}

pub(crate) fn branch_choice(
    branch_row_id: &str,
    subtree_leaf_id: &str,
    summary: &str,
    is_current: bool,
) -> SessionTreeBranchChoice {
    branch_choice_with_metadata(
        branch_row_id,
        subtree_leaf_id,
        summary,
        is_current,
        1,
        1_717_514_800_000,
        1_717_514_800_000,
    )
}

pub(crate) fn branch_choice_with_metadata(
    branch_row_id: &str,
    subtree_leaf_id: &str,
    summary: &str,
    is_current: bool,
    message_count: usize,
    branch_created_at_ms: i64,
    latest_updated_at_ms: i64,
) -> SessionTreeBranchChoice {
    SessionTreeBranchChoice {
        branch: branch_summary(
            branch_row_id,
            subtree_leaf_id,
            summary,
            is_current,
            message_count,
            branch_created_at_ms,
            latest_updated_at_ms,
        ),
    }
}

pub(crate) fn branch_tree_payload() -> SessionBranchTreePayload {
    SessionBranchTreePayload {
        nodes: vec![
            branch_tree_node("root-a", None, "leaf-root-a", "root branch", false, 6),
            branch_tree_node(
                "child-one",
                Some("root-a"),
                "leaf-child-one",
                "child one",
                false,
                2,
            ),
            branch_tree_node(
                "child-two",
                Some("root-a"),
                "leaf-child-two",
                "child two",
                true,
                3,
            ),
            branch_tree_node(
                "grand-child",
                Some("child-two"),
                "leaf-grand-child",
                "grand child",
                false,
                1,
            ),
            branch_tree_node("root-b", None, "leaf-root-b", "second root", false, 3),
        ],
        current_branch_row_id: Some("child-two".to_string()),
        total_message_count: 9,
    }
}

fn branch_tree_node(
    branch_row_id: &str,
    parent_branch_row_id: Option<&str>,
    subtree_leaf_id: &str,
    summary: &str,
    is_current: bool,
    message_count: usize,
) -> SessionBranchTreeNode {
    SessionBranchTreeNode {
        parent_branch_row_id: parent_branch_row_id.map(str::to_string),
        branch: branch_summary(
            branch_row_id,
            subtree_leaf_id,
            summary,
            is_current,
            message_count,
            1_717_514_800_000,
            1_717_514_800_000,
        ),
    }
}

fn branch_summary(
    branch_row_id: &str,
    subtree_leaf_id: &str,
    summary: &str,
    is_current: bool,
    message_count: usize,
    branch_created_at_ms: i64,
    latest_updated_at_ms: i64,
) -> SessionBranchSummary {
    SessionBranchSummary {
        branch_row_id: branch_row_id.to_string(),
        subtree_leaf_id: subtree_leaf_id.to_string(),
        latest_row_id: subtree_leaf_id.to_string(),
        kind: SessionTreeRowKind::Assistant,
        display_summary: summary.to_string(),
        preview_content: summary.to_string(),
        is_current,
        message_count,
        branch_created_at_ms,
        latest_updated_at_ms,
    }
}

pub(crate) fn tree_row(
    row_id: &str,
    kind: SessionTreeRowKind,
    content: &str,
    rewind_prefill: Option<String>,
    rewind_target_id: Option<&str>,
) -> SessionTreeRow {
    SessionTreeRow {
        row_id: row_id.to_string(),
        parent_id: None,
        display_depth: 0,
        kind,
        display_text: content.split_whitespace().collect::<Vec<_>>().join(" "),
        summary: content.split_whitespace().collect::<Vec<_>>().join(" "),
        preview_content: content.to_string(),
        preview_replay_items: Vec::new(),
        rewind_target_id: rewind_target_id.map(str::to_string),
        rewind_prefill,
        is_active_path: true,
        is_current: false,
        branch_choices: Vec::new(),
    }
}
