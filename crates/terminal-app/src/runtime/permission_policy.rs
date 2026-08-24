//! runtime host 拥有的 approval provider registry 与 session permission policy。

use std::{
    collections::{BTreeMap, HashMap},
    fmt,
    sync::{Arc, Mutex, Weak, mpsc},
};

#[cfg(test)]
use conversation_runtime::RuntimeEventNotifier;
use runtime_domain::session::{
    RuntimePermissionOption, RuntimePermissionOptionKind, RuntimePermissionRequest,
};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;
use tool_loop_runtime::runtime_tool_activity_update_from_permission_request;
use tool_runtime::{
    SharedToolPermissionHandler, ToolPermissionDecision, ToolPermissionFuture,
    ToolPermissionHandler, ToolPermissionRequest, ToolPermissionRule, ToolPermissionRuleBehavior,
    ToolPermissionRuleSet,
};

use super::context::{CapabilityLease, RuntimeEventStreamCapability};

pub(super) const TERMINAL_APPROVAL_PROVIDER_ID: &str = "terminal-interactive";

const PERMISSION_REQUEST_PREFIX: &str = "runtime-permission";
const ALLOW_ONCE_OPTION_ID: &str = "allow_once";
const ALLOW_ALWAYS_OPTION_ID: &str = "allow_always";
const REJECT_ONCE_OPTION_ID: &str = "reject_once";
const REJECT_ALWAYS_OPTION_ID: &str = "reject_always";
const TOOL_PERMISSION_DENIED: &str = "Tool permission denied";
const USER_REJECTED_TOOL_CALL: &str = "user rejected the tool call";

type ApprovalFuture<'a> =
    std::pin::Pin<Box<dyn std::future::Future<Output = Option<String>> + Send + 'a>>;
type PermissionResponseSender = oneshot::Sender<Option<String>>;

/// `PermissionPolicyError` 只携带可安全投影的 provider metadata。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub(super) enum PermissionPolicyError {
    #[error("approval provider {provider_id} is already registered")]
    DuplicateProvider { provider_id: String },
    #[error("unknown approval provider {provider_id}")]
    UnknownProvider { provider_id: String },
    #[error("approval provider {provider_id} is unavailable")]
    ProviderUnavailable { provider_id: String },
    #[error("permission response is no longer pending")]
    ResponseUnavailable,
    #[error("permission policy is disposed")]
    Disposed,
}

/// approval adapter 内部错误不得携带 request delivery 或 response id。
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub(super) enum ApprovalProviderError {
    #[error("approval provider is unavailable")]
    Unavailable,
}

/// 每个 turn 的 approval adapter 构造边界。
pub(super) trait ApprovalProviderFactory: Send + Sync {
    fn open(
        &self,
        event_stream: CapabilityLease<RuntimeEventStreamCapability>,
    ) -> Result<Arc<dyn ApprovalProvider>, ApprovalProviderError>;

    fn adapter_kind(&self) -> &'static str;
}

/// turn-local approval delivery/response adapter。
pub(super) trait ApprovalProvider: Send + Sync {
    fn request<'a>(
        &'a self,
        request: RuntimePermissionRequest,
        cancellation: &'a CancellationToken,
    ) -> ApprovalFuture<'a>;

    fn respond(
        &self,
        request_id: &str,
        option_id: Option<String>,
    ) -> Result<(), ApprovalProviderError>;

    fn try_recv_request(&self) -> Option<RuntimePermissionRequest>;

    fn cancel_pending(&self);
}

struct ApprovalProviderEntry {
    _owner: String,
    registration_id: u64,
    adapter_kind: String,
    factory: Arc<dyn ApprovalProviderFactory>,
}

struct PermissionPolicyState {
    is_active: bool,
    next_registration_id: u64,
    next_request_id: u64,
    context_generation: u64,
    rules: ToolPermissionRuleSet,
    providers: BTreeMap<String, ApprovalProviderEntry>,
    active_turns: Vec<Weak<dyn ApprovalProvider>>,
}

