use std::{
    fmt,
    sync::{Arc, Condvar, Mutex, Weak},
    thread::ThreadId,
};

use thiserror::Error;

type EffectDisposer = Box<dyn FnMut() -> Result<(), String> + Send + 'static>;

/// `EffectScope` 拥有一个 component 的可逆副作用及其 child scope。
///
/// scope 关闭后不能再注册 effect 或 child。显式关闭 child 只撤销该 child 的
/// ownership tree，并把它从 parent 脱离；关闭 parent 则按注册顺序的逆序递归撤销。
pub(super) struct EffectScope {
    shared: Arc<EffectScopeShared>,
    parent: Option<ParentLink>,
}

struct ParentLink {
    shared: Weak<EffectScopeShared>,
    entry_id: usize,
}

struct EffectScopeShared {
    state: Mutex<EffectScopeState>,
    disposal_completed: Condvar,
}

struct EffectScopeState {
    owner: String,
    lifecycle: EffectScopeLifecycle,
    disposal_owner: Option<ThreadId>,
    next_id: usize,
    entries: Vec<ScopeEntry>,
    in_flight: Vec<ScopeEntryInspection>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum EffectScopeLifecycle {
    Active,
    Finalizing,
    Disposed,
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
        shared: Arc<EffectScopeShared>,
    },
}

struct ScopeEntryInspection {
    id: usize,
    kind: ScopeEntryInspectionKind,
}

enum ScopeEntryInspectionKind {
    Effect(String),
    Child(Arc<EffectScopeShared>),
}

#[derive(Debug, Error, PartialEq, Eq)]
pub(super) enum EffectScopeError {
    #[error("effect scope is already disposed")]
    Disposed,
    #[error("effect scope cleanup is pending")]
    CleanupPending,
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

    pub(super) fn is_success(&self) -> bool {
        self.failures.is_empty()
    }
}

/// `EffectScopeSnapshot` 只包含稳定 owner/effect label，不包含 disposer 或资源信息。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct EffectScopeSnapshot {
    pub(super) owner: String,
    pub(super) lifecycle: EffectScopeLifecycleSnapshot,
    pub(super) effects: Vec<String>,
    pub(super) children: Vec<EffectScopeSnapshot>,
}

/// Effect scope inspection 只区分可写与等待 cleanup，不暴露 disposer 状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum EffectScopeLifecycleSnapshot {
    Active,
    Finalizing,
}

impl Default for EffectScope {
    fn default() -> Self {
        Self {
            shared: Arc::new(EffectScopeShared::new("runtime_composition")),
            parent: None,
        }
    }
}

impl EffectScopeShared {
    fn new(owner: impl Into<String>) -> Self {
        Self {
            state: Mutex::new(EffectScopeState::new(owner)),
            disposal_completed: Condvar::new(),
        }
    }
}

