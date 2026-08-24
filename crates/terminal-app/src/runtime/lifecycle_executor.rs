use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    error::Error,
    fmt,
    ops::Deref,
};

use super::{
    effect_scope::{EffectScope, EffectScopeSnapshot},
    lifecycle::{
        ActivationToken, CapabilityKey, ComponentDefinition, ComponentFailureReason,
        ComponentGraph, ComponentGraphError, DeactivationToken, ReconciliationReport,
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
        mode: ComponentLifecycleMode,
    ) -> Result<ComponentActivationOutcome, String>;

    fn quiesce_component(
        &mut self,
        component_id: &str,
        mode: ComponentLifecycleMode,
    ) -> Result<(), String>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LifecycleExecutionOperation {
    Graph,
    Activation,
    Quiescence,
    EffectDisposal,
}

impl LifecycleExecutionOperation {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Graph => "graph",
            Self::Activation => "activation",
            Self::Quiescence => "quiescence",
            Self::EffectDisposal => "effect_disposal",
        }
    }
}

struct LifecycleExecutionFailure {
    component_id: String,
    operation: LifecycleExecutionOperation,
    message: String,
}

/// `LifecycleExecutionError` 保留 operational source text，但 `Debug` 只投影安全 metadata。
pub(super) struct LifecycleExecutionError {
    failures: Vec<LifecycleExecutionFailure>,
}

