use std::collections::{HashMap, HashSet};

use provider_protocol::{ConversationItem, Role, ToolCall};
use runtime_domain::session::{
    RuntimeToolActivity, RuntimeToolActivityContent, TranscriptReplayItem,
};

use crate::{ResolveError, SessionEntry, SessionEntryKind};

use super::{replay::push_transcript_replay_snapshot, types::SessionTreeSnapshotRowKind};

pub(super) const SESSION_TREE_SUMMARY_CHAR_LIMIT: usize = 120;

pub(super) fn session_tree_row_kind(entry: &SessionEntry) -> Option<SessionTreeSnapshotRowKind> {
    match &entry.kind {
        SessionEntryKind::Item(item) => match item.role() {
            Some(Role::User) => Some(SessionTreeSnapshotRowKind::User),
            Some(Role::Assistant) => Some(SessionTreeSnapshotRowKind::Assistant),
            Some(Role::System) => None,
            None => match item {
                ConversationItem::ToolResult { .. } => Some(SessionTreeSnapshotRowKind::Tool),
                ConversationItem::Reasoning { .. } => Some(SessionTreeSnapshotRowKind::Reasoning),
                ConversationItem::Message { .. } => None,
            },
        },
        SessionEntryKind::Header(_)
        | SessionEntryKind::Compaction { .. }
        | SessionEntryKind::BranchSummary { .. }
        | SessionEntryKind::ConfigChange(_)
        | SessionEntryKind::TranscriptReplay(_)
        | SessionEntryKind::Leaf { .. } => None,
    }
}

pub(super) fn visible_descendant_row_kind(
    entry: &SessionEntry,
) -> Result<SessionTreeSnapshotRowKind, ResolveError> {
    session_tree_row_kind(entry).ok_or_else(|| ResolveError::InvalidTreeRow(entry.id.clone()))
}

pub(super) fn session_tree_row_preview_content(
    entry: &SessionEntry,
    kind: SessionTreeSnapshotRowKind,
    children_by_parent: &HashMap<&str, Vec<&SessionEntry>>,
) -> String {
    if let Some(content) = transcript_replay_preview_content(entry, kind, children_by_parent) {
        return content;
    }

    session_tree_fallback_preview_content(entry, kind)
}

pub(super) fn session_tree_row_preview_replay_items(
    entry: &SessionEntry,
    kind: SessionTreeSnapshotRowKind,
    children_by_parent: &HashMap<&str, Vec<&SessionEntry>>,
) -> Vec<TranscriptReplayItem> {
    match kind {
        SessionTreeSnapshotRowKind::Assistant => {
            let content = transcript_replay_preview_content(entry, kind, children_by_parent)
                .unwrap_or_else(|| session_tree_fallback_preview_content(entry, kind));
            assistant_preview_replay_items(entry, content)
        }
        SessionTreeSnapshotRowKind::Tool => {
            let replay_items = transcript_replay_descendants(entry, children_by_parent)
                .into_iter()
                .filter(|item| {
                    matches!(
                        item,
                        TranscriptReplayItem::ToolActivity { .. }
                            | TranscriptReplayItem::TerminalSnapshot { .. }
                            | TranscriptReplayItem::ToolResult { .. }
                    )
                })
                .collect::<Vec<_>>();
            if replay_items.is_empty() {
                vec![TranscriptReplayItem::ToolResult {
                    content: session_tree_fallback_preview_content(entry, kind),
                }]
            } else {
                replay_items
            }
        }
        SessionTreeSnapshotRowKind::User => vec![TranscriptReplayItem::Message {
            role: runtime_domain::session::TranscriptReplayRole::User,
            content: transcript_replay_preview_content(entry, kind, children_by_parent)
                .unwrap_or_else(|| session_tree_fallback_preview_content(entry, kind)),
        }],
        SessionTreeSnapshotRowKind::Reasoning => vec![TranscriptReplayItem::Reasoning {
            content: transcript_replay_preview_content(entry, kind, children_by_parent)
                .unwrap_or_else(|| session_tree_fallback_preview_content(entry, kind)),
        }],
    }
}

