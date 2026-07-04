use super::support::*;

#[test]
fn conversation_worker_persists_config_change_and_flushes_finished_turn() {
    let root = tempdir_path("worker-persistence");
    let work_dir = root.join("workspace");
    fs::create_dir_all(&work_dir).expect("work dir should be creatable");
    let store =
        Arc::new(run_store(LocalSessionStore::open_in(root)).expect("local store should open"));
    let store_trait: Arc<dyn SessionStore> = store.clone();
    let mut conversation =
        ProviderConversation::with_session_store(store_trait, sample_header(&work_dir, "qwen3"))
            .expect("persisted conversation should initialize");
    let user = ConversationItem::text(Role::User, "hello");
    let request = conversation
        .prepare_turn(&runtime_domain::session::ConversationTurnRequest::new(
            "local",
            ProviderKind::OpenAiCompatible,
            "qwen3",
            Some("http://127.0.0.1:1234/v1".to_string()),
            None,
            None,
            user.clone(),
        ))
        .expect("turn should prepare");
    let assistant = ConversationItem::text(Role::Assistant, "hi");
    let (sender, receiver) = mpsc::channel();
    let mut runtime = ConversationWorker {
        receiver: Some(receiver),
        cancellation: Some(CancellationToken::new()),
        target: Some(RuntimeTarget::provider("local", "qwen3")),
        permission_broker: None,
        pending_session_id: None,
        pending_user_entry_id: None,
        session_items: Vec::new(),
        upstream_context_tokens: None,
    };
    let sender_copy = sender.clone();
    let persistence = request.persistence_cloned();
    let cancellation = CancellationToken::new();
    let mut state = SessionPersistenceState::default();
    run_persistence(persist_turn_start(
        persistence.as_ref(),
        &sender_copy,
        &cancellation,
        &mut state,
    ))
    .expect("turn start should persist config and user");
    run_persistence(persist_context_item(
        persistence.as_ref(),
        &sender_copy,
        &cancellation,
        assistant.clone(),
        &mut state,
    ))
    .expect("assistant item should persist");
    sender
        .send(ConversationWorkerEvent::Finished {
            response: ConversationResponse::assistant_text("hi"),
            metrics: None,
            upstream_context_tokens: None,
        })
        .expect("finish event should queue");

    assert!(matches!(
        runtime.try_recv_event(),
        Some(ConversationEvent::Finished { .. })
    ));

    let metas = run_store(store.list_sessions(
        &ProjectDir::from_work_dir(&work_dir),
        SessionListOptions::default(),
    ))
    .expect("session meta should list");
    assert_eq!(metas.len(), 1);
    let resolved = run_store(store.resolve(&metas[0].session_id, None))
        .expect("resolved items should be readable");
    let jsonl = fs::read_to_string(&metas[0].jsonl_path).expect("jsonl should be readable");

    assert_eq!(resolved, vec![user, assistant]);
    assert!(jsonl.contains("\"type\":\"config_change\""));
}

#[test]
fn conversation_worker_persists_user_turn_when_request_fails_before_streaming() {
    let root = tempdir_path("worker-pre-stream-failure-persistence");
    let work_dir = root.join("workspace");
    fs::create_dir_all(&work_dir).expect("work dir should be creatable");
    let store =
        Arc::new(run_store(LocalSessionStore::open_in(root)).expect("local store should open"));
    let store_trait: Arc<dyn SessionStore> = store.clone();
    let mut conversation = ProviderConversation::with_session_store(
        store_trait,
        sample_header(&work_dir, "gpt-5-mini"),
    )
    .expect("persisted conversation should initialize");
    let user = ConversationItem::text(Role::User, "please persist even if provider setup fails");
    let request = conversation
        .prepare_turn(&runtime_domain::session::ConversationTurnRequest::new(
            "openai",
            ProviderKind::OpenAi,
            "gpt-5-mini",
            None,
            None,
            None,
            user.clone(),
        ))
        .expect("turn should prepare");
    let (sender, receiver) = mpsc::channel();

    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("test runtime should build")
        .block_on(run_conversation_worker(
            request,
            ToolExecutorRegistry::new(),
            RuntimeRequestPolicy::default(),
            CancellationToken::new(),
            ConversationPermissionBroker::default(),
            sender,
        ));

    let events = receiver.try_iter().collect::<Vec<_>>();
    assert!(events.iter().any(|event| {
        matches!(
            event,
            ConversationWorkerEvent::Progress(ConversationEvent::Failed { message })
                if message.contains("requires API key")
        )
    }));

    let metas = run_store(store.list_sessions(
        &ProjectDir::from_work_dir(&work_dir),
        SessionListOptions::default(),
    ))
    .expect("session meta should list");
    assert_eq!(metas.len(), 1);
    let resolved = run_store(store.resolve(&metas[0].session_id, None))
        .expect("resolved items should be readable");
    let tree = run_store(store.load_session_tree(&metas[0].session_id))
        .expect("session tree should be readable");

    assert_eq!(resolved, vec![user]);
    assert_eq!(tree.rows.len(), 1);
    assert_eq!(
        tree.rows[0].preview_content,
        "please persist even if provider setup fails"
    );
}

