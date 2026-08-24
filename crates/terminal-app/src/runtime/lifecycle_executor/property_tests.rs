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

    const fn capability(self) -> Option<&'static str> {
        match self {
            Self::AlphaSource => Some("alpha"),
            Self::BetaSource => Some("beta"),
            Self::LeftBranch => Some("left"),
            Self::RightBranch => Some("right"),
            Self::DiamondLeaf | Self::Observer => None,
        }
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

    fn snapshot(&self) -> CallbackState {
        lock_callback_state(&self.state).clone()
    }
}

impl ComponentLifecycleCallbacks for PropertyCallbacks {
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
            let Some((live_key, resource)) = state.resources.remove(&component) else {
                state.missing_disposals.insert(key);
                return Err(DISPOSER_SENTINEL.to_string());
            };
            if live_key != key {
                state.mismatched_disposals.insert(key);
                state.resources.insert(component, (live_key, resource));
                return Err(DISPOSER_SENTINEL.to_string());
            }
            if !resource.is_quiesced {
                state.disposed_without_quiescence.insert(key);
            }
            state.events.push(ResourceEvent {
                kind: ResourceEventKind::Dispose,
                key,
            });
            if state.disposal_failures.remove(&component) {
                Err(DISPOSER_SENTINEL.to_string())
            } else {
                Ok(())
            }
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
        if state.quiescence_failures.remove(&component) {
            Err(QUIESCENCE_SENTINEL.to_string())
        } else {
            Ok(())
        }
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
            if executor.is_shutdown {
                return assert_stable_boundary(executor, &callbacks.snapshot());
            }
            if executor.graph().state(root.component().as_str()) == Some(ComponentState::Failed) {
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
    let expected_components = match expectation {
        DeactivationExpectation::RequiredClosure(root) => required_closure(root)
            .intersection(&before_active)
            .copied()
            .collect(),
        DeactivationExpectation::AllActive => before_active.clone(),
    };
    let event_offset = callbacks.snapshot().events.len();
    let result = operation(executor, callbacks);
    if let Err(error) = &result {
        assert_redacted_debug(&format!("{error:?}"))?;
    }
    let state = callbacks.snapshot();
    let events = &state.events[event_offset..];
    let expected = ordered_components(executor.graph().deactivation_order(), &expected_components);
    prop_assert_eq!(
        event_components(events, ResourceEventKind::Quiesce),
        expected.clone()
    );
    prop_assert_eq!(
        event_components(events, ResourceEventKind::Dispose),
        expected
    );
    prop_assert!(
        events
            .iter()
            .all(|event| event.kind != ResourceEventKind::Activate)
    );
    assert_deactivation_failures(
        executor,
        &before_callbacks,
        &expected_components,
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
        prop_assert!(!failure.recoverable);
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
        prop_assert!(
            events
                .iter()
                .all(|event| event.kind == ResourceEventKind::Activate)
        );
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
        let is_live = callbacks
            .resources
            .get(&key.component)
            .is_some_and(|(live_key, _)| *live_key == key);
        let expected = if is_live {
            vec![ResourceEventKind::Activate]
        } else {
            vec![
                ResourceEventKind::Activate,
                ResourceEventKind::Quiesce,
                ResourceEventKind::Dispose,
            ]
        };
        prop_assert_eq!(kinds, expected);
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
    let scope_owners = executor
        .scope_snapshots()
        .into_iter()
        .map(|scope| {
            let component = TestComponent::from_id(&scope.owner);
            let expected_effects = if component.publishes_capability() {
                vec![
                    format!(
                        "capability:{}",
                        component
                            .capability()
                            .expect("publishing test component should declare a capability")
                    ),
                    RESOURCE_EFFECT.to_string(),
                ]
            } else {
                vec![RESOURCE_EFFECT.to_string()]
            };
            prop_assert_eq!(scope.effects, expected_effects);
            prop_assert!(scope.children.is_empty());
            Ok(component)
        })
        .collect::<Result<BTreeSet<_>, TestCaseError>>()?;
    prop_assert_eq!(scope_owners, active.clone());
    prop_assert_eq!(
        callbacks.resources.keys().copied().collect::<BTreeSet<_>>(),
        active,
    );
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
        prop_assert_eq!(snapshot.state, ComponentState::Active);
        prop_assert_eq!(snapshot.epoch, key.epoch);
        prop_assert!(!resource.is_quiesced);
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
    for sentinel in [ACTIVATION_SENTINEL, QUIESCENCE_SENTINEL, DISPOSER_SENTINEL] {
        prop_assert!(!diagnostic.contains(sentinel));
    }
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
            |executor, callbacks| executor.declare_all(definitions(), callbacks),
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
            |executor, callbacks| executor.declare_all(definitions(), callbacks),
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
            |executor, callbacks| executor.declare_all(definitions(), callbacks),
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
            .declare_all(definitions(), &mut callbacks)
            .expect("fixed acyclic graph must activate");
        let _ = executor.shutdown(&mut callbacks);
        let before = (executor_snapshot(&executor), callbacks.snapshot());

        let result = match mutation {
            PostShutdownMutation::Declare => executor.declare_all(
                [ComponentDefinition::new("extra_component")],
                &mut callbacks,
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
