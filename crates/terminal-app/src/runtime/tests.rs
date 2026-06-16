use std::{
    fs,
    future::Future,
    path::{Path, PathBuf},
    pin::Pin,
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
    sync::{Arc, Mutex, mpsc},
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
        ConversationTurnRequest, ManagedSearchTool, RuntimeCommand, RuntimeCommandReceipt,
        RuntimeEvent, RuntimePermissionRequest, RuntimeTarget, RuntimeToolActivity,
        RuntimeToolActivityContent, RuntimeToolActivityRawValue, RuntimeToolActivityStatus,
        RuntimeToolKind, SessionBranchTreePayload, SessionPickerRow, SessionPreviewPayload,
        SessionResumePayload, SessionTreePayload, SessionTreeRowKind, TranscriptReplayItem,
        TranscriptReplayRole,
    },
};
use session_store::{
    ConfigSnapshot, InMemorySessionStore, ResolvedSessionState, SessionHeader, SessionId,
    SessionMeta, SessionStore, SessionStoreError, SessionTreeSnapshot,
};
use terminal_ui::RuntimeCoordinator;

fn runtime_coordinator(options: AppRuntimeOptions) -> AppRuntimeCoordinator {
    AppRuntimeCoordinator::new(options).expect("runtime coordinator should initialize")
}

struct LoadCountingSessionStore {
    inner: Arc<InMemorySessionStore>,
    load_session_calls: AtomicUsize,
}

struct DelayedListSessionStore {
    inner: Arc<InMemorySessionStore>,
    list_started: Mutex<Option<mpsc::Sender<()>>>,
    list_release: Mutex<mpsc::Receiver<()>>,
}

impl DelayedListSessionStore {
    fn new(
        inner: Arc<InMemorySessionStore>,
        list_started: mpsc::Sender<()>,
        list_release: mpsc::Receiver<()>,
    ) -> Self {
        Self {
            inner,
            list_started: Mutex::new(Some(list_started)),
            list_release: Mutex::new(list_release),
        }
    }
}

impl SessionStore for DelayedListSessionStore {
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

    fn append_many<'a>(
        &'a self,
        session_id: &'a SessionId,
        items: Vec<ConversationItem>,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<String>, SessionStoreError>> + Send + 'a>> {
        self.inner.append_many(session_id, items)
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
        Box::pin(async move {
            if let Some(sender) = self
                .list_started
                .lock()
                .expect("list_started mutex should not be poisoned")
                .take()
            {
                let _ = sender.send(());
            }
            self.list_release
                .lock()
                .expect("list_release mutex should not be poisoned")
                .recv()
                .expect("test should release delayed list");
            self.inner.list_sessions(project_dir).await
        })
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

