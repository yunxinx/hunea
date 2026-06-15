use std::{
    fs,
    future::Future,
    path::{Path, PathBuf},
    pin::Pin,
    sync::Arc,
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use super::{
    AppRuntimeCoordinator, AppRuntimeOptions, ensure_conversation_target,
    should_defer_runtime_event_for_render_barrier,
};
use provider_protocol::{ContentBlock, ConversationItem, Role, ToolCall};
use runtime_domain::{
    model_catalog::ModelSelection,
    provider::ProviderKind,
    session::{
        ConversationTurnRequest, ManagedSearchTool, RuntimeCommand, RuntimeEvent,
        RuntimePermissionRequest, RuntimeTarget, RuntimeToolActivity, RuntimeToolActivityContent,
        RuntimeToolActivityRawValue, RuntimeToolActivityStatus, RuntimeToolKind,
        SessionTreeRowKind, TranscriptReplayItem, TranscriptReplayRole,
    },
};
use session_store::{
    ConfigSnapshot, InMemorySessionStore, ResolvedSessionState, SessionHeader, SessionId,
    SessionMeta, SessionStore, SessionStoreError, SessionTreeSnapshot,
};
use terminal_ui::RuntimeCoordinator;

struct LoadCountingSessionStore {
    inner: Arc<InMemorySessionStore>,
    load_session_calls: AtomicUsize,
}

impl LoadCountingSessionStore {
    fn load_session_calls(&self) -> usize {
        self.load_session_calls.load(Ordering::SeqCst)
    }
}

impl SessionStore for LoadCountingSessionStore {
    fn create_session<'a>(
        &'a self,
        header: SessionHeader,
    ) -> Pin<Box<dyn Future<Output = Result<SessionId, SessionStoreError>> + Send + 'a>> {
        self.inner.create_session(header)
    }

    fn append<'a>(
        &'a self,
        session_id: &'a SessionId,
        item: ConversationItem,
    ) -> Pin<Box<dyn Future<Output = Result<String, SessionStoreError>> + Send + 'a>> {
        self.inner.append(session_id, item)
    }

    fn append_config_change<'a>(
        &'a self,
        session_id: &'a SessionId,
        snapshot: ConfigSnapshot,
    ) -> Pin<Box<dyn Future<Output = Result<(), SessionStoreError>> + Send + 'a>> {
        self.inner.append_config_change(session_id, snapshot)
    }

    fn append_transcript_replay<'a>(
        &'a self,
        session_id: &'a SessionId,
        item: TranscriptReplayItem,
    ) -> Pin<Box<dyn Future<Output = Result<String, SessionStoreError>> + Send + 'a>> {
        self.inner.append_transcript_replay(session_id, item)
    }

    fn set_leaf<'a>(
        &'a self,
        session_id: &'a SessionId,
        leaf_id: Option<&'a str>,
    ) -> Pin<Box<dyn Future<Output = Result<(), SessionStoreError>> + Send + 'a>> {
        self.inner.set_leaf(session_id, leaf_id)
    }

    fn resolve<'a>(
        &'a self,
        session_id: &'a SessionId,
        leaf_id: Option<&'a str>,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<ConversationItem>, SessionStoreError>> + Send + 'a>>
    {
        self.inner.resolve(session_id, leaf_id)
    }

    fn load_session<'a>(
        &'a self,
        session_id: &'a SessionId,
        leaf_id: Option<&'a str>,
    ) -> Pin<Box<dyn Future<Output = Result<ResolvedSessionState, SessionStoreError>> + Send + 'a>>
    {
        self.load_session_calls.fetch_add(1, Ordering::SeqCst);
        self.inner.load_session(session_id, leaf_id)
    }

    fn load_session_tree<'a>(
        &'a self,
        session_id: &'a SessionId,
    ) -> Pin<Box<dyn Future<Output = Result<SessionTreeSnapshot, SessionStoreError>> + Send + 'a>>
    {
        self.inner.load_session_tree(session_id)
    }

    fn load_session_tree_for_leaf<'a>(
        &'a self,
        session_id: &'a SessionId,
        leaf_id: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<SessionTreeSnapshot, SessionStoreError>> + Send + 'a>>
    {
        self.inner.load_session_tree_for_leaf(session_id, leaf_id)
    }

    fn load_session_branch_preview<'a>(
        &'a self,
        session_id: &'a SessionId,
        branch_row_id: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<SessionTreeSnapshot, SessionStoreError>> + Send + 'a>>
    {
        self.inner
            .load_session_branch_preview(session_id, branch_row_id)
    }

    fn load_session_branch_tree<'a>(
        &'a self,
        session_id: &'a SessionId,
    ) -> Pin<
        Box<
            dyn Future<Output = Result<session_store::SessionBranchTreeSnapshot, SessionStoreError>>
                + Send
                + 'a,
        >,
    > {
        self.inner.load_session_branch_tree(session_id)
    }

    fn list_sessions<'a>(
        &'a self,
        project_dir: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<SessionMeta>, SessionStoreError>> + Send + 'a>>
    {
        self.inner.list_sessions(project_dir)
    }

    fn get_session_meta<'a>(
        &'a self,
        session_id: &'a SessionId,
    ) -> Pin<Box<dyn Future<Output = Result<SessionMeta, SessionStoreError>> + Send + 'a>> {
        self.inner.get_session_meta(session_id)
    }

    fn flush<'a>(
        &'a self,
        session_id: &'a SessionId,
    ) -> Pin<Box<dyn Future<Output = Result<(), SessionStoreError>> + Send + 'a>> {
        self.inner.flush(session_id)
    }
}

