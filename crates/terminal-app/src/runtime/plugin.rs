use std::{
    borrow::Borrow,
    collections::{BTreeMap, BTreeSet},
    error::Error,
    fmt,
    num::NonZeroU32,
};

use super::lifecycle::{CapabilityKey, ComponentDefinition};

/// `PluginTypeId` 标识编译进宿主的稳定 plugin implementation type。
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(super) struct PluginTypeId(String);

impl PluginTypeId {
    pub(super) fn try_new(value: impl Into<String>) -> Result<Self, PluginTypeIdError> {
        let value = value.into();
        if value.is_empty() {
            return Err(PluginTypeIdError::Empty);
        }
        let bytes = value.as_bytes();
        if !bytes.first().is_some_and(u8::is_ascii_alphanumeric)
            || !bytes.last().is_some_and(u8::is_ascii_alphanumeric)
            || bytes
                .iter()
                .any(|byte| !byte.is_ascii_lowercase() && !byte.is_ascii_digit() && *byte != b'-')
            || bytes.windows(2).any(|pair| pair == b"--")
        {
            return Err(PluginTypeIdError::InvalidFormat);
        }
        Ok(Self(value))
    }

    pub(super) fn as_str(&self) -> &str {
        &self.0
    }
}

/// `PluginTypeIdError` 只暴露封闭校验原因，不保留无效的原始 id。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PluginTypeIdError {
    Empty,
    InvalidFormat,
}

impl fmt::Display for PluginTypeIdError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let kind = match self {
            Self::Empty => "empty_plugin_type_id",
            Self::InvalidFormat => "invalid_plugin_type_id_format",
        };
        formatter.write_str(kind)
    }
}

impl Error for PluginTypeIdError {}

impl fmt::Debug for PluginTypeId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("PluginTypeId")
            .field(&self.0)
            .finish()
    }
}

impl fmt::Display for PluginTypeId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// `PluginComponentId` 标识 composition 中稳定、可替换的 component slot。
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(super) struct PluginComponentId(String);

impl PluginComponentId {
    pub(super) fn try_new(value: impl Into<String>) -> Result<Self, PluginComponentIdError> {
        let value = value.into();
        if value.is_empty() {
            return Err(PluginComponentIdError::Empty);
        }
        let bytes = value.as_bytes();
        if !bytes.first().is_some_and(u8::is_ascii_alphanumeric)
            || !bytes.last().is_some_and(u8::is_ascii_alphanumeric)
            || bytes
                .iter()
                .any(|byte| !byte.is_ascii_lowercase() && !byte.is_ascii_digit() && *byte != b'_')
            || bytes.windows(2).any(|pair| pair == b"__")
        {
            return Err(PluginComponentIdError::InvalidFormat);
        }
        Ok(Self(value))
    }

    pub(super) fn as_str(&self) -> &str {
        &self.0
    }
}

impl Borrow<str> for PluginComponentId {
    fn borrow(&self) -> &str {
        self.as_str()
    }
}

/// `PluginComponentIdError` 不保留未通过校验的原始 slot id。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PluginComponentIdError {
    Empty,
    InvalidFormat,
}

impl fmt::Display for PluginComponentIdError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let kind = match self {
            Self::Empty => "empty_plugin_component_id",
            Self::InvalidFormat => "invalid_plugin_component_id_format",
        };
        formatter.write_str(kind)
    }
}

impl Error for PluginComponentIdError {}

impl fmt::Debug for PluginComponentId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("PluginComponentId")
            .field(&self.0)
            .finish()
    }
}

impl fmt::Display for PluginComponentId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// `PluginReloadPolicy` 是 loader 可执行的封闭 replacement policy。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PluginReloadPolicy {
    Replace,
}

impl PluginReloadPolicy {
    pub(super) const fn as_str(self) -> &'static str {
        match self {
            Self::Replace => "replace",
        }
    }
}

/// `PluginTrust` 记录 plugin code 的宿主信任来源。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PluginTrust {
    Builtin,
}

impl PluginTrust {
    pub(super) const fn as_str(self) -> &'static str {
        match self {
            Self::Builtin => "builtin",
        }
    }
}

/// `PluginDescriptor` 是 immutable loader metadata，不包含 runtime state 或 config body。
#[derive(Clone)]
pub(super) struct PluginDescriptor {
    type_id: PluginTypeId,
    // Loader metadata 保留 display label，但 runtime inspection 明确不投影它。
    _display_name: &'static str,
    config_schema_version: NonZeroU32,
    required: BTreeSet<CapabilityKey>,
    observed: BTreeSet<CapabilityKey>,
    provided: BTreeSet<CapabilityKey>,
    reload_policy: PluginReloadPolicy,
    trust: PluginTrust,
}

impl PluginDescriptor {
    pub(super) fn builder(
        type_id: PluginTypeId,
        display_name: &'static str,
        config_schema_version: NonZeroU32,
        reload_policy: PluginReloadPolicy,
        trust: PluginTrust,
    ) -> PluginDescriptorBuilder {
        PluginDescriptorBuilder {
            type_id,
            display_name,
            config_schema_version,
            required: BTreeSet::new(),
            observed: BTreeSet::new(),
            provided: BTreeSet::new(),
            reload_policy,
            trust,
        }
    }

