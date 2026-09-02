//! Agent scope 内显式授权、generation validation 与可逆副作用 ownership。

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicU64, Ordering},
    },
};

use tokio_util::sync::CancellationToken;
use tool_runtime::{
    Tool, ToolCall, ToolDefinition, ToolExecutionContext, ToolExecutionFuture, ToolExecutor,
    ToolExecutorRegistry, ToolPermissionPreview, ToolResult,
};

use super::{
    context::{
        CapabilityGenerationGuard, CapabilityLease, CapabilityRevocationSubscription,
        PromptAssemblyCapability, RuntimeCapability, ToolCatalogCapability,
    },
    effect_scope::{
        EffectDisposeReport, EffectScope, EffectScopeActivationStatus,
        EffectScopeDisposalSubscription, EffectScopeError,
    },
    prompt_assembly::PromptAssemblySessionSnapshot,
};

const SCOPED_TOOL_UNAVAILABLE: &str = "Agent-scoped tool capability is unavailable";

/// Agent child context 可见的稳定 semantic capability key。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum AgentCapabilityKey {
    Tools,
    Prompt,
}

impl AgentCapabilityKey {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Tools => ToolCatalogCapability::KEY,
            Self::Prompt => PromptAssemblyCapability::KEY,
        }
    }
}

impl fmt::Display for AgentCapabilityKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Agent context owner 是 control-plane 创建的稳定 identity，不接受 delivery text。
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(super) struct AgentContextOwner(String);

impl AgentContextOwner {
    pub(super) fn try_new(value: impl Into<String>) -> Result<Self, AgentContextOwnerError> {
        let value = value.into();
        if value.is_empty() {
            return Err(AgentContextOwnerError::Empty);
        }
        if value.len() > 64 {
            return Err(AgentContextOwnerError::TooLong);
        }
        let bytes = value.as_bytes();
        let is_separator = |byte: u8| matches!(byte, b'-' | b'_');
        if !bytes.first().is_some_and(u8::is_ascii_alphanumeric)
            || !bytes.last().is_some_and(u8::is_ascii_alphanumeric)
            || bytes.iter().any(|byte| {
                !byte.is_ascii_lowercase() && !byte.is_ascii_digit() && !is_separator(*byte)
            })
            || bytes
                .windows(2)
                .any(|pair| is_separator(pair[0]) && is_separator(pair[1]))
        {
            return Err(AgentContextOwnerError::InvalidFormat);
        }
        Ok(Self(value))
    }

    pub(super) fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for AgentContextOwner {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("AgentContextOwner")
            .field(&self.0)
            .finish()
    }
}

impl fmt::Display for AgentContextOwner {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// Invalid owner diagnostics 只暴露封闭原因，不保留输入文本。
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub(super) enum AgentContextOwnerError {
    #[error("Agent context owner is empty")]
    Empty,
    #[error("Agent context owner format is invalid")]
    InvalidFormat,
    #[error("Agent context owner is too long")]
    TooLong,
}

/// Scoped context 操作只返回 closed control metadata。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub(super) enum AgentCapabilityContextError {
    #[error("Agent capability context owner is unavailable")]
    OwnerUnavailable,
    #[error("Agent capability context is unavailable")]
    Unavailable,
    #[error("Agent capability `{capability}` is unavailable")]
    MissingCapability { capability: AgentCapabilityKey },
    #[error("Agent capability context epoch identity is exhausted")]
    EpochExhausted,
    #[error("Agent capability context scope identity is exhausted")]
    ScopeIdentityExhausted,
    #[error("Agent capability context effect identity is exhausted")]
    EffectIdentityExhausted,
    #[error("Agent capability context rejected effect registration")]
    EffectRegistrationRejected,
}

struct RootToolGrant {
    registry: ToolExecutorRegistry,
    guard: CapabilityGenerationGuard,
}

struct RootPromptGrant {
    snapshot: PromptAssemblySessionSnapshot,
    guard: CapabilityGenerationGuard,
}

/// Root grants 只能由 caller 显式提供的 typed RuntimeContext leases 构造。
#[derive(Default)]
pub(super) struct AgentRootCapabilityGrants {
    tools: Option<RootToolGrant>,
    prompt: Option<RootPromptGrant>,
}

impl AgentRootCapabilityGrants {
    pub(super) fn empty() -> Self {
        Self::default()
    }

    pub(super) fn with_tools(
        mut self,
        lease: &CapabilityLease<ToolCatalogCapability>,
        allowed_tool_names: impl IntoIterator<Item = String>,
    ) -> Self {
        let allowed_tool_names = allowed_tool_names.into_iter().collect::<BTreeSet<_>>();
        let guard = lease.generation_guard();
        debug_assert_eq!(guard.key(), AgentCapabilityKey::Tools.as_str());
        self.tools = Some(RootToolGrant {
            registry: lease.filtered(|tool_name| allowed_tool_names.contains(tool_name)),
            guard,
        });
        self
    }

    pub(super) fn with_prompt(mut self, lease: &CapabilityLease<PromptAssemblyCapability>) -> Self {
        let guard = lease.generation_guard();
        debug_assert_eq!(guard.key(), AgentCapabilityKey::Prompt.as_str());
        self.prompt = Some(RootPromptGrant {
            snapshot: lease.session_snapshot(),
            guard,
        });
        self
    }

    fn capability_count(&self) -> usize {
        usize::from(self.tools.is_some()) + usize::from(self.prompt.is_some())
    }
}

impl fmt::Debug for AgentRootCapabilityGrants {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AgentRootCapabilityGrants")
            .field("capability_count", &self.capability_count())
            .finish_non_exhaustive()
    }
}

#[derive(Default)]
enum ToolGrantChoice {
    #[default]
    Omit,
    Inherit,
    InheritFiltered(BTreeSet<String>),
    Shadow(ToolExecutorRegistry),
}

impl ToolGrantChoice {
    const fn kind(&self) -> &'static str {
        match self {
            Self::Omit => "omit",
            Self::Inherit => "inherit",
            Self::InheritFiltered(_) => "inherit_filtered",
            Self::Shadow(_) => "shadow",
        }
    }
}

#[derive(Default)]
enum PromptGrantChoice {
    #[default]
    Omit,
    Inherit,
    Shadow(Box<PromptAssemblySessionSnapshot>),
}

impl PromptGrantChoice {
    const fn kind(&self) -> &'static str {
        match self {
            Self::Omit => "omit",
            Self::Inherit => "inherit",
            Self::Shadow(_) => "shadow",
        }
    }
}

/// Child grants 对每个 semantic capability 显式选择 inherit、filter、shadow 或 omit。
#[derive(Default)]
pub(super) struct AgentChildCapabilityGrants {
    tools: ToolGrantChoice,
    prompt: PromptGrantChoice,
}

impl AgentChildCapabilityGrants {
    pub(super) fn empty() -> Self {
        Self::default()
    }

    pub(super) fn inherit_tools(mut self) -> Self {
        self.tools = ToolGrantChoice::Inherit;
        self
    }

    pub(super) fn inherit_filtered_tools(
        mut self,
        allowed_tool_names: impl IntoIterator<Item = String>,
    ) -> Self {
        self.tools = ToolGrantChoice::InheritFiltered(
            allowed_tool_names.into_iter().collect::<BTreeSet<_>>(),
        );
        self
    }

    pub(super) fn shadow_tools(mut self, tools: ToolExecutorRegistry) -> Self {
        self.tools = ToolGrantChoice::Shadow(tools.filtered(|_| true));
        self
    }

    pub(super) fn inherit_prompt(mut self) -> Self {
        self.prompt = PromptGrantChoice::Inherit;
        self
    }

    pub(super) fn shadow_prompt(mut self, prompt: PromptAssemblySessionSnapshot) -> Self {
        self.prompt = PromptGrantChoice::Shadow(Box::new(prompt));
        self
    }

    fn choice_count(&self) -> usize {
        usize::from(!matches!(self.tools, ToolGrantChoice::Omit))
            + usize::from(!matches!(self.prompt, PromptGrantChoice::Omit))
    }
}

impl fmt::Debug for AgentChildCapabilityGrants {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AgentChildCapabilityGrants")
            .field("choice_count", &self.choice_count())
            .field("tools", &self.tools.kind())
            .field("prompt", &self.prompt.kind())
            .finish_non_exhaustive()
    }
}

#[derive(Clone)]
struct ScopedToolCapability {
    registry: ToolExecutorRegistry,
    generation: u64,
}

#[derive(Clone)]
struct ScopedPromptCapability {
    snapshot: PromptAssemblySessionSnapshot,
    generation: u64,
}

#[derive(Clone)]
enum ScopedCapability {
    Tools(ScopedToolCapability),
    Prompt(Box<ScopedPromptCapability>),
}

impl ScopedCapability {
    const fn generation(&self) -> u64 {
        match self {
            Self::Tools(capability) => capability.generation,
            Self::Prompt(capability) => capability.generation,
        }
    }
}

struct AgentContextTree {
    next_epoch: AtomicU64,
    root_guards: Vec<CapabilityGenerationGuard>,
}

impl AgentContextTree {
    fn new(root_guards: Vec<CapabilityGenerationGuard>) -> Self {
        Self {
            next_epoch: AtomicU64::new(1),
            root_guards,
        }
    }