#[test]
fn flush_session_persistence_preserves_store_error_source() {
    let root = tempdir_path("worker-flush-error-source");
    let work_dir = root.join("workspace");
    fs::create_dir_all(&work_dir).expect("work dir should be creatable");
    let store =
        Arc::new(run_store(LocalSessionStore::open_in(root)).expect("local store should open"));
    let missing_session_id = SessionId::new();
    let store_trait: Arc<dyn SessionStore> = store;
    let mut conversation =
        ProviderConversation::with_session_store(store_trait, sample_header(&work_dir, "qwen3"))
            .expect("persisted conversation should initialize");
    conversation.set_session_id(missing_session_id.clone());
    let request = conversation
        .prepare_turn(&runtime_domain::session::ConversationTurnRequest::new(
            "local",
            ProviderKind::OpenAiCompatible,
            "qwen3",
            Some("http://127.0.0.1:1234/v1".to_string()),
            None,
            None,
            ConversationItem::text(Role::User, "hello"),
        ))
        .expect("turn should prepare");

    let error = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("test runtime should build")
        .block_on(async {
            let (command_sender, command_receiver) = tokio_mpsc::unbounded_channel();
            let (event_sender, _event_receiver) = mpsc::channel();
            let actor = tokio::spawn(run_session_persistence_actor(
                request.persistence_cloned(),
                command_receiver,
                event_sender,
                CancellationToken::new(),
            ));
            let error = flush_session_persistence(&command_sender)
                .await
                .expect_err("flush failure should preserve typed source");
            drop(command_sender);
            actor.await.expect("persistence actor should stop cleanly");
            error
        });

    assert!(matches!(
        error.as_ref(),
        SessionPersistenceError::Flush {
            source: SessionStoreError::SessionNotFound { session_id }
        } if session_id == &missing_session_id
    ));
}

