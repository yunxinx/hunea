//! 由 runtime host 持有的 session backend capability。

use std::sync::{Arc, Mutex, Weak};

use session_store::{
    MessageHistoryStore, PromptAssemblyStore, SessionCatalogStore, SessionFlushStore,
    SessionLifecycleStore, SessionPort, SessionStore, SessionTreeStore,
};

/// Agent 与 session worker 各自消费的窄 capability view。
#[derive(Clone)]
pub(super) struct SessionBackendViews {
    pub(super) port: Arc<dyn SessionPort>,
    pub(super) lifecycle: Arc<dyn SessionLifecycleStore>,
    pub(super) catalog: Arc<dyn SessionCatalogStore>,
    pub(super) tree: Arc<dyn SessionTreeStore>,
    pub(super) flush: Arc<dyn SessionFlushStore>,
    pub(super) message_history: Arc<dyn MessageHistoryStore>,
    pub(super) prompt_assembly: Arc<dyn PromptAssemblyStore>,
}

impl SessionBackendViews {
    fn from_store(store: Arc<dyn SessionStore>) -> Self {
        Self {
            port: store.clone(),
            lifecycle: store.clone(),
            catalog: store.clone(),
            tree: store.clone(),
            flush: store.clone(),
            message_history: store.clone(),
            prompt_assembly: store,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub(super) enum SessionPortError {
    #[error("session port is disposed")]
    Disposed,
    #[error("session backend {backend_id} is already registered")]
    DuplicateBackend { backend_id: String },
    #[error("session backend is not mounted")]
    BackendUnavailable,
}

struct SessionBackendEntry {
    _owner: String,
    backend_id: String,
    adapter_kind: String,
    registration_id: u64,
    store: Arc<dyn SessionStore>,
}

struct SessionPortState {
    is_active: bool,
    next_registration_id: u64,
    backend: Option<SessionBackendEntry>,
}

impl Default for SessionPortState {
    fn default() -> Self {
        Self {
            is_active: true,
            next_registration_id: 0,
            backend: None,
        }
    }
}

/// Runtime host 的 SessionPort capability。它只拥有当前 backend generation 的 slot。
#[derive(Clone, Default)]
pub(super) struct SessionPortHost {
    state: Arc<Mutex<SessionPortState>>,
}

impl SessionPortHost {
    pub(super) fn new() -> Self {
        Self::default()
    }

    /// 注册 backend。任何 duplicate 都在 identity 分配和 slot mutation 前拒绝。
    pub(super) fn register(
        &self,
        owner: impl Into<String>,
        backend_id: impl Into<String>,
        store: Arc<dyn SessionStore>,
    ) -> Result<SessionBackendRegistration, SessionPortError> {
        let owner = owner.into();
        let backend_id = backend_id.into();
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !state.is_active {
            return Err(SessionPortError::Disposed);
        }
        if state.backend.is_some() {
            return Err(SessionPortError::DuplicateBackend { backend_id });
        }
        let registration_id = state.next_registration_id;
        state.next_registration_id = state
            .next_registration_id
            .checked_add(1)
            .expect("session backend registration id space should be unreachable");
        state.backend = Some(SessionBackendEntry {
            _owner: owner,
            backend_id: backend_id.clone(),
            adapter_kind: "session-store".to_string(),
            registration_id,
            store,
        });
        drop(state);
        Ok(SessionBackendRegistration {
            state: Arc::downgrade(&self.state),
            backend_id,
            registration_id,
            is_disposed: false,
        })
    }

    pub(super) fn views(&self) -> Result<SessionBackendViews, SessionPortError> {
        let state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !state.is_active {
            return Err(SessionPortError::Disposed);
        }
        state
            .backend
            .as_ref()
            .map(|entry| SessionBackendViews::from_store(Arc::clone(&entry.store)))
            .ok_or(SessionPortError::BackendUnavailable)
    }

    /// 停止当前 generation 的新 view 解析和 inspection；registration inverse 独立保留。
    pub(super) fn deactivate(&self) {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_active = false;
    }

    pub(super) fn inspection_snapshot(&self) -> Option<SessionBackendSnapshot> {
        let state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !state.is_active {
            return None;
        }
        state.backend.as_ref().map(|entry| SessionBackendSnapshot {
            backend_id: entry.backend_id.clone(),
            adapter_kind: entry.adapter_kind.clone(),
            mounted: true,
        })
    }

    #[cfg(test)]
    fn remove_backend_for_stale_handle_test(&self) {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .backend = None;
    }
}

/// 脱敏的 backend inspection projection，不包含 owner、path 或 registration identity。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SessionBackendSnapshot {
    pub(super) backend_id: String,
    pub(super) adapter_kind: String,
    pub(super) mounted: bool,
}

/// 一个 backend registration 的幂等、stale-safe inverse。
#[derive(Debug)]
pub(super) struct SessionBackendRegistration {
    state: Weak<Mutex<SessionPortState>>,
    backend_id: String,
    registration_id: u64,
    is_disposed: bool,
}

impl SessionBackendRegistration {
    pub(super) fn dispose(&mut self) {
        if self.is_disposed {
            return;
        }
        self.is_disposed = true;
        let Some(state) = self.state.upgrade() else {
            return;
        };
        let mut state = state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let owns_current_backend = state.backend.as_ref().is_some_and(|entry| {
            entry.backend_id == self.backend_id && entry.registration_id == self.registration_id
        });
        if owns_current_backend {
            state.backend = None;
        }
    }
}

impl Drop for SessionBackendRegistration {
    fn drop(&mut self) {
        self.dispose();
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use session_store::{InMemorySessionStore, SessionHeader};

    use super::*;

    fn header() -> SessionHeader {
        SessionHeader {
            session_id: Default::default(),
            work_dir: PathBuf::from("/tmp/hunea-session-port"),
            session_name: None,
            initial_model: "fixture-model".to_string(),
            git_head: None,
            cli_version: None,
        }
    }

    #[tokio::test]
    async fn views_expose_agent_port_without_aggregate_store() {
        let host = SessionPortHost::new();
        let store: Arc<dyn SessionStore> = Arc::new(InMemorySessionStore::new());
        let _registration = host
            .register("fixture", "memory", store)
            .expect("backend should mount");
        let views = host.views().expect("views should resolve");
        let session_id = views
            .port
            .create_session(header())
            .await
            .expect("session should be created");
        views
            .flush
            .flush(&session_id)
            .await
            .expect("flush should succeed");
    }

    #[test]
    fn duplicate_is_rejected_before_mutation_and_drop_removes_the_backend() {
        let host = SessionPortHost::new();
        let first = host
            .register(
                "first-owner",
                "memory",
                Arc::new(InMemorySessionStore::new()),
            )
            .expect("first backend should mount");
        let error = host
            .register(
                "second-owner",
                "memory",
                Arc::new(InMemorySessionStore::new()),
            )
            .expect_err("duplicate backend should be rejected");
        assert_eq!(
            error,
            SessionPortError::DuplicateBackend {
                backend_id: "memory".to_string()
            }
        );
        drop(error);
        drop(first);

        assert!(matches!(
            host.views(),
            Err(SessionPortError::BackendUnavailable)
        ));
    }

    #[test]
    fn stale_registration_cannot_remove_a_fresh_generation() {
        let host = SessionPortHost::new();
        let mut stale = host
            .register(
                "stale-owner",
                "memory",
                Arc::new(InMemorySessionStore::new()),
            )
            .expect("first backend should mount");
        host.remove_backend_for_stale_handle_test();

        let mut fresh = host
            .register(
                "fresh-owner",
                "memory",
                Arc::new(InMemorySessionStore::new()),
            )
            .expect("fresh generation should mount");
        stale.dispose();
        assert!(host.views().is_ok());

        fresh.dispose();
        fresh.dispose();
        assert!(matches!(
            host.views(),
            Err(SessionPortError::BackendUnavailable)
        ));
    }

    #[test]
    fn deactivation_hides_views_and_inspection_but_inverse_remains_safe() {
        let host = SessionPortHost::new();
        let mut registration = host
            .register(
                "fixture-owner",
                "memory",
                Arc::new(InMemorySessionStore::new()),
            )
            .expect("backend should mount");
        assert_eq!(
            host.inspection_snapshot(),
            Some(SessionBackendSnapshot {
                backend_id: "memory".to_string(),
                adapter_kind: "session-store".to_string(),
                mounted: true,
            })
        );
        host.deactivate();
        assert!(matches!(host.views(), Err(SessionPortError::Disposed)));
        assert!(host.inspection_snapshot().is_none());
        registration.dispose();
        registration.dispose();
    }
}
