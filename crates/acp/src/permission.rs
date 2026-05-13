use std::{
    collections::BTreeMap,
    fmt,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

use super::AcpToolCallUpdate;
use mo_core::session::{
    RuntimePermissionOption, RuntimePermissionOptionKind, RuntimePermissionRequest,
};

/// `AcpPermissionRequest` 是传给 TUI 的 ACP 权限确认请求。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcpPermissionRequest {
    pub request_id: String,
    pub title: Option<String>,
    pub tool_call: AcpToolCallUpdate,
    pub options: Vec<AcpPermissionOption>,
}

/// `AcpPermissionOption` 描述权限确认里用户可选择的一项。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcpPermissionOption {
    pub option_id: String,
    pub name: String,
    pub kind: AcpPermissionOptionKind,
}

/// `AcpPermissionOptionKind` 用于 TUI 选择默认允许/拒绝选项。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcpPermissionOptionKind {
    AllowOnce,
    AllowAlways,
    RejectOnce,
    RejectAlways,
    Unknown,
}

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

impl From<AcpPermissionRequest> for RuntimePermissionRequest {
    fn from(request: AcpPermissionRequest) -> Self {
        RuntimePermissionRequest::new(
            request.request_id,
            request.title,
            request
                .options
                .into_iter()
                .map(RuntimePermissionOption::from)
                .collect(),
        )
    }
}

impl From<AcpPermissionOption> for RuntimePermissionOption {
    fn from(option: AcpPermissionOption) -> Self {
        RuntimePermissionOption::new(option.option_id, option.name, option.kind.into())
    }
}

impl From<AcpPermissionOptionKind> for RuntimePermissionOptionKind {
    fn from(kind: AcpPermissionOptionKind) -> Self {
        match kind {
            AcpPermissionOptionKind::AllowOnce => Self::AllowOnce,
            AcpPermissionOptionKind::AllowAlways => Self::AllowAlways,
            AcpPermissionOptionKind::RejectOnce => Self::RejectOnce,
            AcpPermissionOptionKind::RejectAlways => Self::RejectAlways,
            AcpPermissionOptionKind::Unknown => Self::Unknown,
        }
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