#[test]
fn persistence_helpers_store_rich_tool_replay_without_duplicate_tool_result() {
    let root = tempdir_path("worker-tool-replay-persistence");
    let work_dir = root.join("workspace");
    fs::create_dir_all(&work_dir).expect("work dir should be creatable");
    let store =
        Arc::new(run_store(LocalSessionStore::open_in(root)).expect("local store should open"));
    let store_trait: Arc<dyn SessionStore> = store.clone();
    let mut conversation =
        ProviderConversation::with_session_store(store_trait, sample_header(&work_dir, "qwen3"))
            .expect("persisted conversation should initialize");
    let request = conversation
        .prepare_turn(&runtime_domain::session::ConversationTurnRequest::new(
            "local",
            ProviderKind::OpenAiCompatible,
            "qwen3",
            Some("http://127.0.0.1:1234/v1".to_string()),
            None,
            None,
            ConversationItem::text(Role::User, "edit file"),
        ))
        .expect("turn should prepare");
    let (sender, _receiver) = mpsc::channel();
    let persistence = request.persistence_cloned();
    let cancellation = CancellationToken::new();
    let mut state = SessionPersistenceState::default();
    let started_activity = RuntimeToolActivity {
        activity_id: "call-1".to_string(),
        title: "Write src/lib.rs".to_string(),
        kind: RuntimeToolKind::Write,
        status: RuntimeToolActivityStatus::InProgress,
        content: vec![RuntimeToolActivityContent::Text("src/lib.rs".to_string())],
        locations: Vec::new(),
        raw_input: Some(RuntimeToolActivityRawValue::from(
            r#"{"path":"src/lib.rs"}"#,
        )),
        raw_output: None,
    };
    let final_update = RuntimeToolActivityUpdate {
        activity_id: "call-1".to_string(),
        title: Some("Write src/lib.rs".to_string()),
        kind: Some(RuntimeToolKind::Write),
        status: Some(RuntimeToolActivityStatus::Completed),
        content: Some(vec![RuntimeToolActivityContent::Diff {
            path: "src/lib.rs".to_string(),
            old_text: Some("old".to_string()),
            new_text: "new".to_string(),
            is_truncated: false,
        }]),
        locations: Some(Vec::new()),
        raw_input: Some(RuntimeToolActivityRawValue::from(
            r#"{"path":"src/lib.rs"}"#,
        )),
        raw_output: Some(RuntimeToolActivityRawValue::tool_result(
            "plain provider output",
            None,
        )),
    };
    let terminal_snapshot = RuntimeTerminalSnapshot {
        terminal_id: "call-1".to_string(),
        command: Some("write src/lib.rs".to_string()),
        cwd: Some(work_dir.display().to_string()),
        output: "terminal output".to_string(),
        truncated: false,
        exit_status: None,
        released: true,
    };

    run_persistence(persist_turn_start(
        persistence.as_ref(),
        &sender,
        &cancellation,
        &mut state,
    ))
    .expect("turn start should persist");
    run_persistence(persist_tool_activity_started(
        persistence.as_ref(),
        started_activity,
        &mut state,
    ))
    .expect("started activity should persist");
    run_persistence(persist_tool_activity_update(
        persistence.as_ref(),
        final_update,
        &mut state,
    ))
    .expect("final activity should persist");
    run_persistence(persist_terminal_snapshot(
        persistence.as_ref(),
        terminal_snapshot.clone(),
        &state,
    ))
    .expect("terminal snapshot should persist");
    run_persistence(persist_context_item(
        persistence.as_ref(),
        &sender,
        &cancellation,
        ConversationItem::tool_result(
            "call-1",
            vec![ContentBlock::Text("plain provider output".to_string())],
            false,
        ),
        &mut state,
    ))
    .expect("tool result item should persist");

    let meta = run_store(store.list_sessions(
        &ProjectDir::from_work_dir(&work_dir),
        SessionListOptions::default(),
    ))
    .expect("session meta should list")
    .into_iter()
    .next()
    .expect("session should exist");
    let restored =
        run_store(store.load_session(&meta.session_id, None)).expect("session should load");

    assert_eq!(restored.transcript.len(), 3);
    assert!(matches!(
        &restored.transcript[0],
        TranscriptReplayItem::Message {
            role: runtime_domain::session::TranscriptReplayRole::User,
            content,
        } if content == "edit file"
    ));
    assert!(matches!(
        &restored.transcript[1],
        TranscriptReplayItem::ToolActivity { activity }
            if activity.activity_id == "call-1"
                && matches!(
                    activity.content.as_slice(),
                    [RuntimeToolActivityContent::Diff { path, old_text, new_text, is_truncated }]
                        if path == "src/lib.rs"
                            && old_text.as_deref() == Some("old")
                            && new_text == "new"
                            && !is_truncated
                )
    ));
    assert_eq!(
        restored.transcript[2],
        TranscriptReplayItem::TerminalSnapshot {
            snapshot: terminal_snapshot
        }
    );
}