fn assistant_preview_replay_items(
    entry: &SessionEntry,
    visible_content: String,
) -> Vec<TranscriptReplayItem> {
    let Some(item) = entry_item(entry) else {
        return non_empty_preview_content(&visible_content)
            .map(|content| TranscriptReplayItem::Message {
                role: runtime_domain::session::TranscriptReplayRole::Assistant,
                content,
            })
            .into_iter()
            .collect();
    };

    let debug_content = assistant_debug_preview_content(item, &visible_content);
    non_empty_preview_content(&debug_content)
        .map(|content| TranscriptReplayItem::Message {
            role: runtime_domain::session::TranscriptReplayRole::Assistant,
            content,
        })
        .into_iter()
        .collect()
}

fn assistant_debug_preview_content(item: &ConversationItem, visible_content: &str) -> String {
    let mut sections = Vec::new();
    if !visible_content.trim().is_empty() {
        sections.push(visible_content.to_string());
    }

    sections.extend(item.tool_calls().map(assistant_tool_call_debug_section));
    sections.join("\n\n")
}

fn assistant_tool_call_debug_section(call: &ToolCall) -> String {
    format!(
        "Tool call `{}` ({})\n```json\n{}\n```",
        call.name,
        call.call_id,
        pretty_tool_call_arguments(&call.arguments)
    )
}

fn pretty_tool_call_arguments(arguments: &str) -> String {
    let arguments = if arguments.trim().is_empty() {
        "{}"
    } else {
        arguments
    };

    serde_json::from_str::<serde_json::Value>(arguments)
        .ok()
        .and_then(|value| serde_json::to_string_pretty(&value).ok())
        .unwrap_or_else(|| arguments.to_string())
}

fn transcript_replay_preview_content(
    entry: &SessionEntry,
    kind: SessionTreeSnapshotRowKind,
    children_by_parent: &HashMap<&str, Vec<&SessionEntry>>,
) -> Option<String> {
    let mut preview_content = None;
    for item in transcript_replay_descendants(entry, children_by_parent) {
        if let Some(content) = transcript_replay_item_preview_content(&item, kind) {
            preview_content = Some(content);
        }
    }

    preview_content
}

fn transcript_replay_descendants(
    entry: &SessionEntry,
    children_by_parent: &HashMap<&str, Vec<&SessionEntry>>,
) -> Vec<TranscriptReplayItem> {
    let mut replay_items = Vec::new();
    let mut stack = children_by_parent
        .get(entry.id.as_str())
        .into_iter()
        .flat_map(|children| children.iter().rev().copied())
        .collect::<Vec<_>>();
    let mut visited = HashSet::new();

    while let Some(child) = stack.pop() {
        if !visited.insert(child.id.as_str()) {
            continue;
        }

        if session_tree_row_kind(child).is_some() {
            continue;
        }

        if let SessionEntryKind::TranscriptReplay(item) = &child.kind {
            push_transcript_replay_snapshot(&mut replay_items, item.clone());
        }

        if let Some(children) = children_by_parent.get(child.id.as_str()) {
            stack.extend(children.iter().rev().copied());
        }
    }

    replay_items
}

pub(super) fn entry_item(entry: &SessionEntry) -> Option<&ConversationItem> {
    match &entry.kind {
        SessionEntryKind::Item(item) => Some(item),
        _ => None,
    }
}

fn transcript_replay_item_preview_content(
    item: &TranscriptReplayItem,
    kind: SessionTreeSnapshotRowKind,
) -> Option<String> {
    match (kind, item) {
        (
            SessionTreeSnapshotRowKind::User,
            TranscriptReplayItem::Message {
                role: runtime_domain::session::TranscriptReplayRole::User,
                content,
            },
        )
        | (
            SessionTreeSnapshotRowKind::Assistant,
            TranscriptReplayItem::Message {
                role: runtime_domain::session::TranscriptReplayRole::Assistant,
                content,
            },
        )
        | (SessionTreeSnapshotRowKind::Reasoning, TranscriptReplayItem::Reasoning { content })
        | (SessionTreeSnapshotRowKind::Tool, TranscriptReplayItem::ToolResult { content }) => {
            non_empty_preview_content(content)
        }
        (SessionTreeSnapshotRowKind::Tool, TranscriptReplayItem::ToolActivity { activity }) => {
            tool_activity_preview_content(activity)
        }
        _ => None,
    }
}

