use super::support::*;

#[test]
fn switch_branch_moves_leaf_and_rebuilds_transcript_and_tree() {
    let work_dir = temp_test_dir("switch-branch-work");
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
            let root_user_id = store
                .append(&session_id, ConversationItem::text(Role::User, "hello"))
                .await?;
            store
                .append_transcript_replay(
                    &session_id,
                    TranscriptReplayItem::Message {
                        role: TranscriptReplayRole::User,
                        content: "hello".to_string(),
                    },
                )
                .await?;
            store
                .append(
                    &session_id,
                    ConversationItem::text(Role::Assistant, "branch-b"),
                )
                .await?;
            store
                .append_transcript_replay(
                    &session_id,
                    TranscriptReplayItem::Message {
                        role: TranscriptReplayRole::Assistant,
                        content: "branch-b".to_string(),
                    },
                )
                .await?;
            store.set_leaf(&session_id, Some(&root_user_id)).await?;
            store
                .append(
                    &session_id,
                    ConversationItem::text(Role::Assistant, "branch-c"),
                )
                .await?;
            store
                .append_transcript_replay(
                    &session_id,
                    TranscriptReplayItem::Message {
                        role: TranscriptReplayRole::Assistant,
                        content: "branch-c".to_string(),
                    },
                )
                .await?;
            store
                .append(
                    &session_id,
                    ConversationItem::text(Role::User, "branch follow-up"),
                )
                .await?;
            store
                .append_transcript_replay(
                    &session_id,
                    TranscriptReplayItem::Message {
                        role: TranscriptReplayRole::User,
                        content: "branch follow-up".to_string(),
                    },
                )
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
    wait_for_runtime_event(
        &mut coordinator,
        |event| match event {
            RuntimeEvent::SessionResumed { .. } => Some(()),
            _ => None,
        },
        "session resume event",
    );
    coordinator
        .handle_runtime_command(RuntimeCommand::LoadEntryTree {
            request_id: request_id(20),
        })
        .expect("load entry tree should succeed");
    let current_tree = wait_for_runtime_event(
        &mut coordinator,
        |event| match event {
            RuntimeEvent::SessionTreeLoaded { payload, .. } => Some(payload),
            _ => None,
        },
        "session tree payload",
    );
    let branch_choice = current_tree
        .rows
        .iter()
        .find(|row| row.preview_content == "hello")
        .and_then(|row| {
            row.branch_choices
                .iter()
                .find(|branch| branch.branch.display_summary == "branch-b")
        })
        .cloned()
        .expect("inactive branch choice should exist");
    let branch_row_id = branch_choice.branch.branch_row_id;
    let branch_leaf_id = branch_choice.branch.subtree_leaf_id;
    coordinator
        .handle_runtime_command(RuntimeCommand::LoadBranchPreview {
            request_id: request_id(21),
            branch_row_id,
        })
        .expect("branch preview should load");
    let preview_rows = wait_for_session_tree_preview(&mut coordinator)
        .rows
        .into_iter()
        .map(|row| row.preview_content)
        .collect::<Vec<_>>();

    coordinator
        .handle_runtime_command(RuntimeCommand::SwitchBranch {
            request_id: request_id(22),
            leaf_id: branch_leaf_id,
        })
        .expect("switch branch should succeed");

    let events = wait_for_runtime_events(&mut coordinator, "branch switch events");
    assert_eq!(
        coordinator
            .provider_conversation
            .history()
            .map(ConversationItem::text_content)
            .collect::<Vec<_>>(),
        vec!["hello", "branch-b"],
        "provider history should move after the switch event is applied"
    );
    let resumed_payload = events
        .iter()
        .find_map(|event| match event {
            RuntimeEvent::SessionResumed { payload } => Some(payload),
            _ => None,
        })
        .expect("switch should emit a transcript rebuild event");
    assert_eq!(
        resumed_payload
            .transcript
            .iter()
            .map(TranscriptReplayItem::content_text)
            .collect::<Vec<_>>(),
        vec!["hello", "branch-b"]
    );
    let tree_payload = events
        .into_iter()
        .find_map(|event| match event {
            RuntimeEvent::SessionTreeLoaded { payload, .. } => Some(payload),
            _ => None,
        })
        .expect("switch should refresh the committed path tree");
    assert_eq!(
        preview_rows,
        vec!["hello".to_string(), "branch-b".to_string()]
    );
    assert_eq!(
        tree_payload
            .rows
            .iter()
            .map(|row| row.preview_content.as_str())
            .collect::<Vec<_>>(),
        vec!["hello", "branch-b"],
        "switch should still refresh the committed full path tree"
    );
    assert_eq!(
        tree_payload.current_row_id.as_deref(),
        tree_payload
            .rows
            .iter()
            .find(|row| row.preview_content == "branch-b")
            .map(|row| row.row_id.as_str())
    );
    cleanup(&work_dir);
}