    fn definition(&self, component_id: impl Into<String>) -> ComponentDefinition {
        let mut definition = ComponentDefinition::new(component_id);
        for key in &self.required {
            definition = definition.requires(key.clone());
        }
        for key in &self.observed {
            definition = definition.observes(key.clone());
        }
        for key in &self.provided {
            definition = definition.provides(key.clone());
        }
        definition
    }

    fn snapshot(&self, component_id: String) -> PluginDescriptorSnapshot {
        PluginDescriptorSnapshot {
            component_id,
            plugin_type: self.type_id.as_str().to_string(),
            config_schema_version: self.config_schema_version.get(),
            reload_policy: self.reload_policy.as_str(),
            trust: self.trust.as_str(),
        }
    }
}

/// Builder 可以暂存未完成声明；只有 `build` 成功后才产生 immutable descriptor。
pub(super) struct PluginDescriptorBuilder {
    type_id: PluginTypeId,
    display_name: &'static str,
    config_schema_version: NonZeroU32,
    required: BTreeSet<CapabilityKey>,
    observed: BTreeSet<CapabilityKey>,
    provided: BTreeSet<CapabilityKey>,
    reload_policy: PluginReloadPolicy,
    trust: PluginTrust,
}

impl PluginDescriptorBuilder {
    pub(super) fn requires(mut self, key: impl Into<CapabilityKey>) -> Self {
        self.required.insert(key.into());
        self
    }

    pub(super) fn observes(mut self, key: impl Into<CapabilityKey>) -> Self {
        self.observed.insert(key.into());
        self
    }

    pub(super) fn provides(mut self, key: impl Into<CapabilityKey>) -> Self {
        self.provided.insert(key.into());
        self
    }

    pub(super) fn build(self) -> Result<PluginDescriptor, PluginDescriptorError> {
        if self.display_name.is_empty() {
            return Err(PluginDescriptorError::EmptyDisplayName);
        }
        if self.required.iter().any(|key| self.observed.contains(key)) {
            return Err(PluginDescriptorError::ConflictingDependencyRole);
        }
        Ok(PluginDescriptor {
            type_id: self.type_id,
            _display_name: self.display_name,
            config_schema_version: self.config_schema_version,
            required: self.required,
            observed: self.observed,
            provided: self.provided,
            reload_policy: self.reload_policy,
            trust: self.trust,
        })
    }
}

/// `PluginDescriptorError` 不保留 display metadata 或 capability 原值。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PluginDescriptorError {
    EmptyDisplayName,
    ConflictingDependencyRole,
}

impl fmt::Display for PluginDescriptorError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let kind = match self {
            Self::EmptyDisplayName => "empty_plugin_display_name",
            Self::ConflictingDependencyRole => "conflicting_plugin_dependency_role",
        };
        formatter.write_str(kind)
    }
}

impl Error for PluginDescriptorError {}

impl fmt::Debug for PluginDescriptor {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PluginDescriptor")
            .field("type_id", &self.type_id)
            .field("config_schema_version", &self.config_schema_version)
            .field("required_count", &self.required.len())
            .field("observed_count", &self.observed.len())
            .field("provided_count", &self.provided.len())
            .field("reload_policy", &self.reload_policy)
            .field("trust", &self.trust)
            .finish_non_exhaustive()
    }
}

/// `DesiredPluginComposition` 是经过完整校验、按 slot id 排序的期望状态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct DesiredPluginComposition {
    entries: BTreeMap<PluginComponentId, PluginTypeId>,
}

impl DesiredPluginComposition {
    pub(super) fn try_new<S>(
        entries: impl IntoIterator<Item = (S, PluginTypeId)>,
    ) -> Result<Self, PluginCatalogError>
    where
        S: Into<String>,
    {
        let mut validated = BTreeMap::new();
        for (component_id, plugin_type) in entries {
            let component_id = PluginComponentId::try_new(component_id)
                .map_err(|source| PluginCatalogError::InvalidComponentId { source })?;
            if validated
                .insert(component_id.clone(), plugin_type)
                .is_some()
            {
                return Err(PluginCatalogError::DuplicateComponent { component_id });
            }
        }
        Ok(Self { entries: validated })
    }

    pub(super) fn iter(
        &self,
    ) -> impl ExactSizeIterator<Item = (&PluginComponentId, &PluginTypeId)> {
        self.entries.iter()
    }
}

/// `ObservedPluginComposition` 只投影 live composition 的稳定 identity。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ObservedPluginComposition {
    entries: BTreeMap<PluginComponentId, PluginTypeId>,
}

impl ObservedPluginComposition {
    pub(super) fn empty() -> Self {
        Self {
            entries: BTreeMap::new(),
        }
    }

    pub(super) fn iter(
        &self,
    ) -> impl ExactSizeIterator<Item = (&PluginComponentId, &PluginTypeId)> {
        self.entries.iter()
    }
}