fn tool_activity_preview_content(activity: &RuntimeToolActivity) -> Option<String> {
    if let Some(content) = activity
        .raw_output
        .as_ref()
        .and_then(|raw_output| raw_output.display_text())
        .and_then(|content| non_empty_preview_content(&content))
    {
        return Some(content);
    }

    let content = activity
        .content
        .iter()
        .filter_map(runtime_tool_activity_content_preview_text)
        .collect::<Vec<_>>()
        .join("\n");
    non_empty_preview_content(&content).or_else(|| non_empty_preview_content(&activity.title))
}

fn runtime_tool_activity_content_preview_text(
    content: &RuntimeToolActivityContent,
) -> Option<String> {
    match content {
        RuntimeToolActivityContent::Text(text) | RuntimeToolActivityContent::Unknown(text) => {
            non_empty_preview_content(text)
        }
        RuntimeToolActivityContent::Resource {
            text: Some(text), ..
        } => non_empty_preview_content(text),
        RuntimeToolActivityContent::ResourceLink { title, name, uri } => title
            .as_deref()
            .and_then(non_empty_preview_content)
            .or_else(|| non_empty_preview_content(name))
            .or_else(|| non_empty_preview_content(uri)),
        RuntimeToolActivityContent::Diff { path, new_text, .. } => {
            non_empty_preview_content(&format!("{path}\n{new_text}"))
        }
        RuntimeToolActivityContent::Terminal { terminal_id } => {
            non_empty_preview_content(terminal_id)
        }
        RuntimeToolActivityContent::Image { .. }
        | RuntimeToolActivityContent::Audio { .. }
        | RuntimeToolActivityContent::Resource { text: None, .. } => None,
    }
}

fn non_empty_preview_content(content: &str) -> Option<String> {
    (!content.trim().is_empty()).then(|| content.to_string())
}

fn session_tree_fallback_preview_content(
    entry: &SessionEntry,
    kind: SessionTreeSnapshotRowKind,
) -> String {
    match &entry.kind {
        SessionEntryKind::Item(item) => match item {
            ConversationItem::Reasoning { content, .. } => content.clone(),
            ConversationItem::Message {
                role: Role::Assistant,
                ..
            } if item.text_content().trim().is_empty() => assistant_tool_call_summary(item),
            ConversationItem::ToolResult { call_id, .. }
                if item.text_content().trim().is_empty() =>
            {
                format!("tool result {call_id}")
            }
            _ => item.text_content(),
        },
        _ => match kind {
            SessionTreeSnapshotRowKind::User => "user message".to_string(),
            SessionTreeSnapshotRowKind::Assistant => "assistant message".to_string(),
            SessionTreeSnapshotRowKind::Tool => "tool result".to_string(),
            SessionTreeSnapshotRowKind::Reasoning => "reasoning".to_string(),
        },
    }
}

fn assistant_tool_call_summary(item: &ConversationItem) -> String {
    let tool_names = item
        .tool_calls()
        .map(|tool_call| tool_call.name.as_str())
        .collect::<Vec<_>>();
    if tool_names.is_empty() {
        "assistant message".to_string()
    } else {
        format!("tool call: {}", tool_names.join(", "))
    }
}

pub(super) fn single_line_display_text(content: &str) -> String {
    content.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use provider_protocol::ConversationItem;

    use super::*;
    use crate::ConfigSnapshot;

    #[test]
    fn visible_descendant_row_kind_reports_hidden_entry_invariant() {
        let hidden_entry = SessionEntry {
            id: "config-1".to_string(),
            parent_id: None,
            timestamp: 1,
            kind: SessionEntryKind::ConfigChange(ConfigSnapshot {
                provider_id: "local".to_string(),
                model: "qwen3".to_string(),
                system_prompt: None,
            }),
        };

        let error = visible_descendant_row_kind(&hidden_entry)
            .expect_err("hidden tree entries must be reported instead of panicking");

        assert_eq!(error, ResolveError::InvalidTreeRow("config-1".to_string()));
    }

    #[test]
    fn visible_descendant_row_kind_accepts_visible_item_entry() {
        let visible_entry = SessionEntry {
            id: "user-1".to_string(),
            parent_id: None,
            timestamp: 1,
            kind: SessionEntryKind::Item(ConversationItem::text(Role::User, "hello")),
        };

        let kind = visible_descendant_row_kind(&visible_entry)
            .expect("visible item entries should keep their row kind");

        assert_eq!(kind, SessionTreeSnapshotRowKind::User);
    }
}
