//! 与 lifecycle 绑定的 typed runtime capability registry。

use std::{
    any::Any,
    collections::{BTreeMap, BTreeSet},
    fmt,
    marker::PhantomData,
    ops::Deref,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{Arc, Mutex, Weak},
};

use extension_hook_runtime::ExtensionHookRegistry;
use runtime_domain::{event_notifier::RuntimeEventNotifier, runtime_wake::RuntimeWake};

use super::{
    effect_scope::EffectScope,
    lifecycle::{CapabilityKey, CapabilitySnapshot},
    llm_port::LlmPort,
    permission_policy::PermissionPolicy,
    prompt_assembly::PromptAssembly,
    session_port::SessionBackendViews,
};
use tool_runtime::ToolCatalog;

/// Typed capability marker；稳定 key 与 concrete value type 只在 runtime crate 内绑定。
pub(super) trait RuntimeCapability {
    const KEY: &'static str;
    type Value: Send + Sync + 'static;
}

macro_rules! runtime_capability {
    ($name:ident, $key:literal, $value:ty) => {
        pub(super) struct $name;

        impl RuntimeCapability for $name {
            const KEY: &'static str = $key;
            type Value = $value;
        }
    };
}

runtime_capability!(
    ApprovalProviderCapability,
    "approval_provider",
    PermissionPolicy
);
runtime_capability!(LlmPortCapability, "llm_port", LlmPort);
runtime_capability!(ModelCatalogCapability, "model_catalog", LlmPort);
runtime_capability!(
    ExtensionHookRegistryCapability,
    "extension_hooks",
    ExtensionHookRegistry
);
runtime_capability!(
    PermissionPolicyCapability,
    "permission_policy",
    PermissionPolicy
);
runtime_capability!(PromptAssemblyCapability, "prompt_assembly", PromptAssembly);
runtime_capability!(
    RuntimeEventStreamCapability,
    "runtime_event_stream",
    RuntimeEventNotifier
);
runtime_capability!(RuntimeWakeCapability, "runtime_wake", RuntimeWake);
runtime_capability!(
    SessionPersistenceCapability,
    "session_persistence",
    SessionBackendViews
);
runtime_capability!(ToolCatalogCapability, "tool_catalog", ToolCatalog);

/// 一个 current provider generation 的不可变 typed lease。
pub(super) struct CapabilityLease<C>
where
    C: RuntimeCapability,
{
    value: Arc<C::Value>,
    state: Weak<Mutex<RuntimeContextState>>,
    provider_component: String,
    registration_id: u64,
    generation: u64,
    marker: PhantomData<fn() -> C>,
}

impl<C> Clone for CapabilityLease<C>
where
    C: RuntimeCapability,
{
    fn clone(&self) -> Self {
        Self {
            value: Arc::clone(&self.value),
            state: self.state.clone(),
            provider_component: self.provider_component.clone(),
            registration_id: self.registration_id,
            generation: self.generation,
            marker: PhantomData,
        }
    }
}

impl<C> Deref for CapabilityLease<C>
where
    C: RuntimeCapability,
{
    type Target = C::Value;

    fn deref(&self) -> &Self::Target {
        self.value.as_ref()
    }
}

impl<C> fmt::Debug for CapabilityLease<C>
where
    C: RuntimeCapability,
{
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CapabilityLease")
            .field("key", &C::KEY)
            .field("provider_component", &self.provider_component)
            .field("generation", &self.generation)
            .finish()
    }
}

impl<C> CapabilityLease<C>
where
    C: RuntimeCapability,
{
    /// 为需要 live-generation validation 的 scoped consumer 创建 opaque guard。
    pub(super) fn generation_guard(&self) -> CapabilityGenerationGuard {
        CapabilityGenerationGuard {
            state: self.state.clone(),
            key: CapabilityKey::from(C::KEY),
            provider_component: self.provider_component.clone(),
            registration_id: self.registration_id,
            generation: self.generation,
        }
    }

    #[cfg(test)]
    pub(super) fn provider_component(&self) -> &str {
        self.provider_component.as_str()
    }

    #[cfg(test)]
    pub(super) const fn generation(&self) -> u64 {
        self.generation
    }
}

/// `CapabilityGenerationGuard` 只验证 lease 的原始 Context slot 是否仍是 current generation。
///
/// guard 不持有 capability value，也不暴露 private registration identity。
#[derive(Clone)]
pub(super) struct CapabilityGenerationGuard {
    state: Weak<Mutex<RuntimeContextState>>,
    key: CapabilityKey,
    provider_component: String,
    registration_id: u64,
    generation: u64,
}

impl CapabilityGenerationGuard {
    pub(super) fn is_current(&self) -> bool {
        let Some(state) = self.state.upgrade() else {
            return false;
        };
        lock_state(&state).slots.get(&self.key).is_some_and(|slot| {
            slot.provider_component == self.provider_component
                && slot.registration_id == self.registration_id
                && slot.visibility == CapabilityVisibility::Visible(self.generation)
        })
    }

    pub(super) fn key(&self) -> &str {
        self.key.as_str()
    }

    pub(super) const fn generation(&self) -> u64 {
        self.generation
    }

