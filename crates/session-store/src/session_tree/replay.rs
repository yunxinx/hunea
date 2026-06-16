use runtime_domain::session::TranscriptReplayItem;

use crate::{SessionEntry, SessionEntryKind};

pub(super) fn explicit_transcript_from_path(path: &[&SessionEntry]) -> Vec<TranscriptReplayItem> {
    let mut transcript = Vec::new();
    for entry in path {
        if let SessionEntryKind::TranscriptReplay(item) = &entry.kind {
            push_transcript_replay_snapshot(&mut transcript, item.clone());
        }
    }
    transcript
}

pub(super) fn push_transcript_replay_snapshot(
    transcript: &mut Vec<TranscriptReplayItem>,
    item: TranscriptReplayItem,
) {
    match &item {
        TranscriptReplayItem::ToolActivity { activity } => {
            if let Some(existing) = transcript.iter_mut().find(|existing| {
                matches!(
                    existing,
                    TranscriptReplayItem::ToolActivity { activity: existing_activity }
                        if existing_activity.activity_id == activity.activity_id
                )
            }) {
                *existing = item;
                return;
            }
        }
        TranscriptReplayItem::TerminalSnapshot { snapshot } => {
            if let Some(existing) = transcript.iter_mut().find(|existing| {
                matches!(
                    existing,
                    TranscriptReplayItem::TerminalSnapshot { snapshot: existing_snapshot }
                        if existing_snapshot.terminal_id == snapshot.terminal_id
                )
            }) {
                *existing = item;
                return;
            }
        }
        _ => {}
    }

    transcript.push(item);
}