impl EffectScopeState {
    fn new(owner: impl Into<String>) -> Self {
        Self {
            owner: owner.into(),
            lifecycle: EffectScopeLifecycle::Active,
            disposal_owner: None,
            next_id: 0,
            entries: Vec::new(),
            in_flight: Vec::new(),
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
        let mut state = lock_state(&self.shared);
        ensure_active(state.lifecycle)?;
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
        let child_shared = Arc::new(EffectScopeShared::new(owner.clone()));
        state.entries.push(ScopeEntry {
            id: entry_id,
            kind: ScopeEntryKind::Child {
                owner,
                shared: Arc::clone(&child_shared),
            },
        });
        Ok(Self {
            shared: child_shared,
            parent: Some(ParentLink {
                shared: Arc::downgrade(&self.shared),
                entry_id,
            }),
        })
    }

    /// 注册当前 scope 的一个 uniquely-labelled reversible effect。
    pub(super) fn register(
        &self,
        label: impl Into<String>,
        disposer: impl FnMut() -> Result<(), String> + Send + 'static,
    ) -> Result<(), EffectScopeError> {
        let label = label.into();
        let mut state = lock_state(&self.shared);
        ensure_active(state.lifecycle)?;
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

    /// 撤销当前 ownership tree；`Finalizing` 时只重试 pending entries，`Disposed` 后为 no-op。
    pub(super) fn dispose(&self) -> EffectDisposeReport {
        let report = dispose_shared(&self.shared);
        if report.is_success() {
            self.detach_from_parent();
        }
        report
    }

    /// 返回当前 active ownership tree 的稳定、脱敏投影。
    pub(super) fn snapshot(&self) -> Option<EffectScopeSnapshot> {
        snapshot_shared(&self.shared)
    }

    #[cfg(test)]
    fn is_active(&self) -> bool {
        lock_state(&self.shared).lifecycle == EffectScopeLifecycle::Active
    }

    fn detach_from_parent(&self) {
        let Some(parent) = &self.parent else {
            return;
        };
        let Some(parent_shared) = parent.shared.upgrade() else {
            return;
        };
        let mut parent_state = lock_state(&parent_shared);
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

fn ensure_active(lifecycle: EffectScopeLifecycle) -> Result<(), EffectScopeError> {
    match lifecycle {
        EffectScopeLifecycle::Active => Ok(()),
        EffectScopeLifecycle::Finalizing => Err(EffectScopeError::CleanupPending),
        EffectScopeLifecycle::Disposed => Err(EffectScopeError::Disposed),
    }
}

fn lock_state(shared: &Arc<EffectScopeShared>) -> std::sync::MutexGuard<'_, EffectScopeState> {
    shared
        .state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn dispose_shared(shared: &Arc<EffectScopeShared>) -> EffectDisposeReport {
    let current_thread = std::thread::current().id();
    let (scope_owner, entries) = loop {
        let mut state = lock_state(shared);
        if state.lifecycle == EffectScopeLifecycle::Disposed {
            return EffectDisposeReport::success();
        }
        match state.disposal_owner {
            Some(owner) if owner == current_thread => {
                return cleanup_in_progress_report(state.owner.clone());
            }
            Some(_) => {
                drop(
                    shared
                        .disposal_completed
                        .wait(state)
                        .unwrap_or_else(std::sync::PoisonError::into_inner),
                );
            }
            None => {
                state.lifecycle = EffectScopeLifecycle::Finalizing;
                state.disposal_owner = Some(current_thread);
                let entries = std::mem::take(&mut state.entries);
                state.in_flight = entries.iter().map(ScopeEntryInspection::from).collect();
                break (state.owner.clone(), entries);
            }
        }
    };

    let mut report = EffectDisposeReport::success();
    let mut pending = Vec::new();
    for mut entry in entries.into_iter().rev() {
        let failed = match &mut entry.kind {
            ScopeEntryKind::Effect { label, disposer } => {
                if let Some(disposer) = disposer.as_mut()
                    && !matches!(
                        std::panic::catch_unwind(std::panic::AssertUnwindSafe(disposer)),
                        Ok(Ok(()))
                    )
                {
                    report.failures.push(EffectDisposeFailure {
                        scope_owner: scope_owner.clone(),
                        effect_label: label.clone(),
                    });
                    true
                } else {
                    false
                }
            }
            ScopeEntryKind::Child {
                owner,
                shared: child,
            } => {
                let child_report = dispose_shared(child);
                let failed = !child_report.is_success();
                if failed && child_report.failures.is_empty() {
                    report.failures.push(EffectDisposeFailure {
                        scope_owner: scope_owner.clone(),
                        effect_label: format!("child:{owner}"),
                    });
                } else {
                    report.append(child_report);
                }
                failed
            }
        };
        if !failed {
            let mut state = lock_state(shared);
            state
                .in_flight
                .retain(|inspection| inspection.id != entry.id);
        }
        if failed {
            pending.push(entry);
        }
    }
    pending.reverse();

    let mut state = lock_state(shared);
    debug_assert!(state.entries.is_empty());
    state.entries = pending;
    state.in_flight.clear();
    state.lifecycle = if state.entries.is_empty() {
        EffectScopeLifecycle::Disposed
    } else {
        EffectScopeLifecycle::Finalizing
    };
    state.disposal_owner = None;
    drop(state);
    shared.disposal_completed.notify_all();
    report
}

fn cleanup_in_progress_report(scope_owner: String) -> EffectDisposeReport {
    EffectDisposeReport {
        failures: vec![EffectDisposeFailure {
            scope_owner,
            effect_label: "cleanup_in_progress".to_string(),
        }],
    }
}

impl From<&ScopeEntry> for ScopeEntryInspection {
    fn from(entry: &ScopeEntry) -> Self {
        let kind = match &entry.kind {
            ScopeEntryKind::Effect { label, .. } => ScopeEntryInspectionKind::Effect(label.clone()),
            ScopeEntryKind::Child { shared, .. } => {
                ScopeEntryInspectionKind::Child(Arc::clone(shared))
            }
        };
        Self { id: entry.id, kind }
    }
}

fn snapshot_shared(shared: &Arc<EffectScopeShared>) -> Option<EffectScopeSnapshot> {
    let (owner, lifecycle, mut effects, children) = {
        let state = lock_state(shared);
        if state.lifecycle == EffectScopeLifecycle::Disposed {
            return None;
        }
        let mut effects = Vec::new();
        let mut children = Vec::new();
        for entry in &state.entries {
            match &entry.kind {
                ScopeEntryKind::Effect { label, .. } => effects.push(label.clone()),
                ScopeEntryKind::Child { shared, .. } => children.push(Arc::clone(shared)),
            }
        }
        for entry in &state.in_flight {
            match &entry.kind {
                ScopeEntryInspectionKind::Effect(label) => effects.push(label.clone()),
                ScopeEntryInspectionKind::Child(shared) => children.push(Arc::clone(shared)),
            }
        }
        let lifecycle = match state.lifecycle {
            EffectScopeLifecycle::Active => EffectScopeLifecycleSnapshot::Active,
            EffectScopeLifecycle::Finalizing => EffectScopeLifecycleSnapshot::Finalizing,
            EffectScopeLifecycle::Disposed => unreachable!("disposed scope returned above"),
        };
        (state.owner.clone(), lifecycle, effects, children)
    };

    effects.sort();
    let mut children = children
        .into_iter()
        .filter_map(|child| snapshot_shared(&child))
        .collect::<Vec<_>>();
    children.sort_by(|left, right| left.owner.cmp(&right.owner));
    Some(EffectScopeSnapshot {
        owner,
        lifecycle,
        effects,
        children,
    })
}

#[cfg(test)]
mod property_tests;

#[cfg(test)]
mod tests {
    use std::{
        sync::{
            Arc, Mutex,
            atomic::{AtomicUsize, Ordering},
            mpsc,
        },
        time::Duration,
    };

    use super::*;

    fn recorded_order() -> Arc<Mutex<Vec<String>>> {
        Arc::new(Mutex::new(Vec::new()))
    }

    fn record(
        order: &Arc<Mutex<Vec<String>>>,
        label: &'static str,
        failure: Option<&'static str>,
    ) -> impl FnMut() -> Result<(), String> + Send + 'static {
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
                lifecycle: EffectScopeLifecycleSnapshot::Active,
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
                },
                EffectDisposeFailure {
                    scope_owner: "a".to_string(),
                    effect_label: "failure".to_string(),
                },
            ]
        );
        let snapshot = root.snapshot().expect("failed effects must remain owned");
        assert_eq!(snapshot.lifecycle, EffectScopeLifecycleSnapshot::Finalizing);
        assert_eq!(snapshot.children.len(), 2);
        assert!(snapshot.children.iter().all(|child| {
            child.lifecycle == EffectScopeLifecycleSnapshot::Finalizing
                && child.effects == ["failure"]
        }));
        assert_eq!(
            root.register("late", || Ok(())),
            Err(EffectScopeError::CleanupPending)
        );
    }