    /// 注册只响应原始 slot identity 失效的 callback。
    pub(super) fn subscribe_revocation(
        &self,
        callback: impl FnMut() -> Result<(), ()> + Send + 'static,
    ) -> Result<CapabilityRevocationSubscription, CapabilitySubscriptionError> {
        let mut subscription = CapabilityRevocationSubscription {
            context: self.state.clone(),
            key: self.key.clone(),
            provider_component: self.provider_component.clone(),
            registration_id: self.registration_id,
            subscription_id: 0,
            callback: Arc::new(CapabilityRevocationSubscriptionState {
                callback: Mutex::new(CapabilityRevocationCallbackState {
                    callback: Some(Box::new(callback)),
                    is_complete: false,
                }),
            }),
        };
        let Some(state) = self.state.upgrade() else {
            subscription.callback.try_revoke();
            return Ok(subscription);
        };
        let mut state = lock_state(&state);
        let Some(slot) = state.slots.get_mut(&self.key) else {
            drop(state);
            subscription.callback.try_revoke();
            return Ok(subscription);
        };
        let is_current = slot.provider_component == self.provider_component
            && slot.registration_id == self.registration_id
            && slot.visibility == CapabilityVisibility::Visible(self.generation);
        if !is_current {
            drop(state);
            subscription.callback.try_revoke();
            return Ok(subscription);
        }
        let subscription_id = state.next_subscription_id;
        state.next_subscription_id = subscription_id.checked_add(1).ok_or(
            CapabilitySubscriptionError::IdentityExhausted {
                capability: self.key.to_string(),
            },
        )?;
        let slot = state
            .slots
            .get_mut(&self.key)
            .expect("validated capability slot should remain available");
        slot.revocation_subscriptions
            .insert(subscription_id, Arc::clone(&subscription.callback));
        drop(state);
        subscription.subscription_id = subscription_id;
        Ok(subscription)
    }
}

impl fmt::Debug for CapabilityGenerationGuard {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CapabilityGenerationGuard")
            .field("key", &self.key)
            .field("provider_component", &self.provider_component)
            .field("generation", &self.generation)
            .finish_non_exhaustive()
    }
}

/// Typed lookup 只暴露 closed capability metadata，不投影 erased value 或 Rust type name。
#[derive(Clone, PartialEq, Eq, thiserror::Error)]
pub(super) enum RuntimeContextError {
    #[error("runtime capability `{capability}` is unavailable")]
    MissingCapability { capability: &'static str },
    #[error("runtime capability `{capability}` has an incompatible value")]
    CapabilityTypeMismatch { capability: &'static str },
    #[error("runtime capability `{capability}` could not be retained by its consumer")]
    DependencyRetentionRejected { capability: &'static str },
}

impl fmt::Debug for RuntimeContextError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingCapability { capability } => formatter
                .debug_struct("MissingCapability")
                .field("capability", capability)
                .finish(),
            Self::CapabilityTypeMismatch { capability } => formatter
                .debug_struct("CapabilityTypeMismatch")
                .field("capability", capability)
                .finish(),
            Self::DependencyRetentionRejected { capability } => formatter
                .debug_struct("DependencyRetentionRejected")
                .field("capability", capability)
                .finish(),
        }
    }
}

/// Context publication error 不保留 concrete value、registration identity 或 disposer text。
#[derive(Clone, PartialEq, Eq, thiserror::Error)]
pub(super) enum ContextPublicationError {
    #[error("component `{component_id}` does not declare capability `{capability}`")]
    UndeclaredCapability {
        component_id: String,
        capability: String,
    },
    #[error("runtime capability `{capability}` already has a staged or visible provider")]
    DuplicateCapability { capability: String },
    #[error("runtime capability registration identity is exhausted")]
    RegistrationIdentityExhausted,
    #[error("runtime capability `{capability}` could not be attached to its effect scope")]
    EffectRegistrationRejected { capability: String },
    #[error("component `{component_id}` did not stage exactly its declared capabilities")]
    IncompletePublication { component_id: String },
    #[error("runtime capability `{capability}` has no matching graph publication")]
    MissingGraphPublication { capability: String },
    #[error("runtime capability `{capability}` staging is stale")]
    StaleStaging { capability: String },
    #[error("runtime capability `{capability}` visibility does not match the lifecycle graph")]
    VisibilityMismatch { capability: String },
    #[error("runtime capability `{capability}` dependency cleanup is pending")]
    DependencyCleanupPending { capability: String },
}

impl fmt::Debug for ContextPublicationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UndeclaredCapability {
                component_id,
                capability,
            } => formatter
                .debug_struct("UndeclaredCapability")
                .field("component_id", component_id)
                .field("capability", capability)
                .finish(),
            Self::DuplicateCapability { capability } => formatter
                .debug_struct("DuplicateCapability")
                .field("capability", capability)
                .finish(),
            Self::RegistrationIdentityExhausted => {
                formatter.write_str("RegistrationIdentityExhausted")
            }
            Self::EffectRegistrationRejected { capability } => formatter
                .debug_struct("EffectRegistrationRejected")
                .field("capability", capability)
                .finish(),
            Self::IncompletePublication { component_id } => formatter
                .debug_struct("IncompletePublication")
                .field("component_id", component_id)
                .finish(),
            Self::MissingGraphPublication { capability } => formatter
                .debug_struct("MissingGraphPublication")
                .field("capability", capability)
                .finish(),
            Self::StaleStaging { capability } => formatter
                .debug_struct("StaleStaging")
                .field("capability", capability)
                .finish(),
            Self::VisibilityMismatch { capability } => formatter
                .debug_struct("VisibilityMismatch")
                .field("capability", capability)
                .finish(),
            Self::DependencyCleanupPending { capability } => formatter
                .debug_struct("DependencyCleanupPending")
                .field("capability", capability)
                .finish(),
        }
    }
}

/// Context inspection 的封闭投影；顺序由 capability key 决定。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RuntimeCapabilitySnapshot {
    pub(super) key: String,
    pub(super) provider_component: String,
    pub(super) generation: u64,
}