    fn allocate_epoch(&self) -> Result<u64, AgentCapabilityContextError> {
        self.next_epoch
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                value.checked_add(1)
            })
            .map_err(|_| AgentCapabilityContextError::EpochExhausted)
    }

    fn root_generations_are_current(&self) -> bool {
        self.root_guards
            .iter()
            .all(CapabilityGenerationGuard::is_current)
    }
}

struct AgentCapabilityContextState {
    owner: AgentContextOwner,
    epoch: u64,
    tree: Arc<AgentContextTree>,
    parent: Option<Weak<AgentCapabilityContextState>>,
    scope: EffectScope,
    capabilities: BTreeMap<AgentCapabilityKey, ScopedCapability>,
    next_effect_id: AtomicU64,
    revocation_subscriptions: Arc<Mutex<Vec<CapabilityRevocationSubscription>>>,
    _scope_disposal_subscription: Option<EffectScopeDisposalSubscription>,
}

/// Agent scope token 同时绑定 private context identity 与 epoch。
#[derive(Clone)]
pub(super) struct AgentContextToken {
    state: Weak<AgentCapabilityContextState>,
    epoch: u64,
}

impl AgentContextToken {
    fn validate(&self) -> Result<Arc<AgentCapabilityContextState>, AgentCapabilityContextError> {
        let state = self
            .state
            .upgrade()
            .ok_or(AgentCapabilityContextError::Unavailable)?;
        if context_is_current(&state, self.epoch) {
            Ok(state)
        } else {
            Err(AgentCapabilityContextError::Unavailable)
        }
    }
}

impl fmt::Debug for AgentContextToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AgentContextToken")
            .field("epoch", &self.epoch)
            .finish_non_exhaustive()
    }
}

/// Agent context ownership handle；clone 不复制 authority，最后一个 owner Drop 才执行 fallback cleanup。
#[derive(Clone)]
pub(super) struct AgentCapabilityContext {
    state: Arc<AgentCapabilityContextState>,
}

impl AgentCapabilityContext {
    pub(super) fn root(
        owner: AgentContextOwner,
        parent_scope: &EffectScope,
        grants: AgentRootCapabilityGrants,
    ) -> Result<Self, AgentCapabilityContextError> {
        let (capabilities, guards) = resolve_root_capabilities(grants);
        if !guards.iter().all(CapabilityGenerationGuard::is_current) {
            return Err(AgentCapabilityContextError::Unavailable);
        }
        let tree = Arc::new(AgentContextTree::new(guards.clone()));
        let epoch = tree.allocate_epoch()?;
        let scope = parent_scope
            .child(owner.as_str())
            .map_err(map_scope_creation_error)?;
        let revocation_subscriptions = Arc::new(Mutex::new(Vec::new()));
        let owned_subscriptions = Arc::clone(&revocation_subscriptions);
        let scope_disposal_subscription = scope
            .subscribe_disposed(move || {
                owned_subscriptions
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .clear();
            })
            .map_err(map_scope_creation_error)?;
        let state = Arc::new(AgentCapabilityContextState {
            owner,
            epoch,
            tree,
            parent: None,
            scope,
            capabilities,
            next_effect_id: AtomicU64::new(0),
            revocation_subscriptions,
            _scope_disposal_subscription: Some(scope_disposal_subscription),
        });
        let subscriptions = guards
            .into_iter()
            .map(|guard| {
                let cleanup = state.scope.cleanup_handle();
                guard
                    .subscribe_revocation(move || {
                        cleanup.dispose().is_success().then_some(()).ok_or(())
                    })
                    .map_err(|_| AgentCapabilityContextError::Unavailable)
            })
            .collect::<Result<Vec<_>, _>>()?;
        *state
            .revocation_subscriptions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = subscriptions;
        let context = Self { state };
        if context.validate().is_err() {
            let _ = context.dispose();
            return Err(AgentCapabilityContextError::Unavailable);
        }
        Ok(context)
    }

    pub(super) fn child(
        &self,
        owner: AgentContextOwner,
        grants: AgentChildCapabilityGrants,
    ) -> Result<Self, AgentCapabilityContextError> {
        self.validate()?;

        let epoch = self.state.tree.allocate_epoch()?;
        let capabilities = resolve_child_capabilities(&self.state.capabilities, grants, epoch)?;
        let scope = self
            .state
            .scope
            .child(owner.as_str())
            .map_err(map_scope_creation_error)?;
        let state = Arc::new(AgentCapabilityContextState {
            owner,
            epoch,
            tree: Arc::clone(&self.state.tree),
            parent: Some(Arc::downgrade(&self.state)),
            scope,
            capabilities,
            next_effect_id: AtomicU64::new(0),
            revocation_subscriptions: Arc::new(Mutex::new(Vec::new())),
            _scope_disposal_subscription: None,
        });
        let context = Self { state };
        if context.validate().is_err() {
            let _ = context.dispose();
            return Err(AgentCapabilityContextError::Unavailable);
        }
        Ok(context)
    }

    pub(super) fn token(&self) -> AgentContextToken {
        AgentContextToken {
            state: Arc::downgrade(&self.state),
            epoch: self.state.epoch,
        }
    }

    pub(super) fn tools(&self) -> Result<AgentScopedToolView, AgentCapabilityContextError> {
        self.validate()?;
        let Some(ScopedCapability::Tools(capability)) =
            self.state.capabilities.get(&AgentCapabilityKey::Tools)
        else {
            return Err(AgentCapabilityContextError::MissingCapability {
                capability: AgentCapabilityKey::Tools,
            });
        };
        Ok(AgentScopedToolView {
            registry: capability.registry.clone(),
            token: self.token(),
            context_epoch: self.state.epoch,
            capability_generation: capability.generation,
            cancellation: self.state.scope.cancellation_token(),
        })
    }

    pub(super) fn prompt(&self) -> Result<AgentScopedPromptView, AgentCapabilityContextError> {
        self.validate()?;
        let Some(ScopedCapability::Prompt(capability)) =
            self.state.capabilities.get(&AgentCapabilityKey::Prompt)
        else {
            return Err(AgentCapabilityContextError::MissingCapability {
                capability: AgentCapabilityKey::Prompt,
            });
        };
        Ok(AgentScopedPromptView {
            snapshot: capability.snapshot.clone(),
            token: self.token(),
            context_epoch: self.state.epoch,
            capability_generation: capability.generation,
        })
    }

    pub(super) fn register_effect<D, E>(
        &self,
        token: &AgentContextToken,
        kind: AgentScopedEffectKind,
        activate: impl FnOnce() -> Result<D, E>,
    ) -> Result<(), AgentCapabilityContextError>
    where
        D: FnMut() -> Result<(), String> + Send + 'static,
    {
        let token_state = token.validate()?;
        if !Arc::ptr_eq(&token_state, &self.state) {
            return Err(AgentCapabilityContextError::Unavailable);
        }

        let effect_id = self
            .state
            .next_effect_id
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                value.checked_add(1)
            })
            .map_err(|_| AgentCapabilityContextError::EffectIdentityExhausted)?;
        let status = self
            .state
            .scope
            .activate_revertible_effect(
                format!("agent_effect:{}:{effect_id}", kind.as_str()),
                activate,
            )
            .map_err(|_| AgentCapabilityContextError::EffectRegistrationRejected)?;
        match status {
            EffectScopeActivationStatus::Active => Ok(()),
            EffectScopeActivationStatus::Finalizing => {
                Err(AgentCapabilityContextError::EffectRegistrationRejected)
            }
        }
    }

    /// 将额外的 runtime capability generation 纳入 root context 的 reactive ownership。
    ///
    /// provider generation 撤销时会立即关闭整棵 Agent context tree；callback 只持有
    /// cleanup handle，不把 capability value 或 child runtime authority带回 Context。
    pub(super) fn retain_generation(
        &self,
        guard: CapabilityGenerationGuard,
    ) -> Result<(), AgentCapabilityContextError> {
        self.validate()?;
        let cleanup = self.state.scope.cleanup_handle();
        let subscription = guard
            .subscribe_revocation(move || cleanup.dispose().is_success().then_some(()).ok_or(()))
            .map_err(|_| AgentCapabilityContextError::Unavailable)?;
        self.state
            .revocation_subscriptions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(subscription);
        if self.validate().is_err() {
            let _ = self.dispose();
            return Err(AgentCapabilityContextError::Unavailable);
        }
        Ok(())
    }

    /// 先关闭 descendant admission 并取消 in-flight work，inverse 留给后续 dispose。
    pub(super) fn begin_disposal(&self) {
        self.state.scope.begin_disposal();
    }

    pub(super) fn dispose(&self) -> AgentContextDisposeReport {
        let report = self.state.scope.dispose();
        AgentContextDisposeReport::from_effect_report(report)
    }

    pub(super) fn inspection_snapshot(&self) -> AgentCapabilityContextSnapshot {
        let scope_snapshot = self.state.scope.snapshot();
        let capabilities = self
            .state
            .capabilities
            .iter()
            .map(|(key, capability)| AgentCapabilityGenerationSnapshot {
                key: *key,
                generation: capability.generation(),
            })
            .collect::<Vec<_>>();
        AgentCapabilityContextSnapshot {
            owner: self.state.owner.clone(),
            epoch: self.state.epoch,
            capability_count: capabilities.len(),
            effect_count: scope_snapshot
                .as_ref()
                .map_or(0, |snapshot| snapshot.effects.len()),
            child_count: scope_snapshot
                .as_ref()
                .map_or(0, |snapshot| snapshot.children.len()),
            capabilities,
        }
    }

    fn validate(&self) -> Result<(), AgentCapabilityContextError> {
        if context_is_current(&self.state, self.state.epoch) {
            Ok(())
        } else {
            Err(AgentCapabilityContextError::Unavailable)
        }
    }

    /// Child adapter 在每个 command boundary 验证其 owner context 仍然有效。
    pub(super) fn is_current(&self) -> bool {
        self.validate().is_ok()
    }
}