struct CommittedLoadFailsAfterSetLeafStore {
    inner: Arc<InMemorySessionStore>,
    fail_committed_load: AtomicBool,
}

impl CommittedLoadFailsAfterSetLeafStore {
    fn new(inner: Arc<InMemorySessionStore>) -> Self {
        Self {
            inner,
            fail_committed_load: AtomicBool::new(false),
        }
    }
}

impl SessionStore for CommittedLoadFailsAfterSetLeafStore {
    fn create_session<'a>(
        &'a self,
        header: SessionHeader,
    ) -> Pin<Box<dyn Future<Output = Result<SessionId, SessionStoreError>> + Send + 'a>> {
        self.inner.create_session(header)
    }

    fn append<'a>(
        &'a self,
        session_id: &'a SessionId,
        item: ConversationItem,
    ) -> Pin<Box<dyn Future<Output = Result<String, SessionStoreError>> + Send + 'a>> {
        self.inner.append(session_id, item)
    }

    fn append_config_change<'a>(
        &'a self,
        session_id: &'a SessionId,
        snapshot: ConfigSnapshot,
    ) -> Pin<Box<dyn Future<Output = Result<(), SessionStoreError>> + Send + 'a>> {
        self.inner.append_config_change(session_id, snapshot)
    }

    fn append_transcript_replay<'a>(
        &'a self,
        session_id: &'a SessionId,
        item: TranscriptReplayItem,
    ) -> Pin<Box<dyn Future<Output = Result<String, SessionStoreError>> + Send + 'a>> {
        self.inner.append_transcript_replay(session_id, item)
    }

    fn set_leaf<'a>(
        &'a self,
        session_id: &'a SessionId,
        leaf_id: Option<&'a str>,
    ) -> Pin<Box<dyn Future<Output = Result<(), SessionStoreError>> + Send + 'a>> {
        Box::pin(async move {
            self.inner.set_leaf(session_id, leaf_id).await?;
            self.fail_committed_load.store(true, Ordering::SeqCst);
            Ok(())
        })
    }

    fn resolve<'a>(
        &'a self,
        session_id: &'a SessionId,
        leaf_id: Option<&'a str>,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<ConversationItem>, SessionStoreError>> + Send + 'a>>
    {
        self.inner.resolve(session_id, leaf_id)
    }

    fn load_session<'a>(
        &'a self,
        session_id: &'a SessionId,
        leaf_id: Option<&'a str>,
    ) -> Pin<Box<dyn Future<Output = Result<ResolvedSessionState, SessionStoreError>> + Send + 'a>>
    {
        if leaf_id.is_none() && self.fail_committed_load.load(Ordering::SeqCst) {
            return Box::pin(async {
                Err(SessionStoreError::IndexInconsistent {
                    message: "injected committed load failure".to_string(),
                })
            });
        }
        self.inner.load_session(session_id, leaf_id)
    }

    fn load_session_tree<'a>(
        &'a self,
        session_id: &'a SessionId,
    ) -> Pin<Box<dyn Future<Output = Result<SessionTreeSnapshot, SessionStoreError>> + Send + 'a>>
    {
        self.inner.load_session_tree(session_id)
    }

    fn load_session_tree_for_leaf<'a>(
        &'a self,
        session_id: &'a SessionId,
        leaf_id: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<SessionTreeSnapshot, SessionStoreError>> + Send + 'a>>
    {
        self.inner.load_session_tree_for_leaf(session_id, leaf_id)
    }

    fn load_session_branch_preview<'a>(
        &'a self,
        session_id: &'a SessionId,
        branch_row_id: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<SessionTreeSnapshot, SessionStoreError>> + Send + 'a>>
    {
        self.inner
            .load_session_branch_preview(session_id, branch_row_id)
    }

    fn load_session_branch_tree<'a>(
        &'a self,
        session_id: &'a SessionId,
    ) -> Pin<
        Box<
            dyn Future<Output = Result<session_store::SessionBranchTreeSnapshot, SessionStoreError>>
                + Send
                + 'a,
        >,
    > {
        self.inner.load_session_branch_tree(session_id)
    }

    fn list_sessions<'a>(
        &'a self,
        project_dir: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<SessionMeta>, SessionStoreError>> + Send + 'a>>
    {
        self.inner.list_sessions(project_dir)
    }

    fn get_session_meta<'a>(
        &'a self,
        session_id: &'a SessionId,
    ) -> Pin<Box<dyn Future<Output = Result<SessionMeta, SessionStoreError>> + Send + 'a>> {
        self.inner.get_session_meta(session_id)
    }

    fn flush<'a>(
        &'a self,
        session_id: &'a SessionId,
    ) -> Pin<Box<dyn Future<Output = Result<(), SessionStoreError>> + Send + 'a>> {
        self.inner.flush(session_id)
    }
}