#[derive(Default)]
pub(super) struct RuntimeContext {
    state: Arc<Mutex<RuntimeContextState>>,
}

#[derive(Default)]
struct RuntimeContextState {
    next_registration_id: u64,
    next_subscription_id: u64,
    slots: BTreeMap<CapabilityKey, ContextSlot>,
    #[cfg(test)]
    reject_next_hide: bool,
}

struct ContextSlot {
    provider_component: String,
    registration_id: u64,
    visibility: CapabilityVisibility,
    value: Arc<dyn Any + Send + Sync>,
    revocation_subscriptions: BTreeMap<u64, Arc<CapabilityRevocationSubscriptionState>>,
}

impl Drop for ContextSlot {
    fn drop(&mut self) {
        for subscription in self.revocation_subscriptions.values() {
            subscription.try_revoke();
        }
    }
}

type CapabilityRevocationCallback = Box<dyn FnMut() -> Result<(), ()> + Send + 'static>;

struct CapabilityRevocationSubscriptionState {
    callback: Mutex<CapabilityRevocationCallbackState>,
}

struct CapabilityRevocationCallbackState {
    callback: Option<CapabilityRevocationCallback>,
    is_complete: bool,
}

impl CapabilityRevocationSubscriptionState {
    fn try_revoke(&self) -> bool {
        let mut callback = {
            let mut state = self
                .callback
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if state.is_complete {
                return true;
            }
            let Some(callback) = state.callback.take() else {
                // 另一个 caller 正在执行 callback；保守保留 cleanup barrier。
                return false;
            };
            callback
        };
        let succeeded = matches!(catch_unwind(AssertUnwindSafe(&mut callback)), Ok(Ok(())));
        let mut state = self
            .callback
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if succeeded || state.is_complete {
            state.is_complete = true;
        } else {
            state.callback = Some(callback);
        }
        state.is_complete
    }

    fn disarm(&self) {
        let mut state = self
            .callback
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.is_complete = true;
        state.callback.take();
    }
}

pub(super) struct CapabilityRevocationSubscription {
    context: Weak<Mutex<RuntimeContextState>>,
    key: CapabilityKey,
    provider_component: String,
    registration_id: u64,
    subscription_id: u64,
    callback: Arc<CapabilityRevocationSubscriptionState>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub(super) enum CapabilitySubscriptionError {
    #[error("runtime capability `{capability}` subscription identity is exhausted")]
    IdentityExhausted { capability: String },
}

impl Drop for CapabilityRevocationSubscription {
    fn drop(&mut self) {
        let Some(context) = self.context.upgrade() else {
            self.callback.disarm();
            return;
        };
        let mut context = lock_state(&context);
        let Some(slot) = context.slots.get_mut(&self.key) else {
            drop(context);
            self.callback.disarm();
            return;
        };
        let owns_subscription = slot.provider_component == self.provider_component
            && slot.registration_id == self.registration_id
            && slot
                .revocation_subscriptions
                .get(&self.subscription_id)
                .is_some_and(|callback| Arc::ptr_eq(callback, &self.callback));
        if !owns_subscription {
            drop(context);
            self.callback.disarm();
            return;
        }
        if matches!(slot.visibility, CapabilityVisibility::Revoking(_)) {
            return;
        }
        slot.revocation_subscriptions.remove(&self.subscription_id);
        drop(context);
        self.callback.disarm();
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CapabilityVisibility {
    Pending,
    Visible(u64),
    Revoking(u64),
}

pub(super) struct ComponentActivationContext<'a> {
    context: &'a RuntimeContext,
    scope: &'a EffectScope,
    component_id: &'a str,
    declared_capabilities: BTreeSet<CapabilityKey>,
    staged: Vec<StagedCapability>,
}

pub(super) struct StagedCapability {
    key: CapabilityKey,
    provider_component: String,
    registration_id: u64,
}

struct ContextRegistration {
    state: Weak<Mutex<RuntimeContextState>>,
    key: CapabilityKey,
    provider_component: String,
    registration_id: u64,
    is_disposed: bool,
}

impl RuntimeContext {
    pub(super) fn activation<'a>(
        &'a self,
        scope: &'a EffectScope,
        component_id: &'a str,
        declared_capabilities: impl IntoIterator<Item = CapabilityKey>,
    ) -> ComponentActivationContext<'a> {
        ComponentActivationContext {
            context: self,
            scope,
            component_id,
            declared_capabilities: declared_capabilities.into_iter().collect(),
            staged: Vec::new(),
        }
    }

    pub(super) fn event_stream_lease(
        value: RuntimeEventNotifier,
        provider_component: &str,
    ) -> CapabilityLease<RuntimeEventStreamCapability> {
        let context = Self::default();
        let scope = EffectScope::default();
        let mut activation = context.activation(
            &scope,
            provider_component,
            [CapabilityKey::from(RuntimeEventStreamCapability::KEY)],
        );
        activation
            .publish::<RuntimeEventStreamCapability>(value)
            .expect("bootstrap event stream should stage");
        let staged = activation
            .finish(true)
            .expect("bootstrap event stream publication should be complete");
        context
            .commit(
                &staged,
                &[CapabilitySnapshot {
                    key: RuntimeEventStreamCapability::KEY.to_string(),
                    provider_component: provider_component.to_string(),
                    generation: 1,
                }],
            )
            .expect("bootstrap event stream should commit");
        context
            .require::<RuntimeEventStreamCapability>()
            .expect("bootstrap event stream should be visible")
    }

