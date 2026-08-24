use std::{
    collections::BTreeMap,
    fmt,
    future::Future,
    sync::{Arc, Mutex, Weak},
};

use tokio_util::sync::CancellationToken;

use crate::{
    AfterToolResultDecision, AfterToolResultHook, AfterToolResultPayload,
    BeforeToolExecuteDecision, BeforeToolExecuteHook, BeforeToolExecutePayload, BeforeTurnDecision,
    BeforeTurnHook, BeforeTurnPayload, HookDispatchError, HookDispatchErrorKind, HookFailureKind,
    HookId, HookOwnerId, HookPhase, HookRegistrationOptions,
};

/// Thread-safe typed hook registry。
#[derive(Clone, Default)]
pub struct ExtensionHookRegistry {
    state: Arc<Mutex<RegistryState>>,
}

#[derive(Default)]
struct RegistryState {
    next_registration_id: u64,
    registrations: BTreeMap<RegistrationKey, StoredRegistration>,
}

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
struct RegistrationKey {
    phase: HookPhase,
    owner: HookOwnerId,
    hook_id: HookId,
}

struct StoredRegistration {
    registration_id: u64,
    options: HookRegistrationOptions,
    cancellation: CancellationToken,
    hook: StoredHook,
}

enum StoredHook {
    BeforeTurn(Arc<dyn BeforeTurnHook>),
    BeforeToolExecute(Arc<dyn BeforeToolExecuteHook>),
    AfterToolResult(Arc<dyn AfterToolResultHook>),
}

#[derive(Clone)]
struct DispatchEntry {
    key: RegistrationKey,
    options: HookRegistrationOptions,
    cancellation: CancellationToken,
    hook: DispatchHook,
}

#[derive(Clone)]
enum DispatchHook {
    BeforeTurn(Arc<dyn BeforeTurnHook>),
    BeforeToolExecute(Arc<dyn BeforeToolExecuteHook>),
    AfterToolResult(Arc<dyn AfterToolResultHook>),
}

impl ExtensionHookRegistry {
    /// 创建空 registry；所有 dispatch 都保持 identity/allow。
    pub fn new() -> Self {
        Self::default()
    }

    /// 注册一个 owner-scoped `before_turn` hook。
    pub fn register_before_turn(
        &self,
        owner: HookOwnerId,
        hook_id: HookId,
        options: HookRegistrationOptions,
        hook: Arc<dyn BeforeTurnHook>,
    ) -> Result<HookRegistration, HookRegistrationError> {
        self.register(
            HookPhase::BeforeTurn,
            owner,
            hook_id,
            options,
            StoredHook::BeforeTurn(hook),
        )
    }

    /// 注册一个 owner-scoped `before_tool_execute` hook。
    pub fn register_before_tool_execute(
        &self,
        owner: HookOwnerId,
        hook_id: HookId,
        options: HookRegistrationOptions,
        hook: Arc<dyn BeforeToolExecuteHook>,
    ) -> Result<HookRegistration, HookRegistrationError> {
        self.register(
            HookPhase::BeforeToolExecute,
            owner,
            hook_id,
            options,
            StoredHook::BeforeToolExecute(hook),
        )
    }

    /// 注册一个 owner-scoped `after_tool_result` hook。
    pub fn register_after_tool_result(
        &self,
        owner: HookOwnerId,
        hook_id: HookId,
        options: HookRegistrationOptions,
        hook: Arc<dyn AfterToolResultHook>,
    ) -> Result<HookRegistration, HookRegistrationError> {
        self.register(
            HookPhase::AfterToolResult,
            owner,
            hook_id,
            options,
            StoredHook::AfterToolResult(hook),
        )
    }

    fn register(
        &self,
        phase: HookPhase,
        owner: HookOwnerId,
        hook_id: HookId,
        options: HookRegistrationOptions,
        hook: StoredHook,
    ) -> Result<HookRegistration, HookRegistrationError> {
        let key = RegistrationKey {
            phase,
            owner,
            hook_id,
        };
        let mut state = lock_state(&self.state);
        if state.registrations.contains_key(&key) {
            return Err(HookRegistrationError::Duplicate {
                phase,
                owner: key.owner.clone(),
                hook_id: key.hook_id.clone(),
            });
        }
        let registration_id = state.next_registration_id;
        state.next_registration_id = registration_id
            .checked_add(1)
            .ok_or(HookRegistrationError::RegistrationIdentityExhausted)?;
        let cancellation = CancellationToken::new();
        state.registrations.insert(
            key.clone(),
            StoredRegistration {
                registration_id,
                options,
                cancellation: cancellation.clone(),
                hook,
            },
        );
        Ok(HookRegistration {
            state: Arc::downgrade(&self.state),
            key,
            registration_id,
            cancellation,
            is_disposed: false,
        })
    }

