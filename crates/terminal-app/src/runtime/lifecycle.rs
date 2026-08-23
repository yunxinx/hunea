use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
};

use thiserror::Error;

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
    #[cfg_attr(not(test), allow(dead_code))]
    Failed,
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
            Self::Failed => "failed",
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
    pub(super) epoch: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CapabilityChangeReason {
    Added,
    Removed,
    Replaced,
}

impl CapabilityChangeReason {
    #[cfg(test)]
    const fn as_str(self) -> &'static str {
        match self {
            Self::Added => "added",
            Self::Removed => "removed",
            Self::Replaced => "replaced",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct CapabilityChange {
    pub(super) key: String,
    pub(super) reason: CapabilityChangeReason,
    pub(super) old_generation: Option<u64>,
    pub(super) new_generation: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ActivationToken {
    component_id: String,
    epoch: u64,
}

impl ActivationToken {
    #[cfg(test)]
    fn component_id(&self) -> &str {
        self.component_id.as_str()
    }

    #[cfg(test)]
    const fn epoch(&self) -> u64 {
        self.epoch
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(not(test), allow(dead_code))]
pub(super) enum ComponentFailureOperation {
    Activation,
}

#[cfg_attr(not(test), allow(dead_code))]
impl ComponentFailureOperation {
    pub(super) const fn as_str(self) -> &'static str {
        match self {
            Self::Activation => "activation",
        }
    }
}

/// 失败原因是封闭的安全投影，不能携带 adapter 的原始错误文本。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(not(test), allow(dead_code))]
pub(super) enum ComponentFailureReason {
    ActivationRejected,
    ActivationCancelled,
}

#[cfg_attr(not(test), allow(dead_code))]
impl ComponentFailureReason {
    const fn operation(self) -> ComponentFailureOperation {
        ComponentFailureOperation::Activation
    }

    pub(super) const fn code(self) -> &'static str {
        match self {
            Self::ActivationRejected => "activation_rejected",
            Self::ActivationCancelled => "activation_cancelled",
        }
    }

    pub(super) const fn message(self) -> &'static str {
        match self {
            Self::ActivationRejected => "component activation was rejected",
            Self::ActivationCancelled => "component activation was cancelled",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ComponentFailureSnapshot {
    pub(super) component_id: String,
    pub(super) operation: ComponentFailureOperation,
    pub(super) reason: ComponentFailureReason,
    pub(super) recoverable: bool,
    pub(super) epoch: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PendingComponentSnapshot {
    pub(super) component_id: String,
    pub(super) missing_dependencies: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(super) struct ReconciliationReport {
    pub(super) capability_change: Option<CapabilityChange>,
    pub(super) affected_components: Vec<String>,
    pub(super) transitions: Vec<ComponentTransition>,
    pub(super) activation_requests: Vec<ActivationToken>,
    pub(super) failures: Vec<ComponentFailureSnapshot>,
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
    pub(super) epoch: u64,
    pub(super) required: Vec<String>,
    pub(super) optional: Vec<OptionalCapabilitySnapshot>,
}

struct ComponentRecord {
    definition: ComponentDefinition,
    state: ComponentState,
    epoch: u64,
    failure: Option<ComponentFailureSnapshot>,
}

#[derive(Debug, Error, PartialEq, Eq)]
#[cfg_attr(not(test), allow(dead_code))]
pub(super) enum ComponentGraphError {
    #[error("component `{component_id}` is already declared")]
    DuplicateComponent { component_id: String },
    #[error("component `{component_id}` is not declared")]
    UnknownComponent { component_id: String },
    #[error("component `{component_id}` is not activating")]
    NotActivating { component_id: String },
    #[error(
        "component `{component_id}` activation epoch is stale (expected {expected}, current {current})"
    )]
    StaleActivation {
        component_id: String,
        expected: u64,
        current: u64,
    },
    #[error("component `{component_id}` is not failed")]
    NotFailed { component_id: String },
    #[error("component `{component_id}` failure is not recoverable")]
    FailureNotRecoverable { component_id: String },
    #[error("component `{component_id}` activation epoch is exhausted")]
    ActivationEpochExhausted { component_id: String },
    #[error("capability `{capability}` generation is exhausted")]
    CapabilityGenerationExhausted { capability: String },
}

/// `ComponentGraph` 是最小 reactive coeffect resolver。
///
/// capability mutation 只通过反向依赖索引 reconcile 受影响 component。依赖齐全只会产生
/// 带 epoch 的 activation request；host 明确确认成功后，component 才能进入 `Active`。
#[derive(Default)]
pub(super) struct ComponentGraph {
    capabilities: BTreeSet<CapabilityKey>,
    capability_generations: BTreeMap<CapabilityKey, u64>,
    components: BTreeMap<String, ComponentRecord>,
    dependents: BTreeMap<CapabilityKey, BTreeSet<String>>,
}

impl ComponentGraph {
    pub(super) fn declare(
        &mut self,
        definition: ComponentDefinition,
    ) -> Result<ReconciliationReport, ComponentGraphError> {
        let id = definition.id.clone();
        if self.components.contains_key(&id) {
            return Err(ComponentGraphError::DuplicateComponent { component_id: id });
        }

        for dependency in definition.required.iter().chain(&definition.optional) {
            self.dependents
                .entry(dependency.clone())
                .or_default()
                .insert(id.clone());
        }
        self.components.insert(
            id.clone(),
            ComponentRecord {
                definition,
                state: ComponentState::Declared,
                epoch: 0,
                failure: None,
            },
        );
        self.reconcile_components(vec![id], None)
    }

    pub(super) fn add_capability(
        &mut self,
        key: impl Into<CapabilityKey>,
    ) -> Result<ReconciliationReport, ComponentGraphError> {
        let key = key.into();
        if self.capabilities.contains(&key) {
            return Ok(ReconciliationReport::default());
        }
        self.ensure_activation_epochs_available(&key, false)?;
        self.capabilities.insert(key.clone());
        let generation = *self.capability_generations.entry(key.clone()).or_insert(0);
        let change = CapabilityChange {
            key: key.to_string(),
            reason: CapabilityChangeReason::Added,
            old_generation: None,
            new_generation: Some(generation),
        };
        self.reconcile_components(self.dependents_for_key(&key), Some(change))
    }

    pub(super) fn remove_capability(
        &mut self,
        key: &CapabilityKey,
    ) -> Result<ReconciliationReport, ComponentGraphError> {
        if !self.capabilities.remove(key) {
            return Ok(ReconciliationReport::default());
        }
        let change = CapabilityChange {
            key: key.to_string(),
            reason: CapabilityChangeReason::Removed,
            old_generation: self.capability_generations.get(key).copied(),
            new_generation: None,
        };
        self.reconcile_components(self.dependents_for_key(key), Some(change))
    }

    /// 替换同一 capability 的 provider generation，而不是改变依赖 key。
    ///
    /// 依赖方必须先离开 `Active`，旧 owner 的 effect 才能被释放；随后 resolver 才能
    /// 为新 generation 创建唯一的 active owner。
    pub(super) fn validate_replacement(
        &self,
        key: &CapabilityKey,
    ) -> Result<(), ComponentGraphError> {
        self.next_capability_generation(key)?;
        self.ensure_activation_epochs_available(key, true)
    }

    pub(super) fn replace_capability(
        &mut self,
        key: &CapabilityKey,
    ) -> Result<ReconciliationReport, ComponentGraphError> {
        self.validate_replacement(key)?;
        let was_available = self.capabilities.remove(key);
        let old_generation = self.capability_generations.get(key).copied();
        let new_generation = self.next_capability_generation(key)?;
        self.capability_generations
            .insert(key.clone(), new_generation);
        let affected = self.dependents_for_key(key);
        let mut report = ReconciliationReport {
            capability_change: Some(CapabilityChange {
                key: key.to_string(),
                reason: CapabilityChangeReason::Replaced,
                old_generation,
                new_generation: Some(new_generation),
            }),
            affected_components: affected.clone(),
            ..ReconciliationReport::default()
        };
        if was_available {
            self.reconcile_into(&affected, &mut report)?;
        }
        self.capabilities.insert(key.clone());
        self.reconcile_into(&affected, &mut report)?;
        self.refresh_report_failures(&mut report);
        Ok(report)
    }

    pub(super) fn complete_activation(
        &mut self,
        token: ActivationToken,
    ) -> Result<ReconciliationReport, ComponentGraphError> {
        self.validate_activation_token(&token)?;
        let mut report = self.report_for_component(token.component_id.clone());
        self.transition(
            token.component_id.as_str(),
            ComponentState::Active,
            &mut report,
        );
        self.refresh_report_failures(&mut report);
        Ok(report)
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) fn fail_activation(
        &mut self,
        token: ActivationToken,
        reason: ComponentFailureReason,
        recoverable: bool,
    ) -> Result<ReconciliationReport, ComponentGraphError> {
        self.validate_activation_token(&token)?;
        let component_id = token.component_id.clone();
        let failure = ComponentFailureSnapshot {
            component_id: component_id.clone(),
            operation: reason.operation(),
            reason,
            recoverable,
            epoch: token.epoch,
        };
        self.components
            .get_mut(&component_id)
            .expect("validated component should remain declared")
            .failure = Some(failure);
        let mut report = self.report_for_component(component_id.clone());
        self.transition(&component_id, ComponentState::Failed, &mut report);
        self.refresh_report_failures(&mut report);
        Ok(report)
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) fn retry(
        &mut self,
        component_id: &str,
    ) -> Result<ReconciliationReport, ComponentGraphError> {
        let record = self.components.get(component_id).ok_or_else(|| {
            ComponentGraphError::UnknownComponent {
                component_id: component_id.to_string(),
            }
        })?;
        if record.state != ComponentState::Failed {
            return Err(ComponentGraphError::NotFailed {
                component_id: component_id.to_string(),
            });
        }
        if !record
            .failure
            .as_ref()
            .is_some_and(|failure| failure.recoverable)
        {
            return Err(ComponentGraphError::FailureNotRecoverable {
                component_id: component_id.to_string(),
            });
        }

        let mut report = self.report_for_component(component_id.to_string());
        self.begin_activation(component_id, &mut report)?;
        self.refresh_report_failures(&mut report);
        Ok(report)
    }

    #[cfg(test)]
    pub(super) fn state(&self, component_id: &str) -> Option<ComponentState> {
        self.components.get(component_id).map(|record| record.state)
    }

    #[cfg(test)]
    pub(super) fn epoch(&self, component_id: &str) -> Option<u64> {
        self.components.get(component_id).map(|record| record.epoch)
    }

    #[cfg(test)]
    fn inject_epoch_exhaustion(&mut self, component_id: &str) {
        self.components
            .get_mut(component_id)
            .expect("component should exist before epoch exhaustion is injected")
            .epoch = u64::MAX;
    }

    #[cfg(test)]
    fn inject_generation_exhaustion(&mut self, key: CapabilityKey) {
        self.capability_generations.insert(key, u64::MAX);
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

    pub(super) fn capabilities(&self) -> Vec<CapabilitySnapshot> {
        self.capabilities
            .iter()
            .map(|key| CapabilitySnapshot {
                key: key.to_string(),
                generation: self.capability_generations.get(key).copied().unwrap_or(0),
            })
            .collect()
    }

    pub(super) fn has_capability(&self, key: &CapabilityKey) -> bool {
        self.capabilities.contains(key)
    }

    pub(super) fn components(&self) -> Vec<ComponentSnapshot> {
        self.components
            .iter()
            .map(|(id, record)| ComponentSnapshot {
                id: id.clone(),
                state: record.state,
                epoch: record.epoch,
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

    pub(super) fn pending(&self) -> Vec<PendingComponentSnapshot> {
        self.components
            .iter()
            .filter(|(_, record)| record.state == ComponentState::Pending)
            .map(|(component_id, record)| PendingComponentSnapshot {
                component_id: component_id.clone(),
                missing_dependencies: record
                    .definition
                    .required
                    .difference(&self.capabilities)
                    .map(ToString::to_string)
                    .collect(),
            })
            .collect()
    }

    pub(super) fn failures(&self) -> Vec<ComponentFailureSnapshot> {
        self.components
            .values()
            .filter_map(|record| record.failure.clone())
            .collect()
    }

    fn dependents_for_key(&self, key: &CapabilityKey) -> Vec<String> {
        self.dependents
            .get(key)
            .into_iter()
            .flat_map(BTreeSet::iter)
            .cloned()
            .collect()
    }

    fn reconcile_components(
        &mut self,
        affected_components: Vec<String>,
        capability_change: Option<CapabilityChange>,
    ) -> Result<ReconciliationReport, ComponentGraphError> {
        let mut report = ReconciliationReport {
            capability_change,
            affected_components,
            ..ReconciliationReport::default()
        };
        let affected = report.affected_components.clone();
        self.reconcile_into(&affected, &mut report)?;
        self.refresh_report_failures(&mut report);
        Ok(report)
    }

    fn reconcile_into(
        &mut self,
        component_ids: &[String],
        report: &mut ReconciliationReport,
    ) -> Result<(), ComponentGraphError> {
        for id in component_ids {
            let record = self
                .components
                .get(id)
                .expect("dependency index should reference a declared component");
            let required_ready = record.definition.required.is_subset(&self.capabilities);
            let state = record.state;
            match (state, required_ready) {
                (
                    ComponentState::Declared | ComponentState::Pending | ComponentState::Disposed,
                    true,
                ) => self.begin_activation(id, report)?,
                (
                    ComponentState::Declared | ComponentState::Pending | ComponentState::Disposed,
                    false,
                ) => {
                    if state != ComponentState::Pending {
                        self.clear_failure(id);
                        self.transition(id, ComponentState::Pending, report);
                    }
                }
                (ComponentState::Activating | ComponentState::Failed, false) => {
                    self.clear_failure(id);
                    self.transition(id, ComponentState::Pending, report);
                }
                (ComponentState::Active, false) => {
                    self.transition(id, ComponentState::Deactivating, report);
                    self.transition(id, ComponentState::Disposed, report);
                    self.transition(id, ComponentState::Pending, report);
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn begin_activation(
        &mut self,
        id: &str,
        report: &mut ReconciliationReport,
    ) -> Result<(), ComponentGraphError> {
        let record = self
            .components
            .get_mut(id)
            .expect("component should remain declared during activation");
        let epoch = record.epoch.checked_add(1).ok_or_else(|| {
            ComponentGraphError::ActivationEpochExhausted {
                component_id: id.to_string(),
            }
        })?;
        record.failure = None;
        record.epoch = epoch;
        self.transition(id, ComponentState::Activating, report);
        report.activation_requests.push(ActivationToken {
            component_id: id.to_string(),
            epoch,
        });
        Ok(())
    }

    fn ensure_activation_epochs_available(
        &self,
        key: &CapabilityKey,
        is_replacement: bool,
    ) -> Result<(), ComponentGraphError> {
        for id in self.dependents_for_key(key) {
            let record = self
                .components
                .get(&id)
                .expect("dependency index should reference a declared component");
            let will_be_ready = record
                .definition
                .required
                .iter()
                .all(|required| required == key || self.capabilities.contains(required));
            let will_begin = if is_replacement {
                record.definition.required.contains(key) && will_be_ready
            } else {
                matches!(
                    record.state,
                    ComponentState::Declared | ComponentState::Pending | ComponentState::Disposed
                ) && will_be_ready
            };
            if will_begin && record.epoch == u64::MAX {
                return Err(ComponentGraphError::ActivationEpochExhausted { component_id: id });
            }
        }
        Ok(())
    }

    fn next_capability_generation(&self, key: &CapabilityKey) -> Result<u64, ComponentGraphError> {
        match self.capability_generations.get(key) {
            Some(generation) => generation.checked_add(1).ok_or_else(|| {
                ComponentGraphError::CapabilityGenerationExhausted {
                    capability: key.to_string(),
                }
            }),
            None => Ok(1),
        }
    }

    fn validate_activation_token(
        &self,
        token: &ActivationToken,
    ) -> Result<(), ComponentGraphError> {
        let record = self.components.get(&token.component_id).ok_or_else(|| {
            ComponentGraphError::UnknownComponent {
                component_id: token.component_id.clone(),
            }
        })?;
        if record.epoch != token.epoch {
            return Err(ComponentGraphError::StaleActivation {
                component_id: token.component_id.clone(),
                expected: token.epoch,
                current: record.epoch,
            });
        }
        if record.state != ComponentState::Activating {
            return Err(ComponentGraphError::NotActivating {
                component_id: token.component_id.clone(),
            });
        }
        Ok(())
    }

    fn report_for_component(&self, component_id: String) -> ReconciliationReport {
        ReconciliationReport {
            affected_components: vec![component_id],
            ..ReconciliationReport::default()
        }
    }

    fn refresh_report_failures(&self, report: &mut ReconciliationReport) {
        report.failures = self.failures();
    }

    fn clear_failure(&mut self, id: &str) {
        self.components
            .get_mut(id)
            .expect("component should remain declared")
            .failure = None;
    }

    fn transition(&mut self, id: &str, to: ComponentState, report: &mut ReconciliationReport) {
        let record = self
            .components
            .get_mut(id)
            .expect("component id should remain declared");
        let from = record.state;
        record.state = to;
        report.transitions.push(ComponentTransition {
            component_id: id.to_string(),
            from,
            to,
            epoch: record.epoch,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_required_capability_keeps_component_pending() {
        let mut graph = ComponentGraph::default();
        let declaration = graph
            .declare(ComponentDefinition::new("agent").requires("llm"))
            .expect("unique component should declare");
        assert_eq!(graph.state("agent"), Some(ComponentState::Pending));
        assert_eq!(
            declaration.transitions,
            vec![ComponentTransition {
                component_id: "agent".to_string(),
                from: ComponentState::Declared,
                to: ComponentState::Pending,
                epoch: 0,
            }]
        );
        assert_eq!(
            graph.pending(),
            vec![PendingComponentSnapshot {
                component_id: "agent".to_string(),
                missing_dependencies: vec!["llm".to_string()],
            }]
        );

        let added = graph
            .add_capability("llm")
            .expect("capability add should reconcile");
        assert_eq!(graph.state("agent"), Some(ComponentState::Activating));
        assert_eq!(added.affected_components, vec!["agent"]);
        assert_eq!(added.activation_requests.len(), 1);
        assert_eq!(
            added.transitions,
            vec![ComponentTransition {
                component_id: "agent".to_string(),
                from: ComponentState::Pending,
                to: ComponentState::Activating,
                epoch: 1,
            }]
        );

        let completed = graph
            .complete_activation(added.activation_requests[0].clone())
            .expect("current activation should complete");
        assert_eq!(graph.state("agent"), Some(ComponentState::Active));
        assert_eq!(graph.epoch("agent"), Some(1));
        assert_eq!(completed.transitions[0].to, ComponentState::Active);
        assert!(graph.pending().is_empty());
    }

    #[test]
    fn activation_failure_is_redacted_and_retry_rejects_stale_completion() {
        let mut graph = ComponentGraph::default();
        graph
            .add_capability("llm")
            .expect("initial capability should add");
        let declaration = graph
            .declare(ComponentDefinition::new("agent").requires("llm"))
            .expect("unique component should declare");
        let first_token = declaration.activation_requests[0].clone();
        let failure = graph
            .fail_activation(
                first_token.clone(),
                ComponentFailureReason::ActivationRejected,
                true,
            )
            .expect("current activation should be fail-able");

        assert_eq!(graph.state("agent"), Some(ComponentState::Failed));
        assert_eq!(failure.failures.len(), 1);
        assert_eq!(failure.failures[0].component_id, "agent");
        assert_eq!(
            failure.failures[0].operation,
            ComponentFailureOperation::Activation
        );
        assert_eq!(failure.failures[0].reason.code(), "activation_rejected");
        assert_eq!(
            failure.failures[0].reason.message(),
            "component activation was rejected"
        );
        let raw_error = "secret provider credential must stay outside the graph";
        assert!(!format!("{failure:?}").contains(raw_error));

        let retry = graph
            .retry("agent")
            .expect("recoverable failure should retry");
        let retry_token = retry.activation_requests[0].clone();
        assert_eq!(retry_token.epoch(), 2);
        assert!(retry.failures.is_empty());
        assert_eq!(
            graph.complete_activation(first_token.clone()),
            Err(ComponentGraphError::StaleActivation {
                component_id: "agent".to_string(),
                expected: 1,
                current: 2,
            })
        );
        assert_eq!(
            graph.fail_activation(
                first_token,
                ComponentFailureReason::ActivationCancelled,
                true,
            ),
            Err(ComponentGraphError::StaleActivation {
                component_id: "agent".to_string(),
                expected: 1,
                current: 2,
            })
        );
        assert_eq!(graph.state("agent"), Some(ComponentState::Activating));
        assert!(graph.failures().is_empty());
        graph
            .complete_activation(retry_token)
            .expect("current retry should complete");
        assert_eq!(graph.state("agent"), Some(ComponentState::Active));
        assert!(graph.failures().is_empty());
    }

    #[test]
    fn non_recoverable_failure_rejects_retry() {
        let mut graph = ComponentGraph::default();
        let declaration = graph
            .declare(ComponentDefinition::new("agent"))
            .expect("unique component should declare");
        graph
            .fail_activation(
                declaration.activation_requests[0].clone(),
                ComponentFailureReason::ActivationCancelled,
                false,
            )
            .expect("current activation should be fail-able");

        assert_eq!(
            graph.retry("agent"),
            Err(ComponentGraphError::FailureNotRecoverable {
                component_id: "agent".to_string(),
            })
        );
        assert_eq!(graph.state("agent"), Some(ComponentState::Failed));
    }

    #[test]
    fn replacement_deactivates_all_dependents_before_deterministic_reactivation() {
        let mut graph = ComponentGraph::default();
        graph
            .add_capability("llm")
            .expect("initial capability should add");
        for id in ["z-agent", "a-agent"] {
            let declaration = graph
                .declare(ComponentDefinition::new(id).requires("llm"))
                .expect("component ids should be unique");
            graph
                .complete_activation(declaration.activation_requests[0].clone())
                .expect("initial activation should complete");
        }

        let replacement = graph
            .replace_capability(&CapabilityKey::from("llm"))
            .expect("replacement should reconcile");

        assert_eq!(replacement.affected_components, vec!["a-agent", "z-agent"]);
        assert_eq!(
            replacement.capability_change.as_ref().map(|change| (
                change.key.as_str(),
                change.reason.as_str(),
                change.old_generation,
                change.new_generation,
            )),
            Some(("llm", "replaced", Some(0), Some(1)))
        );
        assert_eq!(
            replacement
                .transitions
                .iter()
                .map(|transition| (transition.component_id.as_str(), transition.to))
                .collect::<Vec<_>>(),
            vec![
                ("a-agent", ComponentState::Deactivating),
                ("a-agent", ComponentState::Disposed),
                ("a-agent", ComponentState::Pending),
                ("z-agent", ComponentState::Deactivating),
                ("z-agent", ComponentState::Disposed),
                ("z-agent", ComponentState::Pending),
                ("a-agent", ComponentState::Activating),
                ("z-agent", ComponentState::Activating),
            ]
        );
        assert_eq!(
            replacement
                .activation_requests
                .iter()
                .map(|token| (token.component_id(), token.epoch()))
                .collect::<Vec<_>>(),
            vec![("a-agent", 2), ("z-agent", 2)]
        );
        assert_eq!(graph.capabilities()[0].generation, 1);
    }

    #[test]
    fn observed_and_unrelated_capabilities_do_not_restart_components() {
        let mut graph = ComponentGraph::default();
        let declaration = graph
            .declare(ComponentDefinition::new("agent").observes("metrics"))
            .expect("unique component should declare");
        graph
            .complete_activation(declaration.activation_requests[0].clone())
            .expect("initial activation should complete");

        let unrelated = graph
            .add_capability("tracing")
            .expect("unrelated capability should add");
        assert!(unrelated.affected_components.is_empty());
        assert!(unrelated.transitions.is_empty());
        assert_eq!(graph.epoch("agent"), Some(1));

        let observed = graph
            .add_capability("metrics")
            .expect("observed capability should add");
        assert_eq!(observed.affected_components, vec!["agent"]);
        assert!(observed.transitions.is_empty());
        assert_eq!(
            graph.optional_available("agent", &CapabilityKey::from("metrics")),
            Some(true)
        );
        assert_eq!(graph.state("agent"), Some(ComponentState::Active));
        assert_eq!(graph.epoch("agent"), Some(1));
    }

    #[test]
    fn observed_change_does_not_retry_failed_component() {
        let mut graph = ComponentGraph::default();
        graph
            .add_capability("metrics")
            .expect("observed capability should add");
        let declaration = graph
            .declare(ComponentDefinition::new("agent").observes("metrics"))
            .expect("unique component should declare");
        graph
            .fail_activation(
                declaration.activation_requests[0].clone(),
                ComponentFailureReason::ActivationRejected,
                true,
            )
            .expect("current activation should be fail-able");

        let removed = graph
            .remove_capability(&CapabilityKey::from("metrics"))
            .expect("observed capability removal should reconcile");
        assert_eq!(removed.affected_components, vec!["agent"]);
        assert!(removed.transitions.is_empty());
        assert!(removed.activation_requests.is_empty());
        assert_eq!(graph.state("agent"), Some(ComponentState::Failed));
        assert_eq!(graph.epoch("agent"), Some(1));
    }

    #[test]
    fn removing_required_capability_disposes_the_active_dependent() {
        let mut graph = ComponentGraph::default();
        graph
            .add_capability("llm")
            .expect("initial capability should add");
        let declaration = graph
            .declare(ComponentDefinition::new("agent").requires("llm"))
            .expect("unique component should declare");
        graph
            .complete_activation(declaration.activation_requests[0].clone())
            .expect("initial activation should complete");

        let removed = graph
            .remove_capability(&CapabilityKey::from("llm"))
            .expect("required capability removal should reconcile");

        assert_eq!(graph.state("agent"), Some(ComponentState::Pending));
        assert_eq!(
            removed.transitions,
            vec![
                ComponentTransition {
                    component_id: "agent".to_string(),
                    from: ComponentState::Active,
                    to: ComponentState::Deactivating,
                    epoch: 1,
                },
                ComponentTransition {
                    component_id: "agent".to_string(),
                    from: ComponentState::Deactivating,
                    to: ComponentState::Disposed,
                    epoch: 1,
                },
                ComponentTransition {
                    component_id: "agent".to_string(),
                    from: ComponentState::Disposed,
                    to: ComponentState::Pending,
                    epoch: 1,
                },
            ]
        );
        assert_eq!(
            removed
                .capability_change
                .as_ref()
                .map(|change| change.reason.as_str()),
            Some("removed")
        );
        assert_eq!(
            graph.pending()[0].missing_dependencies,
            vec!["llm".to_string()]
        );
        assert!(graph.failures().is_empty());
    }

    #[test]
    fn duplicate_declaration_and_repeated_capability_mutations_are_no_ops() {
        let mut graph = ComponentGraph::default();
        graph
            .declare(ComponentDefinition::new("agent").requires("llm"))
            .expect("initial declaration should succeed");

        assert_eq!(
            graph.declare(ComponentDefinition::new("agent").observes("metrics")),
            Err(ComponentGraphError::DuplicateComponent {
                component_id: "agent".to_string(),
            })
        );
        let added = graph
            .add_capability("llm")
            .expect("required capability should add");
        assert_eq!(added.affected_components, vec!["agent"]);
        assert_eq!(
            added
                .capability_change
                .as_ref()
                .map(|change| change.reason.as_str()),
            Some("added")
        );
        assert_eq!(
            graph.add_capability("llm"),
            Ok(ReconciliationReport::default())
        );
        graph
            .complete_activation(added.activation_requests[0].clone())
            .expect("current activation should complete");
        graph
            .remove_capability(&CapabilityKey::from("llm"))
            .expect("required capability should remove");
        assert_eq!(
            graph.remove_capability(&CapabilityKey::from("llm")),
            Ok(ReconciliationReport::default())
        );
        let observed = graph
            .add_capability("metrics")
            .expect("unindexed capability should add");
        assert!(
            observed.affected_components.is_empty(),
            "duplicate declaration must not add observed dependency edges"
        );
    }

    #[test]
    fn pending_dependencies_and_index_use_stable_natural_order() {
        let mut graph = ComponentGraph::default();
        graph
            .declare(
                ComponentDefinition::new("z-agent")
                    .requires("tools")
                    .requires("llm"),
            )
            .unwrap();
        graph
            .declare(ComponentDefinition::new("a-agent").requires("llm"))
            .unwrap();

        assert_eq!(
            graph.pending(),
            vec![
                PendingComponentSnapshot {
                    component_id: "a-agent".to_string(),
                    missing_dependencies: vec!["llm".to_string()],
                },
                PendingComponentSnapshot {
                    component_id: "z-agent".to_string(),
                    missing_dependencies: vec!["llm".to_string(), "tools".to_string()],
                },
            ]
        );
        let report = graph
            .add_capability("llm")
            .expect("capability add should reconcile indexed dependents");
        assert_eq!(report.affected_components, vec!["a-agent", "z-agent"]);
    }

    #[test]
    fn required_capability_recovery_clears_failure_and_starts_a_new_epoch() {
        let mut graph = ComponentGraph::default();
        graph
            .add_capability("llm")
            .expect("initial capability should add");
        let declaration = graph
            .declare(ComponentDefinition::new("agent").requires("llm"))
            .expect("unique component should declare");
        graph
            .fail_activation(
                declaration.activation_requests[0].clone(),
                ComponentFailureReason::ActivationRejected,
                true,
            )
            .expect("current activation should fail");

        let removal = graph
            .remove_capability(&CapabilityKey::from("llm"))
            .expect("required capability removal should reconcile");
        assert_eq!(graph.state("agent"), Some(ComponentState::Pending));
        assert!(removal.failures.is_empty());
        assert!(graph.failures().is_empty());

        let recovery = graph
            .add_capability("llm")
            .expect("required capability recovery should reconcile");
        assert_eq!(graph.state("agent"), Some(ComponentState::Activating));
        assert_eq!(recovery.activation_requests[0].epoch(), 2);
        assert!(recovery.failures.is_empty());
    }

    #[test]
    fn reconciliation_reports_all_current_failures_in_component_order() {
        let mut graph = ComponentGraph::default();
        for id in ["z-failed", "a-failed"] {
            let declaration = graph
                .declare(ComponentDefinition::new(id))
                .expect("component ids should be unique");
            graph
                .fail_activation(
                    declaration.activation_requests[0].clone(),
                    ComponentFailureReason::ActivationRejected,
                    true,
                )
                .expect("current activation should fail");
        }
        let observer = graph
            .declare(ComponentDefinition::new("metrics-observer").observes("metrics"))
            .expect("observer id should be unique");
        graph
            .complete_activation(observer.activation_requests[0].clone())
            .expect("observer activation should complete");

        let report = graph
            .add_capability("metrics")
            .expect("observed capability should reconcile");

        assert_eq!(report.affected_components, vec!["metrics-observer"]);
        assert_eq!(
            report
                .failures
                .iter()
                .map(|failure| failure.component_id.as_str())
                .collect::<Vec<_>>(),
            vec!["a-failed", "z-failed"]
        );
    }

    #[test]
    fn activation_epoch_exhaustion_rejects_capability_add_without_mutation() {
        let mut graph = ComponentGraph::default();
        graph
            .declare(ComponentDefinition::new("agent").requires("llm"))
            .expect("unique component should declare");
        graph.inject_epoch_exhaustion("agent");

        assert_eq!(
            graph.add_capability("llm"),
            Err(ComponentGraphError::ActivationEpochExhausted {
                component_id: "agent".to_string(),
            })
        );
        assert!(!graph.has_capability(&CapabilityKey::from("llm")));
        assert_eq!(graph.state("agent"), Some(ComponentState::Pending));
        assert_eq!(graph.epoch("agent"), Some(u64::MAX));
    }

    #[test]
    fn generation_exhaustion_rejects_replacement_without_mutation() {
        let mut graph = ComponentGraph::default();
        let capability = CapabilityKey::from("llm");
        graph
            .add_capability(capability.clone())
            .expect("initial capability should add");
        graph.inject_generation_exhaustion(capability.clone());

        assert_eq!(
            graph.replace_capability(&capability),
            Err(ComponentGraphError::CapabilityGenerationExhausted {
                capability: "llm".to_string(),
            })
        );
        assert!(graph.has_capability(&capability));
        assert_eq!(graph.capabilities()[0].generation, u64::MAX);
    }
}
