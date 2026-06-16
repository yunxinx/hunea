use std::path::PathBuf;

use provider_protocol::{ContentBlock, ConversationItem, Role, ToolCall};
use runtime_domain::session::{
    RuntimeTerminalSnapshot, RuntimeToolActivity, RuntimeToolActivityContent,
    RuntimeToolActivityRawValue, RuntimeToolActivityStatus, RuntimeToolKind, TranscriptReplayItem,
};

use crate::{
    ConfigSnapshot, ResolveError, SessionEntry, SessionEntryKind, SessionHeader, SessionId,
};

#[test]
fn resolve_returns_linear_history_items_in_order() {
    let entries = linear_history_entries();

    let resolved = super::resolve(&entries, "user-2").expect("linear history should resolve");

    assert_eq!(
        resolved,
        vec![
            ConversationItem::text(Role::User, "hello"),
            ConversationItem::text(Role::Assistant, "hi"),
            ConversationItem::text(Role::User, "follow up"),
        ]
    );
}

#[test]
fn resolve_state_returns_explicit_transcript_replay_items() {
    let session_id: SessionId = "01914a5c-3c7e-7a2b-8abc-1234567890ab"
        .parse()
        .expect("fixture session id should parse");
    let expected_activity = sample_tool_activity("call-1", "final");
    let expected_snapshot = RuntimeTerminalSnapshot {
        terminal_id: "call-1".to_string(),
        command: Some("cargo test".to_string()),
        cwd: Some("/repo".to_string()),
        output: "test output".to_string(),
        truncated: false,
        exit_status: None,
        released: true,
    };
    let entries = vec![
        SessionEntry {
            id: "header".to_string(),
            parent_id: None,
            timestamp: 1_717_514_800_000,
            kind: SessionEntryKind::Header(SessionHeader {
                session_id,
                work_dir: PathBuf::from("/repo"),
                session_name: None,
                initial_model: "qwen3".to_string(),
                git_head: None,
                cli_version: None,
            }),
        },
        SessionEntry {
            id: "assistant-1".to_string(),
            parent_id: Some("header".to_string()),
            timestamp: 1_717_514_800_001,
            kind: SessionEntryKind::Item(ConversationItem::assistant_with_tool_calls(
                "editing".to_string(),
                vec![ToolCall::new(
                    "call-1",
                    "write_file",
                    r#"{"path":"src/lib.rs"}"#,
                )],
            )),
        },
        SessionEntry {
            id: "tool-1".to_string(),
            parent_id: Some("assistant-1".to_string()),
            timestamp: 1_717_514_800_002,
            kind: SessionEntryKind::Item(ConversationItem::tool_result(
                "call-1",
                vec![ContentBlock::Text("plain provider output".to_string())],
                false,
            )),
        },
        SessionEntry {
            id: "replay-start".to_string(),
            parent_id: Some("tool-1".to_string()),
            timestamp: 1_717_514_800_003,
            kind: SessionEntryKind::TranscriptReplay(TranscriptReplayItem::ToolActivity {
                activity: sample_tool_activity("call-1", "started"),
            }),
        },
        SessionEntry {
            id: "replay-final".to_string(),
            parent_id: Some("replay-start".to_string()),
            timestamp: 1_717_514_800_004,
            kind: SessionEntryKind::TranscriptReplay(TranscriptReplayItem::ToolActivity {
                activity: expected_activity.clone(),
            }),
        },
        SessionEntry {
            id: "terminal-final".to_string(),
            parent_id: Some("replay-final".to_string()),
            timestamp: 1_717_514_800_005,
            kind: SessionEntryKind::TranscriptReplay(TranscriptReplayItem::TerminalSnapshot {
                snapshot: expected_snapshot.clone(),
            }),
        },
    ];

    let resolved = super::resolve_state(&entries, "terminal-final").expect("state should resolve");

    assert_eq!(
        resolved
            .items
            .iter()
            .map(|item| item.item.text_content())
            .collect::<Vec<_>>(),
        vec!["editing", "plain provider output"]
    );
    assert_eq!(
        resolved.transcript,
        vec![
            TranscriptReplayItem::ToolActivity {
                activity: expected_activity
            },
            TranscriptReplayItem::TerminalSnapshot {
                snapshot: expected_snapshot
            },
        ]
    );
}

#[test]
fn resolve_returns_branch_specific_history() {
    let entries = branching_entries();

    let left_branch = super::resolve(&entries, "assistant-b").expect("left branch should resolve");
    let right_branch =
        super::resolve(&entries, "assistant-c").expect("right branch should resolve");

    assert_eq!(
        left_branch,
        vec![
            ConversationItem::text(Role::User, "hello"),
            ConversationItem::text(Role::Assistant, "branch-b"),
        ]
    );
    assert_eq!(
        right_branch,
        vec![
            ConversationItem::text(Role::User, "hello"),
            ConversationItem::text(Role::Assistant, "branch-c"),
        ]
    );
}

#[test]
fn resolve_keeps_following_appended_entries_on_selected_branch() {
    let entries = branching_with_append_entries();

    let resolved =
        super::resolve(&entries, "user-d").expect("appended branch history should resolve");

    assert_eq!(
        resolved,
        vec![
            ConversationItem::text(Role::User, "hello"),
            ConversationItem::text(Role::Assistant, "branch-c"),
            ConversationItem::text(Role::User, "branch follow-up"),
        ]
    );
}

#[test]
fn session_tree_snapshot_marks_active_path_and_user_rewind_prefill() {
    let entries = linear_history_entries();

    let snapshot = super::session_tree_snapshot(&entries).expect("linear tree should snapshot");

    assert_eq!(snapshot.current_row_id.as_deref(), Some("user-2"));
    assert!(snapshot.active_row_ids.contains("user-1"));
    assert!(snapshot.active_row_ids.contains("assistant-1"));
    assert!(snapshot.active_row_ids.contains("user-2"));
    let user = snapshot
        .rows
        .iter()
        .find(|row| row.id == "user-2")
        .expect("user-2 should exist");
    assert_eq!(user.kind, super::SessionTreeSnapshotRowKind::User);
    assert_eq!(user.rewind_target_id.as_deref(), Some("assistant-1"));
    assert_eq!(user.rewind_prefill.as_deref(), Some("follow up"));
    let assistant = snapshot
        .rows
        .iter()
        .find(|row| row.id == "assistant-1")
        .expect("assistant-1 should exist");
    assert_eq!(assistant.rewind_target_id.as_deref(), Some("assistant-1"));
    assert_eq!(assistant.rewind_prefill, None);
}

#[test]
fn session_tree_snapshot_allows_rewind_only_after_single_tool_call_result() {
    let entries = assistant_tool_batch_entries(&["call-1"], &["call-1"], true);

    let snapshot =
        super::session_tree_snapshot(&entries).expect("single tool batch should snapshot");
    let assistant = snapshot_row(&snapshot, "assistant-1");
    let tool = snapshot_row(&snapshot, "tool-1");

    assert_eq!(
        assistant.rewind_target_id, None,
        "assistant tool-call rows are attached to the following tool results"
    );
    assert_eq!(
        tool.rewind_target_id.as_deref(),
        Some("tool-1"),
        "the only tool result closes the provider-visible batch and is rewindable"
    );
}

#[test]
fn session_tree_snapshot_allows_rewind_only_after_final_tool_call_result() {
    let entries = assistant_tool_batch_entries(
        &["call-1", "call-2", "call-3"],
        &["call-1", "call-2", "call-3"],
        true,
    );

    let snapshot =
        super::session_tree_snapshot(&entries).expect("multi tool batch should snapshot");
    let assistant = snapshot_row(&snapshot, "assistant-1");
    let first_tool = snapshot_row(&snapshot, "tool-1");
    let second_tool = snapshot_row(&snapshot, "tool-2");
    let final_tool = snapshot_row(&snapshot, "tool-3");

    assert_eq!(
        assistant.rewind_target_id, None,
        "assistant tool-call rows must not be independently rewindable"
    );
    assert_eq!(
        first_tool.rewind_target_id, None,
        "intermediate tool results leave unresolved provider tool calls"
    );
    assert_eq!(
        second_tool.rewind_target_id, None,
        "intermediate tool results leave unresolved provider tool calls"
    );
    assert_eq!(
        final_tool.rewind_target_id.as_deref(),
        Some("tool-3"),
        "only the final tool result that resolves every call in the batch is rewindable"
    );
}