impl Default for PermissionPolicyState {
    fn default() -> Self {
        Self {
            is_active: true,
            next_registration_id: 0,
            next_request_id: 0,
            context_generation: 0,
            rules: ToolPermissionRuleSet::default(),
            providers: BTreeMap::new(),
            active_turns: Vec::new(),
        }
    }
}

/// approval provider capability 的脱敏 inspection projection。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ApprovalProviderSnapshot {
    pub(super) provider_id: String,
    pub(super) adapter_kind: String,
}

/// `PermissionPolicy` 是 session rules 与 approval provider 的唯一 live authority。
#[derive(Clone)]
pub(super) struct PermissionPolicy {
    state: Arc<Mutex<PermissionPolicyState>>,
    event_stream: Arc<Mutex<Option<CapabilityLease<RuntimeEventStreamCapability>>>>,
}

impl PermissionPolicy {
    pub(super) fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(PermissionPolicyState::default())),
            event_stream: Arc::new(Mutex::new(None)),
        }
    }

    #[cfg(test)]
    pub(super) fn new_for_test(notifier: RuntimeEventNotifier) -> Self {
        Self {
            state: Arc::new(Mutex::new(PermissionPolicyState::default())),
            event_stream: Arc::new(Mutex::new(Some(
                crate::runtime::context::RuntimeContext::test_event_stream_lease(notifier),
            ))),
        }
    }

    /// 绑定由当前 component dependency 解析出的 event stream generation。
    pub(super) fn bind_event_stream(
        &self,
        event_stream: CapabilityLease<RuntimeEventStreamCapability>,
    ) {
        *lock(&self.event_stream) = Some(event_stream);
        lock(&self.state).is_active = true;
    }

    #[cfg(test)]
    pub(super) fn is_active_for_test(&self) -> bool {
        lock(&self.state).is_active
    }

    #[cfg(test)]
    pub(super) fn context_generation_for_test(&self) -> u64 {
        lock(&self.state).context_generation
    }

    /// 注册一个 approval provider；duplicate 在 identity 分配与 mutation 前拒绝。
    pub(super) fn register(
        &self,
        owner: impl Into<String>,
        provider_id: impl Into<String>,
        factory: Arc<dyn ApprovalProviderFactory>,
    ) -> Result<ApprovalProviderRegistration, PermissionPolicyError> {
        let owner = owner.into();
        let provider_id = provider_id.into();
        let mut state = lock(&self.state);
        if !state.is_active {
            return Err(PermissionPolicyError::Disposed);
        }
        if state.providers.contains_key(&provider_id) {
            return Err(PermissionPolicyError::DuplicateProvider { provider_id });
        }

        let registration_id = state.next_registration_id;
        state.next_registration_id = state
            .next_registration_id
            .checked_add(1)
            .expect("approval provider registration id space should be unreachable");
        state.providers.insert(
            provider_id.clone(),
            ApprovalProviderEntry {
                _owner: owner,
                registration_id,
                adapter_kind: factory.adapter_kind().to_string(),
                factory,
            },
        );
        drop(state);

        Ok(ApprovalProviderRegistration {
            state: Arc::downgrade(&self.state),
            provider_id,
            registration_id,
            is_disposed: false,
        })
    }

    /// 为一个 Agent turn 打开独立 provider instance 与共享 tool handler。
    pub(super) fn begin_turn(
        &self,
        provider_id: &str,
    ) -> Result<PermissionTurn, PermissionPolicyError> {
        let (generation, registration_id, factory) = {
            let state = lock(&self.state);
            if !state.is_active {
                return Err(PermissionPolicyError::Disposed);
            }
            let entry = state.providers.get(provider_id).ok_or_else(|| {
                PermissionPolicyError::UnknownProvider {
                    provider_id: provider_id.to_string(),
                }
            })?;
            (
                state.context_generation,
                entry.registration_id,
                Arc::clone(&entry.factory),
            )
        };

        let event_stream = self.event_stream_lease().ok_or_else(|| {
            PermissionPolicyError::ProviderUnavailable {
                provider_id: provider_id.to_string(),
            }
        })?;
        let provider =
            factory
                .open(event_stream)
                .map_err(|_| PermissionPolicyError::ProviderUnavailable {
                    provider_id: provider_id.to_string(),
                })?;

        let mut state = lock(&self.state);
        let provider_is_current = state
            .providers
            .get(provider_id)
            .is_some_and(|entry| entry.registration_id == registration_id);
        if !state.is_active || state.context_generation != generation || !provider_is_current {
            drop(state);
            provider.cancel_pending();
            return Err(PermissionPolicyError::ProviderUnavailable {
                provider_id: provider_id.to_string(),
            });
        }
        state.active_turns.retain(|turn| turn.strong_count() > 0);
        state.active_turns.push(Arc::downgrade(&provider));
        drop(state);

        let turn_cancellation = CancellationToken::new();
        let handler: SharedToolPermissionHandler = Arc::new(PolicyPermissionHandler {
            state: Arc::clone(&self.state),
            provider: Arc::clone(&provider),
            generation,
            turn_cancellation: turn_cancellation.clone(),
        });
        Ok(PermissionTurn {
            state: Arc::downgrade(&self.state),
            provider,
            handler,
            generation,
            turn_cancellation,
            is_cancelled: false,
        })
    }

    /// 清空 conversation replacement 后不得继续复用的 approval context。
    pub(super) fn clear_context(&self) {
        let turns = {
            let mut state = lock(&self.state);
            state.context_generation = state
                .context_generation
                .checked_add(1)
                .expect("permission context generation space should be unreachable");
            state.rules.clear();
            std::mem::take(&mut state.active_turns)
        };
        cancel_turns(turns);
    }

    /// 停止当前 generation 的 mutation、turn creation 与 inspection。
    pub(super) fn deactivate(&self) {
        let turns = {
            let mut state = lock(&self.state);
            if !state.is_active {
                return;
            }
            state.is_active = false;
            state.context_generation = state
                .context_generation
                .checked_add(1)
                .expect("permission context generation space should be unreachable");
            state.rules.clear();
            std::mem::take(&mut state.active_turns)
        };
        lock(&self.event_stream).take();
        cancel_turns(turns);
    }

    pub(super) fn inspection_snapshot(&self) -> Vec<ApprovalProviderSnapshot> {
        let state = lock(&self.state);
        if !state.is_active {
            return Vec::new();
        }
        state
            .providers
            .iter()
            .map(|(provider_id, entry)| ApprovalProviderSnapshot {
                provider_id: provider_id.clone(),
                adapter_kind: entry.adapter_kind.clone(),
            })
            .collect()
    }

    fn event_stream_lease(&self) -> Option<CapabilityLease<RuntimeEventStreamCapability>> {
        lock(&self.event_stream).clone()
    }
}

