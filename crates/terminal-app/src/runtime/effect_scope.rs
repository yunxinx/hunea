use std::{
    fmt,
    sync::{Arc, Mutex, Weak},
};

use thiserror::Error;

type EffectDisposer = Box<dyn FnOnce() -> Result<(), String> + Send + 'static>;

/// `EffectScope` 拥有一个 component 的可逆副作用及其 child scope。
///
/// scope 关闭后不能再注册 effect 或 child。显式关闭 child 只撤销该 child 的
/// ownership tree，并把它从 parent 脱离；关闭 parent 则按注册顺序的逆序递归撤销。
pub(super) struct EffectScope {
    state: Arc<Mutex<EffectScopeState>>,
    parent: Option<ParentLink>,
}

struct ParentLink {
    state: Weak<Mutex<EffectScopeState>>,
    entry_id: usize,
}

struct EffectScopeState {
    owner: String,
    active: bool,
    next_id: usize,
    entries: Vec<ScopeEntry>,
}

struct ScopeEntry {
    id: usize,
    kind: ScopeEntryKind,
}

enum ScopeEntryKind {
    Effect {
        label: String,
        disposer: Option<EffectDisposer>,
    },
    Child {
        owner: String,
        state: Arc<Mutex<EffectScopeState>>,
    },
}

#[derive(Debug, Error, PartialEq, Eq)]
pub(super) enum EffectScopeError {
    #[error("effect scope is already disposed")]
    Disposed,
    #[error("effect scope already contains child owner `{owner}`")]
    DuplicateChild { owner: String },
    #[error("effect scope already contains effect `{label}`")]
    DuplicateEffect { label: String },
    #[error("effect scope entry identity is exhausted")]
    EntryIdExhausted,
}

#[derive(Clone, PartialEq, Eq)]
pub(super) struct EffectDisposeFailure {
    pub(super) scope_owner: String,
    pub(super) effect_label: String,
    pub(super) message: String,
}

impl fmt::Debug for EffectDisposeFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EffectDisposeFailure")
            .field("scope_owner", &self.scope_owner)
            .field("effect_label", &self.effect_label)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, PartialEq, Eq)]
pub(super) struct EffectDisposeReport {
    pub(super) failures: Vec<EffectDisposeFailure>,
}

impl fmt::Debug for EffectDisposeReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EffectDisposeReport")
            .field("failures", &self.failures)
            .finish()
    }
}

impl EffectDisposeReport {
    fn success() -> Self {
        Self {
            failures: Vec::new(),
        }
    }

    fn append(&mut self, mut other: Self) {
        self.failures.append(&mut other.failures);
    }

    #[cfg(test)]
    fn is_success(&self) -> bool {
        self.failures.is_empty()
    }

    pub(super) fn error_message(&self) -> Option<String> {
        (!self.failures.is_empty()).then(|| {
            self.failures
                .iter()
                .map(|failure| {
                    format!(
                        "effect scope {} effect {} failed to dispose: {}",
                        failure.scope_owner, failure.effect_label, failure.message
                    )
                })
                .collect::<Vec<_>>()
                .join("; ")
        })
    }
}

/// `EffectScopeSnapshot` 只包含稳定 owner/effect label，不包含 disposer 或资源信息。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct EffectScopeSnapshot {
    pub(super) owner: String,
    pub(super) effects: Vec<String>,
    pub(super) children: Vec<EffectScopeSnapshot>,
}

impl Default for EffectScope {
    fn default() -> Self {
        Self {
            state: Arc::new(Mutex::new(EffectScopeState::new("runtime_composition"))),
            parent: None,
        }
    }
}

impl EffectScopeState {
    fn new(owner: impl Into<String>) -> Self {
        Self {
            owner: owner.into(),
            active: true,
            next_id: 0,
            entries: Vec::new(),
        }
    }

    fn allocate_entry_id(&mut self) -> Result<usize, EffectScopeError> {
        let id = self.next_id;
        let next_id = id
            .checked_add(1)
            .ok_or(EffectScopeError::EntryIdExhausted)?;
        self.next_id = next_id;
        Ok(id)
    }
}