impl fmt::Debug for AgentCapabilityContext {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let snapshot = self.inspection_snapshot();
        formatter
            .debug_struct("AgentCapabilityContext")
            .field("owner", &snapshot.owner)
            .field("epoch", &snapshot.epoch)
            .field("capability_count", &snapshot.capability_count)
            .field("effect_count", &snapshot.effect_count)
            .field("child_count", &snapshot.child_count)
            .finish_non_exhaustive()
    }
}

/// Agent-owned effect kind 生成 fixed control label，不接受 delivery-derived string。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AgentScopedEffectKind {
    Worker,
    Listener,
    ToolRegistration,
    PromptContribution,
}

impl AgentScopedEffectKind {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Worker => "worker",
            Self::Listener => "listener",
            Self::ToolRegistration => "tool_registration",
            Self::PromptContribution => "prompt_contribution",
        }
    }
}

/// Scoped cleanup report 只暴露尚未收敛的 inverse 数量。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct AgentContextDisposeReport {
    pub(super) pending_cleanup_count: usize,
}

impl AgentContextDisposeReport {
    fn from_effect_report(report: EffectDisposeReport) -> Self {
        Self {
            pending_cleanup_count: report.failures.len(),
        }
    }

    pub(super) const fn is_success(&self) -> bool {
        self.pending_cleanup_count == 0
    }
}

/// Agent context inspection 不包含 prompt/tool body、effect label 或 registration identity。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct AgentCapabilityContextSnapshot {
    pub(super) owner: AgentContextOwner,
    pub(super) epoch: u64,
    pub(super) capability_count: usize,
    pub(super) effect_count: usize,
    pub(super) child_count: usize,
    pub(super) capabilities: Vec<AgentCapabilityGenerationSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct AgentCapabilityGenerationSnapshot {
    pub(super) key: AgentCapabilityKey,
    pub(super) generation: u64,
}

/// Tool view 在每次 definition、preview 与 execution 前验证相同 context-bound token。
#[derive(Clone)]
pub(super) struct AgentScopedToolView {
    registry: ToolExecutorRegistry,
    token: AgentContextToken,
    context_epoch: u64,
    capability_generation: u64,
    cancellation: CancellationToken,
}

impl AgentScopedToolView {
    /// Construction boundary 使用的同源 filtered registry snapshot。
    ///
    /// 该 snapshot 只能由已验证 token 的 scoped view 生成；child adapter 不接触 host
    /// catalog。context disposal 会先取消 child scope，再由 orchestrator quiesce adapter。
    pub(super) fn construction_registry(
        &self,
    ) -> Result<ToolExecutorRegistry, AgentCapabilityContextError> {
        self.token.validate()?;
        let mut registry = ToolExecutorRegistry::new();
        for tool in self.registry.tools() {
            registry.insert(AgentScopedTool {
                tool,
                token: self.token.clone(),
                cancellation: self.cancellation.clone(),
            });
        }
        Ok(registry)
    }

    pub(super) fn definitions(&self) -> Result<Vec<ToolDefinition>, AgentCapabilityContextError> {
        self.token.validate()?;
        Ok(self.registry.definitions().definitions().cloned().collect())
    }

    pub(super) fn permission_preview(
        &self,
        call: &ToolCall,
        cancellation: &CancellationToken,
    ) -> Result<Option<ToolPermissionPreview>, AgentCapabilityContextError> {
        self.token.validate()?;
        Ok(self.registry.permission_preview(call, cancellation))
    }
}

/// Native child loop 仍消费既有 `ToolExecutorRegistry`，因此每个导出 tool 都用同一个
/// context token 与 lifecycle cancellation 包装；definitions、preview 与 execution 不会
/// 绕过 scoped authority。
struct AgentScopedTool {
    tool: Arc<dyn Tool>,
    token: AgentContextToken,
    cancellation: CancellationToken,
}

impl Tool for AgentScopedTool {
    fn definition(&self) -> ToolDefinition {
        self.tool.definition()
    }

    fn execute<'a>(
        &'a self,
        call: ToolCall,
        cancellation: &'a CancellationToken,
    ) -> ToolExecutionFuture<'a> {
        self.execute_with_context(call, ToolExecutionContext::new(cancellation))
    }

    fn execute_with_context<'a>(
        &'a self,
        call: ToolCall,
        context: ToolExecutionContext<'a>,
    ) -> ToolExecutionFuture<'a> {
        if self.token.validate().is_err() {
            return Box::pin(
                async move { ToolResult::error(call.call_id, SCOPED_TOOL_UNAVAILABLE) },
            );
        }

        let call_id = call.call_id.clone();
        let tool = Arc::clone(&self.tool);
        let token = self.token.clone();
        let scope_cancellation = self.cancellation.clone();
        let execution_cancellation = context.cancellation().child_token();
        Box::pin(async move {
            let context = context.with_cancellation(&execution_cancellation);
            let result = tokio::select! {
                biased;
                () = scope_cancellation.cancelled() => {
                    execution_cancellation.cancel();
                    return ToolResult::error(call_id, SCOPED_TOOL_UNAVAILABLE);
                }
                result = tool.execute_with_context(call, context) => result,
            };
            if token.validate().is_ok() {
                result
            } else {
                ToolResult::error(call_id, SCOPED_TOOL_UNAVAILABLE)
            }
        })
    }

    fn permission_preview(
        &self,
        call: &ToolCall,
        cancellation: &CancellationToken,
    ) -> Option<ToolPermissionPreview> {
        if self.token.validate().is_err() || self.cancellation.is_cancelled() {
            return None;
        }
        self.tool.permission_preview(call, cancellation)
    }
}

impl ToolExecutor for AgentScopedToolView {
    fn execute_tool_with_context<'a>(
        &'a self,
        call: ToolCall,
        context: ToolExecutionContext<'a>,
    ) -> ToolExecutionFuture<'a> {
        if self.token.validate().is_err() {
            return Box::pin(
                async move { ToolResult::error(call.call_id, SCOPED_TOOL_UNAVAILABLE) },
            );
        }

        let call_id = call.call_id.clone();
        let registry = self.registry.clone();
        let token = self.token.clone();
        let scope_cancellation = self.cancellation.clone();
        let execution_cancellation = context.cancellation().child_token();
        Box::pin(async move {
            let context = context.with_cancellation(&execution_cancellation);
            let result = tokio::select! {
                biased;
                () = scope_cancellation.cancelled() => {
                    execution_cancellation.cancel();
                    return ToolResult::error(call_id, SCOPED_TOOL_UNAVAILABLE);
                }
                result = registry.execute_tool_with_context(call, context) => result,
            };
            if token.validate().is_ok() {
                result
            } else {
                ToolResult::error(call_id, SCOPED_TOOL_UNAVAILABLE)
            }
        })
    }
}

impl fmt::Debug for AgentScopedToolView {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AgentScopedToolView")
            .field("context_epoch", &self.context_epoch)
            .field("capability_generation", &self.capability_generation)
            .finish_non_exhaustive()
    }
}

/// Prompt view 只在 token current 时 clone immutable session snapshot。
#[derive(Clone)]
pub(super) struct AgentScopedPromptView {
    snapshot: PromptAssemblySessionSnapshot,
    token: AgentContextToken,
    context_epoch: u64,
    capability_generation: u64,
}

impl AgentScopedPromptView {
    pub(super) fn session_snapshot(
        &self,
    ) -> Result<PromptAssemblySessionSnapshot, AgentCapabilityContextError> {
        self.token.validate()?;
        Ok(self.snapshot.clone())
    }
}

impl fmt::Debug for AgentScopedPromptView {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AgentScopedPromptView")
            .field("context_epoch", &self.context_epoch)
            .field("capability_generation", &self.capability_generation)
            .finish_non_exhaustive()
    }
}

fn resolve_root_capabilities(
    grants: AgentRootCapabilityGrants,
) -> (
    BTreeMap<AgentCapabilityKey, ScopedCapability>,
    Vec<CapabilityGenerationGuard>,
) {
    let mut capabilities = BTreeMap::new();
    let mut guards = Vec::with_capacity(grants.capability_count());
    if let Some(grant) = grants.tools {
        let generation = grant.guard.generation();
        guards.push(grant.guard);
        capabilities.insert(
            AgentCapabilityKey::Tools,
            ScopedCapability::Tools(ScopedToolCapability {
                registry: grant.registry,
                generation,
            }),
        );
    }
    if let Some(grant) = grants.prompt {
        let generation = grant.guard.generation();
        guards.push(grant.guard);
        capabilities.insert(
            AgentCapabilityKey::Prompt,
            ScopedCapability::Prompt(Box::new(ScopedPromptCapability {
                snapshot: grant.snapshot,
                generation,
            })),
        );
    }
    (capabilities, guards)
}