#[test]
fn persist_turn_start_keeps_provider_message_in_items_and_transcript_projection_in_replay() {
    let root = tempdir_path("worker-transcript-projection");
    let work_dir = root.join("workspace");
    fs::create_dir_all(&work_dir).expect("work dir should be creatable");
    let store =
        Arc::new(run_store(LocalSessionStore::open_in(root)).expect("local store should open"));
    let store_trait: Arc<dyn SessionStore> = store.clone();
    let mut conversation =
        ProviderConversation::with_session_store(store_trait, sample_header(&work_dir, "qwen3"))
            .expect("persisted conversation should initialize");
    let provider_user = ConversationItem::text(
        Role::User,
        "<skill>\n<name>code-review</name>\nbody\n</skill>\n\nraw user message",
    );
    let transcript_user = runtime_domain::session::TranscriptUserMessage {
        content: "$code-review raw user message".to_string(),
        attachments: Vec::new(),
        skill_bindings: vec![runtime_domain::session::TranscriptSkillBinding {
            skill_name: "code-review".to_string(),
            origin: runtime_domain::prompt_assembly::PromptSourceOrigin::Project,
            skill_path: "/tmp/code-review/SKILL.md".to_string(),
            start_char: 0,
            end_char: 12,
        }],
        custom_prompt_bindings: Vec::new(),
    };
    let request = conversation
        .prepare_turn_with_transcript(
            &runtime_domain::session::ConversationTurnRequest::new(
                "local",
                ProviderKind::OpenAiCompatible,
                "qwen3",
                Some("http://127.0.0.1:1234/v1".to_string()),
                None,
                None,
                provider_user.clone(),
            ),
            Some(transcript_user),
            vec![TranscriptReplayItem::ToolActivity {
                activity: RuntimeToolActivity {
                    activity_id: "manual-skill-1-code-review".to_string(),
                    title: "Read /tmp/code-review/SKILL.md".to_string(),
                    kind: RuntimeToolKind::Read,
                    status: RuntimeToolActivityStatus::Completed,
                    content: Vec::new(),
                    locations: Vec::new(),
                    raw_input: Some(RuntimeToolActivityRawValue::from(serde_json::json!({
                        "path": "/tmp/code-review/SKILL.md",
                        "hunea_skill_name": "code-review",
                    }))),
                    raw_output: None,
                },
            }],
        )
        .expect("turn should prepare");
    let (sender, _receiver) = mpsc::channel();
    let persistence = request.persistence_cloned();
    let cancellation = CancellationToken::new();
    let mut state = SessionPersistenceState::default();

    run_persistence(persist_turn_start(
        persistence.as_ref(),
        &sender,
        &cancellation,
        &mut state,
    ))
    .expect("turn start should persist");

    let meta = run_store(store.list_sessions(
        &ProjectDir::from_work_dir(&work_dir),
        SessionListOptions::default(),
    ))
    .expect("session meta should list")
    .into_iter()
    .next()
    .expect("session should exist");
    let restored =
        run_store(store.load_session(&meta.session_id, None)).expect("session should load");

    assert_eq!(
        restored
            .items
            .iter()
            .map(|item| item.item.clone())
            .collect::<Vec<_>>(),
        vec![provider_user]
    );
    assert!(matches!(
        restored.transcript.as_slice(),
        [
            TranscriptReplayItem::BoundUserMessage { message },
            TranscriptReplayItem::ToolActivity { activity }
        ] if message.content == "$code-review raw user message"
            && message.skill_bindings.len() == 1
            && message.skill_bindings[0].skill_name == "code-review"
            && activity.activity_id == "manual-skill-1-code-review"
    ));
}