#[test]
fn conversation_target_must_match_running_worker() {
    let active_target = RuntimeTarget::provider("openai", "gpt-4o-mini");
    assert!(ensure_conversation_target(Some(&active_target), None).is_ok());
    assert!(ensure_conversation_target(Some(&active_target), Some(&active_target)).is_ok());

    let inactive_target = RuntimeTarget::provider("openai", "gpt-4.1-mini");
    let inactive_error = ensure_conversation_target(Some(&active_target), Some(&inactive_target))
        .expect_err("wrong conversation target should be rejected");
    assert!(inactive_error.contains("Conversation is not active"));

    let stopped_error = ensure_conversation_target(None, Some(&active_target))
        .expect_err("explicit conversation target should require a running worker");
    assert!(stopped_error.contains("Conversation is not running"));
}

#[test]
fn token_estimate_creates_render_barrier_before_permission_request() {
    let output_batch = vec![RuntimeEvent::OutputTokenEstimate {
        target: Some(RuntimeTarget::provider("local", "qwen3")),
        total_tokens: 57,
    }];
    let input_batch = vec![RuntimeEvent::InputTokenEstimate {
        target: Some(RuntimeTarget::provider("local", "qwen3")),
        total_tokens: 12,
    }];
    let permission_event = RuntimeEvent::PermissionRequested {
        target: RuntimeTarget::provider("local", "qwen3"),
        request: RuntimePermissionRequest::new(
            "permission-1",
            Some("Write temp.md".into()),
            vec![],
        ),
    };

    assert!(
        should_defer_runtime_event_for_render_barrier(&output_batch, &permission_event),
        "permission should wait for the output token estimate batch to render first"
    );
    assert!(
        should_defer_runtime_event_for_render_barrier(&input_batch, &permission_event),
        "permission should wait for the input token estimate batch to render first"
    );
    assert!(
        !should_defer_runtime_event_for_render_barrier(&[], &permission_event),
        "permission should not be deferred when there is no token estimate to render"
    );
}

