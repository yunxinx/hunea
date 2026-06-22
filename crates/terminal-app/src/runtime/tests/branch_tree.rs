use super::support::*;

#[test]
fn load_branch_tree_emits_empty_tree_for_new_unpersisted_session() {
    let work_dir = temp_test_dir("load-branch-tree-empty-work");
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
        .handle_runtime_command(RuntimeCommand::LoadBranchTree {
            request_id: request_id(10),
        })
        .expect("empty new session branch tree should load as an empty payload");

    let payload = wait_for_session_branch_tree(&mut coordinator);
    assert!(
        payload.nodes.is_empty(),
        "new sessions without messages should render an empty branch tree"
    );
    assert_eq!(payload.current_branch_row_id, None);
    assert_eq!(payload.total_message_count, 0);
    cleanup(&work_dir);
}

#[test]
fn load_branch_tree_emits_branch_roots_for_active_session() {
    let work_dir = temp_test_dir("load-branch-tree-work");
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
    let (session_id, root_id, beta_id, alt_id) = store_runtime
        .block_on(async {
            let session_id = store.create_session(header.clone()).await?;
            let root_id = store
                .append(
                    &session_id,
                    ConversationItem::text(Role::User, "root question"),
                )
                .await?;
            store
                .append(
                    &session_id,
                    ConversationItem::text(Role::Assistant, "alpha"),
                )
                .await?;
            store.set_leaf(&session_id, Some(&root_id)).await?;
            let beta_id = store
                .append(&session_id, ConversationItem::text(Role::Assistant, "beta"))
                .await?;
            store
                .append(&session_id, ConversationItem::text(Role::User, "follow"))
                .await?;
            store
                .append(
                    &session_id,
                    ConversationItem::text(Role::Assistant, "follow answer"),
                )
                .await?;
            store.set_leaf(&session_id, Some(&beta_id)).await?;
            let alt_id = store
                .append(&session_id, ConversationItem::text(Role::User, "alt"))
                .await?;
            store
                .append(
                    &session_id,
                    ConversationItem::text(Role::Assistant, "alt answer"),
                )
                .await?;
            Ok::<(SessionId, String, String, String), session_store::SessionStoreError>((
                session_id, root_id, beta_id, alt_id,
            ))
        })
        .expect("branch tree fixture should persist");
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
        .handle_runtime_command(RuntimeCommand::LoadBranchTree {
            request_id: request_id(11),
        })
        .expect("load branch tree should succeed");

    let payload = wait_for_session_branch_tree(&mut coordinator);
    assert_eq!(payload.nodes.len(), 5);
    assert_eq!(payload.total_message_count, 7);
    assert_eq!(
        payload.current_branch_row_id.as_deref(),
        Some(alt_id.as_str())
    );
    assert!(payload.nodes.iter().any(|node| {
        node.branch.branch_row_id == beta_id
            && node.parent_branch_row_id.as_deref() == Some(root_id.as_str())
            && node.branch.message_count == 5
    }));
    cleanup(&work_dir);
}

#[test]
fn load_branch_tree_failure_emits_branch_tree_error_event() {
    let work_dir = temp_test_dir("load-branch-tree-failure-work");
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
        FailingSessionStoreLoad::BranchTree,
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
        .handle_runtime_command(RuntimeCommand::LoadBranchTree {
            request_id: request_id(12),
        })
        .expect("branch tree command should be accepted before async load fails");

    let message = wait_for_runtime_event(
        &mut coordinator,
        |event| match event {
            RuntimeEvent::SessionBranchTreeLoadFailed { message, .. } => Some(message),
            RuntimeEvent::Failed { message, .. } => {
                panic!("branch tree load failure must not become global failure: {message}")
            }
            _ => None,
        },
        "branch tree load failure",
    );
    assert!(message.contains("injected branch tree load failure"));
    cleanup(&work_dir);
}

