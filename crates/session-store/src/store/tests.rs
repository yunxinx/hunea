#[cfg(test)]
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use provider_protocol::{ConversationItem, Role};
use tokio::{
    sync::RwLock,
    time::{Duration, timeout},
};

use crate::{SessionEntry, SessionEntryKind, SessionHeader, SessionId, metadata::MetadataIndex};

use super::{
    SessionStore,
    local::{LocalSessionHandle, LocalSessionStore, MAX_OPEN_SESSION_HANDLES, session_jsonl_path},
};

#[tokio::test]
async fn local_session_read_paths_do_not_wait_for_write_operation_lock() {
    let root = temp_test_dir("read-with-pending-write-lock");
    let work_dir = root.join("workspace");
    fs::create_dir_all(&work_dir).expect("workspace should be created");
    let session_id = SessionId::new();
    let jsonl_path = session_jsonl_path(&root, &work_dir, &session_id);
    let entries = vec![
        SessionEntry {
            id: "header".to_string(),
            parent_id: None,
            timestamp: 1,
            kind: SessionEntryKind::Header(SessionHeader {
                session_id: session_id.clone(),
                work_dir,
                session_name: Some("locked-read".to_string()),
                initial_model: "qwen3".to_string(),
                git_head: None,
                cli_version: None,
            }),
        },
        SessionEntry {
            id: "user-1".to_string(),
            parent_id: Some("header".to_string()),
            timestamp: 2,
            kind: SessionEntryKind::Item(ConversationItem::text(Role::User, "hello")),
        },
    ];
    let handle = Arc::new(
        LocalSessionHandle::new(jsonl_path, entries).expect("session handle should initialize"),
    );
    let _write_guard = handle.operation_lock.lock().await;
    let store = LocalSessionStore {
        hunea_dir: root.clone(),
        recorders: RwLock::new(HashMap::from([(session_id.clone(), handle.clone())])),
        index: MetadataIndex::open(&root.join("index.sqlite"))
            .await
            .expect("index should open"),
    };

    let resolved = timeout(Duration::from_millis(50), store.resolve(&session_id, None))
        .await
        .expect("read path should not wait for the write operation lock")
        .expect("session should resolve");

    assert_eq!(resolved, vec![ConversationItem::text(Role::User, "hello")]);
    let _ = fs::remove_dir_all(root);
}

#[tokio::test]
async fn local_store_evicts_idle_recorders_after_open_limit() {
    let root = temp_test_dir("recorder-eviction");
    let work_dir = root.join("workspace");
    fs::create_dir_all(&work_dir).expect("workspace should be created");
    let store = LocalSessionStore::open_in(root.clone())
        .await
        .expect("store should open");

    for index in 0..(MAX_OPEN_SESSION_HANDLES + 3) {
        store
            .create_session(SessionHeader {
                session_id: SessionId::new(),
                work_dir: work_dir.clone(),
                session_name: Some(format!("session-{index}")),
                initial_model: "qwen3".to_string(),
                git_head: None,
                cli_version: None,
            })
            .await
            .expect("session should be created");
    }

    let open_recorders = store.recorders.read().await.len();
    assert!(
        open_recorders <= MAX_OPEN_SESSION_HANDLES,
        "idle recorder cache should stay bounded, found {open_recorders}"
    );
    let _ = fs::remove_dir_all(root);
}

#[tokio::test]
async fn local_store_recovers_failed_append_before_accepting_next_append() {
    let root = temp_test_dir("failed-append-recovery-order");
    fs::create_dir_all(&root).expect("root should be created");
    let work_dir = root.join("workspace");
    fs::create_dir_all(&work_dir).expect("workspace should be created");
    let session_id = SessionId::new();
    let jsonl_path = root.join("session.jsonl");
    let seed_entries = vec![
        SessionEntry {
            id: "header".to_string(),
            parent_id: None,
            timestamp: 1,
            kind: SessionEntryKind::Header(SessionHeader {
                session_id: session_id.clone(),
                work_dir,
                session_name: Some("append recovery".to_string()),
                initial_model: "qwen3".to_string(),
                git_head: None,
                cli_version: None,
            }),
        },
        SessionEntry {
            id: "user-1".to_string(),
            parent_id: Some("header".to_string()),
            timestamp: 2,
            kind: SessionEntryKind::Item(ConversationItem::text(Role::User, "seed")),
        },
    ];
    write_entries(&jsonl_path, &seed_entries);
    let handle = Arc::new(
        LocalSessionHandle::new(jsonl_path.clone(), seed_entries.clone())
            .expect("session handle should initialize"),
    );
    let store = LocalSessionStore {
        hunea_dir: root.clone(),
        recorders: RwLock::new(HashMap::from([(session_id.clone(), handle)])),
        index: MetadataIndex::open(&root.join("index.sqlite"))
            .await
            .expect("index should open"),
    };
    fs::remove_file(&jsonl_path).expect("seed file should be removable");
    fs::create_dir(&jsonl_path).expect("directory should block jsonl append");

    store
        .append(
            &session_id,
            ConversationItem::text(Role::User, "failed but buffered"),
        )
        .await
        .expect_err("blocked path should fail append");

    fs::remove_dir(&jsonl_path).expect("blocking directory should be removable");
    write_entries(&jsonl_path, &seed_entries);
    store
        .append(
            &session_id,
            ConversationItem::text(Role::Assistant, "after recovery"),
        )
        .await
        .expect("next append should first recover the pending entry");

    let resolved = store
        .resolve(&session_id, None)
        .await
        .expect("session should resolve after recovery");
    assert_eq!(
        resolved,
        vec![
            ConversationItem::text(Role::User, "seed"),
            ConversationItem::text(Role::User, "failed but buffered"),
            ConversationItem::text(Role::Assistant, "after recovery"),
        ]
    );
    let _ = fs::remove_dir_all(root);
}

fn temp_test_dir(prefix: &str) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time should be after unix epoch")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "hunea-session-store-{prefix}-{}-{stamp}",
        std::process::id()
    ))
}

fn write_entries(path: &Path, entries: &[SessionEntry]) {
    let mut contents = String::new();
    for entry in entries {
        contents.push_str(&serde_json::to_string(entry).expect("entry should serialize"));
        contents.push('\n');
    }
    fs::write(path, contents).expect("entries should be writable");
}