    #[test]
    fn failed_child_cleanup_stays_attached_until_retry_succeeds() {
        let root = EffectScope::default();
        let child = root.child("replaceable").unwrap();
        let attempts = Arc::new(AtomicUsize::new(0));
        let attempts_for_disposer = Arc::clone(&attempts);
        child
            .register("transient", move || {
                if attempts_for_disposer.fetch_add(1, Ordering::SeqCst) == 0 {
                    Err("SECRET_TRANSIENT_FAILURE".to_string())
                } else {
                    Ok(())
                }
            })
            .unwrap();

        let failure = child.dispose();
        assert_eq!(failure.failures.len(), 1);
        assert_eq!(
            child.register("late", || Ok(())),
            Err(EffectScopeError::CleanupPending)
        );
        assert!(matches!(
            root.child("replaceable"),
            Err(EffectScopeError::DuplicateChild { .. })
        ));
        let snapshot = root.snapshot().unwrap();
        assert_eq!(
            snapshot.children[0].lifecycle,
            EffectScopeLifecycleSnapshot::Finalizing
        );
        assert!(!format!("{failure:?}").contains("SECRET_TRANSIENT_FAILURE"));

        assert!(root.dispose().is_success());
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
        assert!(root.snapshot().is_none());
        assert!(child.snapshot().is_none());
    }

