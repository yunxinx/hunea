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
use runtime_domain::session::{MessageHistoryEntry, MessageHistoryRow, TranscriptReplayItem};
use session_store::{
    ConfigSnapshot, InMemorySessionStore, ProjectDir, ResolvedSessionState, SessionHeader,
    SessionId, SessionListOptions, SessionMeta, SessionStore, SessionStoreError,
    SessionTreeSnapshot,
};

pub(super) struct LoadCountingSessionStore {
    inner: Arc<InMemorySessionStore>,
    load_session_calls: AtomicUsize,
}

pub(super) struct DelayedListSessionStore {
    inner: Arc<InMemorySessionStore>,
    list_started: Mutex<Option<mpsc::Sender<()>>>,
    list_release: Mutex<Option<mpsc::Receiver<()>>>,
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
        }
    }
}

impl SessionStore for FailingSessionStore {
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

    fn record_message_history<'a>(
        &'a self,
        text: String,
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

    fn record_message_history<'a>(
        &'a self,
        text: String,
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

    fn record_message_history<'a>(
        &'a self,
        text: String,
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

    fn record_message_history<'a>(
        &'a self,
        text: String,
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
