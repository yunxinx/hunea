use provider_protocol::ConversationItem;
use runtime_domain::session::TranscriptReplayItem;

use crate::{
    ResolveError, ResolvedSessionItem, ResolvedSessionState, SessionEntry, SessionEntryKind,
};

use super::{
    replay::explicit_transcript_from_path,
    topology::{build_entry_index, resolve_path},
};

pub fn resolve(
    entries: &[SessionEntry],
    leaf_id: &str,
) -> Result<Vec<ConversationItem>, ResolveError> {
    Ok(resolve_state(entries, leaf_id)?
        .items
        .into_iter()
        .map(|item| item.item)
        .collect())
}

/// 从指定 leaf 解析 provider-visible history 以及恢复所需的附加状态。
pub fn resolve_state(
    entries: &[SessionEntry],
    leaf_id: &str,
) -> Result<ResolvedSessionState, ResolveError> {
    let by_id = build_entry_index(entries)?;
    let path = resolve_path(&by_id, leaf_id)?;

    resolve_state_from_path(&path)
}

fn resolve_state_from_path(path: &[&SessionEntry]) -> Result<ResolvedSessionState, ResolveError> {
    let mut latest_config = None;
    for entry in path {
        if let SessionEntryKind::ConfigChange(snapshot) = &entry.kind {
            latest_config = Some(snapshot.clone());
        }
    }

    let mut resolved_items = Vec::new();
    let compaction_summary;
    let start_index =
        if let Some((entry_id, summary, first_kept_entry_id)) = latest_compaction(path) {
            let keep_index = item_entry_position(path, first_kept_entry_id).ok_or_else(|| {
                ResolveError::InvalidCompactionTarget(first_kept_entry_id.to_string())
            })?;
            resolved_items.push(ResolvedSessionItem {
                entry_id: entry_id.to_string(),
                item: ConversationItem::system(vec![provider_protocol::ContentBlock::Text(
                    summary.to_string(),
                )]),
            });
            compaction_summary = Some(summary.to_string());
            keep_index
        } else {
            compaction_summary = None;
            0
        };

    resolved_items.extend(
        path[start_index..]
            .iter()
            .filter_map(|entry| match &entry.kind {
                SessionEntryKind::Item(item) => Some(ResolvedSessionItem {
                    entry_id: entry.id.clone(),
                    item: item.clone(),
                }),
                _ => None,
            }),
    );

    let mut transcript = explicit_transcript_from_path(&path[start_index..]);
    if !transcript.is_empty()
        && let Some(summary) = compaction_summary
    {
        transcript.insert(0, TranscriptReplayItem::System { content: summary });
    }

    Ok(ResolvedSessionState {
        items: resolved_items,
        transcript,
        latest_config,
    })
}

fn item_entry_position(path: &[&SessionEntry], target_id: &str) -> Option<usize> {
    path.iter()
        .position(|entry| entry.id == target_id && matches!(entry.kind, SessionEntryKind::Item(_)))
}

fn latest_compaction<'a>(path: &'a [&'a SessionEntry]) -> Option<(&'a str, &'a str, &'a str)> {
    path.iter().rev().find_map(|entry| match &entry.kind {
        SessionEntryKind::Compaction {
            summary,
            first_kept_entry_id,
            ..
        } => Some((
            entry.id.as_str(),
            summary.as_str(),
            first_kept_entry_id.as_str(),
        )),
        _ => None,
    })
}
