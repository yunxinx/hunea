use std::collections::{HashMap, HashSet};

use crate::{ResolveError, SessionEntry, SessionEntryKind, util::truncate_chars_with_ellipsis};

use super::{
    branch::session_tree_branch_choices_by_parent,
    preview::{
        SESSION_TREE_SUMMARY_CHAR_LIMIT, session_tree_row_kind, session_tree_row_preview_content,
        session_tree_row_preview_replay_items, single_line_display_text,
    },
    rewind::{
        restore_target_for_visible_row, rewind_target_for_tree_row,
        tool_call_batch_rewind_targets_for_path,
    },
    topology::{
        build_children_by_parent, build_entry_index, latest_non_leaf_descendant,
        nearest_visible_parent_id, resolve_path, validate_entry_parents,
    },
    types::{
        SessionTreeSnapshot, SessionTreeSnapshotBranchChoice, SessionTreeSnapshotRow,
        SessionTreeSnapshotRowKind,
    },
};

/// 生成 `/tree` 使用的逻辑消息行快照。
pub fn session_tree_snapshot(
    entries: &[SessionEntry],
) -> Result<SessionTreeSnapshot, ResolveError> {
    session_tree_snapshot_for_requested_leaf(entries, entries.last().map(|entry| entry.id.as_str()))
}

/// 生成给定 leaf 的 `/tree` 逻辑消息行快照，用于 branch preview 与 switch 后等价性校验。
pub fn session_tree_snapshot_for_leaf(
    entries: &[SessionEntry],
    leaf_id: &str,
) -> Result<SessionTreeSnapshot, ResolveError> {
    session_tree_snapshot_for_requested_leaf(entries, Some(leaf_id))
}

/// 生成给定 branch root 的增量预览快照。
///
/// 与 switch branch 使用的完整 leaf path 不同，branch preview 从最近的可见 fork parent
/// 开始；顶层 branch 没有可见父节点时，从 branch root 自身开始。
pub fn session_branch_preview_snapshot(
    entries: &[SessionEntry],
    branch_row_id: &str,
) -> Result<SessionTreeSnapshot, ResolveError> {
    let by_id = build_entry_index(entries)?;
    validate_entry_parents(entries, &by_id)?;
    let visible_entry_ids = entries
        .iter()
        .filter(|entry| session_tree_row_kind(entry).is_some())
        .map(|entry| entry.id.as_str())
        .collect::<HashSet<_>>();
    let branch_root = by_id
        .get(branch_row_id)
        .copied()
        .filter(|entry| visible_entry_ids.contains(entry.id.as_str()))
        .ok_or_else(|| ResolveError::LeafNotFound(branch_row_id.to_string()))?;
    let subtree_leaf = latest_non_leaf_descendant(entries, &by_id, branch_root.id.as_str())?;
    let start_row_id = nearest_visible_parent_id(branch_root, &by_id, &visible_entry_ids)?
        .unwrap_or_else(|| branch_root.id.clone());

    session_tree_snapshot_for_requested_leaf_from_start(
        entries,
        Some(subtree_leaf.id.as_str()),
        Some(start_row_id.as_str()),
    )
}

fn session_tree_snapshot_for_requested_leaf(
    entries: &[SessionEntry],
    requested_leaf_id: Option<&str>,
) -> Result<SessionTreeSnapshot, ResolveError> {
    session_tree_snapshot_for_requested_leaf_from_start(entries, requested_leaf_id, None)
}

