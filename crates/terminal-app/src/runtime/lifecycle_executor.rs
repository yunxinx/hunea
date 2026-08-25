use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    error::Error,
    fmt,
    ops::Deref,
};

use super::{
    context::{
        CapabilityLease, ComponentActivationContext, RuntimeCapability, RuntimeCapabilitySnapshot,
        RuntimeContext, RuntimeContextError,
    },
    effect_scope::{EffectScope, EffectScopeLifecycleSnapshot, EffectScopeSnapshot},
    lifecycle::{
        ActivationToken, CapabilityKey, ComponentDefinition, ComponentFailureOperation,
        ComponentFailureReason, ComponentGraph, ComponentGraphError, DeactivationToken,
        PreparedDefinitionReconciliation, ReconciliationReport,
    },
};

#[cfg(test)]
use super::lifecycle::ComponentState;

/// `ComponentLifecycleMode` 只描述 concrete callback 当前所处的 host operation。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ComponentLifecycleMode {
    Initial,
    Reconfigure,
    WakeBinding,
    Shutdown,
}

/// Activation callback 显式决定本次成功是否发布 definition 中声明的 capabilities。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ComponentActivationOutcome {
    Ready,
    PublishCapabilities,
}

/// Concrete component 只实现资源 mount/quiesce；顺序、scope 与 graph ack 由 executor 拥有。
pub(super) trait ComponentLifecycleCallbacks {
    fn activate_component(
        &mut self,
        component_id: &str,
        scope: &EffectScope,
        context: &mut ComponentActivationContext<'_>,
        mode: ComponentLifecycleMode,
    ) -> Result<ComponentActivationOutcome, String>;

    fn quiesce_component(
        &mut self,
        component_id: &str,
        mode: ComponentLifecycleMode,
    ) -> Result<(), String>;

    /// 在 old cleanup 完成后准备尚未发布、可丢弃的 coordinator-owned authority。
    fn prepare_authority(&mut self) -> Result<(), String>;

    /// 在 graph definition commit 成功后发布已准备的 coordinator-owned authority。
    fn commit_authority(&mut self);

    /// 丢弃当前尚未发布的 authority preparation；重复调用必须安全。
    fn abort_authority(&mut self);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LifecycleExecutionOperation {
    Graph,
    AuthorityPreparation,
    Activation,
    Quiescence,
    EffectDisposal,
}

impl LifecycleExecutionOperation {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Graph => "graph",
            Self::AuthorityPreparation => "authority_preparation",
            Self::Activation => "activation",
            Self::Quiescence => "quiescence",
            Self::EffectDisposal => "effect_disposal",
        }
    }
}

struct LifecycleExecutionFailure {
    component_id: String,
    operation: LifecycleExecutionOperation,
}

/// Lifecycle error 只投影 component 与 operation，不保留 callback 或 resource error text。
pub(super) struct LifecycleExecutionError {
    failures: Vec<LifecycleExecutionFailure>,
}

impl LifecycleExecutionError {
    fn graph(component_id: impl Into<String>, _error: ComponentGraphError) -> Self {
        Self {
            failures: vec![LifecycleExecutionFailure {
                component_id: component_id.into(),
                operation: LifecycleExecutionOperation::Graph,
            }],
        }
    }

    fn finish(failures: Vec<LifecycleExecutionFailure>) -> Result<(), Self> {
        if failures.is_empty() {
            Ok(())
        } else {
            Err(Self { failures })
        }
    }

    fn executor_shutdown() -> Self {
        Self {
            failures: vec![LifecycleExecutionFailure {
                component_id: "runtime_composition".to_string(),
                operation: LifecycleExecutionOperation::Graph,
            }],
        }
    }

    fn authority_preparation() -> Self {
        Self {
            failures: vec![LifecycleExecutionFailure {
                component_id: "runtime_composition".to_string(),
                operation: LifecycleExecutionOperation::AuthorityPreparation,
            }],
        }
    }
}

impl fmt::Debug for LifecycleExecutionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let diagnostics = self
            .failures
            .iter()
            .map(|failure| (failure.component_id.as_str(), failure.operation.as_str()))
            .collect::<Vec<_>>();
        f.debug_struct("LifecycleExecutionError")
            .field("failures", &diagnostics)
            .finish()
    }
}

impl fmt::Display for LifecycleExecutionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, failure) in self.failures.iter().enumerate() {
            if index > 0 {
                f.write_str("; ")?;
            }
            write!(
                f,
                "component {} {} failed",
                failure.component_id,
                failure.operation.as_str()
            )?;
        }
        Ok(())
    }
}

impl Error for LifecycleExecutionError {}

struct LifecycleExecutionState {
    activation_rank: BTreeMap<String, usize>,
    deactivation_rank: BTreeMap<String, usize>,
    seen_activations: BTreeSet<ActivationToken>,
    seen_deactivations: BTreeSet<(String, u64)>,
    failures: Vec<LifecycleExecutionFailure>,
}

impl LifecycleExecutionState {
    fn new(graph: &ComponentGraph) -> Self {
        Self {
            activation_rank: rank(graph.activation_order()),
            deactivation_rank: rank(graph.deactivation_order()),
            seen_activations: BTreeSet::new(),
            seen_deactivations: BTreeSet::new(),
            failures: Vec::new(),
        }
    }
}

/// `ComponentLifecycleExecutor` 是 graph action 与 concrete reversible effects 的唯一桥梁。
pub(super) struct ComponentLifecycleExecutor {
    graph: ComponentGraph,
    context: RuntimeContext,
    root_scope: EffectScope,
    component_scopes: BTreeMap<String, EffectScope>,
    finalization: LifecycleFinalization,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum LifecycleFinalization {
    #[default]
    Open,
    Finalizing,
    Succeeded,
}

impl Default for ComponentLifecycleExecutor {
    fn default() -> Self {
        Self {
            graph: ComponentGraph::default(),
            context: RuntimeContext::default(),
            root_scope: EffectScope::default(),
            component_scopes: BTreeMap::new(),
            finalization: LifecycleFinalization::Open,
        }
    }
}

impl Deref for ComponentLifecycleExecutor {
    type Target = ComponentGraph;

    fn deref(&self) -> &Self::Target {
        &self.graph
    }
}

#[cfg(test)]
impl std::ops::DerefMut for ComponentLifecycleExecutor {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.graph
    }
}

impl ComponentLifecycleExecutor {
    #[cfg(test)]
    pub(super) fn graph(&self) -> &ComponentGraph {
        &self.graph
    }

    pub(super) fn scope_snapshots(&self) -> Vec<EffectScopeSnapshot> {
        self.root_scope
            .snapshot()
            .map(|snapshot| snapshot.children)
            .unwrap_or_default()
    }

    pub(super) fn require<C>(&self) -> Result<CapabilityLease<C>, RuntimeContextError>
    where
        C: RuntimeCapability + 'static,
    {
        self.context.require::<C>()
    }

    pub(super) fn optional<C>(&self) -> Result<Option<CapabilityLease<C>>, RuntimeContextError>
    where
        C: RuntimeCapability + 'static,
    {
        self.context.optional::<C>()
    }

    pub(super) fn context_snapshots(&self) -> Vec<RuntimeCapabilitySnapshot> {
        self.context.snapshots()
    }

    /// 把完整 desired definition set 作为一个 preflight/retire/commit/activate transaction 执行。
    #[allow(dead_code)]
    pub(super) fn reconcile_definitions(
        &mut self,
        definitions: impl IntoIterator<Item = ComponentDefinition>,
        callbacks: &mut impl ComponentLifecycleCallbacks,
        mode: ComponentLifecycleMode,
    ) -> Result<(), LifecycleExecutionError> {
        self.reconcile_definitions_inner(definitions, callbacks, mode, false)
    }

    /// 执行 definition transaction，并在 graph commit 与 fresh activation 之间切换外部 authority。
    ///
    /// callback host 的 `commit_authority` 只在所有 old cleanup 成功且 graph commit 成功后调用；
    /// 因此 fresh plugin instance 在此之前不会接收 lifecycle callback，cleanup 失败也不会留下
    /// 半切换状态。
    pub(super) fn reconcile_definitions_with_commit(
        &mut self,
        definitions: impl IntoIterator<Item = ComponentDefinition>,
        callbacks: &mut impl ComponentLifecycleCallbacks,
        mode: ComponentLifecycleMode,
    ) -> Result<(), LifecycleExecutionError> {
        self.reconcile_definitions_inner(definitions, callbacks, mode, true)
    }

    fn reconcile_definitions_inner(
        &mut self,
        definitions: impl IntoIterator<Item = ComponentDefinition>,
        callbacks: &mut impl ComponentLifecycleCallbacks,
        mode: ComponentLifecycleMode,
        should_commit_authority: bool,
    ) -> Result<(), LifecycleExecutionError> {
        if let Err(error) = self.ensure_running() {
            if should_commit_authority {
                callbacks.abort_authority();
            }
            return Err(error);
        }
        let definitions = definitions.into_iter().collect::<Vec<_>>();
        match self
            .graph
            .prepare_definition_reconciliation(definitions.clone())
        {
            Ok(_) | Err(ComponentGraphError::DefinitionCleanupBlocked { .. }) => {}
            Err(error) => {
                if should_commit_authority {
                    callbacks.abort_authority();
                }
                return Err(LifecycleExecutionError::graph("runtime_composition", error));
            }
        }
        if let Err(error) = self.retry_pending_cleanup(callbacks, mode) {
            if should_commit_authority {
                callbacks.abort_authority();
            }
            return Err(error);
        }
        let prepared = match self.graph.prepare_definition_reconciliation(definitions) {
            Ok(prepared) => prepared,
            Err(error) => {
                if should_commit_authority {
                    callbacks.abort_authority();
                }
                return Err(LifecycleExecutionError::graph("runtime_composition", error));
            }
        };
        let retirement_order = prepared.retirement_order().to_vec();
        let mut failures = Vec::new();

        for component_id in retirement_order {
            if !self.graph.is_active(&component_id) {
                continue;
            }
            let report = match self.graph.suspend(&component_id) {
                Ok(report) => report,
                Err(error) => {
                    failures.extend(
                        LifecycleExecutionError::graph(component_id.clone(), error).failures,
                    );
                    continue;
                }
            };
            if let Err(error) = self.execute_report(report, callbacks, mode) {
                failures.extend(error.failures);
            }
        }
        if let Err(error) = LifecycleExecutionError::finish(failures) {
            if should_commit_authority {
                callbacks.abort_authority();
            }
            return Err(error);
        }

        if should_commit_authority && callbacks.prepare_authority().is_err() {
            callbacks.abort_authority();
            return Err(LifecycleExecutionError::authority_preparation());
        }

        let activation_order =
            self.commit_prepared_graph(prepared, callbacks, should_commit_authority)?;
        // Candidate preparation 与 graph commit 已完成全部 fallible work；authority switch 是
        // 同一 coordinator critical section 中的 infallible publication，随后才允许 fresh callbacks。
        if should_commit_authority {
            callbacks.commit_authority();
        }
        self.activate_components(activation_order, callbacks, mode)
    }

