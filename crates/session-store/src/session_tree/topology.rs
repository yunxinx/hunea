use std::collections::{HashMap, HashSet};

use crate::{ResolveError, SessionEntry, SessionEntryKind};

pub(super) fn resolve_path<'a>(
    by_id: &HashMap<&'a str, &'a SessionEntry>,
    leaf_id: &'a str,
) -> Result<Vec<&'a SessionEntry>, ResolveError> {
    let effective_leaf_id = effective_leaf_id(by_id, leaf_id);
    let mut path = Vec::new();
    let mut visited = HashSet::new();
    let mut current = *by_id
        .get(effective_leaf_id)
        .ok_or_else(|| ResolveError::LeafNotFound(effective_leaf_id.to_string()))?;

    loop {
        if !visited.insert(current.id.as_str()) {
            return Err(ResolveError::CycleDetected);
        }

        path.push(current);

        let Some(parent_id) = current.parent_id.as_deref() else {
            break;
        };

        current = *by_id
            .get(parent_id)
            .ok_or_else(|| ResolveError::DanglingParent(parent_id.to_string()))?;
    }

    path.reverse();
    Ok(path)
}

pub(super) fn build_entry_index(
    entries: &[SessionEntry],
) -> Result<HashMap<&str, &SessionEntry>, ResolveError> {
    let mut by_id = HashMap::with_capacity(entries.len());
    for entry in entries {
        if by_id.insert(entry.id.as_str(), entry).is_some() {
            return Err(ResolveError::DuplicateId(entry.id.clone()));
        }
    }
    Ok(by_id)
}

pub(super) fn validate_entry_parents<'a>(
    entries: &'a [SessionEntry],
    by_id: &HashMap<&'a str, &'a SessionEntry>,
) -> Result<(), ResolveError> {
    for entry in entries {
        if let Some(parent_id) = entry.parent_id.as_deref()
            && !by_id.contains_key(parent_id)
        {
            return Err(ResolveError::DanglingParent(parent_id.to_string()));
        }
    }
    Ok(())
}

pub(super) fn build_children_by_parent(
    entries: &[SessionEntry],
) -> HashMap<&str, Vec<&SessionEntry>> {
    let mut children_by_parent = HashMap::new();
    for entry in entries {
        if let Some(parent_id) = entry.parent_id.as_deref() {
            children_by_parent
                .entry(parent_id)
                .or_insert_with(Vec::new)
                .push(entry);
        }
    }
    children_by_parent
}

fn effective_leaf_id<'a>(
    by_id: &HashMap<&'a str, &'a SessionEntry>,
    requested_leaf_id: &'a str,
) -> &'a str {
    match by_id.get(requested_leaf_id).map(|entry| &entry.kind) {
        Some(SessionEntryKind::Leaf {
            target_id: Some(target_id),
        }) => target_id.as_str(),
        Some(SessionEntryKind::Leaf { target_id: None }) => requested_leaf_id,
        _ => requested_leaf_id,
    }
}

pub(super) fn nearest_visible_parent_id(
    entry: &SessionEntry,
    by_id: &HashMap<&str, &SessionEntry>,
    visible_entry_ids: &HashSet<&str>,
) -> Result<Option<String>, ResolveError> {
    let mut current_id = entry.parent_id.as_deref();
    while let Some(parent_id) = current_id {
        if visible_entry_ids.contains(parent_id) {
            return Ok(Some(parent_id.to_string()));
        }
        let parent = by_id
            .get(parent_id)
            .ok_or_else(|| ResolveError::DanglingParent(parent_id.to_string()))?;
        current_id = parent.parent_id.as_deref();
    }
    Ok(None)
}

pub(super) fn entry_is_in_subtree<'a>(
    candidate: &'a SessionEntry,
    root_id: &str,
    by_id: &HashMap<&'a str, &'a SessionEntry>,
) -> Result<bool, ResolveError> {
    if candidate.id == root_id {
        return Ok(true);
    }

    let mut parent_id = candidate.parent_id.as_deref();
    let mut visited = HashSet::new();
    while let Some(current_parent_id) = parent_id {
        if current_parent_id == root_id {
            return Ok(true);
        }
        if !visited.insert(current_parent_id) {
            return Err(ResolveError::CycleDetected);
        }
        let parent = by_id
            .get(current_parent_id)
            .ok_or_else(|| ResolveError::DanglingParent(current_parent_id.to_string()))?;
        parent_id = parent.parent_id.as_deref();
    }

    Ok(false)
}

pub(super) fn latest_visible_descendant<'a>(
    entries: &'a [SessionEntry],
    by_id: &HashMap<&'a str, &'a SessionEntry>,
    root_id: &str,
    visible_entry_ids: &HashSet<&str>,
) -> Result<&'a SessionEntry, ResolveError> {
    let mut latest = None;
    for entry in entries
        .iter()
        .filter(|entry| visible_entry_ids.contains(entry.id.as_str()))
    {
        if entry_is_in_subtree(entry, root_id, by_id)? {
            latest = Some(entry);
        }
    }

    latest.ok_or_else(|| ResolveError::LeafNotFound(root_id.to_string()))
}

pub(super) fn latest_non_leaf_descendant<'a>(
    entries: &'a [SessionEntry],
    by_id: &HashMap<&'a str, &'a SessionEntry>,
    root_id: &str,
) -> Result<&'a SessionEntry, ResolveError> {
    let mut latest = None;
    for entry in entries
        .iter()
        .filter(|entry| !matches!(entry.kind, SessionEntryKind::Leaf { .. }))
    {
        if entry_is_in_subtree(entry, root_id, by_id)? {
            latest = Some(entry);
        }
    }

    latest.ok_or_else(|| ResolveError::LeafNotFound(root_id.to_string()))
}