#[test]
fn persist_turn_start_replays_image_only_user_message_as_bound_message() {
    let root = tempdir_path("worker-image-only-user-replay");
    let work_dir = root.join("workspace");
    fs::create_dir_all(&work_dir).expect("work dir should be creatable");
    let store =
        Arc::new(run_store(LocalSessionStore::open_in(root)).expect("local store should open"));
    let store_trait: Arc<dyn SessionStore> = store.clone();
    let mut conversation =
        ProviderConversation::with_session_store(store_trait, sample_header(&work_dir, "gpt-4o"))
            .expect("persisted conversation should initialize");
    let transcript_user = runtime_domain::session::TranscriptUserMessage {
        content: String::new(),
        attachments: vec![runtime_domain::session::TranscriptUserAttachment::Image {
            data_base64: "iVBORw0KGgo=".to_string(),
            mime_type: "image/png".to_string(),
            uri: Some("assets/a.png".to_string()),
            detail: None,
        }],
        skill_bindings: Vec::new(),
        custom_prompt_bindings: Vec::new(),
    };
    let request = conversation
        .prepare_turn(
            &runtime_domain::session::ConversationTurnRequest::new_user_source_message(
                "local",
                ProviderKind::OpenAiCompatible,
                "gpt-4o",
                Some("http://127.0.0.1:1234/v1".to_string()),
                None,
                None,
                transcript_user.clone(),
            ),
        )
        .expect("turn should prepare");
    let (sender, _receiver) = mpsc::channel();
    let persistence = request.persistence_cloned();
    let cancellation = CancellationToken::new();
    let mut state = SessionPersistenceState::default();

    run_persistence(persist_turn_start(
        persistence.as_ref(),
        &sender,
        &cancellation,
        &mut state,
    ))
    .expect("turn start should persist");

    let meta = run_store(store.list_sessions(
        &ProjectDir::from_work_dir(&work_dir),
        SessionListOptions::default(),
    ))
    .expect("session meta should list")
    .into_iter()
    .next()
    .expect("session should exist");
    let restored =
        run_store(store.load_session(&meta.session_id, None)).expect("session should load");

    assert!(matches!(
        restored.transcript.as_slice(),
        [TranscriptReplayItem::BoundUserMessage { message }]
            if message == &transcript_user
    ));
}

#[test]
fn persist_context_item_replays_image_only_tool_result_with_visible_summary() {
    let root = tempdir_path("worker-image-only-tool-result-replay");
    let work_dir = root.join("workspace");
    fs::create_dir_all(&work_dir).expect("work dir should be creatable");
    let store =
        Arc::new(run_store(LocalSessionStore::open_in(root)).expect("local store should open"));
    let store_trait: Arc<dyn SessionStore> = store.clone();
    let mut conversation =
        ProviderConversation::with_session_store(store_trait, sample_header(&work_dir, "gpt-4o"))
            .expect("persisted conversation should initialize");
    let user = ConversationItem::text(Role::User, "inspect image");
    let request = conversation
        .prepare_turn(&runtime_domain::session::ConversationTurnRequest::new(
            "local",
            ProviderKind::OpenAiCompatible,
            "gpt-4o",
            Some("http://127.0.0.1:1234/v1".to_string()),
            None,
            None,
            user,
        ))
        .expect("turn should prepare");
    let tool_result = ConversationItem::tool_result(
        "call-1",
        vec![ContentBlock::Image {
            data_base64: "iVBORw0KGgo=".to_string(),
            mime_type: "image/png".to_string(),
            uri: Some("assets/a.png".to_string()),
            detail: None,
        }],
        false,
    );
    let (sender, _receiver) = mpsc::channel();
    let persistence = request.persistence_cloned();
    let cancellation = CancellationToken::new();
    let mut state = SessionPersistenceState::default();

    run_persistence(persist_turn_start(
        persistence.as_ref(),
        &sender,
        &cancellation,
        &mut state,
    ))
    .expect("turn start should persist");
    run_persistence(persist_context_item(
        persistence.as_ref(),
        &sender,
        &cancellation,
        tool_result,
        &mut state,
    ))
    .expect("tool result should persist");

    let meta = run_store(store.list_sessions(
        &ProjectDir::from_work_dir(&work_dir),
        SessionListOptions::default(),
    ))
    .expect("session meta should list")
    .into_iter()
    .next()
    .expect("session should exist");
    let restored =
        run_store(store.load_session(&meta.session_id, None)).expect("session should load");

    assert!(matches!(
        restored.transcript.as_slice(),
        [
            TranscriptReplayItem::Message { .. },
            TranscriptReplayItem::ToolResult { content }
        ] if content.contains("Attached image")
            && content.contains("image/png")
            && content.contains("assets/a.png")
    ));
}