#[test]
fn session_tree_snapshot_does_not_rewind_incomplete_tool_call_batch() {
    let entries = assistant_tool_batch_entries(&["call-1", "call-2"], &["call-1"], true);

    let snapshot =
        super::session_tree_snapshot(&entries).expect("incomplete tool batch should snapshot");
    let assistant = snapshot_row(&snapshot, "assistant-1");
    let tool = snapshot_row(&snapshot, "tool-1");

    assert_eq!(
        assistant.rewind_target_id, None,
        "assistant tool-call rows are not safe restore targets without all results"
    );
    assert_eq!(
        tool.rewind_target_id, None,
        "a partial tool-result batch would still leave unresolved provider tool calls"
    );
}

#[test]
fn session_tree_snapshot_maps_reasoning_rewind_to_following_assistant() {
    let entries = vec![
        SessionEntry {
            id: "user-1".to_string(),
            parent_id: None,
            timestamp: 1,
            kind: SessionEntryKind::Item(ConversationItem::text(Role::User, "hello")),
        },
        SessionEntry {
            id: "reasoning-1".to_string(),
            parent_id: Some("user-1".to_string()),
            timestamp: 2,
            kind: SessionEntryKind::Item(ConversationItem::Reasoning {
                content: "thinking".to_string(),
                summary: None,
                encrypted: None,
            }),
        },
        SessionEntry {
            id: "assistant-1".to_string(),
            parent_id: Some("reasoning-1".to_string()),
            timestamp: 3,
            kind: SessionEntryKind::Item(ConversationItem::text(Role::Assistant, "answer")),
        },
        SessionEntry {
            id: "assistant-replay".to_string(),
            parent_id: Some("assistant-1".to_string()),
            timestamp: 4,
            kind: SessionEntryKind::TranscriptReplay(TranscriptReplayItem::Message {
                role: runtime_domain::session::TranscriptReplayRole::Assistant,
                content: "answer".to_string(),
            }),
        },
    ];

    let snapshot = super::session_tree_snapshot(&entries).expect("reasoning tree should snapshot");
    let reasoning = snapshot
        .rows
        .iter()
        .find(|row| row.id == "reasoning-1")
        .expect("reasoning row should remain visible");
    let assistant = snapshot
        .rows
        .iter()
        .find(|row| row.id == "assistant-1")
        .expect("assistant row should exist");

    assert_eq!(reasoning.kind, super::SessionTreeSnapshotRowKind::Reasoning);
    assert_eq!(
        reasoning.rewind_target_id, assistant.rewind_target_id,
        "reasoning should rewind to its owning assistant turn, not to itself"
    );
    assert_eq!(
        reasoning.rewind_target_id.as_deref(),
        Some("assistant-replay"),
        "reasoning should reuse the assistant row's final restore target"
    );
}

#[test]
fn session_tree_snapshot_marks_trailing_reasoning_as_not_rewindable() {
    let entries = vec![
        SessionEntry {
            id: "user-1".to_string(),
            parent_id: None,
            timestamp: 1,
            kind: SessionEntryKind::Item(ConversationItem::text(Role::User, "hello")),
        },
        SessionEntry {
            id: "reasoning-1".to_string(),
            parent_id: Some("user-1".to_string()),
            timestamp: 2,
            kind: SessionEntryKind::Item(ConversationItem::Reasoning {
                content: "thinking".to_string(),
                summary: None,
                encrypted: None,
            }),
        },
    ];

    let snapshot = super::session_tree_snapshot(&entries).expect("reasoning tree should snapshot");
    let reasoning = snapshot
        .rows
        .iter()
        .find(|row| row.id == "reasoning-1")
        .expect("reasoning row should remain visible");

    assert_eq!(reasoning.kind, super::SessionTreeSnapshotRowKind::Reasoning);
    assert_eq!(
        reasoning.rewind_target_id, None,
        "trailing reasoning without an assistant answer should be visible but not rewindable"
    );
}

#[test]
fn session_tree_snapshot_projects_only_logical_rows_without_replay_duplicates() {
    let session_id: SessionId = "01914a5c-3c7e-7a2b-8abc-1234567890ab"
        .parse()
        .expect("fixture session id should parse");
    let entries = vec![
        SessionEntry {
            id: "header".to_string(),
            parent_id: None,
            timestamp: 1_717_514_800_000,
            kind: SessionEntryKind::Header(SessionHeader {
                session_id,
                work_dir: PathBuf::from("/repo"),
                session_name: None,
                initial_model: "qwen3".to_string(),
                git_head: None,
                cli_version: None,
            }),
        },
        SessionEntry {
            id: "user-1".to_string(),
            parent_id: Some("header".to_string()),
            timestamp: 1_717_514_800_001,
            kind: SessionEntryKind::Item(ConversationItem::text(Role::User, "hello")),
        },
        SessionEntry {
            id: "user-replay".to_string(),
            parent_id: Some("user-1".to_string()),
            timestamp: 1_717_514_800_002,
            kind: SessionEntryKind::TranscriptReplay(TranscriptReplayItem::Message {
                role: runtime_domain::session::TranscriptReplayRole::User,
                content: "hello".to_string(),
            }),
        },
        SessionEntry {
            id: "assistant-1".to_string(),
            parent_id: Some("user-replay".to_string()),
            timestamp: 1_717_514_800_003,
            kind: SessionEntryKind::Item(ConversationItem::text(Role::Assistant, "answer")),
        },
        SessionEntry {
            id: "assistant-replay".to_string(),
            parent_id: Some("assistant-1".to_string()),
            timestamp: 1_717_514_800_004,
            kind: SessionEntryKind::TranscriptReplay(TranscriptReplayItem::Message {
                role: runtime_domain::session::TranscriptReplayRole::Assistant,
                content: "answer".to_string(),
            }),
        },
        SessionEntry {
            id: "config-1".to_string(),
            parent_id: Some("assistant-replay".to_string()),
            timestamp: 1_717_514_800_005,
            kind: SessionEntryKind::ConfigChange(ConfigSnapshot {
                provider_id: "local".to_string(),
                model: "qwen3".to_string(),
                system_prompt: None,
            }),
        },
        SessionEntry {
            id: "leaf-1".to_string(),
            parent_id: Some("config-1".to_string()),
            timestamp: 1_717_514_800_006,
            kind: SessionEntryKind::Leaf {
                target_id: Some("assistant-1".to_string()),
            },
        },
    ];

    let snapshot = super::session_tree_snapshot(&entries).expect("replay tree should snapshot");

    assert_eq!(
        snapshot
            .rows
            .iter()
            .map(|row| row.id.as_str())
            .collect::<Vec<_>>(),
        vec!["user-1", "assistant-1"],
        "tree projection should show one logical row per user-visible message only"
    );
    assert_eq!(
        snapshot
            .rows
            .iter()
            .map(|row| row.preview_content.as_str())
            .collect::<Vec<_>>(),
        vec!["hello", "answer"],
        "provider items and transcript replay records with the same visible content must not duplicate"
    );
}

#[test]
fn session_tree_snapshot_prefers_assistant_replay_content_for_preview() {
    let collapsed_hint = "… +26 lines (ctrl + t to view transcript)";
    let full_content = "assistant full line 1\nassistant full line 2";
    let entries = vec![
        SessionEntry {
            id: "user-1".to_string(),
            parent_id: None,
            timestamp: 1,
            kind: SessionEntryKind::Item(ConversationItem::text(Role::User, "hello")),
        },
        SessionEntry {
            id: "assistant-1".to_string(),
            parent_id: Some("user-1".to_string()),
            timestamp: 2,
            kind: SessionEntryKind::Item(ConversationItem::text(Role::Assistant, collapsed_hint)),
        },
        SessionEntry {
            id: "assistant-replay".to_string(),
            parent_id: Some("assistant-1".to_string()),
            timestamp: 3,
            kind: SessionEntryKind::TranscriptReplay(TranscriptReplayItem::Message {
                role: runtime_domain::session::TranscriptReplayRole::Assistant,
                content: full_content.to_string(),
            }),
        },
    ];

    let snapshot =
        super::session_tree_snapshot(&entries).expect("assistant replay tree should snapshot");
    let assistant = snapshot
        .rows
        .iter()
        .find(|row| row.id == "assistant-1")
        .expect("assistant row should exist");

    assert_eq!(assistant.preview_content, full_content);
    assert!(
        !assistant.summary.contains("ctrl + t"),
        "tree preview summary should be derived from full replay content, not collapsed UI text"
    );
}