    #[test]
    fn concurrent_dispose_calls_serialize_retryable_disposer() {
        let scope = Arc::new(EffectScope::default());
        let attempts = Arc::new(AtomicUsize::new(0));
        let in_flight = Arc::new(AtomicUsize::new(0));
        let max_in_flight = Arc::new(AtomicUsize::new(0));
        let attempts_for_disposer = Arc::clone(&attempts);
        let in_flight_for_disposer = Arc::clone(&in_flight);
        let max_for_disposer = Arc::clone(&max_in_flight);
        scope
            .register("serialized", move || {
                let active = in_flight_for_disposer.fetch_add(1, Ordering::SeqCst) + 1;
                max_for_disposer.fetch_max(active, Ordering::SeqCst);
                std::thread::sleep(Duration::from_millis(10));
                in_flight_for_disposer.fetch_sub(1, Ordering::SeqCst);
                if attempts_for_disposer.fetch_add(1, Ordering::SeqCst) == 0 {
                    Err("transient".to_string())
                } else {
                    Ok(())
                }
            })
            .unwrap();

        let left = Arc::clone(&scope);
        let right = Arc::clone(&scope);
        let left = std::thread::spawn(move || left.dispose());
        let right = std::thread::spawn(move || right.dispose());
        let reports = [left.join().unwrap(), right.join().unwrap()];

        assert_eq!(
            reports
                .iter()
                .map(|report| report.failures.len())
                .sum::<usize>(),
            1
        );
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
        assert_eq!(max_in_flight.load(Ordering::SeqCst), 1);
        assert!(scope.snapshot().is_none());
    }

    #[test]
    fn reentrant_dispose_reports_pending_without_holding_a_scope_lock() {
        let scope = Arc::new(EffectScope::default());
        let reentrant_scope = Arc::downgrade(&scope);
        scope
            .register("reentrant", move || {
                let active_scope = reentrant_scope
                    .upgrade()
                    .expect("scope should remain owned during disposal");
                let snapshot = active_scope
                    .snapshot()
                    .expect("in-flight cleanup should remain inspectable");
                assert_eq!(snapshot.lifecycle, EffectScopeLifecycleSnapshot::Finalizing);
                assert_eq!(snapshot.effects, ["reentrant"]);
                let report = active_scope.dispose();
                assert!(!report.is_success());
                assert_eq!(report.failures[0].effect_label, "cleanup_in_progress");
                Ok(())
            })
            .unwrap();

        let (completed_tx, completed_rx) = mpsc::channel();
        let disposed_scope = Arc::clone(&scope);
        let disposer = std::thread::spawn(move || {
            completed_tx
                .send(disposed_scope.dispose().is_success())
                .expect("test receiver should remain connected");
        });

        assert_eq!(
            completed_rx.recv_timeout(Duration::from_secs(1)),
            Ok(true),
            "reentrant disposal must not deadlock"
        );
        disposer.join().expect("disposal thread should finish");
        assert!(scope.snapshot().is_none());
    }

    #[test]
    fn in_flight_snapshot_removes_each_successful_inverse_immediately() {
        let scope = Arc::new(EffectScope::default());
        let inspected_scope = Arc::downgrade(&scope);
        scope
            .register("earlier", move || {
                let snapshot = inspected_scope
                    .upgrade()
                    .expect("scope should remain owned during disposal")
                    .snapshot()
                    .expect("current inverse should remain inspectable");
                assert_eq!(snapshot.lifecycle, EffectScopeLifecycleSnapshot::Finalizing);
                assert_eq!(snapshot.effects, ["earlier"]);
                Ok(())
            })
            .unwrap();
        scope.register("later", || Ok(())).unwrap();

        assert!(scope.dispose().is_success());
        assert!(scope.snapshot().is_none());
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
