use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

use provider_protocol::{ConversationItem, Role};
use session_store::{
    InMemorySessionStore, LocalSessionStore, SessionHeader, SessionId, SessionStore,
    SessionStoreError,
};
use uuid::Uuid;

#[tokio::test]
async fn local_store_creates_appends_and_resolves_history() {
    let root = tempdir_path("local-store-e2e");
    let work_dir = root.join("workspace").join("repo");
    fs::create_dir_all(&work_dir).expect("work dir should be creatable");
    let store = LocalSessionStore::open_in(root.clone())
        .await
        .expect("local store should open temp root");
    let header = sample_header(&work_dir, "gpt-4.1", Some("session-a"));
    let user_item = ConversationItem::text(Role::User, "hello");
    let assistant_item = ConversationItem::text(Role::Assistant, "hi");

    let session_id = store
        .create_session(header)
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

    let resolved = store
        .resolve(&session_id, None)
        .await
        .expect("resolve should return canonical history");

    assert_eq!(resolved, vec![user_item, assistant_item]);
}

#[tokio::test]
async fn local_store_isolates_multiple_sessions() {
    let root = tempdir_path("local-store-isolation");
    let work_dir = root.join("workspace").join("repo");
    fs::create_dir_all(&work_dir).expect("work dir should be creatable");
    let store = LocalSessionStore::open_in(root)
        .await
        .expect("local store should open temp root");
    let first_id = store
        .create_session(sample_header(&work_dir, "gpt-4.1", Some("first")))
        .await
        .expect("first session should be created");
    let second_id = store
        .create_session(sample_header(&work_dir, "gpt-4.1", Some("second")))
        .await
        .expect("second session should be created");
    let first_item = ConversationItem::text(Role::User, "first");
    let second_item = ConversationItem::text(Role::User, "second");

    store
        .append(&first_id, first_item.clone())
        .await
        .expect("first session append should succeed");
    store
        .append(&second_id, second_item.clone())
        .await
        .expect("second session append should succeed");

    let first_history = store
        .resolve(&first_id, None)
        .await
        .expect("first history should resolve");
    let second_history = store
        .resolve(&second_id, None)
        .await
        .expect("second history should resolve");

    assert_eq!(first_history, vec![first_item]);
    assert_eq!(second_history, vec![second_item]);
}

#[tokio::test]
async fn local_store_branches_after_leaf_override() {
    let root = tempdir_path("local-store-branch");
    let work_dir = root.join("workspace").join("repo");
    fs::create_dir_all(&work_dir).expect("work dir should be creatable");
    let store = LocalSessionStore::open_in(root)
        .await
        .expect("local store should open temp root");
    let session_id = store
        .create_session(sample_header(&work_dir, "gpt-4.1", Some("branching")))
        .await
        .expect("session should be created");
    let item_a = ConversationItem::text(Role::User, "A");
    let item_b = ConversationItem::text(Role::Assistant, "B");
    let item_c = ConversationItem::text(Role::Assistant, "C");
    let item_d = ConversationItem::text(Role::Assistant, "D");

    store
        .append(&session_id, item_a.clone())
        .await
        .expect("A should append");
    store
        .append(&session_id, item_b.clone())
        .await
        .expect("B should append");
    store
        .append(&session_id, item_c)
        .await
        .expect("C should append");

    let item_a_id = first_item_entry_id(
        &store
            .get_session_meta(&session_id)
            .await
            .expect("meta should load")
            .jsonl_path,
    )
    .expect("session file should contain first item id");
    store
        .set_leaf(&session_id, Some(&item_a_id))
        .await
        .expect("set_leaf should succeed");
    store
        .append(&session_id, item_d.clone())
        .await
        .expect("D should append on branched leaf");

    let resolved = store
        .resolve(&session_id, None)
        .await
        .expect("branched history should resolve");

    assert_eq!(resolved, vec![item_a, item_d]);
}

