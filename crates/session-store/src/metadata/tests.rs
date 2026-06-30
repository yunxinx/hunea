use std::{
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

use provider_protocol::{ConversationItem, Role};
use rusqlite::Connection;
use uuid::Uuid;

use crate::{
    ProjectDir, SessionEntry, SessionEntryKind, SessionHeader, SessionId, SessionListOptions,
    SessionMeta, jsonl::JsonlWriter, metadata::MetadataIndex, session_filename,
};

use super::{initialize_database, repair};
use repair::{
    DiscoveredSessionFile, IndexedProjectFile, SessionFileFingerprint, build_repair_plan,
};

#[tokio::test]
async fn metadata_index_roundtrips_one_session() {
    let root = tempdir_path("metadata-index-roundtrip");
    fs::create_dir_all(&root).expect("temp root should be creatable");
    let index = MetadataIndex::open(&root.join("index.sqlite"))
        .await
        .expect("metadata index should open sqlite file");
    let meta = sample_session_meta();

    index
        .upsert_session(&meta)
        .await
        .expect("upsert should persist session metadata");

    let loaded = index
        .get_session_meta(&meta.session_id.to_string())
        .await
        .expect("session metadata should be queryable by id");

    assert_eq!(loaded, meta);

    fs::remove_dir_all(root).expect("temp root should be removable");
}

#[tokio::test]
async fn list_sessions_orders_results_by_updated_at_descending() {
    let root = tempdir_path("metadata-index-order");
    let sessions_dir = root.join("sessions");
    let work_dir = root.join("workspace").join("repo");
    fs::create_dir_all(&work_dir).expect("work dir should be creatable");
    let index = MetadataIndex::open(&root.join("index.sqlite"))
        .await
        .expect("metadata index should open sqlite file");
    let earliest_id: SessionId = "01914a5c-3c7e-7a2b-8abc-1234567890ab"
        .parse()
        .expect("fixture session id should parse");
    let middle_id: SessionId = "01914a5c-3c7e-7a2b-8abc-1234567890ac"
        .parse()
        .expect("fixture session id should parse");
    let latest_id: SessionId = "01914a5c-3c7e-7a2b-8abc-1234567890ad"
        .parse()
        .expect("fixture session id should parse");
    write_session_jsonl(
        &sessions_dir,
        &work_dir,
        &earliest_id,
        session_fixture_entries(&work_dir, &earliest_id, 10, "first"),
    );
    write_session_jsonl(
        &sessions_dir,
        &work_dir,
        &middle_id,
        session_fixture_entries(&work_dir, &middle_id, 20, "second"),
    );
    write_session_jsonl(
        &sessions_dir,
        &work_dir,
        &latest_id,
        session_fixture_entries(&work_dir, &latest_id, 30, "third"),
    );
    index
        .backfill_from_jsonl(&sessions_dir)
        .await
        .expect("backfill should load ordered fixtures");

    let listed = index
        .list_sessions(
            &ProjectDir::from_work_dir(&work_dir),
            SessionListOptions::default(),
        )
        .await
        .expect("session list should be queryable");

    assert_eq!(
        listed
            .into_iter()
            .map(|meta| meta.session_id.to_string())
            .collect::<Vec<_>>(),
        vec![
            latest_id.to_string(),
            middle_id.to_string(),
            earliest_id.to_string(),
        ]
    );

    fs::remove_dir_all(root).expect("temp root should be removable");
}

#[tokio::test]
async fn list_sessions_filters_by_project_dir() {
    let root = tempdir_path("metadata-index-project-filter");
    let sessions_dir = root.join("sessions");
    let repo_a = root.join("workspace").join("repo-a");
    let repo_b = root.join("workspace").join("repo-b");
    fs::create_dir_all(&repo_a).expect("repo A should be creatable");
    fs::create_dir_all(&repo_b).expect("repo B should be creatable");
    let index = MetadataIndex::open(&root.join("index.sqlite"))
        .await
        .expect("metadata index should open sqlite file");
    let repo_a_id: SessionId = "01914a5c-3c7e-7a2b-8abc-1234567890ab"
        .parse()
        .expect("fixture session id should parse");
    let repo_b_id: SessionId = "01914a5c-3c7e-7a2b-8abc-1234567890ac"
        .parse()
        .expect("fixture session id should parse");
    write_session_jsonl(
        &sessions_dir,
        &repo_a,
        &repo_a_id,
        session_fixture_entries(&repo_a, &repo_a_id, 10, "repo-a"),
    );
    write_session_jsonl(
        &sessions_dir,
        &repo_b,
        &repo_b_id,
        session_fixture_entries(&repo_b, &repo_b_id, 20, "repo-b"),
    );
    index
        .backfill_from_jsonl(&sessions_dir)
        .await
        .expect("backfill should load both projects");

    let listed = index
        .list_sessions(
            &ProjectDir::from_work_dir(&repo_a),
            SessionListOptions::default(),
        )
        .await
        .expect("session list should be filtered by project");

    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].session_id, repo_a_id);

    fs::remove_dir_all(root).expect("temp root should be removable");
}