#[test]
fn switch_branch_is_blocked_while_provider_turn_is_running() {
    let work_dir = temp_test_dir("switch-branch-active-turn-work");
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
    let (session_id, inactive_branch_leaf_id) = store_runtime
        .block_on(async {
            let session_id = store.create_session(header.clone()).await?;
            let root_user_id = store
                .append(&session_id, ConversationItem::text(Role::User, "hello"))
                .await?;
            let inactive_branch_leaf_id = store
                .append(
                    &session_id,
                    ConversationItem::text(Role::Assistant, "branch-b"),
                )
                .await?;
            store.set_leaf(&session_id, Some(&root_user_id)).await?;
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
                inactive_branch_leaf_id,
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
    let request = ConversationTurnRequest::new_user_text(
        "local",
        ProviderKind::OpenAiCompatible,
        "qwen3",
        Some("http://127.0.0.1:9/v1".to_string()),
        None,
        None,
        "pending user",
    );
    let target = request.target();
    coordinator
        .handle_runtime_command(RuntimeCommand::SubmitConversationTurn { target, request })
        .expect("conversation should start");

    let error = coordinator
        .handle_runtime_command(RuntimeCommand::SwitchBranch {
            request_id: request_id(23),
            leaf_id: inactive_branch_leaf_id,
        })
        .expect_err("switch branch should be rejected while provider is running");

    assert_eq!(error, "Cannot switch branch while a request is running");
    coordinator
        .handle_runtime_command(RuntimeCommand::Interrupt { target: None })
        .expect("test conversation should interrupt cleanly");
    cleanup(&work_dir);
}

#[test]
fn switch_branch_failure_keeps_committed_leaf_unchanged() {
    let work_dir = temp_test_dir("switch-branch-invalid-leaf-work");
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
            let root_user_id = store
                .append(&session_id, ConversationItem::text(Role::User, "hello"))
                .await?;
            store
                .append(
                    &session_id,
                    ConversationItem::text(Role::Assistant, "branch-b"),
                )
                .await?;
            store.set_leaf(&session_id, Some(&root_user_id)).await?;
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
            request_id: request_id(24),
        })
        .expect("load entry tree should succeed");
    let before_rows = wait_for_session_tree(&mut coordinator)
        .rows
        .into_iter()
        .map(|row| row.preview_content)
        .collect::<Vec<_>>();

    let receipt = coordinator
        .handle_runtime_command(RuntimeCommand::SwitchBranch {
            request_id: request_id(25),
            leaf_id: "missing-leaf".to_string(),
        })
        .expect("invalid leaf switch should be accepted for async execution");
    assert_eq!(receipt, RuntimeCommandReceipt::Accepted);

    let error = wait_for_runtime_event(
        &mut coordinator,
        |event| match event {
            RuntimeEvent::SessionBranchSwitchFailed {
                request_id: actual_request_id,
                message,
            } if actual_request_id == request_id(25) => Some(message),
            _ => None,
        },
        "invalid leaf failure",
    );
    assert!(
        error.contains("missing-leaf"),
        "failure should include the missing leaf id: {error}"
    );
    coordinator
        .handle_runtime_command(RuntimeCommand::LoadEntryTree {
            request_id: request_id(26),
        })
        .expect("load entry tree should still succeed");
    let after_rows = wait_for_session_tree(&mut coordinator)
        .rows
        .into_iter()
        .map(|row| row.preview_content)
        .collect::<Vec<_>>();
    assert_eq!(
        after_rows, before_rows,
        "failed switch must leave the committed path unchanged"
    );
    cleanup(&work_dir);
}

#[test]
fn switch_branch_uses_prepared_leaf_restore_instead_of_committed_reload() {
    let work_dir = temp_test_dir("switch-branch-prepared-restore-work");
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
    let (session_id, inactive_branch_leaf_id) = store_runtime
        .block_on(async {
            let session_id = inner_store.create_session(header.clone()).await?;
            let root_user_id = inner_store
                .append(&session_id, ConversationItem::text(Role::User, "hello"))
                .await?;
            let inactive_branch_leaf_id = inner_store
                .append(
                    &session_id,
                    ConversationItem::text(Role::Assistant, "branch-b"),
                )
                .await?;
            inner_store
                .set_leaf(&session_id, Some(&root_user_id))
                .await?;
            inner_store
                .append(
                    &session_id,
                    ConversationItem::text(Role::Assistant, "branch-c"),
                )
                .await?;
            inner_store
                .append(
                    &session_id,
                    ConversationItem::text(Role::User, "branch follow-up"),
                )
                .await?;
            Ok::<(SessionId, String), session_store::SessionStoreError>((
                session_id,
                inactive_branch_leaf_id,
            ))
        })
        .expect("session fixture should persist");
    let failing_store = Arc::new(CommittedLoadFailsAfterSetLeafStore::new(inner_store));
    let mut coordinator = runtime_coordinator(AppRuntimeOptions {
        session_store: Some(failing_store),
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
            request_id: request_id(27),
        })
        .expect("load entry tree should succeed");
    let before_rows = wait_for_session_tree(&mut coordinator)
        .rows
        .into_iter()
        .map(|row| row.preview_content)
        .collect::<Vec<_>>();
    coordinator
        .handle_runtime_command(RuntimeCommand::SwitchBranch {
            request_id: request_id(28),
            leaf_id: inactive_branch_leaf_id,
        })
        .expect("switch should not reload from the committed leaf after set_leaf");
    let after_rows = wait_for_runtime_event(
        &mut coordinator,
        |event| match event {
            RuntimeEvent::SessionTreeLoaded { payload, .. } => Some(
                payload
                    .rows
                    .into_iter()
                    .map(|row| row.preview_content)
                    .collect::<Vec<_>>(),
            ),
            _ => None,
        },
        "switch tree payload",
    );
    assert_eq!(before_rows, vec!["hello", "branch-c", "branch follow-up"]);
    assert_eq!(after_rows, vec!["hello", "branch-b"]);
    cleanup(&work_dir);
}