fn resolve_child_capabilities(
    parent: &BTreeMap<AgentCapabilityKey, ScopedCapability>,
    grants: AgentChildCapabilityGrants,
    child_epoch: u64,
) -> Result<BTreeMap<AgentCapabilityKey, ScopedCapability>, AgentCapabilityContextError> {
    let mut capabilities = BTreeMap::new();
    match grants.tools {
        ToolGrantChoice::Omit => {}
        ToolGrantChoice::Inherit => {
            let tools = parent_tools(parent)?.clone();
            capabilities.insert(AgentCapabilityKey::Tools, ScopedCapability::Tools(tools));
        }
        ToolGrantChoice::InheritFiltered(allowed_tool_names) => {
            let tools = parent_tools(parent)?;
            capabilities.insert(
                AgentCapabilityKey::Tools,
                ScopedCapability::Tools(ScopedToolCapability {
                    registry: tools
                        .registry
                        .filtered(|tool_name| allowed_tool_names.contains(tool_name)),
                    generation: child_epoch,
                }),
            );
        }
        ToolGrantChoice::Shadow(registry) => {
            parent_tools(parent)?;
            capabilities.insert(
                AgentCapabilityKey::Tools,
                ScopedCapability::Tools(ScopedToolCapability {
                    registry,
                    generation: child_epoch,
                }),
            );
        }
    }

    match grants.prompt {
        PromptGrantChoice::Omit => {}
        PromptGrantChoice::Inherit => {
            let prompt = parent_prompt(parent)?.clone();
            capabilities.insert(
                AgentCapabilityKey::Prompt,
                ScopedCapability::Prompt(Box::new(prompt)),
            );
        }
        PromptGrantChoice::Shadow(snapshot) => {
            parent_prompt(parent)?;
            capabilities.insert(
                AgentCapabilityKey::Prompt,
                ScopedCapability::Prompt(Box::new(ScopedPromptCapability {
                    snapshot: *snapshot,
                    generation: child_epoch,
                })),
            );
        }
    }
    Ok(capabilities)
}

fn parent_tools(
    parent: &BTreeMap<AgentCapabilityKey, ScopedCapability>,
) -> Result<&ScopedToolCapability, AgentCapabilityContextError> {
    match parent.get(&AgentCapabilityKey::Tools) {
        Some(ScopedCapability::Tools(tools)) => Ok(tools),
        _ => Err(AgentCapabilityContextError::MissingCapability {
            capability: AgentCapabilityKey::Tools,
        }),
    }
}

fn parent_prompt(
    parent: &BTreeMap<AgentCapabilityKey, ScopedCapability>,
) -> Result<&ScopedPromptCapability, AgentCapabilityContextError> {
    match parent.get(&AgentCapabilityKey::Prompt) {
        Some(ScopedCapability::Prompt(prompt)) => Ok(prompt.as_ref()),
        _ => Err(AgentCapabilityContextError::MissingCapability {
            capability: AgentCapabilityKey::Prompt,
        }),
    }
}

fn context_is_current(state: &Arc<AgentCapabilityContextState>, expected_epoch: u64) -> bool {
    if state.epoch != expected_epoch {
        return false;
    }
    let mut current = Some(Arc::clone(state));
    while let Some(context) = current {
        if !context.scope.is_active() {
            return false;
        }
        current = match &context.parent {
            Some(parent) => match parent.upgrade() {
                Some(parent) => Some(parent),
                None => return false,
            },
            None => None,
        };
    }
    state.tree.root_generations_are_current()
}