    /// 按稳定 dispatch 顺序返回脱敏 registration snapshot。
    pub fn snapshot(&self) -> Vec<HookRegistrationSnapshot> {
        let mut snapshots = lock_state(&self.state)
            .registrations
            .iter()
            .map(|(key, registration)| HookRegistrationSnapshot {
                phase: key.phase,
                owner: key.owner.clone(),
                hook_id: key.hook_id.clone(),
                priority: registration.options.priority(),
                timeout: registration.options.timeout(),
                cancellation_grace: registration.options.cancellation_grace(),
            })
            .collect::<Vec<_>>();
        snapshots.sort_by(|left, right| left.dispatch_order().cmp(&right.dispatch_order()));
        snapshots
    }

    /// 串行执行 `before_turn` transforms。
    pub async fn dispatch_before_turn(
        &self,
        mut payload: BeforeTurnPayload,
        cancellation: &CancellationToken,
    ) -> Result<BeforeTurnPayload, HookDispatchError> {
        for entry in self.dispatch_entries(HookPhase::BeforeTurn) {
            let DispatchHook::BeforeTurn(hook) = entry.hook.clone() else {
                unreachable!("phase-filtered dispatch entry must match its hook type");
            };
            let invocation_cancellation = cancellation.child_token();
            let output = await_hook(
                &entry,
                cancellation,
                &invocation_cancellation,
                hook.call(payload, invocation_cancellation.clone()),
            )
            .await?;
            match output {
                BeforeTurnDecision::Continue(next) => payload = next,
                BeforeTurnDecision::Reject(reason) => {
                    return Err(entry.error(HookDispatchErrorKind::Rejected(reason)));
                }
            }
        }
        Ok(payload)
    }

    /// 串行执行 `before_tool_execute` gates。
    pub async fn dispatch_before_tool_execute(
        &self,
        payload: BeforeToolExecutePayload,
        cancellation: &CancellationToken,
    ) -> Result<(), HookDispatchError> {
        for entry in self.dispatch_entries(HookPhase::BeforeToolExecute) {
            let DispatchHook::BeforeToolExecute(hook) = entry.hook.clone() else {
                unreachable!("phase-filtered dispatch entry must match its hook type");
            };
            let invocation_cancellation = cancellation.child_token();
            let output = await_hook(
                &entry,
                cancellation,
                &invocation_cancellation,
                hook.call(
                    BeforeToolExecutePayload::new(payload.call().clone()),
                    invocation_cancellation.clone(),
                ),
            )
            .await?;
            match output {
                BeforeToolExecuteDecision::Continue => {}
                BeforeToolExecuteDecision::Reject(reason) => {
                    return Err(entry.error(HookDispatchErrorKind::Rejected(reason)));
                }
            }
        }
        Ok(())
    }

    /// 串行执行 `after_tool_result` transforms。
    pub async fn dispatch_after_tool_result(
        &self,
        mut payload: AfterToolResultPayload,
        cancellation: &CancellationToken,
    ) -> Result<AfterToolResultPayload, HookDispatchError> {
        let expected_tool_name = payload.tool_name().to_string();
        let expected_call_id = payload.result().call_id().to_string();
        for entry in self.dispatch_entries(HookPhase::AfterToolResult) {
            let DispatchHook::AfterToolResult(hook) = entry.hook.clone() else {
                unreachable!("phase-filtered dispatch entry must match its hook type");
            };
            let invocation_cancellation = cancellation.child_token();
            let output = await_hook(
                &entry,
                cancellation,
                &invocation_cancellation,
                hook.call(payload, invocation_cancellation.clone()),
            )
            .await?;
            match output {
                AfterToolResultDecision::Continue(next) => {
                    if !next.has_identity(&expected_tool_name, &expected_call_id) {
                        return Err(entry.error(HookDispatchErrorKind::InvalidOutput));
                    }
                    payload = next;
                }
            }
        }
        Ok(payload)
    }

    fn dispatch_entries(&self, phase: HookPhase) -> Vec<DispatchEntry> {
        let mut entries = lock_state(&self.state)
            .registrations
            .iter()
            .filter_map(|(key, registration)| {
                if key.phase != phase {
                    return None;
                }
                let hook = match &registration.hook {
                    StoredHook::BeforeTurn(hook) => DispatchHook::BeforeTurn(Arc::clone(hook)),
                    StoredHook::BeforeToolExecute(hook) => {
                        DispatchHook::BeforeToolExecute(Arc::clone(hook))
                    }
                    StoredHook::AfterToolResult(hook) => {
                        DispatchHook::AfterToolResult(Arc::clone(hook))
                    }
                };
                Some(DispatchEntry {
                    key: key.clone(),
                    options: registration.options,
                    cancellation: registration.cancellation.clone(),
                    hook,
                })
            })
            .collect::<Vec<_>>();
        entries.sort_by(|left, right| left.dispatch_order().cmp(&right.dispatch_order()));
        entries
    }
}

impl fmt::Debug for ExtensionHookRegistry {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ExtensionHookRegistry")
            .field(
                "registration_count",
                &lock_state(&self.state).registrations.len(),
            )
            .finish()
    }
}

impl DispatchEntry {
    fn dispatch_order(&self) -> (i32, &HookOwnerId, &HookId) {
        (
            self.options.priority().get(),
            &self.key.owner,
            &self.key.hook_id,
        )
    }

