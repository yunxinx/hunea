use std::collections::{HashMap, HashSet};

use runtime_domain::session::SessionBranchSummary as DomainSessionBranchSummary;

use crate::{ResolveError, SessionEntry, util::truncate_chars_with_ellipsis};

use super::{
    preview::{
        SESSION_TREE_SUMMARY_CHAR_LIMIT, session_tree_row_kind, session_tree_row_preview_content,
        single_line_display_text, visible_descendant_row_kind,
    },
    topology::{
        build_children_by_parent, build_entry_index, entry_is_in_subtree,
        latest_non_leaf_descendant, latest_visible_descendant, nearest_visible_parent_id,
        resolve_path, validate_entry_parents,
    },
    types::{
        SessionBranchTreeSnapshot, SessionBranchTreeSnapshotNode, SessionTreeSnapshotBranchChoice,
        SessionTreeSnapshotRowKind,
    },
};

/// 生成完整 branch 拓扑快照，用于 `/tree` 内的 branch tree 界面。
pub fn session_branch_tree_snapshot(
    entries: &[SessionEntry],
) -> Result<SessionBranchTreeSnapshot, ResolveError> {
    session_branch_tree_snapshot_for_requested_leaf(
        entries,
        entries.last().map(|entry| entry.id.as_str()),
    )
}

fn session_branch_tree_snapshot_for_requested_leaf(
    entries: &[SessionEntry],
    requested_leaf_id: Option<&str>,
) -> Result<SessionBranchTreeSnapshot, ResolveError> {
    let by_id = build_entry_index(entries)?;
    validate_entry_parents(entries, &by_id)?;
    let children_by_parent = build_children_by_parent(entries);
    let active_path = if let Some(leaf_id) = requested_leaf_id {
        resolve_path(&by_id, leaf_id)?
    } else {
        Vec::new()
    };
    let visible_entry_ids = entries
        .iter()
        .filter(|entry| session_tree_row_kind(entry).is_some())
        .map(|entry| entry.id.as_str())
        .collect::<HashSet<_>>();
    let branch_root_ids = branch_tree_root_ids(entries, &by_id, &visible_entry_ids)?
        .into_iter()
        .collect::<HashSet<_>>();
    let current_branch_row_id = active_path
        .iter()
        .filter(|entry| branch_root_ids.contains(entry.id.as_str()))
        .map(|entry| entry.id.clone())
        .next_back();

    let mut nodes = Vec::new();
    for branch_root in entries
        .iter()
        .filter(|entry| branch_root_ids.contains(entry.id.as_str()))
    {
        let latest_visible = latest_visible_descendant(
            entries,
            &by_id,
            branch_root.id.as_str(),
            &visible_entry_ids,
        )?;
        let subtree_leaf = latest_non_leaf_descendant(entries, &by_id, branch_root.id.as_str())?;
        let message_count = visible_subtree_message_count(
            entries,
            &by_id,
            branch_root.id.as_str(),
            &visible_entry_ids,
        )?;
        let kind = visible_descendant_row_kind(latest_visible)?;
        let preview_content =
            session_tree_row_preview_content(latest_visible, kind, &children_by_parent);

        nodes.push(SessionBranchTreeSnapshotNode {
            parent_branch_row_id: nearest_branch_tree_parent_id(
                branch_root,
                &by_id,
                &branch_root_ids,
            )?,
            branch: session_branch_summary(
                branch_root,
                subtree_leaf,
                latest_visible,
                kind,
                preview_content,
                current_branch_row_id.as_deref() == Some(branch_root.id.as_str()),
                message_count,
            ),
        });
    }

    Ok(SessionBranchTreeSnapshot {
        nodes,
        current_branch_row_id,
        total_message_count: visible_entry_ids.len(),
    })
}