fn session_tree_snapshot_for_requested_leaf_from_start(
    entries: &[SessionEntry],
    requested_leaf_id: Option<&str>,
    start_visible_row_id: Option<&str>,
) -> Result<SessionTreeSnapshot, ResolveError> {
    let by_id = build_entry_index(entries)?;
    validate_entry_parents(entries, &by_id)?;
    let children_by_parent = build_children_by_parent(entries);
    let active_path = if let Some(leaf_id) = requested_leaf_id {
        resolve_path(&by_id, leaf_id)?
    } else {
        Vec::new()
    };
    let active_path_ids = active_path
        .iter()
        .map(|entry| entry.id.as_str())
        .collect::<HashSet<_>>();
    let visible_entry_ids = entries
        .iter()
        .filter(|entry| session_tree_row_kind(entry).is_some())
        .map(|entry| entry.id.as_str())
        .collect::<HashSet<_>>();
    let path_visible_entry_ids = active_path
        .iter()
        .filter(|entry| visible_entry_ids.contains(entry.id.as_str()))
        .map(|entry| entry.id.as_str())
        .collect::<HashSet<_>>();
    let start_path_index = start_visible_row_id
        .and_then(|start_row_id| {
            active_path.iter().position(|entry| {
                entry.id == start_row_id && visible_entry_ids.contains(start_row_id)
            })
        })
        .unwrap_or_default();
    let visible_path_entries = active_path
        .iter()
        .skip(start_path_index)
        .copied()
        .filter(|entry| path_visible_entry_ids.contains(entry.id.as_str()))
        .collect::<Vec<_>>();
    let rewind_target_by_id = entries
        .iter()
        .filter(|entry| visible_entry_ids.contains(entry.id.as_str()))
        .map(|entry| {
            (
                entry.id.clone(),
                restore_target_for_visible_row(
                    entry,
                    &visible_entry_ids,
                    &children_by_parent,
                    &active_path_ids,
                ),
            )
        })
        .collect::<HashMap<_, _>>();
    let tool_call_batch_rewind_target_by_id =
        tool_call_batch_rewind_targets_for_path(&active_path, &rewind_target_by_id);
    let branch_choices_by_parent = session_tree_branch_choices_by_parent(
        entries,
        &by_id,
        &visible_entry_ids,
        &active_path_ids,
        &children_by_parent,
    )?;

    let mut rows = visible_path_entries
        .iter()
        .copied()
        .filter_map(|entry| {
            session_tree_snapshot_row(
                entry,
                &by_id,
                &visible_entry_ids,
                &rewind_target_by_id,
                &tool_call_batch_rewind_target_by_id,
                &children_by_parent,
                branch_choices_by_parent
                    .get(entry.id.as_str())
                    .cloned()
                    .unwrap_or_default(),
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    let display_depth_by_id = visible_display_depths(entries, &visible_entry_ids);
    let start_display_depth = if start_visible_row_id.is_some() {
        rows.first()
            .and_then(|row| display_depth_by_id.get(&row.id))
            .copied()
            .unwrap_or_default()
    } else {
        0
    };
    for row in &mut rows {
        row.display_depth = display_depth_by_id
            .get(&row.id)
            .copied()
            .unwrap_or_default()
            .saturating_sub(start_display_depth);
    }

    let active_row_ids = visible_path_entries
        .iter()
        .map(|entry| entry.id.clone())
        .collect::<HashSet<_>>();
    let current_row_id = visible_path_entries
        .iter()
        .rev()
        .map(|entry| entry.id.clone())
        .next();

    Ok(SessionTreeSnapshot {
        rows,
        current_row_id,
        active_row_ids,
    })
}

fn session_tree_snapshot_row(
    entry: &SessionEntry,
    by_id: &HashMap<&str, &SessionEntry>,
    visible_entry_ids: &HashSet<&str>,
    rewind_target_by_id: &HashMap<String, String>,
    tool_call_batch_rewind_target_by_id: &HashMap<String, Option<String>>,
    children_by_parent: &HashMap<&str, Vec<&SessionEntry>>,
    branch_choices: Vec<SessionTreeSnapshotBranchChoice>,
) -> Option<Result<SessionTreeSnapshotRow, ResolveError>> {
    let kind = session_tree_row_kind(entry)?;
    let preview_content = session_tree_row_preview_content(entry, kind, children_by_parent);
    let preview_replay_items =
        session_tree_row_preview_replay_items(entry, kind, children_by_parent);
    let display_text = single_line_display_text(&preview_content);
    let summary = truncate_chars_with_ellipsis(&display_text, SESSION_TREE_SUMMARY_CHAR_LIMIT);
    let parent_id = match nearest_visible_parent_id(entry, by_id, visible_entry_ids) {
        Ok(parent_id) => parent_id,
        Err(error) => return Some(Err(error)),
    };
    let rewind_target_id = rewind_target_for_tree_row(
        entry,
        kind,
        visible_entry_ids,
        rewind_target_by_id,
        tool_call_batch_rewind_target_by_id,
        children_by_parent,
    );
    let rewind_prefill =
        (kind == SessionTreeSnapshotRowKind::User).then(|| preview_content.clone());

    Some(Ok(SessionTreeSnapshotRow {
        id: entry.id.clone(),
        parent_id,
        display_depth: 0,
        kind,
        display_text,
        summary,
        preview_content,
        preview_replay_items,
        rewind_target_id,
        rewind_prefill,
        branch_choices,
    }))
}

/// 沿完整 entry 树（含隐藏的 `ConfigChange`/`TranscriptReplay` 等）计算每条可见行的 graph indent。
///
/// 仅可见行的多子节点会增加深度的策略，会把多次 rewind（每次都把新的 `ConfigChange`
/// 挂到上一次 `ConfigChange` 上）压扁成兄弟分支；因此这里在完整 entry 树上统计 fork，
/// 仅排除 `Leaf` 标记（`Leaf` 用于活跃 leaf 重定向，不代表真实分叉）。
fn visible_display_depths(
    entries: &[SessionEntry],
    visible_entry_ids: &HashSet<&str>,
) -> HashMap<String, usize> {
    let mut children_by_parent: HashMap<Option<&str>, Vec<&SessionEntry>> = HashMap::new();
    for entry in entries {
        if matches!(entry.kind, SessionEntryKind::Leaf { .. }) {
            continue;
        }
        children_by_parent
            .entry(entry.parent_id.as_deref())
            .or_default()
            .push(entry);
    }

    let mut display_depths = HashMap::new();
    let mut stack = children_by_parent
        .get(&None)
        .into_iter()
        .flat_map(|roots| roots.iter().rev())
        .map(|entry| (*entry, 0usize))
        .collect::<Vec<_>>();

    while let Some((entry, display_depth)) = stack.pop() {
        if visible_entry_ids.contains(entry.id.as_str()) {
            display_depths.insert(entry.id.clone(), display_depth);
        }
        let children = children_by_parent
            .get(&Some(entry.id.as_str()))
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        let child_depth = if children.len() > 1 {
            display_depth.saturating_add(1)
        } else {
            display_depth
        };

        for child in children.iter().rev() {
            stack.push((*child, child_depth));
        }
    }

    display_depths
}