    fn error(&self, kind: HookDispatchErrorKind) -> HookDispatchError {
        HookDispatchError::for_registration(
            self.key.phase,
            kind,
            self.key.owner.clone(),
            self.key.hook_id.clone(),
        )
    }
}

async fn await_hook<T>(
    entry: &DispatchEntry,
    caller_cancellation: &CancellationToken,
    invocation_cancellation: &CancellationToken,
    future: impl Future<Output = Result<T, HookFailureKind>>,
) -> Result<T, HookDispatchError> {
    let timeout = tokio::time::sleep(entry.options.timeout());
    tokio::pin!(timeout);
    tokio::pin!(future);
    let outcome = tokio::select! {
        biased;
        _ = caller_cancellation.cancelled() => {
            HookDispatchErrorKind::CallerCancelled
        }
        _ = entry.cancellation.cancelled() => {
            HookDispatchErrorKind::RegistrationDisposed
        }
        _ = &mut timeout => {
            HookDispatchErrorKind::TimedOut
        }
        outcome = &mut future => {
            return outcome.map_err(|kind| entry.error(HookDispatchErrorKind::Failed(kind)));
        }
    };
    invocation_cancellation.cancel();
    let _ = tokio::time::timeout(entry.options.cancellation_grace(), &mut future).await;
    Err(entry.error(outcome))
}

/// 一个 registration 的 redacted inspection snapshot。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookRegistrationSnapshot {
    /// Hook 所属的 typed phase。
    pub phase: HookPhase,
    /// 经过校验、可安全诊断的 owner identity。
    pub owner: HookOwnerId,
    /// 经过校验、可安全诊断的 hook identity。
    pub hook_id: HookId,
    /// 决定同一 phase 内稳定顺序的 priority。
    pub priority: crate::HookPriority,
    /// 单次 invocation 的 timeout。
    pub timeout: std::time::Duration,
    /// Cancellation 后允许 hook 完成 cleanup 的最大时间。
    pub cancellation_grace: std::time::Duration,
}

impl HookRegistrationSnapshot {
    fn dispatch_order(&self) -> (HookPhase, i32, &HookOwnerId, &HookId) {
        (self.phase, self.priority.get(), &self.owner, &self.hook_id)
    }
}

/// `HookRegistration` 拥有一个 registration 的幂等 inverse。
pub struct HookRegistration {
    state: Weak<Mutex<RegistryState>>,
    key: RegistrationKey,
    registration_id: u64,
    cancellation: CancellationToken,
    is_disposed: bool,
}

impl HookRegistration {
    /// 先阻止 fresh dispatch，再取消当前 registration 的 in-flight invocation。
    pub fn dispose(&mut self) -> bool {
        if self.is_disposed {
            return false;
        }
        self.is_disposed = true;
        let removed = self.state.upgrade().is_some_and(|state| {
            let mut state = lock_state(&state);
            let is_current = state
                .registrations
                .get(&self.key)
                .is_some_and(|registration| registration.registration_id == self.registration_id);
            if is_current {
                state.registrations.remove(&self.key);
            }
            is_current
        });
        self.cancellation.cancel();
        removed
    }
}

impl Drop for HookRegistration {
    fn drop(&mut self) {
        let _ = self.dispose();
    }
}

impl fmt::Debug for HookRegistration {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HookRegistration")
            .field("phase", &self.key.phase)
            .field("owner", &self.key.owner)
            .field("hook_id", &self.key.hook_id)
            .field("is_disposed", &self.is_disposed)
            .finish()
    }
}

/// Hook registration 的 typed、mutation-free错误。
#[derive(Clone, PartialEq, Eq, thiserror::Error)]
pub enum HookRegistrationError {
    #[error("duplicate extension hook registration: phase={phase} owner={owner} hook={hook_id}")]
    Duplicate {
        phase: HookPhase,
        owner: HookOwnerId,
        hook_id: HookId,
    },
    #[error("extension hook registration identity is exhausted")]
    RegistrationIdentityExhausted,
}

impl fmt::Debug for HookRegistrationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Duplicate {
                phase,
                owner,
                hook_id,
            } => formatter
                .debug_struct("Duplicate")
                .field("phase", phase)
                .field("owner", owner)
                .field("hook_id", hook_id)
                .finish(),
            Self::RegistrationIdentityExhausted => {
                formatter.write_str("RegistrationIdentityExhausted")
            }
        }
    }
}

