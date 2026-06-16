use std::collections::{HashMap, HashSet};

use provider_protocol::ConversationItem;

use crate::{SessionEntry, SessionEntryKind};

use super::{
    preview::{entry_item, session_tree_row_kind},
    types::SessionTreeSnapshotRowKind,
};

fn rewind_target_before_user_row(entry: &SessionEntry) -> String {
    entry
        .parent_id
        .clone()
        .unwrap_or_else(|| "header".to_string())
}

pub(super) fn tool_call_batch_rewind_targets_for_path(
    path: &[&SessionEntry],
    rewind_target_by_id: &HashMap<String, String>,
) -> HashMap<String, Option<String>> {
    let mut rewind_targets = HashMap::new();
    let mut pending_tool_call_ids: Option<HashSet<String>> = None;

    for entry in path {
        let Some(item) = entry_item(entry) else {
            continue;
        };

        if let Some(pending_call_ids) = pending_tool_call_ids.as_mut() {
            match item {
                ConversationItem::ToolResult { call_id, .. } => {
                    pending_call_ids.remove(call_id);
                    let rewind_target = pending_call_ids
                        .is_empty()
                        .then(|| default_rewind_target_for_visible_row(entry, rewind_target_by_id));
                    rewind_targets.insert(entry.id.clone(), rewind_target.clone());
                    if rewind_target.is_some() {
                        pending_tool_call_ids = None;
                    }
                    continue;
                }
                _ => {
                    pending_tool_call_ids = None;
                }
            }
        }

        let tool_call_ids = item
            .tool_calls()
            .map(|tool_call| tool_call.call_id.clone())
            .collect::<HashSet<_>>();
        if !tool_call_ids.is_empty() {
            rewind_targets.insert(entry.id.clone(), None);
            pending_tool_call_ids = Some(tool_call_ids);
        }
    }

    rewind_targets
}

pub(super) fn rewind_target_for_tree_row(
    entry: &SessionEntry,
    kind: SessionTreeSnapshotRowKind,
    visible_entry_ids: &HashSet<&str>,
    rewind_target_by_id: &HashMap<String, String>,
    tool_call_batch_rewind_target_by_id: &HashMap<String, Option<String>>,
    children_by_parent: &HashMap<&str, Vec<&SessionEntry>>,
) -> Option<String> {
    match kind {
        SessionTreeSnapshotRowKind::User => Some(rewind_target_before_user_row(entry)),
        SessionTreeSnapshotRowKind::Reasoning => owning_assistant_for_reasoning(
            entry,
            visible_entry_ids,
            children_by_parent,
            rewind_target_by_id,
            tool_call_batch_rewind_target_by_id,
        ),
        SessionTreeSnapshotRowKind::Assistant | SessionTreeSnapshotRowKind::Tool => {
            assistant_or_tool_rewind_target(
                entry,
                rewind_target_by_id,
                tool_call_batch_rewind_target_by_id,
            )
        }
    }
}

fn assistant_or_tool_rewind_target(
    entry: &SessionEntry,
    rewind_target_by_id: &HashMap<String, String>,
    tool_call_batch_rewind_target_by_id: &HashMap<String, Option<String>>,
) -> Option<String> {
    tool_call_batch_rewind_target_by_id
        .get(&entry.id)
        .cloned()
        .unwrap_or_else(|| {
            Some(default_rewind_target_for_visible_row(
                entry,
                rewind_target_by_id,
            ))
        })
}

fn default_rewind_target_for_visible_row(
    entry: &SessionEntry,
    rewind_target_by_id: &HashMap<String, String>,
) -> String {
    rewind_target_by_id
        .get(&entry.id)
        .cloned()
        .unwrap_or_else(|| entry.id.clone())
}

fn owning_assistant_for_reasoning(
    reasoning: &SessionEntry,
    visible_entry_ids: &HashSet<&str>,
    children_by_parent: &HashMap<&str, Vec<&SessionEntry>>,
    rewind_target_by_id: &HashMap<String, String>,
    tool_call_batch_rewind_target_by_id: &HashMap<String, Option<String>>,
) -> Option<String> {
    let mut current = reasoning;
    let mut visited = HashSet::new();

    while visited.insert(current.id.as_str()) {
        let children = children_by_parent
            .get(current.id.as_str())
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        let logical_children = children
            .iter()
            .copied()
            .filter(|child| {
                visible_entry_ids.contains(child.id.as_str())
                    || is_recoverable_hidden_tree_entry(child)
            })
            .collect::<Vec<_>>();

        let [next] = logical_children.as_slice() else {
            return None;
        };

        if visible_entry_ids.contains(next.id.as_str()) {
            return match session_tree_row_kind(next) {
                Some(SessionTreeSnapshotRowKind::Assistant) => assistant_or_tool_rewind_target(
                    next,
                    rewind_target_by_id,
                    tool_call_batch_rewind_target_by_id,
                ),
                _ => None,
            };
        }

        current = next;
    }

    None
}

pub(super) fn restore_target_for_visible_row(
    entry: &SessionEntry,
    visible_entry_ids: &HashSet<&str>,
    children_by_parent: &HashMap<&str, Vec<&SessionEntry>>,
    active_path_ids: &HashSet<&str>,
) -> String {
    let mut target = entry;
    let mut visited = HashSet::new();

    while visited.insert(target.id.as_str()) {
        let Some(hidden_children) = children_by_parent.get(target.id.as_str()).map(|children| {
            children
                .iter()
                .copied()
                .filter(|child| !visible_entry_ids.contains(child.id.as_str()))
                .filter(|child| is_recoverable_hidden_tree_entry(child))
                .collect::<Vec<_>>()
        }) else {
            break;
        };

        let Some(next_target) = hidden_children
            .iter()
            .find(|child| active_path_ids.contains(child.id.as_str()))
            .copied()
            .or_else(|| hidden_children.last().copied())
        else {
            break;
        };

        target = next_target;
    }

    target.id.clone()
}

fn is_recoverable_hidden_tree_entry(entry: &SessionEntry) -> bool {
    matches!(
        entry.kind,
        SessionEntryKind::Compaction { .. }
            | SessionEntryKind::BranchSummary { .. }
            | SessionEntryKind::ConfigChange(_)
            | SessionEntryKind::TranscriptReplay(_)
    )
}