#[tokio::test]
async fn backfill_from_jsonl_derives_session_metadata() {
    let root = tempdir_path("metadata-index-backfill");
    let sessions_dir = root.join("sessions");
    let work_dir = root.join("workspace").join("repo");
    fs::create_dir_all(&work_dir).expect("work dir should be creatable");
    let session_id: SessionId = "01914a5c-3c7e-7a2b-8abc-1234567890ab"
        .parse()
        .expect("fixture session id should parse");
    let jsonl_path = write_session_jsonl(
        &sessions_dir,
        &work_dir,
        &session_id,
        vec![
            SessionEntry {
                id: "header".to_string(),
                parent_id: None,
                timestamp: 1_717_514_800_000,
                kind: SessionEntryKind::Header(SessionHeader {
                    session_id: session_id.clone(),
                    work_dir: work_dir.clone(),
                    session_name: None,
                    initial_model: "gpt-4.1".to_string(),
                    git_head: Some("abc123".to_string()),
                    cli_version: Some("0.5.5".to_string()),
                }),
            },
            SessionEntry {
                id: "user-1".to_string(),
                parent_id: Some("header".to_string()),
                timestamp: 1_717_514_800_050,
                kind: SessionEntryKind::Item(ConversationItem::text(
                    Role::User,
                    "Please inspect src/main.rs and explain startup wiring in detail.",
                )),
            },
            SessionEntry {
                id: "assistant-1".to_string(),
                parent_id: Some("user-1".to_string()),
                timestamp: 1_717_514_800_075,
                kind: SessionEntryKind::Item(ConversationItem::text(
                    Role::Assistant,
                    "The startup path is straightforward.",
                )),
            },
            SessionEntry {
                id: "config-1".to_string(),
                parent_id: Some("assistant-1".to_string()),
                timestamp: 1_717_514_800_090,
                kind: SessionEntryKind::ConfigChange(crate::ConfigSnapshot {
                    provider_id: "local".to_string(),
                    model: "gpt-4.1-mini".to_string(),
                    system_prompt: None,
                    prompt_prelude: None,
                }),
            },
            SessionEntry {
                id: "user-2".to_string(),
                parent_id: Some("config-1".to_string()),
                timestamp: 1_717_514_800_100,
                kind: SessionEntryKind::Item(ConversationItem::text(
                    Role::User,
                    "Add persistence hooks after that.",
                )),
            },
        ],
    );
    let index = MetadataIndex::open(&root.join("index.sqlite"))
        .await
        .expect("metadata index should open sqlite file");

    let processed = index
        .backfill_from_jsonl(&sessions_dir)
        .await
        .expect("backfill should parse session jsonl");
    let loaded = index
        .get_session_meta(&session_id.to_string())
        .await
        .expect("backfilled metadata should be queryable");

    assert_eq!(processed, 1);
    assert_eq!(loaded.session_id, session_id);
    assert_eq!(loaded.project_dir, ProjectDir::from_work_dir(&work_dir));
    assert_eq!(
        loaded.title,
        "Please inspect src/main.rs and explain startup wir"
    );
    assert_eq!(
        loaded.preview.as_deref(),
        Some("Add persistence hooks after that.")
    );
    assert_eq!(
        loaded.first_user_preview.as_deref(),
        Some("Please inspect src/main.rs and explain startup wiring in detail.")
    );
    assert_eq!(
        loaded.last_assistant_preview.as_deref(),
        Some("The startup path is straightforward.")
    );
    assert_eq!(loaded.model.as_deref(), Some("gpt-4.1-mini"));
    assert_eq!(loaded.created_at, 1_717_514_800_000);
    assert_eq!(loaded.updated_at, 1_717_514_800_100);
    assert_eq!(loaded.git_head.as_deref(), Some("abc123"));
    assert_eq!(loaded.project_dir.as_path(), work_dir.as_path());
    assert_eq!(loaded.jsonl_path, jsonl_path);

    fs::remove_dir_all(root).expect("temp root should be removable");
}

