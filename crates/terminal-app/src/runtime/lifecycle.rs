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
    implementation_id: Option<String>,
    pub(super) required: BTreeSet<CapabilityKey>,
    pub(super) optional: BTreeSet<CapabilityKey>,
    pub(super) provides: BTreeSet<CapabilityKey>,
}

impl ComponentDefinition {
    pub(super) fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            implementation_id: None,
            required: BTreeSet::new(),
            optional: BTreeSet::new(),
            provides: BTreeSet::new(),
        }
    }

    /// implementation identity 参与 definition equality，但不进入 runtime inspection。
    pub(super) fn implemented_by(mut self, implementation_id: impl Into<String>) -> Self {
        self.implementation_id = Some(implementation_id.into());
        self
    }

    pub(super) fn requires(mut self, key: impl Into<CapabilityKey>) -> Self {
        self.required.insert(key.into());
        self
    }

    pub(super) fn observes(mut self, key: impl Into<CapabilityKey>) -> Self {
        self.optional.insert(key.into());
        self
    }

    pub(super) fn provides(mut self, key: impl Into<CapabilityKey>) -> Self {
        self.provides.insert(key.into());
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

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct ActivationToken {
    component_id: String,
    epoch: u64,
}

impl ActivationToken {
    pub(super) fn component_id(&self) -> &str {
        self.component_id.as_str()
    }

    #[cfg(test)]
    pub(super) const fn epoch(&self) -> u64 {
        self.epoch
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum DeactivationDisposition {
    Reconcile,
    Suspend,
    Dispose,
}

/// `DeactivationToken` 把 concrete quiescence 绑定到发起它的 component epoch。
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct DeactivationToken {
    component_id: String,
    epoch: u64,
    disposition: DeactivationDisposition,
}

impl DeactivationToken {
    pub(super) fn component_id(&self) -> &str {
        self.component_id.as_str()
    }

    pub(super) const fn epoch(&self) -> u64 {
        self.epoch
    }

    #[cfg(test)]
    pub(super) fn with_dispose_disposition(mut self) -> Self {
        self.disposition = DeactivationDisposition::Dispose;
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(not(test), allow(dead_code))]
pub(super) enum ComponentFailureOperation {
    Activation,
    Deactivation,
}

#[cfg_attr(not(test), allow(dead_code))]
impl ComponentFailureOperation {
    pub(super) const fn as_str(self) -> &'static str {
        match self {
            Self::Activation => "activation",
            Self::Deactivation => "deactivation",
        }
    }
}

/// 失败原因是封闭的安全投影，不能携带 adapter 的原始错误文本。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(not(test), allow(dead_code))]
pub(super) enum ComponentFailureReason {
    ActivationRejected,
    ActivationCancelled,
    QuiescenceRejected,
    EffectDisposalRejected,
}

#[cfg_attr(not(test), allow(dead_code))]
impl ComponentFailureReason {
    const fn operation(self) -> ComponentFailureOperation {
        match self {
            Self::ActivationRejected | Self::ActivationCancelled => {
                ComponentFailureOperation::Activation
            }
            Self::QuiescenceRejected | Self::EffectDisposalRejected => {
                ComponentFailureOperation::Deactivation
            }
        }
    }

    pub(super) const fn code(self) -> &'static str {
        match self {
            Self::ActivationRejected => "activation_rejected",
            Self::ActivationCancelled => "activation_cancelled",
            Self::QuiescenceRejected => "quiescence_rejected",
            Self::EffectDisposalRejected => "effect_disposal_rejected",
        }
    }

    pub(super) const fn message(self) -> &'static str {
        match self {
            Self::ActivationRejected => "component activation was rejected",
            Self::ActivationCancelled => "component activation was cancelled",
            Self::QuiescenceRejected => "component quiescence was rejected",
            Self::EffectDisposalRejected => "component effect disposal was rejected",
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
    pub(super) deactivation_requests: Vec<DeactivationToken>,
    pub(super) activation_requests: Vec<ActivationToken>,
    pub(super) failures: Vec<ComponentFailureSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct CapabilitySnapshot {
    pub(super) key: String,
    pub(super) provider_component: String,
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
    pub(super) provides: Vec<String>,
}

#[derive(Clone)]
struct ComponentRecord {
    definition: ComponentDefinition,
    state: ComponentState,
    failure: Option<ComponentFailureSnapshot>,
    cleanup_disposition: Option<DeactivationDisposition>,
}

struct DefinitionIndexes {
    providers: BTreeMap<CapabilityKey, String>,
    dependents: BTreeMap<CapabilityKey, BTreeSet<String>>,
}

/// `PreparedDefinitionReconciliation` 只保存 mutation-free preflight 的封闭 graph 事实。
pub(super) struct PreparedDefinitionReconciliation {
    observed_definitions: BTreeMap<String, ComponentDefinition>,
    observed_epochs: BTreeMap<String, u64>,
    desired_definitions: BTreeMap<String, ComponentDefinition>,
    desired_providers: BTreeMap<CapabilityKey, String>,
    desired_dependents: BTreeMap<CapabilityKey, BTreeSet<String>>,
    retirement_order: Vec<String>,
    activation_order: Vec<String>,
}

impl PreparedDefinitionReconciliation {
    pub(super) fn retirement_order(&self) -> &[String] {
        &self.retirement_order
    }

    #[cfg(test)]
    fn activation_order(&self) -> &[String] {
        &self.activation_order
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
#[cfg_attr(not(test), allow(dead_code))]
pub(super) enum ComponentGraphError {
    #[error("component `{component_id}` is already declared")]
    DuplicateComponent { component_id: String },
    #[error("component `{component_id}` is not declared")]
    UnknownComponent { component_id: String },
    #[error(
        "capability `{capability}` is provided by component `{existing_component_id}`, not `{requested_component_id}`"
    )]
    CapabilityProviderMismatch {
        capability: String,
        existing_component_id: String,
        requested_component_id: String,
    },
    #[error("capability `{capability}` has no declared provider")]
    UndeclaredCapabilityProvider { capability: String },
    #[error("capability `{capability}` is already provided by component `{existing_component_id}`")]
    DuplicateCapabilityProvider {
        capability: String,
        existing_component_id: String,
    },
    #[error("component `{component_id}` requires its own capability `{capability}`")]
    SelfDependency {
        component_id: String,
        capability: String,
    },
    #[error("required capability topology contains a cycle involving {component_ids:?}")]
    DependencyCycle { component_ids: Vec<String> },
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
    #[error("component `{component_id}` is not deactivating")]
    NotDeactivating { component_id: String },
    #[error(
        "component `{component_id}` deactivation epoch is stale (expected {expected}, current {current})"
    )]
    StaleDeactivation {
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
    #[error("component `{component_id}` is busy during definition reconciliation")]
    DefinitionReconciliationBusy { component_id: String },
    #[error("component `{component_id}` has an unresolved cleanup failure")]
    DefinitionCleanupBlocked { component_id: String },
    #[error("component definition reconciliation is stale")]
    StaleDefinitionReconciliation,
}

/// `ComponentGraph` 是最小 reactive coeffect resolver。
///
/// capability mutation 只通过反向依赖索引 reconcile 受影响 component。依赖齐全只会产生
/// 带 epoch 的 activation request；host 明确确认成功后，component 才能进入 `Active`。
#[derive(Clone, Default)]
pub(super) struct ComponentGraph {
    capabilities: BTreeMap<CapabilityKey, String>,
    capability_generations: BTreeMap<CapabilityKey, u64>,
    component_epochs: BTreeMap<String, u64>,
    components: BTreeMap<String, ComponentRecord>,
    providers: BTreeMap<CapabilityKey, String>,
    dependents: BTreeMap<CapabilityKey, BTreeSet<String>>,
}

impl ComponentGraph {
    #[cfg(test)]
    pub(super) fn declare(
        &mut self,
        definition: ComponentDefinition,
    ) -> Result<ReconciliationReport, ComponentGraphError> {
        let id = definition.id.clone();
        if self.components.contains_key(&id) {
            return Err(ComponentGraphError::DuplicateComponent { component_id: id });
        }

        self.validate_definition(&definition)?;

        for dependency in definition.required.iter().chain(&definition.optional) {
            self.dependents
                .entry(dependency.clone())
                .or_default()
                .insert(id.clone());
        }
        for capability in &definition.provides {
            self.providers.insert(capability.clone(), id.clone());
        }
        self.components.insert(
            id.clone(),
            ComponentRecord {
                definition,
                state: ComponentState::Declared,
                failure: None,
                cleanup_disposition: None,
            },
        );
        self.component_epochs.entry(id.clone()).or_insert(0);
        self.reconcile_components(vec![id], None)
    }

    /// 在任何 concrete cleanup 前校验完整 desired definition set 并推导 graph order。
    pub(super) fn prepare_definition_reconciliation(
        &self,
        definitions: impl IntoIterator<Item = ComponentDefinition>,
    ) -> Result<PreparedDefinitionReconciliation, ComponentGraphError> {
        let mut desired_definitions = BTreeMap::new();
        for definition in definitions {
            let component_id = definition.id.clone();
            if desired_definitions
                .insert(component_id.clone(), definition)
                .is_some()
            {
                return Err(ComponentGraphError::DuplicateComponent { component_id });
            }
        }

        let indexes = definition_indexes(&desired_definitions)?;
        let desired_providers = indexes.providers;
        let desired_dependents = indexes.dependents;
        let desired_topology = topology_order(&desired_definitions, &desired_providers)?;
        let observed_definitions = self.component_definitions();
        let changed_roots = observed_definitions
            .keys()
            .chain(desired_definitions.keys())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .filter(|component_id| {
                observed_definitions.get(*component_id) != desired_definitions.get(*component_id)
            })
            .cloned()
            .collect::<BTreeSet<_>>();

        let observed_edges = required_component_dependents(&observed_definitions, &self.providers);
        let desired_edges = required_component_dependents(&desired_definitions, &desired_providers);
        let mut affected = dependent_closure(&changed_roots, &observed_edges);
        affected.extend(dependent_closure(&changed_roots, &desired_edges));

        let observed_ids = observed_definitions
            .keys()
            .cloned()
            .collect::<BTreeSet<_>>();
        let desired_ids = desired_definitions.keys().cloned().collect::<BTreeSet<_>>();
        let retirement_set = affected
            .intersection(&observed_ids)
            .cloned()
            .collect::<BTreeSet<_>>();
        let activation_set = affected
            .intersection(&desired_ids)
            .cloned()
            .collect::<BTreeSet<_>>();
        let retirement_order = self
            .deactivation_order()
            .into_iter()
            .filter(|component_id| retirement_set.contains(component_id))
            .collect::<Vec<_>>();
        let activation_order = desired_topology
            .into_iter()
            .filter(|component_id| activation_set.contains(component_id))
            .collect::<Vec<_>>();

        for component_id in &activation_order {
            if self.component_epoch(component_id) == u64::MAX {
                return Err(ComponentGraphError::ActivationEpochExhausted {
                    component_id: component_id.clone(),
                });
            }
            let definition = desired_definitions
                .get(component_id)
                .expect("activation order should reference a desired component");
            for capability in &definition.provides {
                if self.capability_generations.get(capability) == Some(&u64::MAX) {
                    return Err(ComponentGraphError::CapabilityGenerationExhausted {
                        capability: capability.to_string(),
                    });
                }
            }
        }
        for component_id in &retirement_order {
            let record = self
                .components
                .get(component_id)
                .expect("retirement order should reference an observed component");
            if matches!(
                record.state,
                ComponentState::Activating | ComponentState::Deactivating
            ) {
                return Err(ComponentGraphError::DefinitionReconciliationBusy {
                    component_id: component_id.clone(),
                });
            }
            if record.state == ComponentState::Failed
                && record.failure.as_ref().is_some_and(|failure| {
                    failure.operation == ComponentFailureOperation::Deactivation
                })
            {
                return Err(ComponentGraphError::DefinitionCleanupBlocked {
                    component_id: component_id.clone(),
                });
            }
        }

        Ok(PreparedDefinitionReconciliation {
            observed_definitions,
            observed_epochs: self.component_epochs.clone(),
            desired_definitions,
            desired_providers,
            desired_dependents,
            retirement_order,
            activation_order,
        })
    }

    /// old effects 全部退役后，一次性提交 prepared definitions 与全部 dependency indexes。
    pub(super) fn commit_definition_reconciliation(
        &mut self,
        prepared: PreparedDefinitionReconciliation,
    ) -> Result<Vec<String>, ComponentGraphError> {
        if self.component_definitions() != prepared.observed_definitions
            || self.component_epochs != prepared.observed_epochs
        {
            return Err(ComponentGraphError::StaleDefinitionReconciliation);
        }
        if prepared.retirement_order.iter().any(|component_id| {
            self.components.get(component_id).is_some_and(|record| {
                matches!(
                    record.state,
                    ComponentState::Active
                        | ComponentState::Activating
                        | ComponentState::Deactivating
                )
            })
        }) {
            return Err(ComponentGraphError::StaleDefinitionReconciliation);
        }
        if self.capabilities.iter().any(|(capability, provider)| {
            prepared.desired_providers.get(capability) != Some(provider)
        }) {
            return Err(ComponentGraphError::StaleDefinitionReconciliation);
        }

        let mut observed_records = std::mem::take(&mut self.components);
        let mut desired_records = BTreeMap::new();
        for (component_id, definition) in prepared.desired_definitions {
            self.component_epochs
                .entry(component_id.clone())
                .or_insert(0);
            let record = match observed_records.remove(&component_id) {
                Some(record) if record.definition == definition => record,
                _ => ComponentRecord {
                    definition,
                    state: ComponentState::Declared,
                    failure: None,
                    cleanup_disposition: None,
                },
            };
            desired_records.insert(component_id, record);
        }
        self.components = desired_records;
        self.providers = prepared.desired_providers;
        self.dependents = prepared.desired_dependents;
        Ok(prepared.activation_order)
    }

    pub(super) fn add_capability(
        &mut self,
        provider_component: &str,
        key: impl Into<CapabilityKey>,
    ) -> Result<ReconciliationReport, ComponentGraphError> {
        let key = key.into();
        self.validate_capability_provider(provider_component, &key)?;
        if self.capabilities.contains_key(&key) {
            return Ok(ReconciliationReport::default());
        }
        self.ensure_activation_epochs_available(&key, false)?;
        self.capabilities
            .insert(key.clone(), provider_component.to_string());
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
        provider_component: &str,
        key: &CapabilityKey,
    ) -> Result<ReconciliationReport, ComponentGraphError> {
        self.validate_capability_provider(provider_component, key)?;
        if self.capabilities.remove(key).is_none() {
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

    /// 在 clone 上按稳定 key 顺序移除一组 capability，供 host 与 concrete Context 原子提交。
    pub(super) fn prepare_capability_removals(
        &self,
        provider_component: &str,
        keys: impl IntoIterator<Item = CapabilityKey>,
    ) -> Result<(Self, Vec<ReconciliationReport>), ComponentGraphError> {
        let mut tentative = self.clone();
        let mut keys = keys.into_iter().collect::<Vec<_>>();
        keys.sort();
        let mut reports = Vec::new();
        for key in keys {
            reports.push(tentative.remove_capability(provider_component, &key)?);
        }
        Ok((tentative, reports))
    }

    /// 替换同一 capability 的 provider generation，而不是改变依赖 key。
    ///
    /// 依赖方必须先离开 `Active`，旧 owner 的 effect 才能被释放；随后 resolver 才能
    /// 为新 generation 创建唯一的 active owner。
    pub(super) fn validate_replacement(
        &self,
        provider_component: &str,
        key: &CapabilityKey,
    ) -> Result<(), ComponentGraphError> {
        self.validate_capability_provider(provider_component, key)?;
        self.next_capability_generation(key)?;
        self.ensure_activation_epochs_available(key, true)
    }

    pub(super) fn validate_activation_epoch(
        &self,
        component_id: &str,
    ) -> Result<(), ComponentGraphError> {
        self.components
            .get(component_id)
            .ok_or_else(|| ComponentGraphError::UnknownComponent {
                component_id: component_id.to_string(),
            })?;
        if self.component_epoch(component_id) == u64::MAX {
            return Err(ComponentGraphError::ActivationEpochExhausted {
                component_id: component_id.to_string(),
            });
        }
        Ok(())
    }

    pub(super) fn activation_closure(
        &self,
        component_ids: impl IntoIterator<Item = impl Into<String>>,
    ) -> Result<Vec<String>, ComponentGraphError> {
        let mut pending = component_ids
            .into_iter()
            .map(Into::into)
            .collect::<Vec<_>>();
        let mut closure = BTreeSet::new();
        while let Some(component_id) = pending.pop() {
            let record = self.components.get(&component_id).ok_or_else(|| {
                ComponentGraphError::UnknownComponent {
                    component_id: component_id.clone(),
                }
            })?;
            if !closure.insert(component_id) {
                continue;
            }
            for capability in &record.definition.provides {
                for dependent_id in self.dependents_for_key(capability) {
                    let dependent = self
                        .components
                        .get(&dependent_id)
                        .expect("dependency index should reference a declared component");
                    if dependent.definition.required.contains(capability) {
                        pending.push(dependent_id);
                    }
                }
            }
        }
        Ok(closure.into_iter().collect())
    }

    pub(super) fn replace_capability(
        &mut self,
        provider_component: &str,
        key: &CapabilityKey,
    ) -> Result<ReconciliationReport, ComponentGraphError> {
        self.validate_replacement(provider_component, key)?;
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
        if was_available.is_some() {
            self.reconcile_into(&affected, &mut report)?;
        }
        self.capabilities
            .insert(key.clone(), provider_component.to_string());
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

    #[cfg(test)]
    pub(super) fn complete_activation_and_publish(
        &mut self,
        token: ActivationToken,
    ) -> Result<Vec<ReconciliationReport>, ComponentGraphError> {
        let (tentative, reports) = self.prepare_activation_and_publish(token)?;
        *self = tentative;
        Ok(reports)
    }

    /// 在 clone 上完成 activation/publication，供 host 与 concrete Context 原子提交。
    pub(super) fn prepare_activation_and_publish(
        &self,
        token: ActivationToken,
    ) -> Result<(Self, Vec<ReconciliationReport>), ComponentGraphError> {
        let component_id = token.component_id.clone();
        let mut tentative = self.clone();
        let mut reports = vec![tentative.complete_activation(token)?];
        for capability in tentative.provided_capabilities(&component_id)? {
            let publication = if tentative.has_capability_generation(&capability) {
                tentative.replace_capability(&component_id, &capability)?
            } else {
                tentative.add_capability(&component_id, capability)?
            };
            reports.push(publication);
        }
        Ok((tentative, reports))
    }

    /// 在 clone 上完成不发布 capability 的 activation。
    pub(super) fn prepare_activation(
        &self,
        token: ActivationToken,
    ) -> Result<(Self, ReconciliationReport), ComponentGraphError> {
        let mut tentative = self.clone();
        let report = tentative.complete_activation(token)?;
        Ok((tentative, report))
    }

    pub(super) fn complete_deactivation(
        &mut self,
        token: DeactivationToken,
    ) -> Result<ReconciliationReport, ComponentGraphError> {
        self.validate_deactivation_token(&token)?;
        let component_id = token.component_id.clone();
        let mut report = self.report_for_component(component_id.clone());
        self.components
            .get_mut(&component_id)
            .expect("validated component should remain declared")
            .cleanup_disposition = None;
        self.transition(&component_id, ComponentState::Disposed, &mut report);
        match token.disposition {
            DeactivationDisposition::Reconcile => {
                self.reconcile_component(&component_id, &mut report, true)?;
            }
            DeactivationDisposition::Suspend => {
                self.transition(&component_id, ComponentState::Pending, &mut report);
            }
            DeactivationDisposition::Dispose => {}
        }
        self.refresh_report_failures(&mut report);
        Ok(report)
    }

    /// 撤销尚未触碰 concrete effects 的 deactivation，使失败 transaction 回到原有 active 状态。
    pub(super) fn rollback_deactivation(
        &mut self,
        token: DeactivationToken,
    ) -> Result<ReconciliationReport, ComponentGraphError> {
        self.validate_deactivation_token(&token)?;
        let component_id = token.component_id;
        let mut report = self.report_for_component(component_id.clone());
        self.components
            .get_mut(&component_id)
            .expect("validated component should remain declared")
            .cleanup_disposition = None;
        self.transition(&component_id, ComponentState::Active, &mut report);
        self.refresh_report_failures(&mut report);
        Ok(report)
    }

    pub(super) fn deactivate(
        &mut self,
        component_id: &str,
    ) -> Result<ReconciliationReport, ComponentGraphError> {
        self.begin_deactivation(component_id, DeactivationDisposition::Dispose)
    }

    pub(super) fn suspend(
        &mut self,
        component_id: &str,
    ) -> Result<ReconciliationReport, ComponentGraphError> {
        self.begin_deactivation(component_id, DeactivationDisposition::Suspend)
    }

    fn begin_deactivation(
        &mut self,
        component_id: &str,
        disposition: DeactivationDisposition,
    ) -> Result<ReconciliationReport, ComponentGraphError> {
        let state = self
            .components
            .get(component_id)
            .ok_or_else(|| ComponentGraphError::UnknownComponent {
                component_id: component_id.to_string(),
            })?
            .state;
        let mut report = self.report_for_component(component_id.to_string());
        match state {
            ComponentState::Active => {
                self.components
                    .get_mut(component_id)
                    .expect("component should remain declared")
                    .cleanup_disposition = Some(disposition);
                self.transition(component_id, ComponentState::Deactivating, &mut report);
                let epoch = self.component_epoch(component_id);
                report.deactivation_requests.push(DeactivationToken {
                    component_id: component_id.to_string(),
                    epoch,
                    disposition,
                });
            }
            ComponentState::Deactivating => {}
            _ => {
                let target = match disposition {
                    DeactivationDisposition::Suspend => ComponentState::Pending,
                    DeactivationDisposition::Reconcile | DeactivationDisposition::Dispose => {
                        ComponentState::Disposed
                    }
                };
                if state != target {
                    self.clear_failure(component_id);
                    self.components
                        .get_mut(component_id)
                        .expect("component should remain declared")
                        .cleanup_disposition = None;
                    self.transition(component_id, target, &mut report);
                }
            }
        }
        self.refresh_report_failures(&mut report);
        Ok(report)
    }

    pub(super) fn activate(
        &mut self,
        component_id: &str,
    ) -> Result<ReconciliationReport, ComponentGraphError> {
        if !self.components.contains_key(component_id) {
            return Err(ComponentGraphError::UnknownComponent {
                component_id: component_id.to_string(),
            });
        }
        let mut report = self.report_for_component(component_id.to_string());
        self.reconcile_component(component_id, &mut report, true)?;
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
        self.components
            .get_mut(&component_id)
            .expect("validated component should remain declared")
            .cleanup_disposition = None;
        let mut report = self.report_for_component(component_id.clone());
        self.transition(&component_id, ComponentState::Failed, &mut report);
        self.refresh_report_failures(&mut report);
        Ok(report)
    }

    pub(super) fn fail_deactivation(
        &mut self,
        token: DeactivationToken,
        reason: ComponentFailureReason,
        recoverable: bool,
    ) -> Result<ReconciliationReport, ComponentGraphError> {
        self.validate_deactivation_token(&token)?;
        debug_assert_eq!(reason.operation(), ComponentFailureOperation::Deactivation);
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
        self.components
            .get_mut(&component_id)
            .expect("validated component should remain declared")
            .cleanup_disposition = Some(token.disposition);
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

        let operation = record
            .failure
            .as_ref()
            .expect("failed component should retain a failure")
            .operation;
        let cleanup_disposition = record.cleanup_disposition;
        let mut report = self.report_for_component(component_id.to_string());
        match operation {
            ComponentFailureOperation::Activation => {
                self.begin_activation(component_id, &mut report)?;
            }
            ComponentFailureOperation::Deactivation => {
                let disposition = cleanup_disposition.ok_or_else(|| {
                    ComponentGraphError::FailureNotRecoverable {
                        component_id: component_id.to_string(),
                    }
                })?;
                self.clear_failure(component_id);
                self.transition(component_id, ComponentState::Deactivating, &mut report);
                report.deactivation_requests.push(DeactivationToken {
                    component_id: component_id.to_string(),
                    epoch: self.component_epoch(component_id),
                    disposition,
                });
            }
        }
        self.refresh_report_failures(&mut report);
        Ok(report)
    }

    #[cfg(test)]
    pub(super) fn state(&self, component_id: &str) -> Option<ComponentState> {
        self.components.get(component_id).map(|record| record.state)
    }

    pub(super) fn is_active(&self, component_id: &str) -> bool {
        self.components
            .get(component_id)
            .is_some_and(|record| record.state == ComponentState::Active)
    }

    #[cfg(test)]
    pub(super) fn epoch(&self, component_id: &str) -> Option<u64> {
        self.components
            .contains_key(component_id)
            .then(|| self.component_epoch(component_id))
    }

    #[cfg(test)]
    pub(super) fn inject_epoch_exhaustion(&mut self, component_id: &str) {
        assert!(
            self.components.contains_key(component_id),
            "component should exist before epoch exhaustion is injected"
        );
        self.component_epochs
            .insert(component_id.to_string(), u64::MAX);
    }

    #[cfg(test)]
    pub(super) fn inject_generation_exhaustion(&mut self, key: CapabilityKey) {
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
        Some(self.capabilities.contains_key(key))
    }

    pub(super) fn capabilities(&self) -> Vec<CapabilitySnapshot> {
        self.capabilities
            .iter()
            .map(|(key, provider_component)| CapabilitySnapshot {
                key: key.to_string(),
                provider_component: provider_component.clone(),
                generation: self.capability_generations.get(key).copied().unwrap_or(0),
            })
            .collect()
    }

    pub(super) fn capability(&self, key: &CapabilityKey) -> Option<CapabilitySnapshot> {
        let provider_component = self.capabilities.get(key)?;
        Some(CapabilitySnapshot {
            key: key.to_string(),
            provider_component: provider_component.clone(),
            generation: self.capability_generations.get(key).copied().unwrap_or(0),
        })
    }

    pub(super) fn has_capability(&self, key: &CapabilityKey) -> bool {
        self.capabilities.contains_key(key)
    }

    pub(super) fn has_capability_generation(&self, key: &CapabilityKey) -> bool {
        self.capability_generations.contains_key(key)
    }

    pub(super) fn provided_capabilities(
        &self,
        component_id: &str,
    ) -> Result<Vec<CapabilityKey>, ComponentGraphError> {
        let record = self.components.get(component_id).ok_or_else(|| {
            ComponentGraphError::UnknownComponent {
                component_id: component_id.to_string(),
            }
        })?;
        Ok(record.definition.provides.iter().cloned().collect())
    }

    /// 返回 required provider edge 的稳定 provider-first 顺序。
    pub(super) fn activation_order(&self) -> Vec<String> {
        topology_order(&self.component_definitions(), &self.providers)
            .expect("declared component topology must remain acyclic")
    }

    /// 返回 activation order 的精确逆序，供 consumer-first teardown 使用。
    pub(super) fn deactivation_order(&self) -> Vec<String> {
        let mut order = self.activation_order();
        order.reverse();
        order
    }

    pub(super) fn components(&self) -> Vec<ComponentSnapshot> {
        self.components
            .iter()
            .map(|(id, record)| ComponentSnapshot {
                id: id.clone(),
                state: record.state,
                epoch: self.component_epoch(id),
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
                        available: self.capabilities.contains_key(key),
                    })
                    .collect(),
                provides: record
                    .definition
                    .provides
                    .iter()
                    .map(ToString::to_string)
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
                    .iter()
                    .filter(|required| !self.capabilities.contains_key(*required))
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

    #[cfg(test)]
    fn validate_definition(
        &self,
        definition: &ComponentDefinition,
    ) -> Result<(), ComponentGraphError> {
        for capability in &definition.provides {
            if let Some(existing_component_id) = self.providers.get(capability) {
                return Err(ComponentGraphError::DuplicateCapabilityProvider {
                    capability: capability.to_string(),
                    existing_component_id: existing_component_id.clone(),
                });
            }
            if definition.required.contains(capability) {
                return Err(ComponentGraphError::SelfDependency {
                    component_id: definition.id.clone(),
                    capability: capability.to_string(),
                });
            }
        }

        let mut definitions = self.component_definitions();
        definitions.insert(definition.id.clone(), definition.clone());
        let mut providers = self.providers.clone();
        for capability in &definition.provides {
            providers.insert(capability.clone(), definition.id.clone());
        }
        topology_order(&definitions, &providers)?;
        Ok(())
    }

    fn validate_capability_provider(
        &self,
        provider_component: &str,
        capability: &CapabilityKey,
    ) -> Result<(), ComponentGraphError> {
        if !self.components.contains_key(provider_component) {
            return Err(ComponentGraphError::UnknownComponent {
                component_id: provider_component.to_string(),
            });
        }
        let Some(existing_component_id) = self.providers.get(capability) else {
            return Err(ComponentGraphError::UndeclaredCapabilityProvider {
                capability: capability.to_string(),
            });
        };
        if existing_component_id != provider_component {
            return Err(ComponentGraphError::CapabilityProviderMismatch {
                capability: capability.to_string(),
                existing_component_id: existing_component_id.clone(),
                requested_component_id: provider_component.to_string(),
            });
        }
        Ok(())
    }

    fn component_definitions(&self) -> BTreeMap<String, ComponentDefinition> {
        self.components
            .iter()
            .map(|(id, record)| (id.clone(), record.definition.clone()))
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
            self.reconcile_component(id, report, false)?;
        }
        Ok(())
    }

    fn reconcile_component(
        &mut self,
        id: &str,
        report: &mut ReconciliationReport,
        may_reactivate_disposed: bool,
    ) -> Result<(), ComponentGraphError> {
        let record = self
            .components
            .get(id)
            .expect("dependency index should reference a declared component");
        let required_ready = record
            .definition
            .required
            .iter()
            .all(|required| self.capabilities.contains_key(required));
        let state = record.state;
        match (state, required_ready) {
            (ComponentState::Declared | ComponentState::Pending, true) => {
                self.begin_activation(id, report)?;
            }
            (ComponentState::Disposed, true) if may_reactivate_disposed => {
                self.begin_activation(id, report)?;
            }
            (ComponentState::Declared | ComponentState::Pending, false) => {
                if state != ComponentState::Pending {
                    self.clear_failure(id);
                    self.transition(id, ComponentState::Pending, report);
                }
            }
            (ComponentState::Disposed, false) if may_reactivate_disposed => {
                self.clear_failure(id);
                self.transition(id, ComponentState::Pending, report);
            }
            (ComponentState::Failed, false)
                if self.components.get(id).is_some_and(|record| {
                    record.failure.as_ref().is_some_and(|failure| {
                        failure.operation == ComponentFailureOperation::Deactivation
                    })
                }) => {}
            (ComponentState::Activating | ComponentState::Failed, false) => {
                self.clear_failure(id);
                self.transition(id, ComponentState::Pending, report);
            }
            (ComponentState::Active, false) => {
                self.transition(id, ComponentState::Deactivating, report);
                let epoch = self.component_epoch(id);
                report.deactivation_requests.push(DeactivationToken {
                    component_id: id.to_string(),
                    epoch,
                    disposition: DeactivationDisposition::Reconcile,
                });
            }
            // `Disposed` 是 explicit removal 的稳定状态；只有显式 activate 或 dependency-loss
            // acknowledgement 才能重新进入 reconciliation，普通 coeffect 通知不能复活它。
            (ComponentState::Disposed, _) => {}
            _ => {}
        }
        Ok(())
    }

    fn begin_activation(
        &mut self,
        id: &str,
        report: &mut ReconciliationReport,
    ) -> Result<(), ComponentGraphError> {
        let epoch = self.component_epoch(id).checked_add(1).ok_or_else(|| {
            ComponentGraphError::ActivationEpochExhausted {
                component_id: id.to_string(),
            }
        })?;
        self.component_epochs.insert(id.to_string(), epoch);
        self.components
            .get_mut(id)
            .expect("component should remain declared during activation")
            .failure = None;
        self.components
            .get_mut(id)
            .expect("component should remain declared during activation")
            .cleanup_disposition = None;
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
                .all(|required| required == key || self.capabilities.contains_key(required));
            let will_begin = if is_replacement {
                record.definition.required.contains(key) && will_be_ready
            } else {
                matches!(
                    record.state,
                    ComponentState::Declared | ComponentState::Pending | ComponentState::Disposed
                ) && will_be_ready
            };
            if will_begin && self.component_epoch(&id) == u64::MAX {
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
        let current = self.component_epoch(&token.component_id);
        if current != token.epoch {
            return Err(ComponentGraphError::StaleActivation {
                component_id: token.component_id.clone(),
                expected: token.epoch,
                current,
            });
        }
        if record.state != ComponentState::Activating {
            return Err(ComponentGraphError::NotActivating {
                component_id: token.component_id.clone(),
            });
        }
        Ok(())
    }

    fn validate_deactivation_token(
        &self,
        token: &DeactivationToken,
    ) -> Result<(), ComponentGraphError> {
        let record = self.components.get(&token.component_id).ok_or_else(|| {
            ComponentGraphError::UnknownComponent {
                component_id: token.component_id.clone(),
            }
        })?;
        let current = self.component_epoch(&token.component_id);
        if current != token.epoch {
            return Err(ComponentGraphError::StaleDeactivation {
                component_id: token.component_id.clone(),
                expected: token.epoch,
                current,
            });
        }
        if record.state != ComponentState::Deactivating {
            return Err(ComponentGraphError::NotDeactivating {
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

    fn component_epoch(&self, component_id: &str) -> u64 {
        self.component_epochs
            .get(component_id)
            .copied()
            .unwrap_or(0)
    }

    fn transition(&mut self, id: &str, to: ComponentState, report: &mut ReconciliationReport) {
        let from = {
            let record = self
                .components
                .get_mut(id)
                .expect("component id should remain declared");
            let from = record.state;
            record.state = to;
            from
        };
        report.transitions.push(ComponentTransition {
            component_id: id.to_string(),
            from,
            to,
            epoch: self.component_epoch(id),
        });
    }
}

fn definition_indexes(
    definitions: &BTreeMap<String, ComponentDefinition>,
) -> Result<DefinitionIndexes, ComponentGraphError> {
    let mut providers = BTreeMap::new();
    let mut dependents = BTreeMap::<CapabilityKey, BTreeSet<String>>::new();
    for (component_id, definition) in definitions {
        for capability in &definition.provides {
            if definition.required.contains(capability) {
                return Err(ComponentGraphError::SelfDependency {
                    component_id: component_id.clone(),
                    capability: capability.to_string(),
                });
            }
            if let Some(existing_component_id) =
                providers.insert(capability.clone(), component_id.clone())
            {
                return Err(ComponentGraphError::DuplicateCapabilityProvider {
                    capability: capability.to_string(),
                    existing_component_id,
                });
            }
        }
        for dependency in definition.required.iter().chain(&definition.optional) {
            dependents
                .entry(dependency.clone())
                .or_default()
                .insert(component_id.clone());
        }
    }
    Ok(DefinitionIndexes {
        providers,
        dependents,
    })
}

fn required_component_dependents(
    definitions: &BTreeMap<String, ComponentDefinition>,
    providers: &BTreeMap<CapabilityKey, String>,
) -> BTreeMap<String, BTreeSet<String>> {
    let mut dependents = definitions
        .keys()
        .map(|component_id| (component_id.clone(), BTreeSet::new()))
        .collect::<BTreeMap<_, _>>();
    for (consumer_id, definition) in definitions {
        for dependency in &definition.required {
            let Some(provider_id) = providers.get(dependency) else {
                continue;
            };
            dependents
                .get_mut(provider_id)
                .expect("provider index should reference a desired component")
                .insert(consumer_id.clone());
        }
    }
    dependents
}

fn dependent_closure(
    roots: &BTreeSet<String>,
    dependents: &BTreeMap<String, BTreeSet<String>>,
) -> BTreeSet<String> {
    let mut pending = roots.iter().cloned().collect::<Vec<_>>();
    let mut closure = BTreeSet::new();
    while let Some(component_id) = pending.pop() {
        if !closure.insert(component_id.clone()) {
            continue;
        }
        if let Some(component_dependents) = dependents.get(&component_id) {
            pending.extend(component_dependents.iter().cloned());
        }
    }
    closure
}

fn topology_order(
    definitions: &BTreeMap<String, ComponentDefinition>,
    providers: &BTreeMap<CapabilityKey, String>,
) -> Result<Vec<String>, ComponentGraphError> {
    let mut dependents = definitions
        .keys()
        .map(|id| (id.clone(), BTreeSet::new()))
        .collect::<BTreeMap<_, _>>();
    let mut incoming = definitions
        .keys()
        .map(|id| (id.clone(), 0_usize))
        .collect::<BTreeMap<_, _>>();

    for (consumer_id, definition) in definitions {
        for required in &definition.required {
            let Some(provider_id) = providers.get(required) else {
                continue;
            };
            if dependents
                .get_mut(provider_id)
                .expect("provider index must reference a declared component")
                .insert(consumer_id.clone())
            {
                *incoming
                    .get_mut(consumer_id)
                    .expect("consumer must remain declared") += 1;
            }
        }
    }

    let mut ready = incoming
        .iter()
        .filter(|(_, count)| **count == 0)
        .map(|(id, _)| id.clone())
        .collect::<BTreeSet<_>>();
    let mut order = Vec::with_capacity(definitions.len());
    while let Some(id) = ready.pop_first() {
        order.push(id.clone());
        for dependent in dependents
            .get(&id)
            .expect("topology node must have a dependent set")
        {
            let count = incoming
                .get_mut(dependent)
                .expect("dependent must remain declared");
            *count -= 1;
            if *count == 0 {
                ready.insert(dependent.clone());
            }
        }
    }

    if order.len() == definitions.len() {
        Ok(order)
    } else {
        Err(ComponentGraphError::DependencyCycle {
            component_ids: definitions
                .keys()
                .filter(|id| is_cycle_member(id, &dependents))
                .cloned()
                .collect(),
        })
    }
}

fn is_cycle_member(component_id: &str, dependents: &BTreeMap<String, BTreeSet<String>>) -> bool {
    let mut pending = dependents
        .get(component_id)
        .into_iter()
        .flat_map(BTreeSet::iter)
        .cloned()
        .collect::<Vec<_>>();
    let mut visited = BTreeSet::new();
    while let Some(current) = pending.pop() {
        if current == component_id {
            return true;
        }
        if visited.insert(current.clone())
            && let Some(next) = dependents.get(&current)
        {
            pending.extend(next.iter().cloned());
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn declare_provider(graph: &mut ComponentGraph, component_id: &str, capability: &str) {
        let report = graph
            .declare(ComponentDefinition::new(component_id).provides(capability))
            .expect("provider component should declare");
        for token in report.activation_requests {
            graph
                .complete_activation(token)
                .expect("provider activation should complete");
        }
    }

    #[test]
    fn missing_required_capability_keeps_component_pending() {
        let mut graph = ComponentGraph::default();
        declare_provider(&mut graph, "llm-provider", "llm");
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
            .add_capability("llm-provider", "llm")
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
        declare_provider(&mut graph, "llm-provider", "llm");
        graph
            .add_capability("llm-provider", "llm")
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
        declare_provider(&mut graph, "llm-provider", "llm");
        graph
            .add_capability("llm-provider", "llm")
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
            .replace_capability("llm-provider", &CapabilityKey::from("llm"))
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
                ("z-agent", ComponentState::Deactivating),
            ]
        );
        assert_eq!(
            replacement
                .deactivation_requests
                .iter()
                .map(|token| (token.component_id(), token.epoch()))
                .collect::<Vec<_>>(),
            vec![("a-agent", 1), ("z-agent", 1)]
        );
        assert!(replacement.activation_requests.is_empty());
        assert_eq!(graph.state("a-agent"), Some(ComponentState::Deactivating));
        assert_eq!(graph.state("z-agent"), Some(ComponentState::Deactivating));

        let mut activation_tokens = Vec::new();
        for token in replacement.deactivation_requests.into_iter().rev() {
            let completion = graph
                .complete_deactivation(token)
                .expect("current deactivation should complete");
            activation_tokens.extend(completion.activation_requests);
        }
        activation_tokens.sort_by(|left, right| left.component_id().cmp(right.component_id()));
        assert_eq!(
            activation_tokens
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
        declare_provider(&mut graph, "metrics-provider", "metrics");
        declare_provider(&mut graph, "tracing-provider", "tracing");
        let declaration = graph
            .declare(ComponentDefinition::new("agent").observes("metrics"))
            .expect("unique component should declare");
        graph
            .complete_activation(declaration.activation_requests[0].clone())
            .expect("initial activation should complete");

        let unrelated = graph
            .add_capability("tracing-provider", "tracing")
            .expect("unrelated capability should add");
        assert!(unrelated.affected_components.is_empty());
        assert!(unrelated.transitions.is_empty());
        assert_eq!(graph.epoch("agent"), Some(1));

        let observed = graph
            .add_capability("metrics-provider", "metrics")
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
        declare_provider(&mut graph, "metrics-provider", "metrics");
        graph
            .add_capability("metrics-provider", "metrics")
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
            .remove_capability("metrics-provider", &CapabilityKey::from("metrics"))
            .expect("observed capability removal should reconcile");
        assert_eq!(removed.affected_components, vec!["agent"]);
        assert!(removed.transitions.is_empty());
        assert!(removed.activation_requests.is_empty());
        assert_eq!(graph.state("agent"), Some(ComponentState::Failed));
        assert_eq!(graph.epoch("agent"), Some(1));
    }

    #[test]
    fn removing_required_capability_waits_for_concrete_deactivation() {
        let mut graph = ComponentGraph::default();
        declare_provider(&mut graph, "llm-provider", "llm");
        graph
            .add_capability("llm-provider", "llm")
            .expect("initial capability should add");
        let declaration = graph
            .declare(ComponentDefinition::new("agent").requires("llm"))
            .expect("unique component should declare");
        graph
            .complete_activation(declaration.activation_requests[0].clone())
            .expect("initial activation should complete");

        let removed = graph
            .remove_capability("llm-provider", &CapabilityKey::from("llm"))
            .expect("required capability removal should reconcile");

        assert_eq!(graph.state("agent"), Some(ComponentState::Deactivating));
        assert_eq!(
            removed.transitions,
            vec![ComponentTransition {
                component_id: "agent".to_string(),
                from: ComponentState::Active,
                to: ComponentState::Deactivating,
                epoch: 1,
            }]
        );
        assert_eq!(
            removed
                .capability_change
                .as_ref()
                .map(|change| change.reason.as_str()),
            Some("removed")
        );
        assert!(graph.pending().is_empty());
        let completion = graph
            .complete_deactivation(removed.deactivation_requests[0].clone())
            .expect("current deactivation should complete");
        assert_eq!(graph.state("agent"), Some(ComponentState::Pending));
        assert_eq!(
            completion
                .transitions
                .iter()
                .map(|transition| transition.to)
                .collect::<Vec<_>>(),
            vec![ComponentState::Disposed, ComponentState::Pending]
        );
        assert_eq!(graph.pending()[0].missing_dependencies, vec!["llm"]);
        assert!(graph.failures().is_empty());
    }

    #[test]
    fn deactivation_failure_is_typed_and_completion_cannot_override_it() {
        let mut graph = ComponentGraph::default();
        declare_provider(&mut graph, "llm-provider", "llm");
        graph
            .add_capability("llm-provider", "llm")
            .expect("initial capability should add");
        let declaration = graph
            .declare(ComponentDefinition::new("agent").requires("llm"))
            .expect("consumer should declare");
        graph
            .complete_activation(declaration.activation_requests[0].clone())
            .expect("initial activation should complete");

        let removed = graph
            .remove_capability("llm-provider", &CapabilityKey::from("llm"))
            .expect("required capability should begin deactivation");
        let token = removed.deactivation_requests[0].clone();
        let failure = graph
            .fail_deactivation(
                token.clone(),
                ComponentFailureReason::QuiescenceRejected,
                false,
            )
            .expect("current deactivation should be fail-able");

        assert_eq!(graph.state("agent"), Some(ComponentState::Failed));
        assert_eq!(failure.failures[0].operation.as_str(), "deactivation");
        assert_eq!(failure.failures[0].reason.code(), "quiescence_rejected");
        assert_eq!(
            graph.complete_deactivation(token),
            Err(ComponentGraphError::NotDeactivating {
                component_id: "agent".to_string(),
            })
        );
    }

    #[test]
    fn deactivation_retry_preserves_suspend_and_dispose_dispositions() {
        let mut graph = ComponentGraph::default();
        let declaration = graph
            .declare(ComponentDefinition::new("agent"))
            .expect("component should declare");
        graph
            .complete_activation(declaration.activation_requests[0].clone())
            .expect("initial activation should complete");

        let suspended = graph.suspend("agent").expect("suspend should start");
        let suspend_token = suspended.deactivation_requests[0].clone();
        graph
            .fail_deactivation(
                suspend_token,
                ComponentFailureReason::QuiescenceRejected,
                true,
            )
            .expect("suspend cleanup should fail recoverably");
        let suspend_retry = graph.retry("agent").expect("suspend cleanup should retry");
        assert_eq!(
            suspend_retry.deactivation_requests[0].disposition,
            DeactivationDisposition::Suspend
        );
        graph
            .complete_deactivation(suspend_retry.deactivation_requests[0].clone())
            .expect("suspend cleanup retry should complete");
        assert_eq!(graph.state("agent"), Some(ComponentState::Pending));

        let reactivation = graph
            .activate("agent")
            .expect("component should reactivate");
        graph
            .complete_activation(reactivation.activation_requests[0].clone())
            .expect("reactivation should complete");
        let disposed = graph.deactivate("agent").expect("disposal should start");
        let dispose_token = disposed.deactivation_requests[0].clone();
        graph
            .fail_deactivation(
                dispose_token,
                ComponentFailureReason::EffectDisposalRejected,
                true,
            )
            .expect("disposal cleanup should fail recoverably");
        let dispose_retry = graph.retry("agent").expect("disposal cleanup should retry");
        assert_eq!(
            dispose_retry.deactivation_requests[0].disposition,
            DeactivationDisposition::Dispose
        );
        graph
            .complete_deactivation(dispose_retry.deactivation_requests[0].clone())
            .expect("disposal cleanup retry should complete");
        assert_eq!(graph.state("agent"), Some(ComponentState::Disposed));
    }

    #[test]
    fn deactivation_retry_preserves_dependency_reconciliation() {
        let mut graph = ComponentGraph::default();
        declare_provider(&mut graph, "llm-provider", "llm");
        graph
            .add_capability("llm-provider", "llm")
            .expect("initial capability should add");
        let declaration = graph
            .declare(ComponentDefinition::new("agent").requires("llm"))
            .expect("consumer should declare");
        graph
            .complete_activation(declaration.activation_requests[0].clone())
            .expect("initial activation should complete");

        let removed = graph
            .remove_capability("llm-provider", &CapabilityKey::from("llm"))
            .expect("dependency removal should begin reconciliation");
        graph
            .fail_deactivation(
                removed.deactivation_requests[0].clone(),
                ComponentFailureReason::QuiescenceRejected,
                true,
            )
            .expect("reconciliation cleanup should fail recoverably");
        let retry = graph
            .retry("agent")
            .expect("reconciliation cleanup should retry");
        assert_eq!(
            retry.deactivation_requests[0].disposition,
            DeactivationDisposition::Reconcile
        );
        graph
            .complete_deactivation(retry.deactivation_requests[0].clone())
            .expect("reconciliation cleanup retry should complete");
        assert_eq!(graph.state("agent"), Some(ComponentState::Pending));
        assert_eq!(graph.pending()[0].missing_dependencies, vec!["llm"]);
    }

    #[test]
    fn stale_deactivation_completion_cannot_modify_a_fresh_activation_epoch() {
        let mut graph = ComponentGraph::default();
        declare_provider(&mut graph, "llm-provider", "llm");
        graph
            .add_capability("llm-provider", "llm")
            .expect("initial capability should add");
        let declaration = graph
            .declare(ComponentDefinition::new("agent").requires("llm"))
            .expect("consumer should declare");
        graph
            .complete_activation(declaration.activation_requests[0].clone())
            .expect("initial activation should complete");

        let removed = graph
            .remove_capability("llm-provider", &CapabilityKey::from("llm"))
            .expect("required capability should begin deactivation");
        let stale = removed.deactivation_requests[0].clone();
        graph
            .complete_deactivation(stale.clone())
            .expect("current deactivation should complete");
        graph
            .add_capability("llm-provider", "llm")
            .expect("capability recovery should begin a fresh epoch");

        assert_eq!(graph.state("agent"), Some(ComponentState::Activating));
        assert_eq!(graph.epoch("agent"), Some(2));
        assert_eq!(
            graph.complete_deactivation(stale),
            Err(ComponentGraphError::StaleDeactivation {
                component_id: "agent".to_string(),
                expected: 1,
                current: 2,
            })
        );
        assert_eq!(graph.state("agent"), Some(ComponentState::Activating));
    }

    #[test]
    fn duplicate_declaration_and_repeated_capability_mutations_are_no_ops() {
        let mut graph = ComponentGraph::default();
        declare_provider(&mut graph, "llm-provider", "llm");
        declare_provider(&mut graph, "metrics-provider", "metrics");
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
            .add_capability("llm-provider", "llm")
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
            graph.add_capability("llm-provider", "llm"),
            Ok(ReconciliationReport::default())
        );
        graph
            .complete_activation(added.activation_requests[0].clone())
            .expect("current activation should complete");
        graph
            .remove_capability("llm-provider", &CapabilityKey::from("llm"))
            .expect("required capability should remove");
        assert_eq!(
            graph.remove_capability("llm-provider", &CapabilityKey::from("llm")),
            Ok(ReconciliationReport::default())
        );
        let observed = graph
            .add_capability("metrics-provider", "metrics")
            .expect("unindexed capability should add");
        assert!(
            observed.affected_components.is_empty(),
            "duplicate declaration must not add observed dependency edges"
        );
    }

    #[test]
    fn pending_dependencies_and_index_use_stable_natural_order() {
        let mut graph = ComponentGraph::default();
        declare_provider(&mut graph, "llm-provider", "llm");
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
            .add_capability("llm-provider", "llm")
            .expect("capability add should reconcile indexed dependents");
        assert_eq!(report.affected_components, vec!["a-agent", "z-agent"]);
    }

    #[test]
    fn required_capability_recovery_clears_failure_and_starts_a_new_epoch() {
        let mut graph = ComponentGraph::default();
        declare_provider(&mut graph, "llm-provider", "llm");
        graph
            .add_capability("llm-provider", "llm")
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
            .remove_capability("llm-provider", &CapabilityKey::from("llm"))
            .expect("required capability removal should reconcile");
        assert_eq!(graph.state("agent"), Some(ComponentState::Pending));
        assert!(removal.failures.is_empty());
        assert!(graph.failures().is_empty());

        let recovery = graph
            .add_capability("llm-provider", "llm")
            .expect("required capability recovery should reconcile");
        assert_eq!(graph.state("agent"), Some(ComponentState::Activating));
        assert_eq!(recovery.activation_requests[0].epoch(), 2);
        assert!(recovery.failures.is_empty());
    }

    #[test]
    fn reconciliation_reports_all_current_failures_in_component_order() {
        let mut graph = ComponentGraph::default();
        declare_provider(&mut graph, "metrics-provider", "metrics");
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
            .add_capability("metrics-provider", "metrics")
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
        declare_provider(&mut graph, "llm-provider", "llm");
        graph
            .declare(ComponentDefinition::new("agent").requires("llm"))
            .expect("unique component should declare");
        graph.inject_epoch_exhaustion("agent");

        assert_eq!(
            graph.add_capability("llm-provider", "llm"),
            Err(ComponentGraphError::ActivationEpochExhausted {
                component_id: "agent".to_string(),
            })
        );
        assert!(!graph.has_capability(&CapabilityKey::from("llm")));
        assert_eq!(graph.state("agent"), Some(ComponentState::Pending));
        assert_eq!(graph.epoch("agent"), Some(u64::MAX));
    }

    #[test]
    fn provider_declaration_failures_are_typed_and_mutation_free() {
        let mut graph = ComponentGraph::default();
        declare_provider(&mut graph, "llm-provider", "llm");
        let components_before = graph.components();
        let order_before = graph.activation_order();

        assert_eq!(
            graph.declare(ComponentDefinition::new("other-provider").provides("llm")),
            Err(ComponentGraphError::DuplicateCapabilityProvider {
                capability: "llm".to_string(),
                existing_component_id: "llm-provider".to_string(),
            })
        );
        assert_eq!(
            graph.declare(
                ComponentDefinition::new("self-dependent")
                    .requires("self-capability")
                    .provides("self-capability"),
            ),
            Err(ComponentGraphError::SelfDependency {
                component_id: "self-dependent".to_string(),
                capability: "self-capability".to_string(),
            })
        );
        assert_eq!(graph.components(), components_before);
        assert_eq!(graph.activation_order(), order_before);
        assert_eq!(
            graph.add_capability("other-provider", "llm"),
            Err(ComponentGraphError::UnknownComponent {
                component_id: "other-provider".to_string(),
            })
        );
    }

    #[test]
    fn provider_declaration_that_closes_a_cycle_is_rejected_before_mutation() {
        let mut graph = ComponentGraph::default();
        graph
            .declare(
                ComponentDefinition::new("component-a")
                    .requires("capability-b")
                    .provides("capability-a"),
            )
            .expect("unresolved provider may be declared later");
        graph
            .declare(ComponentDefinition::new("downstream").requires("capability-a"))
            .expect("downstream consumer should remain outside the unresolved cycle");
        let components_before = graph.components();

        assert_eq!(
            graph.declare(
                ComponentDefinition::new("component-b")
                    .requires("capability-a")
                    .provides("capability-b"),
            ),
            Err(ComponentGraphError::DependencyCycle {
                component_ids: vec!["component-a".to_string(), "component-b".to_string()],
            })
        );
        assert_eq!(graph.components(), components_before);
        assert_eq!(
            graph.add_capability("component-b", "capability-b"),
            Err(ComponentGraphError::UnknownComponent {
                component_id: "component-b".to_string(),
            })
        );
    }

    #[test]
    fn topology_order_is_stable_and_observed_edges_do_not_constrain_activation() {
        fn graph_from_order(order: &[&str]) -> ComponentGraph {
            let definitions = BTreeMap::from([
                (
                    "a-observer",
                    ComponentDefinition::new("a-observer").observes("z-capability"),
                ),
                (
                    "agent-a",
                    ComponentDefinition::new("agent-a")
                        .requires("llm")
                        .requires("prompt"),
                ),
                (
                    "agent-z",
                    ComponentDefinition::new("agent-z")
                        .requires("llm")
                        .requires("prompt"),
                ),
                ("llm", ComponentDefinition::new("llm").provides("llm")),
                (
                    "prompt",
                    ComponentDefinition::new("prompt")
                        .requires("tools")
                        .provides("prompt"),
                ),
                ("tools", ComponentDefinition::new("tools").provides("tools")),
                (
                    "z-provider",
                    ComponentDefinition::new("z-provider").provides("z-capability"),
                ),
            ]);
            let mut graph = ComponentGraph::default();
            for id in order {
                graph
                    .declare(definitions[*id].clone())
                    .expect("acyclic definition should declare");
            }
            graph
        }

        let ids = [
            "z-provider",
            "agent-z",
            "prompt",
            "tools",
            "agent-a",
            "llm",
            "a-observer",
        ];
        let forward = graph_from_order(&ids);
        let reverse = graph_from_order(&ids.iter().rev().copied().collect::<Vec<_>>());
        let expected = vec![
            "a-observer".to_string(),
            "llm".to_string(),
            "tools".to_string(),
            "prompt".to_string(),
            "agent-a".to_string(),
            "agent-z".to_string(),
            "z-provider".to_string(),
        ];

        assert_eq!(forward.activation_order(), expected);
        assert_eq!(reverse.activation_order(), expected);
        assert_eq!(
            forward.deactivation_order(),
            expected.into_iter().rev().collect::<Vec<_>>()
        );
    }

    #[test]
    fn capability_mutation_requires_the_declared_provider_without_partial_state() {
        let mut graph = ComponentGraph::default();
        declare_provider(&mut graph, "llm-provider", "llm");
        declare_provider(&mut graph, "metrics-provider", "metrics");
        let components_before = graph.components();

        assert_eq!(
            graph.add_capability("metrics-provider", "llm"),
            Err(ComponentGraphError::CapabilityProviderMismatch {
                capability: "llm".to_string(),
                existing_component_id: "llm-provider".to_string(),
                requested_component_id: "metrics-provider".to_string(),
            })
        );
        assert_eq!(
            graph.add_capability("llm-provider", "undeclared"),
            Err(ComponentGraphError::UndeclaredCapabilityProvider {
                capability: "undeclared".to_string(),
            })
        );
        assert!(graph.capabilities().is_empty());
        assert_eq!(graph.components(), components_before);

        graph
            .add_capability("llm-provider", "llm")
            .expect("declared provider should publish");
        let active_before = graph.capabilities();
        assert_eq!(active_before[0].provider_component, "llm-provider");
        assert_eq!(
            graph.remove_capability("metrics-provider", &CapabilityKey::from("llm")),
            Err(ComponentGraphError::CapabilityProviderMismatch {
                capability: "llm".to_string(),
                existing_component_id: "llm-provider".to_string(),
                requested_component_id: "metrics-provider".to_string(),
            })
        );
        assert_eq!(graph.capabilities(), active_before);
        assert_eq!(
            graph.replace_capability("metrics-provider", &CapabilityKey::from("llm")),
            Err(ComponentGraphError::CapabilityProviderMismatch {
                capability: "llm".to_string(),
                existing_component_id: "llm-provider".to_string(),
                requested_component_id: "metrics-provider".to_string(),
            })
        );
        assert_eq!(graph.capabilities(), active_before);
    }

    #[test]
    fn atomic_activation_publication_keeps_independent_change_reports() {
        let mut graph = ComponentGraph::default();
        let declaration = graph
            .declare(
                ComponentDefinition::new("provider")
                    .provides("alpha")
                    .provides("beta"),
            )
            .expect("provider declaration should activate");

        let reports = graph
            .complete_activation_and_publish(declaration.activation_requests[0].clone())
            .expect("activation and publication should commit atomically");

        assert_eq!(reports.len(), 3);
        assert!(reports[0].capability_change.is_none());
        assert_eq!(
            reports[1]
                .capability_change
                .as_ref()
                .map(|change| change.key.as_str()),
            Some("alpha")
        );
        assert_eq!(
            reports[2]
                .capability_change
                .as_ref()
                .map(|change| change.key.as_str()),
            Some("beta")
        );
        assert_eq!(
            graph
                .capabilities()
                .into_iter()
                .map(|capability| capability.key)
                .collect::<Vec<_>>(),
            vec!["alpha", "beta"]
        );
        assert_eq!(graph.state("provider"), Some(ComponentState::Active));
    }

    #[test]
    fn generation_exhaustion_rejects_replacement_without_mutation() {
        let mut graph = ComponentGraph::default();
        let capability = CapabilityKey::from("llm");
        declare_provider(&mut graph, "llm-provider", "llm");
        graph
            .add_capability("llm-provider", capability.clone())
            .expect("initial capability should add");
        graph.inject_generation_exhaustion(capability.clone());

        assert_eq!(
            graph.replace_capability("llm-provider", &capability),
            Err(ComponentGraphError::CapabilityGenerationExhausted {
                capability: "llm".to_string(),
            })
        );
        assert!(graph.has_capability(&capability));
        assert_eq!(graph.capabilities()[0].generation, u64::MAX);
    }

    #[test]
    fn definition_reconciliation_mixed_diff_is_input_order_independent() {
        let observed = [
            ComponentDefinition::new("database").provides("database"),
            ComponentDefinition::new("service")
                .requires("database")
                .provides("service"),
            ComponentDefinition::new("ui").requires("service"),
            ComponentDefinition::new("removed"),
        ];
        let desired = vec![
            ComponentDefinition::new("storage").provides("database"),
            ComponentDefinition::new("service")
                .requires("database")
                .observes("metrics")
                .provides("service"),
            ComponentDefinition::new("ui").requires("service"),
            ComponentDefinition::new("added"),
        ];
        let mut graph = ComponentGraph::default();
        let initial = graph
            .prepare_definition_reconciliation(observed)
            .expect("initial definitions should preflight");
        graph
            .commit_definition_reconciliation(initial)
            .expect("initial definitions should commit");

        let forward = graph
            .prepare_definition_reconciliation(desired.clone())
            .expect("mixed desired definitions should preflight");
        let reverse = graph
            .prepare_definition_reconciliation(desired.into_iter().rev())
            .expect("input order must not affect preflight");

        assert_eq!(forward.retirement_order(), reverse.retirement_order());
        assert_eq!(forward.activation_order(), reverse.activation_order());
        assert_eq!(
            forward.retirement_order(),
            ["ui", "service", "removed", "database"]
        );
        assert_eq!(
            forward.activation_order(),
            ["added", "storage", "service", "ui"]
        );
    }

    #[test]
    fn removed_component_epoch_tombstone_rejects_old_tokens_after_readd() {
        let mut graph = ComponentGraph::default();
        let declaration = graph
            .declare(ComponentDefinition::new("agent"))
            .expect("agent should declare");
        let stale_activation = declaration.activation_requests[0].clone();
        graph
            .complete_activation(stale_activation.clone())
            .expect("agent should activate");
        let removal = graph
            .prepare_definition_reconciliation([])
            .expect("removal should preflight");
        let deactivation = graph
            .deactivate("agent")
            .expect("active agent should begin deactivation")
            .deactivation_requests
            .into_iter()
            .next()
            .expect("deactivation token should exist");
        graph
            .complete_deactivation(deactivation.clone())
            .expect("old agent should dispose");
        graph
            .commit_definition_reconciliation(removal)
            .expect("removed definition should commit");
        assert_eq!(graph.state("agent"), None);
        assert_eq!(graph.component_epochs.get("agent"), Some(&1));

        let addition = graph
            .prepare_definition_reconciliation([ComponentDefinition::new("agent")])
            .expect("same id should be addable after removal");
        graph
            .commit_definition_reconciliation(addition)
            .expect("fresh definition should commit");
        let fresh = graph
            .activate("agent")
            .expect("fresh agent should begin activation")
            .activation_requests
            .into_iter()
            .next()
            .expect("fresh activation token should exist");
        assert_eq!(fresh.epoch(), 2);
        assert_eq!(
            graph.complete_activation(stale_activation),
            Err(ComponentGraphError::StaleActivation {
                component_id: "agent".to_string(),
                expected: 1,
                current: 2,
            })
        );
        assert_eq!(
            graph.complete_deactivation(deactivation),
            Err(ComponentGraphError::StaleDeactivation {
                component_id: "agent".to_string(),
                expected: 1,
                current: 2,
            })
        );
    }

    #[test]
    fn same_id_definition_replacement_rejects_old_tokens() {
        let mut graph = ComponentGraph::default();
        let initial = graph
            .prepare_definition_reconciliation([
                ComponentDefinition::new("agent").observes("old-observation")
            ])
            .expect("initial definition should preflight");
        graph
            .commit_definition_reconciliation(initial)
            .expect("initial definition should commit");
        let stale_activation = graph
            .activate("agent")
            .expect("initial agent should begin activation")
            .activation_requests
            .into_iter()
            .next()
            .expect("initial activation token should exist");
        graph
            .complete_activation(stale_activation.clone())
            .expect("initial agent should activate");

        let replacement = graph
            .prepare_definition_reconciliation([
                ComponentDefinition::new("agent").observes("fresh-observation")
            ])
            .expect("same-id replacement should preflight");
        let stale_deactivation = graph
            .suspend("agent")
            .expect("old agent should begin suspension")
            .deactivation_requests
            .into_iter()
            .next()
            .expect("old deactivation token should exist");
        graph
            .complete_deactivation(stale_deactivation.clone())
            .expect("old agent should suspend");
        assert_eq!(
            graph
                .commit_definition_reconciliation(replacement)
                .expect("same-id replacement should commit"),
            vec!["agent"]
        );
        let fresh_activation = graph
            .activate("agent")
            .expect("replacement agent should begin activation")
            .activation_requests
            .into_iter()
            .next()
            .expect("fresh activation token should exist");

        assert_eq!(fresh_activation.epoch(), 2);
        assert_eq!(
            graph.complete_activation(stale_activation),
            Err(ComponentGraphError::StaleActivation {
                component_id: "agent".to_string(),
                expected: 1,
                current: 2,
            })
        );
        assert_eq!(
            graph.complete_deactivation(stale_deactivation),
            Err(ComponentGraphError::StaleDeactivation {
                component_id: "agent".to_string(),
                expected: 1,
                current: 2,
            })
        );
        assert_eq!(graph.state("agent"), Some(ComponentState::Activating));
        assert_eq!(
            graph.components()[0]
                .optional
                .iter()
                .map(|dependency| dependency.key.as_str())
                .collect::<Vec<_>>(),
            vec!["fresh-observation"]
        );
    }

    #[test]
    fn definition_preflight_rejects_invalid_batch_without_mutation() {
        let mut graph = ComponentGraph::default();
        let initial = graph
            .prepare_definition_reconciliation([
                ComponentDefinition::new("provider").provides("shared"),
                ComponentDefinition::new("consumer").requires("shared"),
            ])
            .expect("initial definitions should preflight");
        graph
            .commit_definition_reconciliation(initial)
            .expect("initial definitions should commit");
        let components_before = graph.components();
        let order_before = graph.activation_order();

        assert!(matches!(
            graph.prepare_definition_reconciliation([
                ComponentDefinition::new("provider-a").provides("shared"),
                ComponentDefinition::new("provider-b").provides("shared"),
            ]),
            Err(ComponentGraphError::DuplicateCapabilityProvider {
                capability,
                existing_component_id,
            }) if capability == "shared" && existing_component_id == "provider-a"
        ));
        assert!(matches!(
            graph.prepare_definition_reconciliation([
                ComponentDefinition::new("a")
                    .requires("b-capability")
                    .provides("a-capability"),
                ComponentDefinition::new("b")
                    .requires("a-capability")
                    .provides("b-capability"),
            ]),
            Err(ComponentGraphError::DependencyCycle {
                component_ids,
            }) if component_ids == ["a", "b"]
        ));
        assert_eq!(graph.components(), components_before);
        assert_eq!(graph.activation_order(), order_before);
    }
}