impl EffectScope {
    #[cfg(test)]
    fn activate<T>(
        activate: impl FnOnce(&Self) -> Result<T, String>,
    ) -> Result<(Self, T), (String, EffectDisposeReport)> {
        let scope = Self::default();
        match activate(&scope) {
            Ok(value) => Ok((scope, value)),
            Err(message) => {
                let report = scope.dispose();
                Err((message, report))
            }
        }
    }

    /// 创建由当前 scope 拥有的 active child scope。
    pub(super) fn child(&self, owner: impl Into<String>) -> Result<Self, EffectScopeError> {
        let owner = owner.into();
        let mut state = lock_state(&self.state);
        if !state.active {
            return Err(EffectScopeError::Disposed);
        }
        if state.entries.iter().any(|entry| {
            matches!(
                &entry.kind,
                ScopeEntryKind::Child {
                    owner: registered_owner,
                    ..
                } if registered_owner == &owner
            )
        }) {
            return Err(EffectScopeError::DuplicateChild { owner });
        }

        let entry_id = state.allocate_entry_id()?;
        let child_state = Arc::new(Mutex::new(EffectScopeState::new(owner.clone())));
        state.entries.push(ScopeEntry {
            id: entry_id,
            kind: ScopeEntryKind::Child {
                owner,
                state: Arc::clone(&child_state),
            },
        });
        Ok(Self {
            state: child_state,
            parent: Some(ParentLink {
                state: Arc::downgrade(&self.state),
                entry_id,
            }),
        })
    }

    /// 注册当前 scope 的一个 uniquely-labelled reversible effect。
    pub(super) fn register(
        &self,
        label: impl Into<String>,
        disposer: impl FnOnce() -> Result<(), String> + Send + 'static,
    ) -> Result<(), EffectScopeError> {
        let label = label.into();
        let mut state = lock_state(&self.state);
        if !state.active {
            return Err(EffectScopeError::Disposed);
        }
        if state.entries.iter().any(|entry| {
            matches!(
                &entry.kind,
                ScopeEntryKind::Effect {
                    label: registered_label,
                    ..
                } if registered_label == &label
            )
        }) {
            return Err(EffectScopeError::DuplicateEffect { label });
        }

        let id = state.allocate_entry_id()?;
        state.entries.push(ScopeEntry {
            id,
            kind: ScopeEntryKind::Effect {
                label,
                disposer: Some(Box::new(disposer)),
            },
        });
        Ok(())
    }

    /// 撤销当前 ownership tree；重复调用是 no-op。
    pub(super) fn dispose(&self) -> EffectDisposeReport {
        let report = dispose_state(&self.state);
        self.detach_from_parent();
        report
    }

    /// 返回当前 active ownership tree 的稳定、脱敏投影。
    pub(super) fn snapshot(&self) -> Option<EffectScopeSnapshot> {
        snapshot_state(&self.state)
    }

    #[cfg(test)]
    fn is_active(&self) -> bool {
        lock_state(&self.state).active
    }

    fn detach_from_parent(&self) {
        let Some(parent) = &self.parent else {
            return;
        };
        let Some(parent_state) = parent.state.upgrade() else {
            return;
        };
        let mut parent_state = lock_state(&parent_state);
        if let Some(index) = parent_state
            .entries
            .iter()
            .position(|entry| entry.id == parent.entry_id)
        {
            parent_state.entries.remove(index);
        }
    }
}

impl Drop for EffectScope {
    fn drop(&mut self) {
        let _ = self.dispose();
    }
}

fn lock_state(state: &Arc<Mutex<EffectScopeState>>) -> std::sync::MutexGuard<'_, EffectScopeState> {
    state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn dispose_state(state: &Arc<Mutex<EffectScopeState>>) -> EffectDisposeReport {
    let (scope_owner, entries) = {
        let mut state = lock_state(state);
        if !state.active {
            return EffectDisposeReport::success();
        }
        state.active = false;
        (state.owner.clone(), std::mem::take(&mut state.entries))
    };

    let mut report = EffectDisposeReport::success();
    for entry in entries.into_iter().rev() {
        match entry.kind {
            ScopeEntryKind::Effect {
                label,
                mut disposer,
            } => {
                if let Some(disposer) = disposer.take()
                    && let Err(message) = disposer()
                {
                    report.failures.push(EffectDisposeFailure {
                        scope_owner: scope_owner.clone(),
                        effect_label: label,
                        message,
                    });
                }
            }
            ScopeEntryKind::Child { state, .. } => report.append(dispose_state(&state)),
        }
    }
    report
}

