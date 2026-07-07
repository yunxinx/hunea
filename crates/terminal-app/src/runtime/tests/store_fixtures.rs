use std::{
    future::Future,
    pin::Pin,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc,
    },
};

use provider_protocol::ConversationItem;
use runtime_domain::{
    prompt_assembly::persistence::PromptAssemblyScopeState,
    session::{MessageHistoryEntry, MessageHistoryRow, TranscriptReplayItem},
};
use session_store::{
    ConfigSnapshot, InMemorySessionStore, MessageHistoryStore, ProjectDir, PromptAssemblyStore,
    ResolvedSessionState, SessionCatalogStore, SessionFlushStore, SessionHeader, SessionId,
    SessionLifecycleStore, SessionListOptions, SessionMeta, SessionStore, SessionStoreError,
    SessionTreeSnapshot, SessionTreeStore,
};

pub(super) struct LoadCountingSessionStore {
    inner: Arc<InMemorySessionStore>,
    load_session_calls: AtomicUsize,
}

pub(super) struct DelayedListSessionStore {
    inner: Arc<InMemorySessionStore>,
    list_started: Mutex<Option<mpsc::Sender<()>>>,
    list_release: Mutex<Option<mpsc::Receiver<()>>>,
    prompt_assembly_started: Mutex<Option<mpsc::Sender<()>>>,
    prompt_assembly_release: Mutex<Option<mpsc::Receiver<()>>>,
}

pub(super) struct FailingSessionStore {
    inner: Arc<InMemorySessionStore>,
    failed_load: FailingSessionStoreLoad,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum FailingSessionStoreLoad {
    SessionTree,
    BranchTree,
    BranchPreview,
    PromptAssemblyLoad,
    PromptAssemblySave,
}

impl FailingSessionStore {
    pub(super) fn new(
        inner: Arc<InMemorySessionStore>,
        failed_load: FailingSessionStoreLoad,
    ) -> Self {
        Self { inner, failed_load }
    }
}

impl DelayedListSessionStore {
    pub(super) fn new(
        inner: Arc<InMemorySessionStore>,
        list_started: mpsc::Sender<()>,
        list_release: mpsc::Receiver<()>,
    ) -> Self {
        Self {
            inner,
            list_started: Mutex::new(Some(list_started)),
            list_release: Mutex::new(Some(list_release)),
            prompt_assembly_started: Mutex::new(None),
            prompt_assembly_release: Mutex::new(None),
        }
    }

    pub(super) fn new_with_prompt_assembly_delay(
        inner: Arc<InMemorySessionStore>,
        prompt_assembly_started: mpsc::Sender<()>,
        prompt_assembly_release: mpsc::Receiver<()>,
    ) -> Self {
        Self {
            inner,
            list_started: Mutex::new(None),
            list_release: Mutex::new(None),
            prompt_assembly_started: Mutex::new(Some(prompt_assembly_started)),
            prompt_assembly_release: Mutex::new(Some(prompt_assembly_release)),
        }
    }
}

impl SessionLifecycleStore for FailingSessionStore {
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
}

impl SessionTreeStore for FailingSessionStore {
    fn load_session_tree<'a>(
        &'a self,
        session_id: &'a SessionId,
    ) -> Pin<Box<dyn Future<Output = Result<SessionTreeSnapshot, SessionStoreError>> + Send + 'a>>
    {
        if self.failed_load == FailingSessionStoreLoad::SessionTree {
            return Box::pin(async {
                Err(SessionStoreError::CorruptIndex {
                    message: "injected session tree load failure".to_string(),
                })
            });
        }
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
        if self.failed_load == FailingSessionStoreLoad::BranchPreview {
            return Box::pin(async {
                Err(SessionStoreError::CorruptIndex {
                    message: "injected branch preview load failure".to_string(),
                })
            });
        }
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
        if self.failed_load == FailingSessionStoreLoad::BranchTree {
            return Box::pin(async {
                Err(SessionStoreError::CorruptIndex {
                    message: "injected branch tree load failure".to_string(),
                })
            });
        }
        self.inner.load_session_branch_tree(session_id)
    }
}

impl SessionCatalogStore for FailingSessionStore {
    fn list_sessions<'a>(
        &'a self,
        project_dir: &'a ProjectDir,
        options: SessionListOptions,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<SessionMeta>, SessionStoreError>> + Send + 'a>>
    {
        self.inner.list_sessions(project_dir, options)
    }

    fn get_session_meta<'a>(
        &'a self,
        session_id: &'a SessionId,
    ) -> Pin<Box<dyn Future<Output = Result<SessionMeta, SessionStoreError>> + Send + 'a>> {
        self.inner.get_session_meta(session_id)
    }
}

impl SessionFlushStore for FailingSessionStore {
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

impl MessageHistoryStore for FailingSessionStore {
    fn record_message_history<'a>(
        &'a self,
        text: &'a str,
        limit: usize,
    ) -> Pin<Box<dyn Future<Output = Result<(), SessionStoreError>> + Send + 'a>> {
        self.inner.record_message_history(text, limit)
    }

    fn load_message_history_recent<'a>(
        &'a self,
        limit: usize,
    ) -> Pin<
        Box<dyn Future<Output = Result<Vec<MessageHistoryEntry>, SessionStoreError>> + Send + 'a>,
    > {
        self.inner.load_message_history_recent(limit)
    }

    fn load_message_history_all<'a>(
        &'a self,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<MessageHistoryRow>, SessionStoreError>> + Send + 'a>>
    {
        self.inner.load_message_history_all()
    }
}