#[tokio::test]
async fn backfill_keeps_session_message_previews_to_256_chars() {
    let root = tempdir_path("metadata-index-long-previews");
    let sessions_dir = root.join("sessions");
    let work_dir = root.join("workspace").join("repo");
    fs::create_dir_all(&work_dir).expect("work dir should be creatable");
    let session_id: SessionId = "01914a5c-3c7e-7a2b-8abc-1234567890ab"
        .parse()
        .expect("fixture session id should parse");
    let long_user_message = "u".repeat(320);
    let long_assistant_message = "a".repeat(320);
    write_session_jsonl(
        &sessions_dir,
        &work_dir,
        &session_id,
        vec![
            SessionEntry {
                id: "header".to_string(),
                parent_id: None,
                timestamp: 1_717_514_800_000,
                kind: SessionEntryKind::Header(SessionHeader {
                    session_id: session_id.clone(),
                    work_dir: work_dir.clone(),
                    session_name: None,
                    initial_model: "gpt-4.1".to_string(),
                    git_head: None,
                    cli_version: None,
                }),
            },
            SessionEntry {
                id: "user-1".to_string(),
                parent_id: Some("header".to_string()),
                timestamp: 1_717_514_800_050,
                kind: SessionEntryKind::Item(ConversationItem::text(Role::User, long_user_message)),
            },
            SessionEntry {
                id: "assistant-1".to_string(),
                parent_id: Some("user-1".to_string()),
                timestamp: 1_717_514_800_075,
                kind: SessionEntryKind::Item(ConversationItem::text(
                    Role::Assistant,
                    long_assistant_message,
                )),
            },
        ],
    );
    let index = MetadataIndex::open(&root.join("index.sqlite"))
        .await
        .expect("metadata index should open sqlite file");

    index
        .backfill_from_jsonl(&sessions_dir)
        .await
        .expect("backfill should parse session jsonl");
    let loaded = index
        .get_session_meta(&session_id.to_string())
        .await
        .expect("backfilled metadata should be queryable");

    let expected_user_preview = "u".repeat(256);
    let expected_assistant_preview = "a".repeat(256);
    assert_eq!(
        loaded.first_user_preview.as_deref(),
        Some(expected_user_preview.as_str())
    );
    assert_eq!(
        loaded.last_assistant_preview.as_deref(),
        Some(expected_assistant_preview.as_str())
    );

    fs::remove_dir_all(root).expect("temp root should be removable");
}

