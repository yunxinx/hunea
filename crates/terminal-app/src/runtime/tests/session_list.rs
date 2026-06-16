use super::support::*;

#[test]
fn list_sessions_emits_session_picker_rows_for_current_project() {
    let work_dir = temp_test_dir("list-sessions-work");
    let store = Arc::new(InMemorySessionStore::new());
    let store_runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("session store runtime should start");
    let header = SessionHeader {
        session_id: SessionId::new(),
        work_dir: work_dir.clone(),
        session_name: None,
        initial_model: "qwen3".to_string(),
        git_head: None,
        cli_version: None,
    };
    store_runtime
        .block_on(async {
            let session_id = store.create_session(header.clone()).await?;
            let named_header = SessionHeader {
                session_id: SessionId::new(),
                session_name: Some("Named session should not replace first user".to_string()),
                ..header.clone()
            };
            let named_session_id = store.create_session(named_header).await?;
            store
                .append(
                    &named_session_id,
                    ConversationItem::text(Role::User, "first named user"),
                )
                .await?;
            store
                .append(
                    &named_session_id,
                    ConversationItem::text(Role::Assistant, "last named assistant"),
                )
                .await?;
            store
                .append(
                    &session_id,
                    ConversationItem::text(Role::User, "hello resume"),
                )
                .await?;
            store
                .append(
                    &session_id,
                    ConversationItem::text(Role::Assistant, "resume preview answer"),
                )
                .await?;
            Ok::<(), session_store::SessionStoreError>(())
        })
        .expect("session fixture should persist");
    let mut coordinator = runtime_coordinator(AppRuntimeOptions {
        session_store: Some(store),
        session_header_template: Some(header),
        ..AppRuntimeOptions::default()
    });

    coordinator
        .handle_runtime_command(RuntimeCommand::ListSessions)
        .expect("list sessions should succeed");

    let rows = wait_for_session_list_rows(&mut coordinator);
    assert_eq!(rows.len(), 2);
    let row = rows
        .iter()
        .find(|row| row.first_user_message == "hello resume")
        .expect("ordinary session row should use first user message");
    assert_eq!(row.last_assistant_message, "resume preview answer");
    let named_row = rows
        .iter()
        .find(|row| row.title == "Named session should not replace first user")
        .expect("named session row should be present");
    assert_eq!(named_row.first_user_message, "first named user");
    assert_eq!(named_row.last_assistant_message, "last named assistant");
    cleanup(&work_dir);
}

#[test]
fn list_sessions_dispatch_does_not_wait_for_store_io() {
    let work_dir = temp_test_dir("list-sessions-nonblocking-work");
    let inner_store = Arc::new(InMemorySessionStore::new());
    let store_runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("session store runtime should start");
    let header = SessionHeader {
        session_id: SessionId::new(),
        work_dir: work_dir.clone(),
        session_name: None,
        initial_model: "qwen3".to_string(),
        git_head: None,
        cli_version: None,
    };
    store_runtime
        .block_on(async {
            let session_id = inner_store.create_session(header.clone()).await?;
            inner_store
                .append(&session_id, ConversationItem::text(Role::User, "hello"))
                .await?;
            Ok::<(), SessionStoreError>(())
        })
        .expect("session fixture should persist");
    let (list_started_tx, list_started_rx) = mpsc::channel();
    let (list_release_tx, list_release_rx) = mpsc::channel();
    let store = Arc::new(DelayedListSessionStore::new(
        inner_store,
        list_started_tx,
        list_release_rx,
    ));
    let mut coordinator = runtime_coordinator(AppRuntimeOptions {
        session_store: Some(store),
        session_header_template: Some(header),
        ..AppRuntimeOptions::default()
    });

    let (dispatch_done_tx, dispatch_done_rx) = mpsc::channel();
    thread::spawn(move || {
        let receipt = coordinator.handle_runtime_command(RuntimeCommand::ListSessions);
        let _ = dispatch_done_tx.send((receipt, coordinator));
    });
    list_started_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("session list worker should start");
    let early_dispatch = dispatch_done_rx.recv_timeout(Duration::from_millis(100));
    if early_dispatch.is_err() {
        let _ = list_release_tx.send(());
        let _ = dispatch_done_rx.recv_timeout(Duration::from_secs(1));
        panic!("list sessions dispatch should not wait for store IO");
    }
    let (receipt, mut coordinator) = early_dispatch.expect("dispatch result should be available");
    assert_eq!(
        receipt.expect("list sessions command should be accepted"),
        RuntimeCommandReceipt::Accepted
    );
    assert!(
        RuntimeCoordinator::drain_runtime_events(&mut coordinator).is_empty(),
        "no result event should be available before store IO completes"
    );
    list_release_tx
        .send(())
        .expect("delayed list should be releasable");

    let rows = wait_for_session_list_rows(&mut coordinator);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].first_user_message, "hello");
    cleanup(&work_dir);
}