impl PromptAssemblyStore for FailingSessionStore {
    fn save_global_prompt_assembly_state<'a>(
        &'a self,
        state: &'a PromptAssemblyScopeState,
    ) -> Pin<Box<dyn Future<Output = Result<(), SessionStoreError>> + Send + 'a>> {
        if self.failed_load == FailingSessionStoreLoad::PromptAssemblySave {
            return Box::pin(async {
                Err(SessionStoreError::CorruptIndex {
                    message: "injected prompt assembly save failure".to_string(),
                })
            });
        }
        self.inner.save_global_prompt_assembly_state(state)
    }

    fn load_global_prompt_assembly_state<'a>(
        &'a self,
    ) -> Pin<
        Box<dyn Future<Output = Result<PromptAssemblyScopeState, SessionStoreError>> + Send + 'a>,
    > {
        if self.failed_load == FailingSessionStoreLoad::PromptAssemblyLoad {
            return Box::pin(async {
                Err(SessionStoreError::CorruptIndex {
                    message: "injected prompt assembly load failure".to_string(),
                })
            });
        }
        self.inner.load_global_prompt_assembly_state()
    }
}

impl SessionStore for FailingSessionStore {}

impl SessionLifecycleStore for DelayedListSessionStore {
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
}

impl SessionTreeStore for DelayedListSessionStore {
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
}

impl SessionCatalogStore for DelayedListSessionStore {
    fn list_sessions<'a>(
        &'a self,
        project_dir: &'a ProjectDir,
        options: SessionListOptions,
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
                let receiver = self
                    .list_release
                    .lock()
                    .expect("list_release mutex should not be poisoned")
                    .take()
                    .expect("test should provide a first-list release signal");
                receiver.recv().expect("test should release delayed list");
            }
            self.inner.list_sessions(project_dir, options).await
        })
    }

    fn get_session_meta<'a>(
        &'a self,
        session_id: &'a SessionId,
    ) -> Pin<Box<dyn Future<Output = Result<SessionMeta, SessionStoreError>> + Send + 'a>> {
        self.inner.get_session_meta(session_id)
    }
}

impl SessionFlushStore for DelayedListSessionStore {
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

impl MessageHistoryStore for DelayedListSessionStore {
    fn record_message_history<'a>(
        &'a self,
        text: &'a str,
        limit: usize,
    ) -> Pin<Box<dyn Future<Output = Result<(), SessionStoreError>> + Send + 'a>> {
        self.inner.record_message_history(text, limit)
    }

    fn load_message_history_recent<'a>(
        &'a self,
        limit: usize,
    ) -> Pin<
        Box<dyn Future<Output = Result<Vec<MessageHistoryEntry>, SessionStoreError>> + Send + 'a>,
    > {
        self.inner.load_message_history_recent(limit)
    }

    fn load_message_history_all<'a>(
        &'a self,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<MessageHistoryRow>, SessionStoreError>> + Send + 'a>>
    {
        self.inner.load_message_history_all()
    }
}

impl PromptAssemblyStore for DelayedListSessionStore {
    fn save_global_prompt_assembly_state<'a>(
        &'a self,
        state: &'a PromptAssemblyScopeState,
    ) -> Pin<Box<dyn Future<Output = Result<(), SessionStoreError>> + Send + 'a>> {
        self.inner.save_global_prompt_assembly_state(state)
    }

    fn load_global_prompt_assembly_state<'a>(
        &'a self,
    ) -> Pin<
        Box<dyn Future<Output = Result<PromptAssemblyScopeState, SessionStoreError>> + Send + 'a>,
    > {
        Box::pin(async move {
            let started = self
                .prompt_assembly_started
                .lock()
                .expect("prompt_assembly_started mutex should not be poisoned")
                .take();
            if let Some(started) = started {
                let _ = started.send(());
                let release = self
                    .prompt_assembly_release
                    .lock()
                    .expect("prompt_assembly_release mutex should not be poisoned")
                    .take()
                    .expect("test should provide a prompt assembly release signal");
                release
                    .recv()
                    .expect("test should release delayed prompt assembly load");
            }
            self.inner.load_global_prompt_assembly_state().await
        })
    }
}

impl SessionStore for DelayedListSessionStore {}

impl LoadCountingSessionStore {
    pub(super) fn new(inner: Arc<InMemorySessionStore>) -> Self {
        Self {
            inner,
            load_session_calls: AtomicUsize::new(0),
        }
    }

    pub(super) fn load_session_calls(&self) -> usize {
        self.load_session_calls.load(Ordering::SeqCst)
    }
}

impl SessionLifecycleStore for LoadCountingSessionStore {
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
}

impl SessionTreeStore for LoadCountingSessionStore {
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
}