fn map_scope_creation_error(error: EffectScopeError) -> AgentCapabilityContextError {
    match error {
        EffectScopeError::DuplicateChild { .. } => AgentCapabilityContextError::OwnerUnavailable,
        EffectScopeError::EntryIdExhausted => AgentCapabilityContextError::ScopeIdentityExhausted,
        EffectScopeError::Disposed
        | EffectScopeError::CleanupPending
        | EffectScopeError::DuplicateEffect { .. }
        | EffectScopeError::ActivationFailed => AgentCapabilityContextError::Unavailable,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, AtomicUsize};

    use runtime_domain::prompt_assembly::{
        PromptPreludeSection, PromptPreludeSnapshot, PromptSourceKind, PromptSourceOrigin,
        persistence::PromptAssemblyScope,
    };
    use serde_json::json;
    use tool_runtime::{
        Tool, ToolExecutionFuture, ToolExecutorRegistry, ToolPermissionPreview, ToolRegistration,
    };

    use super::*;
    use crate::runtime::{
        context::{ContextPublicationError, RuntimeContext, RuntimeContextError, StagedCapability},
        lifecycle::CapabilitySnapshot,
        prompt_assembly::{PromptAssembly, PromptRegistration, PromptSectionContribution},
    };

    fn owner(value: &'static str) -> AgentContextOwner {
        AgentContextOwner::try_new(value).expect("test owner should be a stable control identity")
    }

    struct StubTool {
        name: &'static str,
        output: &'static str,
        schema_marker: &'static str,
    }

    impl Tool for StubTool {
        fn definition(&self) -> ToolDefinition {
            ToolDefinition::new(self.name).with_input_schema(json!({
                "type": "object",
                "properties": {
                    "payload": {
                        "type": "string",
                        "description": self.schema_marker
                    }
                }
            }))
        }

        fn execute<'a>(
            &'a self,
            call: ToolCall,
            _cancellation: &'a CancellationToken,
        ) -> ToolExecutionFuture<'a> {
            Box::pin(async move { ToolResult::success(call.call_id, self.output) })
        }

        fn permission_preview(
            &self,
            _call: &ToolCall,
            _cancellation: &CancellationToken,
        ) -> Option<ToolPermissionPreview> {
            Some(ToolPermissionPreview {
                path: format!("/preview/{}", self.name),
                old_text: None,
                new_text: self.output.to_string(),
                is_truncated: false,
                snapshot: None,
            })
        }
    }

    struct CapabilityProvider {
        context: RuntimeContext,
        catalog: tool_runtime::ToolCatalog,
        prompt: PromptAssembly,
        publication_scopes: Vec<EffectScope>,
        generation: u64,
        _tool_registration: ToolRegistration,
        _prompt_manager_registration: PromptRegistration,
        _prompt_contribution_registration: PromptRegistration,
    }

    impl CapabilityProvider {
        fn new(prompt_body: &str) -> Self {
            let mut registry = ToolExecutorRegistry::new();
            registry.insert(StubTool {
                name: "read",
                output: "read-result",
                schema_marker: "read-schema",
            });
            registry.insert(StubTool {
                name: "bash",
                output: "bash-result",
                schema_marker: "bash-schema",
            });
            let (catalog, tool_registration) =
                tool_runtime::ToolCatalog::adopt_registry("test-tools", registry)
                    .expect("test tools should be unique");
            let (prompt, prompt_manager_registration) =
                PromptAssembly::adopt_manager("test-prompt", None)
                    .expect("test prompt manager should mount");
            let prompt_contribution_registration = prompt
                .contribute(
                    "test-prompt",
                    PromptSectionContribution {
                        stable_id: "test-prompt".to_string(),
                        scope: PromptAssemblyScope::Project,
                        priority: 0,
                        is_trusted: false,
                        estimated_tokens: Some(1),
                        section: PromptPreludeSection {
                            reference_id: "test-prompt".to_string(),
                            kind: PromptSourceKind::ExtraPrompt,
                            title: "Test prompt".to_string(),
                            origin: Some(PromptSourceOrigin::Project),
                            body: prompt_body.to_string(),
                        },
                    },
                )
                .expect("test prompt contribution should mount");
            let context = RuntimeContext::default();
            let scope = EffectScope::default();
            publish_capabilities(&context, &scope, &catalog, &prompt, 3);
            Self {
                context,
                catalog,
                prompt,
                publication_scopes: vec![scope],
                generation: 3,
                _tool_registration: tool_registration,
                _prompt_manager_registration: prompt_manager_registration,
                _prompt_contribution_registration: prompt_contribution_registration,
            }
        }

        fn root_grants(
            &self,
            allowed_tools: impl IntoIterator<Item = &'static str>,
        ) -> AgentRootCapabilityGrants {
            let tool_lease = self
                .context
                .require::<ToolCatalogCapability>()
                .expect("tool capability should resolve");
            let prompt_lease = self
                .context
                .require::<PromptAssemblyCapability>()
                .expect("prompt capability should resolve");
            AgentRootCapabilityGrants::empty()
                .with_tools(&tool_lease, allowed_tools.into_iter().map(str::to_string))
                .with_prompt(&prompt_lease)
        }

        fn replace(&mut self) {
            self.try_replace()
                .expect("current generation should cleanly replace");
        }

        fn try_replace(&mut self) -> Result<(), ContextPublicationError> {
            let snapshots = capability_snapshots(self.generation);
            self.context.hide_batch(&snapshots)?;
            self.generation += 1;
            let scope = EffectScope::default();
            publish_capabilities(
                &self.context,
                &scope,
                &self.catalog,
                &self.prompt,
                self.generation,
            );
            self.publication_scopes.push(scope);
            Ok(())
        }
    }

    fn publish_capabilities(
        context: &RuntimeContext,
        scope: &EffectScope,
        catalog: &tool_runtime::ToolCatalog,
        prompt: &PromptAssembly,
        generation: u64,
    ) {
        let mut activation = context.activation(
            scope,
            "test_provider",
            [
                super::ToolCatalogCapability::KEY.into(),
                super::PromptAssemblyCapability::KEY.into(),
            ],
        );
        activation
            .publish::<ToolCatalogCapability>(catalog.clone())
            .expect("tool catalog should stage");
        activation
            .publish::<PromptAssemblyCapability>(prompt.clone())
            .expect("prompt assembly should stage");
        let staged: Vec<StagedCapability> = activation
            .finish(true)
            .expect("test publication should finish");
        context
            .commit(&staged, &capability_snapshots(generation))
            .expect("test capabilities should commit");
    }

    fn capability_snapshots(generation: u64) -> Vec<CapabilitySnapshot> {
        [ToolCatalogCapability::KEY, PromptAssemblyCapability::KEY]
            .into_iter()
            .map(|key| CapabilitySnapshot {
                key: key.to_string(),
                provider_component: "test_provider".to_string(),
                generation,
            })
            .collect()
    }

    fn tool_names(view: &AgentScopedToolView) -> Vec<String> {
        view.definitions()
            .expect("tool view should be current")
            .into_iter()
            .map(|definition| definition.name)
            .collect()
    }

    fn prompt_body(view: &AgentScopedPromptView) -> String {
        view.session_snapshot()
            .expect("prompt view should be current")
            .prompt_prelude
            .expect("prompt prelude should exist")
            .effective_system_prompt()
            .expect("prompt body should exist")
    }

    fn shadow_prompt(body: &str) -> PromptAssemblySessionSnapshot {
        PromptAssemblySessionSnapshot {
            manager: None,
            prompt_prelude: Some(PromptPreludeSnapshot {
                sections: vec![PromptPreludeSection {
                    reference_id: "shadow".to_string(),
                    kind: PromptSourceKind::ExtraPrompt,
                    title: "Shadow".to_string(),
                    origin: Some(PromptSourceOrigin::Builtin),
                    body: body.to_string(),
                }],
            }),
            dynamic_environment_session_config: None,
        }
    }

    fn shadow_tools(name: &'static str, output: &'static str) -> ToolExecutorRegistry {
        let mut registry = ToolExecutorRegistry::new();
        registry.insert(StubTool {
            name,
            output,
            schema_marker: "shadow-schema",
        });
        registry
    }

    struct BlockingTool {
        started: Arc<tokio::sync::Notify>,
        release: Arc<tokio::sync::Notify>,
        completed: Arc<AtomicBool>,
        execution_cancellation: Arc<Mutex<Option<CancellationToken>>>,
    }

    impl Tool for BlockingTool {
        fn definition(&self) -> ToolDefinition {
            ToolDefinition::new("blocking")
        }

        fn execute<'a>(
            &'a self,
            call: ToolCall,
            cancellation: &'a CancellationToken,
        ) -> ToolExecutionFuture<'a> {
            let started = Arc::clone(&self.started);
            let release = Arc::clone(&self.release);
            let completed = Arc::clone(&self.completed);
            *self
                .execution_cancellation
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(cancellation.clone());
            Box::pin(async move {
                started.notify_one();
                release.notified().await;
                completed.store(true, Ordering::SeqCst);
                ToolResult::success(call.call_id, "blocking-result")
            })
        }
    }

    #[test]
    fn root_child_nested_and_sibling_visibility_is_explicit_and_isolated() {
        let provider = CapabilityProvider::new("root-prompt");
        let host_scope = EffectScope::default();
        let root = AgentCapabilityContext::root(
            owner("root-agent"),
            &host_scope,
            provider.root_grants(["read", "bash"]),
        )
        .expect("root context should mount");
        let filtered = root
            .child(
                owner("filtered-child"),
                AgentChildCapabilityGrants::empty()
                    .inherit_filtered_tools(["read".to_string()])
                    .inherit_prompt(),
            )
            .expect("filtered child should mount");
        let shadowed = root
            .child(
                owner("shadowed-child"),
                AgentChildCapabilityGrants::empty()
                    .shadow_tools(shadow_tools("review", "review-result"))
                    .shadow_prompt(shadow_prompt("shadow-prompt")),
            )
            .expect("shadowed child should mount");
        let sibling = root
            .child(
                owner("sibling"),
                AgentChildCapabilityGrants::empty()
                    .inherit_tools()
                    .inherit_prompt(),
            )
            .expect("sibling should mount");
        let nested = shadowed
            .child(
                owner("nested"),
                AgentChildCapabilityGrants::empty()
                    .inherit_tools()
                    .inherit_prompt(),
            )
            .expect("nested child should mount");
        let omitted = root
            .child(owner("omitted"), AgentChildCapabilityGrants::empty())
            .expect("omitted child should mount");

        assert_eq!(
            tool_names(&root.tools().expect("root tools")),
            ["bash", "read"]
        );
        assert_eq!(
            tool_names(&filtered.tools().expect("filtered tools")),
            ["read"]
        );
        assert_eq!(
            tool_names(&shadowed.tools().expect("shadow tools")),
            ["review"]
        );
        assert_eq!(
            tool_names(&nested.tools().expect("nested tools")),
            ["review"]
        );
        assert_eq!(
            tool_names(&sibling.tools().expect("sibling tools")),
            ["bash", "read"]
        );
        assert_eq!(
            prompt_body(&root.prompt().expect("root prompt")),
            "root-prompt"
        );
        assert_eq!(
            prompt_body(&filtered.prompt().expect("filtered prompt")),
            "root-prompt"
        );
        assert_eq!(
            prompt_body(&shadowed.prompt().expect("shadow prompt")),
            "shadow-prompt"
        );
        assert_eq!(
            prompt_body(&nested.prompt().expect("nested prompt")),
            "shadow-prompt"
        );
        assert_eq!(
            prompt_body(&sibling.prompt().expect("sibling prompt")),
            "root-prompt"
        );
        assert!(matches!(
            omitted.tools(),
            Err(AgentCapabilityContextError::MissingCapability {
                capability: AgentCapabilityKey::Tools
            })
        ));
        assert!(matches!(
            omitted.prompt(),
            Err(AgentCapabilityContextError::MissingCapability {
                capability: AgentCapabilityKey::Prompt
            })
        ));

        let root_snapshot = root.inspection_snapshot();
        let filtered_snapshot = filtered.inspection_snapshot();
        assert_eq!(root_snapshot.capability_count, 2);
        assert_eq!(root_snapshot.child_count, 4);
        assert_eq!(
            filtered_snapshot.capabilities[0].generation,
            filtered_snapshot.epoch
        );
        assert_ne!(filtered_snapshot.epoch, root_snapshot.epoch);
    }

    #[tokio::test]
    async fn scoped_tool_view_uses_one_filtered_registry_for_all_operations() {
        let provider = CapabilityProvider::new("root-prompt");
        let host_scope = EffectScope::default();
        let root = AgentCapabilityContext::root(
            owner("root-agent"),
            &host_scope,
            provider.root_grants(["read"]),
        )
        .expect("root context should mount");
        let tools = root.tools().expect("root tools should resolve");
        let cancellation = CancellationToken::new();

        assert_eq!(tool_names(&tools), ["read"]);
        let preview = tools
            .permission_preview(
                &ToolCall::new("preview", "read", json!({ "payload": "visible" })),
                &cancellation,
            )
            .expect("preview should validate")
            .expect("read should provide a preview");
        assert_eq!(preview.path, "/preview/read");
        let result = tools
            .execute_tool(
                ToolCall::new("execute", "read", json!({ "payload": "visible" })),
                &cancellation,
            )
            .await;
        assert_eq!(result.text_content(), "read-result");

        assert!(root.dispose().is_success());
        assert!(matches!(
            tools.definitions(),
            Err(AgentCapabilityContextError::Unavailable)
        ));
        assert!(matches!(
            tools.permission_preview(
                &ToolCall::new("stale-preview", "read", json!({})),
                &cancellation
            ),
            Err(AgentCapabilityContextError::Unavailable)
        ));
        let stale_result = tools
            .execute_tool(
                ToolCall::new("stale-execute", "read", json!({ "payload": "private" })),
                &cancellation,
            )
            .await;
        assert!(stale_result.is_error());
        assert_eq!(stale_result.text_content(), SCOPED_TOOL_UNAVAILABLE);
    }

    #[tokio::test]
    async fn context_disposal_cancels_an_in_flight_scoped_tool() {
        let provider = CapabilityProvider::new("root-prompt");
        let host_scope = EffectScope::default();
        let root = AgentCapabilityContext::root(
            owner("root-agent"),
            &host_scope,
            provider.root_grants(["read"]),
        )
        .expect("root context should mount");
        let started = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let completed = Arc::new(AtomicBool::new(false));
        let execution_cancellation = Arc::new(Mutex::new(None));
        let mut registry = ToolExecutorRegistry::new();
        registry.insert(BlockingTool {
            started: Arc::clone(&started),
            release: Arc::clone(&release),
            completed: Arc::clone(&completed),
            execution_cancellation: Arc::clone(&execution_cancellation),
        });
        let child = root
            .child(
                owner("blocking-child"),
                AgentChildCapabilityGrants::empty().shadow_tools(registry),
            )
            .expect("blocking child should mount");
        let tools = child
            .tools()
            .expect("blocking tools should resolve")
            .construction_registry()
            .expect("Native child registry should retain scoped authority");
        let execution = tokio::spawn(async move {
            let cancellation = CancellationToken::new();
            tools
                .execute_tool(
                    ToolCall::new("blocking-call", "blocking", json!({})),
                    &cancellation,
                )
                .await
        });
        started.notified().await;

        assert!(child.dispose().is_success());
        let result = execution.await.expect("tool task should join");
        assert!(result.is_error());
        assert_eq!(result.text_content(), SCOPED_TOOL_UNAVAILABLE);
        assert!(!completed.load(Ordering::SeqCst));
        assert!(
            execution_cancellation
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .as_ref()
                .is_some_and(CancellationToken::is_cancelled),
            "scope teardown must cancel the token observed by the tool body"
        );
        release.notify_waiters();
    }

    #[test]
    fn owning_scope_disposal_revokes_context_authority_and_owned_effects() {
        let provider = CapabilityProvider::new("root-prompt");
        let host_scope = EffectScope::default();
        let root = AgentCapabilityContext::root(
            owner("root-agent"),
            &host_scope,
            provider.root_grants(["read"]),
        )
        .expect("root context should mount");
        let child = root
            .child(
                owner("child"),
                AgentChildCapabilityGrants::empty()
                    .inherit_tools()
                    .inherit_prompt(),
            )
            .expect("child should mount");
        let token = child.token();
        let tools = child.tools().expect("child tools should resolve");
        let prompt = child.prompt().expect("child prompt should resolve");
        let side_effect_is_active = Arc::new(AtomicBool::new(false));
        let cleanup_calls = Arc::new(AtomicUsize::new(0));
        let active_probe = Arc::clone(&side_effect_is_active);
        let cleanup_probe = Arc::clone(&cleanup_calls);
        child
            .register_effect(&token, AgentScopedEffectKind::Worker, move || {
                active_probe.store(true, Ordering::SeqCst);
                Ok::<_, ()>(move || {
                    active_probe.store(false, Ordering::SeqCst);
                    cleanup_probe.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                })
            })
            .expect("owned effect should activate");
        assert!(side_effect_is_active.load(Ordering::SeqCst));

        assert!(host_scope.dispose().is_success());

        assert!(!side_effect_is_active.load(Ordering::SeqCst));
        assert_eq!(cleanup_calls.load(Ordering::SeqCst), 1);
        assert!(matches!(
            token.validate(),
            Err(AgentCapabilityContextError::Unavailable)
        ));
        assert!(matches!(
            tools.definitions(),
            Err(AgentCapabilityContextError::Unavailable)
        ));
        assert!(matches!(
            prompt.session_snapshot(),
            Err(AgentCapabilityContextError::Unavailable)
        ));
        assert!(root.dispose().is_success());
        assert_eq!(cleanup_calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn context_owner_drop_revokes_descendants_and_owned_effects() {
        let provider = CapabilityProvider::new("root-prompt");
        let host_scope = EffectScope::default();
        let root = AgentCapabilityContext::root(
            owner("root-agent"),
            &host_scope,
            provider.root_grants(["read"]),
        )
        .expect("root context should mount");
        let child = root
            .child(
                owner("child"),
                AgentChildCapabilityGrants::empty().inherit_tools(),
            )
            .expect("child should mount");
        let tools = child.tools().expect("child tools should resolve");
        let cleanup_calls = Arc::new(AtomicUsize::new(0));
        let cleanup_probe = Arc::clone(&cleanup_calls);
        child
            .register_effect(&child.token(), AgentScopedEffectKind::Listener, || {
                Ok::<_, ()>(move || {
                    cleanup_probe.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                })
            })
            .expect("child effect should activate");

        drop(root);

        assert!(matches!(
            tools.definitions(),
            Err(AgentCapabilityContextError::Unavailable)
        ));
        assert_eq!(cleanup_calls.load(Ordering::SeqCst), 1);
        assert!(host_scope.dispose().is_success());
        assert_eq!(cleanup_calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn runtime_context_teardown_revokes_scoped_context_without_reentrant_deadlock() {
        let mut provider = CapabilityProvider::new("root-prompt");
        let host_scope = EffectScope::default();
        let root = AgentCapabilityContext::root(
            owner("root-agent"),
            &host_scope,
            provider.root_grants(["read"]),
        )
        .expect("root context should mount");
        let cleanup_calls = Arc::new(AtomicUsize::new(0));
        let cleanup_probe = Arc::clone(&cleanup_calls);
        root.register_effect(&root.token(), AgentScopedEffectKind::Listener, || {
            Ok::<_, ()>(move || {
                cleanup_probe.fetch_add(1, Ordering::SeqCst);
                Ok(())
            })
        })
        .expect("owned effect should activate");

        let runtime_context = std::mem::take(&mut provider.context);
        drop(runtime_context);

        assert!(matches!(
            root.tools(),
            Err(AgentCapabilityContextError::Unavailable)
        ));
        assert_eq!(cleanup_calls.load(Ordering::SeqCst), 1);
        assert!(root.dispose().is_success());
        assert!(host_scope.dispose().is_success());
    }

    #[test]
    fn effect_activation_rejected_by_concurrent_disposal_runs_its_inverse() {
        let provider = CapabilityProvider::new("root-prompt");
        let host_scope = EffectScope::default();
        let root = AgentCapabilityContext::root(
            owner("root-agent"),
            &host_scope,
            provider.root_grants(["read"]),
        )
        .expect("root context should mount");
        let side_effect_is_active = Arc::new(AtomicBool::new(false));
        let cleanup_calls = Arc::new(AtomicUsize::new(0));
        let active_probe = Arc::clone(&side_effect_is_active);
        let cleanup_probe = Arc::clone(&cleanup_calls);

        let registration =
            root.register_effect(&root.token(), AgentScopedEffectKind::Listener, || {
                active_probe.store(true, Ordering::SeqCst);
                let report = host_scope.dispose();
                assert!(!report.is_success());
                Ok::<_, ()>(move || {
                    active_probe.store(false, Ordering::SeqCst);
                    cleanup_probe.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                })
            });

        assert_eq!(
            registration,
            Err(AgentCapabilityContextError::EffectRegistrationRejected)
        );
        assert!(!side_effect_is_active.load(Ordering::SeqCst));
        assert_eq!(cleanup_calls.load(Ordering::SeqCst), 1);
        assert!(host_scope.dispose().is_success());
        assert_eq!(cleanup_calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn fallible_effect_activation_returns_a_closed_error_without_mutating_the_scope() {
        const ACTIVATION_SENTINEL: &str = "PRIVATE_EFFECT_ACTIVATION_FAILURE";
        let provider = CapabilityProvider::new("root-prompt");
        let host_scope = EffectScope::default();
        let root = AgentCapabilityContext::root(
            owner("root-agent"),
            &host_scope,
            provider.root_grants(["read"]),
        )
        .expect("root context should mount");

        let error = root
            .register_effect(&root.token(), AgentScopedEffectKind::Worker, || {
                Err::<fn() -> Result<(), String>, _>(ACTIVATION_SENTINEL)
            })
            .expect_err("fallible activation should reject publication");

        assert_eq!(
            error,
            AgentCapabilityContextError::EffectRegistrationRejected
        );
        assert!(!format!("{error:?}").contains(ACTIVATION_SENTINEL));
        assert!(root.tools().is_ok());
        assert!(root.dispose().is_success());
    }

    #[test]
    fn context_owner_rejects_delivery_shaped_identity_without_echoing_it() {
        const OWNER_SENTINEL: &str = "PRIVATE INSTRUCTION OWNER";
        let error = AgentContextOwner::try_new(OWNER_SENTINEL)
            .expect_err("delivery text must not become a control owner");

        assert_eq!(error, AgentContextOwnerError::InvalidFormat);
        assert!(!format!("{error:?}").contains(OWNER_SENTINEL));
        assert!(!error.to_string().contains(OWNER_SENTINEL));
        assert_eq!(owner("reviewer_2").as_str(), "reviewer_2");
    }

    #[test]
    fn parent_and_child_disposal_preserve_siblings_until_their_owner_closes() {
        let provider = CapabilityProvider::new("root-prompt");
        let host_scope = EffectScope::default();
        let root = AgentCapabilityContext::root(
            owner("root-agent"),
            &host_scope,
            provider.root_grants(["read"]),
        )
        .expect("root context should mount");
        let child = root
            .child(
                owner("child"),
                AgentChildCapabilityGrants::empty().inherit_tools(),
            )
            .expect("child should mount");
        let sibling = root
            .child(
                owner("sibling"),
                AgentChildCapabilityGrants::empty().inherit_tools(),
            )
            .expect("sibling should mount");
        let child_cleanup = Arc::new(AtomicUsize::new(0));
        let sibling_cleanup = Arc::new(AtomicUsize::new(0));
        let child_cleanup_probe = Arc::clone(&child_cleanup);
        child
            .register_effect(&child.token(), AgentScopedEffectKind::Listener, move || {
                Ok::<_, ()>(move || {
                    child_cleanup_probe.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                })
            })
            .expect("child effect should register");
        let sibling_cleanup_probe = Arc::clone(&sibling_cleanup);
        sibling
            .register_effect(&sibling.token(), AgentScopedEffectKind::Worker, move || {
                Ok::<_, ()>(move || {
                    sibling_cleanup_probe.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                })
            })
            .expect("sibling effect should register");

        assert!(child.dispose().is_success());
        assert_eq!(child_cleanup.load(Ordering::SeqCst), 1);
        assert_eq!(sibling_cleanup.load(Ordering::SeqCst), 0);
        assert!(sibling.tools().is_ok());
        assert!(root.tools().is_ok());

        assert!(root.dispose().is_success());
        assert_eq!(sibling_cleanup.load(Ordering::SeqCst), 1);
        assert!(matches!(
            sibling.tools(),
            Err(AgentCapabilityContextError::Unavailable)
        ));
        assert!(root.dispose().is_success());
        assert_eq!(sibling_cleanup.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn failed_cleanup_retains_owner_and_only_retries_pending_inverse() {
        let provider = CapabilityProvider::new("root-prompt");
        let host_scope = EffectScope::default();
        let root = AgentCapabilityContext::root(
            owner("root-agent"),
            &host_scope,
            provider.root_grants(["read"]),
        )
        .expect("root context should mount");
        let successful_calls = Arc::new(AtomicUsize::new(0));
        let retry_calls = Arc::new(AtomicUsize::new(0));
        let successful_probe = Arc::clone(&successful_calls);
        root.register_effect(
            &root.token(),
            AgentScopedEffectKind::ToolRegistration,
            move || {
                Ok::<_, ()>(move || {
                    successful_probe.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                })
            },
        )
        .expect("successful effect should register");
        let retry_probe = Arc::clone(&retry_calls);
        root.register_effect(
            &root.token(),
            AgentScopedEffectKind::PromptContribution,
            move || {
                Ok::<_, ()>(move || {
                    let call = retry_probe.fetch_add(1, Ordering::SeqCst);
                    if call == 0 {
                        Err("PRIVATE_DISPOSER_FAILURE".to_string())
                    } else {
                        Ok(())
                    }
                })
            },
        )
        .expect("retryable effect should register");

        let first_report = root.dispose();
        assert_eq!(first_report.pending_cleanup_count, 1);
        assert_eq!(retry_calls.load(Ordering::SeqCst), 1);
        assert_eq!(successful_calls.load(Ordering::SeqCst), 1);
        assert!(matches!(
            root.tools(),
            Err(AgentCapabilityContextError::Unavailable)
        ));
        assert!(matches!(
            AgentCapabilityContext::root(
                owner("root-agent"),
                &host_scope,
                AgentRootCapabilityGrants::empty()
            ),
            Err(AgentCapabilityContextError::OwnerUnavailable)
        ));
        assert!(!format!("{first_report:?}").contains("PRIVATE_DISPOSER_FAILURE"));

        assert!(root.dispose().is_success());
        assert_eq!(retry_calls.load(Ordering::SeqCst), 2);
        assert_eq!(successful_calls.load(Ordering::SeqCst), 1);
        let replacement = AgentCapabilityContext::root(
            owner("root-agent"),
            &host_scope,
            AgentRootCapabilityGrants::empty(),
        )
        .expect("successful cleanup should release owner");
        assert!(replacement.dispose().is_success());
    }

    #[test]
    fn generation_replacement_waits_for_scoped_cleanup_retry_before_fresh_publication() {
        let mut provider = CapabilityProvider::new("root-prompt");
        let host_scope = EffectScope::default();
        let root = AgentCapabilityContext::root(
            owner("root-agent"),
            &host_scope,
            provider.root_grants(["read"]),
        )
        .expect("root context should mount");
        let cleanup_attempts = Arc::new(AtomicUsize::new(0));
        let cleanup_may_succeed = Arc::new(AtomicBool::new(false));
        let cleanup_probe = Arc::clone(&cleanup_attempts);
        let cleanup_gate = Arc::clone(&cleanup_may_succeed);
        root.register_effect(&root.token(), AgentScopedEffectKind::Worker, move || {
            Ok::<_, ()>(move || {
                cleanup_probe.fetch_add(1, Ordering::SeqCst);
                if !cleanup_gate.load(Ordering::SeqCst) {
                    Err("PRIVATE_TRANSIENT_CLEANUP".to_string())
                } else {
                    Ok(())
                }
            })
        })
        .expect("retryable effect should activate");

        let failure = provider
            .try_replace()
            .expect_err("fresh generation must wait for old cleanup");
        assert!(matches!(
            failure,
            ContextPublicationError::DependencyCleanupPending { .. }
        ));
        assert_eq!(cleanup_attempts.load(Ordering::SeqCst), 2);
        assert!(matches!(
            provider.context.require::<ToolCatalogCapability>(),
            Err(RuntimeContextError::MissingCapability { .. })
        ));

        cleanup_may_succeed.store(true, Ordering::SeqCst);
        provider
            .try_replace()
            .expect("cleanup retry should permit fresh publication");
        assert_eq!(cleanup_attempts.load(Ordering::SeqCst), 3);
        let fresh = AgentCapabilityContext::root(
            owner("root-agent"),
            &host_scope,
            provider.root_grants(["read"]),
        )
        .expect("fresh context should mount after old cleanup");
        assert_eq!(tool_names(&fresh.tools().expect("fresh tools")), ["read"]);
        assert!(fresh.dispose().is_success());
        assert!(root.dispose().is_success());
        assert_eq!(cleanup_attempts.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn disposal_failure_before_generation_replacement_keeps_fresh_authority_blocked() {
        let mut provider = CapabilityProvider::new("root-prompt");
        let host_scope = EffectScope::default();
        let root = AgentCapabilityContext::root(
            owner("root-agent"),
            &host_scope,
            provider.root_grants(["read"]),
        )
        .expect("root context should mount");
        let cleanup_attempts = Arc::new(AtomicUsize::new(0));
        let cleanup_may_succeed = Arc::new(AtomicBool::new(false));
        let cleanup_probe = Arc::clone(&cleanup_attempts);
        let cleanup_gate = Arc::clone(&cleanup_may_succeed);
        root.register_effect(&root.token(), AgentScopedEffectKind::Worker, move || {
            Ok::<_, ()>(move || {
                cleanup_probe.fetch_add(1, Ordering::SeqCst);
                if cleanup_gate.load(Ordering::SeqCst) {
                    Ok(())
                } else {
                    Err("PRIVATE_TRANSIENT_CLEANUP".to_string())
                }
            })
        })
        .expect("retryable effect should activate");

        assert_eq!(root.dispose().pending_cleanup_count, 1);
        assert_eq!(cleanup_attempts.load(Ordering::SeqCst), 1);
        let failure = provider
            .try_replace()
            .expect_err("replacement must retry and retain failed root cleanup");
        assert!(matches!(
            failure,
            ContextPublicationError::DependencyCleanupPending { .. }
        ));
        assert_eq!(cleanup_attempts.load(Ordering::SeqCst), 3);
        assert!(matches!(
            provider.context.require::<ToolCatalogCapability>(),
            Err(RuntimeContextError::MissingCapability { .. })
        ));

        cleanup_may_succeed.store(true, Ordering::SeqCst);
        provider
            .try_replace()
            .expect("successful cleanup retry should release fresh publication");
        assert_eq!(cleanup_attempts.load(Ordering::SeqCst), 4);
        let fresh = AgentCapabilityContext::root(
            owner("root-agent"),
            &host_scope,
            provider.root_grants(["read"]),
        )
        .expect("fresh context should mount only after old cleanup succeeds");
        assert!(fresh.dispose().is_success());
        assert!(root.dispose().is_success());
    }

    #[test]
    fn owner_drop_failure_transfers_generation_cleanup_barrier_to_the_scope() {
        let mut provider = CapabilityProvider::new("root-prompt");
        let host_scope = EffectScope::default();
        let root = AgentCapabilityContext::root(
            owner("root-agent"),
            &host_scope,
            provider.root_grants(["read"]),
        )
        .expect("root context should mount");
        let cleanup_attempts = Arc::new(AtomicUsize::new(0));
        let cleanup_may_succeed = Arc::new(AtomicBool::new(false));
        let cleanup_probe = Arc::clone(&cleanup_attempts);
        let cleanup_gate = Arc::clone(&cleanup_may_succeed);
        root.register_effect(&root.token(), AgentScopedEffectKind::Worker, move || {
            Ok::<_, ()>(move || {
                cleanup_probe.fetch_add(1, Ordering::SeqCst);
                if cleanup_gate.load(Ordering::SeqCst) {
                    Ok(())
                } else {
                    Err("PRIVATE_TRANSIENT_CLEANUP".to_string())
                }
            })
        })
        .expect("retryable effect should activate");

        drop(root);
        assert_eq!(cleanup_attempts.load(Ordering::SeqCst), 1);
        let failure = provider
            .try_replace()
            .expect_err("owner Drop failure must keep replacement blocked");
        assert!(matches!(
            failure,
            ContextPublicationError::DependencyCleanupPending { .. }
        ));
        assert_eq!(cleanup_attempts.load(Ordering::SeqCst), 3);

        cleanup_may_succeed.store(true, Ordering::SeqCst);
        provider
            .try_replace()
            .expect("scope-owned observer should converge old cleanup");
        assert_eq!(cleanup_attempts.load(Ordering::SeqCst), 4);
        let fresh = AgentCapabilityContext::root(
            owner("root-agent"),
            &host_scope,
            provider.root_grants(["read"]),
        )
        .expect("fresh context should mount after transferred cleanup succeeds");
        assert!(fresh.dispose().is_success());
    }

    #[tokio::test]
    async fn provider_generation_replacement_quiesces_the_old_tree_and_in_flight_tool() {
        let mut provider = CapabilityProvider::new("root-prompt");
        let host_scope = EffectScope::default();
        let old_root = AgentCapabilityContext::root(
            owner("old-root"),
            &host_scope,
            provider.root_grants(["read"]),
        )
        .expect("old root should mount");
        let old_child = old_root
            .child(
                owner("old-child"),
                AgentChildCapabilityGrants::empty()
                    .inherit_tools()
                    .inherit_prompt(),
            )
            .expect("old child should mount");
        let old_token = old_child.token();
        let old_tools = old_child.tools().expect("old tools should resolve");
        let old_prompt = old_child.prompt().expect("old prompt should resolve");
        let side_effect_is_active = Arc::new(AtomicBool::new(false));
        let cleanup_calls = Arc::new(AtomicUsize::new(0));
        let active_probe = Arc::clone(&side_effect_is_active);
        let cleanup_probe = Arc::clone(&cleanup_calls);
        old_child
            .register_effect(&old_token, AgentScopedEffectKind::Worker, move || {
                active_probe.store(true, Ordering::SeqCst);
                Ok::<_, ()>(move || {
                    active_probe.store(false, Ordering::SeqCst);
                    cleanup_probe.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                })
            })
            .expect("old effect should activate");
        let started = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let completed = Arc::new(AtomicBool::new(false));
        let execution_cancellation = Arc::new(Mutex::new(None));
        let mut blocking_registry = ToolExecutorRegistry::new();
        blocking_registry.insert(BlockingTool {
            started: Arc::clone(&started),
            release: Arc::clone(&release),
            completed: Arc::clone(&completed),
            execution_cancellation: Arc::clone(&execution_cancellation),
        });
        let blocking_child = old_root
            .child(
                owner("blocking-child"),
                AgentChildCapabilityGrants::empty().shadow_tools(blocking_registry),
            )
            .expect("blocking child should mount");
        let blocking_tools = blocking_child
            .tools()
            .expect("blocking tools should resolve")
            .construction_registry()
            .expect("Native child registry should retain scoped authority");
        let execution = tokio::spawn(async move {
            blocking_tools
                .execute_tool(
                    ToolCall::new("generation-blocking-call", "blocking", json!({})),
                    &CancellationToken::new(),
                )
                .await
        });
        started.notified().await;

        provider.replace();

        let result = execution.await.expect("tool task should join");
        assert!(result.is_error());
        assert_eq!(result.text_content(), SCOPED_TOOL_UNAVAILABLE);
        assert!(!completed.load(Ordering::SeqCst));
        assert!(
            execution_cancellation
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .as_ref()
                .is_some_and(CancellationToken::is_cancelled),
            "generation replacement must cancel the token observed by the old tool body"
        );
        assert!(!side_effect_is_active.load(Ordering::SeqCst));
        assert_eq!(cleanup_calls.load(Ordering::SeqCst), 1);
        release.notify_waiters();

        assert!(matches!(
            old_tools.definitions(),
            Err(AgentCapabilityContextError::Unavailable)
        ));
        assert!(matches!(
            old_prompt.session_snapshot(),
            Err(AgentCapabilityContextError::Unavailable)
        ));
        let rejected_activation_calls = Arc::new(AtomicUsize::new(0));
        let rejected_probe = Arc::clone(&rejected_activation_calls);
        assert!(matches!(
            old_child.register_effect(&old_token, AgentScopedEffectKind::Listener, move || {
                rejected_probe.fetch_add(1, Ordering::SeqCst);
                Ok::<_, ()>(|| Ok(()))
            }),
            Err(AgentCapabilityContextError::Unavailable)
        ));
        assert_eq!(rejected_activation_calls.load(Ordering::SeqCst), 0);

        let fresh_root = AgentCapabilityContext::root(
            owner("fresh-root"),
            &host_scope,
            provider.root_grants(["read"]),
        )
        .expect("fresh generation should mount");
        assert_eq!(
            tool_names(&fresh_root.tools().expect("fresh tools")),
            ["read"]
        );
        assert!(matches!(
            fresh_root.register_effect(&old_token, AgentScopedEffectKind::Listener, || {
                Ok::<_, ()>(|| Ok(()))
            }),
            Err(AgentCapabilityContextError::Unavailable)
        ));
        assert!(fresh_root.dispose().is_success());
        assert!(old_root.dispose().is_success());
        assert_eq!(cleanup_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn diagnostics_do_not_expose_delivery_prompt_tool_or_disposer_content() {
        const INSTRUCTION_SENTINEL: &str = "PRIVATE_INSTRUCTION_SENTINEL";
        const DELIVERY_SENTINEL: &str = "PRIVATE_DELIVERY_SENTINEL";
        const TOOL_SCHEMA_SENTINEL: &str = "PRIVATE_TOOL_SCHEMA_SENTINEL";
        const TOOL_PAYLOAD_SENTINEL: &str = "PRIVATE_TOOL_PAYLOAD_SENTINEL";
        const TOOL_RESULT_SENTINEL: &str = "PRIVATE_TOOL_RESULT_SENTINEL";
        const DISPOSER_SENTINEL: &str = "PRIVATE_DISPOSER_FAILURE_SENTINEL";
        let provider = CapabilityProvider::new(INSTRUCTION_SENTINEL);
        let grants = provider.root_grants(["read"]);
        assert!(!format!("{grants:?}").contains(INSTRUCTION_SENTINEL));
        let mut sensitive_tools = ToolExecutorRegistry::new();
        sensitive_tools.insert(StubTool {
            name: "private-tool",
            output: TOOL_RESULT_SENTINEL,
            schema_marker: TOOL_SCHEMA_SENTINEL,
        });
        let child_grants = AgentChildCapabilityGrants::empty()
            .shadow_tools(sensitive_tools)
            .shadow_prompt(shadow_prompt(&format!(
                "{INSTRUCTION_SENTINEL}\n{DELIVERY_SENTINEL}"
            )));
        let grants_debug = format!("{child_grants:?}");
        assert!(!grants_debug.contains(INSTRUCTION_SENTINEL));
        assert!(!grants_debug.contains(DELIVERY_SENTINEL));
        assert!(!grants_debug.contains(TOOL_SCHEMA_SENTINEL));
        assert!(!grants_debug.contains(TOOL_RESULT_SENTINEL));
        let host_scope = EffectScope::default();
        let root = AgentCapabilityContext::root(owner("root"), &host_scope, grants)
            .expect("root context should mount");
        let child = root
            .child(owner("child"), child_grants)
            .expect("child context should mount");
        let token = child.token();
        let tools = child.tools().expect("child tools should resolve");
        let prompt = child.prompt().expect("child prompt should resolve");
        child
            .register_effect(&token, AgentScopedEffectKind::Listener, || {
                Ok::<_, ()>(|| Err(DISPOSER_SENTINEL.to_string()))
            })
            .expect("sentinel disposer should register");

        let missing = AgentCapabilityContext::root(
            owner("empty"),
            &host_scope,
            AgentRootCapabilityGrants::empty(),
        )
        .expect("empty context should mount")
        .tools()
        .expect_err("empty context should not expose tools");
        let report = child.dispose();
        let cancellation = CancellationToken::new();
        let stale_result = tools
            .execute_tool(
                ToolCall::new(
                    "redacted-call",
                    "private-tool",
                    json!({ "payload": TOOL_PAYLOAD_SENTINEL }),
                ),
                &cancellation,
            )
            .await;
        for diagnostic in [
            format!("{root:?}"),
            format!("{token:?}"),
            format!("{tools:?}"),
            format!("{prompt:?}"),
            format!("{:?}", root.inspection_snapshot()),
            format!("{missing:?}"),
            missing.to_string(),
            format!("{report:?}"),
            format!("{stale_result:?}"),
        ] {
            assert!(!diagnostic.contains(INSTRUCTION_SENTINEL));
            assert!(!diagnostic.contains(DELIVERY_SENTINEL));
            assert!(!diagnostic.contains(TOOL_SCHEMA_SENTINEL));
            assert!(!diagnostic.contains(TOOL_PAYLOAD_SENTINEL));
            assert!(!diagnostic.contains(TOOL_RESULT_SENTINEL));
            assert!(!diagnostic.contains(DISPOSER_SENTINEL));
            assert!(!diagnostic.contains("private-tool"));
        }
        assert_eq!(report.pending_cleanup_count, 1);
    }

    #[test]
    fn shadow_requires_the_immediate_parent_capability() {
        let host_scope = EffectScope::default();
        let root = AgentCapabilityContext::root(
            owner("root"),
            &host_scope,
            AgentRootCapabilityGrants::empty(),
        )
        .expect("empty root should mount");
        let error = root
            .child(
                owner("child"),
                AgentChildCapabilityGrants::empty().shadow_tools(shadow_tools("read", "output")),
            )
            .expect_err("child must not introduce ambient tool authority");
        assert_eq!(
            error,
            AgentCapabilityContextError::MissingCapability {
                capability: AgentCapabilityKey::Tools
            }
        );
        assert_eq!(root.inspection_snapshot().child_count, 0);
    }
}