#[test]
fn session_tree_snapshot_prefers_tool_replay_content_for_preview() {
    let collapsed_hint = "… +8 lines (ctrl + t to view transcript)";
    let entries = vec![
        SessionEntry {
            id: "assistant-1".to_string(),
            parent_id: None,
            timestamp: 1,
            kind: SessionEntryKind::Item(ConversationItem::assistant_with_tool_calls(
                "checking".to_string(),
                vec![ToolCall::new(
                    "call-1",
                    "read_file",
                    r#"{"path":"src/lib.rs"}"#,
                )],
            )),
        },
        SessionEntry {
            id: "tool-1".to_string(),
            parent_id: Some("assistant-1".to_string()),
            timestamp: 2,
            kind: SessionEntryKind::Item(ConversationItem::tool_result(
                "call-1",
                vec![ContentBlock::Text(collapsed_hint.to_string())],
                false,
            )),
        },
        SessionEntry {
            id: "tool-replay".to_string(),
            parent_id: Some("tool-1".to_string()),
            timestamp: 3,
            kind: SessionEntryKind::TranscriptReplay(TranscriptReplayItem::ToolActivity {
                activity: sample_tool_activity("call-1", "full provider output"),
            }),
        },
    ];

    let snapshot =
        super::session_tree_snapshot(&entries).expect("tool replay tree should snapshot");
    let tool = snapshot
        .rows
        .iter()
        .find(|row| row.id == "tool-1")
        .expect("tool row should exist");

    assert_eq!(tool.preview_content, "full provider output");
    assert!(
        !tool.summary.contains("ctrl + t"),
        "tool preview summary should be derived from replay output, not collapsed UI text"
    );
}

#[test]
fn session_tree_snapshot_projects_assistant_tool_calls_into_debug_preview_replay() {
    let entries = vec![
        SessionEntry {
            id: "user-1".to_string(),
            parent_id: None,
            timestamp: 1,
            kind: SessionEntryKind::Item(ConversationItem::text(Role::User, "inspect")),
        },
        SessionEntry {
            id: "assistant-1".to_string(),
            parent_id: Some("user-1".to_string()),
            timestamp: 2,
            kind: SessionEntryKind::Item(ConversationItem::assistant_with_tool_calls(
                "I will inspect the file.".to_string(),
                vec![ToolCall::new(
                    "call-1",
                    "read_file",
                    r#"{"path":"Cargo.toml","limit":20}"#,
                )],
            )),
        },
    ];

    let snapshot =
        super::session_tree_snapshot(&entries).expect("assistant tool call tree should snapshot");
    let assistant = snapshot
        .rows
        .iter()
        .find(|row| row.id == "assistant-1")
        .expect("assistant row should exist");

    assert_eq!(assistant.preview_replay_items.len(), 1);
    assert!(matches!(
        &assistant.preview_replay_items[0],
        TranscriptReplayItem::Message {
            role: runtime_domain::session::TranscriptReplayRole::Assistant,
            content,
        } if content.contains("I will inspect the file.")
            && content.contains("Tool call `read_file` (call-1)")
            && content.contains("```json")
            && content.contains("\"path\": \"Cargo.toml\"")
            && content.contains("\"limit\": 20")
    ));
}