    fn commit_prepared_graph(
        &mut self,
        prepared: PreparedDefinitionReconciliation,
        callbacks: &mut impl ComponentLifecycleCallbacks,
        should_commit_authority: bool,
    ) -> Result<Vec<String>, LifecycleExecutionError> {
        self.graph
            .commit_definition_reconciliation(prepared)
            .map_err(|error| {
                if should_commit_authority {
                    callbacks.abort_authority();
                }
                LifecycleExecutionError::graph("runtime_composition", error)
            })
    }

    #[cfg(test)]
    pub(super) fn commit_prepared_graph_for_test(
        &mut self,
        prepared: PreparedDefinitionReconciliation,
        callbacks: &mut impl ComponentLifecycleCallbacks,
    ) -> Result<Vec<String>, LifecycleExecutionError> {
        self.commit_prepared_graph(prepared, callbacks, true)
    }

    #[cfg(test)]
    pub(super) fn remove_capability(
        &mut self,
        provider_component: &str,
        capability: &CapabilityKey,
        callbacks: &mut impl ComponentLifecycleCallbacks,
        mode: ComponentLifecycleMode,
    ) -> Result<(), LifecycleExecutionError> {
        self.ensure_running()?;
        let snapshot = self.graph.capability(capability).ok_or_else(|| {
            LifecycleExecutionError::graph(
                provider_component,
                ComponentGraphError::UndeclaredCapabilityProvider {
                    capability: capability.to_string(),
                },
            )
        })?;
        let (tentative_graph, reports) = self
            .graph
            .prepare_capability_removals(provider_component, [capability.clone()])
            .map_err(|error| LifecycleExecutionError::graph(provider_component, error))?;
        self.context
            .hide_batch(&[snapshot])
            .map_err(|_error| LifecycleExecutionError {
                failures: vec![LifecycleExecutionFailure {
                    component_id: provider_component.to_string(),
                    operation: LifecycleExecutionOperation::Graph,
                }],
            })?;
        self.graph = tentative_graph;
        self.execute_reports(reports, callbacks, mode)
    }

    pub(super) fn validate_reconfiguration<P, C>(
        &self,
        replacements: impl IntoIterator<Item = (P, CapabilityKey)>,
        activation_roots: impl IntoIterator<Item = C>,
    ) -> Result<(), LifecycleExecutionError>
    where
        P: AsRef<str>,
        C: AsRef<str>,
    {
        for (provider_component, capability) in replacements {
            let provider_component = provider_component.as_ref();
            self.graph
                .validate_replacement(provider_component, &capability)
                .map_err(|error| LifecycleExecutionError::graph(provider_component, error))?;
        }
        let activation_roots = activation_roots
            .into_iter()
            .map(|component_id| component_id.as_ref().to_string())
            .collect::<Vec<_>>();
        let activation_closure = self
            .graph
            .activation_closure(activation_roots)
            .map_err(|error| LifecycleExecutionError::graph("runtime_composition", error))?;
        for component_id in activation_closure {
            self.graph
                .validate_activation_epoch(&component_id)
                .map_err(|error| LifecycleExecutionError::graph(component_id, error))?;
        }
        Ok(())
    }

    pub(super) fn deactivate_components(
        &mut self,
        component_ids: impl IntoIterator<Item = impl Into<String>>,
        callbacks: &mut impl ComponentLifecycleCallbacks,
        mode: ComponentLifecycleMode,
    ) -> Result<(), LifecycleExecutionError> {
        self.ensure_running()?;
        self.retry_pending_cleanup(callbacks, mode)?;
        self.deactivate_components_inner(component_ids, callbacks, mode)
    }

    fn deactivate_components_inner(
        &mut self,
        component_ids: impl IntoIterator<Item = impl Into<String>>,
        callbacks: &mut impl ComponentLifecycleCallbacks,
        mode: ComponentLifecycleMode,
    ) -> Result<(), LifecycleExecutionError> {
        let requested = component_ids
            .into_iter()
            .map(Into::into)
            .collect::<BTreeSet<_>>();
        let order = self
            .graph
            .deactivation_order()
            .into_iter()
            .filter(|component_id| requested.contains(component_id))
            .collect::<Vec<_>>();
        let mut failures = Vec::new();
        let mut reports = Vec::new();

        for component_id in order {
            let component_report = match mode {
                ComponentLifecycleMode::Reconfigure => self.graph.suspend(&component_id),
                ComponentLifecycleMode::Initial
                | ComponentLifecycleMode::WakeBinding
                | ComponentLifecycleMode::Shutdown => self.graph.deactivate(&component_id),
            };
            match component_report {
                Ok(component_report) => reports.push(component_report),
                Err(error) => failures
                    .extend(LifecycleExecutionError::graph(component_id.clone(), error).failures),
            }
        }
        if let Err(error) = self.execute_reports(reports, callbacks, mode) {
            failures.extend(error.failures);
        }

        LifecycleExecutionError::finish(failures)
    }

    pub(super) fn activate_components(
        &mut self,
        component_ids: impl IntoIterator<Item = impl Into<String>>,
        callbacks: &mut impl ComponentLifecycleCallbacks,
        mode: ComponentLifecycleMode,
    ) -> Result<(), LifecycleExecutionError> {
        self.ensure_running()?;
        self.retry_pending_cleanup(callbacks, mode)?;
        let requested = component_ids
            .into_iter()
            .map(Into::into)
            .collect::<BTreeSet<_>>();
        let order = self
            .graph
            .activation_order()
            .into_iter()
            .filter(|component_id| requested.contains(component_id))
            .collect::<Vec<_>>();
        let mut failures = Vec::new();
        let mut reports = Vec::new();

        for component_id in order {
            let is_recoverable_activation_failure =
                self.graph.failures().into_iter().any(|failure| {
                    failure.component_id == component_id
                        && failure.operation == ComponentFailureOperation::Activation
                        && failure.recoverable
                });
            let report = if is_recoverable_activation_failure {
                self.graph.retry(&component_id)
            } else {
                self.graph.activate(&component_id)
            };
            match report {
                Ok(report) => reports.push(report),
                Err(error) => {
                    failures.extend(
                        LifecycleExecutionError::graph(component_id.clone(), error).failures,
                    );
                    continue;
                }
            }
        }
        if let Err(error) = self.execute_reports(reports, callbacks, mode) {
            failures.extend(error.failures);
        }

        LifecycleExecutionError::finish(failures)
    }

    pub(super) fn shutdown(
        &mut self,
        callbacks: &mut impl ComponentLifecycleCallbacks,
    ) -> Result<(), LifecycleExecutionError> {
        if self.finalization == LifecycleFinalization::Succeeded {
            return Ok(());
        }
        if self.finalization == LifecycleFinalization::Open {
            self.finalization = LifecycleFinalization::Finalizing;
            callbacks.abort_authority();
        }
        let mut failures = self
            .retry_pending_cleanup(callbacks, ComponentLifecycleMode::Shutdown)
            .err()
            .map(|error| error.failures)
            .unwrap_or_default();
        let activation_failed = self
            .graph
            .failures()
            .into_iter()
            .filter(|failure| failure.operation == ComponentFailureOperation::Activation)
            .map(|failure| failure.component_id)
            .collect::<BTreeSet<_>>();
        let cleanup_blocked = self
            .component_scopes
            .iter()
            .filter_map(|(component_id, scope)| {
                scope.snapshot().and_then(|snapshot| {
                    (snapshot.lifecycle == EffectScopeLifecycleSnapshot::Finalizing
                        || activation_failed.contains(component_id))
                    .then(|| component_id.clone())
                })
            })
            .collect::<BTreeSet<_>>();
        let deactivation_failed = self
            .graph
            .failures()
            .into_iter()
            .filter(|failure| failure.operation == ComponentFailureOperation::Deactivation)
            .map(|failure| failure.component_id)
            .collect::<BTreeSet<_>>();
        let component_ids = self
            .graph
            .deactivation_order()
            .into_iter()
            .filter(|component_id| {
                !cleanup_blocked.contains(component_id)
                    && !deactivation_failed.contains(component_id)
            })
            .collect::<Vec<_>>();
        failures.extend(
            self.deactivate_components_inner(
                component_ids,
                callbacks,
                ComponentLifecycleMode::Shutdown,
            )
            .err()
            .map(|error| error.failures)
            .unwrap_or_default(),
        );
        let has_graph_failure = failures
            .iter()
            .any(|failure| failure.operation == LifecycleExecutionOperation::Graph);
        if !has_graph_failure && failures.is_empty() && self.component_scopes.is_empty() {
            let report = self.root_scope.dispose();
            if !report.is_success() {
                failures.push(cleanup_pending_failure(
                    "runtime_composition".to_string(),
                    LifecycleExecutionOperation::EffectDisposal,
                ));
            }
        }
        if failures.is_empty()
            && self.component_scopes.is_empty()
            && self.root_scope.snapshot().is_none()
        {
            self.finalization = LifecycleFinalization::Succeeded;
        }
        LifecycleExecutionError::finish(failures)
    }

    fn ensure_running(&self) -> Result<(), LifecycleExecutionError> {
        if self.finalization != LifecycleFinalization::Open {
            Err(LifecycleExecutionError::executor_shutdown())
        } else {
            Ok(())
        }
    }

    fn retry_pending_cleanup(
        &mut self,
        callbacks: &mut impl ComponentLifecycleCallbacks,
        mode: ComponentLifecycleMode,
    ) -> Result<(), LifecycleExecutionError> {
        let mut failures = Vec::new();
        let graph_failures = self
            .graph
            .failures()
            .into_iter()
            .map(|failure| (failure.component_id.clone(), failure))
            .collect::<BTreeMap<_, _>>();
        let cleanup_order = self.graph.deactivation_order();
        let pending_scopes = cleanup_order
            .iter()
            .filter_map(|component_id| {
                let failure = graph_failures.get(component_id)?;
                let scope = self.component_scopes.get(component_id)?;
                let lifecycle = scope.snapshot()?.lifecycle;
                (failure.operation == ComponentFailureOperation::Activation)
                    .then(|| (component_id.clone(), lifecycle))
            })
            .collect::<Vec<_>>();

        for (component_id, lifecycle) in pending_scopes {
            if lifecycle == EffectScopeLifecycleSnapshot::Active
                && callbacks.quiesce_component(&component_id, mode).is_err()
            {
                failures.push(cleanup_pending_failure(
                    component_id,
                    LifecycleExecutionOperation::Quiescence,
                ));
                continue;
            }
            let report = self
                .component_scopes
                .get(&component_id)
                .expect("pending scope should remain owned")
                .dispose();
            if report.is_success() {
                self.component_scopes.remove(&component_id);
            } else {
                failures.push(cleanup_pending_failure(
                    component_id,
                    LifecycleExecutionOperation::EffectDisposal,
                ));
            }
        }

        for component_id in cleanup_order {
            let Some(failure) = graph_failures.get(&component_id) else {
                continue;
            };
            if failure.operation != ComponentFailureOperation::Deactivation || !failure.recoverable
            {
                continue;
            }
            let report = match self.graph.retry(&component_id) {
                Ok(report) => report,
                Err(error) => {
                    failures.extend(
                        LifecycleExecutionError::graph(component_id.clone(), error).failures,
                    );
                    continue;
                }
            };
            if let Err(error) = self.execute_report(report, callbacks, mode) {
                failures.extend(error.failures);
            }
        }

        LifecycleExecutionError::finish(failures)
    }