impl LifecycleExecutionError {
    fn graph(component_id: impl Into<String>, error: ComponentGraphError) -> Self {
        Self {
            failures: vec![LifecycleExecutionFailure {
                component_id: component_id.into(),
                operation: LifecycleExecutionOperation::Graph,
                message: error.to_string(),
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
                message: "component lifecycle executor is shut down".to_string(),
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
                "component {} {} failed: {}",
                failure.component_id,
                failure.operation.as_str(),
                failure.message
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
#[derive(Default)]
pub(super) struct ComponentLifecycleExecutor {
    graph: ComponentGraph,
    root_scope: EffectScope,
    component_scopes: BTreeMap<String, EffectScope>,
    is_shutdown: bool,
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

    pub(super) fn declare_all(
        &mut self,
        definitions: impl IntoIterator<Item = ComponentDefinition>,
        callbacks: &mut impl ComponentLifecycleCallbacks,
    ) -> Result<(), LifecycleExecutionError> {
        self.ensure_running()?;
        let mut reports = Vec::new();
        for definition in definitions {
            let component_id = definition.id.clone();
            let declaration = self
                .graph
                .declare(definition)
                .map_err(|error| LifecycleExecutionError::graph(component_id, error))?;
            reports.push(declaration);
        }
        self.execute_reports(reports, callbacks, ComponentLifecycleMode::Initial)
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
        let report = self
            .graph
            .remove_capability(provider_component, capability)
            .map_err(|error| LifecycleExecutionError::graph(provider_component, error))?;
        self.execute_report(report, callbacks, mode)
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

        for component_id in order {
            match self.graph.activate(&component_id) {
                Ok(report) => {
                    if let Err(error) = self.execute_report(report, callbacks, mode) {
                        failures.extend(error.failures);
                        continue;
                    }
                }
                Err(error) => {
                    failures.extend(
                        LifecycleExecutionError::graph(component_id.clone(), error).failures,
                    );
                    continue;
                }
            }
        }

        LifecycleExecutionError::finish(failures)
    }

    pub(super) fn shutdown(
        &mut self,
        callbacks: &mut impl ComponentLifecycleCallbacks,
    ) -> Result<(), LifecycleExecutionError> {
        if self.is_shutdown {
            return Ok(());
        }
        self.is_shutdown = true;
        let component_ids = self.graph.deactivation_order();
        let mut failures = self
            .deactivate_components_inner(component_ids, callbacks, ComponentLifecycleMode::Shutdown)
            .err()
            .map(|error| error.failures)
            .unwrap_or_default();
        if let Some(message) = self.root_scope.dispose().error_message() {
            failures.push(LifecycleExecutionFailure {
                component_id: "runtime_composition".to_string(),
                operation: LifecycleExecutionOperation::EffectDisposal,
                message,
            });
        }
        LifecycleExecutionError::finish(failures)
    }

    fn ensure_running(&self) -> Result<(), LifecycleExecutionError> {
        if self.is_shutdown {
            Err(LifecycleExecutionError::executor_shutdown())
        } else {
            Ok(())
        }
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
            Err(error) => {
                state.failures.push(LifecycleExecutionFailure {
                    component_id: component_id.clone(),
                    operation: LifecycleExecutionOperation::Activation,
                    message: error.to_string(),
                });
                if let Ok(report) = self.graph.fail_activation(
                    token,
                    ComponentFailureReason::ActivationRejected,
                    true,
                ) {
                    pending.push_back(report);
                }
                return;
            }
        };

        let outcome = match callbacks.activate_component(&component_id, &scope, mode) {
            Ok(outcome) => outcome,
            Err(message) => {
                state.failures.push(LifecycleExecutionFailure {
                    component_id: component_id.clone(),
                    operation: LifecycleExecutionOperation::Activation,
                    message,
                });
                if let Err(message) = callbacks.quiesce_component(&component_id, mode) {
                    state.failures.push(LifecycleExecutionFailure {
                        component_id: component_id.clone(),
                        operation: LifecycleExecutionOperation::Quiescence,
                        message,
                    });
                }
                if let Some(message) = scope.dispose().error_message() {
                    state.failures.push(LifecycleExecutionFailure {
                        component_id: component_id.clone(),
                        operation: LifecycleExecutionOperation::EffectDisposal,
                        message,
                    });
                }
                if let Ok(report) = self.graph.fail_activation(
                    token,
                    ComponentFailureReason::ActivationRejected,
                    true,
                ) {
                    pending.push_back(report);
                }
                return;
            }
        };

        let completion = match outcome {
            ComponentActivationOutcome::Ready => self
                .graph
                .complete_activation(token.clone())
                .map(|report| vec![report]),
            ComponentActivationOutcome::PublishCapabilities => {
                self.graph.complete_activation_and_publish(token.clone())
            }
        };
        match completion {
            Ok(reports) => {
                self.component_scopes.insert(component_id.clone(), scope);
                pending.extend(reports);
            }
            Err(error) => {
                state.failures.push(LifecycleExecutionFailure {
                    component_id: component_id.clone(),
                    operation: LifecycleExecutionOperation::Graph,
                    message: error.to_string(),
                });
                if let Err(message) = callbacks.quiesce_component(&component_id, mode) {
                    state.failures.push(LifecycleExecutionFailure {
                        component_id: component_id.clone(),
                        operation: LifecycleExecutionOperation::Quiescence,
                        message,
                    });
                }
                if let Some(message) = scope.dispose().error_message() {
                    state.failures.push(LifecycleExecutionFailure {
                        component_id: component_id.clone(),
                        operation: LifecycleExecutionOperation::EffectDisposal,
                        message,
                    });
                }
                match self.graph.fail_activation(
                    token,
                    ComponentFailureReason::ActivationRejected,
                    true,
                ) {
                    Ok(report) => pending.push_back(report),
                    Err(error) => state
                        .failures
                        .extend(LifecycleExecutionError::graph(component_id, error).failures),
                }
            }
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
        match self.graph.provided_capabilities(&component_id) {
            Ok(capabilities) => {
                for capability in capabilities {
                    if !self.graph.has_capability(&capability) {
                        continue;
                    }
                    match self.graph.remove_capability(&component_id, &capability) {
                        Ok(report) => dependent_reports.push_back(report),
                        Err(error) => state.failures.extend(
                            LifecycleExecutionError::graph(component_id.clone(), error).failures,
                        ),
                    }
                }
            }
            Err(error) => state
                .failures
                .extend(LifecycleExecutionError::graph(component_id.clone(), error).failures),
        }
        self.execute_reports_inner(dependent_reports, callbacks, mode, state);
        let quiescence_error = callbacks
            .quiesce_component(&component_id, mode)
            .err()
            .inspect(|message| {
                state.failures.push(LifecycleExecutionFailure {
                    component_id: component_id.clone(),
                    operation: LifecycleExecutionOperation::Quiescence,
                    message: message.clone(),
                });
            });
        let disposal_error = self
            .component_scopes
            .remove(&component_id)
            .and_then(|scope| scope.dispose().error_message())
            .inspect(|message| {
                state.failures.push(LifecycleExecutionFailure {
                    component_id: component_id.clone(),
                    operation: LifecycleExecutionOperation::EffectDisposal,
                    message: message.clone(),
                });
            });

        let result = if quiescence_error.is_some() {
            self.graph
                .fail_deactivation(token, ComponentFailureReason::QuiescenceRejected, false)
        } else if disposal_error.is_some() {
            self.graph.fail_deactivation(
                token,
                ComponentFailureReason::EffectDisposalRejected,
                false,
            )
        } else {
            self.graph.complete_deactivation(token)
        };
        match result {
            Ok(report) => pending.push_back(report),
            Err(error) => state.failures.push(LifecycleExecutionFailure {
                component_id,
                operation: LifecycleExecutionOperation::Graph,
                message: error.to_string(),
            }),
        }
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

    use super::*;

    #[derive(Default)]
    struct FakeCallbacks {
        events: Arc<Mutex<Vec<String>>>,
        publishers: BTreeSet<String>,
        activation_failures: BTreeSet<String>,
        quiescence_failures: BTreeSet<String>,
        disposal_failures: BTreeSet<String>,
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
    }

    impl ComponentLifecycleCallbacks for FakeCallbacks {
        fn activate_component(
            &mut self,
            component_id: &str,
            scope: &EffectScope,
            _mode: ComponentLifecycleMode,
        ) -> Result<ComponentActivationOutcome, String> {
            self.record(format!("activate:{component_id}"));
            let events = Arc::clone(&self.events);
            let disposed_component = component_id.to_string();
            let should_fail_disposal = self.disposal_failures.contains(component_id);
            scope
                .register("owned_effect", move || {
                    events
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .push(format!("dispose:{disposed_component}"));
                    if should_fail_disposal {
                        Err("DISPOSER_SECRET".to_string())
                    } else {
                        Ok(())
                    }
                })
                .map_err(|error| error.to_string())?;
            if self.activation_failures.contains(component_id) {
                return Err("ACTIVATION_SECRET".to_string());
            }
            Ok(if self.publishers.contains(component_id) {
                ComponentActivationOutcome::PublishCapabilities
            } else {
                ComponentActivationOutcome::Ready
            })
        }

        fn quiesce_component(
            &mut self,
            component_id: &str,
            _mode: ComponentLifecycleMode,
        ) -> Result<(), String> {
            self.record(format!("quiesce:{component_id}"));
            if self.quiescence_failures.contains(component_id) {
                Err("QUIESCENCE_SECRET".to_string())
            } else {
                Ok(())
            }
        }
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

    #[test]
    fn activation_and_recursive_deactivation_follow_topology() {
        let mut executor = ComponentLifecycleExecutor::default();
        let mut callbacks = FakeCallbacks::publishing(&["database", "service"]);

        executor
            .declare_all(definitions(), &mut callbacks)
            .expect("acyclic composition should activate");
        assert_eq!(
            callbacks.snapshot(),
            vec!["activate:database", "activate:service", "activate:ui"]
        );
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
    fn declaration_order_does_not_change_physical_activation_order() {
        let mut forward = ComponentLifecycleExecutor::default();
        let mut forward_callbacks = FakeCallbacks::publishing(&["database", "service"]);
        forward
            .declare_all(definitions(), &mut forward_callbacks)
            .expect("forward graph should activate");

        let mut reverse = ComponentLifecycleExecutor::default();
        let mut reverse_callbacks = FakeCallbacks::publishing(&["database", "service"]);
        reverse
            .declare_all(definitions().into_iter().rev(), &mut reverse_callbacks)
            .expect("reverse graph should activate");

        assert_eq!(forward_callbacks.snapshot(), reverse_callbacks.snapshot());
        assert_eq!(
            reverse_callbacks.snapshot(),
            vec!["activate:database", "activate:service", "activate:ui"]
        );
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
            .declare_all(definitions, &mut callbacks)
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
            .declare_all(
                [
                    ComponentDefinition::new("database").provides("database"),
                    ComponentDefinition::new("service")
                        .requires("database")
                        .provides("service"),
                    ComponentDefinition::new("leaf").requires("service"),
                ],
                &mut callbacks,
            )
            .expect("composition should activate");
        executor.inject_epoch_exhaustion("leaf");

        let error = executor
            .validate_reconfiguration(
                [("database", CapabilityKey::from("database"))],
                ["database"],
            )
            .expect_err("downstream epoch exhaustion should reject before deactivation");

        assert!(
            error
                .to_string()
                .contains("component `leaf` activation epoch is exhausted")
        );
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
            .declare_all(definitions(), &mut callbacks)
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
            .declare_all(
                [
                    ComponentDefinition::new("database").provides("database"),
                    ComponentDefinition::new("service").requires("database"),
                ],
                &mut callbacks,
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
            .declare_all(
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
            .declare_all(
                [
                    ComponentDefinition::new("a-provider").provides("optional-input"),
                    ComponentDefinition::new("z-observer").observes("optional-input"),
                ],
                &mut callbacks,
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
            .declare_all(
                [ComponentDefinition::new("database").provides("database")],
                &mut callbacks,
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
        assert!(error.to_string().contains("ACTIVATION_SECRET"));
        assert!(!format!("{error:?}").contains("ACTIVATION_SECRET"));
    }

    #[test]
    fn publication_failure_quiesces_callback_and_rolls_back_scope_atomically() {
        let mut executor = ComponentLifecycleExecutor::default();
        executor.inject_generation_exhaustion(CapabilityKey::from("database"));
        let mut callbacks = FakeCallbacks::publishing(&["database"]);

        let error = executor
            .declare_all(
                [ComponentDefinition::new("database").provides("database")],
                &mut callbacks,
            )
            .expect_err("generation exhaustion should reject publication");

        assert_eq!(
            callbacks.snapshot(),
            vec!["activate:database", "quiesce:database", "dispose:database",]
        );
        assert_eq!(
            executor.graph().state("database"),
            Some(ComponentState::Failed)
        );
        assert!(executor.graph().capabilities().is_empty());
        assert!(executor.scope_snapshots().is_empty());
        assert!(error.to_string().contains("generation is exhausted"));
        assert!(!format!("{error:?}").contains("generation is exhausted"));
    }

    #[test]
    fn shutdown_cleanup_failure_remains_a_safe_terminal_fact() {
        let mut executor = ComponentLifecycleExecutor::default();
        let mut callbacks = FakeCallbacks::publishing(&["database"]);
        callbacks.quiescence_failures.insert("service".to_string());
        executor
            .declare_all(
                [
                    ComponentDefinition::new("database").provides("database"),
                    ComponentDefinition::new("service").requires("database"),
                ],
                &mut callbacks,
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
            .expect("repeated shutdown should not erase or repeat failure cleanup");
        assert_eq!(
            executor.graph().state("service"),
            Some(ComponentState::Failed)
        );
        assert_eq!(executor.graph().failures().len(), 1);
    }

    #[test]
    fn cleanup_failures_do_not_skip_siblings_and_inspection_is_redacted() {
        let mut executor = ComponentLifecycleExecutor::default();
        let mut callbacks = FakeCallbacks::publishing(&["database"]);
        callbacks.disposal_failures.insert("a-consumer".to_string());
        executor
            .declare_all(
                [
                    ComponentDefinition::new("database").provides("database"),
                    ComponentDefinition::new("a-consumer").requires("database"),
                    ComponentDefinition::new("z-consumer").requires("database"),
                ],
                &mut callbacks,
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
                "dispose:z-consumer",
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
        assert_eq!(executor.scope_snapshots().len(), 1);
        assert!(error.to_string().contains("QUIESCENCE_SECRET"));
        assert!(error.to_string().contains("DISPOSER_SECRET"));
        let diagnostic = format!("{error:?}");
        assert!(!diagnostic.contains("QUIESCENCE_SECRET"));
        assert!(!diagnostic.contains("DISPOSER_SECRET"));
        let failures = executor.graph().failures();
        assert_eq!(failures.len(), 2);
        assert!(failures.iter().all(|failure| {
            matches!(
                failure.reason,
                ComponentFailureReason::QuiescenceRejected
                    | ComponentFailureReason::EffectDisposalRejected
            )
        }));
    }
}
