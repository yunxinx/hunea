use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Mutex},
};

use proptest::{prelude::*, test_runner::TestCaseResult};

use super::*;
use crate::runtime::lifecycle::{
    CapabilitySnapshot, ComponentFailureOperation, ComponentFailureReason,
    ComponentFailureSnapshot, ComponentSnapshot, PendingComponentSnapshot,
};

const ACTIVATION_SENTINEL: &str = "ACTIVATION_SECRET";
const QUIESCENCE_SENTINEL: &str = "QUIESCENCE_SECRET";
const DISPOSER_SENTINEL: &str = "DISPOSER_SECRET";
const AUTHORITY_PREPARATION_SENTINEL: &str = "AUTHORITY_PREPARATION_SECRET";
const RESOURCE_EFFECT: &str = "resource";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum TestComponent {
    AlphaSource,
    BetaSource,
    LeftBranch,
    RightBranch,
    DiamondLeaf,
    Observer,
}

impl TestComponent {
    const ALL: [Self; 6] = [
        Self::AlphaSource,
        Self::BetaSource,
        Self::LeftBranch,
        Self::RightBranch,
        Self::DiamondLeaf,
        Self::Observer,
    ];

    const fn as_str(self) -> &'static str {
        match self {
            Self::AlphaSource => "alpha_source",
            Self::BetaSource => "beta_source",
            Self::LeftBranch => "left_branch",
            Self::RightBranch => "right_branch",
            Self::DiamondLeaf => "diamond_leaf",
            Self::Observer => "observer",
        }
    }

    fn from_id(component_id: &str) -> Self {
        Self::ALL
            .into_iter()
            .find(|component| component.as_str() == component_id)
            .expect("fixed graph callbacks must use closed component ids")
    }

    const fn publishes_capability(self) -> bool {
        matches!(
            self,
            Self::AlphaSource | Self::BetaSource | Self::LeftBranch | Self::RightBranch
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProviderRoot {
    Alpha,
    Beta,
}

impl ProviderRoot {
    const fn component(self) -> TestComponent {
        match self {
            Self::Alpha => TestComponent::AlphaSource,
            Self::Beta => TestComponent::BetaSource,
        }
    }

    const fn capability(self) -> &'static str {
        match self {
            Self::Alpha => "alpha",
            Self::Beta => "beta",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LifecycleAction {
    Reconfigure(ProviderRoot),
    Deactivate(ProviderRoot),
    Activate(ProviderRoot),
    FailPublication(ProviderRoot),
    FailNextActivation(TestComponent),
    FailNextQuiescence(TestComponent),
    FailNextDisposal(TestComponent),
    Shutdown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PostShutdownMutation {
    Declare,
    RemoveCapability,
    Deactivate,
    Activate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ActivationExpectation {
    Root(TestComponent),
    All,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DeactivationExpectation {
    RequiredClosure(TestComponent),
    AllActive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DeactivationFailureKind {
    Quiescence,
    Disposal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AuthorityScenario {
    Success,
    GraphPreflightFailure,
    QuiescenceFailure,
    DisposalFailure,
    PreparationFailure,
    StaleGraphCommit,
    ActivationFailure,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum AuthorityTransactionStatus {
    #[default]
    Idle,
    Prepared,
    Committed,
    Aborted,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct AuthorityState {
    status: AuthorityTransactionStatus,
    has_unpublished_authority: bool,
    preparation_should_fail: bool,
    preparations: usize,
    commits: usize,
    aborts: usize,
    generation: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct ResourceKey {
    component: TestComponent,
    epoch: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ResourceState {
    is_quiesced: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum ResourceEventKind {
    Activate,
    Quiesce,
    Dispose,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ResourceEvent {
    kind: ResourceEventKind,
    key: ResourceKey,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct CallbackState {
    resources: BTreeMap<TestComponent, (ResourceKey, ResourceState)>,
    activation_epochs: BTreeMap<TestComponent, u64>,
    events: Vec<ResourceEvent>,
    disposed_without_quiescence: BTreeSet<ResourceKey>,
    duplicate_activations: BTreeSet<ResourceKey>,
    missing_quiescence: BTreeSet<TestComponent>,
    missing_disposals: BTreeSet<ResourceKey>,
    mismatched_disposals: BTreeSet<ResourceKey>,
    activation_failures: BTreeSet<TestComponent>,
    quiescence_failures: BTreeSet<TestComponent>,
    disposal_failures: BTreeSet<TestComponent>,
    authority: AuthorityState,
}

#[derive(Default)]
struct PropertyCallbacks {
    state: Arc<Mutex<CallbackState>>,
}

impl PropertyCallbacks {
    fn arm_activation_failure(&self, component: TestComponent) {
        lock_callback_state(&self.state)
            .activation_failures
            .insert(component);
    }

    fn arm_quiescence_failure(&self, component: TestComponent) {
        lock_callback_state(&self.state)
            .quiescence_failures
            .insert(component);
    }

    fn arm_disposal_failure(&self, component: TestComponent) {
        lock_callback_state(&self.state)
            .disposal_failures
            .insert(component);
    }

    fn disarm_activation_failure(&self, component: TestComponent) {
        lock_callback_state(&self.state)
            .activation_failures
            .remove(&component);
    }

    fn clear_failure_injections(&self) {
        let mut state = lock_callback_state(&self.state);
        state.activation_failures.clear();
        state.quiescence_failures.clear();
        state.disposal_failures.clear();
    }

    fn begin_authority_transaction(&self) {
        let mut state = lock_callback_state(&self.state);
        assert!(
            !state.authority.has_unpublished_authority,
            "property model cannot overlap authority transactions"
        );
        state.authority.status = AuthorityTransactionStatus::Idle;
        state.authority.has_unpublished_authority = true;
        state.authority.preparation_should_fail = false;
    }

    fn arm_authority_preparation_failure(&self) {
        lock_callback_state(&self.state)
            .authority
            .preparation_should_fail = true;
    }

    fn snapshot(&self) -> CallbackState {
        lock_callback_state(&self.state).clone()
    }
}

impl ComponentLifecycleCallbacks for PropertyCallbacks {
    fn prepare_authority(&mut self) -> Result<(), String> {
        let mut state = lock_callback_state(&self.state);
        assert!(state.authority.has_unpublished_authority);
        assert_eq!(state.authority.status, AuthorityTransactionStatus::Idle);
        state.authority.preparations += 1;
        state.authority.status = AuthorityTransactionStatus::Prepared;
        if state.authority.preparation_should_fail {
            Err(AUTHORITY_PREPARATION_SENTINEL.to_string())
        } else {
            Ok(())
        }
    }

    fn abort_authority(&mut self) {
        let mut state = lock_callback_state(&self.state);
        state.authority.aborts += 1;
        if state.authority.has_unpublished_authority {
            state.authority.has_unpublished_authority = false;
            state.authority.status = AuthorityTransactionStatus::Aborted;
            state.authority.preparation_should_fail = false;
        }
    }

    fn commit_authority(&mut self) {
        let mut state = lock_callback_state(&self.state);
        assert!(state.authority.has_unpublished_authority);
        assert_eq!(state.authority.status, AuthorityTransactionStatus::Prepared);
        state.authority.has_unpublished_authority = false;
        state.authority.status = AuthorityTransactionStatus::Committed;
        state.authority.commits += 1;
        state.authority.generation += 1;
    }

    fn activate_component(
        &mut self,
        component_id: &str,
        scope: &EffectScope,
        context: &mut super::ComponentActivationContext<'_>,
        _mode: ComponentLifecycleMode,
    ) -> Result<ComponentActivationOutcome, String> {
        let component = TestComponent::from_id(component_id);
        let key = {
            let mut state = lock_callback_state(&self.state);
            let epoch = state.activation_epochs.entry(component).or_default();
            *epoch += 1;
            let key = ResourceKey {
                component,
                epoch: *epoch,
            };
            if state.resources.contains_key(&component) {
                state.duplicate_activations.insert(key);
                return Err(ACTIVATION_SENTINEL.to_string());
            }
            state
                .resources
                .insert(component, (key, ResourceState { is_quiesced: false }));
            state.events.push(ResourceEvent {
                kind: ResourceEventKind::Activate,
                key,
            });
            key
        };

        let disposer_state = Arc::clone(&self.state);
        if let Err(error) = scope.register(RESOURCE_EFFECT, move || {
            let mut state = lock_callback_state(&disposer_state);
            let Some((live_key, _)) = state.resources.get(&component) else {
                state.missing_disposals.insert(key);
                return Err(DISPOSER_SENTINEL.to_string());
            };
            if *live_key != key {
                state.mismatched_disposals.insert(key);
                return Err(DISPOSER_SENTINEL.to_string());
            }
            if state.disposal_failures.remove(&component) {
                return Err(DISPOSER_SENTINEL.to_string());
            }
            let (_, resource) = state
                .resources
                .remove(&component)
                .expect("validated resource should remain owned");
            if !resource.is_quiesced {
                state.disposed_without_quiescence.insert(key);
            }
            state.events.push(ResourceEvent {
                kind: ResourceEventKind::Dispose,
                key,
            });
            Ok(())
        }) {
            lock_callback_state(&self.state)
                .resources
                .remove(&component);
            return Err(error.to_string());
        }

        if lock_callback_state(&self.state)
            .activation_failures
            .remove(&component)
        {
            return Err(ACTIVATION_SENTINEL.to_string());
        }
        if component.publishes_capability() {
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
        let component = TestComponent::from_id(component_id);
        let mut state = lock_callback_state(&self.state);
        if state.quiescence_failures.remove(&component) {
            return Err(QUIESCENCE_SENTINEL.to_string());
        }
        let Some((key, resource)) = state.resources.get_mut(&component) else {
            state.missing_quiescence.insert(component);
            return Err(QUIESCENCE_SENTINEL.to_string());
        };
        let key = {
            resource.is_quiesced = true;
            *key
        };
        state.events.push(ResourceEvent {
            kind: ResourceEventKind::Quiesce,
            key,
        });
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct GraphSnapshot {
    capabilities: Vec<CapabilitySnapshot>,
    components: Vec<ComponentSnapshot>,
    pending: Vec<PendingComponentSnapshot>,
    failures: Vec<ComponentFailureSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ExecutorSnapshot {
    graph: GraphSnapshot,
    scopes: Vec<EffectScopeSnapshot>,
}

fn graph_snapshot(graph: &ComponentGraph) -> GraphSnapshot {
    GraphSnapshot {
        capabilities: graph.capabilities(),
        components: graph.components(),
        pending: graph.pending(),
        failures: graph.failures(),
    }
}

fn executor_snapshot(executor: &ComponentLifecycleExecutor) -> ExecutorSnapshot {
    ExecutorSnapshot {
        graph: graph_snapshot(executor.graph()),
        scopes: executor.scope_snapshots(),
    }
}

fn definitions() -> Vec<ComponentDefinition> {
    vec![
        ComponentDefinition::new(TestComponent::AlphaSource.as_str()).provides("alpha"),
        ComponentDefinition::new(TestComponent::BetaSource.as_str()).provides("beta"),
        ComponentDefinition::new(TestComponent::LeftBranch.as_str())
            .requires("alpha")
            .provides("left"),
        ComponentDefinition::new(TestComponent::RightBranch.as_str())
            .requires("alpha")
            .requires("beta")
            .provides("right"),
        ComponentDefinition::new(TestComponent::DiamondLeaf.as_str())
            .requires("left")
            .requires("right"),
        ComponentDefinition::new(TestComponent::Observer.as_str()).observes("alpha"),
    ]
}

fn replacement_definitions_for_generation(generation: usize) -> Vec<ComponentDefinition> {
    definitions()
        .into_iter()
        .map(|definition| {
            if definition.id == TestComponent::AlphaSource.as_str() {
                definition.implemented_by(format!("replacement-alpha-source-{generation}"))
            } else {
                definition
            }
        })
        .collect()
}

fn provider_strategy() -> impl Strategy<Value = ProviderRoot> {
    prop_oneof![Just(ProviderRoot::Alpha), Just(ProviderRoot::Beta)]
}

fn component_strategy() -> impl Strategy<Value = TestComponent> {
    prop_oneof![
        Just(TestComponent::AlphaSource),
        Just(TestComponent::BetaSource),
        Just(TestComponent::LeftBranch),
        Just(TestComponent::RightBranch),
        Just(TestComponent::DiamondLeaf),
        Just(TestComponent::Observer),
    ]
}

fn deactivation_failure_strategy() -> impl Strategy<Value = DeactivationFailureKind> {
    prop_oneof![
        Just(DeactivationFailureKind::Quiescence),
        Just(DeactivationFailureKind::Disposal),
    ]
}

fn authority_scenario_strategy() -> impl Strategy<Value = AuthorityScenario> {
    prop_oneof![
        Just(AuthorityScenario::Success),
        Just(AuthorityScenario::GraphPreflightFailure),
        Just(AuthorityScenario::QuiescenceFailure),
        Just(AuthorityScenario::DisposalFailure),
        Just(AuthorityScenario::PreparationFailure),
        Just(AuthorityScenario::StaleGraphCommit),
        Just(AuthorityScenario::ActivationFailure),
    ]
}

fn lifecycle_action_strategy() -> impl Strategy<Value = LifecycleAction> {
    prop_oneof![
        4 => provider_strategy().prop_map(LifecycleAction::Reconfigure),
        2 => provider_strategy().prop_map(LifecycleAction::Deactivate),
        2 => provider_strategy().prop_map(LifecycleAction::Activate),
        1 => provider_strategy().prop_map(LifecycleAction::FailPublication),
        1 => component_strategy().prop_map(LifecycleAction::FailNextActivation),
        1 => component_strategy().prop_map(LifecycleAction::FailNextQuiescence),
        1 => component_strategy().prop_map(LifecycleAction::FailNextDisposal),
        1 => Just(LifecycleAction::Shutdown),
    ]
}

fn lifecycle_prefix_action_strategy() -> impl Strategy<Value = LifecycleAction> {
    prop_oneof![
        4 => provider_strategy().prop_map(LifecycleAction::Reconfigure),
        2 => provider_strategy().prop_map(LifecycleAction::Deactivate),
        2 => provider_strategy().prop_map(LifecycleAction::Activate),
        1 => component_strategy().prop_map(LifecycleAction::FailNextActivation),
        1 => component_strategy().prop_map(LifecycleAction::FailNextQuiescence),
        1 => component_strategy().prop_map(LifecycleAction::FailNextDisposal),
    ]
}

fn successful_lifecycle_prefix_action_strategy() -> impl Strategy<Value = LifecycleAction> {
    prop_oneof![
        4 => provider_strategy().prop_map(LifecycleAction::Reconfigure),
        2 => provider_strategy().prop_map(LifecycleAction::Deactivate),
        2 => provider_strategy().prop_map(LifecycleAction::Activate),
    ]
}

fn post_shutdown_mutation_strategy() -> impl Strategy<Value = PostShutdownMutation> {
    prop_oneof![
        Just(PostShutdownMutation::Declare),
        Just(PostShutdownMutation::RemoveCapability),
        Just(PostShutdownMutation::Deactivate),
        Just(PostShutdownMutation::Activate),
    ]
}

fn apply_lifecycle_action(
    action: LifecycleAction,
    executor: &mut ComponentLifecycleExecutor,
    callbacks: &mut PropertyCallbacks,
) -> TestCaseResult {
    match action {
        LifecycleAction::Reconfigure(root) => {
            let component_id = root.component().as_str();
            let _deactivation = run_deactivation_phase(
                executor,
                callbacks,
                DeactivationExpectation::RequiredClosure(root.component()),
                |executor, callbacks| {
                    executor.deactivate_components(
                        [component_id],
                        callbacks,
                        ComponentLifecycleMode::Reconfigure,
                    )
                },
            )?;
            let _activation = run_activation_phase(
                executor,
                callbacks,
                ActivationExpectation::Root(root.component()),
                |executor, callbacks| {
                    executor.activate_components(
                        [component_id],
                        callbacks,
                        ComponentLifecycleMode::Reconfigure,
                    )
                },
            )?;
        }
        LifecycleAction::Deactivate(root) => {
            let _deactivation = run_deactivation_phase(
                executor,
                callbacks,
                DeactivationExpectation::RequiredClosure(root.component()),
                |executor, callbacks| {
                    executor.deactivate_components(
                        [root.component().as_str()],
                        callbacks,
                        ComponentLifecycleMode::Reconfigure,
                    )
                },
            )?;
        }
        LifecycleAction::Activate(root) => {
            let _activation = run_activation_phase(
                executor,
                callbacks,
                ActivationExpectation::Root(root.component()),
                |executor, callbacks| {
                    executor.activate_components(
                        [root.component().as_str()],
                        callbacks,
                        ComponentLifecycleMode::Reconfigure,
                    )
                },
            )?;
        }
        LifecycleAction::FailPublication(root) => {
            let mut deactivation = run_deactivation_phase(
                executor,
                callbacks,
                DeactivationExpectation::RequiredClosure(root.component()),
                |executor, callbacks| {
                    executor.deactivate_components(
                        [root.component().as_str()],
                        callbacks,
                        ComponentLifecycleMode::Reconfigure,
                    )
                },
            )?;
            if executor.finalization != LifecycleFinalization::Open {
                return assert_stable_boundary(executor, &callbacks.snapshot());
            }
            for _ in 0..3 {
                if deactivation.is_ok() {
                    break;
                }
                deactivation = run_deactivation_phase(
                    executor,
                    callbacks,
                    DeactivationExpectation::RequiredClosure(root.component()),
                    |executor, callbacks| {
                        executor.deactivate_components(
                            [root.component().as_str()],
                            callbacks,
                            ComponentLifecycleMode::Reconfigure,
                        )
                    },
                )?;
            }
            prop_assert!(deactivation.is_ok());
            callbacks.disarm_activation_failure(root.component());
            executor.inject_generation_exhaustion(CapabilityKey::from(root.capability()));
            let result = run_activation_phase(
                executor,
                callbacks,
                ActivationExpectation::Root(root.component()),
                |executor, callbacks| {
                    executor.activate_components(
                        [root.component().as_str()],
                        callbacks,
                        ComponentLifecycleMode::Reconfigure,
                    )
                },
            )?;
            prop_assert!(result.is_err());
            prop_assert_eq!(
                executor.graph().state(root.component().as_str()),
                Some(ComponentState::Failed)
            );
            prop_assert!(
                !executor
                    .graph()
                    .has_capability(&CapabilityKey::from(root.capability()))
            );
        }
        LifecycleAction::FailNextActivation(component) => {
            callbacks.arm_activation_failure(component);
        }
        LifecycleAction::FailNextQuiescence(component) => {
            callbacks.arm_quiescence_failure(component);
        }
        LifecycleAction::FailNextDisposal(component) => {
            callbacks.arm_disposal_failure(component);
        }
        LifecycleAction::Shutdown => {
            let _shutdown = run_deactivation_phase(
                executor,
                callbacks,
                DeactivationExpectation::AllActive,
                |executor, callbacks| executor.shutdown(callbacks),
            )?;
        }
    }

    assert_stable_boundary(executor, &callbacks.snapshot())
}

fn run_deactivation_phase(
    executor: &mut ComponentLifecycleExecutor,
    callbacks: &mut PropertyCallbacks,
    expectation: DeactivationExpectation,
    operation: impl FnOnce(
        &mut ComponentLifecycleExecutor,
        &mut PropertyCallbacks,
    ) -> Result<(), LifecycleExecutionError>,
) -> Result<Result<(), LifecycleExecutionError>, TestCaseError> {
    let before_active = active_components(executor);
    let before_callbacks = callbacks.snapshot();
    let can_execute = executor.finalization == LifecycleFinalization::Open
        || expectation == DeactivationExpectation::AllActive;
    let expected_components = if !can_execute {
        BTreeSet::new()
    } else {
        match expectation {
            DeactivationExpectation::RequiredClosure(root) => required_closure(root)
                .intersection(&before_active)
                .copied()
                .collect(),
            DeactivationExpectation::AllActive => before_active.clone(),
        }
    };
    let retained_scope_lifecycles = executor
        .scope_snapshots()
        .into_iter()
        .map(|scope| (TestComponent::from_id(&scope.owner), scope.lifecycle))
        .collect::<BTreeMap<_, _>>();
    let retry_deactivations = executor
        .graph()
        .failures()
        .into_iter()
        .filter(|failure| {
            can_execute
                && failure.recoverable
                && failure.operation == ComponentFailureOperation::Deactivation
        })
        .map(|failure| TestComponent::from_id(&failure.component_id))
        .collect::<BTreeSet<_>>();
    let retry_activation_active = executor
        .graph()
        .failures()
        .into_iter()
        .filter(|failure| {
            can_execute
                && failure.recoverable
                && failure.operation == ComponentFailureOperation::Activation
                && retained_scope_lifecycles.get(&TestComponent::from_id(&failure.component_id))
                    == Some(&EffectScopeLifecycleSnapshot::Active)
        })
        .map(|failure| TestComponent::from_id(&failure.component_id))
        .collect::<BTreeSet<_>>();
    let retry_activation_finalizing = executor
        .graph()
        .failures()
        .into_iter()
        .filter(|failure| {
            can_execute
                && failure.recoverable
                && failure.operation == ComponentFailureOperation::Activation
                && retained_scope_lifecycles.get(&TestComponent::from_id(&failure.component_id))
                    == Some(&EffectScopeLifecycleSnapshot::Finalizing)
        })
        .map(|failure| TestComponent::from_id(&failure.component_id))
        .collect::<BTreeSet<_>>();
    let retried_components = retry_deactivations
        .union(&retry_activation_active)
        .copied()
        .collect::<BTreeSet<_>>()
        .union(&retry_activation_finalizing)
        .copied()
        .collect::<BTreeSet<_>>();
    let event_offset = callbacks.snapshot().events.len();
    let result = operation(executor, callbacks);
    if let Err(error) = &result {
        assert_redacted_debug(&format!("{error:?}"))?;
    }
    let state = callbacks.snapshot();
    let events = &state.events[event_offset..];
    let retry_quiescence_candidates = retry_deactivations
        .union(&retry_activation_active)
        .copied()
        .collect::<BTreeSet<_>>();
    let retry_quiescence = retry_quiescence_candidates
        .difference(&before_callbacks.quiescence_failures)
        .copied()
        .collect::<BTreeSet<_>>();
    let retry_disposal_candidates = retry_quiescence
        .union(&retry_activation_finalizing)
        .copied()
        .collect::<BTreeSet<_>>();
    let retry_disposal = retry_disposal_candidates
        .difference(&before_callbacks.disposal_failures)
        .copied()
        .collect::<BTreeSet<_>>();
    let retry_failed = retry_quiescence_candidates
        .iter()
        .any(|component| before_callbacks.quiescence_failures.contains(component))
        || retry_disposal_candidates
            .iter()
            .any(|component| before_callbacks.disposal_failures.contains(component));
    let retry_blocks_fresh = retry_failed && expectation != DeactivationExpectation::AllActive;
    let fresh_components = if retry_blocks_fresh {
        BTreeSet::new()
    } else {
        expected_components
            .difference(&retried_components)
            .copied()
            .collect::<BTreeSet<_>>()
    };
    let fresh_quiescence = fresh_components
        .difference(&before_callbacks.quiescence_failures)
        .copied()
        .collect::<BTreeSet<_>>();
    let fresh_disposal = fresh_quiescence
        .difference(&before_callbacks.disposal_failures)
        .copied()
        .collect::<BTreeSet<_>>();
    let expected_quiescence = retry_quiescence
        .union(&fresh_quiescence)
        .copied()
        .collect::<BTreeSet<_>>();
    let expected_disposal = retry_disposal
        .union(&fresh_disposal)
        .copied()
        .collect::<BTreeSet<_>>();
    let actual_quiescence = event_components(events, ResourceEventKind::Quiesce);
    let actual_disposal = event_components(events, ResourceEventKind::Dispose);
    prop_assert_eq!(actual_quiescence.len(), expected_quiescence.len());
    prop_assert_eq!(
        actual_quiescence
            .iter()
            .map(|component| TestComponent::from_id(component))
            .collect::<BTreeSet<_>>(),
        expected_quiescence
    );
    prop_assert_eq!(actual_disposal.len(), expected_disposal.len());
    prop_assert_eq!(
        actual_disposal
            .iter()
            .map(|component| TestComponent::from_id(component))
            .collect::<BTreeSet<_>>(),
        expected_disposal
    );
    prop_assert!(
        events
            .iter()
            .all(|event| event.kind != ResourceEventKind::Activate)
    );
    assert_deactivation_failures(
        executor,
        &before_callbacks,
        &fresh_components
            .union(&retry_deactivations)
            .copied()
            .collect(),
        result.is_err(),
    )?;
    assert_resource_event_history(&state)?;
    Ok(result)
}

fn assert_deactivation_failures(
    executor: &ComponentLifecycleExecutor,
    before_callbacks: &CallbackState,
    deactivated_components: &BTreeSet<TestComponent>,
    operation_failed: bool,
) -> TestCaseResult {
    let expected = deactivated_components
        .iter()
        .filter_map(|component| {
            if before_callbacks.quiescence_failures.contains(component) {
                Some((*component, ComponentFailureReason::QuiescenceRejected))
            } else if before_callbacks.disposal_failures.contains(component) {
                Some((*component, ComponentFailureReason::EffectDisposalRejected))
            } else {
                None
            }
        })
        .collect::<BTreeMap<_, _>>();
    if !expected.is_empty() {
        prop_assert!(operation_failed);
    }

    let failures = executor.graph().failures();
    for (component, reason) in expected {
        let failure = failures
            .iter()
            .find(|failure| failure.component_id == component.as_str());
        prop_assert!(failure.is_some());
        let failure = failure.expect("checked generated deactivation failure must exist");
        prop_assert_eq!(failure.operation, ComponentFailureOperation::Deactivation);
        prop_assert_eq!(failure.reason, reason);
        prop_assert!(failure.recoverable);
        prop_assert_eq!(
            executor.graph().state(component.as_str()),
            Some(ComponentState::Failed)
        );
    }
    Ok(())
}

fn run_activation_phase(
    executor: &mut ComponentLifecycleExecutor,
    callbacks: &mut PropertyCallbacks,
    expectation: ActivationExpectation,
    operation: impl FnOnce(
        &mut ComponentLifecycleExecutor,
        &mut PropertyCallbacks,
    ) -> Result<(), LifecycleExecutionError>,
) -> Result<Result<(), LifecycleExecutionError>, TestCaseError> {
    let expected_components = expected_activation_components(executor, expectation);
    let event_offset = callbacks.snapshot().events.len();
    let result = operation(executor, callbacks);
    if let Err(error) = &result {
        assert_redacted_debug(&format!("{error:?}"))?;
    }
    let state = callbacks.snapshot();
    let events = &state.events[event_offset..];
    let actual = event_components(events, ResourceEventKind::Activate);
    prop_assert!(is_rank_ordered(
        &actual,
        &rank(executor.graph().activation_order())
    ));
    if result.is_ok() {
        let expected =
            ordered_components(executor.graph().activation_order(), &expected_components);
        prop_assert_eq!(actual, expected);
        if let Some(first_activation) = events
            .iter()
            .position(|event| event.kind == ResourceEventKind::Activate)
        {
            prop_assert!(
                events[first_activation..]
                    .iter()
                    .all(|event| event.kind == ResourceEventKind::Activate)
            );
        }
    }
    assert_resource_event_history(&state)?;
    Ok(result)
}

fn active_components(executor: &ComponentLifecycleExecutor) -> BTreeSet<TestComponent> {
    executor
        .graph()
        .components()
        .into_iter()
        .filter(|component| component.state == ComponentState::Active)
        .map(|component| TestComponent::from_id(&component.id))
        .collect()
}

fn expected_activation_components(
    executor: &ComponentLifecycleExecutor,
    expectation: ActivationExpectation,
) -> BTreeSet<TestComponent> {
    if expectation == ActivationExpectation::All {
        return TestComponent::ALL.into_iter().collect();
    }
    let ActivationExpectation::Root(root) = expectation else {
        unreachable!("all-components activation was handled above");
    };
    let states = executor
        .graph()
        .components()
        .into_iter()
        .map(|component| (TestComponent::from_id(&component.id), component.state))
        .collect::<BTreeMap<_, _>>();
    let retryable_deactivation_failures = executor
        .graph()
        .failures()
        .into_iter()
        .filter(|failure| {
            failure.recoverable && failure.operation == ComponentFailureOperation::Deactivation
        })
        .map(|failure| TestComponent::from_id(&failure.component_id))
        .collect::<BTreeSet<_>>();
    let retryable_activation_failures = executor
        .graph()
        .failures()
        .into_iter()
        .filter(|failure| {
            failure.recoverable && failure.operation == ComponentFailureOperation::Activation
        })
        .map(|failure| TestComponent::from_id(&failure.component_id))
        .collect::<BTreeSet<_>>();
    let before_active = active_components(executor);
    let mut expected_active = before_active.clone();
    let reachable = required_closure(root);
    let mut changed = true;
    while changed {
        changed = false;
        for component_id in executor.graph().activation_order() {
            let component = TestComponent::from_id(&component_id);
            if !reachable.contains(&component) || expected_active.contains(&component) {
                continue;
            }
            let Some(state) = states.get(&component).copied() else {
                continue;
            };
            let can_activate = matches!(state, ComponentState::Declared | ComponentState::Pending)
                || (state == ComponentState::Failed
                    && (retryable_deactivation_failures.contains(&component)
                        || (component == root
                            && retryable_activation_failures.contains(&component))))
                || (component == root && state == ComponentState::Disposed);
            if can_activate
                && required_providers(component)
                    .iter()
                    .all(|provider| expected_active.contains(provider))
            {
                expected_active.insert(component);
                changed = true;
            }
        }
    }
    expected_active
        .difference(&before_active)
        .copied()
        .collect()
}

fn required_closure(root: TestComponent) -> BTreeSet<TestComponent> {
    TestComponent::ALL
        .into_iter()
        .filter(|component| *component == root || transitively_requires(*component, root))
        .collect()
}

fn transitively_requires(component: TestComponent, provider: TestComponent) -> bool {
    required_providers(component)
        .iter()
        .any(|required| *required == provider || transitively_requires(*required, provider))
}

const fn required_providers(component: TestComponent) -> &'static [TestComponent] {
    match component {
        TestComponent::AlphaSource | TestComponent::BetaSource | TestComponent::Observer => &[],
        TestComponent::LeftBranch => &[TestComponent::AlphaSource],
        TestComponent::RightBranch => &[TestComponent::AlphaSource, TestComponent::BetaSource],
        TestComponent::DiamondLeaf => &[TestComponent::LeftBranch, TestComponent::RightBranch],
    }
}

fn ordered_components(order: Vec<String>, included: &BTreeSet<TestComponent>) -> Vec<String> {
    order
        .into_iter()
        .filter(|component_id| included.contains(&TestComponent::from_id(component_id)))
        .collect()
}

fn event_components(events: &[ResourceEvent], kind: ResourceEventKind) -> Vec<String> {
    events
        .iter()
        .filter(|event| event.kind == kind)
        .map(|event| event.key.component.as_str().to_string())
        .collect()
}

fn assert_resource_event_history(callbacks: &CallbackState) -> TestCaseResult {
    let mut events_by_resource = BTreeMap::<ResourceKey, Vec<ResourceEventKind>>::new();
    for event in &callbacks.events {
        events_by_resource
            .entry(event.key)
            .or_default()
            .push(event.kind);
    }
    for (key, kinds) in events_by_resource {
        let live_resource = callbacks
            .resources
            .get(&key.component)
            .filter(|(live_key, _)| *live_key == key);
        prop_assert_eq!(kinds.first(), Some(&ResourceEventKind::Activate));
        prop_assert_eq!(
            kinds
                .iter()
                .filter(|kind| **kind == ResourceEventKind::Activate)
                .count(),
            1
        );
        if let Some((_, resource)) = live_resource {
            prop_assert!(!kinds.contains(&ResourceEventKind::Dispose));
            if resource.is_quiesced {
                prop_assert!(
                    kinds[1..]
                        .iter()
                        .all(|kind| *kind == ResourceEventKind::Quiesce)
                );
                prop_assert!(kinds.len() >= 2);
            } else {
                prop_assert_eq!(kinds, vec![ResourceEventKind::Activate]);
            }
        } else {
            prop_assert_eq!(kinds.last(), Some(&ResourceEventKind::Dispose));
            prop_assert!(
                kinds[1..kinds.len() - 1]
                    .iter()
                    .all(|kind| *kind == ResourceEventKind::Quiesce)
            );
        }
    }
    Ok(())
}

fn assert_stable_boundary(
    executor: &ComponentLifecycleExecutor,
    callbacks: &CallbackState,
) -> TestCaseResult {
    prop_assert!(callbacks.disposed_without_quiescence.is_empty());
    prop_assert!(callbacks.duplicate_activations.is_empty());
    prop_assert!(callbacks.missing_quiescence.is_empty());
    prop_assert!(callbacks.missing_disposals.is_empty());
    prop_assert!(callbacks.mismatched_disposals.is_empty());
    assert_resource_event_history(callbacks)?;
    let components = executor.graph().components();
    let component_by_id = components
        .iter()
        .map(|component| (component.id.as_str(), component))
        .collect::<BTreeMap<_, _>>();
    let active = components
        .iter()
        .filter(|component| component.state == ComponentState::Active)
        .map(|component| TestComponent::from_id(&component.id))
        .collect::<BTreeSet<_>>();
    let scope_lifecycles = executor
        .scope_snapshots()
        .into_iter()
        .map(|scope| {
            let component = TestComponent::from_id(&scope.owner);
            prop_assert!(scope.effects.contains(&RESOURCE_EFFECT.to_string()));
            prop_assert!(scope.children.is_empty());
            Ok((component, scope.lifecycle))
        })
        .collect::<Result<BTreeMap<_, _>, TestCaseError>>()?;
    let scope_owners = scope_lifecycles.keys().copied().collect::<BTreeSet<_>>();
    let resource_owners = callbacks.resources.keys().copied().collect::<BTreeSet<_>>();
    prop_assert_eq!(&scope_owners, &resource_owners);
    prop_assert!(active.is_subset(&resource_owners));
    let graph_capabilities = executor
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
    let context_capabilities = executor
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
    prop_assert_eq!(context_capabilities, graph_capabilities);

    for (component, (key, resource)) in &callbacks.resources {
        let snapshot = component_by_id
            .get(component.as_str())
            .expect("resource owner must remain declared");
        let lifecycle = scope_lifecycles
            .get(component)
            .expect("live resource must retain its scope owner");
        match snapshot.state {
            ComponentState::Active => {
                prop_assert_eq!(*lifecycle, EffectScopeLifecycleSnapshot::Active);
                prop_assert!(!resource.is_quiesced);
            }
            ComponentState::Failed => match lifecycle {
                EffectScopeLifecycleSnapshot::Active => prop_assert!(!resource.is_quiesced),
                EffectScopeLifecycleSnapshot::Finalizing => prop_assert!(resource.is_quiesced),
            },
            state => prop_assert!(false, "resource owner reached invalid state {state:?}"),
        }
        prop_assert_eq!(snapshot.epoch, key.epoch);
    }
    let all_components_are_stable = components.iter().all(|component| {
        !matches!(
            component.state,
            ComponentState::Declared | ComponentState::Activating | ComponentState::Deactivating
        )
    });
    prop_assert!(all_components_are_stable);
    for component in &components {
        let callback_epoch = callbacks
            .activation_epochs
            .get(&TestComponent::from_id(&component.id))
            .copied()
            .unwrap_or(0);
        prop_assert_eq!(callback_epoch, component.epoch);
    }

    let expected_capabilities = [
        (TestComponent::AlphaSource, "alpha"),
        (TestComponent::BetaSource, "beta"),
        (TestComponent::LeftBranch, "left"),
        (TestComponent::RightBranch, "right"),
    ]
    .into_iter()
    .filter(|(provider, _)| {
        component_by_id
            .get(provider.as_str())
            .is_some_and(|component| component.state == ComponentState::Active)
    })
    .map(|(provider, capability)| (capability.to_string(), provider.as_str().to_string()))
    .collect::<BTreeMap<_, _>>();
    let actual_capabilities = executor
        .graph()
        .capabilities()
        .into_iter()
        .map(|capability| {
            prop_assert_eq!(
                component_by_id
                    .get(capability.provider_component.as_str())
                    .map(|component| component.state),
                Some(ComponentState::Active),
            );
            Ok((capability.key, capability.provider_component))
        })
        .collect::<Result<BTreeMap<_, _>, TestCaseError>>()?;
    prop_assert_eq!(actual_capabilities, expected_capabilities);

    let diagnostic = format!("{:?}", executor_snapshot(executor));
    assert_redacted_debug(&diagnostic)
}

fn assert_redacted_debug(diagnostic: &str) -> TestCaseResult {
    for sentinel in [
        ACTIVATION_SENTINEL,
        QUIESCENCE_SENTINEL,
        DISPOSER_SENTINEL,
        AUTHORITY_PREPARATION_SENTINEL,
    ] {
        prop_assert!(!diagnostic.contains(sentinel));
    }
    Ok(())
}

fn execute_authority_scenario(
    scenario: AuthorityScenario,
    desired: Vec<ComponentDefinition>,
    sequence: usize,
    executor: &mut ComponentLifecycleExecutor,
    callbacks: &mut PropertyCallbacks,
) -> Result<(), LifecycleExecutionError> {
    callbacks.begin_authority_transaction();

    match scenario {
        AuthorityScenario::GraphPreflightFailure => executor.reconcile_definitions_with_commit(
            [
                ComponentDefinition::new("duplicate"),
                ComponentDefinition::new("duplicate"),
            ],
            callbacks,
            ComponentLifecycleMode::Reconfigure,
        ),
        AuthorityScenario::QuiescenceFailure => {
            callbacks.arm_quiescence_failure(TestComponent::AlphaSource);
            executor.reconcile_definitions_with_commit(
                desired,
                callbacks,
                ComponentLifecycleMode::Reconfigure,
            )
        }
        AuthorityScenario::DisposalFailure => {
            callbacks.arm_disposal_failure(TestComponent::AlphaSource);
            executor.reconcile_definitions_with_commit(
                desired,
                callbacks,
                ComponentLifecycleMode::Reconfigure,
            )
        }
        AuthorityScenario::PreparationFailure => {
            callbacks.arm_authority_preparation_failure();
            executor.reconcile_definitions_with_commit(
                desired,
                callbacks,
                ComponentLifecycleMode::Reconfigure,
            )
        }
        AuthorityScenario::ActivationFailure => {
            callbacks.arm_activation_failure(TestComponent::AlphaSource);
            executor.reconcile_definitions_with_commit(
                desired,
                callbacks,
                ComponentLifecycleMode::Reconfigure,
            )
        }
        AuthorityScenario::Success => executor.reconcile_definitions_with_commit(
            desired,
            callbacks,
            ComponentLifecycleMode::Reconfigure,
        ),
        AuthorityScenario::StaleGraphCommit => {
            let prepared = match executor.graph.prepare_definition_reconciliation(desired) {
                Ok(prepared) => prepared,
                Err(error) => {
                    callbacks.abort_authority();
                    return Err(LifecycleExecutionError::graph("runtime_composition", error));
                }
            };
            let retirement_order = prepared.retirement_order().to_vec();
            if let Err(error) = executor.deactivate_components(
                retirement_order,
                callbacks,
                ComponentLifecycleMode::Reconfigure,
            ) {
                callbacks.abort_authority();
                return Err(error);
            }
            if callbacks.prepare_authority().is_err() {
                callbacks.abort_authority();
                return Err(LifecycleExecutionError::authority_preparation());
            }
            executor
                .graph
                .declare(ComponentDefinition::new(format!(
                    "unexpected_component_{sequence}"
                )))
                .expect("unique test mutation should stale the prepared graph");
            executor
                .commit_prepared_graph(prepared, callbacks, true)
                .map(|_| ())
        }
    }
}

fn exercise_authority_sequence(
    scenarios: &[AuthorityScenario],
    executor: &mut ComponentLifecycleExecutor,
    callbacks: &mut PropertyCallbacks,
) -> TestCaseResult {
    for (sequence, scenario) in scenarios.iter().copied().enumerate() {
        let before = callbacks.snapshot().authority;
        let before_alpha_epoch = executor
            .graph()
            .epoch(TestComponent::AlphaSource.as_str())
            .expect("authority provider should retain its epoch tombstone");
        let result = execute_authority_scenario(
            scenario,
            replacement_definitions_for_generation(sequence + 1),
            sequence,
            executor,
            callbacks,
        );
        if let Err(error) = &result {
            assert_redacted_debug(&format!("{error:?}"))?;
        }

        let after = callbacks.snapshot().authority;
        let commit_delta = after.commits - before.commits;
        let abort_delta = after.aborts - before.aborts;
        prop_assert!(commit_delta <= 1);
        prop_assert!(abort_delta <= 1);
        prop_assert_eq!(commit_delta + abort_delta, 1);
        prop_assert!(after.preparations - before.preparations <= 1);
        prop_assert!(!after.has_unpublished_authority);
        prop_assert_eq!(after.generation, after.commits as u64);
        prop_assert_eq!(
            after.status,
            if commit_delta == 1 {
                AuthorityTransactionStatus::Committed
            } else {
                AuthorityTransactionStatus::Aborted
            }
        );
        let after_alpha_epoch = executor
            .graph()
            .epoch(TestComponent::AlphaSource.as_str())
            .expect("authority provider should retain its epoch tombstone");
        prop_assert!(after_alpha_epoch >= before_alpha_epoch);
        prop_assert_eq!(after_alpha_epoch - before_alpha_epoch, commit_delta as u64);
        if result.is_ok() {
            prop_assert_eq!(commit_delta, 1);
        }
    }

    let terminal = callbacks.snapshot().authority;
    callbacks.abort_authority();
    callbacks.abort_authority();
    let after_repeated_abort = callbacks.snapshot().authority;
    prop_assert_eq!(after_repeated_abort.status, terminal.status);
    prop_assert_eq!(after_repeated_abort.commits, terminal.commits);
    prop_assert_eq!(after_repeated_abort.generation, terminal.generation);
    prop_assert!(!after_repeated_abort.has_unpublished_authority);
    prop_assert_eq!(after_repeated_abort.aborts, terminal.aborts + 2);
    Ok(())
}

fn is_rank_ordered(components: &[String], ranks: &BTreeMap<String, usize>) -> bool {
    components.windows(2).all(|pair| {
        ranks.get(&pair[0]).copied().unwrap_or(usize::MAX)
            <= ranks.get(&pair[1]).copied().unwrap_or(usize::MAX)
    })
}

fn lock_callback_state(
    state: &Arc<Mutex<CallbackState>>,
) -> std::sync::MutexGuard<'_, CallbackState> {
    state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[test]
fn shutdown_aborts_pending_authority_once() {
    let mut executor = ComponentLifecycleExecutor::default();
    let mut callbacks = PropertyCallbacks::default();
    callbacks.begin_authority_transaction();

    executor
        .shutdown(&mut callbacks)
        .expect("empty composition should shut down");
    let after_shutdown = callbacks.snapshot().authority;
    assert_eq!(after_shutdown.status, AuthorityTransactionStatus::Aborted);
    assert_eq!(after_shutdown.aborts, 1);
    assert!(!after_shutdown.has_unpublished_authority);

    executor
        .shutdown(&mut callbacks)
        .expect("repeated shutdown should be a no-op");
    assert_eq!(callbacks.snapshot().authority, after_shutdown);
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 64,
        max_shrink_iters: 4_096,
        ..ProptestConfig::default()
    })]

    #[test]
    fn arbitrary_lifecycle_sequences_preserve_composition_invariants(
        actions in prop::collection::vec(lifecycle_action_strategy(), 1..49),
    ) {
        let mut executor = ComponentLifecycleExecutor::default();
        let mut callbacks = PropertyCallbacks::default();
        let initial = run_activation_phase(
            &mut executor,
            &mut callbacks,
            ActivationExpectation::All,
            |executor, callbacks| {
                executor.reconcile_definitions(
                    definitions(),
                    callbacks,
                    ComponentLifecycleMode::Initial,
                )
            },
        )?;
        prop_assert!(initial.is_ok());
        let initial_state = callbacks.snapshot();
        assert_stable_boundary(&executor, &initial_state)?;

        for action in actions {
            apply_lifecycle_action(action, &mut executor, &mut callbacks)?;
        }

        let _shutdown = run_deactivation_phase(
            &mut executor,
            &mut callbacks,
            DeactivationExpectation::AllActive,
            |executor, callbacks| executor.shutdown(callbacks),
        )?;
        for _ in 0..3 {
            if executor.finalization == LifecycleFinalization::Succeeded {
                break;
            }
            let _ = executor.shutdown(&mut callbacks);
        }
        prop_assert!(executor.finalization == LifecycleFinalization::Succeeded);
        let final_state = callbacks.snapshot();
        assert_stable_boundary(&executor, &final_state)?;
        prop_assert!(executor.scope_snapshots().is_empty());
        prop_assert!(executor.graph().capabilities().is_empty());
        prop_assert!(final_state.resources.is_empty());

        let before_repeat = (executor_snapshot(&executor), final_state);
        executor
            .shutdown(&mut callbacks)
            .expect("repeated shutdown must be a no-op");
        prop_assert_eq!(
            (executor_snapshot(&executor), callbacks.snapshot()),
            before_repeat,
        );
    }

    #[test]
    fn definition_transactions_preserve_authority_state_machine(
        scenarios in prop::collection::vec(authority_scenario_strategy(), 1..17),
    ) {
        let mut executor = ComponentLifecycleExecutor::default();
        let mut callbacks = PropertyCallbacks::default();
        executor
            .reconcile_definitions(
                definitions(),
                &mut callbacks,
                ComponentLifecycleMode::Initial,
            )
            .expect("fixed acyclic graph must activate");

        exercise_authority_sequence(&scenarios, &mut executor, &mut callbacks)?;
    }

    #[test]
    fn publication_failure_rolls_back_after_arbitrary_lifecycle_prefix(
        actions in prop::collection::vec(lifecycle_prefix_action_strategy(), 0..33),
        root in provider_strategy(),
    ) {
        let mut executor = ComponentLifecycleExecutor::default();
        let mut callbacks = PropertyCallbacks::default();
        let initial = run_activation_phase(
            &mut executor,
            &mut callbacks,
            ActivationExpectation::All,
            |executor, callbacks| {
                executor.reconcile_definitions(
                    definitions(),
                    callbacks,
                    ComponentLifecycleMode::Initial,
                )
            },
        )?;
        prop_assert!(initial.is_ok());

        for action in actions {
            apply_lifecycle_action(action, &mut executor, &mut callbacks)?;
        }
        prop_assume!(
            executor.graph().state(root.component().as_str()) != Some(ComponentState::Failed)
        );
        callbacks.clear_failure_injections();
        let event_offset = callbacks.snapshot().events.len();
        let previous_epoch = executor
            .graph()
            .components()
            .into_iter()
            .find(|component| component.id == root.component().as_str())
            .expect("publication provider must remain declared")
            .epoch;
        apply_lifecycle_action(
            LifecycleAction::FailPublication(root),
            &mut executor,
            &mut callbacks,
        )?;

        let state = callbacks.snapshot();
        let failed_epoch_events = state.events[event_offset..]
            .iter()
            .filter(|event| {
                event.key.component == root.component() && event.key.epoch > previous_epoch
            })
            .map(|event| event.kind)
            .collect::<Vec<_>>();
        prop_assert_eq!(
            failed_epoch_events,
            vec![
                ResourceEventKind::Activate,
                ResourceEventKind::Quiesce,
                ResourceEventKind::Dispose,
            ]
        );

        let component = executor
            .graph()
            .components()
            .into_iter()
            .find(|component| component.id == root.component().as_str())
            .expect("publication provider must remain declared");
        prop_assert_eq!(component.state, ComponentState::Failed);
        prop_assert_eq!(component.epoch, previous_epoch + 1);
        let publication_failure = executor
            .graph()
            .failures()
            .into_iter()
            .find(|failure| failure.component_id == root.component().as_str());
        prop_assert!(publication_failure.is_some());
        let publication_failure =
            publication_failure.expect("failed publication must leave a closed graph failure");
        prop_assert_eq!(
            publication_failure.operation,
            ComponentFailureOperation::Activation
        );
        prop_assert_eq!(
            publication_failure.reason,
            ComponentFailureReason::ActivationRejected
        );
        prop_assert!(!executor
            .scope_snapshots()
            .iter()
            .any(|scope| scope.owner == root.component().as_str()));
        prop_assert!(!state.resources.contains_key(&root.component()));
    }

    #[test]
    fn deactivation_failures_remain_terminal_after_arbitrary_lifecycle_prefix(
        actions in prop::collection::vec(successful_lifecycle_prefix_action_strategy(), 0..33),
        component in component_strategy(),
        failure_kind in deactivation_failure_strategy(),
    ) {
        let mut executor = ComponentLifecycleExecutor::default();
        let mut callbacks = PropertyCallbacks::default();
        let initial = run_activation_phase(
            &mut executor,
            &mut callbacks,
            ActivationExpectation::All,
            |executor, callbacks| {
                executor.reconcile_definitions(
                    definitions(),
                    callbacks,
                    ComponentLifecycleMode::Initial,
                )
            },
        )?;
        prop_assert!(initial.is_ok());

        for action in actions {
            apply_lifecycle_action(action, &mut executor, &mut callbacks)?;
        }
        for root in [ProviderRoot::Alpha, ProviderRoot::Beta] {
            let activation = run_activation_phase(
                &mut executor,
                &mut callbacks,
                ActivationExpectation::Root(root.component()),
                |executor, callbacks| {
                    executor.activate_components(
                        [root.component().as_str()],
                        callbacks,
                        ComponentLifecycleMode::Reconfigure,
                    )
                },
            )?;
            prop_assert!(activation.is_ok());
        }
        prop_assert_eq!(
            executor.graph().state(component.as_str()),
            Some(ComponentState::Active)
        );

        match failure_kind {
            DeactivationFailureKind::Quiescence => {
                callbacks.arm_quiescence_failure(component);
            }
            DeactivationFailureKind::Disposal => callbacks.arm_disposal_failure(component),
        }
        let deactivation = run_deactivation_phase(
            &mut executor,
            &mut callbacks,
            DeactivationExpectation::RequiredClosure(component),
            |executor, callbacks| {
                executor.deactivate_components(
                    [component.as_str()],
                    callbacks,
                    ComponentLifecycleMode::Reconfigure,
                )
            },
        )?;
        prop_assert!(deactivation.is_err());
        assert_stable_boundary(&executor, &callbacks.snapshot())?;
    }

    #[test]
    fn stale_activation_tokens_are_mutation_free(
        rounds in 1_usize..12,
        fail_first in any::<bool>(),
    ) {
        let mut graph = ComponentGraph::default();
        let declaration = graph
            .declare(ComponentDefinition::new("provider").provides("service"))
            .expect("provider declaration must succeed");
        let mut current = declaration.activation_requests[0].clone();
        let mut stale = Vec::new();

        for _ in 0..rounds {
            stale.push(current.clone());
            graph
                .complete_activation_and_publish(current)
                .expect("current activation must publish atomically");
            graph
                .remove_capability("provider", &CapabilityKey::from("service"))
                .expect("current capability must be removable");
            let deactivation = graph.deactivate("provider").expect("active provider must deactivate");
            graph
                .complete_deactivation(deactivation.deactivation_requests[0].clone())
                .expect("current deactivation must complete");
            let activation = graph.activate("provider").expect("provider must reactivate");
            current = activation.activation_requests[0].clone();
        }

        let before = graph_snapshot(&graph);
        for token in stale {
            let calls = if fail_first { [true, false] } else { [false, true] };
            for fail in calls {
                let outcome = if fail {
                    graph.fail_activation(
                        token.clone(),
                        ComponentFailureReason::ActivationRejected,
                        true,
                    ).map(|_| ())
                } else {
                    graph.complete_activation_and_publish(token.clone()).map(|_| ())
                };
                prop_assert!(outcome.is_err());
                prop_assert_eq!(graph_snapshot(&graph), before.clone());
            }
        }
    }

    #[test]
    fn stale_deactivation_tokens_are_mutation_free(
        rounds in 1_usize..12,
        fail_first in any::<bool>(),
    ) {
        let mut graph = ComponentGraph::default();
        let declaration = graph
            .declare(ComponentDefinition::new("worker"))
            .expect("worker declaration must succeed");
        graph
            .complete_activation(declaration.activation_requests[0].clone())
            .expect("initial activation must complete");
        let mut stale = Vec::new();

        for _ in 0..rounds {
            let deactivation = graph.deactivate("worker").expect("active worker must deactivate");
            let token = deactivation.deactivation_requests[0].clone();
            stale.push(token.clone());
            graph
                .complete_deactivation(token)
                .expect("current deactivation must complete");
            let activation = graph.activate("worker").expect("worker must reactivate");
            graph
                .complete_activation(activation.activation_requests[0].clone())
                .expect("current activation must complete");
        }
        graph.deactivate("worker").expect("worker must enter a newer deactivation");

        let before = graph_snapshot(&graph);
        for token in stale {
            let calls = if fail_first { [true, false] } else { [false, true] };
            for fail in calls {
                let outcome = if fail {
                    graph.fail_deactivation(
                        token.clone(),
                        ComponentFailureReason::QuiescenceRejected,
                        false,
                    )
                } else {
                    graph.complete_deactivation(token.clone())
                };
                prop_assert!(outcome.is_err());
                prop_assert_eq!(graph_snapshot(&graph), before.clone());
            }
        }
    }

    #[test]
    fn post_shutdown_mutations_are_rejected_without_observable_change(
        mutation in post_shutdown_mutation_strategy(),
    ) {
        let mut executor = ComponentLifecycleExecutor::default();
        let mut callbacks = PropertyCallbacks::default();
        executor
            .reconcile_definitions(
                definitions(),
                &mut callbacks,
                ComponentLifecycleMode::Initial,
            )
            .expect("fixed acyclic graph must activate");
        let _ = executor.shutdown(&mut callbacks);
        let before = (executor_snapshot(&executor), callbacks.snapshot());

        let result = match mutation {
            PostShutdownMutation::Declare => executor.reconcile_definitions(
                [ComponentDefinition::new("extra_component")],
                &mut callbacks,
                ComponentLifecycleMode::Initial,
            ),
            PostShutdownMutation::RemoveCapability => executor.remove_capability(
                TestComponent::AlphaSource.as_str(),
                &CapabilityKey::from("alpha"),
                &mut callbacks,
                ComponentLifecycleMode::Reconfigure,
            ),
            PostShutdownMutation::Deactivate => executor.deactivate_components(
                [TestComponent::AlphaSource.as_str()],
                &mut callbacks,
                ComponentLifecycleMode::Reconfigure,
            ),
            PostShutdownMutation::Activate => executor.activate_components(
                [TestComponent::AlphaSource.as_str()],
                &mut callbacks,
                ComponentLifecycleMode::Reconfigure,
            ),
        };
        let error = result.expect_err("post-shutdown mutation must be rejected");
        assert_redacted_debug(&format!("{error:?}"))?;
        prop_assert_eq!(
            (executor_snapshot(&executor), callbacks.snapshot()),
            before,
        );
    }
}