impl SessionCatalogStore for LoadCountingSessionStore {
    fn list_sessions<'a>(
        &'a self,
        project_dir: &'a ProjectDir,
        options: SessionListOptions,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<SessionMeta>, SessionStoreError>> + Send + 'a>>
    {
        self.inner.list_sessions(project_dir, options)
    }

    fn get_session_meta<'a>(
        &'a self,
        session_id: &'a SessionId,
    ) -> Pin<Box<dyn Future<Output = Result<SessionMeta, SessionStoreError>> + Send + 'a>> {
        self.inner.get_session_meta(session_id)
    }
}

impl SessionFlushStore for LoadCountingSessionStore {
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

impl MessageHistoryStore for LoadCountingSessionStore {
    fn record_message_history<'a>(
        &'a self,
        text: &'a str,
        limit: usize,
    ) -> Pin<Box<dyn Future<Output = Result<(), SessionStoreError>> + Send + 'a>> {
        self.inner.record_message_history(text, limit)
    }

    fn load_message_history_recent<'a>(
        &'a self,
        limit: usize,
    ) -> Pin<
        Box<dyn Future<Output = Result<Vec<MessageHistoryEntry>, SessionStoreError>> + Send + 'a>,
    > {
        self.inner.load_message_history_recent(limit)
    }

    fn load_message_history_all<'a>(
        &'a self,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<MessageHistoryRow>, SessionStoreError>> + Send + 'a>>
    {
        self.inner.load_message_history_all()
    }
}

impl PromptAssemblyStore for LoadCountingSessionStore {
    fn save_global_prompt_assembly_state<'a>(
        &'a self,
        state: &'a PromptAssemblyScopeState,
    ) -> Pin<Box<dyn Future<Output = Result<(), SessionStoreError>> + Send + 'a>> {
        self.inner.save_global_prompt_assembly_state(state)
    }

    fn load_global_prompt_assembly_state<'a>(
        &'a self,
    ) -> Pin<
        Box<dyn Future<Output = Result<PromptAssemblyScopeState, SessionStoreError>> + Send + 'a>,
    > {
        self.inner.load_global_prompt_assembly_state()
    }
}

impl SessionStore for LoadCountingSessionStore {}

pub(super) struct CommittedLoadFailsAfterSetLeafStore {
    inner: Arc<InMemorySessionStore>,
    fail_committed_load: AtomicBool,
}

impl CommittedLoadFailsAfterSetLeafStore {
    pub(super) fn new(inner: Arc<InMemorySessionStore>) -> Self {
        Self {
            inner,
            fail_committed_load: AtomicBool::new(false),
        }
    }
}

impl SessionLifecycleStore for CommittedLoadFailsAfterSetLeafStore {
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
}

impl SessionTreeStore for CommittedLoadFailsAfterSetLeafStore {
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
}

impl SessionCatalogStore for CommittedLoadFailsAfterSetLeafStore {
    fn list_sessions<'a>(
        &'a self,
        project_dir: &'a ProjectDir,
        options: SessionListOptions,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<SessionMeta>, SessionStoreError>> + Send + 'a>>
    {
        self.inner.list_sessions(project_dir, options)
    }

    fn get_session_meta<'a>(
        &'a self,
        session_id: &'a SessionId,
    ) -> Pin<Box<dyn Future<Output = Result<SessionMeta, SessionStoreError>> + Send + 'a>> {
        self.inner.get_session_meta(session_id)
    }
}

impl SessionFlushStore for CommittedLoadFailsAfterSetLeafStore {
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

impl MessageHistoryStore for CommittedLoadFailsAfterSetLeafStore {
    fn record_message_history<'a>(
        &'a self,
        text: &'a str,
        limit: usize,
    ) -> Pin<Box<dyn Future<Output = Result<(), SessionStoreError>> + Send + 'a>> {
        self.inner.record_message_history(text, limit)
    }

    fn load_message_history_recent<'a>(
        &'a self,
        limit: usize,
    ) -> Pin<
        Box<dyn Future<Output = Result<Vec<MessageHistoryEntry>, SessionStoreError>> + Send + 'a>,
    > {
        self.inner.load_message_history_recent(limit)
    }

    fn load_message_history_all<'a>(
        &'a self,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<MessageHistoryRow>, SessionStoreError>> + Send + 'a>>
    {
        self.inner.load_message_history_all()
    }
}

impl PromptAssemblyStore for CommittedLoadFailsAfterSetLeafStore {
    fn save_global_prompt_assembly_state<'a>(
        &'a self,
        state: &'a PromptAssemblyScopeState,
    ) -> Pin<Box<dyn Future<Output = Result<(), SessionStoreError>> + Send + 'a>> {
        self.inner.save_global_prompt_assembly_state(state)
    }

    fn load_global_prompt_assembly_state<'a>(
        &'a self,
    ) -> Pin<
        Box<dyn Future<Output = Result<PromptAssemblyScopeState, SessionStoreError>> + Send + 'a>,
    > {
        self.inner.load_global_prompt_assembly_state()
    }
}

impl SessionStore for CommittedLoadFailsAfterSetLeafStore {}