pub(super) fn session_tree_branch_choices_by_parent<'a>(
    entries: &'a [SessionEntry],
    by_id: &HashMap<&'a str, &'a SessionEntry>,
    visible_entry_ids: &HashSet<&'a str>,
    active_path_ids: &HashSet<&'a str>,
    children_by_parent: &HashMap<&'a str, Vec<&'a SessionEntry>>,
) -> Result<HashMap<String, Vec<SessionTreeSnapshotBranchChoice>>, ResolveError> {
    let mut visible_children_by_parent: HashMap<String, Vec<&SessionEntry>> = HashMap::new();
    for entry in entries
        .iter()
        .filter(|entry| visible_entry_ids.contains(entry.id.as_str()))
    {
        let Some(parent_id) = nearest_visible_parent_id(entry, by_id, visible_entry_ids)? else {
            continue;
        };
        visible_children_by_parent
            .entry(parent_id)
            .or_default()
            .push(entry);
    }

    let mut choices_by_parent = HashMap::new();
    for (parent_id, branch_roots) in visible_children_by_parent {
        if branch_roots.len() < 2 {
            continue;
        }

        let mut choices = Vec::with_capacity(branch_roots.len());
        for branch_root in branch_roots {
            let latest_visible = latest_visible_descendant(
                entries,
                by_id,
                branch_root.id.as_str(),
                visible_entry_ids,
            )?;
            let subtree_leaf = latest_non_leaf_descendant(entries, by_id, branch_root.id.as_str())?;
            let message_count = visible_subtree_message_count(
                entries,
                by_id,
                branch_root.id.as_str(),
                visible_entry_ids,
            )?;
            let kind = visible_descendant_row_kind(latest_visible)?;
            let preview_content =
                session_tree_row_preview_content(latest_visible, kind, children_by_parent);

            choices.push(SessionTreeSnapshotBranchChoice {
                branch: session_branch_summary(
                    branch_root,
                    subtree_leaf,
                    latest_visible,
                    kind,
                    preview_content,
                    active_path_ids.contains(branch_root.id.as_str()),
                    message_count,
                ),
            });
        }

        choices_by_parent.insert(parent_id, choices);
    }

    Ok(choices_by_parent)
}

fn session_branch_summary(
    branch_root: &SessionEntry,
    subtree_leaf: &SessionEntry,
    latest_visible: &SessionEntry,
    kind: SessionTreeSnapshotRowKind,
    preview_content: String,
    is_current: bool,
    message_count: usize,
) -> DomainSessionBranchSummary {
    let display_text = single_line_display_text(&preview_content);
    DomainSessionBranchSummary {
        branch_row_id: branch_root.id.clone(),
        subtree_leaf_id: subtree_leaf.id.clone(),
        latest_row_id: latest_visible.id.clone(),
        kind,
        display_summary: truncate_chars_with_ellipsis(
            &display_text,
            SESSION_TREE_SUMMARY_CHAR_LIMIT,
        ),
        preview_content,
        is_current,
        message_count,
        branch_created_at_ms: branch_root.timestamp,
        latest_updated_at_ms: latest_visible.timestamp,
    }
}

fn branch_tree_root_ids<'a>(
    entries: &'a [SessionEntry],
    by_id: &HashMap<&'a str, &'a SessionEntry>,
    visible_entry_ids: &HashSet<&'a str>,
) -> Result<Vec<&'a str>, ResolveError> {
    let mut visible_children_by_parent: HashMap<Option<String>, Vec<&SessionEntry>> =
        HashMap::new();
    for entry in entries
        .iter()
        .filter(|entry| visible_entry_ids.contains(entry.id.as_str()))
    {
        let parent_id = nearest_visible_parent_id(entry, by_id, visible_entry_ids)?;
        visible_children_by_parent
            .entry(parent_id)
            .or_default()
            .push(entry);
    }

    let mut root_ids = HashSet::new();
    if let Some(top_level_roots) = visible_children_by_parent.get(&None) {
        root_ids.extend(top_level_roots.iter().map(|entry| entry.id.as_str()));
    }
    for (parent_id, branch_roots) in visible_children_by_parent {
        if parent_id.is_some() && branch_roots.len() >= 2 {
            root_ids.extend(branch_roots.into_iter().map(|entry| entry.id.as_str()));
        }
    }

    Ok(entries
        .iter()
        .map(|entry| entry.id.as_str())
        .filter(|entry_id| root_ids.contains(entry_id))
        .collect())
}

fn nearest_branch_tree_parent_id(
    entry: &SessionEntry,
    by_id: &HashMap<&str, &SessionEntry>,
    branch_root_ids: &HashSet<&str>,
) -> Result<Option<String>, ResolveError> {
    let mut current_id = entry.parent_id.as_deref();
    let mut visited = HashSet::new();

    while let Some(parent_id) = current_id {
        if branch_root_ids.contains(parent_id) {
            return Ok(Some(parent_id.to_string()));
        }
        if !visited.insert(parent_id) {
            return Err(ResolveError::CycleDetected);
        }
        let parent = by_id
            .get(parent_id)
            .ok_or_else(|| ResolveError::DanglingParent(parent_id.to_string()))?;
        current_id = parent.parent_id.as_deref();
    }

    Ok(None)
}

fn visible_subtree_message_count(
    entries: &[SessionEntry],
    by_id: &HashMap<&str, &SessionEntry>,
    root_id: &str,
    visible_entry_ids: &HashSet<&str>,
) -> Result<usize, ResolveError> {
    let mut count = 0;
    for entry in entries
        .iter()
        .filter(|entry| visible_entry_ids.contains(entry.id.as_str()))
    {
        if session_tree_row_kind(entry).is_some() && entry_is_in_subtree(entry, root_id, by_id)? {
            count += 1;
        }
    }

    Ok(count)
}