#[test]
fn app_layer_persists_managed_search_tool_authorization() {
    let root = temp_test_dir("managed-search-authorization");
    let config_path = root.join("config.toml");
    let mut coordinator = AppRuntimeCoordinator::new(AppRuntimeOptions {
        managed_search_authorization_config_path: Some(config_path.clone()),
        ..AppRuntimeOptions::default()
    });

    let event = coordinator.persist_managed_search_tool_authorization(ManagedSearchTool::Fd, None);

    assert_eq!(event, None);
    assert_eq!(
        coordinator.options.managed_search_tools.allow_managed_fd,
        Some(true)
    );
    let content = fs::read_to_string(&config_path).expect("config should be readable");
    assert!(content.contains("allow_managed_fd = true"));
    cleanup(&root);
}

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
    let mut coordinator = AppRuntimeCoordinator::new(AppRuntimeOptions {
        session_store: Some(store),
        session_header_template: Some(header),
        ..AppRuntimeOptions::default()
    });

    coordinator
        .handle_runtime_command(RuntimeCommand::ListSessions)
        .expect("list sessions should succeed");

    let events = RuntimeCoordinator::drain_runtime_events(&mut coordinator);
    let Some(RuntimeEvent::SessionListLoaded { rows }) = events.into_iter().next() else {
        panic!("expected session list event");
    };
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
    let store = Arc::new(LoadCountingSessionStore {
        inner: inner_store,
        load_session_calls: AtomicUsize::new(0),
    });
    let mut coordinator = AppRuntimeCoordinator::new(AppRuntimeOptions {
        session_store: Some(store.clone()),
        session_header_template: Some(header),
        ..AppRuntimeOptions::default()
    });

    coordinator
        .handle_runtime_command(RuntimeCommand::ListSessions)
        .expect("list sessions should succeed");

    let events = RuntimeCoordinator::drain_runtime_events(&mut coordinator);
    let Some(RuntimeEvent::SessionListLoaded { rows }) = events.into_iter().next() else {
        panic!("expected session list event");
    };
    assert_eq!(store.load_session_calls(), 0);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].first_user_message, "metadata first user");
    assert_eq!(rows[0].last_assistant_message, "metadata assistant answer");
    cleanup(&work_dir);
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
    let mut coordinator = AppRuntimeCoordinator::new(AppRuntimeOptions {
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

    let events = RuntimeCoordinator::drain_runtime_events(&mut coordinator);
    let Some(RuntimeEvent::SessionListLoaded { rows }) = events.into_iter().next() else {
        panic!("expected session list event");
    };
    assert_eq!(
        rows.iter()
            .map(|row| row.session_id.as_str())
            .collect::<Vec<_>>(),
        vec![other_session_id.to_string()]
    );
    cleanup(&work_dir);
}

#[test]
fn resume_session_emits_transcript_and_restored_model() {
    let work_dir = temp_test_dir("resume-session-work");
    let store = Arc::new(InMemorySessionStore::new());
    let store_runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("session store runtime should start");
    let header = SessionHeader {
        session_id: SessionId::new(),
        work_dir: work_dir.clone(),
        session_name: None,
        initial_model: "qwen2".to_string(),
        git_head: None,
        cli_version: None,
    };
    let session_id = store_runtime
        .block_on(async {
            let session_id = store.create_session(header.clone()).await?;
            store
                .append(
                    &session_id,
                    ConversationItem::text(Role::User, "hello resume"),
                )
                .await?;
            store
                .append_transcript_replay(
                    &session_id,
                    TranscriptReplayItem::Message {
                        role: TranscriptReplayRole::User,
                        content: "hello resume".to_string(),
                    },
                )
                .await?;
            store
                .append(
                    &session_id,
                    ConversationItem::text(Role::Assistant, "resume answer"),
                )
                .await?;
            store
                .append_transcript_replay(
                    &session_id,
                    TranscriptReplayItem::Message {
                        role: TranscriptReplayRole::Assistant,
                        content: "resume answer".to_string(),
                    },
                )
                .await?;
            store
                .append_config_change(
                    &session_id,
                    ConfigSnapshot {
                        provider_id: "local".to_string(),
                        model: "qwen3".to_string(),
                        system_prompt: Some("historical prompt".to_string()),
                    },
                )
                .await?;
            Ok::<SessionId, session_store::SessionStoreError>(session_id)
        })
        .expect("session fixture should persist");
    let mut coordinator = AppRuntimeCoordinator::new(AppRuntimeOptions {
        session_store: Some(store),
        session_header_template: Some(header),
        ..AppRuntimeOptions::default()
    });

    coordinator
        .handle_runtime_command(RuntimeCommand::ResumeSession {
            session_id: session_id.to_string(),
        })
        .expect("resume session should succeed");

    let events = RuntimeCoordinator::drain_runtime_events(&mut coordinator);
    let Some(RuntimeEvent::SessionResumed { payload }) = events.into_iter().next() else {
        panic!("expected session resumed event");
    };
    assert_eq!(payload.session_id, session_id.to_string());
    assert_eq!(
        payload.restored_model,
        Some(ModelSelection::new("local", "qwen3"))
    );
    assert_eq!(
        payload
            .transcript
            .iter()
            .map(TranscriptReplayItem::content_text)
            .collect::<Vec<_>>(),
        vec!["hello resume", "resume answer"]
    );
    assert_eq!(
        coordinator
            .provider_conversation
            .history()
            .iter()
            .map(ConversationItem::text_content)
            .collect::<Vec<_>>(),
        vec!["hello resume", "resume answer"]
    );
    assert_eq!(
        coordinator.provider_conversation.system_prompt(),
        Some("historical prompt")
    );
    cleanup(&work_dir);
}

#[test]
fn resume_session_payload_does_not_label_reasoning_as_system() {
    let work_dir = temp_test_dir("resume-session-reasoning-work");
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
                .append(&session_id, ConversationItem::reasoning("private chain"))
                .await?;
            store
                .append_transcript_replay(
                    &session_id,
                    TranscriptReplayItem::Reasoning {
                        content: "private chain".to_string(),
                    },
                )
                .await?;
            store
                .append(&session_id, ConversationItem::text(Role::Assistant, "done"))
                .await?;
            store
                .append_transcript_replay(
                    &session_id,
                    TranscriptReplayItem::Message {
                        role: TranscriptReplayRole::Assistant,
                        content: "done".to_string(),
                    },
                )
                .await?;
            Ok::<SessionId, session_store::SessionStoreError>(session_id)
        })
        .expect("session fixture should persist");
    let mut coordinator = AppRuntimeCoordinator::new(AppRuntimeOptions {
        session_store: Some(store),
        session_header_template: Some(header),
        ..AppRuntimeOptions::default()
    });

    coordinator
        .handle_runtime_command(RuntimeCommand::ResumeSession {
            session_id: session_id.to_string(),
        })
        .expect("resume session should succeed");

    let events = RuntimeCoordinator::drain_runtime_events(&mut coordinator);
    let Some(RuntimeEvent::SessionResumed { payload }) = events.into_iter().next() else {
        panic!("expected session resumed event");
    };
    let reasoning = payload
            .transcript
            .iter()
            .find(|item| {
                matches!(item, TranscriptReplayItem::Reasoning { content } if content == "private chain")
            })
            .expect("reasoning replay item should be present");
    assert!(
        !matches!(reasoning, TranscriptReplayItem::System { .. }),
        "reasoning must not be replayed as a system message"
    );
    cleanup(&work_dir);
}

#[test]
fn resume_session_payload_does_not_reconstruct_transcript_from_provider_history() {
    let work_dir = temp_test_dir("resume-session-provider-only-work");
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
                .append(
                    &session_id,
                    ConversationItem::text(Role::User, "provider-only user"),
                )
                .await?;
            store
                .append(
                    &session_id,
                    ConversationItem::text(Role::Assistant, "provider-only answer"),
                )
                .await?;
            Ok::<SessionId, session_store::SessionStoreError>(session_id)
        })
        .expect("session fixture should persist");
    let mut coordinator = AppRuntimeCoordinator::new(AppRuntimeOptions {
        session_store: Some(store),
        session_header_template: Some(header),
        ..AppRuntimeOptions::default()
    });

    coordinator
        .handle_runtime_command(RuntimeCommand::ResumeSession {
            session_id: session_id.to_string(),
        })
        .expect("resume session should succeed");

    let events = RuntimeCoordinator::drain_runtime_events(&mut coordinator);
    let Some(RuntimeEvent::SessionResumed { payload }) = events.into_iter().next() else {
        panic!("expected session resumed event");
    };
    assert!(payload.transcript.is_empty());
    cleanup(&work_dir);
}