/// `PluginReconciliationAction` 是 loader 与 lifecycle transaction 间的封闭 diff 词汇。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum PluginReconciliationAction {
    Keep {
        component_id: PluginComponentId,
        plugin_type: PluginTypeId,
    },
    Add {
        component_id: PluginComponentId,
        plugin_type: PluginTypeId,
    },
    Remove {
        component_id: PluginComponentId,
        plugin_type: PluginTypeId,
    },
    Replace {
        component_id: PluginComponentId,
        from: PluginTypeId,
        to: PluginTypeId,
    },
}

impl PluginReconciliationAction {
    fn target_factory(&self) -> Option<(&PluginComponentId, &PluginTypeId)> {
        match self {
            Self::Add {
                component_id,
                plugin_type,
            } => Some((component_id, plugin_type)),
            Self::Replace {
                component_id, to, ..
            } => Some((component_id, to)),
            Self::Keep { .. } | Self::Remove { .. } => None,
        }
    }
}

/// `PluginReconciliationPlan` 只描述 mutation-free planning 结果，不提供执行入口。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PluginReconciliationPlan {
    actions: Vec<PluginReconciliationAction>,
}

impl PluginReconciliationPlan {
    pub(super) fn actions(&self) -> &[PluginReconciliationAction] {
        &self.actions
    }
}

/// Factory construction error 的 `Debug` 只暴露 closed kind，不投影 raw source text。
pub(super) struct PluginConstructionError {
    message: String,
}

impl From<String> for PluginConstructionError {
    fn from(message: String) -> Self {
        Self { message }
    }
}

impl fmt::Debug for PluginConstructionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PluginConstructionError")
            .field("kind", &"construction_failed")
            .finish()
    }
}

impl fmt::Display for PluginConstructionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for PluginConstructionError {}

/// `PluginFactory` 把 descriptor 与同一种 typed implementation 的 constructor 绑定。
pub(super) struct PluginFactory<I> {
    descriptor: PluginDescriptor,
    construct: Box<dyn Fn() -> Result<I, PluginConstructionError> + Send + Sync>,
}

impl<I> PluginFactory<I> {
    pub(super) fn new(
        descriptor: PluginDescriptor,
        construct: impl Fn() -> Result<I, PluginConstructionError> + Send + Sync + 'static,
    ) -> Self {
        Self {
            descriptor,
            construct: Box::new(construct),
        }
    }
}

/// Catalog validation error 的 `Debug` 不包含 factory source 或 implementation details。
pub(super) enum PluginCatalogError {
    InvalidComponentId {
        source: PluginComponentIdError,
    },
    DuplicatePluginType {
        plugin_type: PluginTypeId,
    },
    DuplicateComponent {
        component_id: PluginComponentId,
    },
    MissingFactory {
        component_id: PluginComponentId,
        plugin_type: PluginTypeId,
    },
    ConstructionFailed {
        component_id: PluginComponentId,
        plugin_type: PluginTypeId,
        source: PluginConstructionError,
    },
}

impl fmt::Debug for PluginCatalogError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidComponentId { source } => formatter
                .debug_struct("PluginCatalogError")
                .field("kind", &"invalid_component_id")
                .field("reason", source)
                .finish(),
            Self::DuplicatePluginType { plugin_type } => formatter
                .debug_struct("PluginCatalogError")
                .field("kind", &"duplicate_plugin_type")
                .field("plugin_type", plugin_type)
                .finish(),
            Self::DuplicateComponent { component_id } => formatter
                .debug_struct("PluginCatalogError")
                .field("kind", &"duplicate_component")
                .field("component_id", component_id)
                .finish(),
            Self::MissingFactory {
                component_id,
                plugin_type,
            } => formatter
                .debug_struct("PluginCatalogError")
                .field("kind", &"missing_factory")
                .field("component_id", component_id)
                .field("plugin_type", plugin_type)
                .finish(),
            Self::ConstructionFailed {
                component_id,
                plugin_type,
                ..
            } => formatter
                .debug_struct("PluginCatalogError")
                .field("kind", &"construction_failed")
                .field("component_id", component_id)
                .field("plugin_type", plugin_type)
                .finish(),
        }
    }
}

impl fmt::Display for PluginCatalogError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidComponentId { source } => {
                write!(formatter, "plugin component id is invalid: {source}")
            }
            Self::DuplicatePluginType { plugin_type } => {
                write!(
                    formatter,
                    "plugin type `{plugin_type}` is registered more than once"
                )
            }
            Self::DuplicateComponent { component_id } => {
                write!(
                    formatter,
                    "component `{component_id}` is declared more than once"
                )
            }
            Self::MissingFactory {
                component_id,
                plugin_type,
            } => write!(
                formatter,
                "component `{component_id}` references missing plugin type `{plugin_type}`"
            ),
            Self::ConstructionFailed {
                component_id,
                plugin_type,
                ..
            } => write!(
                formatter,
                "plugin `{plugin_type}` construction failed for component `{component_id}`"
            ),
        }
    }
}