    pub(super) fn require<C>(&self) -> Result<CapabilityLease<C>, RuntimeContextError>
    where
        C: RuntimeCapability + 'static,
    {
        let key = CapabilityKey::from(C::KEY);
        let state = lock_state(&self.state);
        let slot = state
            .slots
            .get(&key)
            .ok_or(RuntimeContextError::MissingCapability { capability: C::KEY })?;
        let CapabilityVisibility::Visible(generation) = slot.visibility else {
            return Err(RuntimeContextError::MissingCapability { capability: C::KEY });
        };
        let value = Arc::clone(&slot.value)
            .downcast::<C::Value>()
            .map_err(|_| RuntimeContextError::CapabilityTypeMismatch { capability: C::KEY })?;
        Ok(CapabilityLease {
            value,
            state: Arc::downgrade(&self.state),
            provider_component: slot.provider_component.clone(),
            registration_id: slot.registration_id,
            generation,
            marker: PhantomData,
        })
    }

    pub(super) fn optional<C>(&self) -> Result<Option<CapabilityLease<C>>, RuntimeContextError>
    where
        C: RuntimeCapability + 'static,
    {
        match self.require::<C>() {
            Ok(lease) => Ok(Some(lease)),
            Err(RuntimeContextError::MissingCapability { .. }) => Ok(None),
            Err(error) => Err(error),
        }
    }

    pub(super) fn snapshots(&self) -> Vec<RuntimeCapabilitySnapshot> {
        lock_state(&self.state)
            .slots
            .iter()
            .filter_map(|(key, slot)| {
                let CapabilityVisibility::Visible(generation) = slot.visibility else {
                    return None;
                };
                Some(RuntimeCapabilitySnapshot {
                    key: key.to_string(),
                    provider_component: slot.provider_component.clone(),
                    generation,
                })
            })
            .collect()
    }

    fn stage(
        &self,
        provider_component: &str,
        key: CapabilityKey,
        value: Arc<dyn Any + Send + Sync>,
    ) -> Result<(ContextRegistration, StagedCapability), ContextPublicationError> {
        let mut state = lock_state(&self.state);
        if state.slots.contains_key(&key) {
            return Err(ContextPublicationError::DuplicateCapability {
                capability: key.to_string(),
            });
        }
        let registration_id = state.next_registration_id;
        state.next_registration_id = registration_id
            .checked_add(1)
            .ok_or(ContextPublicationError::RegistrationIdentityExhausted)?;
        state.slots.insert(
            key.clone(),
            ContextSlot {
                provider_component: provider_component.to_string(),
                registration_id,
                visibility: CapabilityVisibility::Pending,
                value,
                revocation_subscriptions: BTreeMap::new(),
            },
        );
        drop(state);

        Ok((
            ContextRegistration {
                state: Arc::downgrade(&self.state),
                key: key.clone(),
                provider_component: provider_component.to_string(),
                registration_id,
                is_disposed: false,
            },
            StagedCapability {
                key,
                provider_component: provider_component.to_string(),
                registration_id,
            },
        ))
    }

    pub(super) fn commit(
        &self,
        staged: &[StagedCapability],
        graph_capabilities: &[CapabilitySnapshot],
    ) -> Result<(), ContextPublicationError> {
        let publications = graph_capabilities
            .iter()
            .map(|snapshot| (snapshot.key.as_str(), snapshot))
            .collect::<BTreeMap<_, _>>();
        let mut state = lock_state(&self.state);

        for staged_capability in staged {
            let capability = staged_capability.key.to_string();
            let publication = publications.get(capability.as_str()).ok_or_else(|| {
                ContextPublicationError::MissingGraphPublication {
                    capability: capability.clone(),
                }
            })?;
            if publication.provider_component != staged_capability.provider_component {
                return Err(ContextPublicationError::MissingGraphPublication { capability });
            }
            let is_current = state.slots.get(&staged_capability.key).is_some_and(|slot| {
                slot.registration_id == staged_capability.registration_id
                    && slot.provider_component == staged_capability.provider_component
                    && slot.visibility == CapabilityVisibility::Pending
            });
            if !is_current {
                return Err(ContextPublicationError::StaleStaging { capability });
            }
        }

        for staged_capability in staged {
            let capability = staged_capability.key.to_string();
            let generation = publications
                .get(capability.as_str())
                .expect("validated graph publication should remain available")
                .generation;
            state
                .slots
                .get_mut(&staged_capability.key)
                .expect("validated staged capability should remain available")
                .visibility = CapabilityVisibility::Visible(generation);
        }
        Ok(())
    }

