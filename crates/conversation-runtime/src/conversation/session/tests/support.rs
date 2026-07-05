pub(super) use std::{
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

pub(super) use provider_protocol::{ContentBlock, ConversationItem, Role, ToolCall};
pub(super) use runtime_domain::{
    request_policy::RuntimeRequestPolicy,
    session::{
        RuntimeTarget, RuntimeTerminalSnapshot, RuntimeToolActivity, RuntimeToolActivityContent,
        RuntimeToolActivityRawValue, RuntimeToolActivityStatus, RuntimeToolActivityUpdate,
        RuntimeToolKind, TranscriptReplayItem,
    },
};
pub(super) use session_store::{
    LocalSessionStore, ProjectDir, SessionCatalogStore, SessionHeader, SessionId,
    SessionLifecycleStore, SessionListOptions, SessionStore, SessionStoreError, SessionTreeStore,
};
pub(super) use tokio::sync::mpsc as tokio_mpsc;
pub(super) use tokio_util::sync::CancellationToken;
pub(super) use tool_runtime::ToolExecutorRegistry;

pub(super) use super::super::persistence::{SessionPersistenceCommand, SessionPersistenceError};
pub(super) use super::super::{
    ConversationDelta, ConversationEvent, ConversationPermissionBroker, ConversationTimeoutPause,
    ConversationWorker, ConversationWorkerEvent, ProviderContextRepairLedger,
    SessionPersistenceState, TOOL_EXECUTION_TIMED_OUT, TurnAttemptOutcome,
    flush_session_persistence, persist_context_item, persist_terminal_snapshot,
    persist_tool_activity_started, persist_tool_activity_update, persist_turn_start,
    run_conversation_worker, run_session_persistence_actor, run_with_soft_timeout,
};
pub(super) use crate::{
    ConversationResponse, PreparedConversationRequest, PreparedTurnOptions, ProviderConversation,
    ProviderKind, conversation::PersistedConversationItem,
};

pub(super) fn run_store<T>(
    future: impl std::future::Future<Output = Result<T, SessionStoreError>>,
) -> Result<T, SessionStoreError> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("test runtime should build");
    runtime.block_on(future)
}

pub(super) fn run_persistence<T>(
    future: impl std::future::Future<Output = Result<T, SessionPersistenceError>>,
) -> Result<T, SessionPersistenceError> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("test runtime should build");
    runtime.block_on(future)
}

pub(super) fn sample_header(work_dir: &Path, model: &str) -> SessionHeader {
    SessionHeader {
        session_id: SessionId::new(),
        work_dir: work_dir.to_path_buf(),
        session_name: Some("worker-test".to_string()),
        initial_model: model.to_string(),
        git_head: Some("abc123".to_string()),
        cli_version: Some("0.5.7".to_string()),
    }
}

pub(super) fn tempdir_path(label: &str) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time should be after epoch")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "lumos-conversation-worker-{label}-{}-{stamp}",
        std::process::id()
    ))
}