#[test]
fn session_tree_snapshot_projects_tool_activity_into_debug_preview_replay() {
    let activity = RuntimeToolActivity {
        activity_id: "call-1".to_string(),
        title: "Run cargo test".to_string(),
        kind: RuntimeToolKind::Execute,
        status: RuntimeToolActivityStatus::Completed,
        content: Vec::new(),
        locations: Vec::new(),
        raw_input: Some(r#"{"command":"cargo test"}"#.into()),
        raw_output: Some(
            (1..=8)
                .map(|line| format!("test output line {line}"))
                .collect::<Vec<_>>()
                .join("\n")
                .into(),
        ),
    };
    let entries = vec![
        SessionEntry {
            id: "assistant-1".to_string(),
            parent_id: None,
            timestamp: 1,
            kind: SessionEntryKind::Item(ConversationItem::assistant_with_tool_calls(
                "running tests".to_string(),
                vec![ToolCall::new(
                    "call-1",
                    "bash",
                    r#"{"command":"cargo test"}"#,
                )],
            )),
        },
        SessionEntry {
            id: "tool-1".to_string(),
            parent_id: Some("assistant-1".to_string()),
            timestamp: 2,
            kind: SessionEntryKind::Item(ConversationItem::tool_result(
                "call-1",
                vec![ContentBlock::Text("compact result".to_string())],
                false,
            )),
        },
        SessionEntry {
            id: "tool-replay".to_string(),
            parent_id: Some("tool-1".to_string()),
            timestamp: 3,
            kind: SessionEntryKind::TranscriptReplay(TranscriptReplayItem::ToolActivity {
                activity: activity.clone(),
            }),
        },
    ];

    let snapshot =
        super::session_tree_snapshot(&entries).expect("tool activity tree should snapshot");
    let tool = snapshot
        .rows
        .iter()
        .find(|row| row.id == "tool-1")
        .expect("tool row should exist");

    assert_eq!(
        tool.preview_replay_items,
        vec![TranscriptReplayItem::ToolActivity { activity }]
    );
}

#[test]
fn session_tree_snapshot_keeps_linear_logical_rows_flat() {
    let entries = linear_history_entries();

    let snapshot = super::session_tree_snapshot(&entries).expect("linear tree should snapshot");

    assert_eq!(
        snapshot
            .rows
            .iter()
            .map(|row| (row.id.as_str(), row.display_depth))
            .collect::<Vec<_>>(),
        vec![("user-1", 0), ("assistant-1", 0), ("user-2", 0)],
        "linear visible history should not inherit physical parent-chain depth as visual indent"
    );
}

#[test]
fn session_tree_snapshot_lists_only_current_leaf_path_rows() {
    let entries = branching_with_append_entries();

    let snapshot =
        super::session_tree_snapshot(&entries).expect("branch path tree should snapshot");

    assert_eq!(
        snapshot
            .rows
            .iter()
            .map(|row| row.id.as_str())
            .collect::<Vec<_>>(),
        vec!["user-a", "assistant-c", "user-d"],
        "path tree must not include messages exclusive to sibling branches"
    );
    assert_eq!(snapshot.current_row_id.as_deref(), Some("user-d"));
}

#[test]
fn session_tree_snapshot_lists_branch_choices_at_fork_parent() {
    let entries = branching_with_append_entries();

    let snapshot = super::session_tree_snapshot(&entries).expect("branch choices should snapshot");
    let branch_parent = snapshot
        .rows
        .iter()
        .find(|row| row.id == "user-a")
        .expect("fork parent should be visible on the active path");

    assert_eq!(
        branch_parent
            .branch_choices
            .iter()
            .map(|branch| {
                (
                    branch.branch.branch_row_id.as_str(),
                    branch.branch.subtree_leaf_id.as_str(),
                    branch.branch.latest_row_id.as_str(),
                    branch.branch.display_summary.as_str(),
                    branch.branch.kind,
                    branch.branch.is_current,
                    branch.branch.message_count,
                    branch.branch.branch_created_at_ms,
                    branch.branch.latest_updated_at_ms,
                )
            })
            .collect::<Vec<_>>(),
        vec![
            (
                "assistant-b",
                "assistant-b",
                "assistant-b",
                "branch-b",
                super::SessionTreeSnapshotRowKind::Assistant,
                false,
                1,
                1_717_514_800_002,
                1_717_514_800_002,
            ),
            (
                "assistant-c",
                "user-d",
                "user-d",
                "branch follow-up",
                super::SessionTreeSnapshotRowKind::User,
                true,
                2,
                1_717_514_800_003,
                1_717_514_800_004,
            ),
        ],
        "branch picker choices should summarize each sibling branch by its subtree leaf"
    );
}

#[test]
fn session_branch_preview_snapshot_starts_at_fork_parent() {
    let entries = branching_after_visible_context_entries();

    let preview = super::session_branch_preview_snapshot(&entries, "assistant-b")
        .expect("branch preview should snapshot");

    assert_eq!(
        preview
            .rows
            .iter()
            .map(|row| (row.id.as_str(), row.display_depth))
            .collect::<Vec<_>>(),
        vec![("user-a", 0), ("assistant-b", 1)],
        "branch preview should skip visible ancestors before the fork point"
    );
    assert_eq!(preview.current_row_id.as_deref(), Some("assistant-b"));
}

#[test]
fn session_tree_snapshot_for_hypothetical_leaf_matches_after_switch() {
    let entries = branching_with_append_entries();

    let preview = super::session_tree_snapshot_for_leaf(&entries, "assistant-b")
        .expect("hypothetical branch should snapshot");
    let mut switched_entries = entries.clone();
    switched_entries.push(SessionEntry {
        id: "leaf-switch".to_string(),
        parent_id: Some("user-d".to_string()),
        timestamp: 1_717_514_800_005,
        kind: SessionEntryKind::Leaf {
            target_id: Some("assistant-b".to_string()),
        },
    });
    let switched =
        super::session_tree_snapshot(&switched_entries).expect("switched branch should snapshot");

    assert_eq!(
        preview
            .rows
            .iter()
            .map(|row| row.id.as_str())
            .collect::<Vec<_>>(),
        switched
            .rows
            .iter()
            .map(|row| row.id.as_str())
            .collect::<Vec<_>>(),
        "preview path must match the committed path after switching to that branch"
    );
    assert_eq!(preview.current_row_id, switched.current_row_id);
}

#[test]
fn session_branch_tree_snapshot_lists_branch_roots_with_tree_parents() {
    let entries = nested_branch_tree_entries();

    let snapshot = super::session_branch_tree_snapshot(&entries)
        .expect("branch tree should snapshot nested branches");

    assert_eq!(snapshot.nodes.len(), 5);
    assert_eq!(snapshot.total_message_count, 7);
    assert_eq!(
        snapshot.current_branch_row_id.as_deref(),
        Some("user-b-alt")
    );

    let root = branch_tree_node(&snapshot, "user-root");
    assert_eq!(root.parent_branch_row_id, None);
    assert_eq!(root.branch.message_count, 7);

    let alpha = branch_tree_node(&snapshot, "assistant-a");
    assert_eq!(alpha.parent_branch_row_id.as_deref(), Some("user-root"));
    assert_eq!(alpha.branch.subtree_leaf_id, "assistant-a");
    assert_eq!(alpha.branch.message_count, 1);

    let beta = branch_tree_node(&snapshot, "assistant-b");
    assert_eq!(beta.parent_branch_row_id.as_deref(), Some("user-root"));
    assert_eq!(beta.branch.message_count, 5);

    let follow = branch_tree_node(&snapshot, "user-b-follow");
    assert_eq!(follow.parent_branch_row_id.as_deref(), Some("assistant-b"));
    assert_eq!(follow.branch.message_count, 2);
    assert!(!follow.branch.is_current);

    let alternate = branch_tree_node(&snapshot, "user-b-alt");
    assert_eq!(
        alternate.parent_branch_row_id.as_deref(),
        Some("assistant-b")
    );
    assert_eq!(alternate.branch.message_count, 2);
    assert!(alternate.branch.is_current);
    assert_eq!(alternate.branch.display_summary, "alt answer");
}

#[test]
fn session_tree_snapshot_indents_true_sibling_branches() {
    let entries = branching_entries();

    let snapshot = super::session_tree_snapshot(&entries).expect("branching tree should snapshot");

    assert_eq!(
        snapshot
            .rows
            .iter()
            .map(|row| (row.id.as_str(), row.display_depth))
            .collect::<Vec<_>>(),
        vec![("user-a", 0), ("assistant-c", 1)],
        "path tree should keep current branch indent while omitting sibling-only rows"
    );
}

#[test]
fn session_tree_snapshot_indents_rewinded_user_branch_under_outer_assistant() {
    let entries = nested_rewind_user_branch_entries();
    let snapshot =
        super::session_tree_snapshot(&entries).expect("nested rewind tree should snapshot");

    assert_eq!(
        snapshot
            .rows
            .iter()
            .map(|row| (row.id.as_str(), row.display_depth))
            .collect::<Vec<_>>(),
        vec![
            ("user-root", 0),
            ("reason-root", 0),
            ("assistant-root", 0),
            ("user-a", 1),
            ("reason-a", 1),
            ("assistant-a", 1),
            ("user-c", 2),
            ("reason-c", 2),
            ("assistant-c", 2),
        ],
        "path tree should preserve nested branch depth without listing inactive sibling paths"
    );
}

#[test]
fn session_tree_snapshot_indents_each_rewind_branch_progressively_through_config_chain() {
    let entries = nested_config_rewind_chain_entries();
    let snapshot =
        super::session_tree_snapshot(&entries).expect("nested config rewind tree should snapshot");

    assert_eq!(
        snapshot
            .rows
            .iter()
            .map(|row| (row.id.as_str(), row.display_depth))
            .collect::<Vec<_>>(),
        vec![
            ("user-root", 0),
            ("reason-root", 0),
            ("assistant-root", 0),
            ("user-branch-4", 3),
            ("reason-branch-4", 3),
            ("assistant-branch-4", 3),
        ],
        "path tree should keep the active rewind branch's full computed depth"
    );
}

#[test]
fn session_tree_snapshot_keeps_linear_follow_up_after_branch_at_branch_depth() {
    let entries = branching_with_append_entries();
    let snapshot =
        super::session_tree_snapshot(&entries).expect("branch append tree should snapshot");

    assert_eq!(
        snapshot
            .rows
            .iter()
            .map(|row| (row.id.as_str(), row.display_depth))
            .collect::<Vec<_>>(),
        vec![("user-a", 0), ("assistant-c", 1), ("user-d", 1)],
        "linear follow-up after a selected branch should stay at the branch depth"
    );
}

#[test]
fn resolve_follows_requested_leaf_entry_target() {
    let entries = entries_with_trailing_leaf_override();

    let resolved =
        super::resolve(&entries, "leaf-1").expect("leaf entry should redirect canonical history");

    assert_eq!(resolved, vec![ConversationItem::text(Role::User, "hello")]);
}

#[test]
fn resolve_keeps_explicit_non_leaf_selection_when_a_trailing_leaf_exists() {
    let entries = entries_with_trailing_leaf_override();

    let resolved = super::resolve(&entries, "assistant-c")
        .expect("leaf override should redirect canonical history");

    assert_eq!(
        resolved,
        vec![
            ConversationItem::text(Role::User, "hello"),
            ConversationItem::text(Role::Assistant, "branch-c"),
        ]
    );
}

#[test]
fn resolve_replaces_compacted_history_with_summary_and_kept_tail() {
    let entries = entries_with_compaction();

    let resolved = super::resolve(&entries, "assistant-d")
        .expect("compacted history should resolve to summary plus kept tail");

    assert_eq!(
        resolved,
        vec![
            ConversationItem::system(vec![ContentBlock::Text("compacted summary".to_string(),)]),
            ConversationItem::text(Role::Assistant, "keep me"),
            ConversationItem::text(Role::Assistant, "after compaction"),
        ]
    );
}

#[test]
fn resolve_uses_latest_compaction_boundary() {
    let entries = entries_with_multiple_compactions();

    let resolved = super::resolve(&entries, "assistant-f").expect("latest compaction should win");

    assert_eq!(
        resolved,
        vec![
            ConversationItem::system(vec![ContentBlock::Text("latest summary".to_string(),)]),
            ConversationItem::text(Role::Assistant, "second keep"),
            ConversationItem::text(Role::Assistant, "after latest compaction"),
        ]
    );
}

#[test]
fn resolve_uses_latest_non_leaf_entry_when_requested_leaf_resets_target() {
    let entries = entries_with_trailing_leaf_reset();

    let resolved = super::resolve(&entries, "leaf-reset")
        .expect("leaf reset should fall back to the latest concrete entry");

    assert_eq!(
        resolved,
        vec![
            ConversationItem::text(Role::User, "hello"),
            ConversationItem::text(Role::Assistant, "branch-c"),
        ]
    );
}

#[test]
fn resolve_returns_empty_history_for_header_only_session() {
    let entries = header_only_entries();

    let resolved = super::resolve(&entries, "header").expect("header-only session resolves");

    assert!(resolved.is_empty());
}

#[test]
fn resolve_skips_config_and_branch_summary_entries() {
    let entries = entries_with_non_history_metadata();

    let resolved = super::resolve(&entries, "assistant-c")
        .expect("non-history metadata should not appear in canonical history");

    assert_eq!(
        resolved,
        vec![
            ConversationItem::text(Role::User, "hello"),
            ConversationItem::text(Role::Assistant, "final reply"),
        ]
    );
}

#[test]
fn resolve_reports_missing_parent_on_selected_path() {
    let entries = entries_with_dangling_parent();

    let error =
        super::resolve(&entries, "assistant-1").expect_err("dangling parent should fail resolve");

    assert_eq!(error, ResolveError::DanglingParent("missing".to_string()));
}

#[test]
fn resolve_reports_cycle_on_selected_path() {
    let entries = entries_with_cycle();

    let error = super::resolve(&entries, "assistant-b").expect_err("cycle should fail resolve");

    assert_eq!(error, ResolveError::CycleDetected);
}

#[test]
fn resolve_reports_missing_leaf_target_from_requested_leaf_entry() {
    let entries = entries_with_missing_leaf_target();

    let error = super::resolve(&entries, "leaf-missing")
        .expect_err("missing leaf target should fail resolve");

    assert_eq!(
        error,
        ResolveError::LeafNotFound("missing-target".to_string())
    );
}

#[test]
fn resolve_reports_invalid_compaction_target() {
    let entries = entries_with_invalid_compaction_target();

    let error = super::resolve(&entries, "assistant-d")
        .expect_err("unknown compaction target should fail resolve");

    assert_eq!(
        error,
        ResolveError::InvalidCompactionTarget("missing-target".to_string())
    );
}

#[test]
fn resolve_reports_duplicate_entry_id() {
    let entries = entries_with_duplicate_id();

    let error =
        super::resolve(&entries, "assistant-1").expect_err("duplicate id should fail resolve");

    assert_eq!(error, ResolveError::DuplicateId("assistant-1".to_string()));
}

#[test]
fn resolve_rejects_compaction_target_that_is_not_an_item() {
    let entries = entries_with_non_item_compaction_target();

    let error = super::resolve(&entries, "assistant-d")
        .expect_err("non-item compaction target should fail resolve");

    assert_eq!(
        error,
        ResolveError::InvalidCompactionTarget("config-1".to_string())
    );
}

#[test]
fn resolve_handles_large_linear_history() {
    let entries = long_linear_history_entries(1_000);

    let resolved = super::resolve(&entries, "assistant-999")
        .expect("large linear history should resolve successfully");

    assert_eq!(resolved.len(), 1_000);
    assert_eq!(
        resolved.first(),
        Some(&ConversationItem::text(Role::Assistant, "message-0"))
    );
    assert_eq!(
        resolved.last(),
        Some(&ConversationItem::text(Role::Assistant, "message-999"))
    );
}
fn snapshot_row<'a>(
    snapshot: &'a super::SessionTreeSnapshot,
    row_id: &str,
) -> &'a super::SessionTreeSnapshotRow {
    snapshot
        .rows
        .iter()
        .find(|row| row.id == row_id)
        .unwrap_or_else(|| panic!("{row_id} should exist in tree snapshot"))
}