    pub(super) fn hide_batch(
        &self,
        graph_capabilities: &[CapabilitySnapshot],
    ) -> Result<(), ContextPublicationError> {
        let mut state = lock_state(&self.state);
        #[cfg(test)]
        if state.reject_next_hide {
            state.reject_next_hide = false;
            return Err(ContextPublicationError::VisibilityMismatch {
                capability: graph_capabilities.first().map_or_else(
                    || "injected_capability".to_string(),
                    |item| item.key.clone(),
                ),
            });
        }
        for capability in graph_capabilities {
            let key = CapabilityKey::from(capability.key.clone());
            let matches_graph = state.slots.get(&key).is_some_and(|slot| {
                slot.provider_component == capability.provider_component
                    && matches!(
                        slot.visibility,
                        CapabilityVisibility::Visible(generation)
                            | CapabilityVisibility::Revoking(generation)
                            if generation == capability.generation
                    )
            });
            if !matches_graph {
                return Err(ContextPublicationError::VisibilityMismatch {
                    capability: capability.key.clone(),
                });
            }
        }
        let subscriptions = graph_capabilities
            .iter()
            .map(|capability| {
                let key = CapabilityKey::from(capability.key.clone());
                let slot = state
                    .slots
                    .get_mut(&key)
                    .expect("validated capability slot should remain available");
                slot.visibility = CapabilityVisibility::Revoking(capability.generation);
                slot.revocation_subscriptions
                    .values()
                    .cloned()
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();
        drop(state);

        let cleanup_succeeded = subscriptions
            .into_iter()
            .flatten()
            .map(|subscription| subscription.try_revoke())
            .fold(true, |all_succeeded, succeeded| all_succeeded & succeeded);
        if !cleanup_succeeded {
            return Err(ContextPublicationError::DependencyCleanupPending {
                capability: graph_capabilities
                    .first()
                    .map_or_else(|| "runtime_dependency".to_string(), |item| item.key.clone()),
            });
        }

        let mut state = lock_state(&self.state);
        for capability in graph_capabilities {
            let key = CapabilityKey::from(capability.key.clone());
            let can_remove = state.slots.get(&key).is_some_and(|slot| {
                slot.provider_component == capability.provider_component
                    && slot.visibility == CapabilityVisibility::Revoking(capability.generation)
            });
            if !can_remove {
                return Err(ContextPublicationError::VisibilityMismatch {
                    capability: capability.key.clone(),
                });
            }
        }
        let revoked_slots = graph_capabilities
            .iter()
            .map(|capability| {
                let key = CapabilityKey::from(capability.key.clone());
                state
                    .slots
                    .remove(&key)
                    .expect("validated revoking slot should remain available")
            })
            .collect::<Vec<_>>();
        drop(state);
        drop(revoked_slots);
        Ok(())
    }

    #[cfg(test)]
    pub(super) fn reject_next_hide(&self) {
        lock_state(&self.state).reject_next_hide = true;
    }
}

#[cfg(test)]
impl RuntimeContext {
    pub(super) fn test_event_stream_lease(
        value: RuntimeEventNotifier,
    ) -> CapabilityLease<RuntimeEventStreamCapability> {
        Self::event_stream_lease(value, "test_runtime_event_stream")
    }
}

impl ComponentActivationContext<'_> {
    /// 暂存一个已声明 typed value，并把 identity-safe inverse 绑定到当前 component scope。
    pub(super) fn publish<C>(&mut self, value: C::Value) -> Result<(), ContextPublicationError>
    where
        C: RuntimeCapability,
    {
        self.publish_erased(CapabilityKey::from(C::KEY), Arc::new(value))
    }

    /// 从当前 stable Context 解析一个 generation-bound dependency lease。
    pub(super) fn require<C>(&self) -> Result<CapabilityLease<C>, RuntimeContextError>
    where
        C: RuntimeCapability + 'static,
    {
        let lease = self.context.require::<C>()?;
        let mut retained_lease = Some(lease.clone());
        self.scope
            .register(format!("dependency:{}", C::KEY), move || {
                retained_lease.take();
                Ok(())
            })
            .map_err(|_| RuntimeContextError::DependencyRetentionRejected { capability: C::KEY })?;
        Ok(lease)
    }

    #[cfg(test)]
    pub(super) fn publish_declared_presence(&mut self) -> Result<(), ContextPublicationError> {
        for key in self
            .declared_capabilities
            .iter()
            .cloned()
            .collect::<Vec<_>>()
        {
            self.publish_erased(key, Arc::new(()))?;
        }
        Ok(())
    }

    fn publish_erased(
        &mut self,
        key: CapabilityKey,
        value: Arc<dyn Any + Send + Sync>,
    ) -> Result<(), ContextPublicationError> {
        if !self.declared_capabilities.contains(&key) {
            return Err(ContextPublicationError::UndeclaredCapability {
                component_id: self.component_id.to_string(),
                capability: key.to_string(),
            });
        }
        let (mut registration, staged) =
            self.context.stage(self.component_id, key.clone(), value)?;
        self.scope
            .register(format!("capability:{key}"), move || {
                registration
                    .dispose()
                    .map_err(|()| "runtime capability dependency cleanup is pending".to_string())
            })
            .map_err(|_| ContextPublicationError::EffectRegistrationRejected {
                capability: key.to_string(),
            })?;
        self.staged.push(staged);
        Ok(())
    }

    pub(super) fn finish(
        self,
        publishes_capabilities: bool,
    ) -> Result<Vec<StagedCapability>, ContextPublicationError> {
        let staged_keys = self
            .staged
            .iter()
            .map(|capability| capability.key.clone())
            .collect::<BTreeSet<_>>();
        let is_complete = if publishes_capabilities {
            staged_keys == self.declared_capabilities
        } else {
            staged_keys.is_empty()
        };
        if !is_complete {
            return Err(ContextPublicationError::IncompletePublication {
                component_id: self.component_id.to_string(),
            });
        }
        Ok(self.staged)
    }
}

impl ContextRegistration {
    fn dispose(&mut self) -> Result<(), ()> {
        if self.is_disposed {
            return Ok(());
        }
        let Some(context) = self.state.upgrade() else {
            self.is_disposed = true;
            return Ok(());
        };
        let mut state = lock_state(&context);
        let owns_current_slot = state.slots.get(&self.key).is_some_and(|slot| {
            slot.provider_component == self.provider_component
                && slot.registration_id == self.registration_id
        });
        if !owns_current_slot {
            self.is_disposed = true;
            return Ok(());
        }
        let slot = state
            .slots
            .get_mut(&self.key)
            .expect("owned capability slot should remain available");
        let generation = match slot.visibility {
            CapabilityVisibility::Visible(generation)
            | CapabilityVisibility::Revoking(generation) => Some(generation),
            CapabilityVisibility::Pending => None,
        };
        let subscriptions = slot
            .revocation_subscriptions
            .values()
            .cloned()
            .collect::<Vec<_>>();
        if let Some(generation) = generation {
            slot.visibility = CapabilityVisibility::Revoking(generation);
        }
        drop(state);
        let cleanup_succeeded = subscriptions
            .into_iter()
            .map(|subscription| subscription.try_revoke())
            .fold(true, |all_succeeded, succeeded| all_succeeded & succeeded);
        if !cleanup_succeeded {
            return Err(());
        }
        let mut state = lock_state(&context);
        let revoked_slot = state.slots.get(&self.key).is_some_and(|slot| {
            slot.provider_component == self.provider_component
                && slot.registration_id == self.registration_id
        });
        if revoked_slot {
            state.slots.remove(&self.key);
        }
        self.is_disposed = true;
        Ok(())
    }
}

impl Drop for ContextRegistration {
    fn drop(&mut self) {
        let _ = self.dispose();
    }
}

fn lock_state(
    state: &Arc<Mutex<RuntimeContextState>>,
) -> std::sync::MutexGuard<'_, RuntimeContextState> {
    state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    struct TestCapability;

