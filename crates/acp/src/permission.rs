use std::{
    collections::BTreeMap,
    fmt,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

pub use mo_core::acp::{
    AcpPermissionOption, AcpPermissionOptionKind, AcpPermissionRequest, AcpToolCallUpdate,
};

#[derive(Debug, Default, Clone)]
pub(crate) struct AcpPermissionRegistry {
    inner: Arc<AcpPermissionRegistryInner>,
}

#[derive(Debug, Default)]
struct AcpPermissionRegistryInner {
    next_id: AtomicUsize,
    pending: Mutex<BTreeMap<String, tokio::sync::oneshot::Sender<Option<String>>>>,
}

impl AcpPermissionRegistry {
    pub(crate) fn register(&self) -> (String, tokio::sync::oneshot::Receiver<Option<String>>) {
        let id = format!(
            "permission-{}",
            self.inner.next_id.fetch_add(1, Ordering::SeqCst)
        );
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.inner
            .pending
            .lock()
            .expect("ACP permission registry lock should not be poisoned")
            .insert(id.clone(), tx);
        (id, rx)
    }

    pub(crate) fn respond(
        &self,
        request_id: &str,
        option_id: Option<String>,
    ) -> Result<(), AcpPermissionRespondError> {
        let sender = self
            .inner
            .pending
            .lock()
            .expect("ACP permission registry lock should not be poisoned")
            .remove(request_id)
            .ok_or(AcpPermissionRespondError::NotFound)?;
        sender
            .send(option_id)
            .map_err(|_| AcpPermissionRespondError::Closed)
    }
}

/// `AcpPermissionRespondError` 描述权限确认回传失败。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcpPermissionRespondError {
    NotFound,
    Closed,
}

impl fmt::Display for AcpPermissionRespondError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound => write!(f, "ACP permission request not found"),
            Self::Closed => write!(f, "ACP permission request is closed"),
        }
    }
}

impl std::error::Error for AcpPermissionRespondError {}

pub(crate) fn acp_permission_request_from_sdk(
    request_id: String,
    request: &agent_client_protocol::schema::RequestPermissionRequest,
    tool_call: AcpToolCallUpdate,
) -> AcpPermissionRequest {
    AcpPermissionRequest {
        request_id,
        title: tool_call.title.clone(),
        tool_call,
        options: request
            .options
            .iter()
            .map(|option| AcpPermissionOption {
                option_id: option.option_id.to_string(),
                name: option.name.clone(),
                kind: acp_permission_option_kind(option.kind),
            })
            .collect(),
    }
}

fn acp_permission_option_kind(
    kind: agent_client_protocol::schema::PermissionOptionKind,
) -> AcpPermissionOptionKind {
    use agent_client_protocol::schema::PermissionOptionKind;

    match kind {
        PermissionOptionKind::AllowOnce => AcpPermissionOptionKind::AllowOnce,
        PermissionOptionKind::AllowAlways => AcpPermissionOptionKind::AllowAlways,
        PermissionOptionKind::RejectOnce => AcpPermissionOptionKind::RejectOnce,
        PermissionOptionKind::RejectAlways => AcpPermissionOptionKind::RejectAlways,
        _ => AcpPermissionOptionKind::Unknown,
    }
}