#[tokio::test]
async fn local_store_resolves_explicit_leaf_even_after_branch_override() {
    let root = tempdir_path("local-store-explicit-leaf");
    let work_dir = root.join("workspace").join("repo");
    fs::create_dir_all(&work_dir).expect("work dir should be creatable");
    let store = LocalSessionStore::open_in(root)
        .await
        .expect("local store should open temp root");
    let session_id = store
        .create_session(sample_header(&work_dir, "gpt-4.1", Some("explicit-leaf")))
        .await
        .expect("session should be created");
    let item_a = ConversationItem::text(Role::User, "A");
    let item_b = ConversationItem::text(Role::Assistant, "B");
    let item_c = ConversationItem::text(Role::Assistant, "C");

    store
        .append(&session_id, item_a.clone())
        .await
        .expect("A should append");
    store
        .append(&session_id, item_b.clone())
        .await
        .expect("B should append");
    store
        .append(&session_id, item_c.clone())
        .await
        .expect("C should append");

    let item_ids = item_entry_ids(
        &store
            .get_session_meta(&session_id)
            .await
            .expect("meta should load")
            .jsonl_path,
    )
    .expect("session file should contain item ids");
    let item_a_id = item_ids[0].clone();
    let item_c_id = item_ids[2].clone();

    store
        .set_leaf(&session_id, Some(&item_a_id))
        .await
        .expect("set_leaf should succeed");

    let resolved = store
        .resolve(&session_id, Some(&item_c_id))
        .await
        .expect("explicit leaf should resolve requested branch");

    assert_eq!(resolved, vec![item_a, item_b, item_c]);
}

#[tokio::test]
async fn local_store_flush_persists_complete_jsonl() {
    let root = tempdir_path("local-store-flush");
    let work_dir = root.join("workspace").join("repo");
    fs::create_dir_all(&work_dir).expect("work dir should be creatable");
    let store = LocalSessionStore::open_in(root)
        .await
        .expect("local store should open temp root");
    let session_id = store
        .create_session(sample_header(&work_dir, "gpt-4.1", Some("flush")))
        .await
        .expect("session should be created");

    store
        .append(&session_id, ConversationItem::text(Role::User, "hello"))
        .await
        .expect("append should succeed");
    store
        .append(&session_id, ConversationItem::text(Role::Assistant, "hi"))
        .await
        .expect("append should succeed");
    store
        .flush(&session_id)
        .await
        .expect("flush should succeed");

    let meta = store
        .get_session_meta(&session_id)
        .await
        .expect("meta should load after flush");
    let jsonl = fs::read_to_string(&meta.jsonl_path).expect("jsonl should be readable after flush");

    assert_eq!(jsonl.lines().count(), 3);
}

#[tokio::test]
async fn local_store_lists_sessions_by_project_and_updated_order() {
    let root = tempdir_path("local-store-list");
    let repo_a = root.join("workspace").join("repo-a");
    let repo_b = root.join("workspace").join("repo-b");
    fs::create_dir_all(&repo_a).expect("repo A should be creatable");
    fs::create_dir_all(&repo_b).expect("repo B should be creatable");
    let store = LocalSessionStore::open_in(root)
        .await
        .expect("local store should open temp root");

    let older_id = store
        .create_session(sample_header(&repo_a, "gpt-4.1", Some("older")))
        .await
        .expect("older session should be created");
    store
        .append(
            &older_id,
            ConversationItem::text(Role::User, "older preview"),
        )
        .await
        .expect("older append should succeed");

    tokio::time::sleep(Duration::from_millis(2)).await;

    let newer_id = store
        .create_session(sample_header(&repo_a, "gpt-4.1", Some("newer")))
        .await
        .expect("newer session should be created");
    store
        .append(
            &newer_id,
            ConversationItem::text(Role::User, "newer preview"),
        )
        .await
        .expect("newer append should succeed");

    let other_project_id = store
        .create_session(sample_header(&repo_b, "gpt-4.1", Some("other-project")))
        .await
        .expect("other project session should be created");
    store
        .append(
            &other_project_id,
            ConversationItem::text(Role::User, "other preview"),
        )
        .await
        .expect("other project append should succeed");

    let listed = store
        .list_sessions(repo_a.to_string_lossy().as_ref())
        .await
        .expect("repo A sessions should list");

    assert_eq!(listed.len(), 2);
    assert_eq!(
        listed
            .iter()
            .map(|meta| meta.session_id.clone())
            .collect::<Vec<_>>(),
        vec![newer_id, older_id]
    );
    assert_eq!(listed[0].preview.as_deref(), Some("newer preview"));
}