#[test]
fn resume_session_payload_prefers_persisted_transcript_replay() {
    let work_dir = temp_test_dir("resume-session-explicit-replay-work");
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
    let replay_activity = RuntimeToolActivity {
        activity_id: "call-1".to_string(),
        title: "Write src/lib.rs".to_string(),
        kind: RuntimeToolKind::Write,
        status: RuntimeToolActivityStatus::Completed,
        content: vec![RuntimeToolActivityContent::Diff {
            path: "src/lib.rs".to_string(),
            old_text: Some("old".to_string()),
            new_text: "new".to_string(),
            is_truncated: false,
        }],
        locations: Vec::new(),
        raw_input: Some(RuntimeToolActivityRawValue::from(
            r#"{"path":"src/lib.rs"}"#,
        )),
        raw_output: Some(RuntimeToolActivityRawValue::tool_result(
            "plain provider output",
            None,
        )),
    };
    let session_id = store_runtime
        .block_on(async {
            let session_id = store.create_session(header.clone()).await?;
            store
                .append(
                    &session_id,
                    ConversationItem::assistant_with_tool_calls(
                        "editing".to_string(),
                        vec![ToolCall::new(
                            "call-1",
                            "write_file",
                            r#"{"path":"src/lib.rs"}"#,
                        )],
                    ),
                )
                .await?;
            store
                .append(
                    &session_id,
                    ConversationItem::tool_result(
                        "call-1",
                        vec![ContentBlock::Text("plain provider output".to_string())],
                        false,
                    ),
                )
                .await?;
            store
                .append_transcript_replay(
                    &session_id,
                    TranscriptReplayItem::Message {
                        role: TranscriptReplayRole::Assistant,
                        content: "editing".to_string(),
                    },
                )
                .await?;
            store
                .append_transcript_replay(
                    &session_id,
                    TranscriptReplayItem::ToolActivity {
                        activity: replay_activity.clone(),
                    },
                )
                .await?;
            Ok::<SessionId, session_store::SessionStoreError>(session_id)
        })
        .expect("session fixture should persist");
    let mut coordinator = AppRuntimeCoordinator::new(AppRuntimeOptions {
        session_store: Some(store),
        session_header_template: Some(header),
        ..AppRuntimeOptions::default()
    });

    coordinator
        .handle_runtime_command(RuntimeCommand::ResumeSession {
            session_id: session_id.to_string(),
        })
        .expect("resume session should succeed");

    let events = RuntimeCoordinator::drain_runtime_events(&mut coordinator);
    let Some(RuntimeEvent::SessionResumed { payload }) = events.into_iter().next() else {
        panic!("expected session resumed event");
    };
    assert_eq!(
        payload.transcript,
        vec![
            TranscriptReplayItem::Message {
                role: TranscriptReplayRole::Assistant,
                content: "editing".to_string(),
            },
            TranscriptReplayItem::ToolActivity {
                activity: replay_activity,
            },
        ],
        "explicit replay should preserve rich diff content instead of fallback text"
    );
    cleanup(&work_dir);
}