impl fmt::Debug for PermissionPolicy {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let state = lock(&self.state);
        formatter
            .debug_struct("PermissionPolicy")
            .field("is_active", &state.is_active)
            .field("provider_count", &state.providers.len())
            .field("has_rules", &(!state.rules.is_empty()))
            .finish()
    }
}

fn cancel_turns(turns: Vec<Weak<dyn ApprovalProvider>>) {
    for provider in turns.into_iter().filter_map(|provider| provider.upgrade()) {
        provider.cancel_pending();
    }
}

/// 一次 approval provider registration 的幂等、stale-safe inverse。
pub(super) struct ApprovalProviderRegistration {
    state: Weak<Mutex<PermissionPolicyState>>,
    provider_id: String,
    registration_id: u64,
    is_disposed: bool,
}

impl ApprovalProviderRegistration {
    pub(super) fn dispose(&mut self) {
        if self.is_disposed {
            return;
        }
        self.is_disposed = true;
        let Some(state) = self.state.upgrade() else {
            return;
        };
        let mut state = lock(&state);
        let owns_current_entry = state
            .providers
            .get(&self.provider_id)
            .is_some_and(|entry| entry.registration_id == self.registration_id);
        if owns_current_entry {
            state.providers.remove(&self.provider_id);
        }
    }
}

impl fmt::Debug for ApprovalProviderRegistration {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ApprovalProviderRegistration")
            .field("provider_id", &self.provider_id)
            .field("is_disposed", &self.is_disposed)
            .finish_non_exhaustive()
    }
}

