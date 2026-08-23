use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;
use tool_runtime::{ToolDefinition, ToolKind, ToolPermissionPolicy};

use super::{
    AppRuntimeCoordinator,
    lifecycle::{
        ComponentDefinition, ComponentGraph, ComponentSnapshot, OptionalCapabilitySnapshot,
    },
};

const COMPOSITION_SNAPSHOT_VERSION: u32 = 1;

/// `RuntimeCompositionSnapshot` 是默认 runtime composition 的只读诊断投影。
///
/// 该类型不是配置或持久化格式。字段只记录可组合性事实，禁止包含 API key、base URL、
/// prompt/instruction body、用户内容或 tool output。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct RuntimeCompositionSnapshot {
    schema_version: u32,
    capabilities: Vec<CapabilitySnapshot>,
    components: Vec<RuntimeComponentSnapshot>,
    providers: Vec<ProviderSnapshot>,
    selected_model: Option<ModelSelectionSnapshot>,
    workspace_tools: Vec<ToolSnapshot>,
    session_tools: Vec<String>,
    prompt_tools: Vec<PromptToolSnapshot>,
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

        let capabilities = self
            .capabilities
            .iter()
            .map(|capability| capability.key.as_str())
            .collect::<BTreeSet<_>>();
        for component in &self.components {
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
                _ => {}
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct CapabilitySnapshot {
    key: String,
    generation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct RuntimeComponentSnapshot {
    id: String,
    state: String,
    required: Vec<String>,
    optional: Vec<OptionalDependencySnapshot>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct OptionalDependencySnapshot {
    key: String,
    available: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct ProviderSnapshot {
    id: String,
    kind: String,
    enabled: bool,
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
struct SessionPersistenceSnapshot {
    available: bool,
}

impl AppRuntimeCoordinator {
    pub(super) fn inspect_composition(&self) -> RuntimeCompositionSnapshot {
        let lifecycle = self.composition_lifecycle();
        let capabilities = lifecycle
            .capabilities()
            .into_iter()
            .map(|capability| CapabilitySnapshot {
                key: capability.key,
                generation: capability.generation,
            })
            .collect();
        let components = lifecycle
            .components()
            .into_iter()
            .map(runtime_component_snapshot)
            .collect();

        let mut providers = self
            .options
            .loaded_models
            .catalog
            .providers()
            .map(|provider| {
                let mut model_ids = provider
                    .models
                    .iter()
                    .map(|model| model.id.clone())
                    .collect::<Vec<_>>();
                model_ids.sort();
                ProviderSnapshot {
                    id: provider.id.clone(),
                    kind: provider.connection.kind.as_config_value().to_string(),
                    enabled: provider.enabled,
                    model_ids,
                }
            })
            .collect::<Vec<_>>();
        providers.sort_by(|left, right| left.id.cmp(&right.id));

        let selected_model = self
            .options
            .loaded_models
            .selected_model
            .as_ref()
            .map(|selection| ModelSelectionSnapshot {
                provider_id: selection.provider_id.clone(),
                model_id: selection.model_id.clone(),
            });

        let mut workspace_tools = self
            .workspace_tools
            .definitions()
            .definitions()
            .map(tool_snapshot)
            .collect::<Vec<_>>();
        workspace_tools.sort_by(|left, right| left.name.cmp(&right.name));

        let mut session_tools = self
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
            providers,
            selected_model,
            workspace_tools,
            session_tools,
            prompt_tools: self.prompt_tool_snapshots(&session_tool_names),
            session_persistence: SessionPersistenceSnapshot {
                available: self.options.session_store.is_some(),
            },
        }
    }

    fn composition_lifecycle(&self) -> ComponentGraph {
        let mut graph = ComponentGraph::default();
        for capability in [
            "conversation_worker",
            "model_catalog",
            "prompt_assembly",
            "runtime_event_stream",
            "tool_catalog",
        ] {
            graph.add_capability(capability);
        }
        if self.options.session_store.is_some() {
            graph.add_capability("session_persistence");
        }
        if self.runtime_wake_effect.is_some() {
            graph.add_capability("runtime_wake");
        }

        graph.declare(
            ComponentDefinition::new("native_agent_runtime")
                .requires("conversation_worker")
                .requires("model_catalog")
                .requires("prompt_assembly")
                .requires("tool_catalog")
                .observes("session_persistence"),
        );
        graph.declare(
            ComponentDefinition::new("prompt_assembly")
                .requires("tool_catalog")
                .observes("session_persistence"),
        );
        graph.declare(
            ComponentDefinition::new("ui_runtime_bridge")
                .requires("runtime_event_stream")
                .requires("runtime_wake"),
        );
        graph.take_transitions();
        graph
    }

    fn prompt_tool_snapshots(
        &self,
        session_tool_names: &BTreeSet<String>,
    ) -> Vec<PromptToolSnapshot> {
        let mut tools = BTreeMap::new();
        if let Some(manager) = self.options.prompt_assembly_manager.as_ref() {
            for candidate in &manager.candidates.tools {
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
            for definition in &self.prompt_assembly_tool_definitions {
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

fn runtime_component_snapshot(component: ComponentSnapshot) -> RuntimeComponentSnapshot {
    RuntimeComponentSnapshot {
        id: component.id,
        state: component.state.as_str().to_string(),
        required: component.required,
        optional: component
            .optional
            .into_iter()
            .map(optional_dependency_snapshot)
            .collect(),
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