fn branch_tree_node<'a>(
    snapshot: &'a super::SessionBranchTreeSnapshot,
    branch_row_id: &str,
) -> &'a super::SessionBranchTreeSnapshotNode {
    snapshot
        .nodes
        .iter()
        .find(|node| node.branch.branch_row_id == branch_row_id)
        .unwrap_or_else(|| panic!("{branch_row_id} should exist in branch tree snapshot"))
}

fn assistant_tool_batch_entries(
    call_ids: &[&str],
    result_call_ids: &[&str],
    include_replay_before_results: bool,
) -> Vec<SessionEntry> {
    let mut entries = vec![
        SessionEntry {
            id: "user-1".to_string(),
            parent_id: None,
            timestamp: 1,
            kind: SessionEntryKind::Item(ConversationItem::text(Role::User, "read files")),
        },
        SessionEntry {
            id: "assistant-1".to_string(),
            parent_id: Some("user-1".to_string()),
            timestamp: 2,
            kind: SessionEntryKind::Item(ConversationItem::assistant_with_tool_calls(
                "reading".to_string(),
                call_ids
                    .iter()
                    .map(|call_id| ToolCall::new(*call_id, "read", "{}"))
                    .collect(),
            )),
        },
    ];

    let mut parent_id = "assistant-1".to_string();
    let mut timestamp = 3;
    if include_replay_before_results {
        for call_id in call_ids {
            let replay_id = format!("replay-{call_id}");
            entries.push(SessionEntry {
                id: replay_id.clone(),
                parent_id: Some(parent_id),
                timestamp,
                kind: SessionEntryKind::TranscriptReplay(TranscriptReplayItem::ToolActivity {
                    activity: sample_tool_activity(call_id, "completed"),
                }),
            });
            parent_id = replay_id;
            timestamp += 1;
        }
    }

    for (index, call_id) in result_call_ids.iter().enumerate() {
        let tool_id = format!("tool-{}", index + 1);
        entries.push(SessionEntry {
            id: tool_id.clone(),
            parent_id: Some(parent_id),
            timestamp,
            kind: SessionEntryKind::Item(ConversationItem::tool_result(
                *call_id,
                vec![ContentBlock::Text(format!("result {}", index + 1))],
                false,
            )),
        });
        parent_id = tool_id;
        timestamp += 1;
    }

    entries
}

fn nested_branch_tree_entries() -> Vec<SessionEntry> {
    vec![
        SessionEntry {
            id: "header".to_string(),
            parent_id: None,
            timestamp: 1_717_514_800_000,
            kind: SessionEntryKind::Header(SessionHeader {
                session_id: "01914a5c-3c7e-7a2b-8abc-1234567890ab"
                    .parse()
                    .expect("fixture session id should parse"),
                work_dir: PathBuf::from("/repo"),
                session_name: None,
                initial_model: "qwen3".to_string(),
                git_head: None,
                cli_version: None,
            }),
        },
        SessionEntry {
            id: "user-root".to_string(),
            parent_id: Some("header".to_string()),
            timestamp: 1_717_514_800_001,
            kind: SessionEntryKind::Item(ConversationItem::text(Role::User, "root question")),
        },
        SessionEntry {
            id: "assistant-a".to_string(),
            parent_id: Some("user-root".to_string()),
            timestamp: 1_717_514_800_002,
            kind: SessionEntryKind::Item(ConversationItem::text(Role::Assistant, "alpha")),
        },
        SessionEntry {
            id: "assistant-b".to_string(),
            parent_id: Some("user-root".to_string()),
            timestamp: 1_717_514_800_003,
            kind: SessionEntryKind::Item(ConversationItem::text(Role::Assistant, "beta")),
        },
        SessionEntry {
            id: "user-b-follow".to_string(),
            parent_id: Some("assistant-b".to_string()),
            timestamp: 1_717_514_800_004,
            kind: SessionEntryKind::Item(ConversationItem::text(Role::User, "follow")),
        },
        SessionEntry {
            id: "assistant-b-follow".to_string(),
            parent_id: Some("user-b-follow".to_string()),
            timestamp: 1_717_514_800_005,
            kind: SessionEntryKind::Item(ConversationItem::text(Role::Assistant, "follow answer")),
        },
        SessionEntry {
            id: "user-b-alt".to_string(),
            parent_id: Some("assistant-b".to_string()),
            timestamp: 1_717_514_800_006,
            kind: SessionEntryKind::Item(ConversationItem::text(Role::User, "alt")),
        },
        SessionEntry {
            id: "assistant-b-alt".to_string(),
            parent_id: Some("user-b-alt".to_string()),
            timestamp: 1_717_514_800_007,
            kind: SessionEntryKind::Item(ConversationItem::text(Role::Assistant, "alt answer")),
        },
    ]
}