    impl RuntimeCapability for TestCapability {
        const KEY: &'static str = "test_capability";
        type Value = String;
    }

    struct CollidingCapability;

    impl RuntimeCapability for CollidingCapability {
        const KEY: &'static str = TestCapability::KEY;
        type Value = usize;
    }

    struct SecondCapability;

    impl RuntimeCapability for SecondCapability {
        const KEY: &'static str = "second_capability";
        type Value = usize;
    }

    fn graph_snapshot(generation: u64) -> CapabilitySnapshot {
        CapabilitySnapshot {
            key: TestCapability::KEY.to_string(),
            provider_component: "test_provider".to_string(),
            generation,
        }
    }

    #[test]
    fn staged_value_is_hidden_until_atomic_commit() {
        let context = RuntimeContext::default();
        let scope = EffectScope::default();
        let mut activation = context.activation(
            &scope,
            "test_provider",
            [CapabilityKey::from(TestCapability::KEY)],
        );
        activation
            .publish::<TestCapability>("DELIVERY_SECRET".to_string())
            .expect("declared capability should stage");
        assert!(
            context
                .optional::<TestCapability>()
                .expect("typed lookup should succeed")
                .is_none()
        );

        let staged = activation
            .finish(true)
            .expect("declared publication should be complete");
        context
            .commit(&staged, &[graph_snapshot(4)])
            .expect("matching graph publication should commit");

        let lease = context
            .require::<TestCapability>()
            .expect("committed value should resolve");
        assert_eq!(lease.as_str(), "DELIVERY_SECRET");
        assert_eq!(lease.provider_component(), "test_provider");
        assert_eq!(lease.generation(), 4);
        assert!(!format!("{lease:?}").contains("DELIVERY_SECRET"));
        let guard = lease.generation_guard();
        assert!(guard.is_current());
        assert_eq!(guard.key(), TestCapability::KEY);
        assert_eq!(guard.generation(), 4);
        assert!(!format!("{guard:?}").contains("DELIVERY_SECRET"));
    }

    #[test]
    fn scope_disposal_rolls_back_pending_and_visible_slots_exactly_once() {
        let context = RuntimeContext::default();
        let scope = EffectScope::default();
        let mut activation = context.activation(
            &scope,
            "test_provider",
            [CapabilityKey::from(TestCapability::KEY)],
        );
        activation
            .publish::<TestCapability>("value".to_string())
            .expect("declared capability should stage");
        let staged = activation.finish(true).expect("publication should finish");
        context
            .commit(&staged, &[graph_snapshot(0)])
            .expect("publication should commit");

        assert!(scope.dispose().failures.is_empty());
        assert!(scope.dispose().failures.is_empty());
        assert!(
            context
                .optional::<TestCapability>()
                .expect("typed lookup should succeed")
                .is_none()
        );
        assert!(context.snapshots().is_empty());
    }

    #[test]
    fn stale_scope_inverse_cannot_remove_replacement_generation() {
        let context = RuntimeContext::default();
        let old_scope = EffectScope::default();
        let mut old_activation = context.activation(
            &old_scope,
            "test_provider",
            [CapabilityKey::from(TestCapability::KEY)],
        );
        old_activation
            .publish::<TestCapability>("old".to_string())
            .expect("old value should stage");
        let old_staged = old_activation
            .finish(true)
            .expect("old stage should finish");
        context
            .commit(&old_staged, &[graph_snapshot(0)])
            .expect("old value should commit");
        let old_lease = context
            .require::<TestCapability>()
            .expect("old lease should resolve");
        let old_guard = old_lease.generation_guard();
        context
            .hide_batch(&[graph_snapshot(0)])
            .expect("old generation should hide");
        assert!(!old_guard.is_current());

        let new_scope = EffectScope::default();
        let mut new_activation = context.activation(
            &new_scope,
            "test_provider",
            [CapabilityKey::from(TestCapability::KEY)],
        );
        new_activation
            .publish::<TestCapability>("new".to_string())
            .expect("new value should stage");
        let new_staged = new_activation
            .finish(true)
            .expect("new stage should finish");
        context
            .commit(&new_staged, &[graph_snapshot(1)])
            .expect("new value should commit");

        assert!(old_scope.dispose().failures.is_empty());
        assert_eq!(old_lease.as_str(), "old");
        let new_lease = context
            .require::<TestCapability>()
            .expect("new lease should remain visible");
        assert_eq!(new_lease.as_str(), "new");
        assert_eq!(new_lease.generation(), 1);
        assert!(new_lease.generation_guard().is_current());
    }