    fn execute_report(
        &mut self,
        report: ReconciliationReport,
        callbacks: &mut impl ComponentLifecycleCallbacks,
        mode: ComponentLifecycleMode,
    ) -> Result<(), LifecycleExecutionError> {
        self.execute_reports([report], callbacks, mode)
    }

    fn execute_reports(
        &mut self,
        reports: impl IntoIterator<Item = ReconciliationReport>,
        callbacks: &mut impl ComponentLifecycleCallbacks,
        mode: ComponentLifecycleMode,
    ) -> Result<(), LifecycleExecutionError> {
        let mut state = LifecycleExecutionState::new(&self.graph);
        self.execute_reports_inner(reports.into_iter().collect(), callbacks, mode, &mut state);
        LifecycleExecutionError::finish(state.failures)
    }

    fn execute_reports_inner(
        &mut self,
        mut pending: VecDeque<ReconciliationReport>,
        callbacks: &mut impl ComponentLifecycleCallbacks,
        mode: ComponentLifecycleMode,
        state: &mut LifecycleExecutionState,
    ) {
        let mut deactivations = Vec::new();
        let mut activations = Vec::new();

        loop {
            while let Some(report) = pending.pop_front() {
                deactivations.extend(report.deactivation_requests);
                activations.extend(report.activation_requests);
            }

            deactivations.sort_by_key(|token| {
                state
                    .deactivation_rank
                    .get(token.component_id())
                    .copied()
                    .unwrap_or(usize::MAX)
            });
            for token in deactivations.drain(..) {
                let action_key = (token.component_id().to_string(), token.epoch());
                if !state.seen_deactivations.insert(action_key) {
                    continue;
                }
                self.execute_deactivation(token, callbacks, mode, &mut pending, state);
            }
            if !pending.is_empty() {
                continue;
            }

            activations.sort_by_key(|token| {
                state
                    .activation_rank
                    .get(token.component_id())
                    .copied()
                    .unwrap_or(usize::MAX)
            });
            let Some(token) = activations.first().cloned() else {
                break;
            };
            activations.remove(0);
            if state.seen_activations.insert(token.clone()) {
                self.execute_activation(token, callbacks, mode, &mut pending, state);
            }
        }
    }

    fn execute_activation(
        &mut self,
        token: ActivationToken,
        callbacks: &mut impl ComponentLifecycleCallbacks,
        mode: ComponentLifecycleMode,
        pending: &mut VecDeque<ReconciliationReport>,
        state: &mut LifecycleExecutionState,
    ) {
        let component_id = token.component_id().to_string();
        let scope = match self.root_scope.child(component_id.clone()) {
            Ok(scope) => scope,
            Err(_error) => {
                state.failures.push(LifecycleExecutionFailure {
                    component_id: component_id.clone(),
                    operation: LifecycleExecutionOperation::Activation,
                });
                self.reject_activation(token, &component_id, pending, state);
                return;
            }
        };

        let declared_capabilities = match self.graph.provided_capabilities(&component_id) {
            Ok(capabilities) => capabilities,
            Err(error) => {
                state
                    .failures
                    .extend(LifecycleExecutionError::graph(component_id.clone(), error).failures);
                let disposal = scope.dispose();
                if !disposal.is_success() {
                    state.failures.push(cleanup_pending_failure(
                        component_id.clone(),
                        LifecycleExecutionOperation::EffectDisposal,
                    ));
                    self.component_scopes.insert(component_id.clone(), scope);
                }
                self.reject_activation(token, &component_id, pending, state);
                return;
            }
        };
        let mut activation_context =
            self.context
                .activation(&scope, &component_id, declared_capabilities);
        let outcome = match callbacks.activate_component(
            &component_id,
            &scope,
            &mut activation_context,
            mode,
        ) {
            Ok(outcome) => outcome,
            Err(_message) => {
                state.failures.push(LifecycleExecutionFailure {
                    component_id: component_id.clone(),
                    operation: LifecycleExecutionOperation::Activation,
                });
                self.rollback_activation(token, scope, callbacks, mode, pending, state);
                return;
            }
        };

        let publishes_capabilities = outcome == ComponentActivationOutcome::PublishCapabilities;
        let staged = match activation_context.finish(publishes_capabilities) {
            Ok(staged) => staged,
            Err(_error) => {
                state.failures.push(LifecycleExecutionFailure {
                    component_id: component_id.clone(),
                    operation: LifecycleExecutionOperation::Activation,
                });
                self.rollback_activation(token, scope, callbacks, mode, pending, state);
                return;
            }
        };

        let completion = match outcome {
            ComponentActivationOutcome::Ready => self
                .graph
                .prepare_activation(token.clone())
                .map(|(graph, report)| (graph, vec![report])),
            ComponentActivationOutcome::PublishCapabilities => {
                self.graph.prepare_activation_and_publish(token.clone())
            }
        };
        match completion {
            Ok((tentative_graph, reports)) => {
                if let Err(_error) = self
                    .context
                    .commit(&staged, &tentative_graph.capabilities())
                {
                    state.failures.push(LifecycleExecutionFailure {
                        component_id: component_id.clone(),
                        operation: LifecycleExecutionOperation::Activation,
                    });
                    self.rollback_activation(token, scope, callbacks, mode, pending, state);
                    return;
                }
                self.graph = tentative_graph;
                self.component_scopes.insert(component_id.clone(), scope);
                pending.extend(reports);
            }
            Err(_error) => {
                state.failures.push(LifecycleExecutionFailure {
                    component_id: component_id.clone(),
                    operation: LifecycleExecutionOperation::Graph,
                });
                self.rollback_activation(token, scope, callbacks, mode, pending, state);
            }
        }
    }

    fn rollback_activation(
        &mut self,
        token: ActivationToken,
        scope: EffectScope,
        callbacks: &mut impl ComponentLifecycleCallbacks,
        mode: ComponentLifecycleMode,
        pending: &mut VecDeque<ReconciliationReport>,
        state: &mut LifecycleExecutionState,
    ) {
        let component_id = token.component_id().to_string();
        if callbacks.quiesce_component(&component_id, mode).is_err() {
            state.failures.push(cleanup_pending_failure(
                component_id.clone(),
                LifecycleExecutionOperation::Quiescence,
            ));
            self.component_scopes.insert(component_id.clone(), scope);
            self.reject_activation(token, &component_id, pending, state);
            return;
        }
        let disposal = scope.dispose();
        if !disposal.is_success() {
            state.failures.push(cleanup_pending_failure(
                component_id.clone(),
                LifecycleExecutionOperation::EffectDisposal,
            ));
            self.component_scopes.insert(component_id.clone(), scope);
        }
        self.reject_activation(token, &component_id, pending, state);
    }

    fn reject_activation(
        &mut self,
        token: ActivationToken,
        component_id: &str,
        pending: &mut VecDeque<ReconciliationReport>,
        state: &mut LifecycleExecutionState,
    ) {
        match self
            .graph
            .fail_activation(token, ComponentFailureReason::ActivationRejected, true)
        {
            Ok(report) => pending.push_back(report),
            Err(error) => state
                .failures
                .extend(LifecycleExecutionError::graph(component_id.to_string(), error).failures),
        }
    }

    fn execute_deactivation(
        &mut self,
        token: DeactivationToken,
        callbacks: &mut impl ComponentLifecycleCallbacks,
        mode: ComponentLifecycleMode,
        pending: &mut VecDeque<ReconciliationReport>,
        state: &mut LifecycleExecutionState,
    ) {
        let component_id = token.component_id().to_string();
        let mut dependent_reports = VecDeque::new();
        let preparation = match self.graph.provided_capabilities(&component_id) {
            Ok(capabilities) => {
                let active = capabilities
                    .into_iter()
                    .filter(|capability| self.graph.has_capability(capability))
                    .collect::<Vec<_>>();
                let snapshots = active
                    .iter()
                    .filter_map(|capability| self.graph.capability(capability))
                    .collect::<Vec<_>>();
                match self
                    .graph
                    .prepare_capability_removals(&component_id, active)
                {
                    Ok((tentative_graph, reports)) => {
                        if self.context.hide_batch(&snapshots).is_err() {
                            Err(())
                        } else {
                            self.graph = tentative_graph;
                            dependent_reports.extend(reports);
                            Ok(())
                        }
                    }
                    Err(_) => Err(()),
                }
            }
            Err(_) => Err(()),
        };
        if preparation.is_err() {
            state.failures.push(LifecycleExecutionFailure {
                component_id: component_id.clone(),
                operation: LifecycleExecutionOperation::Graph,
            });
            if let Err(error) = self.graph.rollback_deactivation(token) {
                state
                    .failures
                    .extend(LifecycleExecutionError::graph(component_id, error).failures);
            }
            return;
        }
        self.execute_reports_inner(dependent_reports, callbacks, mode, state);
        let quiescence_error = callbacks
            .quiesce_component(&component_id, mode)
            .is_err()
            .then(|| {
                state.failures.push(cleanup_pending_failure(
                    component_id.clone(),
                    LifecycleExecutionOperation::Quiescence,
                ));
            });
        let disposal_error = quiescence_error.is_none().then(|| {
            self.component_scopes
                .get(&component_id)
                .is_some_and(|scope| {
                    let report = scope.dispose();
                    if report.is_success() {
                        false
                    } else {
                        state.failures.push(cleanup_pending_failure(
                            component_id.clone(),
                            LifecycleExecutionOperation::EffectDisposal,
                        ));
                        true
                    }
                })
        });
        let has_disposal_error = disposal_error.unwrap_or(false);
        if quiescence_error.is_none() && !has_disposal_error {
            self.component_scopes.remove(&component_id);
        }

        let result = if quiescence_error.is_some() {
            self.graph
                .fail_deactivation(token, ComponentFailureReason::QuiescenceRejected, true)
        } else if has_disposal_error {
            self.graph.fail_deactivation(
                token,
                ComponentFailureReason::EffectDisposalRejected,
                true,
            )
        } else {
            self.graph.complete_deactivation(token)
        };
        match result {
            Ok(report) => pending.push_back(report),
            Err(error) => state
                .failures
                .extend(LifecycleExecutionError::graph(component_id, error).failures),
        }
    }
}

fn cleanup_pending_failure(
    component_id: String,
    operation: LifecycleExecutionOperation,
) -> LifecycleExecutionFailure {
    LifecycleExecutionFailure {
        component_id,
        operation,
    }
}

fn rank(order: Vec<String>) -> BTreeMap<String, usize> {
    order
        .into_iter()
        .enumerate()
        .map(|(index, component_id)| (component_id, index))
        .collect()
}