fn snapshot_state(state: &Arc<Mutex<EffectScopeState>>) -> Option<EffectScopeSnapshot> {
    let (owner, mut effects, children) = {
        let state = lock_state(state);
        if !state.active {
            return None;
        }
        let mut effects = Vec::new();
        let mut children = Vec::new();
        for entry in &state.entries {
            match &entry.kind {
                ScopeEntryKind::Effect { label, .. } => effects.push(label.clone()),
                ScopeEntryKind::Child { state, .. } => children.push(Arc::clone(state)),
            }
        }
        (state.owner.clone(), effects, children)
    };

    effects.sort();
    let mut children = children
        .into_iter()
        .filter_map(|child| snapshot_state(&child))
        .collect::<Vec<_>>();
    children.sort_by(|left, right| left.owner.cmp(&right.owner));
    Some(EffectScopeSnapshot {
        owner,
        effects,
        children,
    })
}

#[cfg(test)]
mod property_tests;

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;

    fn recorded_order() -> Arc<Mutex<Vec<String>>> {
        Arc::new(Mutex::new(Vec::new()))
    }

    fn record(
        order: &Arc<Mutex<Vec<String>>>,
        label: &'static str,
        failure: Option<&'static str>,
    ) -> impl FnOnce() -> Result<(), String> + Send + 'static {
        let order = Arc::clone(order);
        move || {
            order
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(label.to_string());
            failure.map_or(Ok(()), |message| Err(message.to_string()))
        }
    }

    fn scope_error(result: Result<EffectScope, EffectScopeError>) -> EffectScopeError {
        match result {
            Ok(_) => panic!("scope operation should fail"),
            Err(error) => error,
        }
    }

    #[test]
    fn child_disposal_is_lifo_and_leaves_sibling_and_parent_active() {
        let order = recorded_order();
        let root = EffectScope::default();
        let child_a = root.child("a").expect("child a should register");
        let child_b = root.child("b").expect("child b should register");
        child_a
            .register("first", record(&order, "a:first", None))
            .expect("first effect should register");
        child_a
            .register("second", record(&order, "a:second", None))
            .expect("second effect should register");

        assert!(child_a.dispose().is_success());
        assert_eq!(
            *order
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            vec!["a:second", "a:first"]
        );
        assert!(root.is_active());
        assert!(child_b.is_active());
        child_b
            .register("only", record(&order, "b:only", None))
            .expect("sibling should remain writable");
        assert!(child_b.dispose().is_success());
        assert!(root.is_active());
    }

    #[test]
    fn parent_disposal_recurses_in_reverse_registration_order() {
        let order = recorded_order();
        let root = EffectScope::default();
        root.register("root-first", record(&order, "root:first", None))
            .unwrap();
        let child_a = root.child("a").unwrap();
        child_a
            .register("a-first", record(&order, "a:first", None))
            .unwrap();
        let child_b = root.child("b").unwrap();
        child_b
            .register("b-first", record(&order, "b:first", None))
            .unwrap();
        root.register("root-last", record(&order, "root:last", None))
            .unwrap();

        assert!(root.dispose().is_success());
        assert_eq!(
            *order
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            vec!["root:last", "b:first", "a:first", "root:first"]
        );
        assert!(!child_a.is_active());
        assert!(!child_b.is_active());
        assert!(root.dispose().is_success());
        assert!(child_a.dispose().is_success());
    }

    #[test]
    fn explicit_child_disposal_detaches_and_allows_same_owner_replacement() {
        let root = EffectScope::default();
        let child = root.child("replaceable").unwrap();
        child.register("old", || Ok(())).unwrap();
        assert!(child.dispose().is_success());

        let snapshot = root.snapshot().expect("root should remain active");
        assert!(snapshot.children.is_empty());
        let replacement = root
            .child("replaceable")
            .expect("disposed child owner should be reusable");
        replacement.register("fresh", || Ok(())).unwrap();
        assert_eq!(
            root.snapshot().unwrap().children,
            vec![EffectScopeSnapshot {
                owner: "replaceable".to_string(),
                effects: vec!["fresh".to_string()],
                children: Vec::new(),
            }]
        );
    }

    #[test]
    fn duplicate_and_disposed_registration_fail_before_scope_mutation() {
        let root = EffectScope::default();
        let child = root.child("owner").unwrap();
        child.register("effect", || Ok(())).unwrap();
        let before = root.snapshot();

        assert_eq!(
            scope_error(root.child("owner")),
            EffectScopeError::DuplicateChild {
                owner: "owner".to_string(),
            }
        );
        assert_eq!(
            child
                .register("effect", || Ok(()))
                .expect_err("duplicate effect must fail"),
            EffectScopeError::DuplicateEffect {
                label: "effect".to_string(),
            }
        );
        assert_eq!(root.snapshot(), before);

        child.dispose();
        assert_eq!(
            child
                .register("late", || Ok(()))
                .expect_err("disposed child must reject effects"),
            EffectScopeError::Disposed
        );
        root.dispose();
        assert_eq!(scope_error(root.child("late")), EffectScopeError::Disposed);
    }

    #[test]
    fn nested_failures_do_not_skip_effects_or_siblings_and_preserve_order() {
        let order = recorded_order();
        let root = EffectScope::default();
        let child_a = root.child("a").unwrap();
        child_a
            .register("success", record(&order, "a:success", None))
            .unwrap();
        child_a
            .register("failure", record(&order, "a:failure", Some("a failed")))
            .unwrap();
        let child_b = root.child("b").unwrap();
        child_b
            .register("failure", record(&order, "b:failure", Some("b failed")))
            .unwrap();

        let report = root.dispose();

        assert_eq!(
            *order
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            vec!["b:failure", "a:failure", "a:success"]
        );
        assert_eq!(
            report.failures,
            vec![
                EffectDisposeFailure {
                    scope_owner: "b".to_string(),
                    effect_label: "failure".to_string(),
                    message: "b failed".to_string(),
                },
                EffectDisposeFailure {
                    scope_owner: "a".to_string(),
                    effect_label: "failure".to_string(),
                    message: "a failed".to_string(),
                },
            ]
        );
    }

    #[test]
    fn failed_activation_rolls_back_registered_effects() {
        let order = recorded_order();
        let activation = EffectScope::activate(|scope| {
            scope
                .register("worker", record(&order, "worker", None))
                .unwrap();
            scope
                .register("wake", record(&order, "wake", None))
                .unwrap();
            Err::<(), _>("provider activation failed".to_string())
        });
        let (activation_error, report) = match activation {
            Ok(_) => panic!("activation should fail"),
            Err(failure) => failure,
        };

        assert!(report.is_success());
        assert_eq!(
            *order
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            vec!["wake", "worker"]
        );
        assert_eq!(activation_error, "provider activation failed");
    }

    #[test]
    fn snapshot_is_sorted_and_contains_only_safe_metadata() {
        let root = EffectScope::default();
        let child_z = root.child("z-owner").unwrap();
        let child_a = root.child("a-owner").unwrap();
        child_z.register("z-effect", || Ok(())).unwrap();
        child_z.register("a-effect", || Ok(())).unwrap();
        child_a
            .register("safe-label", || Err("SECRET_ERROR".to_string()))
            .unwrap();

        let snapshot = root.snapshot().unwrap();
        assert_eq!(
            snapshot
                .children
                .iter()
                .map(|child| child.owner.as_str())
                .collect::<Vec<_>>(),
            vec!["a-owner", "z-owner"]
        );
        assert_eq!(
            snapshot.children[1].effects,
            vec!["a-effect".to_string(), "z-effect".to_string()]
        );
        let diagnostic = format!("{snapshot:?}");
        assert!(!diagnostic.contains("SECRET_ERROR"));
        assert!(!diagnostic.contains("entry_id"));
        assert!(!diagnostic.contains("disposer"));
    }
}