impl Drop for ApprovalProviderRegistration {
    fn drop(&mut self) {
        self.dispose();
    }
}

/// Native-owned turn handle；Drop 只撤销本 turn 的 pending approval。
pub(super) struct PermissionTurn {
    state: Weak<Mutex<PermissionPolicyState>>,
    provider: Arc<dyn ApprovalProvider>,
    handler: SharedToolPermissionHandler,
    generation: u64,
    turn_cancellation: CancellationToken,
    is_cancelled: bool,
}

impl PermissionTurn {
    pub(super) fn handler(&self) -> SharedToolPermissionHandler {
        Arc::clone(&self.handler)
    }

    pub(super) fn try_recv_request(&self) -> Option<RuntimePermissionRequest> {
        if self.is_current() {
            self.provider.try_recv_request()
        } else {
            None
        }
    }

    pub(super) fn respond(
        &self,
        request_id: &str,
        option_id: Option<String>,
    ) -> Result<(), PermissionPolicyError> {
        if !self.is_current() {
            return Err(PermissionPolicyError::ResponseUnavailable);
        }
        self.provider
            .respond(request_id, option_id)
            .map_err(|_| PermissionPolicyError::ResponseUnavailable)
    }

    pub(super) fn cancel_pending(&mut self) {
        if self.is_cancelled {
            return;
        }
        self.is_cancelled = true;
        self.turn_cancellation.cancel();
        self.provider.cancel_pending();
    }

    fn is_current(&self) -> bool {
        self.state.upgrade().is_some_and(|state| {
            let state = lock(&state);
            state.is_active
                && state.context_generation == self.generation
                && !self.is_cancelled
                && !self.turn_cancellation.is_cancelled()
        })
    }
}

impl fmt::Debug for PermissionTurn {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PermissionTurn")
            .field("is_current", &self.is_current())
            .field("is_cancelled", &self.is_cancelled)
            .finish()
    }
}

impl Drop for PermissionTurn {
    fn drop(&mut self) {
        self.cancel_pending();
    }
}

struct PolicyPermissionHandler {
    state: Arc<Mutex<PermissionPolicyState>>,
    provider: Arc<dyn ApprovalProvider>,
    generation: u64,
    turn_cancellation: CancellationToken,
}

impl fmt::Debug for PolicyPermissionHandler {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PolicyPermissionHandler")
            .field("generation_is_current", &self.generation_is_current())
            .finish()
    }
}

impl PolicyPermissionHandler {
    fn generation_is_current(&self) -> bool {
        let state = lock(&self.state);
        state.is_active
            && state.context_generation == self.generation
            && !self.turn_cancellation.is_cancelled()
    }

    fn evaluate_rule(&self, request: &ToolPermissionRequest) -> Option<ToolPermissionRuleBehavior> {
        let state = lock(&self.state);
        if !state.is_active
            || state.context_generation != self.generation
            || self.turn_cancellation.is_cancelled()
        {
            return None;
        }
        state.rules.evaluate(request)
    }

    fn allocate_request_id(&self) -> Option<String> {
        let mut state = lock(&self.state);
        if !state.is_active
            || state.context_generation != self.generation
            || self.turn_cancellation.is_cancelled()
        {
            return None;
        }
        let request_id = state.next_request_id;
        state.next_request_id = state
            .next_request_id
            .checked_add(1)
            .expect("permission request id space should be unreachable");
        Some(format!("{PERMISSION_REQUEST_PREFIX}-{}", request_id + 1))
    }

    fn insert_rule_if_current(
        &self,
        cancellation: &CancellationToken,
        rule: ToolPermissionRule,
    ) -> bool {
        let mut state = lock(&self.state);
        if cancellation.is_cancelled()
            || !state.is_active
            || state.context_generation != self.generation
            || self.turn_cancellation.is_cancelled()
        {
            return false;
        }
        state.rules.insert(rule);
        true
    }
}