#[cfg(test)]
mod property_tests;

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use extension_hook_runtime::{
        BeforeTurnDecision, BeforeTurnPayload, ExtensionHookRegistry, HookDispatchError,
        HookDispatchErrorKind, HookFailureKind, HookId, HookOwnerId, HookPriority,
        HookRegistrationOptions,
    };
    use provider_protocol::{ConversationItem, Role};
    use tokio::sync::Notify;
    use tokio_util::sync::CancellationToken;

    use super::*;
    use crate::runtime::context::ExtensionHookRegistryCapability;

    #[derive(Default)]
    struct FakeCallbacks {
        events: Arc<Mutex<Vec<String>>>,
        authority: Option<Arc<Mutex<String>>>,
        authority_after_commit: Option<String>,
        authority_preparation_failure: bool,
        authority_preparations: usize,
        authority_commits: usize,
        authority_aborts: usize,
        publishers: BTreeSet<String>,
        activation_failures: BTreeSet<String>,
        quiescence_failures: BTreeSet<String>,
        disposal_failures: Arc<Mutex<BTreeSet<String>>>,
    }

    impl FakeCallbacks {
        fn publishing(component_ids: &[&str]) -> Self {
            Self {
                publishers: component_ids
                    .iter()
                    .map(|component_id| (*component_id).to_string())
                    .collect(),
                ..Self::default()
            }
        }

        fn record(&self, event: impl Into<String>) {
            self.events
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(event.into());
        }

        fn snapshot(&self) -> Vec<String> {
            self.events
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        }

        fn record_lifecycle(&self, phase: &str, component_id: &str) {
            let event = self
                .authority
                .as_ref()
                .map(|authority| {
                    let authority = authority
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    format!("{phase}:{component_id}:{authority}")
                })
                .unwrap_or_else(|| format!("{phase}:{component_id}"));
            self.record(event);
        }

        fn fail_next_disposal(&self, component_id: &str) {
            self.disposal_failures
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .insert(component_id.to_string());
        }
    }

    impl ComponentLifecycleCallbacks for FakeCallbacks {
        fn prepare_authority(&mut self) -> Result<(), String> {
            self.authority_preparations += 1;
            if self.authority_preparation_failure {
                Err("AUTHORITY_PREPARATION_SECRET".to_string())
            } else {
                Ok(())
            }
        }

        fn commit_authority(&mut self) {
            self.authority_commits += 1;
            if let (Some(authority), Some(next)) =
                (&self.authority, self.authority_after_commit.take())
            {
                *authority
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = next;
            }
        }

        fn abort_authority(&mut self) {
            self.authority_aborts += 1;
        }

        fn activate_component(
            &mut self,
            component_id: &str,
            scope: &EffectScope,
            context: &mut ComponentActivationContext<'_>,
            _mode: ComponentLifecycleMode,
        ) -> Result<ComponentActivationOutcome, String> {
            self.record_lifecycle("activate", component_id);
            let events = Arc::clone(&self.events);
            let disposal_failures = Arc::clone(&self.disposal_failures);
            let disposed_component = component_id.to_string();
            scope
                .register("owned_effect", move || {
                    events
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .push(format!("dispose:{disposed_component}"));
                    if disposal_failures
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .remove(&disposed_component)
                    {
                        Err("DISPOSER_SECRET".to_string())
                    } else {
                        Ok(())
                    }
                })
                .map_err(|error| error.to_string())?;
            if self.activation_failures.contains(component_id) {
                return Err("ACTIVATION_SECRET".to_string());
            }
            if self.publishers.contains(component_id) {
                context
                    .publish_declared_presence()
                    .map_err(|error| error.to_string())?;
                Ok(ComponentActivationOutcome::PublishCapabilities)
            } else {
                Ok(ComponentActivationOutcome::Ready)
            }
        }

        fn quiesce_component(
            &mut self,
            component_id: &str,
            _mode: ComponentLifecycleMode,
        ) -> Result<(), String> {
            self.record_lifecycle("quiesce", component_id);
            if self.quiescence_failures.remove(component_id) {
                Err("QUIESCENCE_SECRET".to_string())
            } else {
                Ok(())
            }
        }
    }

    fn assert_closed_lifecycle_error(
        error: &LifecycleExecutionError,
        component_id: &str,
        operation: &str,
        forbidden: &[&str],
    ) {
        let display = error.to_string();
        let debug = format!("{error:?}");
        assert!(
            display.contains(&format!("component {component_id} {operation} failed")),
            "unexpected lifecycle error projection: {display}"
        );
        for sentinel in forbidden {
            assert!(!display.contains(sentinel));
            assert!(!debug.contains(sentinel));
        }
        assert!(std::error::Error::source(error).is_none());
    }

    const HOOK_PROVIDER_COMPONENT: &str = "extension-hooks";
    const HOOK_PRODUCER_COMPONENT: &str = "hook-producer";

    struct HookLifecycleCallbacks {
        registry: ExtensionHookRegistry,
        invocation_started: Arc<Notify>,
        reject_producer_activation: bool,
    }

    impl HookLifecycleCallbacks {
        fn new(reject_producer_activation: bool) -> Self {
            Self {
                registry: ExtensionHookRegistry::new(),
                invocation_started: Arc::new(Notify::new()),
                reject_producer_activation,
            }
        }
    }

    impl ComponentLifecycleCallbacks for HookLifecycleCallbacks {
        fn prepare_authority(&mut self) -> Result<(), String> {
            Ok(())
        }

        fn commit_authority(&mut self) {}

        fn abort_authority(&mut self) {}

        fn activate_component(
            &mut self,
            component_id: &str,
            scope: &EffectScope,
            context: &mut ComponentActivationContext<'_>,
            _mode: ComponentLifecycleMode,
        ) -> Result<ComponentActivationOutcome, String> {
            match component_id {
                HOOK_PROVIDER_COMPONENT => {
                    context
                        .publish::<ExtensionHookRegistryCapability>(self.registry.clone())
                        .map_err(|error| error.to_string())?;
                    Ok(ComponentActivationOutcome::PublishCapabilities)
                }
                HOOK_PRODUCER_COMPONENT => {
                    let registry = context
                        .require::<ExtensionHookRegistryCapability>()
                        .map_err(|error| error.to_string())?;
                    let invocation_started = Arc::clone(&self.invocation_started);
                    let mut registration = registry
                        .register_before_turn(
                            HookOwnerId::try_new(component_id)
                                .expect("component id should be a valid hook owner"),
                            HookId::try_new("before-turn").expect("hook id should validate"),
                            HookRegistrationOptions::try_new(
                                HookPriority::default(),
                                std::time::Duration::from_secs(30),
                            )
                            .expect("hook options should validate"),
                            Arc::new(move |_: BeforeTurnPayload, _| {
                                let invocation_started = Arc::clone(&invocation_started);
                                async move {
                                    invocation_started.notify_one();
                                    std::future::pending::<
                                        Result<BeforeTurnDecision, HookFailureKind>,
                                    >()
                                    .await
                                }
                            }),
                        )
                        .map_err(|error| error.to_string())?;
                    scope
                        .register("before_turn_hook", move || {
                            registration.dispose();
                            Ok(())
                        })
                        .map_err(|error| error.to_string())?;
                    if self.reject_producer_activation {
                        return Err("hook producer activation rejected".to_string());
                    }
                    Ok(ComponentActivationOutcome::Ready)
                }
                _ => Err("unknown test component".to_string()),
            }
        }

        fn quiesce_component(
            &mut self,
            _component_id: &str,
            _mode: ComponentLifecycleMode,
        ) -> Result<(), String> {
            Ok(())
        }
    }

    fn hook_definitions(producer_implementation: &str) -> Vec<ComponentDefinition> {
        vec![
            ComponentDefinition::new(HOOK_PROVIDER_COMPONENT)
                .implemented_by("typed-extension-hooks")
                .provides(ExtensionHookRegistryCapability::KEY),
            ComponentDefinition::new(HOOK_PRODUCER_COMPONENT)
                .implemented_by(producer_implementation)
                .requires(ExtensionHookRegistryCapability::KEY),
        ]
    }

    fn hook_turn_payload() -> BeforeTurnPayload {
        BeforeTurnPayload::try_new(vec![ConversationItem::text(Role::User, "delivery")])
            .expect("hook payload should validate")
    }

    fn spawn_hook_dispatch(
        registry: ExtensionHookRegistry,
    ) -> tokio::task::JoinHandle<Result<BeforeTurnPayload, HookDispatchError>> {
        tokio::spawn(async move {
            let cancellation = CancellationToken::new();
            registry
                .dispatch_before_turn(hook_turn_payload(), &cancellation)
                .await
        })
    }

    async fn expect_registration_disposed(
        dispatch: tokio::task::JoinHandle<Result<BeforeTurnPayload, HookDispatchError>>,
    ) {
        let error = tokio::time::timeout(std::time::Duration::from_secs(1), dispatch)
            .await
            .expect("registration disposal should stop the invocation")
            .expect("dispatch task should not panic")
            .expect_err("disposed registration should fail closed");
        assert_eq!(error.kind(), HookDispatchErrorKind::RegistrationDisposed);
    }

    #[test]
    fn hook_registration_rolls_back_through_the_lifecycle_executor() {
        let mut executor = ComponentLifecycleExecutor::default();
        let mut callbacks = HookLifecycleCallbacks::new(true);

        executor
            .reconcile_definitions(
                hook_definitions("hook-producer-v1"),
                &mut callbacks,
                ComponentLifecycleMode::Initial,
            )
            .expect_err("producer activation should be rejected");

        assert!(callbacks.registry.snapshot().is_empty());
        assert_eq!(
            executor.graph().state(HOOK_PRODUCER_COMPONENT),
            Some(ComponentState::Failed)
        );
    }

    #[tokio::test]
    async fn hook_registration_removal_cancels_the_in_flight_invocation() {
        let mut executor = ComponentLifecycleExecutor::default();
        let mut callbacks = HookLifecycleCallbacks::new(false);
        executor
            .reconcile_definitions(
                hook_definitions("hook-producer-v1"),
                &mut callbacks,
                ComponentLifecycleMode::Initial,
            )
            .expect("hook producer should activate");
        assert_eq!(
            callbacks.registry.snapshot()[0].owner.as_str(),
            HOOK_PRODUCER_COMPONENT
        );
        let dispatch = spawn_hook_dispatch(callbacks.registry.clone());
        callbacks.invocation_started.notified().await;

        executor
            .reconcile_definitions(
                [ComponentDefinition::new(HOOK_PROVIDER_COMPONENT)
                    .implemented_by("typed-extension-hooks")
                    .provides(ExtensionHookRegistryCapability::KEY)],
                &mut callbacks,
                ComponentLifecycleMode::Reconfigure,
            )
            .expect("hook producer should be removed");

        expect_registration_disposed(dispatch).await;
        assert!(callbacks.registry.snapshot().is_empty());
    }

    #[tokio::test]
    async fn hook_registration_replacement_and_shutdown_dispose_each_generation() {
        let mut executor = ComponentLifecycleExecutor::default();
        let mut callbacks = HookLifecycleCallbacks::new(false);
        executor
            .reconcile_definitions(
                hook_definitions("hook-producer-v1"),
                &mut callbacks,
                ComponentLifecycleMode::Initial,
            )
            .expect("initial hook producer should activate");
        let old_dispatch = spawn_hook_dispatch(callbacks.registry.clone());
        callbacks.invocation_started.notified().await;

        executor
            .reconcile_definitions(
                hook_definitions("hook-producer-v2"),
                &mut callbacks,
                ComponentLifecycleMode::Reconfigure,
            )
            .expect("replacement hook producer should activate");

        expect_registration_disposed(old_dispatch).await;
        let snapshot = callbacks.registry.snapshot();
        assert_eq!(snapshot.len(), 1);
        assert_eq!(snapshot[0].owner.as_str(), HOOK_PRODUCER_COMPONENT);

        let fresh_dispatch = spawn_hook_dispatch(callbacks.registry.clone());
        callbacks.invocation_started.notified().await;
        executor
            .shutdown(&mut callbacks)
            .expect("shutdown should dispose the fresh hook producer");

        expect_registration_disposed(fresh_dispatch).await;
        assert!(callbacks.registry.snapshot().is_empty());
    }

    fn definitions() -> Vec<ComponentDefinition> {
        vec![
            ComponentDefinition::new("database").provides("database"),
            ComponentDefinition::new("service")
                .requires("database")
                .provides("service"),
            ComponentDefinition::new("ui").requires("service"),
        ]
    }

    fn storage_definitions() -> Vec<ComponentDefinition> {
        vec![
            ComponentDefinition::new("storage").provides("database"),
            ComponentDefinition::new("service")
                .requires("database")
                .observes("metrics")
                .provides("service"),
            ComponentDefinition::new("ui").requires("service"),
        ]
    }

    #[test]
    fn activation_and_recursive_deactivation_follow_topology() {
        let mut executor = ComponentLifecycleExecutor::default();
        let mut callbacks = FakeCallbacks::publishing(&["database", "service"]);

        executor
            .reconcile_definitions(
                definitions(),
                &mut callbacks,
                ComponentLifecycleMode::Initial,
            )
            .expect("acyclic composition should activate");
        assert_eq!(
            callbacks.snapshot(),
            vec!["activate:database", "activate:service", "activate:ui"]
        );
        assert_eq!(callbacks.authority_preparations, 0);
        assert_eq!(callbacks.authority_commits, 0);
        assert_eq!(callbacks.authority_aborts, 0);
        assert_eq!(executor.graph().state("ui"), Some(ComponentState::Active));
        assert_eq!(executor.scope_snapshots().len(), 3);

        callbacks
            .events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
        executor
            .deactivate_components(
                ["database"],
                &mut callbacks,
                ComponentLifecycleMode::Reconfigure,
            )
            .expect("component tree should deactivate");

        assert_eq!(
            callbacks.snapshot(),
            vec![
                "quiesce:ui",
                "dispose:ui",
                "quiesce:service",
                "dispose:service",
                "quiesce:database",
                "dispose:database",
            ]
        );
        assert_eq!(
            executor.graph().state("database"),
            Some(ComponentState::Pending)
        );
        assert_eq!(
            executor.graph().state("service"),
            Some(ComponentState::Pending)
        );
        assert_eq!(executor.graph().state("ui"), Some(ComponentState::Pending));
        assert!(executor.scope_snapshots().is_empty());
    }

    #[test]
    fn authority_switch_happens_after_old_cleanup_before_fresh_activation() {
        let authority = Arc::new(Mutex::new("old".to_string()));
        let mut executor = ComponentLifecycleExecutor::default();
        let mut callbacks = FakeCallbacks {
            authority: Some(Arc::clone(&authority)),
            publishers: ["database", "storage", "service"]
                .into_iter()
                .map(str::to_string)
                .collect(),
            ..FakeCallbacks::default()
        };
        executor
            .reconcile_definitions(
                definitions(),
                &mut callbacks,
                ComponentLifecycleMode::Initial,
            )
            .expect("initial composition should activate");
        callbacks
            .events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
        callbacks.authority_after_commit = Some("fresh".to_string());

        executor
            .reconcile_definitions_with_commit(
                storage_definitions(),
                &mut callbacks,
                ComponentLifecycleMode::Reconfigure,
            )
            .expect("fresh composition should activate");

        assert_eq!(
            callbacks.snapshot(),
            vec![
                "quiesce:ui:old",
                "dispose:ui",
                "quiesce:service:old",
                "dispose:service",
                "quiesce:database:old",
                "dispose:database",
                "activate:storage:fresh",
                "activate:service:fresh",
                "activate:ui:fresh",
            ]
        );
        assert_eq!(callbacks.authority_preparations, 1);
        assert_eq!(callbacks.authority_commits, 1);
        assert_eq!(callbacks.authority_aborts, 0);
    }

    #[test]
    fn graph_preflight_failure_aborts_unpublished_authority() {
        let mut executor = ComponentLifecycleExecutor::default();
        let mut callbacks = FakeCallbacks::default();

        executor
            .reconcile_definitions_with_commit(
                [
                    ComponentDefinition::new("duplicate"),
                    ComponentDefinition::new("duplicate"),
                ],
                &mut callbacks,
                ComponentLifecycleMode::Reconfigure,
            )
            .expect_err("invalid graph must abort before cleanup or authority preparation");

        assert_eq!(callbacks.authority_preparations, 0);
        assert_eq!(callbacks.authority_commits, 0);
        assert_eq!(callbacks.authority_aborts, 1);
    }

    #[test]
    fn cleanup_failure_blocks_authority_switch() {
        let authority = Arc::new(Mutex::new("old".to_string()));
        let mut executor = ComponentLifecycleExecutor::default();
        let mut callbacks = FakeCallbacks {
            authority: Some(Arc::clone(&authority)),
            publishers: ["database", "storage", "service"]
                .into_iter()
                .map(str::to_string)
                .collect(),
            ..FakeCallbacks::default()
        };
        executor
            .reconcile_definitions(
                definitions(),
                &mut callbacks,
                ComponentLifecycleMode::Initial,
            )
            .expect("initial composition should activate");
        callbacks
            .events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
        callbacks.authority_after_commit = Some("fresh".to_string());
        callbacks.quiescence_failures.insert("service".to_string());

        let error = executor
            .reconcile_definitions_with_commit(
                storage_definitions(),
                &mut callbacks,
                ComponentLifecycleMode::Reconfigure,
            )
            .expect_err("cleanup failure must block commit");

        assert_eq!(*authority.lock().unwrap(), "old");
        assert_eq!(callbacks.authority_preparations, 0);
        assert_eq!(callbacks.authority_commits, 0);
        assert_eq!(callbacks.authority_aborts, 1);
        assert!(!format!("{error:?}").contains("QUIESCENCE_SECRET"));
        assert_eq!(executor.graph().state("storage"), None);
    }

    #[test]
    fn effect_disposal_failure_blocks_authority_switch() {
        let authority = Arc::new(Mutex::new("old".to_string()));
        let mut executor = ComponentLifecycleExecutor::default();
        let mut callbacks = FakeCallbacks {
            authority: Some(Arc::clone(&authority)),
            publishers: ["database", "storage", "service"]
                .into_iter()
                .map(str::to_string)
                .collect(),
            ..FakeCallbacks::default()
        };
        executor
            .reconcile_definitions(
                definitions(),
                &mut callbacks,
                ComponentLifecycleMode::Initial,
            )
            .expect("initial composition should activate");
        callbacks
            .events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
        callbacks.authority_after_commit = Some("fresh".to_string());
        executor
            .component_scopes
            .get("service")
            .expect("active service should own a scope")
            .register("failing_cleanup", || Err("DISPOSER_SECRET".to_string()))
            .expect("test disposer should register");

        executor
            .reconcile_definitions_with_commit(
                storage_definitions(),
                &mut callbacks,
                ComponentLifecycleMode::Reconfigure,
            )
            .expect_err("effect disposal failure must block commit");

        assert_eq!(*authority.lock().unwrap(), "old");
        assert_eq!(callbacks.authority_preparations, 0);
        assert_eq!(callbacks.authority_commits, 0);
        assert_eq!(callbacks.authority_aborts, 1);
        assert_eq!(executor.graph().state("storage"), None);
        assert!(
            callbacks
                .snapshot()
                .iter()
                .all(|event| !event.starts_with("activate:storage"))
        );
    }

    #[test]
    fn authority_preparation_failure_aborts_after_cleanup_before_graph_commit() {
        let authority = Arc::new(Mutex::new("old".to_string()));
        let mut executor = ComponentLifecycleExecutor::default();
        let mut callbacks = FakeCallbacks {
            authority: Some(Arc::clone(&authority)),
            publishers: ["database", "storage", "service"]
                .into_iter()
                .map(str::to_string)
                .collect(),
            ..FakeCallbacks::default()
        };
        executor
            .reconcile_definitions(
                definitions(),
                &mut callbacks,
                ComponentLifecycleMode::Initial,
            )
            .expect("initial composition should activate");
        callbacks.authority_after_commit = Some("fresh".to_string());
        callbacks.authority_preparation_failure = true;

        let error = executor
            .reconcile_definitions_with_commit(
                storage_definitions(),
                &mut callbacks,
                ComponentLifecycleMode::Reconfigure,
            )
            .expect_err("authority preparation failure must abort before graph commit");

        assert_eq!(*authority.lock().unwrap(), "old");
        assert_eq!(callbacks.authority_preparations, 1);
        assert_eq!(callbacks.authority_commits, 0);
        assert_eq!(callbacks.authority_aborts, 1);
        assert_closed_lifecycle_error(
            &error,
            "runtime_composition",
            "authority_preparation",
            &["AUTHORITY_PREPARATION_SECRET"],
        );
        assert_eq!(executor.graph().state("storage"), None);
        assert_eq!(
            executor.graph().state("database"),
            Some(ComponentState::Pending)
        );
    }

    #[test]
    fn stale_graph_commit_aborts_already_prepared_authority() {
        let mut executor = ComponentLifecycleExecutor::default();
        let mut callbacks = FakeCallbacks::publishing(&["database", "service"]);
        executor
            .reconcile_definitions(
                definitions(),
                &mut callbacks,
                ComponentLifecycleMode::Initial,
            )
            .expect("initial composition should activate");
        let prepared = executor
            .graph
            .prepare_definition_reconciliation(storage_definitions())
            .expect("replacement graph should preflight");
        callbacks
            .prepare_authority()
            .expect("test authority should prepare");
        executor
            .graph
            .declare(ComponentDefinition::new("unexpected"))
            .expect("test mutation should stale the prepared graph");

        executor
            .commit_prepared_graph(prepared, &mut callbacks, true)
            .expect_err("stale graph commit must abort prepared authority");

        assert_eq!(callbacks.authority_preparations, 1);
        assert_eq!(callbacks.authority_commits, 0);
        assert_eq!(callbacks.authority_aborts, 1);
    }

    #[test]
    fn fresh_activation_failure_keeps_new_authority() {
        let authority = Arc::new(Mutex::new("old".to_string()));
        let mut executor = ComponentLifecycleExecutor::default();
        let mut callbacks = FakeCallbacks {
            authority: Some(Arc::clone(&authority)),
            publishers: ["database", "storage", "service"]
                .into_iter()
                .map(str::to_string)
                .collect(),
            ..FakeCallbacks::default()
        };
        executor
            .reconcile_definitions(
                definitions(),
                &mut callbacks,
                ComponentLifecycleMode::Initial,
            )
            .expect("initial composition should activate");
        callbacks
            .events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
        callbacks.authority_after_commit = Some("fresh".to_string());
        callbacks.activation_failures.insert("storage".to_string());

        executor
            .reconcile_definitions_with_commit(
                storage_definitions(),
                &mut callbacks,
                ComponentLifecycleMode::Reconfigure,
            )
            .expect_err("fresh activation failure should be reported");

        assert_eq!(*authority.lock().unwrap(), "fresh");
        assert_eq!(
            executor.graph().state("storage"),
            Some(ComponentState::Failed)
        );
        assert!(
            callbacks
                .snapshot()
                .iter()
                .any(|event| event == "activate:storage:fresh")
        );
    }

    #[test]
    fn declaration_order_does_not_change_physical_activation_order() {
        let mut forward = ComponentLifecycleExecutor::default();
        let mut forward_callbacks = FakeCallbacks::publishing(&["database", "service"]);
        forward
            .reconcile_definitions(
                definitions(),
                &mut forward_callbacks,
                ComponentLifecycleMode::Initial,
            )
            .expect("forward graph should activate");

        let mut reverse = ComponentLifecycleExecutor::default();
        let mut reverse_callbacks = FakeCallbacks::publishing(&["database", "service"]);
        reverse
            .reconcile_definitions(
                definitions().into_iter().rev(),
                &mut reverse_callbacks,
                ComponentLifecycleMode::Initial,
            )
            .expect("reverse graph should activate");

        assert_eq!(forward_callbacks.snapshot(), reverse_callbacks.snapshot());
        assert_eq!(
            reverse_callbacks.snapshot(),
            vec!["activate:database", "activate:service", "activate:ui"]
        );
    }

    #[test]
    fn mixed_definition_reconciliation_is_graph_ordered_and_deterministic() {
        let mut forward = ComponentLifecycleExecutor::default();
        let mut forward_callbacks = FakeCallbacks::publishing(&["database", "storage", "service"]);
        forward
            .reconcile_definitions(
                definitions(),
                &mut forward_callbacks,
                ComponentLifecycleMode::Initial,
            )
            .expect("initial composition should activate");
        forward_callbacks
            .events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();

        let mut reverse = ComponentLifecycleExecutor::default();
        let mut reverse_callbacks = FakeCallbacks::publishing(&["database", "storage", "service"]);
        reverse
            .reconcile_definitions(
                definitions(),
                &mut reverse_callbacks,
                ComponentLifecycleMode::Initial,
            )
            .expect("initial composition should activate");
        reverse_callbacks
            .events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();

        forward
            .reconcile_definitions(
                storage_definitions(),
                &mut forward_callbacks,
                ComponentLifecycleMode::Reconfigure,
            )
            .expect("mixed definitions should reconcile");
        reverse
            .reconcile_definitions(
                storage_definitions().into_iter().rev(),
                &mut reverse_callbacks,
                ComponentLifecycleMode::Reconfigure,
            )
            .expect("desired input order must not affect reconciliation");

        let expected = vec![
            "quiesce:ui",
            "dispose:ui",
            "quiesce:service",
            "dispose:service",
            "quiesce:database",
            "dispose:database",
            "activate:storage",
            "activate:service",
            "activate:ui",
        ];
        assert_eq!(forward_callbacks.snapshot(), expected);
        assert_eq!(reverse_callbacks.snapshot(), expected);
        assert_eq!(forward.graph().components(), reverse.graph().components());
        assert_eq!(
            forward.graph().capabilities(),
            reverse.graph().capabilities()
        );
        assert_eq!(forward.context_snapshots(), reverse.context_snapshots());
        assert_eq!(forward.graph().state("database"), None);
        assert_eq!(
            forward.graph().provided_capabilities("database"),
            Err(ComponentGraphError::UnknownComponent {
                component_id: "database".to_string(),
            })
        );
        assert_eq!(
            forward
                .graph
                .add_capability("database", CapabilityKey::from("database")),
            Err(ComponentGraphError::UnknownComponent {
                component_id: "database".to_string(),
            })
        );
        assert_eq!(forward.graph().epoch("storage"), Some(1));
        assert_eq!(forward.graph().epoch("service"), Some(2));
        assert_eq!(forward.graph().epoch("ui"), Some(2));
        let database = forward
            .graph()
            .capability(&CapabilityKey::from("database"))
            .expect("storage should publish database");
        assert_eq!(database.key, "database");
        assert_eq!(database.provider_component, "storage");
        assert_eq!(database.generation, 1);
    }

    #[test]
    fn definition_cleanup_failure_keeps_old_authority_and_blocks_fresh_activation() {
        let mut executor = ComponentLifecycleExecutor::default();
        let mut callbacks = FakeCallbacks::publishing(&["database", "storage", "service"]);
        executor
            .reconcile_definitions(
                definitions(),
                &mut callbacks,
                ComponentLifecycleMode::Initial,
            )
            .expect("initial composition should activate");
        callbacks
            .events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
        callbacks.quiescence_failures.insert("service".to_string());

        let error = executor
            .reconcile_definitions(
                storage_definitions(),
                &mut callbacks,
                ComponentLifecycleMode::Reconfigure,
            )
            .expect_err("old cleanup failure must abort definition commit");

        assert_eq!(
            callbacks.snapshot(),
            vec![
                "quiesce:ui",
                "dispose:ui",
                "quiesce:service",
                "quiesce:database",
                "dispose:database",
            ]
        );
        assert_eq!(executor.graph().state("storage"), None);
        assert_eq!(
            executor.graph().state("service"),
            Some(ComponentState::Failed)
        );
        assert_eq!(
            executor.graph().state("database"),
            Some(ComponentState::Pending)
        );
        let service = executor
            .graph()
            .components()
            .into_iter()
            .find(|component| component.id == "service")
            .expect("old service definition should remain visible");
        assert!(service.optional.is_empty());
        assert!(executor.graph().capabilities().is_empty());
        assert!(executor.context_snapshots().is_empty());
        let scopes = executor.scope_snapshots();
        assert_eq!(scopes.len(), 1);
        assert_eq!(scopes[0].owner, "service");
        assert_eq!(scopes[0].lifecycle, EffectScopeLifecycleSnapshot::Active);
        assert_closed_lifecycle_error(&error, "service", "quiescence", &["QUIESCENCE_SECRET"]);

        executor
            .reconcile_definitions(
                storage_definitions(),
                &mut callbacks,
                ComponentLifecycleMode::Reconfigure,
            )
            .expect("transient cleanup failure should converge before replacement");
        assert_eq!(
            callbacks.snapshot(),
            vec![
                "quiesce:ui",
                "dispose:ui",
                "quiesce:service",
                "quiesce:database",
                "dispose:database",
                "quiesce:service",
                "dispose:service",
                "activate:storage",
                "activate:service",
                "activate:ui",
            ]
        );
        assert_eq!(executor.graph().state("database"), None);
        assert_eq!(
            executor.graph().state("storage"),
            Some(ComponentState::Active)
        );
        assert_eq!(
            executor.graph().state("service"),
            Some(ComponentState::Active)
        );
    }

    #[test]
    fn definition_disposal_failure_keeps_old_authority_and_blocks_fresh_activation() {
        let mut executor = ComponentLifecycleExecutor::default();
        let mut callbacks = FakeCallbacks::publishing(&["database", "storage", "service"]);
        callbacks.fail_next_disposal("service");
        executor
            .reconcile_definitions(
                definitions(),
                &mut callbacks,
                ComponentLifecycleMode::Initial,
            )
            .expect("initial composition should activate");
        callbacks
            .events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();

        let error = executor
            .reconcile_definitions(
                storage_definitions(),
                &mut callbacks,
                ComponentLifecycleMode::Reconfigure,
            )
            .expect_err("old disposal failure must abort definition commit");

        assert_eq!(
            callbacks.snapshot(),
            vec![
                "quiesce:ui",
                "dispose:ui",
                "quiesce:service",
                "dispose:service",
                "quiesce:database",
                "dispose:database",
            ]
        );
        assert_eq!(executor.graph().state("storage"), None);
        assert_eq!(
            executor.graph().state("service"),
            Some(ComponentState::Failed)
        );
        assert_eq!(
            executor.graph().state("database"),
            Some(ComponentState::Pending)
        );
        let service = executor
            .graph()
            .components()
            .into_iter()
            .find(|component| component.id == "service")
            .expect("old service definition should remain visible");
        assert!(service.optional.is_empty());
        assert!(executor.graph().capabilities().is_empty());
        assert!(executor.context_snapshots().is_empty());
        let scopes = executor.scope_snapshots();
        assert_eq!(scopes.len(), 1);
        assert_eq!(scopes[0].owner, "service");
        assert_eq!(
            scopes[0].lifecycle,
            EffectScopeLifecycleSnapshot::Finalizing
        );
        assert_closed_lifecycle_error(&error, "service", "effect_disposal", &["DISPOSER_SECRET"]);

        executor
            .reconcile_definitions(
                storage_definitions(),
                &mut callbacks,
                ComponentLifecycleMode::Reconfigure,
            )
            .expect("transient disposer failure should converge before replacement");
        assert_eq!(
            executor.graph().state("storage"),
            Some(ComponentState::Active)
        );
        assert_eq!(
            executor.graph().state("service"),
            Some(ComponentState::Active)
        );
    }

    #[test]
    fn invalid_definition_preflight_does_not_retry_pending_cleanup() {
        let mut executor = ComponentLifecycleExecutor::default();
        let mut callbacks = FakeCallbacks::publishing(&["database", "storage", "service"]);
        callbacks.fail_next_disposal("service");
        executor
            .reconcile_definitions(
                definitions(),
                &mut callbacks,
                ComponentLifecycleMode::Initial,
            )
            .expect("initial composition should activate");
        executor
            .reconcile_definitions(
                storage_definitions(),
                &mut callbacks,
                ComponentLifecycleMode::Reconfigure,
            )
            .expect_err("transient disposal failure should remain pending");
        callbacks
            .events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
        let components_before = executor.graph().components();
        let capabilities_before = executor.graph().capabilities();
        let context_before = executor.context_snapshots();
        let scopes_before = executor.scope_snapshots();

        let error = executor
            .reconcile_definitions(
                [
                    ComponentDefinition::new("provider-a").provides("shared"),
                    ComponentDefinition::new("provider-b").provides("shared"),
                ],
                &mut callbacks,
                ComponentLifecycleMode::Reconfigure,
            )
            .expect_err("invalid desired definitions must fail before cleanup retry");

        assert_closed_lifecycle_error(&error, "runtime_composition", "graph", &["shared"]);
        assert!(callbacks.snapshot().is_empty());
        assert_eq!(executor.graph().components(), components_before);
        assert_eq!(executor.graph().capabilities(), capabilities_before);
        assert_eq!(executor.context_snapshots(), context_before);
        assert_eq!(executor.scope_snapshots(), scopes_before);

        executor
            .reconcile_definitions(
                storage_definitions(),
                &mut callbacks,
                ComponentLifecycleMode::Reconfigure,
            )
            .expect("valid desired definitions should retry and converge cleanup");
    }

    #[test]
    fn fresh_definition_activation_failure_never_restores_old_definition() {
        let mut executor = ComponentLifecycleExecutor::default();
        let mut callbacks = FakeCallbacks::publishing(&["database", "storage", "service"]);
        executor
            .reconcile_definitions(
                definitions(),
                &mut callbacks,
                ComponentLifecycleMode::Initial,
            )
            .expect("initial composition should activate");
        callbacks
            .events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
        callbacks.activation_failures.insert("storage".to_string());

        executor
            .reconcile_definitions(
                storage_definitions(),
                &mut callbacks,
                ComponentLifecycleMode::Reconfigure,
            )
            .expect_err("fresh storage activation should fail");

        assert_eq!(
            callbacks.snapshot(),
            vec![
                "quiesce:ui",
                "dispose:ui",
                "quiesce:service",
                "dispose:service",
                "quiesce:database",
                "dispose:database",
                "activate:storage",
                "quiesce:storage",
                "dispose:storage",
            ]
        );
        assert_eq!(executor.graph().state("database"), None);
        assert_eq!(
            executor.graph().state("storage"),
            Some(ComponentState::Failed)
        );
        assert_eq!(
            executor.graph().state("service"),
            Some(ComponentState::Pending)
        );
        assert_eq!(executor.graph().state("ui"), Some(ComponentState::Pending));
        let service = executor
            .graph()
            .components()
            .into_iter()
            .find(|component| component.id == "service")
            .expect("desired service definition should be committed");
        assert_eq!(
            service
                .optional
                .into_iter()
                .map(|optional| optional.key)
                .collect::<Vec<_>>(),
            vec!["metrics"]
        );
        assert!(executor.graph().capabilities().is_empty());
        assert!(executor.context_snapshots().is_empty());
        assert!(executor.scope_snapshots().is_empty());
    }

    #[test]
    fn invalid_definition_batch_is_rejected_before_concrete_cleanup() {
        let mut executor = ComponentLifecycleExecutor::default();
        let mut callbacks = FakeCallbacks::publishing(&["database", "service"]);
        executor
            .reconcile_definitions(
                definitions(),
                &mut callbacks,
                ComponentLifecycleMode::Initial,
            )
            .expect("initial composition should activate");
        callbacks
            .events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
        let components_before = executor.graph().components();
        let capabilities_before = executor.graph().capabilities();
        let context_before = executor.context_snapshots();
        let scopes_before = executor.scope_snapshots();

        executor
            .reconcile_definitions(
                [
                    ComponentDefinition::new("provider-a").provides("shared"),
                    ComponentDefinition::new("provider-b").provides("shared"),
                ],
                &mut callbacks,
                ComponentLifecycleMode::Reconfigure,
            )
            .expect_err("duplicate provider must fail during preflight");

        assert!(callbacks.snapshot().is_empty());
        assert_eq!(executor.graph().components(), components_before);
        assert_eq!(executor.graph().capabilities(), capabilities_before);
        assert_eq!(executor.context_snapshots(), context_before);
        assert_eq!(executor.scope_snapshots(), scopes_before);

        executor.inject_epoch_exhaustion("service");
        let components_before = executor.graph().components();
        let capabilities_before = executor.graph().capabilities();
        let context_before = executor.context_snapshots();
        let scopes_before = executor.scope_snapshots();
        let error = executor
            .reconcile_definitions(
                storage_definitions(),
                &mut callbacks,
                ComponentLifecycleMode::Reconfigure,
            )
            .expect_err("activation epoch exhaustion must fail during preflight");
        assert_closed_lifecycle_error(&error, "runtime_composition", "graph", &["service"]);
        assert!(callbacks.snapshot().is_empty());
        assert_eq!(executor.graph().components(), components_before);
        assert_eq!(executor.graph().capabilities(), capabilities_before);
        assert_eq!(executor.context_snapshots(), context_before);
        assert_eq!(executor.scope_snapshots(), scopes_before);
    }

    #[test]
    fn newly_ready_activation_preempts_lower_priority_root() {
        let definitions = [
            ComponentDefinition::new("a-provider").provides("provider-ready"),
            ComponentDefinition::new("b-consumer").requires("provider-ready"),
            ComponentDefinition::new("z-independent"),
        ];
        let mut executor = ComponentLifecycleExecutor::default();
        let mut callbacks = FakeCallbacks::publishing(&["a-provider"]);

        executor
            .reconcile_definitions(definitions, &mut callbacks, ComponentLifecycleMode::Initial)
            .expect("branched composition should activate");

        assert_eq!(
            executor.graph().activation_order(),
            vec!["a-provider", "b-consumer", "z-independent"]
        );
        assert_eq!(
            callbacks.snapshot(),
            vec![
                "activate:a-provider",
                "activate:b-consumer",
                "activate:z-independent",
            ]
        );
    }

    #[test]
    fn reconfiguration_preflight_covers_the_required_activation_closure() {
        let mut executor = ComponentLifecycleExecutor::default();
        let mut callbacks = FakeCallbacks::publishing(&["database", "service"]);
        executor
            .reconcile_definitions(
                [
                    ComponentDefinition::new("database").provides("database"),
                    ComponentDefinition::new("service")
                        .requires("database")
                        .provides("service"),
                    ComponentDefinition::new("leaf").requires("service"),
                ],
                &mut callbacks,
                ComponentLifecycleMode::Initial,
            )
            .expect("composition should activate");
        executor.inject_epoch_exhaustion("leaf");

        let error = executor
            .validate_reconfiguration(
                [("database", CapabilityKey::from("database"))],
                ["database"],
            )
            .expect_err("downstream epoch exhaustion should reject before deactivation");

        assert_closed_lifecycle_error(&error, "leaf", "graph", &["epoch is exhausted"]);
        assert_eq!(
            executor.graph().state("database"),
            Some(ComponentState::Active)
        );
        assert_eq!(
            executor.graph().state("service"),
            Some(ComponentState::Active)
        );
        assert_eq!(executor.graph().state("leaf"), Some(ComponentState::Active));
        assert!(
            executor
                .graph()
                .has_capability(&CapabilityKey::from("database"))
        );
        assert!(
            executor
                .graph()
                .has_capability(&CapabilityKey::from("service"))
        );
    }

    #[test]
    fn duplicate_action_for_the_same_epoch_executes_once() {
        let mut executor = ComponentLifecycleExecutor::default();
        let mut callbacks = FakeCallbacks::publishing(&["database", "service"]);
        executor
            .reconcile_definitions(
                definitions(),
                &mut callbacks,
                ComponentLifecycleMode::Initial,
            )
            .expect("composition should activate");
        callbacks
            .events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();

        let mut report = executor
            .graph
            .remove_capability("database", &CapabilityKey::from("database"))
            .expect("required capability should request deactivation");
        report
            .deactivation_requests
            .push(report.deactivation_requests[0].clone());
        executor
            .execute_report(report, &mut callbacks, ComponentLifecycleMode::Reconfigure)
            .expect("duplicate token should be ignored");

        assert_eq!(
            callbacks
                .snapshot()
                .into_iter()
                .filter(|event| event == "quiesce:service")
                .count(),
            1
        );
    }

    #[test]
    fn deactivation_dedup_ignores_disposition_for_the_same_epoch() {
        let mut executor = ComponentLifecycleExecutor::default();
        let mut callbacks = FakeCallbacks::publishing(&["database"]);
        executor
            .reconcile_definitions(
                [
                    ComponentDefinition::new("database").provides("database"),
                    ComponentDefinition::new("service").requires("database"),
                ],
                &mut callbacks,
                ComponentLifecycleMode::Initial,
            )
            .expect("composition should activate");
        callbacks
            .events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();

        let mut report = executor
            .graph
            .remove_capability("database", &CapabilityKey::from("database"))
            .expect("required capability should request deactivation");
        report.deactivation_requests.push(
            report.deactivation_requests[0]
                .clone()
                .with_dispose_disposition(),
        );
        executor
            .execute_report(report, &mut callbacks, ComponentLifecycleMode::Reconfigure)
            .expect("same epoch should execute once regardless of disposition");

        assert_eq!(
            callbacks
                .snapshot()
                .into_iter()
                .filter(|event| event == "quiesce:service")
                .count(),
            1
        );
    }

    #[test]
    fn multi_capability_cascade_uses_one_global_deactivation_order() {
        let mut executor = ComponentLifecycleExecutor::default();
        let mut callbacks = FakeCallbacks::publishing(&["source", "left-branch", "right-branch"]);
        executor
            .reconcile_definitions(
                [
                    ComponentDefinition::new("source")
                        .provides("left-input")
                        .provides("right-input"),
                    ComponentDefinition::new("left-branch")
                        .requires("left-input")
                        .provides("left-ready"),
                    ComponentDefinition::new("right-branch")
                        .requires("right-input")
                        .provides("right-ready"),
                    ComponentDefinition::new("leaf")
                        .requires("left-ready")
                        .requires("right-ready"),
                ],
                &mut callbacks,
                ComponentLifecycleMode::Initial,
            )
            .expect("diamond composition should activate");
        callbacks
            .events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();

        executor
            .deactivate_components(
                ["source"],
                &mut callbacks,
                ComponentLifecycleMode::Reconfigure,
            )
            .expect("diamond composition should deactivate");

        assert_eq!(
            callbacks.snapshot(),
            vec![
                "quiesce:leaf",
                "dispose:leaf",
                "quiesce:right-branch",
                "dispose:right-branch",
                "quiesce:left-branch",
                "dispose:left-branch",
                "quiesce:source",
                "dispose:source",
            ]
        );
    }

    #[test]
    fn shutdown_does_not_reactivate_a_disposed_observer() {
        let mut executor = ComponentLifecycleExecutor::default();
        let mut callbacks = FakeCallbacks::publishing(&["a-provider"]);
        executor
            .reconcile_definitions(
                [
                    ComponentDefinition::new("a-provider").provides("optional-input"),
                    ComponentDefinition::new("z-observer").observes("optional-input"),
                ],
                &mut callbacks,
                ComponentLifecycleMode::Initial,
            )
            .expect("observed composition should activate");
        callbacks
            .events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();

        executor
            .shutdown(&mut callbacks)
            .expect("shutdown should dispose the composition");

        assert_eq!(
            callbacks.snapshot(),
            vec![
                "quiesce:z-observer",
                "dispose:z-observer",
                "quiesce:a-provider",
                "dispose:a-provider",
            ]
        );
        assert_eq!(
            executor.graph().state("z-observer"),
            Some(ComponentState::Disposed)
        );
        executor
            .shutdown(&mut callbacks)
            .expect("repeated shutdown should be a no-op");
        assert_eq!(callbacks.snapshot().len(), 4);
    }

    #[test]
    fn activation_failure_rolls_back_scope_and_keeps_capability_absent() {
        let mut executor = ComponentLifecycleExecutor::default();
        let mut callbacks = FakeCallbacks::publishing(&["database"]);
        callbacks.activation_failures.insert("database".to_string());

        let error = executor
            .reconcile_definitions(
                [ComponentDefinition::new("database").provides("database")],
                &mut callbacks,
                ComponentLifecycleMode::Initial,
            )
            .expect_err("injected activation failure should be returned");

        assert_eq!(
            callbacks.snapshot(),
            vec!["activate:database", "quiesce:database", "dispose:database"]
        );
        assert_eq!(
            executor.graph().state("database"),
            Some(ComponentState::Failed)
        );
        assert!(executor.graph().capabilities().is_empty());
        assert!(executor.scope_snapshots().is_empty());
        assert_closed_lifecycle_error(&error, "database", "activation", &["ACTIVATION_SECRET"]);
    }

    #[test]
    fn definition_preflight_rejects_generation_exhaustion_before_callback() {
        let mut executor = ComponentLifecycleExecutor::default();
        executor.inject_generation_exhaustion(CapabilityKey::from("database"));
        let mut callbacks = FakeCallbacks::publishing(&["database"]);

        let error = executor
            .reconcile_definitions(
                [ComponentDefinition::new("database").provides("database")],
                &mut callbacks,
                ComponentLifecycleMode::Initial,
            )
            .expect_err("generation exhaustion should reject publication");

        assert!(callbacks.snapshot().is_empty());
        assert_eq!(executor.graph().state("database"), None);
        assert!(executor.graph().capabilities().is_empty());
        assert!(executor.scope_snapshots().is_empty());
        assert_closed_lifecycle_error(
            &error,
            "runtime_composition",
            "graph",
            &["generation is exhausted"],
        );
    }

    #[test]
    fn context_hide_rejection_rolls_back_before_provider_teardown() {
        let mut executor = ComponentLifecycleExecutor::default();
        let mut callbacks = FakeCallbacks::publishing(&["database", "service"]);
        executor
            .reconcile_definitions(
                definitions(),
                &mut callbacks,
                ComponentLifecycleMode::Initial,
            )
            .expect("composition should activate");
        callbacks
            .events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
        let capabilities_before = executor.graph().capabilities();
        let context_before = executor.context_snapshots();
        let scopes_before = executor.scope_snapshots();
        executor.context.reject_next_hide();

        let error = executor
            .deactivate_components(
                ["database"],
                &mut callbacks,
                ComponentLifecycleMode::Reconfigure,
            )
            .expect_err("context rejection should abort deactivation");

        assert!(callbacks.snapshot().is_empty());
        assert_eq!(
            executor.graph().state("database"),
            Some(ComponentState::Active)
        );
        assert_eq!(
            executor.graph().state("service"),
            Some(ComponentState::Active)
        );
        assert_eq!(executor.graph().state("ui"), Some(ComponentState::Active));
        assert_eq!(executor.graph().capabilities(), capabilities_before);
        assert_eq!(executor.context_snapshots(), context_before);
        assert_eq!(executor.scope_snapshots(), scopes_before);
        assert_closed_lifecycle_error(&error, "database", "graph", &["visibility does not match"]);
    }

    #[test]
    fn shutdown_context_rejection_preserves_a_consistent_terminal_state() {
        let mut executor = ComponentLifecycleExecutor::default();
        let mut callbacks = FakeCallbacks::publishing(&["database", "service"]);
        executor
            .reconcile_definitions(
                definitions(),
                &mut callbacks,
                ComponentLifecycleMode::Initial,
            )
            .expect("composition should activate");
        callbacks
            .events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
        executor.context.reject_next_hide();

        let error = executor
            .shutdown(&mut callbacks)
            .expect_err("shutdown should report context rejection");

        let graph = executor
            .graph()
            .capabilities()
            .into_iter()
            .map(|snapshot| {
                (
                    snapshot.key,
                    snapshot.provider_component,
                    snapshot.generation,
                )
            })
            .collect::<Vec<_>>();
        let context = executor
            .context_snapshots()
            .into_iter()
            .map(|snapshot| {
                (
                    snapshot.key,
                    snapshot.provider_component,
                    snapshot.generation,
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(graph, context);
        assert_closed_lifecycle_error(&error, "ui", "graph", &["visibility does not match"]);
    }

    #[test]
    fn shutdown_retries_transient_cleanup_before_succeeding() {
        let mut executor = ComponentLifecycleExecutor::default();
        let mut callbacks = FakeCallbacks::publishing(&["database"]);
        callbacks.quiescence_failures.insert("service".to_string());
        executor
            .reconcile_definitions(
                [
                    ComponentDefinition::new("database").provides("database"),
                    ComponentDefinition::new("service").requires("database"),
                ],
                &mut callbacks,
                ComponentLifecycleMode::Initial,
            )
            .expect("composition should activate");

        executor
            .shutdown(&mut callbacks)
            .expect_err("quiescence failure should be returned");

        assert_eq!(
            executor.graph().state("service"),
            Some(ComponentState::Failed)
        );
        assert_eq!(executor.graph().failures().len(), 1);
        executor
            .shutdown(&mut callbacks)
            .expect("repeated shutdown should retry pending cleanup");
        assert_eq!(
            executor.graph().state("service"),
            Some(ComponentState::Disposed)
        );
        assert!(executor.graph().failures().is_empty());
        assert!(executor.scope_snapshots().is_empty());
    }

    #[test]
    fn activation_rollback_retains_failed_disposer_until_retry() {
        let mut executor = ComponentLifecycleExecutor::default();
        let mut callbacks = FakeCallbacks::default();
        callbacks.activation_failures.insert("service".to_string());
        callbacks.fail_next_disposal("service");

        executor
            .reconcile_definitions(
                [ComponentDefinition::new("service")],
                &mut callbacks,
                ComponentLifecycleMode::Initial,
            )
            .expect_err("activation rollback cleanup should remain pending");

        assert_eq!(
            executor.graph().state("service"),
            Some(ComponentState::Failed)
        );
        let scopes = executor.scope_snapshots();
        assert_eq!(scopes.len(), 1);
        assert_eq!(scopes[0].owner, "service");
        assert_eq!(
            scopes[0].lifecycle,
            EffectScopeLifecycleSnapshot::Finalizing
        );
        callbacks.activation_failures.remove("service");

        executor
            .activate_components(
                ["service"],
                &mut callbacks,
                ComponentLifecycleMode::Reconfigure,
            )
            .expect("pending rollback cleanup should finish before fresh activation");
        assert_eq!(
            executor.graph().state("service"),
            Some(ComponentState::Active)
        );
        assert_eq!(
            callbacks.snapshot(),
            vec![
                "activate:service",
                "quiesce:service",
                "dispose:service",
                "dispose:service",
                "activate:service",
            ]
        );
        assert_eq!(executor.scope_snapshots().len(), 1);
        assert_eq!(
            executor.scope_snapshots()[0].lifecycle,
            EffectScopeLifecycleSnapshot::Active
        );
    }

    #[test]
    fn shutdown_does_not_bypass_pending_activation_quiescence() {
        let mut executor = ComponentLifecycleExecutor::default();
        let mut callbacks = FakeCallbacks::default();
        callbacks.activation_failures.insert("service".to_string());
        callbacks.quiescence_failures.insert("service".to_string());

        executor
            .reconcile_definitions(
                [ComponentDefinition::new("service")],
                &mut callbacks,
                ComponentLifecycleMode::Initial,
            )
            .expect_err("activation rollback quiescence should remain pending");
        callbacks.quiescence_failures.insert("service".to_string());

        executor
            .shutdown(&mut callbacks)
            .expect_err("shutdown must retain a still-unquiesced activation scope");
        assert_eq!(
            executor.graph().state("service"),
            Some(ComponentState::Failed)
        );
        assert_eq!(
            executor.graph().failures()[0].operation,
            ComponentFailureOperation::Activation
        );
        let scopes = executor.scope_snapshots();
        assert_eq!(scopes.len(), 1);
        assert_eq!(scopes[0].owner, "service");
        assert_eq!(scopes[0].lifecycle, EffectScopeLifecycleSnapshot::Active);

        executor
            .shutdown(&mut callbacks)
            .expect("shutdown should converge after quiescence recovers");
        assert_eq!(
            executor.graph().state("service"),
            Some(ComponentState::Disposed)
        );
        assert!(executor.scope_snapshots().is_empty());
    }

    #[test]
    fn cleanup_failures_do_not_skip_siblings_and_inspection_is_redacted() {
        let mut executor = ComponentLifecycleExecutor::default();
        let mut callbacks = FakeCallbacks::publishing(&["database"]);
        callbacks.fail_next_disposal("a-consumer");
        executor
            .reconcile_definitions(
                [
                    ComponentDefinition::new("database").provides("database"),
                    ComponentDefinition::new("a-consumer").requires("database"),
                    ComponentDefinition::new("z-consumer").requires("database"),
                ],
                &mut callbacks,
                ComponentLifecycleMode::Initial,
            )
            .expect("composition should activate");
        callbacks
            .events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
        callbacks
            .quiescence_failures
            .insert("z-consumer".to_string());

        let error = executor
            .remove_capability(
                "database",
                &CapabilityKey::from("database"),
                &mut callbacks,
                ComponentLifecycleMode::Reconfigure,
            )
            .expect_err("injected cleanup failures should be aggregated");

        assert_eq!(
            callbacks.snapshot(),
            vec![
                "quiesce:z-consumer",
                "quiesce:a-consumer",
                "dispose:a-consumer",
            ]
        );
        assert_eq!(
            executor.graph().state("z-consumer"),
            Some(ComponentState::Failed)
        );
        assert_eq!(
            executor.graph().state("a-consumer"),
            Some(ComponentState::Failed)
        );
        let scopes = executor
            .scope_snapshots()
            .into_iter()
            .filter(|scope| scope.owner != "database")
            .collect::<Vec<_>>();
        assert_eq!(scopes.len(), 2);
        assert_eq!(
            scopes
                .iter()
                .map(|scope| (scope.owner.as_str(), scope.lifecycle))
                .collect::<Vec<_>>(),
            vec![
                ("a-consumer", EffectScopeLifecycleSnapshot::Finalizing),
                ("z-consumer", EffectScopeLifecycleSnapshot::Active),
            ]
        );
        assert_closed_lifecycle_error(
            &error,
            "a-consumer",
            "effect_disposal",
            &["QUIESCENCE_SECRET", "DISPOSER_SECRET"],
        );
        assert!(
            error
                .to_string()
                .contains("component z-consumer quiescence failed")
        );
        let failures = executor.graph().failures();
        assert_eq!(failures.len(), 2);
        assert!(failures.iter().all(|failure| {
            matches!(
                failure.reason,
                ComponentFailureReason::QuiescenceRejected
                    | ComponentFailureReason::EffectDisposalRejected
            )
        }));

        executor
            .deactivate_components(
                ["a-consumer", "z-consumer"],
                &mut callbacks,
                ComponentLifecycleMode::Reconfigure,
            )
            .expect("transient sibling cleanup failures should both converge");
        let scopes = executor.scope_snapshots();
        assert_eq!(scopes.len(), 1);
        assert_eq!(scopes[0].owner, "database");
        assert_eq!(scopes[0].lifecycle, EffectScopeLifecycleSnapshot::Active);
    }
}