    #[test]
    fn generation_guard_fails_closed_after_context_drop() {
        let scope = EffectScope::default();
        let guard = {
            let context = RuntimeContext::default();
            let mut activation = context.activation(
                &scope,
                "test_provider",
                [CapabilityKey::from(TestCapability::KEY)],
            );
            activation
                .publish::<TestCapability>("DELIVERY_SECRET".to_string())
                .expect("test capability should stage");
            let staged = activation.finish(true).expect("stage should finish");
            context
                .commit(&staged, &[graph_snapshot(7)])
                .expect("test capability should commit");
            context
                .require::<TestCapability>()
                .expect("test lease should resolve")
                .generation_guard()
        };

        assert!(!guard.is_current());
        assert!(!format!("{guard:?}").contains("DELIVERY_SECRET"));
    }

    #[test]
    fn generation_revocation_subscription_fires_once_for_the_original_slot() {
        let context = RuntimeContext::default();
        let old_scope = EffectScope::default();
        let mut activation = context.activation(
            &old_scope,
            "test_provider",
            [CapabilityKey::from(TestCapability::KEY)],
        );
        activation
            .publish::<TestCapability>("old".to_string())
            .expect("old capability should stage");
        let staged = activation.finish(true).expect("old stage should finish");
        context
            .commit(&staged, &[graph_snapshot(8)])
            .expect("old capability should commit");
        let guard = context
            .require::<TestCapability>()
            .expect("old lease should resolve")
            .generation_guard();
        let revocations = Arc::new(AtomicUsize::new(0));
        let revocation_probe = Arc::clone(&revocations);
        let _subscription = guard
            .subscribe_revocation(move || {
                revocation_probe.fetch_add(1, Ordering::SeqCst);
                Ok(())
            })
            .expect("revocation observer should subscribe");

        context
            .hide_batch(&[graph_snapshot(8)])
            .expect("old generation should hide");
        assert_eq!(revocations.load(Ordering::SeqCst), 1);
        assert!(!guard.is_current());

        let new_scope = EffectScope::default();
        let mut activation = context.activation(
            &new_scope,
            "test_provider",
            [CapabilityKey::from(TestCapability::KEY)],
        );
        activation
            .publish::<TestCapability>("new".to_string())
            .expect("new capability should stage");
        let staged = activation.finish(true).expect("new stage should finish");
        context
            .commit(&staged, &[graph_snapshot(9)])
            .expect("new capability should commit");
        assert!(old_scope.dispose().is_success());
        assert_eq!(revocations.load(Ordering::SeqCst), 1);
        assert!(
            context
                .require::<TestCapability>()
                .expect("fresh lease should resolve")
                .generation_guard()
                .is_current()
        );
    }

    #[test]
    fn subscription_drop_removes_the_original_slot_entry_without_tombstones() {
        let context = RuntimeContext::default();
        let scope = EffectScope::default();
        let mut activation = context.activation(
            &scope,
            "test_provider",
            [CapabilityKey::from(TestCapability::KEY)],
        );
        activation
            .publish::<TestCapability>("value".to_string())
            .expect("capability should stage");
        let staged = activation.finish(true).expect("stage should finish");
        context
            .commit(&staged, &[graph_snapshot(10)])
            .expect("capability should commit");
        let guard = context
            .require::<TestCapability>()
            .expect("lease should resolve")
            .generation_guard();

        for _ in 0..16 {
            let subscription = guard
                .subscribe_revocation(|| Ok(()))
                .expect("observer should subscribe");
            assert_eq!(
                lock_state(&context.state)
                    .slots
                    .get(&CapabilityKey::from(TestCapability::KEY))
                    .expect("visible slot should remain")
                    .revocation_subscriptions
                    .len(),
                1
            );
            drop(subscription);
            assert!(
                lock_state(&context.state)
                    .slots
                    .get(&CapabilityKey::from(TestCapability::KEY))
                    .expect("visible slot should remain")
                    .revocation_subscriptions
                    .is_empty()
            );
        }
    }

    #[test]
    fn dependency_cleanup_failure_keeps_a_revoking_slot_as_a_fresh_generation_barrier() {
        let context = RuntimeContext::default();
        let old_scope = EffectScope::default();
        let mut activation = context.activation(
            &old_scope,
            "test_provider",
            [CapabilityKey::from(TestCapability::KEY)],
        );
        activation
            .publish::<TestCapability>("old".to_string())
            .expect("old capability should stage");
        let staged = activation.finish(true).expect("old stage should finish");
        context
            .commit(&staged, &[graph_snapshot(11)])
            .expect("old capability should commit");
        let guard = context
            .require::<TestCapability>()
            .expect("old lease should resolve")
            .generation_guard();
        let attempts = Arc::new(AtomicUsize::new(0));
        let attempt_probe = Arc::clone(&attempts);
        let _subscription = guard
            .subscribe_revocation(move || {
                if attempt_probe.fetch_add(1, Ordering::SeqCst) == 0 {
                    Err(())
                } else {
                    Ok(())
                }
            })
            .expect("observer should subscribe");

        let failure = context
            .hide_batch(&[graph_snapshot(11)])
            .expect_err("first cleanup attempt should remain pending");
        assert!(matches!(
            failure,
            ContextPublicationError::DependencyCleanupPending { .. }
        ));
        assert!(!guard.is_current());
        assert!(matches!(
            context.require::<TestCapability>(),
            Err(RuntimeContextError::MissingCapability { .. })
        ));
        let fresh_scope = EffectScope::default();
        let mut fresh_activation = context.activation(
            &fresh_scope,
            "test_provider",
            [CapabilityKey::from(TestCapability::KEY)],
        );
        assert!(matches!(
            fresh_activation.publish::<TestCapability>("fresh".to_string()),
            Err(ContextPublicationError::DuplicateCapability { .. })
        ));

        context
            .hide_batch(&[graph_snapshot(11)])
            .expect("cleanup retry should release the old slot");
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
        fresh_activation
            .publish::<TestCapability>("fresh".to_string())
            .expect("fresh authority should stage only after cleanup");
    }

