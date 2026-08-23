use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    sync::{Arc, Mutex},
};

use thiserror::Error;

/// `EffectScope` 拥有一个 runtime component 注册的全部可逆副作用。
///
/// 注册成功后，effect 只通过 scope 的 dispose 路径撤销；scope 关闭后不能再注册新
/// effect。这个同步版本用于先固定 ownership、逆序和错误聚合语义，异步 worker 会在
/// 后续切片把 quiescence 封装进单个 disposer。
#[derive(Default)]
pub(super) struct EffectScope {
    state: Arc<Mutex<EffectScopeState>>,
}

struct EffectScopeState {
    active: bool,
    next_id: usize,
    effects: Vec<RegisteredEffect>,
}

struct RegisteredEffect {
    id: usize,
    owner: String,
    disposer: Option<Box<dyn FnOnce() -> Result<(), String> + Send + 'static>>,
}

impl Default for EffectScopeState {
    fn default() -> Self {
        Self {
            active: true,
            next_id: 0,
            effects: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct EffectId(usize);

#[derive(Debug, Error, PartialEq, Eq)]
pub(super) enum EffectScopeError {
    #[error("effect scope is already disposed")]
    Disposed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct EffectDisposeFailure {
    pub(super) id: EffectId,
    pub(super) owner: String,
    pub(super) message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct EffectDisposeReport {
    pub(super) failures: Vec<EffectDisposeFailure>,
}

impl EffectDisposeReport {
    fn success() -> Self {
        Self {
            failures: Vec::new(),
        }
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
                        "effect {} ({}) failed to dispose: {}",
                        failure.id.0, failure.owner, failure.message
                    )
                })
                .collect::<Vec<_>>()
                .join("; ")
        })
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

    pub(super) fn register(
        &self,
        owner: impl Into<String>,
        disposer: impl FnOnce() -> Result<(), String> + Send + 'static,
    ) -> Result<EffectId, EffectScopeError> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !state.active {
            return Err(EffectScopeError::Disposed);
        }
        let id = EffectId(state.next_id);
        state.next_id += 1;
        state.effects.push(RegisteredEffect {
            id: id.0,
            owner: owner.into(),
            disposer: Some(Box::new(disposer)),
        });
        Ok(id)
    }

    #[cfg(test)]
    pub(super) fn is_active(&self) -> bool {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .active
    }

    pub(super) fn dispose(&self) -> EffectDisposeReport {
        let effects = {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if !state.active {
                return EffectDisposeReport::success();
            }
            state.active = false;
            std::mem::take(&mut state.effects)
        };

        let mut report = EffectDisposeReport::success();
        for mut effect in effects.into_iter().rev() {
            let Some(disposer) = effect.disposer.take() else {
                continue;
            };
            if let Err(message) = disposer() {
                report.failures.push(EffectDisposeFailure {
                    id: EffectId(effect.id),
                    owner: effect.owner,
                    message,
                });
            }
        }
        report
    }

    /// 撤销一个 owner effect；重复撤销视为成功。
    pub(super) fn dispose_effect(&self, id: EffectId) -> EffectDisposeReport {
        let effect = {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let Some(index) = state.effects.iter().position(|effect| effect.id == id.0) else {
                return EffectDisposeReport::success();
            };
            state.effects.remove(index)
        };

        let mut report = EffectDisposeReport::success();
        if let Some(disposer) = effect.disposer
            && let Err(message) = disposer()
        {
            report.failures.push(EffectDisposeFailure {
                id,
                owner: effect.owner,
                message,
            });
        }
        report
    }
}

impl Drop for EffectScope {
    fn drop(&mut self) {
        let _ = self.dispose();
    }
}

/// `CapabilityKey` 是 component 声明依赖时使用的稳定 typed key。
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(super) struct CapabilityKey(String);

impl From<&str> for CapabilityKey {
    fn from(value: &str) -> Self {
        Self(value.to_string())
    }
}