fn lock_state(state: &Arc<Mutex<RegistryState>>) -> std::sync::MutexGuard<'_, RegistryState> {
    state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use std::{
        sync::atomic::{AtomicBool, Ordering},
        time::Duration,
    };

    use provider_protocol::{ConversationItem, Role};
    use tool_runtime::ToolResult;

    use super::*;
    use crate::{HookPriority, HookRejectionKind};

    fn owner(value: &str) -> HookOwnerId {
        HookOwnerId::try_new(value).expect("owner should validate")
    }

    fn hook_id(value: &str) -> HookId {
        HookId::try_new(value).expect("hook id should validate")
    }

    fn options(priority: i32) -> HookRegistrationOptions {
        HookRegistrationOptions::try_new(HookPriority::new(priority), Duration::from_millis(100))
            .expect("options should validate")
    }

    fn turn_payload(text: &str) -> BeforeTurnPayload {
        BeforeTurnPayload::try_new(vec![ConversationItem::text(Role::User, text)])
            .expect("payload should validate")
    }

    #[tokio::test]
    async fn before_turn_dispatch_is_identity_when_empty() {
        let registry = ExtensionHookRegistry::new();
        let output = registry
            .dispatch_before_turn(turn_payload("delivery-secret"), &CancellationToken::new())
            .await
            .expect("empty registry should continue");
        assert_eq!(output.items()[0].text_content(), "delivery-secret");
    }

    #[tokio::test]
    async fn empty_tool_gate_allows_and_empty_result_transform_is_identity() {
        let registry = ExtensionHookRegistry::new();
        registry
            .dispatch_before_tool_execute(
                BeforeToolExecutePayload::new(tool_runtime::ToolCall::new(
                    "call-1",
                    "echo",
                    serde_json::json!({"text": "hello"}),
                )),
                &CancellationToken::new(),
            )
            .await
            .expect("empty tool gate should allow");

        let output = registry
            .dispatch_after_tool_result(
                AfterToolResultPayload::new("echo", ToolResult::success("call-1", "raw")),
                &CancellationToken::new(),
            )
            .await
            .expect("empty result transform should be identity");

        assert_eq!(output.tool_name(), "echo");
        assert_eq!(output.result().call_id(), "call-1");
        assert_eq!(output.result().text_content(), "raw");
    }

    #[tokio::test]
    async fn serial_transform_uses_priority_owner_and_hook_order() {
        let registry = ExtensionHookRegistry::new();
        let observed = Arc::new(Mutex::new(Vec::new()));
        let mut registrations = Vec::new();
        for (owner_id, id, priority) in [
            ("z-owner", "b-hook", 0),
            ("a-owner", "z-hook", 0),
            ("a-owner", "a-hook", 0),
            ("last-owner", "last-hook", 10),
            ("first-owner", "first-hook", -10),
        ] {
            let observed = Arc::clone(&observed);
            let label = format!("{owner_id}/{id}");
            registrations.push(
                registry
                    .register_before_turn(
                        owner(owner_id),
                        hook_id(id),
                        options(priority),
                        Arc::new(move |payload: BeforeTurnPayload, _| {
                            let observed = Arc::clone(&observed);
                            let label = label.clone();
                            async move {
                                observed.lock().unwrap().push(label.clone());
                                let text = format!("{}|{label}", payload.items()[0].text_content());
                                Ok(BeforeTurnDecision::Continue(
                                    payload
                                        .replace_items(vec![ConversationItem::text(
                                            Role::User,
                                            text,
                                        )])
                                        .expect("replacement should validate"),
                                ))
                            }
                        }),
                    )
                    .expect("registration should succeed"),
            );
        }

        let output = registry
            .dispatch_before_turn(turn_payload("start"), &CancellationToken::new())
            .await
            .expect("dispatch should succeed");

        assert_eq!(
            *observed.lock().unwrap(),
            [
                "first-owner/first-hook",
                "a-owner/a-hook",
                "a-owner/z-hook",
                "z-owner/b-hook",
                "last-owner/last-hook",
            ]
        );
        assert_eq!(
            output.items()[0].text_content(),
            "start|first-owner/first-hook|a-owner/a-hook|a-owner/z-hook|z-owner/b-hook|last-owner/last-hook"
        );
        drop(registrations);
    }

    #[tokio::test]
    async fn tool_gate_uses_priority_owner_and_hook_order() {
        let registry = ExtensionHookRegistry::new();
        let observed = Arc::new(Mutex::new(Vec::new()));
        let mut registrations = Vec::new();
        for (owner_id, id, priority) in [
            ("z-owner", "b-hook", 0),
            ("a-owner", "z-hook", 0),
            ("a-owner", "a-hook", 0),
            ("last-owner", "last-hook", 10),
            ("first-owner", "first-hook", -10),
        ] {
            let observed = Arc::clone(&observed);
            let label = format!("{owner_id}/{id}");
            registrations.push(
                registry
                    .register_before_tool_execute(
                        owner(owner_id),
                        hook_id(id),
                        options(priority),
                        Arc::new(move |_: BeforeToolExecutePayload, _| {
                            let observed = Arc::clone(&observed);
                            let label = label.clone();
                            async move {
                                observed.lock().unwrap().push(label);
                                Ok(BeforeToolExecuteDecision::Continue)
                            }
                        }),
                    )
                    .expect("registration should succeed"),
            );
        }

        registry
            .dispatch_before_tool_execute(
                BeforeToolExecutePayload::new(tool_runtime::ToolCall::new(
                    "call-1",
                    "echo",
                    serde_json::json!({}),
                )),
                &CancellationToken::new(),
            )
            .await
            .expect("ordered gates should allow");

        assert_eq!(
            *observed.lock().unwrap(),
            [
                "first-owner/first-hook",
                "a-owner/a-hook",
                "a-owner/z-hook",
                "z-owner/b-hook",
                "last-owner/last-hook",
            ]
        );
        drop(registrations);
    }

    #[tokio::test]
    async fn result_transform_is_serial_and_uses_priority_owner_and_hook_order() {
        let registry = ExtensionHookRegistry::new();
        let mut registrations = Vec::new();
        for (owner_id, id, priority) in [
            ("z-owner", "b-hook", 0),
            ("a-owner", "z-hook", 0),
            ("a-owner", "a-hook", 0),
            ("last-owner", "last-hook", 10),
            ("first-owner", "first-hook", -10),
        ] {
            let label = format!("{owner_id}/{id}");
            registrations.push(
                registry
                    .register_after_tool_result(
                        owner(owner_id),
                        hook_id(id),
                        options(priority),
                        Arc::new(move |payload: AfterToolResultPayload, _| {
                            let label = label.clone();
                            async move {
                                let text = format!("{}|{label}", payload.result().text_content());
                                let replacement =
                                    ToolResult::success(payload.result().call_id(), text);
                                Ok(AfterToolResultDecision::Continue(
                                    payload
                                        .replace_result(replacement)
                                        .expect("call identity should remain stable"),
                                ))
                            }
                        }),
                    )
                    .expect("registration should succeed"),
            );
        }

        let output = registry
            .dispatch_after_tool_result(
                AfterToolResultPayload::new("echo", ToolResult::success("call-1", "start")),
                &CancellationToken::new(),
            )
            .await
            .expect("ordered transforms should succeed");

        assert_eq!(output.tool_name(), "echo");
        assert_eq!(output.result().call_id(), "call-1");
        assert_eq!(
            output.result().text_content(),
            "start|first-owner/first-hook|a-owner/a-hook|a-owner/z-hook|z-owner/b-hook|last-owner/last-hook"
        );
        drop(registrations);
    }

    #[tokio::test]
    async fn rejection_short_circuits_remaining_hooks() {
        let registry = ExtensionHookRegistry::new();
        let later_ran = Arc::new(AtomicBool::new(false));
        let first = registry
            .register_before_tool_execute(
                owner("policy"),
                hook_id("reject"),
                options(0),
                Arc::new(|_: BeforeToolExecutePayload, _| async {
                    Ok(BeforeToolExecuteDecision::Reject(
                        HookRejectionKind::PolicyDenied,
                    ))
                }),
            )
            .unwrap();
        let later_ran_for_hook = Arc::clone(&later_ran);
        let second = registry
            .register_before_tool_execute(
                owner("policy"),
                hook_id("should-not-run"),
                options(1),
                Arc::new(move |_: BeforeToolExecutePayload, _| {
                    let later_ran = Arc::clone(&later_ran_for_hook);
                    async move {
                        later_ran.store(true, Ordering::SeqCst);
                        Ok(BeforeToolExecuteDecision::Continue)
                    }
                }),
            )
            .unwrap();

        let error = registry
            .dispatch_before_tool_execute(
                BeforeToolExecutePayload::new(tool_runtime::ToolCall::new(
                    "secret-call-id",
                    "secret-tool",
                    serde_json::json!({"secret": "argument"}),
                )),
                &CancellationToken::new(),
            )
            .await
            .expect_err("rejection should fail closed");

        assert_eq!(
            error.kind(),
            HookDispatchErrorKind::Rejected(HookRejectionKind::PolicyDenied)
        );
        assert!(!later_ran.load(Ordering::SeqCst));
        drop((first, second));
    }

    #[tokio::test]
    async fn timeout_cancels_invocation_and_reports_closed_metadata() {
        let registry = ExtensionHookRegistry::new();
        let later_ran = Arc::new(AtomicBool::new(false));
        let invocation_token = Arc::new(Mutex::new(None::<CancellationToken>));
        let invocation_token_for_hook = Arc::clone(&invocation_token);
        let registration = registry
            .register_before_turn(
                owner("timeout-owner"),
                hook_id("wait"),
                HookRegistrationOptions::try_new(
                    HookPriority::default(),
                    Duration::from_millis(10),
                )
                .unwrap(),
                Arc::new(
                    move |_: BeforeTurnPayload, cancellation: CancellationToken| {
                        *invocation_token_for_hook.lock().unwrap() = Some(cancellation);
                        async move { std::future::pending().await }
                    },
                ),
            )
            .unwrap();
        let later_ran_for_hook = Arc::clone(&later_ran);
        let later = registry
            .register_before_turn(
                owner("timeout-owner"),
                hook_id("later"),
                options(1),
                Arc::new(move |payload: BeforeTurnPayload, _| {
                    let later_ran = Arc::clone(&later_ran_for_hook);
                    async move {
                        later_ran.store(true, Ordering::SeqCst);
                        Ok(BeforeTurnDecision::Continue(payload))
                    }
                }),
            )
            .unwrap();

        let error = registry
            .dispatch_before_turn(
                turn_payload("instruction-secret"),
                &CancellationToken::new(),
            )
            .await
            .expect_err("hook should time out");
        tokio::task::yield_now().await;

        assert_eq!(error.kind(), HookDispatchErrorKind::TimedOut);
        assert!(
            invocation_token
                .lock()
                .unwrap()
                .as_ref()
                .is_some_and(CancellationToken::is_cancelled)
        );
        let diagnostic = format!("{error:?} {error}");
        assert!(!diagnostic.contains("instruction-secret"));
        assert!(!later_ran.load(Ordering::SeqCst));
        drop((registration, later));
    }

    #[tokio::test]
    async fn cancellation_grace_polls_cleanup_without_replacing_timeout_error() {
        let registry = ExtensionHookRegistry::new();
        let cleanup_finished = Arc::new(AtomicBool::new(false));
        let cleanup_finished_for_hook = Arc::clone(&cleanup_finished);
        let registration = registry
            .register_before_turn(
                owner("cleanup-owner"),
                hook_id("cleanup"),
                HookRegistrationOptions::try_new_with_cancellation_grace(
                    HookPriority::default(),
                    Duration::from_millis(5),
                    Duration::from_millis(50),
                )
                .unwrap(),
                Arc::new(
                    move |payload: BeforeTurnPayload, cancellation: CancellationToken| {
                        let cleanup_finished = Arc::clone(&cleanup_finished_for_hook);
                        async move {
                            cancellation.cancelled().await;
                            cleanup_finished.store(true, Ordering::SeqCst);
                            Ok(BeforeTurnDecision::Continue(payload))
                        }
                    },
                ),
            )
            .unwrap();

        let error = registry
            .dispatch_before_turn(turn_payload("private"), &CancellationToken::new())
            .await
            .expect_err("timeout should remain the dispatch result");

        assert_eq!(error.kind(), HookDispatchErrorKind::TimedOut);
        assert!(cleanup_finished.load(Ordering::SeqCst));
        drop(registration);
    }

    #[tokio::test]
    async fn cancellation_grace_drops_hook_that_ignores_cancellation() {
        let registry = ExtensionHookRegistry::new();
        let registration = registry
            .register_before_turn(
                owner("bounded-owner"),
                hook_id("ignores-cancel"),
                HookRegistrationOptions::try_new_with_cancellation_grace(
                    HookPriority::default(),
                    Duration::from_millis(5),
                    Duration::from_millis(10),
                )
                .unwrap(),
                Arc::new(|_: BeforeTurnPayload, _| async move { std::future::pending().await }),
            )
            .unwrap();

        let cancellation = CancellationToken::new();
        let dispatch = registry.dispatch_before_turn(turn_payload("private"), &cancellation);
        let error = tokio::time::timeout(Duration::from_millis(500), dispatch)
            .await
            .expect("timeout plus grace must stay bounded")
            .expect_err("hook should time out");

        assert_eq!(error.kind(), HookDispatchErrorKind::TimedOut);
        drop(registration);
    }

    #[tokio::test]
    async fn caller_cancellation_wins_over_ready_hook_and_timeout() {
        let registry = ExtensionHookRegistry::new();
        let later_ran = Arc::new(AtomicBool::new(false));
        let registration = registry
            .register_before_turn(
                owner("cancel-owner"),
                hook_id("ready"),
                options(0),
                Arc::new(|payload: BeforeTurnPayload, _| async move {
                    Ok(BeforeTurnDecision::Continue(payload))
                }),
            )
            .unwrap();
        let later_ran_for_hook = Arc::clone(&later_ran);
        let later = registry
            .register_before_turn(
                owner("cancel-owner"),
                hook_id("later"),
                options(1),
                Arc::new(move |payload: BeforeTurnPayload, _| {
                    let later_ran = Arc::clone(&later_ran_for_hook);
                    async move {
                        later_ran.store(true, Ordering::SeqCst);
                        Ok(BeforeTurnDecision::Continue(payload))
                    }
                }),
            )
            .unwrap();
        let cancellation = CancellationToken::new();
        cancellation.cancel();

        let error = registry
            .dispatch_before_turn(turn_payload("private-user-content"), &cancellation)
            .await
            .expect_err("cancellation should win");

        assert_eq!(error.kind(), HookDispatchErrorKind::CallerCancelled);
        assert!(!later_ran.load(Ordering::SeqCst));
        drop((registration, later));
    }

    #[tokio::test]
    async fn dispose_removes_registration_and_cancels_in_flight_invocation() {
        let registry = ExtensionHookRegistry::new();
        let later_ran = Arc::new(AtomicBool::new(false));
        let entered = Arc::new(tokio::sync::Notify::new());
        let entered_for_hook = Arc::clone(&entered);
        let mut registration = registry
            .register_before_turn(
                owner("owner"),
                hook_id("blocking"),
                options(0),
                Arc::new(
                    move |_: BeforeTurnPayload, cancellation: CancellationToken| {
                        let entered = Arc::clone(&entered_for_hook);
                        async move {
                            entered.notify_one();
                            cancellation.cancelled().await;
                            std::future::pending().await
                        }
                    },
                ),
            )
            .unwrap();
        let later_ran_for_hook = Arc::clone(&later_ran);
        let later = registry
            .register_before_turn(
                owner("owner"),
                hook_id("later"),
                options(1),
                Arc::new(move |payload: BeforeTurnPayload, _| {
                    let later_ran = Arc::clone(&later_ran_for_hook);
                    async move {
                        later_ran.store(true, Ordering::SeqCst);
                        Ok(BeforeTurnDecision::Continue(payload))
                    }
                }),
            )
            .unwrap();
        let dispatch_registry = registry.clone();
        let dispatch = tokio::spawn(async move {
            dispatch_registry
                .dispatch_before_turn(turn_payload("secret"), &CancellationToken::new())
                .await
        });
        entered.notified().await;

        assert!(registration.dispose());
        assert!(!registration.dispose());
        let error = dispatch
            .await
            .unwrap()
            .expect_err("disposed registration should stop in-flight dispatch");

        assert_eq!(error.kind(), HookDispatchErrorKind::RegistrationDisposed);
        assert!(!later_ran.load(Ordering::SeqCst));
        assert_eq!(registry.snapshot().len(), 1);
        drop(later);
        assert!(registry.snapshot().is_empty());
    }

    #[test]
    fn duplicate_is_rejected_before_mutation_and_drop_is_inverse() {
        let registry = ExtensionHookRegistry::new();
        let registration = registry
            .register_before_turn(
                owner("owner"),
                hook_id("hook"),
                options(0),
                Arc::new(|payload: BeforeTurnPayload, _| async move {
                    Ok(BeforeTurnDecision::Continue(payload))
                }),
            )
            .unwrap();
        let before = registry.snapshot();

        let duplicate = registry.register_before_turn(
            owner("owner"),
            hook_id("hook"),
            options(-100),
            Arc::new(|payload: BeforeTurnPayload, _| async move {
                Ok(BeforeTurnDecision::Continue(payload))
            }),
        );

        assert!(matches!(
            duplicate,
            Err(HookRegistrationError::Duplicate { .. })
        ));
        assert_eq!(registry.snapshot(), before);
        drop(registration);
        assert!(registry.snapshot().is_empty());
    }

    #[test]
    fn disposed_generation_handle_cannot_remove_a_fresh_registration() {
        let registry = ExtensionHookRegistry::new();
        let mut stale = registry
            .register_before_turn(
                owner("owner"),
                hook_id("hook"),
                options(0),
                Arc::new(|payload: BeforeTurnPayload, _| async move {
                    Ok(BeforeTurnDecision::Continue(payload))
                }),
            )
            .unwrap();
        assert!(stale.dispose());
        let fresh = registry
            .register_before_turn(
                owner("owner"),
                hook_id("hook"),
                options(1),
                Arc::new(|payload: BeforeTurnPayload, _| async move {
                    Ok(BeforeTurnDecision::Continue(payload))
                }),
            )
            .unwrap();

        drop(stale);

        assert_eq!(registry.snapshot().len(), 1);
        assert_eq!(registry.snapshot()[0].priority, HookPriority::new(1));
        drop(fresh);
        assert!(registry.snapshot().is_empty());
    }

    #[tokio::test]
    async fn after_result_rejects_identity_change_and_redacts_payloads() {
        let registry = ExtensionHookRegistry::new();
        let later_ran = Arc::new(AtomicBool::new(false));
        let registration = registry
            .register_after_tool_result(
                owner("result-owner"),
                hook_id("replace"),
                options(0),
                Arc::new(|_: AfterToolResultPayload, _| async move {
                    Ok(AfterToolResultDecision::Continue(
                        AfterToolResultPayload::new(
                            "changed-tool",
                            ToolResult::success("changed-call", "result-secret"),
                        ),
                    ))
                }),
            )
            .unwrap();
        let later_ran_for_hook = Arc::clone(&later_ran);
        let later = registry
            .register_after_tool_result(
                owner("result-owner"),
                hook_id("later"),
                options(1),
                Arc::new(move |payload: AfterToolResultPayload, _| {
                    let later_ran = Arc::clone(&later_ran_for_hook);
                    async move {
                        later_ran.store(true, Ordering::SeqCst);
                        Ok(AfterToolResultDecision::Continue(payload))
                    }
                }),
            )
            .unwrap();

        let error = registry
            .dispatch_after_tool_result(
                AfterToolResultPayload::new(
                    "secret-tool",
                    ToolResult::success_content(
                        "secret-call",
                        vec![tool_runtime::ToolResultContent::Text(
                            "result-secret".to_string(),
                        )],
                    ),
                ),
                &CancellationToken::new(),
            )
            .await
            .expect_err("identity changes should fail closed");

        assert_eq!(error.kind(), HookDispatchErrorKind::InvalidOutput);
        let diagnostic = format!("{error:?} {error}");
        for secret in [
            "secret-tool",
            "secret-call",
            "result-secret",
            "changed-call",
        ] {
            assert!(!diagnostic.contains(secret));
        }
        assert!(!later_ran.load(Ordering::SeqCst));
        drop((registration, later));
    }

    #[test]
    fn payload_debug_redacts_instruction_arguments_results_and_identifiers() {
        let before_turn = format!("{:?}", turn_payload("instruction-secret"));
        let before_tool = format!(
            "{:?}",
            BeforeToolExecutePayload::new(tool_runtime::ToolCall::new(
                "call-secret",
                "tool-secret",
                serde_json::json!({"password": "credential-secret"}),
            ))
        );
        let after_tool = format!(
            "{:?}",
            AfterToolResultPayload::new(
                "tool-secret",
                ToolResult::success("call-secret", "result-secret")
                    .with_display_content("display-secret"),
            )
        );

        for diagnostic in [before_turn, before_tool, after_tool] {
            for secret in [
                "instruction-secret",
                "call-secret",
                "tool-secret",
                "credential-secret",
                "result-secret",
                "display-secret",
            ] {
                assert!(!diagnostic.contains(secret));
            }
        }
    }

    #[tokio::test]
    async fn registration_snapshot_and_dispatch_diagnostics_expose_only_closed_metadata() {
        const PRIVATE_VALUES: &[&str] = &[
            "private-instruction-body",
            "private-user-content",
            "private-tool-arguments",
            "private-tool-schema",
            "private-tool-result",
            "/private/workspace/file",
            "private-credential",
            "https://private.example/v1",
            "private-session-id",
            "private-request-id",
            "private-call-id",
            "private-raw-hook-error",
        ];
        let registry = ExtensionHookRegistry::new();
        let later_ran = Arc::new(AtomicBool::new(false));
        let captured_raw_error = Arc::<str>::from(PRIVATE_VALUES[11]);
        let captured_raw_error_for_hook = Arc::clone(&captured_raw_error);
        let registration = registry
            .register_before_tool_execute(
                owner("safe-owner"),
                hook_id("safe-hook"),
                options(0),
                Arc::new(move |_: BeforeToolExecutePayload, _| {
                    let captured_raw_error = Arc::clone(&captured_raw_error_for_hook);
                    async move {
                        assert!(!captured_raw_error.is_empty());
                        Err(HookFailureKind::Internal)
                    }
                }),
            )
            .expect("hook should register");
        let later_ran_for_hook = Arc::clone(&later_ran);
        let later = registry
            .register_before_tool_execute(
                owner("safe-owner"),
                hook_id("later-hook"),
                options(1),
                Arc::new(move |_: BeforeToolExecutePayload, _| {
                    let later_ran = Arc::clone(&later_ran_for_hook);
                    async move {
                        later_ran.store(true, Ordering::SeqCst);
                        Ok(BeforeToolExecuteDecision::Continue)
                    }
                }),
            )
            .expect("later hook should register");
        let duplicate = registry
            .register_before_tool_execute(
                owner("safe-owner"),
                hook_id("safe-hook"),
                options(1),
                Arc::new(|_: BeforeToolExecutePayload, _| async {
                    Ok(BeforeToolExecuteDecision::Continue)
                }),
            )
            .expect_err("duplicate should fail before mutation");
        let snapshot = registry.snapshot();
        let dispatch = registry
            .dispatch_before_tool_execute(
                BeforeToolExecutePayload::new(tool_runtime::ToolCall::new(
                    PRIVATE_VALUES[10],
                    "safe-tool",
                    serde_json::json!({
                        "instruction": PRIVATE_VALUES[0],
                        "user": PRIVATE_VALUES[1],
                        "arguments": PRIVATE_VALUES[2],
                        "schema": PRIVATE_VALUES[3],
                        "result": PRIVATE_VALUES[4],
                        "path": PRIVATE_VALUES[5],
                        "credential": PRIVATE_VALUES[6],
                        "endpoint": PRIVATE_VALUES[7],
                        "session_id": PRIVATE_VALUES[8],
                        "request_id": PRIVATE_VALUES[9],
                    }),
                )),
                &CancellationToken::new(),
            )
            .await
            .expect_err("closed hook failure should stop dispatch");

        for diagnostic in [
            format!("{duplicate:?} {duplicate}"),
            format!("{snapshot:?}"),
            format!("{dispatch:?} {dispatch}"),
        ] {
            for private in PRIVATE_VALUES {
                assert!(!diagnostic.contains(private), "diagnostic leaked {private}");
            }
        }
        assert_eq!(snapshot.len(), 2);
        assert_eq!(
            dispatch.kind(),
            HookDispatchErrorKind::Failed(HookFailureKind::Internal)
        );
        assert!(!later_ran.load(Ordering::SeqCst));
        drop((captured_raw_error, registration, later));
    }

    #[test]
    fn registry_debug_and_weak_registration_do_not_retain_payload_or_host() {
        let registry = ExtensionHookRegistry::new();
        let registration = registry
            .register_before_turn(
                owner("owner"),
                hook_id("hook"),
                options(0),
                Arc::new(|payload: BeforeTurnPayload, _| async move {
                    Ok(BeforeTurnDecision::Continue(payload))
                }),
            )
            .unwrap();
        assert_eq!(
            format!("{registry:?}"),
            "ExtensionHookRegistry { registration_count: 1 }"
        );

        drop(registry);
        assert_eq!(registration.state.strong_count(), 0);
        drop(registration);
    }

    #[test]
    fn before_turn_items_remain_structured() {
        let payload =
            BeforeTurnPayload::try_new(vec![ConversationItem::text(Role::User, "body")]).unwrap();
        assert_eq!(payload.items()[0].text_content(), "body");
    }
}