    fn flush_all<'a>(
        &'a self,
    ) -> Pin<Box<dyn Future<Output = Result<(), SessionStoreError>> + Send + 'a>> {
        self.inner.flush_all()
    }
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

    fn append_many<'a>(
        &'a self,
        session_id: &'a SessionId,
        items: Vec<ConversationItem>,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<String>, SessionStoreError>> + Send + 'a>> {
        self.inner.append_many(session_id, items)
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

    fn flush_all<'a>(
        &'a self,
    ) -> Pin<Box<dyn Future<Output = Result<(), SessionStoreError>> + Send + 'a>> {
        self.inner.flush_all()
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

    fn append_many<'a>(
        &'a self,
        session_id: &'a SessionId,
        items: Vec<ConversationItem>,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<String>, SessionStoreError>> + Send + 'a>> {
        self.inner.append_many(session_id, items)
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
                Err(SessionStoreError::CorruptIndex {
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

    fn flush_all<'a>(
        &'a self,
    ) -> Pin<Box<dyn Future<Output = Result<(), SessionStoreError>> + Send + 'a>> {
        self.inner.flush_all()
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
    let mut coordinator = runtime_coordinator(AppRuntimeOptions {
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
    let store = Arc::new(LoadCountingSessionStore {
        inner: inner_store,
        load_session_calls: AtomicUsize::new(0),
    });
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

    let payload = wait_for_session_resumed(&mut coordinator);
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

    let payload = wait_for_session_resumed(&mut coordinator);
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

    let payload = wait_for_session_resumed(&mut coordinator);
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

    let payload = wait_for_session_resumed(&mut coordinator);
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
    let mut coordinator = runtime_coordinator(AppRuntimeOptions {
        session_store: Some(store),
        session_header_template: Some(header),
        ..AppRuntimeOptions::default()
    });

    coordinator
        .handle_runtime_command(RuntimeCommand::LoadSessionPreview {
            session_id: preview_session_id.to_string(),
        })
        .expect("load preview should succeed");

    let payload = wait_for_session_preview(&mut coordinator);
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
        coordinator.provider_conversation.is_history_empty(),
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
        .handle_runtime_command(RuntimeCommand::LoadEntryTree)
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
        .handle_runtime_command(RuntimeCommand::LoadBranchTree)
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
        .handle_runtime_command(RuntimeCommand::LoadEntryTree)
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
        .handle_runtime_command(RuntimeCommand::LoadEntryTree)
        .expect("load entry tree should succeed");
    let current_tree = wait_for_runtime_event(
        &mut coordinator,
        |event| match event {
            RuntimeEvent::SessionTreeLoaded { payload } => Some(payload),
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
        .handle_runtime_command(RuntimeCommand::LoadBranchPreview { branch_row_id })
        .expect("branch preview should load");
    let preview_rows = wait_for_session_tree_preview(&mut coordinator)
        .rows
        .into_iter()
        .map(|row| row.preview_content)
        .collect::<Vec<_>>();

    coordinator
        .handle_runtime_command(RuntimeCommand::SwitchBranch {
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
        .handle_runtime_command(RuntimeCommand::LoadEntryTree)
        .expect("load entry tree should succeed");
    let before_rows = wait_for_session_tree(&mut coordinator)
        .rows
        .into_iter()
        .map(|row| row.preview_content)
        .collect::<Vec<_>>();

    let receipt = coordinator
        .handle_runtime_command(RuntimeCommand::SwitchBranch {
            leaf_id: "missing-leaf".to_string(),
        })
        .expect("invalid leaf switch should be accepted for async execution");
    assert_eq!(receipt, RuntimeCommandReceipt::Accepted);

    let error = wait_for_runtime_event(
        &mut coordinator,
        |event| match event {
            RuntimeEvent::Failed { message, .. } => Some(message),
            _ => None,
        },
        "invalid leaf failure",
    );
    assert!(
        error.contains("missing-leaf"),
        "failure should include the missing leaf id: {error}"
    );
    coordinator
        .handle_runtime_command(RuntimeCommand::LoadEntryTree)
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
        .handle_runtime_command(RuntimeCommand::LoadEntryTree)
        .expect("load entry tree should succeed");
    let before_rows = wait_for_session_tree(&mut coordinator)
        .rows
        .into_iter()
        .map(|row| row.preview_content)
        .collect::<Vec<_>>();
    coordinator
        .handle_runtime_command(RuntimeCommand::SwitchBranch {
            leaf_id: inactive_branch_leaf_id,
        })
        .expect("switch should not reload from the committed leaf after set_leaf");
    let after_rows = wait_for_runtime_event(
        &mut coordinator,
        |event| match event {
            RuntimeEvent::SessionTreeLoaded { payload } => Some(
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

#[test]
fn conversation_failure_before_provider_request_rolls_back_pending_user() {
    let mut coordinator = runtime_coordinator(AppRuntimeOptions {
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
    assert!(coordinator.provider_conversation.is_history_empty());

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

fn wait_for_session_list_rows(coordinator: &mut AppRuntimeCoordinator) -> Vec<SessionPickerRow> {
    wait_for_runtime_event(
        coordinator,
        |event| match event {
            RuntimeEvent::SessionListLoaded { rows } => Some(rows),
            _ => None,
        },
        "session list rows",
    )
}

fn wait_for_session_preview(coordinator: &mut AppRuntimeCoordinator) -> SessionPreviewPayload {
    wait_for_runtime_event(
        coordinator,
        |event| match event {
            RuntimeEvent::SessionPreviewLoaded { payload } => Some(payload),
            _ => None,
        },
        "session preview payload",
    )
}

fn wait_for_session_resumed(coordinator: &mut AppRuntimeCoordinator) -> SessionResumePayload {
    wait_for_runtime_event(
        coordinator,
        |event| match event {
            RuntimeEvent::SessionResumed { payload } => Some(payload),
            _ => None,
        },
        "session resumed payload",
    )
}

fn wait_for_session_tree(coordinator: &mut AppRuntimeCoordinator) -> SessionTreePayload {
    wait_for_runtime_event(
        coordinator,
        |event| match event {
            RuntimeEvent::SessionTreeLoaded { payload } => Some(payload),
            _ => None,
        },
        "session tree payload",
    )
}

fn wait_for_session_branch_tree(
    coordinator: &mut AppRuntimeCoordinator,
) -> SessionBranchTreePayload {
    wait_for_runtime_event(
        coordinator,
        |event| match event {
            RuntimeEvent::SessionBranchTreeLoaded { payload } => Some(payload),
            _ => None,
        },
        "session branch tree payload",
    )
}

fn wait_for_session_tree_preview(coordinator: &mut AppRuntimeCoordinator) -> SessionTreePayload {
    wait_for_runtime_event(
        coordinator,
        |event| match event {
            RuntimeEvent::SessionTreePreviewLoaded { payload } => Some(payload),
            _ => None,
        },
        "session tree preview payload",
    )
}

fn wait_for_runtime_events(
    coordinator: &mut AppRuntimeCoordinator,
    expected: &str,
) -> Vec<RuntimeEvent> {
    for _ in 0..100 {
        let events = RuntimeCoordinator::drain_runtime_events(coordinator);
        if !events.is_empty() {
            return events;
        }
        thread::sleep(Duration::from_millis(10));
    }
    panic!("{expected} should be emitted");
}

fn wait_for_runtime_idle(coordinator: &mut AppRuntimeCoordinator) {
    for _ in 0..100 {
        let events = RuntimeCoordinator::drain_runtime_events(coordinator);
        assert!(
            events.is_empty(),
            "runtime should not emit events while waiting for no-op command: {events:?}"
        );
        if !RuntimeCoordinator::has_background_runtime(coordinator) {
            return;
        }
        thread::sleep(Duration::from_millis(10));
    }
    panic!("runtime should become idle");
}

fn assert_no_runtime_events(coordinator: &mut AppRuntimeCoordinator, message: &str) {
    assert_eq!(
        RuntimeCoordinator::drain_runtime_events(coordinator),
        Vec::<RuntimeEvent>::new(),
        "{message}"
    );
}

fn wait_for_runtime_event<T>(
    coordinator: &mut AppRuntimeCoordinator,
    mut select: impl FnMut(RuntimeEvent) -> Option<T>,
    expected: &str,
) -> T {
    for _ in 0..100 {
        for event in RuntimeCoordinator::drain_runtime_events(coordinator) {
            if let Some(value) = select(event) {
                return value;
            }
        }
        thread::sleep(Duration::from_millis(10));
    }
    panic!("{expected} should be emitted");
}