#[tokio::test]
async fn backfill_prefers_header_session_name_for_title_when_present() {
    let root = tempdir_path("metadata-index-session-name");
    let sessions_dir = root.join("sessions");
    let work_dir = root.join("workspace").join("repo");
    fs::create_dir_all(&work_dir).expect("work dir should be creatable");
    let session_id: SessionId = "01914a5c-3c7e-7a2b-8abc-1234567890ab"
        .parse()
        .expect("fixture session id should parse");
    write_session_jsonl(
        &sessions_dir,
        &work_dir,
        &session_id,
        vec![
            SessionEntry {
                id: "header".to_string(),
                parent_id: None,
                timestamp: 1_717_514_800_000,
                kind: SessionEntryKind::Header(SessionHeader {
                    session_id: session_id.clone(),
                    work_dir: work_dir.clone(),
                    session_name: Some("Debug persistence rollout".to_string()),
                    initial_model: "gpt-4.1".to_string(),
                    git_head: Some("abc123".to_string()),
                    cli_version: Some("0.5.5".to_string()),
                }),
            },
            SessionEntry {
                id: "user-1".to_string(),
                parent_id: Some("header".to_string()),
                timestamp: 1_717_514_800_050,
                kind: SessionEntryKind::Item(ConversationItem::text(
                    Role::User,
                    "Fallback title should not be used.",
                )),
            },
        ],
    );
    let index = MetadataIndex::open(&root.join("index.sqlite"))
        .await
        .expect("metadata index should open sqlite file");

    index
        .backfill_from_jsonl(&sessions_dir)
        .await
        .expect("backfill should parse session jsonl");
    let loaded = index
        .get_session_meta(&session_id.to_string())
        .await
        .expect("backfilled metadata should be queryable");

    assert_eq!(loaded.title, "Debug persistence rollout");

    fs::remove_dir_all(root).expect("temp root should be removable");
}

#[tokio::test]
async fn list_sessions_repairs_missing_sqlite_rows_from_jsonl() {
    let root = tempdir_path("metadata-index-read-repair-restore");
    let sessions_dir = root.join("sessions");
    let work_dir = root.join("workspace").join("repo");
    fs::create_dir_all(&work_dir).expect("work dir should be creatable");
    let session_id: SessionId = "01914a5c-3c7e-7a2b-8abc-1234567890ab"
        .parse()
        .expect("fixture session id should parse");
    write_session_jsonl(
        &sessions_dir,
        &work_dir,
        &session_id,
        session_fixture_entries(
            &work_dir,
            &session_id,
            1_717_514_800_050,
            "Repair me from jsonl.",
        ),
    );
    let index_path = root.join("index.sqlite");
    let index = MetadataIndex::open(&index_path)
        .await
        .expect("metadata index should open sqlite file");
    let project_dir = ProjectDir::from_work_dir(&work_dir);

    index
        .backfill_from_jsonl(&sessions_dir)
        .await
        .expect("initial backfill should succeed");
    delete_session_row_for_test(&index_path, &session_id.to_string());

    let listed = index
        .list_sessions(&project_dir, SessionListOptions { repair: true })
        .await
        .expect("list should repair missing sqlite rows");

    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].session_id, session_id);

    fs::remove_dir_all(root).expect("temp root should be removable");
}