    #[test]
    fn registration_inverse_requires_provider_and_private_identity() {
        let context = RuntimeContext::default();
        let scope = EffectScope::default();
        let mut activation = context.activation(
            &scope,
            "test_provider",
            [CapabilityKey::from(TestCapability::KEY)],
        );
        activation
            .publish::<TestCapability>("value".to_string())
            .expect("test capability should stage");

        lock_state(&context.state)
            .slots
            .get_mut(&CapabilityKey::from(TestCapability::KEY))
            .expect("staged slot should exist")
            .provider_component = "replacement_provider".to_string();
        assert!(scope.dispose().failures.is_empty());

        assert!(
            lock_state(&context.state)
                .slots
                .contains_key(&CapabilityKey::from(TestCapability::KEY)),
            "provider mismatch must make the old inverse a no-op"
        );
    }

    #[test]
    fn publication_validation_is_atomic_and_diagnostics_are_redacted() {
        let context = RuntimeContext::default();
        let scope = EffectScope::default();
        let mut activation = context.activation(
            &scope,
            "test_provider",
            [CapabilityKey::from(TestCapability::KEY)],
        );
        activation
            .publish::<TestCapability>("DELIVERY_SECRET".to_string())
            .expect("declared capability should stage");
        let staged = activation.finish(true).expect("stage should finish");
        let error = context
            .commit(&staged, &[])
            .expect_err("missing graph publication should reject the batch");
        assert!(context.snapshots().is_empty());
        let debug = format!("{error:?}");
        assert!(!debug.contains("DELIVERY_SECRET"));
        assert!(!debug.contains("String"));

        assert!(scope.dispose().failures.is_empty());
        assert!(matches!(
            context.require::<TestCapability>(),
            Err(RuntimeContextError::MissingCapability { .. })
        ));
    }

    #[test]
    fn batch_validation_does_not_publish_an_earlier_valid_slot() {
        let context = RuntimeContext::default();
        let scope = EffectScope::default();
        let mut activation = context.activation(
            &scope,
            "test_provider",
            [
                CapabilityKey::from(TestCapability::KEY),
                CapabilityKey::from(SecondCapability::KEY),
            ],
        );
        activation
            .publish::<TestCapability>("first".to_string())
            .expect("first capability should stage");
        activation
            .publish::<SecondCapability>(2)
            .expect("second capability should stage");
        let staged = activation.finish(true).expect("batch should finish");

        let error = context
            .commit(&staged, &[graph_snapshot(3)])
            .expect_err("incomplete graph publication should reject the whole batch");
        assert!(matches!(
            error,
            ContextPublicationError::MissingGraphPublication { .. }
        ));
        assert!(context.snapshots().is_empty());
        assert!(
            context
                .optional::<TestCapability>()
                .expect("typed lookup should succeed")
                .is_none()
        );
        assert!(
            context
                .optional::<SecondCapability>()
                .expect("typed lookup should succeed")
                .is_none()
        );
    }

    #[test]
    fn disposed_scope_rejects_staging_without_leaving_a_slot() {
        let context = RuntimeContext::default();
        let scope = EffectScope::default();
        assert!(scope.dispose().failures.is_empty());
        let mut activation = context.activation(
            &scope,
            "test_provider",
            [CapabilityKey::from(TestCapability::KEY)],
        );

        let error = activation
            .publish::<TestCapability>("value".to_string())
            .expect_err("inactive scope must reject context registration");
        assert!(matches!(
            error,
            ContextPublicationError::EffectRegistrationRejected { .. }
        ));
        assert!(context.snapshots().is_empty());
        assert!(
            context
                .optional::<TestCapability>()
                .expect("typed lookup should succeed")
                .is_none()
        );
    }

    #[test]
    fn type_collision_is_closed_and_does_not_expose_value_or_type_name() {
        let context = RuntimeContext::default();
        let scope = EffectScope::default();
        let mut activation = context.activation(
            &scope,
            "test_provider",
            [CapabilityKey::from(TestCapability::KEY)],
        );
        activation
            .publish::<TestCapability>("DELIVERY_SECRET".to_string())
            .expect("test capability should stage");
        let staged = activation.finish(true).expect("stage should finish");
        context
            .commit(&staged, &[graph_snapshot(0)])
            .expect("test capability should commit");

        let error = context
            .require::<CollidingCapability>()
            .expect_err("wrong typed marker must reject the erased slot");
        assert!(matches!(
            error,
            RuntimeContextError::CapabilityTypeMismatch { .. }
        ));
        assert!(matches!(
            context.optional::<CollidingCapability>(),
            Err(RuntimeContextError::CapabilityTypeMismatch { .. })
        ));
        let debug = format!("{error:?}");
        assert!(!debug.contains("DELIVERY_SECRET"));
        assert!(!debug.contains("String"));
        assert!(!debug.contains("usize"));
    }
}
