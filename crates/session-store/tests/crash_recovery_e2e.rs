use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

#[path = "support/common.rs"]
mod common;
#[path = "support/crash_recovery.rs"]
mod crash_recovery_support;

use common::{TestSessionRoot, first_item_entry_id, open_store, sample_header};
use crash_recovery_support::{
    PermissionGuard, read_session_entries, remove_index_files, truncate_last_line,
    write_session_fixture,
};
use provider_protocol::{ContentBlock, ConversationItem, Role, ToolCall};
use session_store::{
    ConfigSnapshot, ProjectDir, SessionEntry, SessionEntryKind, SessionHeader, SessionId,
    SessionListOptions, SessionMeta, SessionStore,
};

#[tokio::test]
async fn local_store_restores_complex_history_after_restart() {
    let root = TestSessionRoot::new("restart");
    let work_dir = root.workspace_path("repo");
    let tool_call = ToolCall::new("call-1", "bash", r#"{"cmd":"pwd"}"#);
    let user_item = ConversationItem::text(Role::User, "inspect workspace");
    let assistant_item =
        ConversationItem::assistant_with_tool_calls("running".to_string(), vec![tool_call.clone()]);
    let tool_result = ConversationItem::tool_result(
        tool_call.call_id.clone(),
        vec![ContentBlock::Text("/tmp/repo".to_string())],
        false,
    );

    let (session_id, before_restart) = {
        let store = open_store(&root).await;
        let session_id = store
            .create_session(sample_header(&work_dir, "gpt-4.1", Some("restartable")))
            .await
            .expect("session should be created");
        store
            .append(&session_id, user_item.clone())
            .await
            .expect("user item should append");
        store
            .append(&session_id, assistant_item.clone())
            .await
            .expect("assistant item should append");
        store
            .append(&session_id, tool_result.clone())
            .await
            .expect("tool result should append");

        let resolved = store
            .resolve(&session_id, None)
            .await
            .expect("history should resolve before restart");
        (session_id, resolved)
    };

    let reopened_store = open_store(&root).await;
    let after_restart = reopened_store
        .resolve(&session_id, None)
        .await
        .expect("history should resolve after restart");

    assert_eq!(before_restart, vec![user_item, assistant_item, tool_result]);
    assert_eq!(after_restart, before_restart);
}

#[tokio::test]
async fn local_store_skips_a_truncated_jsonl_tail_on_restart() {
    let root = TestSessionRoot::new("truncate-tail");
    let work_dir = root.workspace_path("repo");

    let (session_id, jsonl_path, expected_history) = {
        let store = open_store(&root).await;
        let session_id = store
            .create_session(sample_header(&work_dir, "gpt-4.1", Some("truncated")))
            .await
            .expect("session should be created");
        let items = vec![
            ConversationItem::text(Role::User, "message-1"),
            ConversationItem::text(Role::Assistant, "message-2"),
            ConversationItem::text(Role::User, "message-3"),
            ConversationItem::text(Role::Assistant, "message-4"),
            ConversationItem::text(Role::User, "message-5"),
        ];

        for item in &items {
            store
                .append(&session_id, item.clone())
                .await
                .expect("fixture item should append");
        }
        store
            .flush(&session_id)
            .await
            .expect("flush should persist the complete session");

        let meta = store
            .get_session_meta(&session_id)
            .await
            .expect("session meta should load");
        (session_id, meta.jsonl_path, items[..4].to_vec())
    };

    truncate_last_line(&jsonl_path, 10);

    let reopened_store = open_store(&root).await;
    let resolved = reopened_store
        .resolve(&session_id, None)
        .await
        .expect("restart should ignore the truncated tail");

    assert_eq!(resolved, expected_history);
}

#[tokio::test]
async fn local_store_rebuilds_sqlite_index_from_jsonl_after_index_deletion() {
    let root = TestSessionRoot::new("rebuild-index");
    let work_dir = root.workspace_path("repo");

    let before_restart = {
        let store = open_store(&root).await;
        let first_id = store
            .create_session(sample_header(&work_dir, "gpt-4.1", Some("first")))
            .await
            .expect("first session should be created");
        store
            .append(
                &first_id,
                ConversationItem::text(Role::User, "first preview"),
            )
            .await
            .expect("first preview should append");

        let second_id = store
            .create_session(sample_header(&work_dir, "gpt-4.1", Some("second")))
            .await
            .expect("second session should be created");
        store
            .append(
                &second_id,
                ConversationItem::text(Role::User, "second preview"),
            )
            .await
            .expect("second preview should append");
        store
            .append_config_change(
                &second_id,
                ConfigSnapshot {
                    provider_id: "local".to_string(),
                    model: "gpt-4.1-mini".to_string(),
                    system_prompt: Some("brief".to_string()),
                },
            )
            .await
            .expect("config change should append");

        let third_id = store
            .create_session(sample_header(&work_dir, "gpt-4.1", Some("third")))
            .await
            .expect("third session should be created");
        store
            .append(
                &third_id,
                ConversationItem::text(Role::User, "third preview"),
            )
            .await
            .expect("third preview should append");

        session_meta_map(
            store
                .list_sessions(
                    &ProjectDir::from_work_dir(&work_dir),
                    SessionListOptions::default(),
                )
                .await
                .expect("sessions should list before index deletion"),
        )
    };

    remove_index_files(&root);

    let reopened_store = open_store(&root).await;
    let after_restart = session_meta_map(
        reopened_store
            .list_sessions(
                &ProjectDir::from_work_dir(&work_dir),
                SessionListOptions::default(),
            )
            .await
            .expect("sessions should list after backfill"),
    );

    assert_eq!(after_restart.len(), 3);
    assert_eq!(after_restart, before_restart);
}

#[tokio::test]
async fn local_store_flushes_pending_entries_after_recovering_from_io_error() {
    let root = TestSessionRoot::new("io-recovery");
    let work_dir = root.workspace_path("repo");

    let (session_id, jsonl_path) = {
        let store = open_store(&root).await;
        let session_id = store
            .create_session(sample_header(&work_dir, "gpt-4.1", Some("io-recovery")))
            .await
            .expect("session should be created");
        store
            .append(&session_id, ConversationItem::text(Role::User, "message-1"))
            .await
            .expect("first item should append");
        store
            .append(
                &session_id,
                ConversationItem::text(Role::Assistant, "message-2"),
            )
            .await
            .expect("second item should append");
        store
            .flush(&session_id)
            .await
            .expect("flush should sync initial items");
        let meta = store
            .get_session_meta(&session_id)
            .await
            .expect("meta should load");
        (session_id, meta.jsonl_path)
    };

    let readonly_guard = PermissionGuard::make_readonly(&jsonl_path);

    let reopened_store = open_store(&root).await;
    let append_error = reopened_store
        .append(&session_id, ConversationItem::text(Role::User, "message-3"))
        .await
        .expect_err("append should fail while the jsonl file is read-only");
    assert!(
        append_error
            .to_string()
            .contains("failed to access session storage"),
        "expected a storage I/O error, got: {append_error}"
    );

    readonly_guard.restore();
    reopened_store
        .flush(&session_id)
        .await
        .expect("flush should retry the pending entry after permissions recover");

    let same_process_resolved = reopened_store
        .resolve(&session_id, None)
        .await
        .expect("history should resolve in the same process after recovered flush");
    assert_eq!(
        same_process_resolved,
        vec![
            ConversationItem::text(Role::User, "message-1"),
            ConversationItem::text(Role::Assistant, "message-2"),
            ConversationItem::text(Role::User, "message-3"),
        ],
        "recovered flush must also commit the pending entry to in-memory state"
    );

    let final_store = open_store(&root).await;
    let resolved = final_store
        .resolve(&session_id, None)
        .await
        .expect("history should resolve after recovered flush");

    assert_eq!(
        resolved,
        vec![
            ConversationItem::text(Role::User, "message-1"),
            ConversationItem::text(Role::Assistant, "message-2"),
            ConversationItem::text(Role::User, "message-3"),
        ]
    );
}

#[tokio::test]
async fn local_store_persists_leaf_overrides_across_restart() {
    let root = TestSessionRoot::new("branch-restart");
    let work_dir = root.workspace_path("repo");

    let (session_id, item_a_id, jsonl_path) = {
        let store = open_store(&root).await;
        let session_id = store
            .create_session(sample_header(&work_dir, "gpt-4.1", Some("branching")))
            .await
            .expect("session should be created");
        store
            .append(&session_id, ConversationItem::text(Role::User, "A"))
            .await
            .expect("A should append");
        store
            .append(&session_id, ConversationItem::text(Role::Assistant, "B"))
            .await
            .expect("B should append");
        store
            .append(&session_id, ConversationItem::text(Role::Assistant, "C"))
            .await
            .expect("C should append");

        let meta = store
            .get_session_meta(&session_id)
            .await
            .expect("meta should load");
        let item_a_id =
            first_item_entry_id(&meta.jsonl_path).expect("session file should contain item ids");

        store
            .set_leaf(&session_id, Some(&item_a_id))
            .await
            .expect("leaf override should persist");
        store
            .append(&session_id, ConversationItem::text(Role::Assistant, "D"))
            .await
            .expect("branched append should succeed");
        store
            .flush(&session_id)
            .await
            .expect("branched session should flush");

        (session_id, item_a_id, meta.jsonl_path)
    };

    let reopened_store = open_store(&root).await;
    let resolved = reopened_store
        .resolve(&session_id, None)
        .await
        .expect("branched history should resolve after restart");
    let entries = read_session_entries(&jsonl_path);

    assert_eq!(
        resolved,
        vec![
            ConversationItem::text(Role::User, "A"),
            ConversationItem::text(Role::Assistant, "D"),
        ]
    );
    assert!(
        entries.iter().any(|entry| {
            matches!(
                &entry.kind,
                SessionEntryKind::Leaf {
                    target_id: Some(target_id)
                } if target_id == &item_a_id
            )
        }),
        "jsonl should persist the leaf override entry"
    );
}

#[tokio::test]
async fn local_store_resolves_compacted_history_after_restart() {
    let root = TestSessionRoot::new("compaction-restart");
    let work_dir = root.workspace_path("repo");
    let session_id = SessionId::new();
    let entries = compacted_fixture_entries(&work_dir, &session_id);

    write_session_fixture(&root, &work_dir, &session_id, &entries);

    let store = open_store(&root).await;
    let resolved = store
        .resolve(&session_id, None)
        .await
        .expect("compacted history should resolve after restart");

    assert_eq!(
        resolved,
        vec![
            ConversationItem::system(vec![ContentBlock::Text(
                "summary of messages 1-7".to_string(),
            )]),
            ConversationItem::text(Role::Assistant, "message-8"),
            ConversationItem::text(Role::Assistant, "message-9"),
            ConversationItem::text(Role::Assistant, "message-10"),
        ]
    );
}

#[tokio::test]
async fn local_store_keeps_concurrent_session_writes_isolated() {
    let root = TestSessionRoot::new("concurrent-writes");
    let work_dir = root.workspace_path("repo");
    let store = std::sync::Arc::new(open_store(&root).await);
    let first_session = store
        .create_session(sample_header(&work_dir, "gpt-4.1", Some("first")))
        .await
        .expect("first session should be created");
    let second_session = store
        .create_session(sample_header(&work_dir, "gpt-4.1", Some("second")))
        .await
        .expect("second session should be created");

    let first_task = {
        let store = store.clone();
        let session_id = first_session.clone();
        tokio::spawn(async move {
            let expected = (0..32)
                .map(|index| ConversationItem::text(Role::User, format!("first-{index}")))
                .collect::<Vec<_>>();
            for item in &expected {
                store
                    .append(&session_id, item.clone())
                    .await
                    .expect("first session append should succeed");
            }
            expected
        })
    };
    let second_task = {
        let store = store.clone();
        let session_id = second_session.clone();
        tokio::spawn(async move {
            let expected = (0..32)
                .map(|index| ConversationItem::text(Role::Assistant, format!("second-{index}")))
                .collect::<Vec<_>>();
            for item in &expected {
                store
                    .append(&session_id, item.clone())
                    .await
                    .expect("second session append should succeed");
            }
            expected
        })
    };

    let first_expected = first_task.await.expect("first append task should complete");
    let second_expected = second_task
        .await
        .expect("second append task should complete");
    store
        .flush(&first_session)
        .await
        .expect("first session should flush");
    store
        .flush(&second_session)
        .await
        .expect("second session should flush");

    let first_resolved = store
        .resolve(&first_session, None)
        .await
        .expect("first history should resolve");
    let second_resolved = store
        .resolve(&second_session, None)
        .await
        .expect("second history should resolve");

    assert_eq!(first_resolved, first_expected);
    assert_eq!(second_resolved, second_expected);
}

#[tokio::test]
#[ignore = "machine-sensitive performance validation; run explicitly when profiling session-store"]
async fn local_store_meets_phase2_recovery_performance_targets() {
    let resolve_root = TestSessionRoot::new("perf-resolve");
    let resolve_work_dir = resolve_root.workspace_path("repo");
    let resolve_session_id = SessionId::new();
    let resolve_entries = linear_session_entries(
        &resolve_work_dir,
        &resolve_session_id,
        "resolve-benchmark",
        1_000,
    );

    write_session_fixture(
        &resolve_root,
        &resolve_work_dir,
        &resolve_session_id,
        &resolve_entries,
    );

    let resolve_store = open_store(&resolve_root).await;
    resolve_store
        .resolve(&resolve_session_id, None)
        .await
        .expect("warm-up resolve should succeed");
    let mut resolve_samples = Vec::with_capacity(7);
    let mut resolved = Vec::new();
    for _ in 0..7 {
        let resolve_started = Instant::now();
        let current_resolved = resolve_store
            .resolve(&resolve_session_id, None)
            .await
            .expect("benchmark session should resolve");
        resolve_samples.push(resolve_started.elapsed());
        resolved = current_resolved;
    }

    assert_eq!(resolved.len(), 1_000);
    assert!(
        median_duration(&mut resolve_samples) < Duration::from_millis(10),
        "median resolve(1000 items) exceeded 10ms"
    );

    let list_root = TestSessionRoot::new("perf-list-backfill");
    let list_work_dir = list_root.workspace_path("repo");
    for index in 0..100 {
        let session_id = SessionId::new();
        let entries = linear_session_entries(
            &list_work_dir,
            &session_id,
            &format!("session-{index:03}"),
            500,
        );
        write_session_fixture(&list_root, &list_work_dir, &session_id, &entries);
    }

    let backfill_started = Instant::now();
    let list_store = open_store(&list_root).await;
    let backfill_elapsed = backfill_started.elapsed();
    let mut listed = Vec::new();
    let mut list_samples = Vec::with_capacity(7);
    for _ in 0..7 {
        let list_started = Instant::now();
        listed = list_store
            .list_sessions(
                &ProjectDir::from_work_dir(&list_work_dir),
                SessionListOptions::default(),
            )
            .await
            .expect("benchmark sessions should list");
        list_samples.push(list_started.elapsed());
    }

    assert_eq!(listed.len(), 100);
    assert!(
        median_duration(&mut list_samples) < Duration::from_millis(50),
        "median list_sessions(100 sessions) exceeded 50ms"
    );
    assert!(
        backfill_elapsed < Duration::from_secs(5),
        "backfill(100x500 items) took {:?}, expected under 5s",
        backfill_elapsed
    );
}

fn session_meta_map(metas: Vec<SessionMeta>) -> BTreeMap<String, SessionMetaSummary> {
    metas
        .into_iter()
        .map(|meta| {
            (
                meta.session_id.to_string(),
                SessionMetaSummary {
                    title: meta.title,
                    preview: meta.preview,
                    model: meta.model,
                    created_at: meta.created_at,
                    updated_at: meta.updated_at,
                    work_dir: meta.project_dir.as_path().to_path_buf(),
                    jsonl_path: meta.jsonl_path,
                },
            )
        })
        .collect()
}

fn compacted_fixture_entries(work_dir: &Path, session_id: &SessionId) -> Vec<SessionEntry> {
    let mut entries = vec![SessionEntry {
        id: "header".to_string(),
        parent_id: None,
        timestamp: 1_717_514_800_000,
        kind: SessionEntryKind::Header(SessionHeader {
            session_id: session_id.clone(),
            work_dir: work_dir.to_path_buf(),
            session_name: Some("compacted".to_string()),
            initial_model: "gpt-4.1".to_string(),
            git_head: Some("abc123".to_string()),
            cli_version: Some("0.5.8".to_string()),
        }),
    }];

    let mut parent_id = "header".to_string();
    for index in 1..=10 {
        let item_id = format!("item-{index}");
        entries.push(SessionEntry {
            id: item_id.clone(),
            parent_id: Some(parent_id.clone()),
            timestamp: 1_717_514_800_000 + i64::from(index),
            kind: SessionEntryKind::Item(ConversationItem::text(
                Role::Assistant,
                format!("message-{index}"),
            )),
        });
        parent_id = item_id;
    }
    entries.push(SessionEntry {
        id: "compaction-1".to_string(),
        parent_id: Some(parent_id),
        timestamp: 1_717_514_800_100,
        kind: SessionEntryKind::Compaction {
            summary: "summary of messages 1-7".to_string(),
            first_kept_entry_id: "item-8".to_string(),
            tokens_before: 700,
        },
    });

    entries
}

fn linear_session_entries(
    work_dir: &Path,
    session_id: &SessionId,
    session_name: &str,
    item_count: usize,
) -> Vec<SessionEntry> {
    let mut entries = vec![SessionEntry {
        id: "header".to_string(),
        parent_id: None,
        timestamp: 1_717_514_800_000,
        kind: SessionEntryKind::Header(SessionHeader {
            session_id: session_id.clone(),
            work_dir: work_dir.to_path_buf(),
            session_name: Some(session_name.to_string()),
            initial_model: "gpt-4.1".to_string(),
            git_head: Some("abc123".to_string()),
            cli_version: Some("0.5.8".to_string()),
        }),
    }];

    let mut parent_id = "header".to_string();
    for index in 0..item_count {
        let item_id = format!("item-{index:04}");
        entries.push(SessionEntry {
            id: item_id.clone(),
            parent_id: Some(parent_id.clone()),
            timestamp: 1_717_514_800_001 + index as i64,
            kind: SessionEntryKind::Item(ConversationItem::text(
                Role::Assistant,
                format!("message-{index}"),
            )),
        });
        parent_id = item_id;
    }

    entries
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct SessionMetaSummary {
    title: String,
    preview: Option<String>,
    model: Option<String>,
    created_at: i64,
    updated_at: i64,
    work_dir: PathBuf,
    jsonl_path: PathBuf,
}

fn median_duration(samples: &mut [Duration]) -> Duration {
    samples.sort_unstable();
    samples[samples.len() / 2]
}