#[tokio::test]
async fn list_sessions_does_not_repair_missing_sqlite_rows_by_default() {
    let root = tempdir_path("metadata-index-list-read-only");
    let sessions_dir = root.join("sessions");
    let work_dir = root.join("workspace").join("repo");
    fs::create_dir_all(&work_dir).expect("work dir should be creatable");
    let session_id: SessionId = "01914a5c-3c7e-7a2b-8abc-1234567890ab"
        .parse()
        .expect("fixture session id should parse");
    write_session_jsonl(
        &sessions_dir,
        &work_dir,
        &session_id,
        session_fixture_entries(
            &work_dir,
            &session_id,
            1_717_514_800_050,
            "Do not repair me during a read-only list.",
        ),
    );
    let index_path = root.join("index.sqlite");
    let index = MetadataIndex::open(&index_path)
        .await
        .expect("metadata index should open sqlite file");
    let project_dir = ProjectDir::from_work_dir(&work_dir);

    index
        .backfill_from_jsonl(&sessions_dir)
        .await
        .expect("initial backfill should succeed");
    delete_session_row_for_test(&index_path, &session_id.to_string());

    let listed = index
        .list_sessions(&project_dir, SessionListOptions::default())
        .await
        .expect("read-only list should be queryable");

    assert!(listed.is_empty());
    assert!(matches!(
        index.get_session_meta(&session_id.to_string()).await,
        Err(crate::SessionStoreError::SessionNotFound { .. })
    ));

    fs::remove_dir_all(root).expect("temp root should be removable");
}

#[tokio::test]
async fn list_sessions_removes_orphaned_sqlite_rows_when_jsonl_is_deleted() {
    let root = tempdir_path("metadata-index-read-repair-prune");
    let sessions_dir = root.join("sessions");
    let work_dir = root.join("workspace").join("repo");
    fs::create_dir_all(&work_dir).expect("work dir should be creatable");
    let session_id: SessionId = "01914a5c-3c7e-7a2b-8abc-1234567890ab"
        .parse()
        .expect("fixture session id should parse");
    let jsonl_path = write_session_jsonl(
        &sessions_dir,
        &work_dir,
        &session_id,
        session_fixture_entries(
            &work_dir,
            &session_id,
            1_717_514_800_050,
            "Prune me when the jsonl is gone.",
        ),
    );
    let index = MetadataIndex::open(&root.join("index.sqlite"))
        .await
        .expect("metadata index should open sqlite file");
    let project_dir = ProjectDir::from_work_dir(&work_dir);

    index
        .backfill_from_jsonl(&sessions_dir)
        .await
        .expect("initial backfill should succeed");
    fs::remove_file(&jsonl_path).expect("fixture jsonl should be removable");

    let listed = index
        .list_sessions(&project_dir, SessionListOptions { repair: true })
        .await
        .expect("list should prune orphaned sqlite rows");

    assert!(listed.is_empty());
    assert!(matches!(
        index.get_session_meta(&session_id.to_string()).await,
        Err(crate::SessionStoreError::SessionNotFound { .. })
    ));

    fs::remove_dir_all(root).expect("temp root should be removable");
}

#[tokio::test]
async fn backfill_rebuilds_metadata_after_sqlite_file_is_deleted() {
    let root = tempdir_path("metadata-index-rebuild");
    let sessions_dir = root.join("sessions");
    let work_dir = root.join("workspace").join("repo");
    fs::create_dir_all(&work_dir).expect("work dir should be creatable");
    let first_id: SessionId = "01914a5c-3c7e-7a2b-8abc-1234567890ab"
        .parse()
        .expect("fixture session id should parse");
    let second_id: SessionId = "01914a5c-3c7e-7a2b-8abc-1234567890ac"
        .parse()
        .expect("fixture session id should parse");
    write_session_jsonl(
        &sessions_dir,
        &work_dir,
        &first_id,
        session_fixture_entries(
            &work_dir,
            &first_id,
            1_717_514_800_050,
            "First rebuilt session.",
        ),
    );
    write_session_jsonl(
        &sessions_dir,
        &work_dir,
        &second_id,
        session_fixture_entries(
            &work_dir,
            &second_id,
            1_717_514_900_050,
            "Second rebuilt session.",
        ),
    );
    let index_path = root.join("index.sqlite");
    let project_dir = ProjectDir::from_work_dir(&work_dir);

    let before_delete = {
        let index = MetadataIndex::open(&index_path)
            .await
            .expect("metadata index should open");
        index
            .backfill_from_jsonl(&sessions_dir)
            .await
            .expect("initial backfill should succeed");
        index
            .list_sessions(&project_dir, SessionListOptions::default())
            .await
            .expect("session list should be queryable after backfill")
    };

    fs::remove_file(&index_path).expect("sqlite file should be removable");

    let after_rebuild = {
        let index = MetadataIndex::open(&index_path)
            .await
            .expect("metadata index should reopen");
        index
            .backfill_from_jsonl(&sessions_dir)
            .await
            .expect("rebuild backfill should succeed");
        index
            .list_sessions(&project_dir, SessionListOptions::default())
            .await
            .expect("session list should be queryable after rebuild")
    };

    assert_eq!(after_rebuild, before_delete);

    fs::remove_dir_all(root).expect("temp root should be removable");
}

