use super::support::*;
use std::sync::Mutex;

use extension_hook_runtime::{
    AfterToolResultDecision, AfterToolResultPayload, HookRegistrationOptions,
};
use tool_runtime::{
    Tool, ToolCall as RuntimeToolCall, ToolDefinition, ToolExecutionFuture, ToolKind,
    ToolPermissionPolicy, ToolResult,
};

#[test]
fn conversation_worker_persists_config_change_and_flushes_finished_turn() {
    let root = tempdir_path("worker-persistence");
    let work_dir = root.join("workspace");
    fs::create_dir_all(&work_dir).expect("work dir should be creatable");
    let store =
        Arc::new(run_store(LocalSessionStore::open_in(root)).expect("local store should open"));
    let store_trait: Arc<dyn SessionStore> = store.clone();
    let mut conversation =
        ProviderConversation::with_session_port(store_trait, sample_header(&work_dir, "qwen3"))
            .expect("persisted conversation should initialize");
    let user = ConversationItem::text(Role::User, "hello");
    let request = conversation
        .prepare_turn(&runtime_domain::session::ConversationTurnRequest::new(
            "local",
            "qwen3",
            user.clone(),
        ))
        .expect("turn should prepare");
    let assistant = ConversationItem::text(Role::Assistant, "hi");
    let (sender, receiver) = conversation_worker_event_channel();
    let mut runtime = ConversationWorker {
        receiver: Some(receiver),
        worker_thread: None,
        cancellation: Some(CancellationToken::new()),
        target: Some(RuntimeTarget::provider("local", "qwen3")),
        pending_session_id: None,
        pending_user_entry_id: None,
        session_items: Vec::new(),
        upstream_context_tokens: None,
        event_notifier: RuntimeEventNotifier::default(),
    };
    let sender_copy = sender.clone();
    let persistence = request.persistence_cloned();
    let mut state = SessionPersistenceState::default();
    run_persistence(persist_turn_start(
        persistence.as_ref(),
        &sender_copy,
        &mut state,
    ))
    .expect("turn start should persist config and user");
    run_persistence(persist_context_item(
        persistence.as_ref(),
        &sender_copy,
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
fn conversation_worker_persists_only_the_transformed_tool_result() {
    const RAW_RESULT: &str = "raw-tool-result-secret";
    const TRANSFORMED_RESULT: &str = "transformed-tool-result";

    let root = tempdir_path("worker-transformed-tool-result");
    let work_dir = root.join("workspace");
    fs::create_dir_all(&work_dir).expect("work dir should be creatable");
    let store =
        Arc::new(run_store(LocalSessionStore::open_in(root)).expect("local store should open"));
    let store_trait: Arc<dyn SessionStore> = store.clone();
    let mut conversation =
        ProviderConversation::with_session_port(store_trait, sample_header(&work_dir, "qwen3"))
            .expect("persisted conversation should initialize");
    let user = ConversationItem::text(Role::User, "run echo");
    let request = conversation
        .prepare_turn(&runtime_domain::session::ConversationTurnRequest::new(
            "local",
            "qwen3",
            user.clone(),
        ))
        .expect("turn should prepare");
    let mut executor = ToolExecutorRegistry::new();
    executor.insert(RawPersistenceTool);
    let hooks = ExtensionHookRegistry::new();
    let _registration = hooks
        .register_after_tool_result(
            HookOwnerId::try_new("persistence-owner").expect("owner id should validate"),
            HookId::try_new("replace-result").expect("hook id should validate"),
            HookRegistrationOptions::try_new(HookPriority::default(), Duration::from_secs(1))
                .expect("hook options should validate"),
            Arc::new(|payload: AfterToolResultPayload, _| async move {
                let replacement =
                    ToolResult::success(payload.result().call_id().to_string(), TRANSFORMED_RESULT);
                Ok(AfterToolResultDecision::Continue(
                    payload
                        .replace_result(replacement)
                        .expect("call identity should remain stable"),
                ))
            }),
        )
        .expect("result hook should register");
    let provider = Arc::new(ToolResultPersistenceProvider {
        calls: Mutex::new(0),
    });
    let provider_lease = ProviderClientLease::new(
        "local",
        ProviderKind::OpenAiCompatible,
        provider,
        ProviderPromptCachePolicy::Disabled,
    );
    let (sender, receiver) = conversation_worker_event_channel();

    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("test runtime should build")
        .block_on(run_conversation_worker(
            request,
            provider_lease,
            executor,
            CancellationToken::new(),
            conversation_worker_options(RuntimeRequestPolicy::default(), hooks),
            sender,
        ));

    let events = receiver.try_iter().collect::<Vec<_>>();
    assert!(matches!(
        events.last(),
        Some(ConversationWorkerEvent::Finished { .. })
    ));
    let metas = run_store(store.list_sessions(
        &ProjectDir::from_work_dir(&work_dir),
        SessionListOptions::default(),
    ))
    .expect("session meta should list");
    assert_eq!(metas.len(), 1);
    let resolved = run_store(store.resolve(&metas[0].session_id, None))
        .expect("resolved items should be readable");
    let persisted_tool_result = resolved
        .iter()
        .find(|item| matches!(item, ConversationItem::ToolResult { .. }))
        .expect("transformed tool result should persist");

    assert_eq!(persisted_tool_result.text_content(), TRANSFORMED_RESULT);
    assert!(!format!("{resolved:?}").contains(RAW_RESULT));
    assert_eq!(resolved.first(), Some(&user));
}

struct ToolResultPersistenceProvider {
    calls: Mutex<usize>,
}

impl ProviderClient for ToolResultPersistenceProvider {
    fn stream_prompt<'a>(
        &'a self,
        request: &'a PromptRequest,
        _sink: &'a mut (dyn StreamEventSink + Send),
    ) -> ProviderFuture<'a, Result<PromptCompletion, ProviderError>> {
        Box::pin(async move {
            let mut calls = self.calls.lock().expect("provider lock should not poison");
            *calls += 1;
            if *calls == 1 {
                return Ok(PromptCompletion::new(
                    vec![ConversationItem::assistant_with_tool_calls(
                        "checking".to_string(),
                        vec![ToolCall::new("call-1", "echo", r#"{"text":"hello"}"#)],
                    )],
                    provider_protocol::FinishReason::ToolCalls,
                    None,
                ));
            }

            let tool_result = request
                .items
                .iter()
                .find(|item| matches!(item, ConversationItem::ToolResult { .. }))
                .expect("second provider request should contain a tool result");
            assert_eq!(tool_result.text_content(), "transformed-tool-result");
            assert!(
                !tool_result
                    .text_content()
                    .contains("raw-tool-result-secret")
            );
            Ok(PromptCompletion::new(
                vec![ConversationItem::text(Role::Assistant, "done")],
                provider_protocol::FinishReason::Stop,
                None,
            ))
        })
    }

    fn list_models<'a>(
        &'a self,
    ) -> ProviderFuture<'a, Result<Vec<ModelDescriptor>, ProviderError>> {
        Box::pin(async { Ok(Vec::new()) })
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities::chat_completions()
    }
}

struct RawPersistenceTool;

impl Tool for RawPersistenceTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new("echo")
            .with_label("Echo")
            .with_kind(ToolKind::Other)
            .with_permission_policy(ToolPermissionPolicy::Always)
    }

    fn execute<'a>(
        &'a self,
        call: RuntimeToolCall,
        _cancellation: &'a CancellationToken,
    ) -> ToolExecutionFuture<'a> {
        Box::pin(async move { ToolResult::success(call.call_id, "raw-tool-result-secret") })
    }
}

