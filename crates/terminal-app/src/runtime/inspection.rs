use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;
use tool_runtime::{ToolDefinition, ToolKind, ToolPermissionPolicy};

use super::{
    AppRuntimeCoordinator,
    context::{
        LlmPortCapability, PermissionPolicyCapability, PromptAssemblyCapability,
        ToolCatalogCapability,
    },
    effect_scope::EffectScopeSnapshot,
    lifecycle::{
        CapabilityKey, ComponentFailureSnapshot, ComponentSnapshot, OptionalCapabilitySnapshot,
        PendingComponentSnapshot,
    },
    prompt_assembly::PromptContributionSnapshot,
};

const COMPOSITION_SNAPSHOT_VERSION: u32 = 8;

/// `RuntimeCompositionSnapshot` 是默认 runtime composition 的只读诊断投影。
///
/// 该类型不是配置或持久化格式。字段只记录可组合性事实，禁止包含 API key、base URL、
/// prompt/instruction body、用户内容或 tool output。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct RuntimeCompositionSnapshot {
    schema_version: u32,
    capabilities: Vec<CapabilitySnapshot>,
    components: Vec<RuntimeComponentSnapshot>,
    activation_order: Vec<String>,
    deactivation_order: Vec<String>,
    pending: Vec<PendingSnapshot>,
    failures: Vec<FailureSnapshot>,
    effect_scopes: Vec<EffectScopeOwnershipSnapshot>,
    approval_providers: Vec<ApprovalProviderSnapshot>,
    providers: Vec<ProviderSnapshot>,
    selected_model: Option<ModelSelectionSnapshot>,
    workspace_tools: Vec<ToolSnapshot>,
    session_tools: Vec<String>,
    prompt_tools: Vec<PromptToolSnapshot>,
    prompt_sources: Vec<PromptSourceSnapshot>,
    session_persistence: SessionPersistenceSnapshot,
}