#[test]
fn load_session_preview_emits_transcript_without_resuming_runtime_session() {
    let work_dir = temp_test_dir("preview-session-work");
    let store = Arc::new(InMemorySessionStore::new());
    let store_runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("session store runtime should start");
    let header = SessionHeader {
        session_id: SessionId::new(),
        work_dir: work_dir.clone(),
        session_name: None,
        initial_model: "qwen2".to_string(),
        git_head: None,
        cli_version: None,
    };
    let preview_session_id = store_runtime
        .block_on(async {
            let active_session_id = store.create_session(header.clone()).await?;
            store
                .append(
                    &active_session_id,
                    ConversationItem::text(Role::User, "active user"),
                )
                .await?;
            let preview_session_id = store
                .create_session(SessionHeader {
                    session_id: SessionId::new(),
                    session_name: Some("preview".to_string()),
                    ..header.clone()
                })
                .await?;
            store
                .append(
                    &preview_session_id,
                    ConversationItem::text(Role::User, "preview user"),
                )
                .await?;
            store
                .append_transcript_replay(
                    &preview_session_id,
                    TranscriptReplayItem::Message {
                        role: TranscriptReplayRole::User,
                        content: "preview user".to_string(),
                    },
                )
                .await?;
            store
                .append(
                    &preview_session_id,
                    ConversationItem::text(Role::Assistant, "preview answer"),
                )
                .await?;
            store
                .append_transcript_replay(
                    &preview_session_id,
                    TranscriptReplayItem::Message {
                        role: TranscriptReplayRole::Assistant,
                        content: "preview answer".to_string(),
                    },
                )
                .await?;
            Ok::<SessionId, session_store::SessionStoreError>(preview_session_id)
        })
        .expect("session fixture should persist");
    let mut coordinator = AppRuntimeCoordinator::new(AppRuntimeOptions {
        session_store: Some(store),
        session_header_template: Some(header),
        ..AppRuntimeOptions::default()
    });

    coordinator
        .handle_runtime_command(RuntimeCommand::LoadSessionPreview {
            session_id: preview_session_id.to_string(),
        })
        .expect("load preview should succeed");

    let events = RuntimeCoordinator::drain_runtime_events(&mut coordinator);
    let Some(RuntimeEvent::SessionPreviewLoaded { payload }) = events.into_iter().next() else {
        panic!("expected session preview event");
    };
    assert_eq!(payload.session_id, preview_session_id.to_string());
    assert_eq!(
        payload
            .transcript
            .iter()
            .map(TranscriptReplayItem::content_text)
            .collect::<Vec<_>>(),
        vec!["preview user", "preview answer"]
    );
    assert!(
        coordinator.provider_conversation.history().is_empty(),
        "loading preview should not replace the active provider conversation"
    );
    cleanup(&work_dir);
}

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
    let mut coordinator = AppRuntimeCoordinator::new(AppRuntimeOptions {
        session_store: Some(store),
        session_header_template: Some(header),
        ..AppRuntimeOptions::default()
    });
    coordinator
        .handle_runtime_command(RuntimeCommand::ResumeSession {
            session_id: session_id.to_string(),
        })
        .expect("resume session should succeed");
    RuntimeCoordinator::drain_runtime_events(&mut coordinator);

    coordinator
        .handle_runtime_command(RuntimeCommand::LoadEntryTree)
        .expect("load entry tree should succeed");

    let events = RuntimeCoordinator::drain_runtime_events(&mut coordinator);
    let Some(RuntimeEvent::SessionTreeLoaded { payload }) = events.into_iter().next() else {
        panic!("expected session tree loaded event");
    };
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
    let mut coordinator = AppRuntimeCoordinator::new(AppRuntimeOptions {
        session_store: Some(store),
        session_header_template: Some(header),
        ..AppRuntimeOptions::default()
    });

    coordinator
        .handle_runtime_command(RuntimeCommand::LoadEntryTree)
        .expect("empty new session tree should load as an empty payload");

    let events = RuntimeCoordinator::drain_runtime_events(&mut coordinator);
    let Some(RuntimeEvent::SessionTreeLoaded { payload }) = events.into_iter().next() else {
        panic!("expected empty session tree loaded event");
    };
    assert!(
        payload.rows.is_empty(),
        "new sessions without messages should render an empty tree"
    );
    assert_eq!(payload.current_row_id, None);
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
    let mut coordinator = AppRuntimeCoordinator::new(AppRuntimeOptions {
        session_store: Some(store),
        session_header_template: Some(header),
        ..AppRuntimeOptions::default()
    });
    coordinator
        .handle_runtime_command(RuntimeCommand::ResumeSession {
            session_id: session_id.to_string(),
        })
        .expect("resume session should succeed");
    RuntimeCoordinator::drain_runtime_events(&mut coordinator);

    coordinator
        .handle_runtime_command(RuntimeCommand::LoadBranchTree)
        .expect("load branch tree should succeed");

    let events = RuntimeCoordinator::drain_runtime_events(&mut coordinator);
    let Some(RuntimeEvent::SessionBranchTreeLoaded { payload }) = events.into_iter().next() else {
        panic!("expected branch tree loaded event");
    };
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
    let mut coordinator = AppRuntimeCoordinator::new(AppRuntimeOptions {
        session_store: Some(store),
        session_header_template: Some(header),
        ..AppRuntimeOptions::default()
    });
    coordinator
        .handle_runtime_command(RuntimeCommand::ResumeSession {
            session_id: session_id.to_string(),
        })
        .expect("resume session should succeed");
    RuntimeCoordinator::drain_runtime_events(&mut coordinator);

    coordinator
        .handle_runtime_command(RuntimeCommand::LoadBranchPreview {
            branch_row_id: inactive_branch_row_id.clone(),
        })
        .expect("load branch preview should succeed");

    let events = RuntimeCoordinator::drain_runtime_events(&mut coordinator);
    let Some(RuntimeEvent::SessionTreePreviewLoaded { payload }) = events.into_iter().next() else {
        panic!("expected branch preview loaded event");
    };
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
        .handle_runtime_command(RuntimeCommand::LoadEntryTree)
        .expect("load committed tree should still succeed");
    let committed_events = RuntimeCoordinator::drain_runtime_events(&mut coordinator);
    let committed_payload = committed_events
        .into_iter()
        .find_map(|event| match event {
            RuntimeEvent::SessionTreeLoaded { payload } => Some(payload),
            _ => None,
        })
        .expect("committed session tree should load");
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
    let mut coordinator = AppRuntimeCoordinator::new(AppRuntimeOptions {
        session_store: Some(store),
        session_header_template: Some(header),
        ..AppRuntimeOptions::default()
    });
    coordinator
        .handle_runtime_command(RuntimeCommand::ResumeSession {
            session_id: session_id.to_string(),
        })
        .expect("resume session should succeed");
    RuntimeCoordinator::drain_runtime_events(&mut coordinator);
    coordinator
        .handle_runtime_command(RuntimeCommand::LoadEntryTree)
        .expect("load entry tree should succeed");
    let tree_events = RuntimeCoordinator::drain_runtime_events(&mut coordinator);
    let current_tree = tree_events
        .into_iter()
        .find_map(|event| match event {
            RuntimeEvent::SessionTreeLoaded { payload } => Some(payload),
            _ => None,
        })
        .expect("session tree payload should be loaded");
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
        .handle_runtime_command(RuntimeCommand::LoadBranchPreview { branch_row_id })
        .expect("branch preview should load");
    let preview_rows = RuntimeCoordinator::drain_runtime_events(&mut coordinator)
        .into_iter()
        .find_map(|event| match event {
            RuntimeEvent::SessionTreePreviewLoaded { payload } => Some(
                payload
                    .rows
                    .into_iter()
                    .map(|row| row.preview_content)
                    .collect::<Vec<_>>(),
            ),
            _ => None,
        })
        .expect("preview payload should be emitted");

    coordinator
        .handle_runtime_command(RuntimeCommand::SwitchBranch {
            leaf_id: branch_leaf_id,
        })
        .expect("switch branch should succeed");

    assert_eq!(
        coordinator
            .provider_conversation
            .history()
            .iter()
            .map(ConversationItem::text_content)
            .collect::<Vec<_>>(),
        vec!["hello", "branch-b"],
        "provider history should immediately move to the switched branch"
    );
    let events = RuntimeCoordinator::drain_runtime_events(&mut coordinator);
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
            RuntimeEvent::SessionTreeLoaded { payload } => Some(payload),
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
    let mut coordinator = AppRuntimeCoordinator::new(AppRuntimeOptions {
        session_store: Some(store),
        session_header_template: Some(header),
        ..AppRuntimeOptions::default()
    });
    coordinator
        .handle_runtime_command(RuntimeCommand::ResumeSession {
            session_id: session_id.to_string(),
        })
        .expect("resume session should succeed");
    RuntimeCoordinator::drain_runtime_events(&mut coordinator);
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
    let mut coordinator = AppRuntimeCoordinator::new(AppRuntimeOptions {
        session_store: Some(store),
        session_header_template: Some(header),
        ..AppRuntimeOptions::default()
    });
    coordinator
        .handle_runtime_command(RuntimeCommand::ResumeSession {
            session_id: session_id.to_string(),
        })
        .expect("resume session should succeed");
    RuntimeCoordinator::drain_runtime_events(&mut coordinator);
    coordinator
        .handle_runtime_command(RuntimeCommand::LoadEntryTree)
        .expect("load entry tree should succeed");
    let before_rows = RuntimeCoordinator::drain_runtime_events(&mut coordinator)
        .into_iter()
        .find_map(|event| match event {
            RuntimeEvent::SessionTreeLoaded { payload } => Some(
                payload
                    .rows
                    .into_iter()
                    .map(|row| row.preview_content)
                    .collect::<Vec<_>>(),
            ),
            _ => None,
        })
        .expect("before tree should load");

    let error = coordinator
        .handle_runtime_command(RuntimeCommand::SwitchBranch {
            leaf_id: "missing-leaf".to_string(),
        })
        .expect_err("invalid leaf should fail");

    assert!(
        error.contains("missing-leaf"),
        "failure should include the missing leaf id: {error}"
    );
    coordinator
        .handle_runtime_command(RuntimeCommand::LoadEntryTree)
        .expect("load entry tree should still succeed");
    let after_rows = RuntimeCoordinator::drain_runtime_events(&mut coordinator)
        .into_iter()
        .find_map(|event| match event {
            RuntimeEvent::SessionTreeLoaded { payload } => Some(
                payload
                    .rows
                    .into_iter()
                    .map(|row| row.preview_content)
                    .collect::<Vec<_>>(),
            ),
            _ => None,
        })
        .expect("after tree should load");
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
    let mut coordinator = AppRuntimeCoordinator::new(AppRuntimeOptions {
        session_store: Some(failing_store),
        session_header_template: Some(header),
        ..AppRuntimeOptions::default()
    });
    coordinator
        .handle_runtime_command(RuntimeCommand::ResumeSession {
            session_id: session_id.to_string(),
        })
        .expect("resume session should succeed");
    RuntimeCoordinator::drain_runtime_events(&mut coordinator);
    coordinator
        .handle_runtime_command(RuntimeCommand::LoadEntryTree)
        .expect("load entry tree should succeed");
    let before_rows = RuntimeCoordinator::drain_runtime_events(&mut coordinator)
        .into_iter()
        .find_map(|event| match event {
            RuntimeEvent::SessionTreeLoaded { payload } => Some(
                payload
                    .rows
                    .into_iter()
                    .map(|row| row.preview_content)
                    .collect::<Vec<_>>(),
            ),
            _ => None,
        })
        .expect("before tree should load");
    coordinator
        .handle_runtime_command(RuntimeCommand::SwitchBranch {
            leaf_id: inactive_branch_leaf_id,
        })
        .expect("switch should not reload from the committed leaf after set_leaf");
    coordinator
        .handle_runtime_command(RuntimeCommand::LoadEntryTree)
        .expect("load entry tree should still succeed");
    let after_rows = RuntimeCoordinator::drain_runtime_events(&mut coordinator)
        .into_iter()
        .find_map(|event| match event {
            RuntimeEvent::SessionTreeLoaded { payload } => Some(
                payload
                    .rows
                    .into_iter()
                    .map(|row| row.preview_content)
                    .collect::<Vec<_>>(),
            ),
            _ => None,
        })
        .expect("after tree should load");
    assert_eq!(before_rows, vec!["hello", "branch-c", "branch follow-up"]);
    assert_eq!(after_rows, vec!["hello", "branch-b"]);
    cleanup(&work_dir);
}

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
    let mut coordinator = AppRuntimeCoordinator::new(AppRuntimeOptions {
        session_store: Some(store),
        session_header_template: Some(header),
        ..AppRuntimeOptions::default()
    });
    coordinator
        .handle_runtime_command(RuntimeCommand::ResumeSession {
            session_id: session_id.to_string(),
        })
        .expect("resume session should succeed");
    RuntimeCoordinator::drain_runtime_events(&mut coordinator);

    coordinator
        .handle_runtime_command(RuntimeCommand::LoadEntryTree)
        .expect("load entry tree should succeed");
    let events = RuntimeCoordinator::drain_runtime_events(&mut coordinator);
    let payload = events
        .into_iter()
        .find_map(|event| match event {
            RuntimeEvent::SessionTreeLoaded { payload } => Some(payload),
            _ => None,
        })
        .expect("session tree payload should be loaded");
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

    assert_eq!(
        coordinator
            .provider_conversation
            .history()
            .iter()
            .map(ConversationItem::text_content)
            .collect::<Vec<_>>(),
        vec!["first", "answer"]
    );
    let events = RuntimeCoordinator::drain_runtime_events(&mut coordinator);
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
    let mut coordinator = AppRuntimeCoordinator::new(AppRuntimeOptions {
        session_store: Some(store),
        session_header_template: Some(header),
        ..AppRuntimeOptions::default()
    });
    coordinator
        .handle_runtime_command(RuntimeCommand::ResumeSession {
            session_id: session_id.to_string(),
        })
        .expect("resume session should succeed");
    RuntimeCoordinator::drain_runtime_events(&mut coordinator);

    coordinator
        .handle_runtime_command(RuntimeCommand::LoadEntryTree)
        .expect("load entry tree should succeed");
    let events = RuntimeCoordinator::drain_runtime_events(&mut coordinator);
    let payload = events
        .into_iter()
        .find_map(|event| match event {
            RuntimeEvent::SessionTreeLoaded { payload } => Some(payload),
            _ => None,
        })
        .expect("session tree payload should be loaded");
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

    assert_eq!(
        coordinator.provider_conversation.history(),
        &[
            ConversationItem::text(Role::User, "first"),
            ConversationItem::Reasoning {
                content: "thinking".to_string(),
                summary: None,
                encrypted: None,
            },
        ]
    );
    assert_eq!(
        RuntimeCoordinator::drain_runtime_events(&mut coordinator),
        Vec::<RuntimeEvent>::new(),
        "non-rewindable reasoning should not emit a resumed payload"
    );
    cleanup(&work_dir);
}