#[tokio::test]
async fn in_memory_store_matches_local_store_for_same_linear_history() {
    let root = tempdir_path("local-store-consistency");
    let work_dir = root.join("workspace").join("repo");
    fs::create_dir_all(&work_dir).expect("work dir should be creatable");
    let local_store = LocalSessionStore::open_in(root)
        .await
        .expect("local store should open temp root");
    let memory_store = InMemorySessionStore::new();
    let local_header = sample_header(&work_dir, "gpt-4.1", Some("consistent"));
    let memory_header = sample_header(&work_dir, "gpt-4.1", Some("consistent"));
    let items = vec![
        ConversationItem::text(Role::User, "hello"),
        ConversationItem::text(Role::Assistant, "hi"),
        ConversationItem::text(Role::User, "follow up"),
    ];

    let local_id = local_store
        .create_session(local_header)
        .await
        .expect("local session should be created");
    let memory_id = memory_store
        .create_session(memory_header)
        .await
        .expect("memory session should be created");

    for item in &items {
        local_store
            .append(&local_id, item.clone())
            .await
            .expect("local append should succeed");
        memory_store
            .append(&memory_id, item.clone())
            .await
            .expect("memory append should succeed");
    }

    let local_resolved = local_store
        .resolve(&local_id, None)
        .await
        .expect("local resolve should succeed");
    let memory_resolved = memory_store
        .resolve(&memory_id, None)
        .await
        .expect("memory resolve should succeed");
    let local_meta = local_store
        .get_session_meta(&local_id)
        .await
        .expect("local meta should load");
    let memory_meta = memory_store
        .get_session_meta(&memory_id)
        .await
        .expect("memory meta should load");

    assert_eq!(local_resolved, memory_resolved);
    assert_eq!(memory_resolved, items);
    assert_eq!(local_meta.project_dir, memory_meta.project_dir);
    assert_eq!(local_meta.title, memory_meta.title);
    assert_eq!(local_meta.preview, memory_meta.preview);
    assert_eq!(local_meta.model, memory_meta.model);
}

fn sample_header(work_dir: &Path, model: &str, session_name: Option<&str>) -> SessionHeader {
    SessionHeader {
        session_id: SessionId::new(),
        work_dir: work_dir.to_path_buf(),
        session_name: session_name.map(str::to_string),
        initial_model: model.to_string(),
        git_head: Some("abc123".to_string()),
        cli_version: Some("0.5.6".to_string()),
    }
}

fn first_item_entry_id(path: &Path) -> Result<String, SessionStoreError> {
    Ok(item_entry_ids(path)?
        .into_iter()
        .next()
        .expect("session fixture should include first item"))
}

fn item_entry_ids(path: &Path) -> Result<Vec<String>, SessionStoreError> {
    let jsonl = fs::read_to_string(path).map_err(|source| SessionStoreError::IoError { source })?;
    jsonl
        .lines()
        .skip(1)
        .map(|line| {
            let value: serde_json::Value =
                serde_json::from_str(line).expect("item line should parse");
            Ok(value["id"]
                .as_str()
                .expect("entry id should exist")
                .to_string())
        })
        .collect()
}

fn tempdir_path(label: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "hunea-session-store-test-{label}-{}",
        Uuid::now_v7()
    ))
}