impl RuntimeCompositionSnapshot {
    pub(super) fn validate(&self) -> Result<(), String> {
        ensure_sorted_unique(
            self.capabilities.iter().map(|capability| &capability.key),
            "capability",
        )?;
        ensure_sorted_unique(
            self.components.iter().map(|component| &component.id),
            "component",
        )?;
        ensure_sorted_unique(
            self.pending.iter().map(|pending| &pending.component_id),
            "pending component",
        )?;
        ensure_sorted_unique(
            self.failures.iter().map(|failure| &failure.component_id),
            "failed component",
        )?;
        validate_effect_scopes(&self.effect_scopes)?;
        ensure_sorted_unique(
            self.approval_providers
                .iter()
                .map(|provider| &provider.provider_id),
            "approval provider",
        )?;
        ensure_sorted_unique(
            self.providers.iter().map(|provider| &provider.id),
            "provider",
        )?;
        ensure_sorted_unique(
            self.workspace_tools.iter().map(|tool| &tool.name),
            "workspace tool",
        )?;
        ensure_sorted_unique(self.session_tools.iter(), "session tool")?;
        ensure_sorted_unique(
            self.prompt_tools.iter().map(|tool| &tool.name),
            "prompt tool",
        )?;
        ensure_prompt_sources_valid(&self.prompt_sources)?;

        let capabilities = self
            .capabilities
            .iter()
            .map(|capability| capability.key.as_str())
            .collect::<BTreeSet<_>>();
        let components = self
            .components
            .iter()
            .map(|component| (component.id.as_str(), component))
            .collect::<BTreeMap<_, _>>();
        validate_topology_orders(
            &self.activation_order,
            &self.deactivation_order,
            &components,
        )?;
        for capability in &self.capabilities {
            let provider = components
                .get(capability.provider_component.as_str())
                .ok_or_else(|| {
                    format!(
                        "capability {} references undeclared provider component {}",
                        capability.key, capability.provider_component
                    )
                })?;
            if !provider.provides.contains(&capability.key) {
                return Err(format!(
                    "component {} does not declare provided capability {}",
                    capability.provider_component, capability.key
                ));
            }
        }
        for component in &self.components {
            ensure_sorted_unique(component.required.iter(), "required dependency")?;
            ensure_sorted_unique(component.provides.iter(), "provided capability")?;
            let requirements_ready = component
                .required
                .iter()
                .all(|required| capabilities.contains(required.as_str()));
            match component.state.as_str() {
                "active" if !requirements_ready => {
                    return Err(format!(
                        "active component {} has missing required capabilities",
                        component.id
                    ));
                }
                "pending" if requirements_ready => {
                    return Err(format!(
                        "pending component {} has all required capabilities",
                        component.id
                    ));
                }
                "failed"
                    if !self
                        .failures
                        .iter()
                        .any(|failure| failure.component_id == component.id) =>
                {
                    return Err(format!(
                        "failed component {} has no failure diagnostic",
                        component.id
                    ));
                }
                _ => {}
            }
        }
        for pending in &self.pending {
            let component = components
                .get(pending.component_id.as_str())
                .ok_or_else(|| {
                    format!("pending component {} is not declared", pending.component_id)
                })?;
            if component.state != "pending" {
                return Err(format!(
                    "pending diagnostic for {} does not match component state",
                    pending.component_id
                ));
            }
            ensure_sorted_unique(pending.missing_dependencies.iter(), "missing dependency")?;
            let expected = component
                .required
                .iter()
                .filter(|required| !capabilities.contains(required.as_str()))
                .cloned()
                .collect::<Vec<_>>();
            if pending.missing_dependencies != expected {
                return Err(format!(
                    "pending component {} has inconsistent missing dependencies",
                    pending.component_id
                ));
            }
        }
        let pending_ids = self
            .pending
            .iter()
            .map(|pending| pending.component_id.as_str())
            .collect::<BTreeSet<_>>();
        if let Some(component) = self.components.iter().find(|component| {
            component.state == "pending" && !pending_ids.contains(component.id.as_str())
        }) {
            return Err(format!(
                "pending component {} has no dependency diagnostic",
                component.id
            ));
        }
        for failure in &self.failures {
            let component = components
                .get(failure.component_id.as_str())
                .ok_or_else(|| {
                    format!("failed component {} is not declared", failure.component_id)
                })?;
            if component.state != "failed" || component.epoch != failure.epoch {
                return Err(format!(
                    "failure diagnostic for {} does not match component state",
                    failure.component_id
                ));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct CapabilitySnapshot {
    key: String,
    provider_component: String,
    generation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct RuntimeComponentSnapshot {
    id: String,
    state: String,
    epoch: u64,
    required: Vec<String>,
    optional: Vec<OptionalDependencySnapshot>,
    provides: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct PendingSnapshot {
    component_id: String,
    missing_dependencies: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct FailureSnapshot {
    component_id: String,
    operation: String,
    code: String,
    message: String,
    recoverable: bool,
    epoch: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct EffectScopeOwnershipSnapshot {
    owner: String,
    effects: Vec<String>,
    children: Vec<EffectScopeOwnershipSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct OptionalDependencySnapshot {
    key: String,
    available: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct ApprovalProviderSnapshot {
    provider_id: String,
    adapter_kind: String,
    mounted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct ProviderSnapshot {
    id: String,
    kind: String,
    adapter_kind: String,
    mounted: bool,
    model_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct ModelSelectionSnapshot {
    provider_id: String,
    model_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct ToolSnapshot {
    name: String,
    kind: String,
    permission_policy: String,
    has_prompt_guidelines: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct PromptToolSnapshot {
    name: String,
    tool_enabled: bool,
    session_enabled: bool,
    guidelines_available: bool,
    guidelines_selected: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct PromptSourceSnapshot {
    effective_order: usize,
    stable_id: String,
    kind: String,
    origin: Option<String>,
    scope: Option<String>,
    priority: i32,
    is_trusted: bool,
    estimated_tokens: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct SessionPersistenceSnapshot {
    available: bool,
    mounted: bool,
    backend_id: Option<String>,
    adapter_kind: Option<String>,
}

impl AppRuntimeCoordinator {
    pub(super) fn inspect_composition(&self) -> RuntimeCompositionSnapshot {
        let capabilities = self
            .components
            .lifecycle
            .capabilities()
            .into_iter()
            .map(|capability| CapabilitySnapshot {
                key: capability.key,
                provider_component: capability.provider_component,
                generation: capability.generation,
            })
            .collect();
        let components = self
            .components
            .lifecycle
            .components()
            .into_iter()
            .map(runtime_component_snapshot)
            .collect();
        let pending = self
            .components
            .lifecycle
            .pending()
            .into_iter()
            .map(pending_snapshot)
            .collect();
        let failures = self
            .components
            .lifecycle
            .failures()
            .into_iter()
            .map(failure_snapshot)
            .collect();

        let mut providers = self
            .components
            .optional::<LlmPortCapability>()
            .expect("LlmPort capability marker must match its registered value")
            .into_iter()
            .flat_map(|llm_port| llm_port.inspection_snapshot())
            .map(|registration| {
                let mut model_ids = self
                    .options
                    .loaded_models
                    .catalog
                    .enabled_provider_by_id(&registration.provider_id)
                    .into_iter()
                    .flat_map(|provider| provider.models.iter())
                    .map(|model| model.id.clone())
                    .collect::<Vec<_>>();
                model_ids.sort();
                ProviderSnapshot {
                    id: registration.provider_id,
                    kind: registration.provider_kind.as_config_value().to_string(),
                    adapter_kind: registration.adapter_kind,
                    mounted: true,
                    model_ids,
                }
            })
            .collect::<Vec<_>>();
        providers.sort_by(|left, right| left.id.cmp(&right.id));

        let approval_providers = self
            .components
            .optional::<PermissionPolicyCapability>()
            .expect("PermissionPolicy capability marker must match its registered value")
            .into_iter()
            .flat_map(|policy| policy.inspection_snapshot())
            .map(|provider| ApprovalProviderSnapshot {
                provider_id: provider.provider_id,
                adapter_kind: provider.adapter_kind,
                mounted: true,
            })
            .collect();

        let selected_model = self
            .options
            .loaded_models
            .selected_model
            .as_ref()
            .map(|selection| ModelSelectionSnapshot {
                provider_id: selection.provider_id.clone(),
                model_id: selection.model_id.clone(),
            });

        let tool_definitions = self
            .components
            .optional::<ToolCatalogCapability>()
            .expect("ToolCatalog capability marker must match its registered value")
            .map(|catalog| catalog.definitions())
            .unwrap_or_default();
        let mut workspace_tools = tool_definitions
            .iter()
            .map(tool_snapshot)
            .collect::<Vec<_>>();
        workspace_tools.sort_by(|left, right| left.name.cmp(&right.name));

        let mut session_tools = self
            .components
            .session_workspace_tools
            .definitions()
            .definitions()
            .map(|definition| definition.name.clone())
            .collect::<Vec<_>>();
        session_tools.sort();
        let session_tool_names = session_tools.iter().cloned().collect::<BTreeSet<_>>();

        RuntimeCompositionSnapshot {
            schema_version: COMPOSITION_SNAPSHOT_VERSION,
            capabilities,
            components,
            activation_order: self.components.lifecycle.activation_order(),
            deactivation_order: self.components.lifecycle.deactivation_order(),
            pending,
            failures,
            effect_scopes: self
                .components
                .effect_scope_snapshots()
                .into_iter()
                .map(effect_scope_snapshot)
                .collect(),
            approval_providers,
            providers,
            selected_model,
            workspace_tools,
            session_tools,
            prompt_tools: self.prompt_tool_snapshots(&tool_definitions, &session_tool_names),
            prompt_sources: self
                .components
                .optional::<PromptAssemblyCapability>()
                .expect("PromptAssembly capability marker must match its registered value")
                .into_iter()
                .flat_map(|assembly| assembly.inspection_snapshot())
                .map(prompt_source_snapshot)
                .collect(),
            session_persistence: self.session_persistence_snapshot(),
        }
    }

    fn session_persistence_snapshot(&self) -> SessionPersistenceSnapshot {
        let snapshot = self
            .components
            .session_port
            .as_ref()
            .and_then(|session_port| session_port.inspection_snapshot());
        SessionPersistenceSnapshot {
            available: self
                .components
                .lifecycle
                .has_capability(&CapabilityKey::from("session_persistence")),
            mounted: snapshot.as_ref().is_some_and(|snapshot| snapshot.mounted),
            backend_id: snapshot
                .as_ref()
                .map(|snapshot| snapshot.backend_id.clone()),
            adapter_kind: snapshot.map(|snapshot| snapshot.adapter_kind),
        }
    }

    fn prompt_tool_snapshots(
        &self,
        tool_definitions: &[ToolDefinition],
        session_tool_names: &BTreeSet<String>,
    ) -> Vec<PromptToolSnapshot> {
        let mut tools = BTreeMap::new();
        let manager = self
            .components
            .optional::<PromptAssemblyCapability>()
            .expect("PromptAssembly capability marker must match its registered value")
            .and_then(|assembly| assembly.manager_snapshot());
        if let Some(manager) = manager.as_ref() {
            let catalog_tool_names = tool_definitions
                .iter()
                .map(|definition| definition.name.as_str())
                .collect::<BTreeSet<_>>();
            for candidate in manager
                .candidates
                .tools
                .iter()
                .filter(|candidate| catalog_tool_names.contains(candidate.name.as_str()))
            {
                tools.insert(
                    candidate.name.clone(),
                    PromptToolSnapshot {
                        name: candidate.name.clone(),
                        tool_enabled: candidate.tool_enabled,
                        session_enabled: session_tool_names.contains(&candidate.name),
                        guidelines_available: candidate.prompt_guidelines.is_some(),
                        guidelines_selected: candidate.selection.is_selected(),
                    },
                );
            }
        } else {
            for definition in tool_definitions {
                tools.insert(
                    definition.name.clone(),
                    PromptToolSnapshot {
                        name: definition.name.clone(),
                        tool_enabled: true,
                        session_enabled: session_tool_names.contains(&definition.name),
                        guidelines_available: definition.prompt_guidelines.is_some(),
                        guidelines_selected: definition.prompt_guidelines.is_some(),
                    },
                );
            }
        }
        tools.into_values().collect()
    }
}

fn effect_scope_snapshot(snapshot: EffectScopeSnapshot) -> EffectScopeOwnershipSnapshot {
    EffectScopeOwnershipSnapshot {
        owner: snapshot.owner,
        effects: snapshot.effects,
        children: snapshot
            .children
            .into_iter()
            .map(effect_scope_snapshot)
            .collect(),
    }
}

fn validate_effect_scopes(scopes: &[EffectScopeOwnershipSnapshot]) -> Result<(), String> {
    ensure_sorted_unique(
        scopes.iter().map(|scope| &scope.owner),
        "effect scope owner",
    )?;
    for scope in scopes {
        ensure_sorted_unique(scope.effects.iter(), "effect label")?;
        validate_effect_scopes(&scope.children)?;
    }
    Ok(())
}

fn validate_topology_orders(
    activation_order: &[String],
    deactivation_order: &[String],
    components: &BTreeMap<&str, &RuntimeComponentSnapshot>,
) -> Result<(), String> {
    let activation_ids = activation_order.iter().collect::<BTreeSet<_>>();
    if activation_ids.len() != activation_order.len()
        || activation_ids
            != components
                .values()
                .map(|component| &component.id)
                .collect::<BTreeSet<_>>()
    {
        return Err(
            "runtime composition activation order must contain every component exactly once"
                .to_string(),
        );
    }
    if deactivation_order != activation_order.iter().rev().cloned().collect::<Vec<_>>() {
        return Err(
            "runtime composition deactivation order must reverse activation order".to_string(),
        );
    }

    let positions = activation_order
        .iter()
        .enumerate()
        .map(|(index, id)| (id.as_str(), index))
        .collect::<BTreeMap<_, _>>();
    let mut providers = BTreeMap::new();
    for component in components.values() {
        for capability in &component.provides {
            if providers
                .insert(capability.as_str(), component.id.as_str())
                .is_some()
            {
                return Err(format!(
                    "runtime composition capability {} has multiple providers",
                    capability
                ));
            }
        }
    }
    for component in components.values() {
        for required in &component.required {
            let Some(provider_id) = providers.get(required.as_str()) else {
                continue;
            };
            if positions[provider_id] >= positions[component.id.as_str()] {
                return Err(format!(
                    "runtime composition provider {} must precede consumer {}",
                    provider_id, component.id
                ));
            }
        }
    }
    Ok(())
}

fn runtime_component_snapshot(component: ComponentSnapshot) -> RuntimeComponentSnapshot {
    RuntimeComponentSnapshot {
        id: component.id,
        state: component.state.as_str().to_string(),
        epoch: component.epoch,
        required: component.required,
        optional: component
            .optional
            .into_iter()
            .map(optional_dependency_snapshot)
            .collect(),
        provides: component.provides,
    }
}

fn pending_snapshot(pending: PendingComponentSnapshot) -> PendingSnapshot {
    PendingSnapshot {
        component_id: pending.component_id,
        missing_dependencies: pending.missing_dependencies,
    }
}

fn failure_snapshot(failure: ComponentFailureSnapshot) -> FailureSnapshot {
    FailureSnapshot {
        component_id: failure.component_id,
        operation: failure.operation.as_str().to_string(),
        code: failure.reason.code().to_string(),
        message: failure.reason.message().to_string(),
        recoverable: failure.recoverable,
        epoch: failure.epoch,
    }
}

fn optional_dependency_snapshot(
    dependency: OptionalCapabilitySnapshot,
) -> OptionalDependencySnapshot {
    OptionalDependencySnapshot {
        key: dependency.key,
        available: dependency.available,
    }
}

fn tool_snapshot(definition: &ToolDefinition) -> ToolSnapshot {
    ToolSnapshot {
        name: definition.name.clone(),
        kind: tool_kind_name(definition.kind).to_string(),
        permission_policy: permission_policy_name(definition.permission_policy).to_string(),
        has_prompt_guidelines: definition.prompt_guidelines.is_some(),
    }
}

fn prompt_source_snapshot(source: PromptContributionSnapshot) -> PromptSourceSnapshot {
    PromptSourceSnapshot {
        effective_order: source.effective_order,
        stable_id: source.stable_id,
        kind: prompt_source_kind_name(source.kind).to_string(),
        origin: source.origin.map(|origin| origin.as_str().to_string()),
        scope: source
            .scope
            .map(|scope| scope.as_stored_value().to_string()),
        priority: source.priority,
        is_trusted: source.is_trusted,
        estimated_tokens: source.estimated_tokens,
    }
}

const fn prompt_source_kind_name(
    kind: runtime_domain::prompt_assembly::PromptSourceKind,
) -> &'static str {
    use runtime_domain::prompt_assembly::PromptSourceKind;

    match kind {
        PromptSourceKind::CoreSystemPrompt => "core_system_prompt",
        PromptSourceKind::InstructionsFile => "instructions_file",
        PromptSourceKind::ExtraPrompt => "extra_prompt",
        PromptSourceKind::SkillDiscovery => "skill_discovery",
        PromptSourceKind::LongLivedSkill => "long_lived_skill",
        PromptSourceKind::ToolGuidelines => "tool_guidelines",
        PromptSourceKind::DynamicEnvironmentBaseline => "dynamic_environment_baseline",
        PromptSourceKind::DynamicEnvironmentChanges => "dynamic_environment_changes",
    }
}

const fn tool_kind_name(kind: ToolKind) -> &'static str {
    match kind {
        ToolKind::Read => "read",
        ToolKind::Write => "write",
        ToolKind::Edit => "edit",
        ToolKind::Delete => "delete",
        ToolKind::Move => "move",
        ToolKind::Search => "search",
        ToolKind::Execute => "execute",
        ToolKind::Think => "think",
        ToolKind::Fetch => "fetch",
        ToolKind::SwitchMode => "switch_mode",
        ToolKind::Other => "other",
    }
}

const fn permission_policy_name(policy: ToolPermissionPolicy) -> &'static str {
    match policy {
        ToolPermissionPolicy::Never => "never",
        ToolPermissionPolicy::Ask => "ask",
        ToolPermissionPolicy::Always => "always",
    }
}

fn ensure_sorted_unique<'a>(
    values: impl IntoIterator<Item = &'a String>,
    label: &str,
) -> Result<(), String> {
    let mut previous: Option<&str> = None;
    for value in values {
        if previous.is_some_and(|previous| previous >= value.as_str()) {
            return Err(format!(
                "runtime composition {label} entries must be sorted and unique"
            ));
        }
        previous = Some(value);
    }
    Ok(())
}

fn ensure_prompt_sources_valid(sources: &[PromptSourceSnapshot]) -> Result<(), String> {
    let mut stable_ids = BTreeSet::new();
    for (expected_order, source) in sources.iter().enumerate() {
        if source.effective_order != expected_order {
            return Err(
                "runtime composition prompt sources must follow effective order".to_string(),
            );
        }
        if !stable_ids.insert(source.stable_id.as_str()) {
            return Err("runtime composition prompt source entries must be unique".to_string());
        }
    }
    Ok(())
}