impl ToolPermissionHandler for PolicyPermissionHandler {
    fn request_permission<'a>(
        &'a self,
        request: ToolPermissionRequest,
        cancellation: &'a CancellationToken,
    ) -> ToolPermissionFuture<'a> {
        Box::pin(async move {
            if cancellation.is_cancelled() || !self.generation_is_current() {
                return deny_permission(&request.definition.name, "permission request cancelled");
            }

            let stored_behavior = self.evaluate_rule(&request);
            if cancellation.is_cancelled() || !self.generation_is_current() {
                return deny_permission(&request.definition.name, USER_REJECTED_TOOL_CALL);
            }
            if let Some(behavior) = stored_behavior {
                return decision_for_rule(&request.definition.name, behavior);
            }

            let Some(request_id) = self.allocate_request_id() else {
                return deny_permission(&request.definition.name, USER_REJECTED_TOOL_CALL);
            };
            let session_rule =
                ToolPermissionRule::from_request(&request, ToolPermissionRuleBehavior::Allow);
            let options = permission_options(session_rule.is_some());
            let delivery = permission_request(&request_id, &request, options.clone());
            let option_id = self.provider.request(delivery, cancellation).await;

            if cancellation.is_cancelled() || !self.generation_is_current() {
                return deny_permission(&request.definition.name, USER_REJECTED_TOOL_CALL);
            }

            let selected_kind = option_id.as_deref().and_then(|option_id| {
                options
                    .iter()
                    .find(|option| option.option_id == option_id)
                    .map(|option| option.kind)
            });
            match selected_kind {
                Some(RuntimePermissionOptionKind::AllowOnce) => ToolPermissionDecision::Allow,
                Some(RuntimePermissionOptionKind::AllowAlways) => {
                    if let Some(rule) = session_rule
                        && !self.insert_rule_if_current(cancellation, rule)
                    {
                        return deny_permission(&request.definition.name, USER_REJECTED_TOOL_CALL);
                    }
                    ToolPermissionDecision::Allow
                }
                Some(RuntimePermissionOptionKind::RejectOnce) => {
                    deny_permission(&request.definition.name, USER_REJECTED_TOOL_CALL)
                }
                Some(RuntimePermissionOptionKind::RejectAlways) => {
                    if let Some(rule) = session_rule {
                        self.insert_rule_if_current(
                            cancellation,
                            rule.with_behavior(ToolPermissionRuleBehavior::Deny),
                        );
                    }
                    deny_permission(&request.definition.name, USER_REJECTED_TOOL_CALL)
                }
                _ => deny_permission(&request.definition.name, USER_REJECTED_TOOL_CALL),
            }
        })
    }
}

fn permission_request(
    request_id: &str,
    request: &ToolPermissionRequest,
    options: Vec<RuntimePermissionOption>,
) -> RuntimePermissionRequest {
    let tool_activity =
        runtime_tool_activity_update_from_permission_request(&request.call.call_id, request);
    RuntimePermissionRequest::new(request_id.to_string(), tool_activity.title.clone(), options)
        .with_tool_activity(tool_activity)
}

fn permission_options(can_remember: bool) -> Vec<RuntimePermissionOption> {
    let mut options = vec![RuntimePermissionOption::new(
        ALLOW_ONCE_OPTION_ID,
        "Yes",
        RuntimePermissionOptionKind::AllowOnce,
    )];
    if can_remember {
        options.push(RuntimePermissionOption::new(
            ALLOW_ALWAYS_OPTION_ID,
            "Yes, allow similar requests during this session",
            RuntimePermissionOptionKind::AllowAlways,
        ));
    }
    options.push(RuntimePermissionOption::new(
        REJECT_ONCE_OPTION_ID,
        "No",
        RuntimePermissionOptionKind::RejectOnce,
    ));
    if can_remember {
        options.push(RuntimePermissionOption::new(
            REJECT_ALWAYS_OPTION_ID,
            "No, reject similar requests during this session",
            RuntimePermissionOptionKind::RejectAlways,
        ));
    }
    options
}

fn decision_for_rule(
    tool_name: &str,
    behavior: ToolPermissionRuleBehavior,
) -> ToolPermissionDecision {
    match behavior {
        ToolPermissionRuleBehavior::Allow => ToolPermissionDecision::Allow,
        ToolPermissionRuleBehavior::Deny => {
            deny_permission(tool_name, "a stored permission rule rejected the tool call")
        }
    }
}

