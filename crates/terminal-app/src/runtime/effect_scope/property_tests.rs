use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Mutex},
};

use proptest::{prelude::*, test_runner::TestCaseResult};

use super::*;

const DISPOSER_SENTINEL: &str = "DISPOSER_SECRET";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum ChildOwner {
    Alpha,
    Beta,
    Gamma,
}

impl ChildOwner {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Alpha => "alpha",
            Self::Beta => "beta",
            Self::Gamma => "gamma",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum EffectLabel {
    First,
    Second,
    Third,
}

impl EffectLabel {
    const fn as_str(self) -> &'static str {
        match self {
            Self::First => "first",
            Self::Second => "second",
            Self::Third => "third",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DisposerOutcome {
    Success,
    Failure,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EffectAction {
    CreateChild(ChildOwner),
    RegisterRoot(EffectLabel, DisposerOutcome),
    RegisterChild(ChildOwner, EffectLabel, DisposerOutcome),
    DisposeChild(ChildOwner),
    DisposeRoot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct EffectIdentity(u32);

#[derive(Debug, Clone, Copy)]
struct RegisteredEffect {
    identity: EffectIdentity,
    outcome: DisposerOutcome,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum ScopeLifecycleModel {
    #[default]
    Active,
    Finalizing,
    Disposed,
}

#[derive(Default)]
struct ScopeModel {
    lifecycle: ScopeLifecycleModel,
    effects: BTreeMap<EffectLabel, RegisteredEffect>,
}

struct EffectModel {
    root: ScopeModel,
    children: BTreeMap<ChildOwner, ScopeModel>,
    next_identity: u32,
    accepted: BTreeSet<EffectIdentity>,
}

impl Default for EffectModel {
    fn default() -> Self {
        Self {
            root: ScopeModel {
                lifecycle: ScopeLifecycleModel::Active,
                ..ScopeModel::default()
            },
            children: BTreeMap::new(),
            next_identity: 0,
            accepted: BTreeSet::new(),
        }
    }
}

impl EffectModel {
    fn allocate_identity(&mut self) -> EffectIdentity {
        let identity = EffectIdentity(self.next_identity);
        self.next_identity += 1;
        identity
    }

    fn dispose_child(&mut self, owner: ChildOwner) -> usize {
        let Some(child) = self.children.get_mut(&owner) else {
            return 0;
        };
        if child.lifecycle == ScopeLifecycleModel::Disposed {
            return 0;
        }
        dispose_scope(child)
    }

    fn dispose_root(&mut self) -> usize {
        if self.root.lifecycle == ScopeLifecycleModel::Disposed {
            return 0;
        }
        let mut failures = dispose_effects(&mut self.root.effects);
        for child in self.children.values_mut() {
            if child.lifecycle != ScopeLifecycleModel::Disposed {
                failures += dispose_scope(child);
            }
        }
        self.root.lifecycle = if self.root.effects.is_empty()
            && self
                .children
                .values()
                .all(|child| child.lifecycle == ScopeLifecycleModel::Disposed)
        {
            ScopeLifecycleModel::Disposed
        } else {
            ScopeLifecycleModel::Finalizing
        };
        failures
    }

    fn active_identities(&self) -> BTreeSet<EffectIdentity> {
        self.root
            .effects
            .values()
            .chain(
                self.children
                    .values()
                    .flat_map(|child| child.effects.values()),
            )
            .map(|effect| effect.identity)
            .collect()
    }
}

#[derive(Default)]
struct ExternalRegistry {
    active: BTreeSet<EffectIdentity>,
    transient_failures: BTreeSet<EffectIdentity>,
    failed_attempts: BTreeSet<EffectIdentity>,
    disposal_counts: BTreeMap<EffectIdentity, usize>,
    missing_disposals: BTreeSet<EffectIdentity>,
}

fn owner_strategy() -> impl Strategy<Value = ChildOwner> {
    prop_oneof![
        Just(ChildOwner::Alpha),
        Just(ChildOwner::Beta),
        Just(ChildOwner::Gamma),
    ]
}

fn label_strategy() -> impl Strategy<Value = EffectLabel> {
    prop_oneof![
        Just(EffectLabel::First),
        Just(EffectLabel::Second),
        Just(EffectLabel::Third),
    ]
}

fn outcome_strategy() -> impl Strategy<Value = DisposerOutcome> {
    prop_oneof![
        3 => Just(DisposerOutcome::Success),
        1 => Just(DisposerOutcome::Failure),
    ]
}

fn action_strategy() -> impl Strategy<Value = EffectAction> {
    prop_oneof![
        owner_strategy().prop_map(EffectAction::CreateChild),
        (label_strategy(), outcome_strategy())
            .prop_map(|(label, outcome)| EffectAction::RegisterRoot(label, outcome)),
        (owner_strategy(), label_strategy(), outcome_strategy()).prop_map(
            |(owner, label, outcome)| EffectAction::RegisterChild(owner, label, outcome),
        ),
        owner_strategy().prop_map(EffectAction::DisposeChild),
        Just(EffectAction::DisposeRoot),
    ]
}

fn register_effect(
    scope: &EffectScope,
    label: EffectLabel,
    outcome: DisposerOutcome,
    identity: EffectIdentity,
    registry: &Arc<Mutex<ExternalRegistry>>,
) -> Result<(), EffectScopeError> {
    let registry_for_dispose = Arc::clone(registry);
    let registration = scope.register(label.as_str(), move || {
        let mut registry = lock_registry(&registry_for_dispose);
        *registry.disposal_counts.entry(identity).or_default() += 1;
        if registry.transient_failures.contains(&identity) {
            registry.failed_attempts.insert(identity);
            return Err(DISPOSER_SENTINEL.to_string());
        }
        if !registry.active.remove(&identity) {
            registry.missing_disposals.insert(identity);
        }
        Ok(())
    });
    if registration.is_ok() {
        let mut registry = lock_registry(registry);
        assert!(registry.active.insert(identity));
        if outcome == DisposerOutcome::Failure {
            registry.transient_failures.insert(identity);
        }
    }
    registration
}

fn apply_action(
    action: EffectAction,
    root: &EffectScope,
    children: &mut BTreeMap<ChildOwner, EffectScope>,
    model: &mut EffectModel,
    registry: &Arc<Mutex<ExternalRegistry>>,
) -> TestCaseResult {
    match action {
        EffectAction::CreateChild(owner) => {
            let expected_success = model.root.lifecycle == ScopeLifecycleModel::Active
                && !model
                    .children
                    .get(&owner)
                    .is_some_and(|child| child.lifecycle != ScopeLifecycleModel::Disposed);
            match root.child(owner.as_str()) {
                Ok(child) => {
                    prop_assert!(expected_success);
                    children.insert(owner, child);
                    model.children.insert(
                        owner,
                        ScopeModel {
                            lifecycle: ScopeLifecycleModel::Active,
                            ..ScopeModel::default()
                        },
                    );
                }
                Err(error) => {
                    prop_assert!(!expected_success);
                    let expected = match model.root.lifecycle {
                        ScopeLifecycleModel::Active => EffectScopeError::DuplicateChild {
                            owner: owner.as_str().to_string(),
                        },
                        ScopeLifecycleModel::Finalizing => EffectScopeError::CleanupPending,
                        ScopeLifecycleModel::Disposed => EffectScopeError::Disposed,
                    };
                    prop_assert_eq!(error, expected);
                }
            }
        }
        EffectAction::RegisterRoot(label, outcome) => {
            let identity = model.allocate_identity();
            let expected_success = model.root.lifecycle == ScopeLifecycleModel::Active
                && !model.root.effects.contains_key(&label);
            let actual = register_effect(root, label, outcome, identity, registry);
            assert_registration_result(actual, expected_success, &model.root, label)?;
            if expected_success {
                model
                    .root
                    .effects
                    .insert(label, RegisteredEffect { identity, outcome });
                model.accepted.insert(identity);
            }
        }
        EffectAction::RegisterChild(owner, label, outcome) => {
            let identity = model.allocate_identity();
            let model_child = model.children.get_mut(&owner);
            let expected_success = model_child.as_ref().is_some_and(|child| {
                child.lifecycle == ScopeLifecycleModel::Active
                    && !child.effects.contains_key(&label)
            });
            let Some(child) = children.get(&owner) else {
                prop_assert!(!expected_success);
                return assert_consistency(root, model, registry);
            };
            let actual = register_effect(child, label, outcome, identity, registry);
            let model_child = model_child.expect("stored child handle must have a model");
            assert_registration_result(actual, expected_success, model_child, label)?;
            if expected_success {
                model_child
                    .effects
                    .insert(label, RegisteredEffect { identity, outcome });
                model.accepted.insert(identity);
            }
        }
        EffectAction::DisposeChild(owner) => {
            let expected_failures = model.dispose_child(owner);
            if let Some(child) = children.get(&owner) {
                assert_dispose_report(&child.dispose(), expected_failures)?;
            } else {
                prop_assert_eq!(expected_failures, 0);
            }
        }
        EffectAction::DisposeRoot => {
            let expected_failures = model.dispose_root();
            assert_dispose_report(&root.dispose(), expected_failures)?;
        }
    }
    assert_consistency(root, model, registry)
}

fn assert_registration_result(
    actual: Result<(), EffectScopeError>,
    expected_success: bool,
    scope: &ScopeModel,
    label: EffectLabel,
) -> TestCaseResult {
    if expected_success {
        prop_assert_eq!(actual, Ok(()));
    } else {
        let expected = match scope.lifecycle {
            ScopeLifecycleModel::Active => EffectScopeError::DuplicateEffect {
                label: label.as_str().to_string(),
            },
            ScopeLifecycleModel::Finalizing => EffectScopeError::CleanupPending,
            ScopeLifecycleModel::Disposed => EffectScopeError::Disposed,
        };
        prop_assert_eq!(actual, Err(expected));
    }
    Ok(())
}

fn assert_dispose_report(report: &EffectDisposeReport, expected_failures: usize) -> TestCaseResult {
    prop_assert_eq!(report.failures.len(), expected_failures);
    let diagnostic = format!("{:?}", report);
    prop_assert!(!diagnostic.contains(DISPOSER_SENTINEL));
    Ok(())
}

fn assert_consistency(
    root: &EffectScope,
    model: &EffectModel,
    registry: &Arc<Mutex<ExternalRegistry>>,
) -> TestCaseResult {
    let snapshot = root.snapshot();
    if model.root.lifecycle == ScopeLifecycleModel::Disposed {
        prop_assert_eq!(snapshot, None);
    } else {
        let snapshot = snapshot.expect("owned root must have a snapshot");
        prop_assert_eq!(snapshot.owner, "runtime_composition");
        prop_assert_eq!(snapshot.lifecycle, snapshot_lifecycle(model.root.lifecycle));
        prop_assert_eq!(snapshot.effects, labels(model.root.effects.keys().copied()));
        let expected_children = model
            .children
            .iter()
            .filter(|(_, child)| child.lifecycle != ScopeLifecycleModel::Disposed)
            .map(|(owner, child)| {
                (
                    owner.as_str().to_string(),
                    snapshot_lifecycle(child.lifecycle),
                    labels(child.effects.keys().copied()),
                )
            })
            .collect::<Vec<_>>();
        let actual_children = snapshot
            .children
            .into_iter()
            .map(|child| (child.owner, child.lifecycle, child.effects))
            .collect::<Vec<_>>();
        prop_assert_eq!(actual_children, expected_children);
    }
    prop_assert_eq!(
        lock_registry(registry).active.clone(),
        model.active_identities()
    );
    prop_assert!(lock_registry(registry).missing_disposals.is_empty());
    Ok(())
}

fn labels(labels: impl IntoIterator<Item = EffectLabel>) -> Vec<String> {
    labels
        .into_iter()
        .map(|label| label.as_str().to_string())
        .collect()
}

fn dispose_scope(scope: &mut ScopeModel) -> usize {
    let failures = dispose_effects(&mut scope.effects);
    scope.lifecycle = if scope.effects.is_empty() {
        ScopeLifecycleModel::Disposed
    } else {
        ScopeLifecycleModel::Finalizing
    };
    failures
}

fn dispose_effects(effects: &mut BTreeMap<EffectLabel, RegisteredEffect>) -> usize {
    let failed_labels = effects
        .iter()
        .filter_map(|(label, effect)| {
            (effect.outcome == DisposerOutcome::Failure).then_some(*label)
        })
        .collect::<Vec<_>>();
    let failures = failed_labels.len();
    effects.retain(|_, effect| effect.outcome == DisposerOutcome::Failure);
    failures
}

fn snapshot_lifecycle(lifecycle: ScopeLifecycleModel) -> EffectScopeLifecycleSnapshot {
    match lifecycle {
        ScopeLifecycleModel::Active => EffectScopeLifecycleSnapshot::Active,
        ScopeLifecycleModel::Finalizing => EffectScopeLifecycleSnapshot::Finalizing,
        ScopeLifecycleModel::Disposed => unreachable!("disposed scope has no snapshot"),
    }
}

fn make_cleanup_succeed(model: &mut EffectModel, registry: &Arc<Mutex<ExternalRegistry>>) {
    for effect in model.root.effects.values_mut().chain(
        model
            .children
            .values_mut()
            .flat_map(|child| child.effects.values_mut()),
    ) {
        effect.outcome = DisposerOutcome::Success;
    }
    lock_registry(registry).transient_failures.clear();
}

fn lock_registry(
    registry: &Arc<Mutex<ExternalRegistry>>,
) -> std::sync::MutexGuard<'_, ExternalRegistry> {
    registry
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 96,
        max_shrink_iters: 4_096,
        ..ProptestConfig::default()
    })]

    #[test]
    fn arbitrary_effect_sequences_restore_the_external_registry(
        actions in prop::collection::vec(action_strategy(), 1..65),
    ) {
        let root = EffectScope::default();
        let mut children = BTreeMap::new();
        let mut model = EffectModel::default();
        let registry = Arc::new(Mutex::new(ExternalRegistry::default()));

        for action in actions {
            apply_action(
                action,
                &root,
                &mut children,
                &mut model,
                &registry,
            )?;
        }

        make_cleanup_succeed(&mut model, &registry);
        let expected_failures = model.dispose_root();
        prop_assert_eq!(expected_failures, 0);
        assert_dispose_report(&root.dispose(), 0)?;
        assert_dispose_report(&root.dispose(), 0)?;
        assert_consistency(&root, &model, &registry)?;

        let registry = lock_registry(&registry);
        prop_assert!(registry.active.is_empty());
        prop_assert!(registry.missing_disposals.is_empty());
        prop_assert_eq!(
            registry.disposal_counts.keys().copied().collect::<BTreeSet<_>>(),
            model.accepted,
        );
        prop_assert!(registry.disposal_counts.values().all(|count| *count >= 1));
        let all_failed_effects_were_retried = registry.failed_attempts.iter().all(|identity| {
            registry
                .disposal_counts
                .get(identity)
                .is_some_and(|count| *count >= 2)
        });
        prop_assert!(all_failed_effects_were_retried);
    }
}