#[test]
fn list_sessions_builds_rows_from_metadata_without_loading_full_sessions() {
    let work_dir = temp_test_dir("list-sessions-metadata-only-work");
    let inner_store = Arc::new(InMemorySessionStore::new());
    let store_runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("session store runtime should start");
    let header = SessionHeader {
        session_id: SessionId::new(),
        work_dir: work_dir.clone(),
        session_name: None,
        initial_model: "qwen3".to_string(),
        git_head: None,
        cli_version: None,
    };
    store_runtime
        .block_on(async {
            let session_id = inner_store.create_session(header.clone()).await?;
            inner_store
                .append(
                    &session_id,
                    ConversationItem::text(Role::User, "metadata first user"),
                )
                .await?;
            inner_store
                .append(
                    &session_id,
                    ConversationItem::text(Role::Assistant, "metadata assistant answer"),
                )
                .await?;
            Ok::<(), SessionStoreError>(())
        })
        .expect("session fixture should persist");
    let store = Arc::new(LoadCountingSessionStore::new(inner_store));
    let mut coordinator = runtime_coordinator(AppRuntimeOptions {
        session_store: Some(store.clone()),
        session_header_template: Some(header),
        ..AppRuntimeOptions::default()
    });

    coordinator
        .handle_runtime_command(RuntimeCommand::ListSessions)
        .expect("list sessions should succeed");

    let rows = wait_for_session_list_rows(&mut coordinator);
    assert_eq!(store.load_session_calls(), 0);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].first_user_message, "metadata first user");
    assert_eq!(rows[0].last_assistant_message, "metadata assistant answer");
    cleanup(&work_dir);
}

#[test]
fn list_sessions_auto_repairs_external_jsonl_after_fast_sqlite_rows() {
    let root = temp_test_dir("list-sessions-auto-repair-root");
    let work_dir = root.join("workspace");
    fs::create_dir_all(&work_dir).expect("workspace should be creatable");
    let store_runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("session store runtime should start");
    let store = Arc::new(
        store_runtime
            .block_on(LocalSessionStore::open_in(root.clone()))
            .expect("local store should open"),
    );
    let cached_header = SessionHeader {
        session_id: SessionId::new(),
        work_dir: work_dir.clone(),
        session_name: Some("cached session".to_string()),
        initial_model: "qwen3".to_string(),
        git_head: None,
        cli_version: None,
    };
    store_runtime
        .block_on(async {
            let session_id = store.create_session(cached_header.clone()).await?;
            store
                .append(
                    &session_id,
                    ConversationItem::text(Role::User, "cached user"),
                )
                .await?;
            store.flush_all().await?;
            Ok::<(), SessionStoreError>(())
        })
        .expect("cached session should persist");
    let external_session_id = SessionId::new();
    write_external_session_jsonl(
        &root,
        &work_dir,
        &external_session_id,
        "external user from jsonl",
    );
    let external_session_key = external_session_id.to_string();
    let mut coordinator = runtime_coordinator(AppRuntimeOptions {
        session_store: Some(store),
        session_header_template: Some(SessionHeader {
            session_id: SessionId::new(),
            work_dir: work_dir.clone(),
            session_name: None,
            initial_model: "qwen3".to_string(),
            git_head: None,
            cli_version: None,
        }),
        ..AppRuntimeOptions::default()
    });

    coordinator
        .handle_runtime_command(RuntimeCommand::ListSessions)
        .expect("list sessions should start");

    let mut loaded_rows = wait_for_runtime_events(&mut coordinator, "initial session list rows")
        .into_iter()
        .filter_map(|event| match event {
            RuntimeEvent::SessionListLoaded { rows } => Some(rows),
            _ => None,
        });
    let initial_rows = loaded_rows
        .next()
        .expect("first event should include session rows");
    assert!(
        initial_rows
            .iter()
            .any(|row| row.first_user_message == "cached user"),
        "first event should show fast SQLite rows"
    );
    assert!(
        !initial_rows
            .iter()
            .any(|row| row.session_id == external_session_key),
        "external JSONL should not be visible before background repair"
    );

    let repaired_rows = loaded_rows
        .next()
        .unwrap_or_else(|| wait_for_session_list_rows(&mut coordinator));
    assert!(
        repaired_rows
            .iter()
            .any(|row| row.session_id == external_session_key
                && row.first_user_message == "external user from jsonl"),
        "background repair should refresh the picker rows with external JSONL sessions"
    );
    cleanup(&root);
}

#[test]
fn list_sessions_excludes_active_session() {
    let work_dir = temp_test_dir("list-sessions-excludes-active-work");
    let store = Arc::new(InMemorySessionStore::new());
    let store_runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("session store runtime should start");
    let active_header = SessionHeader {
        session_id: SessionId::new(),
        work_dir: work_dir.clone(),
        session_name: Some("active session".to_string()),
        initial_model: "qwen3".to_string(),
        git_head: None,
        cli_version: None,
    };
    let other_header = SessionHeader {
        session_id: SessionId::new(),
        session_name: Some("other session".to_string()),
        ..active_header.clone()
    };
    let (active_session_id, other_session_id) = store_runtime
        .block_on(async {
            let active_session_id = store.create_session(active_header.clone()).await?;
            let other_session_id = store.create_session(other_header).await?;
            Ok::<(SessionId, SessionId), session_store::SessionStoreError>((
                active_session_id,
                other_session_id,
            ))
        })
        .expect("session fixture should persist");
    let mut coordinator = runtime_coordinator(AppRuntimeOptions {
        session_store: Some(store),
        session_header_template: Some(SessionHeader {
            session_id: active_session_id.clone(),
            ..active_header
        }),
        ..AppRuntimeOptions::default()
    });

    coordinator
        .handle_runtime_command(RuntimeCommand::ListSessions)
        .expect("list sessions should succeed");

    let rows = wait_for_session_list_rows(&mut coordinator);
    assert_eq!(
        rows.iter()
            .map(|row| row.session_id.as_str())
            .collect::<Vec<_>>(),
        vec![other_session_id.to_string()]
    );
    cleanup(&work_dir);
}