fn sample_tool_activity(activity_id: &str, text: &str) -> RuntimeToolActivity {
    RuntimeToolActivity {
        activity_id: activity_id.to_string(),
        title: format!("Write {text}"),
        kind: RuntimeToolKind::Write,
        status: RuntimeToolActivityStatus::Completed,
        content: vec![RuntimeToolActivityContent::Diff {
            path: "src/lib.rs".to_string(),
            old_text: Some("old".to_string()),
            new_text: text.to_string(),
            is_truncated: false,
        }],
        locations: Vec::new(),
        raw_input: Some(RuntimeToolActivityRawValue::from(
            serde_json::json!({"path":"src/lib.rs"}),
        )),
        raw_output: Some(RuntimeToolActivityRawValue::tool_result(
            text.to_string(),
            None,
        )),
    }
}

fn linear_history_entries() -> Vec<SessionEntry> {
    let session_id: SessionId = "01914a5c-3c7e-7a2b-8abc-1234567890ab"
        .parse()
        .expect("fixture session id should parse");

    vec![
        SessionEntry {
            id: "header".to_string(),
            parent_id: None,
            timestamp: 1_717_514_800_000,
            kind: SessionEntryKind::Header(SessionHeader {
                session_id,
                work_dir: PathBuf::from("/repo"),
                session_name: None,
                initial_model: "gpt-4.1".to_string(),
                git_head: Some("abc123".to_string()),
                cli_version: Some("0.5.2".to_string()),
            }),
        },
        SessionEntry {
            id: "user-1".to_string(),
            parent_id: Some("header".to_string()),
            timestamp: 1_717_514_800_001,
            kind: SessionEntryKind::Item(ConversationItem::text(Role::User, "hello")),
        },
        SessionEntry {
            id: "assistant-1".to_string(),
            parent_id: Some("user-1".to_string()),
            timestamp: 1_717_514_800_002,
            kind: SessionEntryKind::Item(ConversationItem::text(Role::Assistant, "hi")),
        },
        SessionEntry {
            id: "user-2".to_string(),
            parent_id: Some("assistant-1".to_string()),
            timestamp: 1_717_514_800_003,
            kind: SessionEntryKind::Item(ConversationItem::text(Role::User, "follow up")),
        },
    ]
}

fn branching_entries() -> Vec<SessionEntry> {
    let session_id: SessionId = "01914a5c-3c7e-7a2b-8abc-1234567890ab"
        .parse()
        .expect("fixture session id should parse");

    vec![
        SessionEntry {
            id: "header".to_string(),
            parent_id: None,
            timestamp: 1_717_514_800_000,
            kind: SessionEntryKind::Header(SessionHeader {
                session_id,
                work_dir: PathBuf::from("/repo"),
                session_name: None,
                initial_model: "gpt-4.1".to_string(),
                git_head: Some("abc123".to_string()),
                cli_version: Some("0.5.2".to_string()),
            }),
        },
        SessionEntry {
            id: "user-a".to_string(),
            parent_id: Some("header".to_string()),
            timestamp: 1_717_514_800_001,
            kind: SessionEntryKind::Item(ConversationItem::text(Role::User, "hello")),
        },
        SessionEntry {
            id: "assistant-b".to_string(),
            parent_id: Some("user-a".to_string()),
            timestamp: 1_717_514_800_002,
            kind: SessionEntryKind::Item(ConversationItem::text(Role::Assistant, "branch-b")),
        },
        SessionEntry {
            id: "assistant-c".to_string(),
            parent_id: Some("user-a".to_string()),
            timestamp: 1_717_514_800_003,
            kind: SessionEntryKind::Item(ConversationItem::text(Role::Assistant, "branch-c")),
        },
    ]
}

fn branching_with_append_entries() -> Vec<SessionEntry> {
    let mut entries = branching_entries();
    entries.push(SessionEntry {
        id: "user-d".to_string(),
        parent_id: Some("assistant-c".to_string()),
        timestamp: 1_717_514_800_004,
        kind: SessionEntryKind::Item(ConversationItem::text(Role::User, "branch follow-up")),
    });
    entries
}

fn branching_after_visible_context_entries() -> Vec<SessionEntry> {
    let session_id: SessionId = "01914a5c-3c7e-7a2b-8abc-1234567890ab"
        .parse()
        .expect("fixture session id should parse");

    vec![
        SessionEntry {
            id: "header".to_string(),
            parent_id: None,
            timestamp: 1_717_514_800_000,
            kind: SessionEntryKind::Header(SessionHeader {
                session_id,
                work_dir: PathBuf::from("/repo"),
                session_name: None,
                initial_model: "gpt-4.1".to_string(),
                git_head: Some("abc123".to_string()),
                cli_version: Some("0.5.2".to_string()),
            }),
        },
        SessionEntry {
            id: "user-context".to_string(),
            parent_id: Some("header".to_string()),
            timestamp: 1_717_514_800_001,
            kind: SessionEntryKind::Item(ConversationItem::text(Role::User, "context")),
        },
        SessionEntry {
            id: "assistant-context".to_string(),
            parent_id: Some("user-context".to_string()),
            timestamp: 1_717_514_800_002,
            kind: SessionEntryKind::Item(ConversationItem::text(Role::Assistant, "not shown")),
        },
        SessionEntry {
            id: "user-a".to_string(),
            parent_id: Some("assistant-context".to_string()),
            timestamp: 1_717_514_800_003,
            kind: SessionEntryKind::Item(ConversationItem::text(Role::User, "hello")),
        },
        SessionEntry {
            id: "assistant-b".to_string(),
            parent_id: Some("user-a".to_string()),
            timestamp: 1_717_514_800_004,
            kind: SessionEntryKind::Item(ConversationItem::text(Role::Assistant, "branch-b")),
        },
        SessionEntry {
            id: "assistant-c".to_string(),
            parent_id: Some("user-a".to_string()),
            timestamp: 1_717_514_800_005,
            kind: SessionEntryKind::Item(ConversationItem::text(Role::Assistant, "branch-c")),
        },
        SessionEntry {
            id: "user-d".to_string(),
            parent_id: Some("assistant-c".to_string()),
            timestamp: 1_717_514_800_006,
            kind: SessionEntryKind::Item(ConversationItem::text(Role::User, "branch follow-up")),
        },
    ]
}