fn deny_permission(tool_name: &str, reason: &str) -> ToolPermissionDecision {
    ToolPermissionDecision::Deny {
        message: format!("{TOOL_PERMISSION_DENIED}: {tool_name} {reason}"),
    }
}

/// 当前 TUI approval delivery 的 built-in turn factory。
#[derive(Debug, Default)]
pub(super) struct InteractiveApprovalProviderFactory;

impl ApprovalProviderFactory for InteractiveApprovalProviderFactory {
    fn open(
        &self,
        event_stream: CapabilityLease<RuntimeEventStreamCapability>,
    ) -> Result<Arc<dyn ApprovalProvider>, ApprovalProviderError> {
        Ok(Arc::new(InteractiveApprovalProvider::new(event_stream)))
    }

    fn adapter_kind(&self) -> &'static str {
        "terminal-interactive"
    }
}

#[derive(Default)]
struct InteractiveApprovalState {
    is_cancelled: bool,
    pending: HashMap<String, PermissionResponseSender>,
}

struct InteractiveApprovalProvider {
    state: Mutex<InteractiveApprovalState>,
    request_sender: mpsc::Sender<RuntimePermissionRequest>,
    request_receiver: Mutex<mpsc::Receiver<RuntimePermissionRequest>>,
    event_stream: CapabilityLease<RuntimeEventStreamCapability>,
}

impl InteractiveApprovalProvider {
    fn new(event_stream: CapabilityLease<RuntimeEventStreamCapability>) -> Self {
        let (request_sender, request_receiver) = mpsc::channel();
        Self {
            state: Mutex::new(InteractiveApprovalState::default()),
            request_sender,
            request_receiver: Mutex::new(request_receiver),
            event_stream,
        }
    }

    fn remove_pending(&self, request_id: &str) {
        lock(&self.state).pending.remove(request_id);
    }
}

impl ApprovalProvider for InteractiveApprovalProvider {
    fn request<'a>(
        &'a self,
        request: RuntimePermissionRequest,
        cancellation: &'a CancellationToken,
    ) -> ApprovalFuture<'a> {
        Box::pin(async move {
            if cancellation.is_cancelled() {
                return None;
            }
            let request_id = request.request_id.clone();
            let (response_sender, response_receiver) = oneshot::channel();
            {
                let mut state = lock(&self.state);
                if state.is_cancelled || cancellation.is_cancelled() {
                    return None;
                }
                state.pending.insert(request_id.clone(), response_sender);
                if self.request_sender.send(request).is_err() {
                    state.pending.remove(&request_id);
                    return None;
                }
            }
            self.event_stream.notify();

            let option_id = tokio::select! {
                biased;
                _ = cancellation.cancelled() => None,
                response = response_receiver => response.ok().flatten(),
            };
            self.remove_pending(&request_id);
            option_id
        })
    }

    fn respond(
        &self,
        request_id: &str,
        option_id: Option<String>,
    ) -> Result<(), ApprovalProviderError> {
        let sender = lock(&self.state)
            .pending
            .remove(request_id)
            .ok_or(ApprovalProviderError::Unavailable)?;
        sender
            .send(option_id)
            .map_err(|_| ApprovalProviderError::Unavailable)
    }

    fn try_recv_request(&self) -> Option<RuntimePermissionRequest> {
        lock(&self.request_receiver).try_recv().ok()
    }

    fn cancel_pending(&self) {
        let pending = {
            let mut state = lock(&self.state);
            state.is_cancelled = true;
            std::mem::take(&mut state.pending)
        };
        for (_, sender) in pending {
            let _ = sender.send(None);
        }
        let receiver = lock(&self.request_receiver);
        while receiver.try_recv().is_ok() {}
    }
}

impl fmt::Debug for InteractiveApprovalProvider {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let state = lock(&self.state);
        formatter
            .debug_struct("InteractiveApprovalProvider")
            .field("is_cancelled", &state.is_cancelled)
            .field("pending_count", &state.pending.len())
            .finish()
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests;