impl Error for PluginCatalogError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::InvalidComponentId { source } => Some(source),
            Self::ConstructionFailed { source, .. } => Some(source),
            _ => None,
        }
    }
}

/// `PluginFactoryCatalog` 是 compile-time factory 的唯一 lookup authority。
pub(super) struct PluginFactoryCatalog<I> {
    factories: BTreeMap<PluginTypeId, PluginFactory<I>>,
}

impl<I> PluginFactoryCatalog<I> {
    pub(super) fn try_new(
        factories: impl IntoIterator<Item = PluginFactory<I>>,
    ) -> Result<Self, PluginCatalogError> {
        let mut registered = BTreeMap::new();
        for factory in factories {
            let plugin_type = factory.descriptor.type_id.clone();
            if registered.insert(plugin_type.clone(), factory).is_some() {
                return Err(PluginCatalogError::DuplicatePluginType { plugin_type });
            }
        }
        Ok(Self {
            factories: registered,
        })
    }

    pub(super) fn instantiate(
        &self,
        desired: &DesiredPluginComposition,
    ) -> Result<PluginComposition<I>, PluginCatalogError> {
        // Startup 是从空 observed state 进行 Add-only prepare；plan preflight 必须先于 constructor。
        self.plan_reconciliation(desired, &ObservedPluginComposition::empty())?;

        let mut prepared = Vec::with_capacity(desired.entries.len());
        for (component_id, plugin_type) in desired.iter() {
            let factory = self
                .factories
                .get(plugin_type)
                .expect("factories were resolved before construction");
            let implementation = match (factory.construct)() {
                Ok(implementation) => implementation,
                Err(source) => {
                    while let Some(instance) = prepared.pop() {
                        drop(instance);
                    }
                    return Err(PluginCatalogError::ConstructionFailed {
                        component_id: component_id.clone(),
                        plugin_type: plugin_type.clone(),
                        source,
                    });
                }
            };
            prepared.push(PluginInstance {
                component_id: component_id.clone(),
                descriptor: factory.descriptor.clone(),
                implementation,
            });
        }

        Ok(PluginComposition {
            instances: prepared
                .into_iter()
                .map(|instance| (instance.component_id.clone(), instance))
                .collect(),
        })
    }

    pub(super) fn plan_reconciliation(
        &self,
        desired: &DesiredPluginComposition,
        observed: &ObservedPluginComposition,
    ) -> Result<PluginReconciliationPlan, PluginCatalogError> {
        let component_ids = desired
            .iter()
            .map(|(component_id, _)| component_id)
            .chain(observed.iter().map(|(component_id, _)| component_id))
            .cloned()
            .collect::<BTreeSet<_>>();
        let actions = component_ids
            .into_iter()
            .map(|component_id| {
                match (
                    desired.entries.get(&component_id),
                    observed.entries.get(&component_id),
                ) {
                    (Some(desired_type), Some(observed_type)) if desired_type == observed_type => {
                        PluginReconciliationAction::Keep {
                            component_id,
                            plugin_type: desired_type.clone(),
                        }
                    }
                    (Some(desired_type), Some(observed_type)) => {
                        PluginReconciliationAction::Replace {
                            component_id,
                            from: observed_type.clone(),
                            to: desired_type.clone(),
                        }
                    }
                    (Some(plugin_type), None) => PluginReconciliationAction::Add {
                        component_id,
                        plugin_type: plugin_type.clone(),
                    },
                    (None, Some(plugin_type)) => PluginReconciliationAction::Remove {
                        component_id,
                        plugin_type: plugin_type.clone(),
                    },
                    (None, None) => unreachable!("component id came from desired/observed union"),
                }
            })
            .collect::<Vec<_>>();
        let plan = PluginReconciliationPlan { actions };

        for action in plan.actions() {
            let Some((component_id, plugin_type)) = action.target_factory() else {
                continue;
            };
            if !self.factories.contains_key(plugin_type) {
                return Err(PluginCatalogError::MissingFactory {
                    component_id: component_id.clone(),
                    plugin_type: plugin_type.clone(),
                });
            }
        }

        Ok(plan)
    }
}

struct PluginInstance<I> {
    component_id: PluginComponentId,
    descriptor: PluginDescriptor,
    implementation: I,
}

/// `PluginComposition` 是已完整 prepare、尚未 publication 的 immutable instance 集合。
pub(super) struct PluginComposition<I> {
    instances: BTreeMap<PluginComponentId, PluginInstance<I>>,
}

impl<I> PluginComposition<I> {
    pub(super) fn definitions(&self) -> Vec<ComponentDefinition> {
        self.instances
            .values()
            .map(|instance| {
                instance
                    .descriptor
                    .definition(instance.component_id.as_str().to_string())
            })
            .collect()
    }

    pub(super) fn implementation(&self, component_id: &str) -> Option<&I> {
        self.instances
            .get(component_id)
            .map(|instance| &instance.implementation)
    }