fn nested_rewind_user_branch_entries() -> Vec<SessionEntry> {
    let session_id: SessionId = "01914a5c-3c7e-7a2b-8abc-1234567890ab"
        .parse()
        .expect("fixture session id should parse");

    vec![
        SessionEntry {
            id: "header".to_string(),
            parent_id: None,
            timestamp: 1_717_514_800_000,
            kind: SessionEntryKind::Header(SessionHeader {
                session_id,
                work_dir: PathBuf::from("/repo"),
                session_name: None,
                initial_model: "gpt-4.1".to_string(),
                git_head: Some("abc123".to_string()),
                cli_version: Some("0.5.2".to_string()),
            }),
        },
        SessionEntry {
            id: "user-root".to_string(),
            parent_id: Some("header".to_string()),
            timestamp: 1_717_514_800_001,
            kind: SessionEntryKind::Item(ConversationItem::text(Role::User, "你好哦")),
        },
        SessionEntry {
            id: "reason-root".to_string(),
            parent_id: Some("user-root".to_string()),
            timestamp: 1_717_514_800_002,
            kind: SessionEntryKind::Item(ConversationItem::Reasoning {
                content: "think root".to_string(),
                summary: None,
                encrypted: None,
            }),
        },
        SessionEntry {
            id: "assistant-root".to_string(),
            parent_id: Some("reason-root".to_string()),
            timestamp: 1_717_514_800_003,
            kind: SessionEntryKind::Item(ConversationItem::text(Role::Assistant, "root reply")),
        },
        SessionEntry {
            id: "user-inactive".to_string(),
            parent_id: Some("assistant-root".to_string()),
            timestamp: 1_717_514_800_004,
            kind: SessionEntryKind::Item(ConversationItem::text(Role::User, "skipped branch")),
        },
        SessionEntry {
            id: "user-a".to_string(),
            parent_id: Some("assistant-root".to_string()),
            timestamp: 1_717_514_800_005,
            kind: SessionEntryKind::Item(ConversationItem::text(Role::User, "你是谁")),
        },
        SessionEntry {
            id: "reason-a".to_string(),
            parent_id: Some("user-a".to_string()),
            timestamp: 1_717_514_800_005,
            kind: SessionEntryKind::Item(ConversationItem::Reasoning {
                content: "think a".to_string(),
                summary: None,
                encrypted: None,
            }),
        },
        SessionEntry {
            id: "assistant-a".to_string(),
            parent_id: Some("reason-a".to_string()),
            timestamp: 1_717_514_800_006,
            kind: SessionEntryKind::Item(ConversationItem::text(Role::Assistant, "你好！")),
        },
        SessionEntry {
            id: "user-b".to_string(),
            parent_id: Some("assistant-a".to_string()),
            timestamp: 1_717_514_800_007,
            kind: SessionEntryKind::Item(ConversationItem::text(Role::User, "linear follow up")),
        },
        SessionEntry {
            id: "reason-b".to_string(),
            parent_id: Some("user-b".to_string()),
            timestamp: 1_717_514_800_008,
            kind: SessionEntryKind::Item(ConversationItem::Reasoning {
                content: "think b".to_string(),
                summary: None,
                encrypted: None,
            }),
        },
        SessionEntry {
            id: "assistant-b".to_string(),
            parent_id: Some("reason-b".to_string()),
            timestamp: 1_717_514_800_009,
            kind: SessionEntryKind::Item(ConversationItem::text(Role::Assistant, "linear reply")),
        },
        SessionEntry {
            id: "leaf-1".to_string(),
            parent_id: Some("assistant-b".to_string()),
            timestamp: 1_717_514_800_010,
            kind: SessionEntryKind::Leaf {
                target_id: Some("assistant-a".to_string()),
            },
        },
        SessionEntry {
            id: "user-c".to_string(),
            parent_id: Some("assistant-a".to_string()),
            timestamp: 1_717_514_800_011,
            kind: SessionEntryKind::Item(ConversationItem::text(Role::User, "你能做什么")),
        },
        SessionEntry {
            id: "reason-c".to_string(),
            parent_id: Some("user-c".to_string()),
            timestamp: 1_717_514_800_012,
            kind: SessionEntryKind::Item(ConversationItem::Reasoning {
                content: "think c".to_string(),
                summary: None,
                encrypted: None,
            }),
        },
        SessionEntry {
            id: "assistant-c".to_string(),
            parent_id: Some("reason-c".to_string()),
            timestamp: 1_717_514_800_013,
            kind: SessionEntryKind::Item(ConversationItem::text(Role::Assistant, "nested tail")),
        },
    ]
}

fn nested_config_rewind_chain_entries() -> Vec<SessionEntry> {
    let session_id: SessionId = "01914a5c-3c7e-7a2b-8abc-1234567890ab"
        .parse()
        .expect("fixture session id should parse");

    let mut timestamp = 1_717_514_800_000i64;
    let mut next_timestamp = || {
        let value = timestamp;
        timestamp += 1;
        value
    };

    let config_snapshot = || ConfigSnapshot {
        provider_id: "opencode".to_string(),
        model: "gpt-4.1".to_string(),
        system_prompt: None,
    };

    // 复刻真实 session：rewind 时新建的 ConfigChange 总是挂到上一次 ConfigChange，
    // 形成隐藏的 fork 链——这是触发本次 bug 的关键拓扑。
    let mut entries = vec![
        SessionEntry {
            id: "header".to_string(),
            parent_id: None,
            timestamp: next_timestamp(),
            kind: SessionEntryKind::Header(SessionHeader {
                session_id,
                work_dir: PathBuf::from("/repo"),
                session_name: None,
                initial_model: "gpt-4.1".to_string(),
                git_head: Some("abc123".to_string()),
                cli_version: Some("0.6.0".to_string()),
            }),
        },
        SessionEntry {
            id: "config-root".to_string(),
            parent_id: Some("header".to_string()),
            timestamp: next_timestamp(),
            kind: SessionEntryKind::ConfigChange(config_snapshot()),
        },
    ];

    let push_user_assistant_chain = |entries: &mut Vec<SessionEntry>,
                                     ts: &mut dyn FnMut() -> i64,
                                     slug: &str,
                                     parent: &str|
     -> String {
        let user_id = format!("user-{slug}");
        entries.push(SessionEntry {
            id: user_id.clone(),
            parent_id: Some(parent.to_string()),
            timestamp: ts(),
            kind: SessionEntryKind::Item(ConversationItem::text(
                Role::User,
                format!("question {slug}"),
            )),
        });
        let reason_id = format!("reason-{slug}");
        entries.push(SessionEntry {
            id: reason_id.clone(),
            parent_id: Some(user_id),
            timestamp: ts(),
            kind: SessionEntryKind::Item(ConversationItem::Reasoning {
                content: format!("think {slug}"),
                summary: None,
                encrypted: None,
            }),
        });
        let assistant_id = format!("assistant-{slug}");
        entries.push(SessionEntry {
            id: assistant_id.clone(),
            parent_id: Some(reason_id),
            timestamp: ts(),
            kind: SessionEntryKind::Item(ConversationItem::text(
                Role::Assistant,
                format!("answer {slug}"),
            )),
        });
        assistant_id
    };

    let root_assistant_id =
        push_user_assistant_chain(&mut entries, &mut next_timestamp, "root", "config-root");

    entries.push(SessionEntry {
        id: "tr-root".to_string(),
        parent_id: Some(root_assistant_id),
        timestamp: next_timestamp(),
        kind: SessionEntryKind::TranscriptReplay(TranscriptReplayItem::Message {
            role: runtime_domain::session::TranscriptReplayRole::Assistant,
            content: "answer root".to_string(),
        }),
    });

    let mut config_chain_parent = "tr-root".to_string();
    for branch_index in 1..=4 {
        let config_id = format!("config-{branch_index}");
        entries.push(SessionEntry {
            id: config_id.clone(),
            parent_id: Some(config_chain_parent.clone()),
            timestamp: next_timestamp(),
            kind: SessionEntryKind::ConfigChange(config_snapshot()),
        });
        push_user_assistant_chain(
            &mut entries,
            &mut next_timestamp,
            &format!("branch-{branch_index}"),
            &config_id,
        );
        config_chain_parent = config_id;
    }

    entries
}

fn entries_with_trailing_leaf_override() -> Vec<SessionEntry> {
    let mut entries = branching_entries();
    entries.push(SessionEntry {
        id: "leaf-1".to_string(),
        parent_id: Some("assistant-c".to_string()),
        timestamp: 1_717_514_800_004,
        kind: SessionEntryKind::Leaf {
            target_id: Some("user-a".to_string()),
        },
    });
    entries
}

fn entries_with_compaction() -> Vec<SessionEntry> {
    let session_id: SessionId = "01914a5c-3c7e-7a2b-8abc-1234567890ab"
        .parse()
        .expect("fixture session id should parse");

    vec![
        SessionEntry {
            id: "header".to_string(),
            parent_id: None,
            timestamp: 1_717_514_800_000,
            kind: SessionEntryKind::Header(SessionHeader {
                session_id,
                work_dir: PathBuf::from("/repo"),
                session_name: None,
                initial_model: "gpt-4.1".to_string(),
                git_head: Some("abc123".to_string()),
                cli_version: Some("0.5.2".to_string()),
            }),
        },
        SessionEntry {
            id: "user-a".to_string(),
            parent_id: Some("header".to_string()),
            timestamp: 1_717_514_800_001,
            kind: SessionEntryKind::Item(ConversationItem::text(Role::User, "drop me")),
        },
        SessionEntry {
            id: "assistant-b".to_string(),
            parent_id: Some("user-a".to_string()),
            timestamp: 1_717_514_800_002,
            kind: SessionEntryKind::Item(ConversationItem::text(Role::Assistant, "drop me too")),
        },
        SessionEntry {
            id: "assistant-c".to_string(),
            parent_id: Some("assistant-b".to_string()),
            timestamp: 1_717_514_800_003,
            kind: SessionEntryKind::Item(ConversationItem::text(Role::Assistant, "keep me")),
        },
        SessionEntry {
            id: "compaction-1".to_string(),
            parent_id: Some("assistant-c".to_string()),
            timestamp: 1_717_514_800_004,
            kind: SessionEntryKind::Compaction {
                summary: "compacted summary".to_string(),
                first_kept_entry_id: "assistant-c".to_string(),
                tokens_before: 64,
            },
        },
        SessionEntry {
            id: "assistant-d".to_string(),
            parent_id: Some("compaction-1".to_string()),
            timestamp: 1_717_514_800_005,
            kind: SessionEntryKind::Item(ConversationItem::text(
                Role::Assistant,
                "after compaction",
            )),
        },
    ]
}