impl From<String> for CapabilityKey {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl fmt::Display for CapabilityKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ComponentState {
    Declared,
    Pending,
    Activating,
    Active,
    Deactivating,
    Disposed,
}

impl ComponentState {
    pub(super) const fn as_str(self) -> &'static str {
        match self {
            Self::Declared => "declared",
            Self::Pending => "pending",
            Self::Activating => "activating",
            Self::Active => "active",
            Self::Deactivating => "deactivating",
            Self::Disposed => "disposed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ComponentDefinition {
    pub(super) id: String,
    pub(super) required: BTreeSet<CapabilityKey>,
    pub(super) optional: BTreeSet<CapabilityKey>,
}

impl ComponentDefinition {
    pub(super) fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            required: BTreeSet::new(),
            optional: BTreeSet::new(),
        }
    }

    pub(super) fn requires(mut self, key: impl Into<CapabilityKey>) -> Self {
        self.required.insert(key.into());
        self
    }

    pub(super) fn observes(mut self, key: impl Into<CapabilityKey>) -> Self {
        self.optional.insert(key.into());
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ComponentTransition {
    pub(super) component_id: String,
    pub(super) from: ComponentState,
    pub(super) to: ComponentState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct CapabilitySnapshot {
    pub(super) key: String,
    pub(super) generation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct OptionalCapabilitySnapshot {
    pub(super) key: String,
    pub(super) available: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ComponentSnapshot {
    pub(super) id: String,
    pub(super) state: ComponentState,
    pub(super) required: Vec<String>,
    pub(super) optional: Vec<OptionalCapabilitySnapshot>,
}

struct ComponentRecord {
    definition: ComponentDefinition,
    state: ComponentState,
}

/// `ComponentGraph` 是最小 reactive coeffect resolver。
///
/// capability 的 add/remove 会立即重新计算所有声明过的 component。component id 与
/// capability key 都按 `BTree*` 排序，因此同一组变更始终产生确定性生命周期顺序。
#[derive(Default)]
pub(super) struct ComponentGraph {
    capabilities: BTreeSet<CapabilityKey>,
    capability_generations: BTreeMap<CapabilityKey, u64>,
    components: BTreeMap<String, ComponentRecord>,
    transitions: Vec<ComponentTransition>,
}

impl ComponentGraph {
    pub(super) fn declare(&mut self, definition: ComponentDefinition) {
        let id = definition.id.clone();
        self.components.insert(
            id,
            ComponentRecord {
                definition,
                state: ComponentState::Declared,
            },
        );
        self.reconcile();
    }

    pub(super) fn add_capability(&mut self, key: impl Into<CapabilityKey>) {
        let key = key.into();
        if self.capabilities.insert(key.clone()) {
            self.capability_generations.entry(key).or_insert(0);
        }
        self.reconcile();
    }

    #[cfg(test)]
    pub(super) fn remove_capability(&mut self, key: &CapabilityKey) {
        self.capabilities.remove(key);
        self.reconcile();
    }

    /// 替换同一 capability 的 provider generation，而不是改变依赖 key。
    ///
    /// 依赖方必须先离开 `Active`，旧 owner 的 effect 才能被释放；随后 resolver 才能
    /// 为新 generation 创建唯一的 active owner。
    #[cfg(test)]
    pub(super) fn replace_capability(&mut self, key: &CapabilityKey) {
        if !self.capabilities.contains(key) {
            self.add_capability(key.clone());
            return;
        }

        self.capability_generations
            .entry(key.clone())
            .and_modify(|generation| *generation = generation.saturating_add(1))
            .or_insert(1);

        let ids = self
            .components
            .iter()
            .filter(|(_, record)| {
                record.state == ComponentState::Active && record.definition.required.contains(key)
            })
            .map(|(id, _)| id.clone())
            .collect::<Vec<_>>();
        for id in ids {
            self.transition(&id, ComponentState::Deactivating);
            self.transition(&id, ComponentState::Disposed);
        }
        self.reconcile();
    }

    #[cfg(test)]
    pub(super) fn state(&self, component_id: &str) -> Option<ComponentState> {
        self.components.get(component_id).map(|record| record.state)
    }

    #[cfg(test)]
    pub(super) fn optional_available(
        &self,
        component_id: &str,
        key: &CapabilityKey,
    ) -> Option<bool> {
        let record = self.components.get(component_id)?;
        if !record.definition.optional.contains(key) {
            return None;
        }
        Some(self.capabilities.contains(key))
    }

    pub(super) fn take_transitions(&mut self) -> Vec<ComponentTransition> {
        std::mem::take(&mut self.transitions)
    }

    pub(super) fn capabilities(&self) -> Vec<CapabilitySnapshot> {
        self.capabilities
            .iter()
            .map(|key| CapabilitySnapshot {
                key: key.to_string(),
                generation: self.capability_generations.get(key).copied().unwrap_or(0),
            })
            .collect()
    }

    pub(super) fn components(&self) -> Vec<ComponentSnapshot> {
        self.components
            .iter()
            .map(|(id, record)| ComponentSnapshot {
                id: id.clone(),
                state: record.state,
                required: record
                    .definition
                    .required
                    .iter()
                    .map(ToString::to_string)
                    .collect(),
                optional: record
                    .definition
                    .optional
                    .iter()
                    .map(|key| OptionalCapabilitySnapshot {
                        key: key.to_string(),
                        available: self.capabilities.contains(key),
                    })
                    .collect(),
            })
            .collect()
    }

    fn reconcile(&mut self) {
        let ids: Vec<String> = self.components.keys().cloned().collect();
        for id in ids {
            let required_ready = self
                .components
                .get(&id)
                .is_some_and(|record| record.definition.required.is_subset(&self.capabilities));
            let state = self
                .components
                .get(&id)
                .map(|record| record.state)
                .expect("component id came from component map");
            match (state, required_ready) {
                (
                    ComponentState::Declared | ComponentState::Pending | ComponentState::Disposed,
                    true,
                ) => {
                    self.transition(&id, ComponentState::Activating);
                    self.transition(&id, ComponentState::Active);
                }
                (
                    ComponentState::Declared | ComponentState::Pending | ComponentState::Disposed,
                    false,
                ) => {
                    if state != ComponentState::Pending {
                        self.transition(&id, ComponentState::Pending);
                    }
                }
                (ComponentState::Active, false) => {
                    self.transition(&id, ComponentState::Deactivating);
                    self.transition(&id, ComponentState::Disposed);
                    self.transition(&id, ComponentState::Pending);
                }
                _ => {}
            }
        }
    }

    fn transition(&mut self, id: &str, to: ComponentState) {
        let record = self
            .components
            .get_mut(id)
            .expect("component id should remain declared");
        let from = record.state;
        record.state = to;
        self.transitions.push(ComponentTransition {
            component_id: id.to_string(),
            from,
            to,
        });
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;

    #[test]
    fn effect_scope_rolls_back_in_reverse_registration_order() {
        let order = Arc::new(Mutex::new(Vec::new()));
        let scope = EffectScope::default();
        for label in ["one", "two", "three"] {
            let order = Arc::clone(&order);
            scope
                .register(label, move || {
                    order
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .push(label.to_string());
                    Ok(())
                })
                .expect("active scope accepts effects");
        }

        assert!(scope.dispose().is_success());
        assert_eq!(
            *order
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            vec!["three", "two", "one"]
        );
        assert!(!scope.is_active());
    }

    #[test]
    fn effect_scope_continues_after_a_disposer_failure_and_is_idempotent() {
        let order = Arc::new(Mutex::new(Vec::new()));
        let scope = EffectScope::default();
        let order_for_failure = Arc::clone(&order);
        scope
            .register("first", move || {
                order_for_failure
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push("first");
                Err("first failed".to_string())
            })
            .unwrap();
        let order_for_success = Arc::clone(&order);
        scope
            .register("second", move || {
                order_for_success
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push("second");
                Ok(())
            })
            .unwrap();

        let report = scope.dispose();
        assert_eq!(report.failures.len(), 1);
        assert_eq!(report.failures[0].owner, "first");
        assert_eq!(
            *order
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            vec!["second", "first"]
        );
        assert!(scope.dispose().is_success());
        assert!(matches!(
            scope.register("late", || Ok(())),
            Err(EffectScopeError::Disposed)
        ));
    }

    #[test]
    fn failed_activation_rolls_back_effects_that_were_registered_before_failure() {
        let order = Arc::new(Mutex::new(Vec::new()));
        let activation = EffectScope::activate(|scope| {
            for label in ["worker", "wake"] {
                let order = Arc::clone(&order);
                scope
                    .register(label, move || {
                        order
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .push(label.to_string());
                        Ok(())
                    })
                    .expect("activation should register its effect");
            }
            Err::<(), _>("provider activation failed".to_string())
        });
        let (activation_error, report) = match activation {
            Ok(_) => panic!("activation should fail"),
            Err(failure) => failure,
        };

        assert!(report.failures.is_empty(), "rollback should be clean");
        assert_eq!(
            *order
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            vec!["wake", "worker"]
        );
        assert_eq!(activation_error, "provider activation failed");
    }

    #[test]
    fn missing_required_capability_keeps_component_pending() {
        let mut graph = ComponentGraph::default();
        graph.declare(ComponentDefinition::new("agent").requires("llm"));
        assert_eq!(graph.state("agent"), Some(ComponentState::Pending));

        graph.add_capability("llm");
        assert_eq!(graph.state("agent"), Some(ComponentState::Active));
        assert_eq!(
            graph.take_transitions(),
            vec![
                ComponentTransition {
                    component_id: "agent".to_string(),
                    from: ComponentState::Declared,
                    to: ComponentState::Pending,
                },
                ComponentTransition {
                    component_id: "agent".to_string(),
                    from: ComponentState::Pending,
                    to: ComponentState::Activating,
                },
                ComponentTransition {
                    component_id: "agent".to_string(),
                    from: ComponentState::Activating,
                    to: ComponentState::Active,
                },
            ]
        );
    }

    #[test]
    fn removing_or_replacing_a_capability_deactivates_dependents_first() {
        let mut graph = ComponentGraph::default();
        graph.add_capability("llm");
        graph.declare(ComponentDefinition::new("agent").requires("llm"));
        graph.take_transitions();

        graph.replace_capability(&CapabilityKey::from("llm"));
        assert_eq!(graph.state("agent"), Some(ComponentState::Active));
        assert_eq!(graph.capabilities()[0].generation, 1);
        assert_eq!(
            graph.take_transitions(),
            vec![
                ComponentTransition {
                    component_id: "agent".to_string(),
                    from: ComponentState::Active,
                    to: ComponentState::Deactivating,
                },
                ComponentTransition {
                    component_id: "agent".to_string(),
                    from: ComponentState::Deactivating,
                    to: ComponentState::Disposed,
                },
                ComponentTransition {
                    component_id: "agent".to_string(),
                    from: ComponentState::Disposed,
                    to: ComponentState::Activating,
                },
                ComponentTransition {
                    component_id: "agent".to_string(),
                    from: ComponentState::Activating,
                    to: ComponentState::Active,
                },
            ]
        );
    }

    #[test]
    fn optional_capability_is_observable_without_blocking_activation() {
        let mut graph = ComponentGraph::default();
        graph.declare(ComponentDefinition::new("agent").observes("metrics"));

        assert_eq!(graph.state("agent"), Some(ComponentState::Active));
        assert_eq!(
            graph.optional_available("agent", &CapabilityKey::from("metrics")),
            Some(false)
        );
        graph.add_capability("metrics");
        assert_eq!(
            graph.optional_available("agent", &CapabilityKey::from("metrics")),
            Some(true)
        );
    }

    #[test]
    fn removing_required_capability_disposes_the_dependent() {
        let mut graph = ComponentGraph::default();
        graph.add_capability("llm");
        graph.declare(ComponentDefinition::new("agent").requires("llm"));
        graph.take_transitions();

        graph.remove_capability(&CapabilityKey::from("llm"));

        assert_eq!(graph.state("agent"), Some(ComponentState::Pending));
        assert_eq!(
            graph.take_transitions(),
            vec![
                ComponentTransition {
                    component_id: "agent".to_string(),
                    from: ComponentState::Active,
                    to: ComponentState::Deactivating,
                },
                ComponentTransition {
                    component_id: "agent".to_string(),
                    from: ComponentState::Deactivating,
                    to: ComponentState::Disposed,
                },
                ComponentTransition {
                    component_id: "agent".to_string(),
                    from: ComponentState::Disposed,
                    to: ComponentState::Pending,
                },
            ]
        );
    }
}