#[tokio::test]
async fn multiple_metadata_indexes_can_write_different_sessions_without_lock_conflicts() {
    let root = tempdir_path("metadata-index-concurrency");
    fs::create_dir_all(&root).expect("temp root should be creatable");
    let index_path = root.join("index.sqlite");
    let first_meta = sample_session_meta_with("01914a5c-3c7e-7a2b-8abc-1234567890ab", 10);
    let second_meta = sample_session_meta_with("01914a5c-3c7e-7a2b-8abc-1234567890ac", 20);
    let first_id = first_meta.session_id.to_string();
    let second_id = second_meta.session_id.to_string();
    let barrier = Arc::new(tokio::sync::Barrier::new(2));

    let first_future = async {
        let index = MetadataIndex::open(&index_path)
            .await
            .expect("first index should open");
        barrier.wait().await;
        index
            .upsert_session(&first_meta)
            .await
            .expect("first session should persist");
    };
    let second_future = async {
        let index = MetadataIndex::open(&index_path)
            .await
            .expect("second index should open");
        barrier.wait().await;
        index
            .upsert_session(&second_meta)
            .await
            .expect("second session should persist");
    };

    tokio::join!(first_future, second_future);

    let index = MetadataIndex::open(&index_path)
        .await
        .expect("verification index should open");
    assert_eq!(
        index
            .get_session_meta(&first_id)
            .await
            .expect("first session should exist")
            .session_id
            .to_string(),
        first_id
    );
    assert_eq!(
        index
            .get_session_meta(&second_id)
            .await
            .expect("second session should exist")
            .session_id
            .to_string(),
        second_id
    );

    fs::remove_dir_all(root).expect("temp root should be removable");
}

#[tokio::test]
async fn open_enables_wal_mode() {
    let root = tempdir_path("metadata-index-wal");
    let index_path = root.join("index.sqlite");

    let _index = MetadataIndex::open(&index_path)
        .await
        .expect("metadata index should open sqlite file");

    let conn = Connection::open(&index_path).expect("sqlite file should be reopenable");
    let journal_mode: String = conn
        .query_row("PRAGMA journal_mode;", [], |row| row.get(0))
        .expect("journal mode should be queryable");

    assert_eq!(journal_mode.to_ascii_lowercase(), "wal");

    fs::remove_dir_all(root).expect("temp root should be removable");
}

#[test]
fn repair_plan_skips_unchanged_files() {
    let unchanged = DiscoveredSessionFile {
        path: PathBuf::from("/sessions/a.jsonl"),
        fingerprint: SessionFileFingerprint {
            file_size: 128,
            modified_at_ms: 42,
        },
    };
    let changed = DiscoveredSessionFile {
        path: PathBuf::from("/sessions/b.jsonl"),
        fingerprint: SessionFileFingerprint {
            file_size: 256,
            modified_at_ms: 99,
        },
    };
    let indexed_files = vec![
        IndexedProjectFile {
            session_id: "a".to_string(),
            jsonl_path: unchanged.path.clone(),
            fingerprint: Some(unchanged.fingerprint.clone()),
        },
        IndexedProjectFile {
            session_id: "b".to_string(),
            jsonl_path: changed.path.clone(),
            fingerprint: Some(SessionFileFingerprint {
                file_size: 255,
                modified_at_ms: 99,
            }),
        },
        IndexedProjectFile {
            session_id: "stale".to_string(),
            jsonl_path: PathBuf::from("/sessions/missing.jsonl"),
            fingerprint: Some(SessionFileFingerprint {
                file_size: 1,
                modified_at_ms: 1,
            }),
        },
    ];

    let plan = build_repair_plan(&[unchanged, changed.clone()], &indexed_files);

    assert_eq!(plan.stale_session_ids, vec!["stale".to_string()]);
    assert_eq!(plan.files_to_refresh, vec![changed]);
}

