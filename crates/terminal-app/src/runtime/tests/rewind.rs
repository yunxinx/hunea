use super::support::*;

#[test]
fn select_entry_rewind_rebuilds_provider_history_to_selected_entry() {
    let work_dir = temp_test_dir("select-entry-rewind-work");
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
    let (session_id, assistant_replay_entry_id) = store_runtime
        .block_on(async {
            let session_id = store.create_session(header.clone()).await?;
            store
                .append(&session_id, ConversationItem::text(Role::User, "first"))
                .await?;
            store
                .append_transcript_replay(
                    &session_id,
                    TranscriptReplayItem::Message {
                        role: TranscriptReplayRole::User,
                        content: "first".to_string(),
                    },
                )
                .await?;
            store
                .append(
                    &session_id,
                    ConversationItem::text(Role::Assistant, "answer"),
                )
                .await?;
            let assistant_replay_entry_id = store
                .append_transcript_replay(
                    &session_id,
                    TranscriptReplayItem::Message {
                        role: TranscriptReplayRole::Assistant,
                        content: "answer".to_string(),
                    },
                )
                .await?;
            store
                .append(&session_id, ConversationItem::text(Role::User, "second"))
                .await?;
            store
                .append_transcript_replay(
                    &session_id,
                    TranscriptReplayItem::Message {
                        role: TranscriptReplayRole::User,
                        content: "second".to_string(),
                    },
                )
                .await?;
            Ok::<(SessionId, String), session_store::SessionStoreError>((
                session_id,
                assistant_replay_entry_id,
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
        .handle_runtime_command(RuntimeCommand::LoadEntryTree)
        .expect("load entry tree should succeed");
    let payload = wait_for_session_tree(&mut coordinator);
    let assistant_row = payload
        .rows
        .iter()
        .find(|row| row.preview_content == "answer")
        .expect("assistant row should be present");
    assert_eq!(
        assistant_row.rewind_target_id.as_deref(),
        Some(assistant_replay_entry_id.as_str()),
        "visible assistant row should rewind through hidden transcript replay"
    );

    coordinator
        .handle_runtime_command(RuntimeCommand::SelectEntryRewind {
            entry_id: assistant_row.row_id.clone(),
        })
        .expect("select entry rewind should succeed");

    let events = wait_for_runtime_events(&mut coordinator, "entry rewind events");
    assert_eq!(
        coordinator
            .provider_conversation
            .history()
            .map(ConversationItem::text_content)
            .collect::<Vec<_>>(),
        vec!["first", "answer"]
    );
    let Some(RuntimeEvent::SessionResumed { payload }) = events.into_iter().next() else {
        panic!("expected resumed payload after entry rewind");
    };
    assert_eq!(
        payload
            .transcript
            .iter()
            .map(TranscriptReplayItem::content_text)
            .collect::<Vec<_>>(),
        vec!["first", "answer"]
    );
    cleanup(&work_dir);
}

#[test]
fn select_entry_rewind_ignores_reasoning_without_restore_target() {
    let work_dir = temp_test_dir("select-entry-rewind-reasoning-work");
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
                    ConversationItem::Reasoning {
                        content: "thinking".to_string(),
                        summary: None,
                        encrypted: None,
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
    wait_for_session_resumed(&mut coordinator);

    coordinator
        .handle_runtime_command(RuntimeCommand::LoadEntryTree)
        .expect("load entry tree should succeed");
    let payload = wait_for_session_tree(&mut coordinator);
    let reasoning_row = payload
        .rows
        .iter()
        .find(|row| row.kind == SessionTreeRowKind::Reasoning)
        .expect("reasoning row should be present");
    assert_eq!(reasoning_row.rewind_target_id, None);

    coordinator
        .handle_runtime_command(RuntimeCommand::SelectEntryRewind {
            entry_id: reasoning_row.row_id.clone(),
        })
        .expect("non-rewindable reasoning should be accepted as a no-op");
    wait_for_runtime_idle(&mut coordinator);

    let expected_history = [
        ConversationItem::text(Role::User, "first"),
        ConversationItem::Reasoning {
            content: "thinking".to_string(),
            summary: None,
            encrypted: None,
        },
    ];
    assert!(
        coordinator
            .provider_conversation
            .history()
            .eq(expected_history.iter())
    );
    assert_no_runtime_events(
        &mut coordinator,
        "non-rewindable reasoning should not emit a resumed payload",
    );
    cleanup(&work_dir);
}