#[test]
fn conversation_worker_persists_user_turn_when_request_fails_before_streaming() {
    let root = tempdir_path("worker-pre-stream-failure-persistence");
    let work_dir = root.join("workspace");
    fs::create_dir_all(&work_dir).expect("work dir should be creatable");
    let store =
        Arc::new(run_store(LocalSessionStore::open_in(root)).expect("local store should open"));
    let store_trait: Arc<dyn SessionStore> = store.clone();
    let mut conversation = ProviderConversation::with_session_port(
        store_trait,
        sample_header(&work_dir, "gpt-5-mini"),
    )
    .expect("persisted conversation should initialize");
    let user = ConversationItem::text(Role::User, "please persist even if provider setup fails");
    let request = conversation
        .prepare_turn(&runtime_domain::session::ConversationTurnRequest::new(
            "openai",
            "gpt-5-mini",
            user.clone(),
        ))
        .expect("turn should prepare");
    let (sender, receiver) = conversation_worker_event_channel();

    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("test runtime should build")
        .block_on(run_conversation_worker(
            request,
            fake_provider_lease(),
            ToolExecutorRegistry::new(),
            CancellationToken::new(),
            conversation_worker_options(
                RuntimeRequestPolicy::default(),
                ExtensionHookRegistry::new(),
            ),
            sender,
        ));

    let events = receiver.try_iter().collect::<Vec<_>>();
    assert!(events.iter().any(|event| {
        matches!(
            event,
            ConversationWorkerEvent::Progress(ConversationEvent::Failed { message })
                if message == "provider request failed"
        )
    }));
    assert!(
        events
            .iter()
            .all(|event| { !format!("{event:?}").contains("fixture provider failure") })
    );

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
        ProviderConversation::with_session_port(store_trait, sample_header(&work_dir, "qwen3"))
            .expect("persisted conversation should initialize");
    conversation.set_session_id(missing_session_id.clone());
    let request = conversation
        .prepare_turn(&runtime_domain::session::ConversationTurnRequest::new(
            "local",
            "qwen3",
            ConversationItem::text(Role::User, "hello"),
        ))
        .expect("turn should prepare");

    let error = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("test runtime should build")
        .block_on(async {
            let (command_sender, command_receiver) = tokio_mpsc::channel(16);
            let (event_sender, _event_receiver) = conversation_worker_event_channel();
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
fn session_persistence_actor_replies_to_pending_flush_when_error_stops_actor() {
    let root = tempdir_path("worker-error-drains-pending-flush");
    let work_dir = root.join("workspace");
    fs::create_dir_all(&work_dir).expect("work dir should be creatable");
    let store =
        Arc::new(run_store(LocalSessionStore::open_in(root)).expect("local store should open"));
    let store_trait: Arc<dyn SessionStore> = store;
    let mut conversation =
        ProviderConversation::with_session_port(store_trait, sample_header(&work_dir, "qwen3"))
            .expect("persisted conversation should initialize");
    let request = conversation
        .prepare_turn(&runtime_domain::session::ConversationTurnRequest::new(
            "local",
            "qwen3",
            ConversationItem::text(Role::User, "hello"),
        ))
        .expect("turn should prepare");

    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("test runtime should build")
        .block_on(async {
            let (command_sender, command_receiver) = tokio_mpsc::channel(16);
            let (event_sender, _event_receiver) = conversation_worker_event_channel();
            command_sender
                .send(SessionPersistenceCommand::ProviderContextItem(
                    ConversationItem::text(Role::Assistant, "hi"),
                ))
                .await
                .expect("context item command should queue before actor starts");
            let (flush_ack, flush_result) = tokio::sync::oneshot::channel();
            command_sender
                .send(SessionPersistenceCommand::Flush { ack: flush_ack })
                .await
                .expect("flush command should queue behind the failing command");

            let actor = tokio::spawn(run_session_persistence_actor(
                request.persistence_cloned(),
                command_receiver,
                event_sender,
                CancellationToken::new(),
            ));
            let error = flush_result
                .await
                .expect("pending flush acknowledgement should not be dropped")
                .expect_err("pending flush should receive the actor stop error");

            assert!(matches!(
                error.as_ref(),
                SessionPersistenceError::MissingSession
            ));
            drop(command_sender);
            actor.await.expect("persistence actor should stop cleanly");
        });
}

#[test]
fn session_persistence_actor_flushes_finish_work_after_conversation_cancellation() {
    let root = tempdir_path("worker-cancellation-drains-finish-work");
    let work_dir = root.join("workspace");
    fs::create_dir_all(&work_dir).expect("work dir should be creatable");
    let store =
        Arc::new(run_store(LocalSessionStore::open_in(root)).expect("local store should open"));
    let store_trait: Arc<dyn SessionStore> = store.clone();
    let mut conversation =
        ProviderConversation::with_session_port(store_trait, sample_header(&work_dir, "qwen3"))
            .expect("persisted conversation should initialize");
    let user = ConversationItem::text(Role::User, "run a tool");
    let assistant = ConversationItem::text(Role::Assistant, "tool was interrupted");
    let request = conversation
        .prepare_turn(&runtime_domain::session::ConversationTurnRequest::new(
            "local",
            "qwen3",
            user.clone(),
        ))
        .expect("turn should prepare");

    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("test runtime should build")
        .block_on(async {
            let (command_sender, command_receiver) = tokio_mpsc::channel(16);
            let (event_sender, _event_receiver) = conversation_worker_event_channel();
            let cancellation = CancellationToken::new();
            let actor = tokio::spawn(run_session_persistence_actor(
                request.persistence_cloned(),
                command_receiver,
                event_sender,
                cancellation.clone(),
            ));

            command_sender
                .send(SessionPersistenceCommand::ProviderTurnStarted)
                .await
                .expect("turn start command should queue");
            cancellation.cancel();
            command_sender
                .send(SessionPersistenceCommand::ProviderContextItem(assistant))
                .await
                .expect("finish context item should queue after cancellation");
            flush_session_persistence(&command_sender)
                .await
                .expect("finish work should flush after cancellation");

            drop(command_sender);
            actor.await.expect("persistence actor should stop cleanly");
        });

    let metas = run_store(store.list_sessions(
        &ProjectDir::from_work_dir(&work_dir),
        SessionListOptions::default(),
    ))
    .expect("session meta should list");
    assert_eq!(metas.len(), 1);
    let resolved = run_store(store.resolve(&metas[0].session_id, None))
        .expect("resolved items should be readable");

    assert_eq!(
        resolved,
        vec![
            user,
            ConversationItem::text(Role::Assistant, "tool was interrupted")
        ]
    );
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
        ProviderConversation::with_session_port(store_trait, sample_header(&work_dir, "qwen3"))
            .expect("persisted conversation should initialize");
    let request = conversation
        .prepare_turn(&runtime_domain::session::ConversationTurnRequest::new(
            "local",
            "qwen3",
            ConversationItem::text(Role::User, "edit file"),
        ))
        .expect("turn should prepare");
    let (sender, _receiver) = conversation_worker_event_channel();
    let persistence = request.persistence_cloned();
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
        ProviderConversation::with_session_port(store_trait, sample_header(&work_dir, "qwen3"))
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
        .prepare_turn_with_options(
            &runtime_domain::session::ConversationTurnRequest::new(
                "local",
                "qwen3",
                provider_user.clone(),
            ),
            PreparedTurnOptions::default()
                .with_transcript_user_message(transcript_user)
                .with_transcript_replay_after_user(vec![TranscriptReplayItem::ToolActivity {
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
                }]),
        )
        .expect("turn should prepare");
    let (sender, _receiver) = conversation_worker_event_channel();
    let persistence = request.persistence_cloned();
    let mut state = SessionPersistenceState::default();

    run_persistence(persist_turn_start(
        persistence.as_ref(),
        &sender,
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
            .conversation
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
        ProviderConversation::with_session_port(store_trait, sample_header(&work_dir, "gpt-4o"))
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
                "gpt-4o",
                transcript_user.clone(),
            ),
        )
        .expect("turn should prepare");
    let (sender, _receiver) = conversation_worker_event_channel();
    let persistence = request.persistence_cloned();
    let mut state = SessionPersistenceState::default();

    run_persistence(persist_turn_start(
        persistence.as_ref(),
        &sender,
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
            if *message == transcript_user
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
        ProviderConversation::with_session_port(store_trait, sample_header(&work_dir, "gpt-4o"))
            .expect("persisted conversation should initialize");
    let user = ConversationItem::text(Role::User, "inspect image");
    let request = conversation
        .prepare_turn(&runtime_domain::session::ConversationTurnRequest::new(
            "local", "gpt-4o", user,
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
    let (sender, _receiver) = conversation_worker_event_channel();
    let persistence = request.persistence_cloned();
    let mut state = SessionPersistenceState::default();

    run_persistence(persist_turn_start(
        persistence.as_ref(),
        &sender,
        &mut state,
    ))
    .expect("turn start should persist");
    run_persistence(persist_context_item(
        persistence.as_ref(),
        &sender,
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