fn sample_session_meta() -> SessionMeta {
    sample_session_meta_with("01914a5c-3c7e-7a2b-8abc-1234567890ab", 1_717_514_800_123)
}

fn sample_session_meta_with(session_id: &str, updated_at: i64) -> SessionMeta {
    let session_id: SessionId = session_id.parse().expect("fixture session id should parse");
    SessionMeta {
        session_id,
        project_dir: ProjectDir::from_stored_path(PathBuf::from("/repo")),
        title: "Inspect session index".to_string(),
        preview: Some("please persist this metadata".to_string()),
        first_user_preview: Some("first user preview".to_string()),
        last_assistant_preview: Some("last assistant preview".to_string()),
        total_tokens: 512,
        model: Some("gpt-4.1".to_string()),
        created_at: 1_717_514_800_000,
        updated_at,
        git_head: Some("abc123".to_string()),
        jsonl_path: PathBuf::from("/tmp/session.jsonl"),
        size_bytes: None,
    }
}

fn tempdir_path(label: &str) -> PathBuf {
    std::env::temp_dir().join(format!("hunea-session-store-{label}-{}", Uuid::now_v7()))
}

fn write_session_jsonl(
    sessions_dir: &std::path::Path,
    work_dir: &std::path::Path,
    session_id: &SessionId,
    entries: Vec<SessionEntry>,
) -> PathBuf {
    let project_dir = sessions_dir.join(ProjectDir::from_work_dir(work_dir).encoded_session_dir());
    fs::create_dir_all(&project_dir).expect("project dir should be creatable");
    let path = project_dir.join(session_filename(session_id));
    let mut writer = JsonlWriter::new(path.clone());
    writer
        .write_batch(&entries)
        .expect("fixture session jsonl should be writable");
    path
}

fn session_fixture_entries(
    work_dir: &std::path::Path,
    session_id: &SessionId,
    updated_at: i64,
    user_text: &str,
) -> Vec<SessionEntry> {
    vec![
        SessionEntry {
            id: "header".to_string(),
            parent_id: None,
            timestamp: updated_at - 1,
            kind: SessionEntryKind::Header(SessionHeader {
                session_id: session_id.clone(),
                work_dir: work_dir.to_path_buf(),
                session_name: None,
                initial_model: "gpt-4.1".to_string(),
                git_head: Some("abc123".to_string()),
                cli_version: Some("0.5.5".to_string()),
            }),
        },
        SessionEntry {
            id: "user-1".to_string(),
            parent_id: Some("header".to_string()),
            timestamp: updated_at,
            kind: SessionEntryKind::Item(ConversationItem::text(Role::User, user_text)),
        },
    ]
}

fn delete_session_row_for_test(index_path: &Path, session_id: &str) {
    initialize_database(index_path).expect("sqlite schema should initialize");
    let conn = Connection::open(index_path).expect("sqlite file should reopen");
    conn.execute(
        "DELETE FROM session_repair_state WHERE session_id = ?1",
        rusqlite::params![session_id],
    )
    .expect("repair state row should be deletable");
    conn.execute(
        "DELETE FROM sessions WHERE session_id = ?1",
        rusqlite::params![session_id],
    )
    .expect("session row should be deletable");
}
