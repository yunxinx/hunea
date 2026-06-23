use super::support::*;

#[test]
fn load_entry_tree_emits_rewind_targets_for_active_session() {
    let work_dir = temp_test_dir("load-entry-tree-work");
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
    let session_id = store_runtime
        .block_on(async {
            let session_id = store.create_session(header.clone()).await?;
            store
                .append(&session_id, ConversationItem::text(Role::User, "first"))
                .await?;
            store
                .append(
                    &session_id,
                    ConversationItem::text(Role::Assistant, "answer"),
                )
                .await?;
            store
                .append(&session_id, ConversationItem::text(Role::User, "second"))
                .await?;
            Ok::<SessionId, session_store::SessionStoreError>(session_id)
        })
        .expect("session fixture should persist");
    let mut coordinator = runtime_coordinator(AppRuntimeOptions {
        session_store: Some(store),
        session_header_template: Some(header),
        ..AppRuntimeOptions::default()
    });
    coordinator
        .handle_runtime_command(RuntimeCommand::ResumeSession {
            session_id: session_id.to_string(),
        })
        .expect("resume session should succeed");
    wait_for_session_resumed(&mut coordinator);

    coordinator
        .handle_runtime_command(RuntimeCommand::LoadEntryTree {
            request_id: request_id(1),
        })
        .expect("load entry tree should succeed");

    let payload = wait_for_session_tree(&mut coordinator);
    let second_user = payload
        .rows
        .iter()
        .find(|row| row.preview_content == "second")
        .expect("second user row should be present");
    assert_eq!(
        payload.current_row_id.as_deref(),
        Some(second_user.row_id.as_str()),
        "runtime payload should expose the committed path current row directly"
    );
    assert_eq!(second_user.kind, SessionTreeRowKind::User);
    assert_eq!(second_user.rewind_prefill.as_deref(), Some("second"));
    let assistant = payload
        .rows
        .iter()
        .find(|row| row.preview_content == "answer")
        .expect("assistant row should be present");
    assert_eq!(
        second_user.rewind_target_id.as_deref(),
        Some(assistant.row_id.as_str())
    );
    assert!(payload.rows.iter().any(|row| row.is_current));
    cleanup(&work_dir);
}

#[test]
fn load_entry_tree_emits_empty_tree_for_new_unpersisted_session() {
    let work_dir = temp_test_dir("load-entry-tree-empty-work");
    let store = Arc::new(InMemorySessionStore::new());
    let header = SessionHeader {
        session_id: SessionId::new(),
        work_dir: work_dir.clone(),
        session_name: None,
        initial_model: "qwen3".to_string(),
        git_head: None,
        cli_version: None,
    };
    let mut coordinator = runtime_coordinator(AppRuntimeOptions {
        session_store: Some(store),
        session_header_template: Some(header),
        ..AppRuntimeOptions::default()
    });

    coordinator
        .handle_runtime_command(RuntimeCommand::LoadEntryTree {
            request_id: request_id(2),
        })
        .expect("empty new session tree should load as an empty payload");

    let payload = wait_for_session_tree(&mut coordinator);
    assert!(
        payload.rows.is_empty(),
        "new sessions without messages should render an empty tree"
    );
    assert_eq!(payload.current_row_id, None);
    cleanup(&work_dir);
}

#[test]
fn load_copy_picker_tree_emits_empty_tree_for_new_unpersisted_session() {
    let work_dir = temp_test_dir("load-copy-picker-tree-empty-work");
    let store = Arc::new(InMemorySessionStore::new());
    let header = SessionHeader {
        session_id: SessionId::new(),
        work_dir: work_dir.clone(),
        session_name: None,
        initial_model: "qwen3".to_string(),
        git_head: None,
        cli_version: None,
    };
    let mut coordinator = runtime_coordinator(AppRuntimeOptions {
        session_store: Some(store),
        session_header_template: Some(header),
        ..AppRuntimeOptions::default()
    });

    coordinator
        .handle_runtime_command(RuntimeCommand::LoadCopyPickerTree {
            request_id: request_id(3),
        })
        .expect("empty new session copy picker should load as an empty payload");

    let payload = wait_for_copy_picker_tree(&mut coordinator);
    assert!(
        payload.rows.is_empty(),
        "new sessions without messages should render an empty copy picker"
    );
    assert_eq!(payload.current_row_id, None);
    cleanup(&work_dir);
}

#[test]
fn load_copy_picker_tree_failure_emits_copy_picker_error_event() {
    let work_dir = temp_test_dir("load-copy-picker-tree-failure-work");
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
    let session_id = store_runtime
        .block_on(async {
            let session_id = inner_store.create_session(header.clone()).await?;
            inner_store
                .append(&session_id, ConversationItem::text(Role::User, "first"))
                .await?;
            Ok::<SessionId, session_store::SessionStoreError>(session_id)
        })
        .expect("session fixture should persist");
    let store = Arc::new(FailingSessionStore::new(
        inner_store,
        FailingSessionStoreLoad::SessionTree,
    ));
    let mut coordinator = runtime_coordinator(AppRuntimeOptions {
        session_store: Some(store),
        session_header_template: Some(header),
        ..AppRuntimeOptions::default()
    });
    coordinator
        .handle_runtime_command(RuntimeCommand::ResumeSession {
            session_id: session_id.to_string(),
        })
        .expect("resume session should succeed");
    wait_for_session_resumed(&mut coordinator);

    coordinator
        .handle_runtime_command(RuntimeCommand::LoadCopyPickerTree {
            request_id: request_id(4),
        })
        .expect("copy picker tree command should be accepted before async load fails");

    let message = wait_for_runtime_event(
        &mut coordinator,
        |event| match event {
            RuntimeEvent::CopyPickerTreeLoadFailed { message, .. } => Some(message),
            RuntimeEvent::Failed { message, .. } => {
                panic!("copy picker tree load failure must not become global failure: {message}")
            }
            _ => None,
        },
        "copy picker tree load failure",
    );
    assert!(message.contains("injected session tree load failure"));
    cleanup(&work_dir);
}