#[test]
fn conversation_failure_before_provider_request_rolls_back_pending_user() {
    let mut coordinator = AppRuntimeCoordinator::new(AppRuntimeOptions {
        runtime_request_policy: runtime_domain::request_policy::RuntimeRequestPolicy::new(
            0,
            Vec::new(),
            1,
        ),
        ..AppRuntimeOptions::default()
    });
    let request = ConversationTurnRequest::new(
        "openai",
        ProviderKind::OpenAi,
        "gpt-4o-mini",
        None,
        None,
        None,
        ConversationItem::text(Role::User, "hello"),
    );
    let target = request.target();

    coordinator
        .handle_runtime_command(RuntimeCommand::SubmitConversationTurn { target, request })
        .expect("conversation request should start");

    let mut events = Vec::new();
    for _ in 0..50 {
        events.extend(RuntimeCoordinator::drain_runtime_events(&mut coordinator));
        if events
            .iter()
            .any(|event| matches!(event, RuntimeEvent::Failed { .. }))
        {
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }

    assert!(
        events
            .iter()
            .any(|event| matches!(event, RuntimeEvent::Failed { .. })),
        "preflight failure should be reported"
    );
    assert!(coordinator.provider_conversation.history().is_empty());

    let next_request = ConversationTurnRequest::new(
        "local",
        ProviderKind::OpenAiCompatible,
        "qwen3",
        Some("http://127.0.0.1:1234/v1".to_string()),
        None,
        None,
        ConversationItem::text(Role::User, "next"),
    );
    coordinator
        .provider_conversation
        .prepare_turn(&next_request)
        .expect("failed preflight turn should not leave stale pending state");
}

fn temp_test_dir(prefix: &str) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time should be after unix epoch")
        .as_nanos();
    let root = std::env::temp_dir().join(format!("hunea-{prefix}-{}-{stamp}", std::process::id()));
    fs::create_dir_all(&root).expect("create temp root");
    root
}

fn cleanup(path: &Path) {
    let _ = fs::remove_dir_all(path);
}