fn entries_with_multiple_compactions() -> Vec<SessionEntry> {
    let mut entries = entries_with_compaction();
    entries.push(SessionEntry {
        id: "assistant-e".to_string(),
        parent_id: Some("assistant-d".to_string()),
        timestamp: 1_717_514_800_006,
        kind: SessionEntryKind::Item(ConversationItem::text(Role::Assistant, "second keep")),
    });
    entries.push(SessionEntry {
        id: "compaction-2".to_string(),
        parent_id: Some("assistant-e".to_string()),
        timestamp: 1_717_514_800_007,
        kind: SessionEntryKind::Compaction {
            summary: "latest summary".to_string(),
            first_kept_entry_id: "assistant-e".to_string(),
            tokens_before: 96,
        },
    });
    entries.push(SessionEntry {
        id: "assistant-f".to_string(),
        parent_id: Some("compaction-2".to_string()),
        timestamp: 1_717_514_800_008,
        kind: SessionEntryKind::Item(ConversationItem::text(
            Role::Assistant,
            "after latest compaction",
        )),
    });
    entries
}

fn entries_with_trailing_leaf_reset() -> Vec<SessionEntry> {
    let mut entries = branching_entries();
    entries.push(SessionEntry {
        id: "leaf-reset".to_string(),
        parent_id: Some("assistant-c".to_string()),
        timestamp: 1_717_514_800_004,
        kind: SessionEntryKind::Leaf { target_id: None },
    });
    entries
}

fn header_only_entries() -> Vec<SessionEntry> {
    let session_id: SessionId = "01914a5c-3c7e-7a2b-8abc-1234567890ab"
        .parse()
        .expect("fixture session id should parse");

    vec![SessionEntry {
        id: "header".to_string(),
        parent_id: None,
        timestamp: 1_717_514_800_000,
        kind: SessionEntryKind::Header(SessionHeader {
            session_id,
            work_dir: PathBuf::from("/repo"),
            session_name: None,
            initial_model: "gpt-4.1".to_string(),
            git_head: Some("abc123".to_string()),
            cli_version: Some("0.5.2".to_string()),
        }),
    }]
}

fn entries_with_non_history_metadata() -> Vec<SessionEntry> {
    let mut entries = branching_entries();
    entries.truncate(2);
    entries.push(SessionEntry {
        id: "branch-summary".to_string(),
        parent_id: Some("user-a".to_string()),
        timestamp: 1_717_514_800_002,
        kind: SessionEntryKind::BranchSummary {
            from_id: "user-a".to_string(),
            summary: "alternate".to_string(),
        },
    });
    entries.push(SessionEntry {
        id: "config-change".to_string(),
        parent_id: Some("branch-summary".to_string()),
        timestamp: 1_717_514_800_003,
        kind: SessionEntryKind::ConfigChange(ConfigSnapshot {
            provider_id: "local".to_string(),
            model: "gpt-4.1-mini".to_string(),
            system_prompt: Some("be terse".to_string()),
        }),
    });
    entries.push(SessionEntry {
        id: "assistant-c".to_string(),
        parent_id: Some("config-change".to_string()),
        timestamp: 1_717_514_800_004,
        kind: SessionEntryKind::Item(ConversationItem::text(Role::Assistant, "final reply")),
    });
    entries
}

fn entries_with_dangling_parent() -> Vec<SessionEntry> {
    let mut entries = linear_history_entries();
    entries[2].parent_id = Some("missing".to_string());
    entries.truncate(3);
    entries
}

fn entries_with_cycle() -> Vec<SessionEntry> {
    let mut entries = branching_entries();
    entries[1].parent_id = Some("assistant-b".to_string());
    entries[2].parent_id = Some("user-a".to_string());
    entries.truncate(3);
    entries
}

fn entries_with_missing_leaf_target() -> Vec<SessionEntry> {
    let mut entries = branching_entries();
    entries.push(SessionEntry {
        id: "leaf-missing".to_string(),
        parent_id: Some("assistant-c".to_string()),
        timestamp: 1_717_514_800_004,
        kind: SessionEntryKind::Leaf {
            target_id: Some("missing-target".to_string()),
        },
    });
    entries
}

fn entries_with_invalid_compaction_target() -> Vec<SessionEntry> {
    let mut entries = entries_with_compaction();
    entries[4].kind = SessionEntryKind::Compaction {
        summary: "compacted summary".to_string(),
        first_kept_entry_id: "missing-target".to_string(),
        tokens_before: 64,
    };
    entries
}

fn entries_with_duplicate_id() -> Vec<SessionEntry> {
    let mut entries = linear_history_entries();
    entries.push(SessionEntry {
        id: "assistant-1".to_string(),
        parent_id: Some("user-2".to_string()),
        timestamp: 1_717_514_800_004,
        kind: SessionEntryKind::Item(ConversationItem::text(
            Role::Assistant,
            "shadowed duplicate",
        )),
    });
    entries
}

fn entries_with_non_item_compaction_target() -> Vec<SessionEntry> {
    let session_id: SessionId = "01914a5c-3c7e-7a2b-8abc-1234567890ab"
        .parse()
        .expect("fixture session id should parse");

    vec![
        SessionEntry {
            id: "header".to_string(),
            parent_id: None,
            timestamp: 1_717_514_800_000,
            kind: SessionEntryKind::Header(SessionHeader {
                session_id,
                work_dir: PathBuf::from("/repo"),
                session_name: None,
                initial_model: "gpt-4.1".to_string(),
                git_head: Some("abc123".to_string()),
                cli_version: Some("0.5.2".to_string()),
            }),
        },
        SessionEntry {
            id: "user-a".to_string(),
            parent_id: Some("header".to_string()),
            timestamp: 1_717_514_800_001,
            kind: SessionEntryKind::Item(ConversationItem::text(Role::User, "hello")),
        },
        SessionEntry {
            id: "config-1".to_string(),
            parent_id: Some("user-a".to_string()),
            timestamp: 1_717_514_800_002,
            kind: SessionEntryKind::ConfigChange(ConfigSnapshot {
                provider_id: "local".to_string(),
                model: "gpt-4.1-mini".to_string(),
                system_prompt: Some("be terse".to_string()),
            }),
        },
        SessionEntry {
            id: "compaction-1".to_string(),
            parent_id: Some("config-1".to_string()),
            timestamp: 1_717_514_800_003,
            kind: SessionEntryKind::Compaction {
                summary: "summary".to_string(),
                first_kept_entry_id: "config-1".to_string(),
                tokens_before: 32,
            },
        },
        SessionEntry {
            id: "assistant-d".to_string(),
            parent_id: Some("compaction-1".to_string()),
            timestamp: 1_717_514_800_004,
            kind: SessionEntryKind::Item(ConversationItem::text(
                Role::Assistant,
                "after compaction",
            )),
        },
    ]
}

fn long_linear_history_entries(item_count: usize) -> Vec<SessionEntry> {
    let session_id: SessionId = "01914a5c-3c7e-7a2b-8abc-1234567890ab"
        .parse()
        .expect("fixture session id should parse");
    let mut entries = Vec::with_capacity(item_count + 1);
    entries.push(SessionEntry {
        id: "header".to_string(),
        parent_id: None,
        timestamp: 1_717_514_800_000,
        kind: SessionEntryKind::Header(SessionHeader {
            session_id,
            work_dir: PathBuf::from("/repo"),
            session_name: None,
            initial_model: "gpt-4.1".to_string(),
            git_head: Some("abc123".to_string()),
            cli_version: Some("0.5.2".to_string()),
        }),
    });

    let mut parent_id = "header".to_string();
    for index in 0..item_count {
        let id = format!("assistant-{index}");
        entries.push(SessionEntry {
            id: id.clone(),
            parent_id: Some(parent_id),
            timestamp: 1_717_514_800_001 + index as i64,
            kind: SessionEntryKind::Item(ConversationItem::text(
                Role::Assistant,
                format!("message-{index}"),
            )),
        });
        parent_id = id;
    }

    entries
}