    pub(super) fn descriptor_snapshots(&self) -> Vec<PluginDescriptorSnapshot> {
        let observed = self.observed();
        observed
            .iter()
            .map(|(component_id, plugin_type)| {
                let instance = self
                    .instances
                    .get(component_id)
                    .expect("observed identity was projected from prepared instances");
                debug_assert_eq!(plugin_type, &instance.descriptor.type_id);
                instance
                    .descriptor
                    .snapshot(instance.component_id.as_str().to_string())
            })
            .collect()
    }

    pub(super) fn observed(&self) -> ObservedPluginComposition {
        let mut observed = ObservedPluginComposition::empty();
        observed
            .entries
            .extend(self.instances.iter().map(|(component_id, instance)| {
                (component_id.clone(), instance.descriptor.type_id.clone())
            }));
        observed
    }
}

/// Runtime inspection 可见的 descriptor projection 只包含 closed metadata。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PluginDescriptorSnapshot {
    pub(super) component_id: String,
    pub(super) plugin_type: String,
    pub(super) config_schema_version: u32,
    pub(super) reload_policy: &'static str,
    pub(super) trust: &'static str,
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;

    fn plugin_type(type_id: &'static str) -> PluginTypeId {
        PluginTypeId::try_new(type_id).expect("test plugin type id should be valid")
    }

    fn descriptor_builder(type_id: &'static str) -> PluginDescriptorBuilder {
        PluginDescriptor::builder(
            plugin_type(type_id),
            "Test plugin",
            NonZeroU32::MIN,
            PluginReloadPolicy::Replace,
            PluginTrust::Builtin,
        )
    }

    fn descriptor(type_id: &'static str) -> PluginDescriptor {
        descriptor_builder(type_id)
            .build()
            .expect("test descriptor should be valid")
    }

    fn factory(type_id: &'static str, value: usize) -> PluginFactory<usize> {
        PluginFactory::new(descriptor(type_id), move || Ok(value))
    }

    fn desired<const N: usize>(
        entries: [(&'static str, &'static str); N],
    ) -> DesiredPluginComposition {
        DesiredPluginComposition::try_new(
            entries
                .into_iter()
                .map(|(component_id, type_id)| (component_id, plugin_type(type_id))),
        )
        .expect("test desired composition should be valid")
    }

    fn observed<const N: usize>(
        entries: [(&'static str, &'static str); N],
    ) -> ObservedPluginComposition {
        let desired = desired(entries);
        let factories = entries
            .iter()
            .map(|(_, type_id)| *type_id)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .map(|type_id| factory(type_id, 0));
        PluginFactoryCatalog::try_new(factories)
            .expect("observed catalog should validate")
            .instantiate(&desired)
            .expect("observed composition should prepare")
            .observed()
    }

    #[test]
    fn descriptor_is_the_only_component_definition_source() {
        let catalog = PluginFactoryCatalog::try_new([PluginFactory::new(
            descriptor_builder("agent")
                .requires("llm")
                .observes("metrics")
                .provides("agent")
                .build()
                .expect("descriptor should be valid"),
            || Ok(()),
        )])
        .expect("catalog should validate");
        let desired = desired([("main_agent", "agent")]);
        let composition = catalog
            .instantiate(&desired)
            .expect("composition should prepare");

        assert_eq!(
            composition.definitions(),
            vec![
                ComponentDefinition::new("main_agent")
                    .requires("llm")
                    .observes("metrics")
                    .provides("agent")
            ]
        );
    }

    #[test]
    fn duplicate_plugin_type_is_rejected_without_returning_a_catalog() {
        let error = PluginFactoryCatalog::try_new([factory("agent", 1), factory("agent", 2)])
            .err()
            .expect("duplicate plugin type should fail");

        assert!(matches!(
            error,
            PluginCatalogError::DuplicatePluginType {
                plugin_type: actual_plugin_type,
            } if actual_plugin_type == plugin_type("agent")
        ));
    }

    #[test]
    fn plugin_type_id_rejects_invalid_stable_ids_without_retaining_the_source() {
        assert_eq!(
            PluginTypeId::try_new("").expect_err("empty id should fail"),
            PluginTypeIdError::Empty
        );
        let invalid_source = "SENSITIVE/PLUGIN_TYPE";
        let error = PluginTypeId::try_new(invalid_source).expect_err("invalid id should fail");

        assert_eq!(error, PluginTypeIdError::InvalidFormat);
        assert!(!error.to_string().contains(invalid_source));
        assert!(!format!("{error:?}").contains(invalid_source));
        assert!(PluginTypeId::try_new("valid-plugin-2").is_ok());
        assert!(PluginTypeId::try_new("invalid--plugin").is_err());
        assert!(PluginTypeId::try_new("invalid-").is_err());
    }

    #[test]
    fn component_id_rejects_invalid_slot_ids_without_retaining_the_source() {
        assert_eq!(
            PluginComponentId::try_new("").expect_err("empty id should fail"),
            PluginComponentIdError::Empty
        );
        for invalid_source in [
            "SENSITIVE/COMPONENT/PATH",
            "UPPERCASE",
            "with-hyphen",
            " leading_space",
            "trailing_space ",
            "repeated__separator",
            "_leading_separator",
            "trailing_separator_",
            "line\nbreak",
        ] {
            let error = PluginComponentId::try_new(invalid_source)
                .expect_err("invalid component id should fail");
            assert_eq!(error, PluginComponentIdError::InvalidFormat);
            assert!(!error.to_string().contains(invalid_source));
            assert!(!format!("{error:?}").contains(invalid_source));

            let catalog_error =
                DesiredPluginComposition::try_new([(invalid_source, plugin_type("agent"))])
                    .expect_err("invalid desired slot should fail");
            assert!(!catalog_error.to_string().contains(invalid_source));
            assert!(!format!("{catalog_error:?}").contains(invalid_source));
        }

        for valid in ["agent", "agent_2", "2_agent", "runtime_event_stream"] {
            assert_eq!(
                PluginComponentId::try_new(valid)
                    .expect("valid component id should pass")
                    .as_str(),
                valid
            );
        }
    }

    #[test]
    fn desired_and_observed_compositions_are_immutable_sorted_identity_projections() {
        let desired = desired([("z_component", "z-plugin"), ("a_component", "a-plugin")]);
        assert_eq!(
            desired
                .iter()
                .map(|(component_id, plugin_type)| {
                    (component_id.as_str(), plugin_type.as_str())
                })
                .collect::<Vec<_>>(),
            vec![("a_component", "a-plugin"), ("z_component", "z-plugin")]
        );

        let catalog =
            PluginFactoryCatalog::try_new([factory("z-plugin", 2), factory("a-plugin", 1)])
                .expect("catalog should validate");
        let composition = catalog
            .instantiate(&desired)
            .expect("composition should prepare");
        let observed = composition.observed();
        assert_eq!(
            observed
                .iter()
                .map(|(component_id, plugin_type)| {
                    (component_id.as_str(), plugin_type.as_str())
                })
                .collect::<Vec<_>>(),
            vec![("a_component", "a-plugin"), ("z_component", "z-plugin")]
        );
    }

    #[test]
    fn descriptor_requires_non_zero_schema_version_and_disjoint_dependency_roles() {
        assert!(NonZeroU32::new(0).is_none());

        let error = descriptor_builder("agent")
            .requires("shared")
            .observes("shared")
            .build()
            .expect_err("conflicting dependency roles should fail before catalog construction");

        assert_eq!(error, PluginDescriptorError::ConflictingDependencyRole);
    }

    #[test]
    fn descriptor_rejects_empty_display_metadata_without_retaining_it() {
        let error = PluginDescriptor::builder(
            plugin_type("agent"),
            "",
            NonZeroU32::MIN,
            PluginReloadPolicy::Replace,
            PluginTrust::Builtin,
        )
        .build()
        .expect_err("empty display name should fail before catalog construction");

        assert_eq!(error, PluginDescriptorError::EmptyDisplayName);
    }

    #[test]
    fn desired_composition_validation_finishes_before_construction() {
        let constructions = Arc::new(Mutex::new(Vec::new()));
        let observed = Arc::clone(&constructions);
        let catalog =
            PluginFactoryCatalog::try_new([PluginFactory::new(descriptor("agent"), move || {
                observed
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push("constructed");
                Ok(())
            })])
            .expect("catalog should validate");

        let duplicate = DesiredPluginComposition::try_new([
            ("main", plugin_type("agent")),
            ("main", plugin_type("agent")),
        ])
        .expect_err("duplicate component should fail");
        assert!(matches!(
            duplicate,
            PluginCatalogError::DuplicateComponent { component_id }
                if component_id.as_str() == "main"
        ));
        assert!(
            constructions
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .is_empty()
        );

        let missing_desired = desired([("missing", "missing")]);
        let missing = catalog
            .instantiate(&missing_desired)
            .err()
            .expect("missing factory should fail");
        assert!(matches!(
            missing,
            PluginCatalogError::MissingFactory {
                component_id,
                plugin_type: actual_plugin_type,
            } if component_id.as_str() == "missing"
                && actual_plugin_type == plugin_type("missing")
        ));
        assert!(
            constructions
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .is_empty()
        );
    }

    #[test]
    fn reconciliation_plan_is_empty_for_empty_compositions() {
        let catalog =
            PluginFactoryCatalog::<usize>::try_new([]).expect("empty catalog should validate");
        let desired = desired([]);
        let observed = ObservedPluginComposition::empty();

        let plan = catalog
            .plan_reconciliation(&desired, &observed)
            .expect("empty composition should plan");

        assert!(plan.actions().is_empty());
    }

    #[test]
    fn reconciliation_plan_classifies_each_slot_once_in_stable_order() {
        let catalog =
            PluginFactoryCatalog::try_new([factory("added", 1), factory("replacement", 2)])
                .expect("target catalog should validate");
        let desired = desired([
            ("replace_slot", "replacement"),
            ("keep_slot", "stable"),
            ("add_slot", "added"),
        ]);
        let observed = observed([
            ("remove_slot", "retired"),
            ("keep_slot", "stable"),
            ("replace_slot", "previous"),
        ]);

        let plan = catalog
            .plan_reconciliation(&desired, &observed)
            .expect("mixed composition should plan");

        assert_eq!(
            plan.actions(),
            &[
                PluginReconciliationAction::Add {
                    component_id: PluginComponentId::try_new("add_slot")
                        .expect("component id should validate"),
                    plugin_type: plugin_type("added"),
                },
                PluginReconciliationAction::Keep {
                    component_id: PluginComponentId::try_new("keep_slot")
                        .expect("component id should validate"),
                    plugin_type: plugin_type("stable"),
                },
                PluginReconciliationAction::Remove {
                    component_id: PluginComponentId::try_new("remove_slot")
                        .expect("component id should validate"),
                    plugin_type: plugin_type("retired"),
                },
                PluginReconciliationAction::Replace {
                    component_id: PluginComponentId::try_new("replace_slot")
                        .expect("component id should validate"),
                    from: plugin_type("previous"),
                    to: plugin_type("replacement"),
                },
            ]
        );
    }

    #[test]
    fn reconciliation_is_idempotent_and_independent_of_input_order() {
        let catalog = PluginFactoryCatalog::try_new([factory("fresh-a", 1), factory("fresh-z", 2)])
            .expect("target catalog should validate");
        let desired_forward = desired([
            ("z_slot", "fresh-z"),
            ("a_slot", "fresh-a"),
            ("keep_slot", "stable"),
        ]);
        let desired_reverse = desired([
            ("keep_slot", "stable"),
            ("a_slot", "fresh-a"),
            ("z_slot", "fresh-z"),
        ]);
        let observed_forward = observed([
            ("z_slot", "old-z"),
            ("keep_slot", "stable"),
            ("removed_slot", "retired"),
        ]);
        let observed_reverse = observed([
            ("removed_slot", "retired"),
            ("keep_slot", "stable"),
            ("z_slot", "old-z"),
        ]);

        let first = catalog
            .plan_reconciliation(&desired_forward, &observed_forward)
            .expect("first plan should succeed");
        let repeated = catalog
            .plan_reconciliation(&desired_forward, &observed_forward)
            .expect("repeated plan should succeed");
        let reordered = catalog
            .plan_reconciliation(&desired_reverse, &observed_reverse)
            .expect("reordered plan should succeed");

        assert_eq!(first, repeated);
        assert_eq!(first, reordered);
        assert_eq!(format!("{first:?}"), format!("{repeated:?}"));
    }

    #[test]
    fn reconciliation_preflights_add_and_replace_without_construction_or_mutation() {
        let constructions = Arc::new(Mutex::new(0_u32));
        let observed_constructions = Arc::clone(&constructions);
        let catalog =
            PluginFactoryCatalog::try_new([PluginFactory::new(descriptor("known"), move || {
                *observed_constructions
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) += 1;
                Ok(())
            })])
            .expect("catalog should validate");

        let empty_observed = ObservedPluginComposition::empty();
        let add_desired = desired([("a_known", "known"), ("z_missing", "missing-add-target")]);
        let add_error = catalog
            .plan_reconciliation(&add_desired, &empty_observed)
            .expect_err("missing Add factory should fail preflight");
        assert!(matches!(
            add_error,
            PluginCatalogError::MissingFactory {
                component_id,
                plugin_type: missing_type,
            } if component_id.as_str() == "z_missing"
                && missing_type == plugin_type("missing-add-target")
        ));
        assert_eq!(
            *constructions
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            0
        );

        let observed = observed([("replace_slot", "previous")]);
        let original_observed = observed.clone();
        let replace_desired = desired([("replace_slot", "missing-replacement-target")]);
        let replace_error = catalog
            .plan_reconciliation(&replace_desired, &observed)
            .expect_err("missing Replace factory should fail preflight");
        assert!(matches!(
            replace_error,
            PluginCatalogError::MissingFactory {
                component_id,
                plugin_type: missing_type,
            } if component_id.as_str() == "replace_slot"
                && missing_type == plugin_type("missing-replacement-target")
        ));
        assert_eq!(observed, original_observed);
        assert_eq!(
            *constructions
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            0
        );
    }

    #[test]
    fn observed_and_plan_debug_omit_descriptor_and_implementation_internals() {
        struct SensitiveImplementation;

        impl fmt::Debug for SensitiveImplementation {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("SENSITIVE_IMPLEMENTATION_SENTINEL")
            }
        }

        let catalog = PluginFactoryCatalog::try_new([PluginFactory::new(
            PluginDescriptor::builder(
                plugin_type("safe-plugin"),
                "SENSITIVE_DISPLAY_SENTINEL",
                NonZeroU32::MIN,
                PluginReloadPolicy::Replace,
                PluginTrust::Builtin,
            )
            .requires("SENSITIVE_CAPABILITY_SENTINEL")
            .build()
            .expect("descriptor should validate"),
            || Ok(SensitiveImplementation),
        )])
        .expect("catalog should validate");
        let desired = desired([("safe_slot", "safe-plugin")]);
        let composition = catalog
            .instantiate(&desired)
            .expect("composition should prepare");
        let observed = composition.observed();
        let planning_catalog = PluginFactoryCatalog::<SensitiveImplementation>::try_new([])
            .expect("empty planning catalog should validate");
        let plan = planning_catalog
            .plan_reconciliation(&desired, &observed)
            .expect("Keep does not need a factory");

        let observed_debug = format!("{observed:?}");
        let plan_debug = format!("{plan:?}");
        for sentinel in [
            "SENSITIVE_DISPLAY_SENTINEL",
            "SENSITIVE_CAPABILITY_SENTINEL",
            "SENSITIVE_IMPLEMENTATION_SENTINEL",
        ] {
            assert!(!observed_debug.contains(sentinel));
            assert!(!plan_debug.contains(sentinel));
        }
        assert!(observed_debug.contains("safe_slot"));
        assert!(observed_debug.contains("safe-plugin"));
    }

    #[derive(Clone)]
    struct DropProbe {
        name: &'static str,
        drops: Arc<Mutex<Vec<&'static str>>>,
    }

    impl Drop for DropProbe {
        fn drop(&mut self) {
            self.drops
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(self.name);
        }
    }

    #[test]
    fn construction_failure_drops_prepared_instances_in_reverse_order() {
        let drops = Arc::new(Mutex::new(Vec::new()));
        let first_drops = Arc::clone(&drops);
        let second_drops = Arc::clone(&drops);
        let catalog = PluginFactoryCatalog::try_new([
            PluginFactory::new(descriptor("first"), move || {
                Ok(DropProbe {
                    name: "first",
                    drops: Arc::clone(&first_drops),
                })
            }),
            PluginFactory::new(descriptor("second"), move || {
                Ok(DropProbe {
                    name: "second",
                    drops: Arc::clone(&second_drops),
                })
            }),
            PluginFactory::new(descriptor("third"), || {
                Err("SENSITIVE_FACTORY_SOURCE_SENTINEL".to_string().into())
            }),
        ])
        .expect("catalog should validate");

        let desired = desired([("a", "first"), ("b", "second"), ("c", "third")]);
        let error = catalog
            .instantiate(&desired)
            .err()
            .expect("construction should fail");

        assert_eq!(
            *drops
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            vec!["second", "first"]
        );
        assert!(
            !error
                .to_string()
                .contains("SENSITIVE_FACTORY_SOURCE_SENTINEL")
        );
        assert!(!format!("{error:?}").contains("SENSITIVE_FACTORY_SOURCE_SENTINEL"));
        assert!(
            error
                .source()
                .expect("construction error should preserve its source")
                .to_string()
                .contains("SENSITIVE_FACTORY_SOURCE_SENTINEL")
        );
    }

    #[test]
    fn composition_order_is_independent_of_factory_and_desired_order() {
        let catalog = PluginFactoryCatalog::try_new([factory("z", 2), factory("a", 1)])
            .expect("catalog should validate");
        let desired = desired([("z_component", "z"), ("a_component", "a")]);
        let composition = catalog
            .instantiate(&desired)
            .expect("composition should prepare");

        let definitions = composition.definitions();
        assert_eq!(
            definitions
                .iter()
                .map(|definition| definition.id.as_str())
                .collect::<Vec<_>>(),
            vec!["a_component", "z_component"]
        );
        assert_eq!(composition.implementation("a_component"), Some(&1));
        assert_eq!(composition.implementation("z_component"), Some(&2));
    }

    #[test]
    fn descriptor_debug_and_snapshot_omit_delivery_and_factory_internals() {
        let descriptor = PluginDescriptor::builder(
            plugin_type("agent"),
            "SENSITIVE_DISPLAY_INSTRUCTION_SENTINEL",
            NonZeroU32::MIN,
            PluginReloadPolicy::Replace,
            PluginTrust::Builtin,
        )
        .requires("SENSITIVE_REQUIRED_INSTRUCTION_SENTINEL")
        .observes("SENSITIVE/OBSERVED/PATH/SENTINEL")
        .provides("SENSITIVE_PROVIDED_RESOURCE_SENTINEL")
        .build()
        .expect("descriptor should be valid");
        let debug = format!("{descriptor:?}");
        let snapshot = descriptor.snapshot("main".to_string());

        for sentinel in [
            "SENSITIVE_DISPLAY_INSTRUCTION_SENTINEL",
            "SENSITIVE_REQUIRED_INSTRUCTION_SENTINEL",
            "SENSITIVE/OBSERVED/PATH/SENTINEL",
            "SENSITIVE_PROVIDED_RESOURCE_SENTINEL",
        ] {
            assert!(!debug.contains(sentinel));
            assert!(!format!("{snapshot:?}").contains(sentinel));
        }
        assert!(debug.contains("required_count: 1"));
        assert!(debug.contains("observed_count: 1"));
        assert!(debug.contains("provided_count: 1"));
        assert_eq!(snapshot.plugin_type, "agent");
        assert_eq!(snapshot.reload_policy, "replace");
        assert_eq!(snapshot.trust, "builtin");
    }
}