#[test]
fn load_branch_preview_emits_delta_for_requested_branch_without_switching() {
    let work_dir = temp_test_dir("load-branch-preview-work");
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
    let (session_id, inactive_branch_row_id) = store_runtime
        .block_on(async {
            let session_id = store.create_session(header.clone()).await?;
            store
                .append(&session_id, ConversationItem::text(Role::User, "context"))
                .await?;
            store
                .append(
                    &session_id,
                    ConversationItem::text(Role::Assistant, "not shown"),
                )
                .await?;
            let fork_user_id = store
                .append(&session_id, ConversationItem::text(Role::User, "hello"))
                .await?;
            let inactive_branch_row_id = store
                .append(
                    &session_id,
                    ConversationItem::text(Role::Assistant, "branch-b"),
                )
                .await?;
            store.set_leaf(&session_id, Some(&fork_user_id)).await?;
            store
                .append(
                    &session_id,
                    ConversationItem::text(Role::Assistant, "branch-c"),
                )
                .await?;
            store
                .append(
                    &session_id,
                    ConversationItem::text(Role::User, "branch follow-up"),
                )
                .await?;
            Ok::<(SessionId, String), session_store::SessionStoreError>((
                session_id,
                inactive_branch_row_id,
            ))
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
        .handle_runtime_command(RuntimeCommand::LoadBranchPreview {
            request_id: request_id(13),
            branch_row_id: inactive_branch_row_id.clone(),
        })
        .expect("load branch preview should succeed");

    let payload = wait_for_session_tree_preview(&mut coordinator);
    assert_eq!(
        payload
            .rows
            .iter()
            .map(|row| row.preview_content.as_str())
            .collect::<Vec<_>>(),
        vec!["hello", "branch-b"],
        "preview payload should skip visible ancestors before the fork point"
    );
    assert_eq!(
        payload.current_row_id.as_deref(),
        Some(inactive_branch_row_id.as_str())
    );

    coordinator
        .handle_runtime_command(RuntimeCommand::LoadEntryTree {
            request_id: request_id(14),
        })
        .expect("load committed tree should still succeed");
    let committed_payload = wait_for_session_tree(&mut coordinator);
    assert_eq!(
        committed_payload.current_row_id.as_deref(),
        committed_payload
            .rows
            .iter()
            .find(|row| row.preview_content == "branch follow-up")
            .map(|row| row.row_id.as_str()),
        "preview loading must not change the committed leaf"
    );
    cleanup(&work_dir);
}

#[test]
fn load_branch_preview_failure_emits_branch_preview_error_event() {
    let work_dir = temp_test_dir("load-branch-preview-failure-work");
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
    let (session_id, branch_row_id) = store_runtime
        .block_on(async {
            let session_id = inner_store.create_session(header.clone()).await?;
            inner_store
                .append(&session_id, ConversationItem::text(Role::User, "first"))
                .await?;
            let branch_row_id = inner_store
                .append(
                    &session_id,
                    ConversationItem::text(Role::Assistant, "branch"),
                )
                .await?;
            Ok::<(SessionId, String), session_store::SessionStoreError>((session_id, branch_row_id))
        })
        .expect("session fixture should persist");
    let store = Arc::new(FailingSessionStore::new(
        inner_store,
        FailingSessionStoreLoad::BranchPreview,
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
        .handle_runtime_command(RuntimeCommand::LoadBranchPreview {
            request_id: request_id(15),
            branch_row_id,
        })
        .expect("branch preview command should be accepted before async load fails");

    let message = wait_for_runtime_event(
        &mut coordinator,
        |event| match event {
            RuntimeEvent::SessionBranchPreviewLoadFailed { message, .. } => Some(message),
            RuntimeEvent::Failed { message, .. } => {
                panic!("branch preview load failure must not become global failure: {message}")
            }
            _ => None,
        },
        "branch preview load failure",
    );
    assert!(message.contains("injected branch preview load failure"));
    cleanup(&work_dir);
}
