use std::{
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

/// `DesiredPlugin` 将稳定 composition slot 绑定到一个 registered plugin type。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct DesiredPlugin {
    component_id: String,
    plugin_type: PluginTypeId,
}

impl DesiredPlugin {
    pub(super) fn new(component_id: impl Into<String>, plugin_type: PluginTypeId) -> Self {
        Self {
            component_id: component_id.into(),
            plugin_type,
        }
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
    DuplicatePluginType {
        plugin_type: PluginTypeId,
    },
    DuplicateComponent {
        component_id: String,
    },
    MissingFactory {
        component_id: String,
        plugin_type: PluginTypeId,
    },
    ConstructionFailed {
        component_id: String,
        plugin_type: PluginTypeId,
        source: PluginConstructionError,
    },
}

impl fmt::Debug for PluginCatalogError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
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
        desired: impl IntoIterator<Item = DesiredPlugin>,
    ) -> Result<PluginComposition<I>, PluginCatalogError> {
        let mut entries = BTreeMap::new();
        for entry in desired {
            let component_id = entry.component_id.clone();
            if entries.insert(component_id.clone(), entry).is_some() {
                return Err(PluginCatalogError::DuplicateComponent { component_id });
            }
        }

        for entry in entries.values() {
            if !self.factories.contains_key(&entry.plugin_type) {
                return Err(PluginCatalogError::MissingFactory {
                    component_id: entry.component_id.clone(),
                    plugin_type: entry.plugin_type.clone(),
                });
            }
        }

        let mut prepared = Vec::with_capacity(entries.len());
        for entry in entries.into_values() {
            let factory = self
                .factories
                .get(&entry.plugin_type)
                .expect("factories were resolved before construction");
            let implementation = match (factory.construct)() {
                Ok(implementation) => implementation,
                Err(source) => {
                    while let Some(instance) = prepared.pop() {
                        drop(instance);
                    }
                    return Err(PluginCatalogError::ConstructionFailed {
                        component_id: entry.component_id,
                        plugin_type: entry.plugin_type,
                        source,
                    });
                }
            };
            prepared.push(PluginInstance {
                component_id: entry.component_id,
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
}

struct PluginInstance<I> {
    component_id: String,
    descriptor: PluginDescriptor,
    implementation: I,
}

/// `PluginComposition` 是已完整 prepare、尚未 publication 的 immutable instance 集合。
pub(super) struct PluginComposition<I> {
    instances: BTreeMap<String, PluginInstance<I>>,
}

impl<I> PluginComposition<I> {
    pub(super) fn definitions(&self) -> Vec<ComponentDefinition> {
        self.instances
            .values()
            .map(|instance| {
                instance
                    .descriptor
                    .definition(instance.component_id.clone())
            })
            .collect()
    }

    pub(super) fn implementation(&self, component_id: &str) -> Option<&I> {
        self.instances
            .get(component_id)
            .map(|instance| &instance.implementation)
    }

    pub(super) fn descriptor_snapshots(&self) -> Vec<PluginDescriptorSnapshot> {
        self.instances
            .values()
            .map(|instance| instance.descriptor.snapshot(instance.component_id.clone()))
            .collect()
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
        let composition = catalog
            .instantiate([DesiredPlugin::new("main-agent", plugin_type("agent"))])
            .expect("composition should prepare");

        assert_eq!(
            composition.definitions(),
            vec![
                ComponentDefinition::new("main-agent")
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

        let duplicate = catalog
            .instantiate([
                DesiredPlugin::new("main", plugin_type("agent")),
                DesiredPlugin::new("main", plugin_type("agent")),
            ])
            .err()
            .expect("duplicate component should fail");
        assert!(matches!(
            duplicate,
            PluginCatalogError::DuplicateComponent { component_id } if component_id == "main"
        ));
        assert!(
            constructions
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .is_empty()
        );

        let missing = catalog
            .instantiate([DesiredPlugin::new("missing", plugin_type("missing"))])
            .err()
            .expect("missing factory should fail");
        assert!(matches!(
            missing,
            PluginCatalogError::MissingFactory {
                component_id,
                plugin_type: actual_plugin_type,
            } if component_id == "missing" && actual_plugin_type == plugin_type("missing")
        ));
        assert!(
            constructions
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .is_empty()
        );
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

        let error = catalog
            .instantiate([
                DesiredPlugin::new("a", plugin_type("first")),
                DesiredPlugin::new("b", plugin_type("second")),
                DesiredPlugin::new("c", plugin_type("third")),
            ])
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
        let composition = catalog
            .instantiate([
                DesiredPlugin::new("z-component", plugin_type("z")),
                DesiredPlugin::new("a-component", plugin_type("a")),
            ])
            .expect("composition should prepare");

        let definitions = composition.definitions();
        assert_eq!(
            definitions
                .iter()
                .map(|definition| definition.id.as_str())
                .collect::<Vec<_>>(),
            vec!["a-component", "z-component"]
        );
        assert_eq!(composition.implementation("a-component"), Some(&1));
        assert_eq!(composition.implementation("z-component"), Some(&2));
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
