use runtime_domain::session::{SessionTreeRow, TranscriptReplayItem, TranscriptReplayRole};

use crate::session_tree_row_kind_view::CopyableSessionTreeRowKind;

/// `SessionTreePreviewReplay` 描述树行预览 replay 的来源。
pub(crate) enum SessionTreePreviewReplay<'a> {
    Borrowed(&'a [TranscriptReplayItem]),
    Fallback(TranscriptReplayItem),
}

impl<'a> SessionTreePreviewReplay<'a> {
    /// `from_session_tree_row` 只使用 session-store 已物化的 replay。
    ///
    /// Entry tree preview 不回退到 legacy `preview_content`，避免把缺失 replay
    /// 误展示成完整预览。
    pub(crate) fn from_session_tree_row(row: &'a SessionTreeRow) -> Self {
        Self::Borrowed(&row.preview_replay_items)
    }

    /// `from_copyable_parts` 为 copy picker 的 copyable 行提供文本 fallback。
    pub(crate) fn from_copyable_parts(
        kind: CopyableSessionTreeRowKind,
        replay_items: &'a [TranscriptReplayItem],
        fallback_content: &str,
    ) -> Self {
        if !replay_items.is_empty() {
            return Self::Borrowed(replay_items);
        }

        Self::Fallback(fallback_replay_item(kind, fallback_content))
    }
}

fn fallback_replay_item(kind: CopyableSessionTreeRowKind, content: &str) -> TranscriptReplayItem {
    match kind {
        CopyableSessionTreeRowKind::User => TranscriptReplayItem::Message {
            role: TranscriptReplayRole::User,
            content: content.to_string(),
        },
        CopyableSessionTreeRowKind::Assistant => TranscriptReplayItem::Message {
            role: TranscriptReplayRole::Assistant,
            content: content.to_string(),
        },
    }
}

#[cfg(test)]
mod tests {
    use runtime_domain::session::{SessionTreeRowKind, TranscriptReplayItem};

    use crate::session_tree_row_kind_view::CopyableSessionTreeRowKind;

    use super::*;

    #[test]
    fn preview_replay_borrows_existing_items() {
        let items = vec![TranscriptReplayItem::ToolResult {
            content: "tool output".to_string(),
        }];

        match SessionTreePreviewReplay::from_copyable_parts(
            CopyableSessionTreeRowKind::Assistant,
            &items,
            "fallback",
        ) {
            SessionTreePreviewReplay::Borrowed(replay_items) => {
                assert!(std::ptr::eq(replay_items.as_ptr(), items.as_ptr()));
            }
            SessionTreePreviewReplay::Fallback(_) => panic!("expected borrowed replay items"),
        }
    }

    #[test]
    fn preview_replay_fallback_matches_row_kind() {
        let replay = SessionTreePreviewReplay::from_copyable_parts(
            CopyableSessionTreeRowKind::Assistant,
            &[],
            "assistant fallback",
        );

        assert!(matches!(
            replay,
            SessionTreePreviewReplay::Fallback(TranscriptReplayItem::Message {
                role: TranscriptReplayRole::Assistant,
                content,
            }) if content == "assistant fallback"
        ));
    }

    #[test]
    fn session_tree_row_preview_does_not_fallback_to_preview_content() {
        let row = SessionTreeRow {
            row_id: "row-1".to_string(),
            parent_id: None,
            display_depth: 0,
            kind: SessionTreeRowKind::Assistant,
            display_text: "display".to_string(),
            summary: "summary".to_string(),
            preview_content: "legacy preview content".to_string(),
            preview_replay_items: Vec::new(),
            rewind_target_id: None,
            rewind_prefill: None,
            is_active_path: false,
            is_current: false,
            branch_choices: Vec::new(),
        };

        match SessionTreePreviewReplay::from_session_tree_row(&row) {
            SessionTreePreviewReplay::Borrowed(replay_items) => assert!(replay_items.is_empty()),
            SessionTreePreviewReplay::Fallback(_) => {
                panic!("entry tree preview should not fallback")
            }
        }
    }
}
