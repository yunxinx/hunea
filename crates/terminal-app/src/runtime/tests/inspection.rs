use std::sync::atomic::{AtomicUsize, Ordering};

use runtime_domain::runtime_wake::RuntimeWake;
use runtime_domain::{
    model_catalog::{ModelCatalog, ModelEntry, ModelProvider, ModelSelection, ModelSource},
    prompt_assembly::{
        PromptAssemblyManagerSnapshot, PromptAssemblySelectionState, PromptAssemblyToolCandidate,
        PromptPreludeSection, PromptPreludeSnapshot, PromptSourceKind, PromptSourceOrigin,
        persistence::PromptAssemblyScope,
    },
    provider::ProviderKind,
};
use terminal_ui::RuntimeEventPort;

use super::support::*;
use crate::runtime::{
    agent::{
        AgentRuntimeActivationGrants, AgentRuntimeActivity, AgentRuntimePort,
        AgentSessionCapability,
    },
    agent_capability_context::{AgentChildCapabilityGrants, AgentContextOwner},
    context::{PromptAssemblyCapability, ToolCatalogCapability},
    lifecycle::ComponentFailureReason,
    prompt_assembly::PromptSectionContribution,
};
use runtime_domain::agent::{
    AgentCommand, AgentCommandReceipt, AgentEvent, AgentId, AgentObjective, AgentRuntime,
    AgentRuntimeError, AgentTitle, AgentTurnId,
};

const SECRET_SENTINEL: &str = "inspection-secret-sentinel";

#[test]
fn composition_snapshot_is_deterministic_and_redacted() {
    let store: Arc<dyn SessionStore> = Arc::new(InMemorySessionStore::new());
    let provider = ModelProvider::new(
        "z-provider",
        ProviderKind::OpenAiCompatible,
        "Z Provider",
        true,
        ModelSource::Configured,
        vec![
            ModelEntry::new("z-model", None, ModelSource::Configured),
            ModelEntry::new("a-model", None, ModelSource::Configured),
        ],
    );
    let disabled_provider = ModelProvider::disabled(
        "a-provider",
        ProviderKind::Anthropic,
        "A Provider",
        false,
        ModelSource::Configured,
        vec![ModelEntry::new(
            "disabled-model",
            None,
            ModelSource::Configured,
        )],
    );
    let unsupported_provider = ModelProvider::new(
        "unsupported-provider",
        ProviderKind::Anthropic,
        "Unsupported Provider",
        true,
        ModelSource::Configured,
        vec![ModelEntry::new(
            "unsupported-model",
            None,
            ModelSource::Configured,
        )],
    );
    let mut coordinator = runtime_coordinator(AppRuntimeOptions {
        loaded_models: conversation_runtime::models::LoadedModelCatalog {
            catalog: ModelCatalog::new(vec![provider, disabled_provider, unsupported_provider]),
            provider_configs: vec![
                conversation_runtime::models::LoadedProviderConfig::new(
                    "z-provider",
                    ProviderKind::OpenAiCompatible,
                    Some(format!("https://{SECRET_SENTINEL}@example.test/v1")),
                    Some(runtime_domain::provider::ProviderApiKey::new(
                        SECRET_SENTINEL,
                    )),
                    Some(SECRET_SENTINEL.to_string()),
                    true,
                ),
                conversation_runtime::models::LoadedProviderConfig::new(
                    "unsupported-provider",
                    ProviderKind::Anthropic,
                    Some(format!("https://{SECRET_SENTINEL}.unsupported.test/v1")),
                    None,
                    None,
                    true,
                ),
            ],
            selected_model: Some(ModelSelection::new("z-provider", "z-model")),
            ..conversation_runtime::models::LoadedModelCatalog::default()
        },
        session_store: Some(store),
        initial_prompt_assembly: Some(prompt_manager_with_prelude(PromptPreludeSnapshot {
            sections: vec![PromptPreludeSection {
                reference_id: "private-instruction".to_string(),
                kind: PromptSourceKind::CoreSystemPrompt,
                title: "private instruction".to_string(),
                origin: None,
                body: SECRET_SENTINEL.to_string(),
            }],
        })),
        ..AppRuntimeOptions::default()
    });
    let _private_runtime_registration = coordinator
        .components
        .require::<PromptAssemblyCapability>()
        .expect("prompt assembly capability should be available")
        .contribute(
            "runtime-private-owner",
            PromptSectionContribution {
                stable_id: "runtime-private".to_string(),
                scope: PromptAssemblyScope::Global,
                priority: -1,
                is_trusted: true,
                estimated_tokens: Some(7),
                section: PromptPreludeSection {
                    reference_id: "runtime-private".to_string(),
                    kind: PromptSourceKind::ExtraPrompt,
                    title: "private runtime title".to_string(),
                    origin: Some(PromptSourceOrigin::Builtin),
                    body: SECRET_SENTINEL.to_string(),
                },
            },
        )
        .expect("private runtime contribution should register");

    let workspace_tool_names = coordinator
        .components
        .require::<ToolCatalogCapability>()
        .expect("tool catalog capability should be available")
        .definitions()
        .into_iter()
        .map(|definition| definition.name)
        .collect::<Vec<_>>();
    let mut prompt_manager = coordinator
        .components
        .require::<PromptAssemblyCapability>()
        .expect("prompt assembly capability should be available")
        .manager_snapshot()
        .expect("test prelude should be owned by a manager");
    prompt_manager.candidates.tools = coordinator
        .components
        .require::<ToolCatalogCapability>()
        .expect("tool catalog capability should be available")
        .definitions()
        .into_iter()
        .map(|definition| PromptAssemblyToolCandidate {
            name: definition.name,
            label: None,
            description: None,
            prompt_guidelines: definition.prompt_guidelines,
            origin: PromptSourceOrigin::Builtin,
            selection_scope: PromptAssemblyScope::Global,
            tool_enabled: true,
            selection: PromptAssemblySelectionState::Selected { order: None },
        })
        .collect();
    coordinator
        .components
        .require::<PromptAssemblyCapability>()
        .expect("prompt assembly capability should be available")
        .replace_manager(Some(prompt_manager))
        .expect("test prompt manager should be replaceable");
    let disabled_session_tool = workspace_tool_names
        .first()
        .expect("default composition should expose workspace tools")
        .clone();
    coordinator.components.session_workspace_tools = coordinator
        .components
        .require::<ToolCatalogCapability>()
        .expect("tool catalog capability should be available")
        .filtered(|name| name != disabled_session_tool);

    let first = serde_json::to_vec(&coordinator.inspect_composition())
        .expect("composition snapshot should serialize");
    let second = serde_json::to_vec(&coordinator.inspect_composition())
        .expect("composition snapshot should serialize repeatedly");
    assert_eq!(first, second);

    let json = String::from_utf8(first).expect("snapshot JSON should be UTF-8");
    assert!(
        !json.contains(SECRET_SENTINEL),
        "snapshot must not contain credentials or instruction bodies: {json}"
    );
    assert!(!json.contains("runtime-private-owner"));
    assert!(!json.contains("terminal-runtime"));
    assert!(!json.contains("runtime_composition"));
    assert!(!json.contains("registration_id"));
    assert!(!json.contains("entry_id"));
    assert!(!json.contains("display_name"));
    assert!(!json.contains("constructor"));
    assert!(!json.contains("callback"));

    let snapshot: serde_json::Value =
        serde_json::from_str(&json).expect("snapshot JSON should decode");
    assert_eq!(snapshot["schema_version"], 10);
    assert_eq!(snapshot["failures"], serde_json::json!([]));
    assert_eq!(
        snapshot["capabilities"],
        serde_json::json!([
            {
                "key": "approval_provider",
                "provider_component": "approval_provider",
                "generation": 0,
            },
            {
                "key": "extension_hooks",
                "provider_component": "extension_hooks",
                "generation": 0,
            },
            {
                "key": "llm_port",
                "provider_component": "llm_port",
                "generation": 0,
            },
            {
                "key": "model_catalog",
                "provider_component": "llm_port",
                "generation": 0,
            },
            {
                "key": "permission_policy",
                "provider_component": "permission_policy",
                "generation": 0,
            },
            {
                "key": "prompt_assembly",
                "provider_component": "prompt_assembly",
                "generation": 0,
            },
            {
                "key": "runtime_event_stream",
                "provider_component": "runtime_event_stream",
                "generation": 0,
            },
            {
                "key": "session_persistence",
                "provider_component": "session_persistence",
                "generation": 0,
            },
            {
                "key": "tool_catalog",
                "provider_component": "tool_catalog",
                "generation": 0,
            },
        ])
    );
    assert_eq!(
        names(&snapshot["components"]),
        vec![
            "agent_runtime",
            "approval_provider",
            "context_budget",
            "extension_hooks",
            "llm_port",
            "model_refresh",
            "permission_policy",
            "prompt_assembly",
            "runtime_event_stream",
            "runtime_wake_binding",
            "session_persistence",
            "tool_catalog",
            "ui_runtime_bridge",
        ]
    );
    for (component_id, plugin_type) in [
        ("approval_provider", "terminal-approval-provider"),
        ("context_budget", "context-budget"),
        ("extension_hooks", "typed-extension-hooks"),
        ("llm_port", "openai-compatible-provider-catalog"),
        ("model_refresh", "model-refresh"),
        ("agent_runtime", "native-agent-loop"),
        ("permission_policy", "permission-policy"),
        ("prompt_assembly", "prompt-assembly"),
        ("runtime_event_stream", "runtime-event-stream"),
        ("runtime_wake_binding", "runtime-wake-slot"),
        ("session_persistence", "session-persistence"),
        ("tool_catalog", "workspace-tools"),
        ("ui_runtime_bridge", "terminal-ui-runtime-adapter"),
    ] {
        let component = snapshot["components"]
            .as_array()
            .expect("components should be an array")
            .iter()
            .find(|component| component["id"] == component_id)
            .expect("component should have plugin metadata");
        assert_eq!(component["plugin_type"], plugin_type);
        assert_eq!(component["config_schema_version"], 1);
        assert_eq!(component["reload_policy"], "replace");
        assert_eq!(component["trust"], "builtin");
    }
    for (component_id, provides) in [
        ("approval_provider", &["approval_provider"][..]),
        ("extension_hooks", &["extension_hooks"][..]),
        ("llm_port", &["llm_port", "model_catalog"][..]),
        ("permission_policy", &["permission_policy"][..]),
        ("prompt_assembly", &["prompt_assembly"][..]),
        ("runtime_event_stream", &["runtime_event_stream"][..]),
        ("runtime_wake_binding", &["runtime_wake"][..]),
        ("session_persistence", &["session_persistence"][..]),
        ("tool_catalog", &["tool_catalog"][..]),
    ] {
        assert_eq!(component_provides(&snapshot, component_id), provides);
    }
    assert_eq!(
        snapshot["activation_order"],
        serde_json::json!([
            "approval_provider",
            "extension_hooks",
            "llm_port",
            "runtime_event_stream",
            "model_refresh",
            "permission_policy",
            "runtime_wake_binding",
            "session_persistence",
            "tool_catalog",
            "prompt_assembly",
            "agent_runtime",
            "context_budget",
            "ui_runtime_bridge",
        ])
    );
    assert_eq!(
        snapshot["deactivation_order"],
        serde_json::json!([
            "ui_runtime_bridge",
            "context_budget",
            "agent_runtime",
            "prompt_assembly",
            "tool_catalog",
            "session_persistence",
            "runtime_wake_binding",
            "permission_policy",
            "model_refresh",
            "runtime_event_stream",
            "llm_port",
            "extension_hooks",
            "approval_provider",
        ])
    );
    assert_eq!(
        snapshot["pending"],
        serde_json::json!([{
            "component_id": "ui_runtime_bridge",
            "missing_dependencies": ["runtime_wake"],
        }])
    );
    assert_eq!(snapshot["session_persistence"]["available"], true);
    assert_eq!(snapshot["session_persistence"]["mounted"], true);
    assert_eq!(
        snapshot["session_persistence"]["backend_id"],
        "configured-session-store"
    );
    assert_eq!(
        snapshot["session_persistence"]["adapter_kind"],
        "session-store"
    );
    assert_eq!(
        snapshot["selected_model"],
        serde_json::json!({"provider_id": "z-provider", "model_id": "z-model"})
    );
    assert_eq!(names(&snapshot["providers"]), vec!["z-provider"]);
    assert_eq!(snapshot["providers"][0]["kind"], "openai_compatible");
    assert_eq!(
        snapshot["providers"][0]["adapter_kind"],
        "openai-compatible"
    );
    assert_eq!(snapshot["providers"][0]["mounted"], true);
    assert_eq!(
        snapshot["providers"][0]["model_ids"],
        serde_json::json!(["a-model", "z-model"])
    );
    assert_eq!(
        snapshot["approval_providers"],
        serde_json::json!([{
            "provider_id": "terminal-interactive",
            "adapter_kind": "terminal-interactive",
            "mounted": true,
        }])
    );
    assert_eq!(
        snapshot["effect_scopes"],
        serde_json::json!([
            {
                "owner": "agent_runtime",
                "lifecycle": "active",
                "effects": [
                    "dependency:extension_hooks",
                    "dependency:llm_port",
                    "dependency:permission_policy",
                    "dependency:prompt_assembly",
                    "dependency:runtime_event_stream",
                    "dependency:tool_catalog",
                ],
                "children": [{
                    "owner": "main-agent",
                    "lifecycle": "active",
                    "effects": [],
                    "children": [],
                }],
            },
            {
                "owner": "approval_provider",
                "lifecycle": "active",
                "effects": ["approval_provider_registration", "capability:approval_provider"],
                "children": [],
            },
            {
                "owner": "context_budget",
                "lifecycle": "active",
                "effects": ["dependency:runtime_event_stream"],
                "children": [],
            },
            {
                "owner": "extension_hooks",
                "lifecycle": "active",
                "effects": ["capability:extension_hooks"],
                "children": [],
            },
            {
                "owner": "llm_port",
                "lifecycle": "active",
                "effects": ["capability:llm_port", "capability:model_catalog", "provider_registrations"],
                "children": [],
            },
            {
                "owner": "model_refresh",
                "lifecycle": "active",
                "effects": ["dependency:runtime_event_stream"],
                "children": [],
            },
            {
                "owner": "permission_policy",
                "lifecycle": "active",
                "effects": ["capability:permission_policy", "dependency:runtime_event_stream"],
                "children": [],
            },
            {
                "owner": "prompt_assembly",
                "lifecycle": "active",
                "effects": ["capability:prompt_assembly", "prompt_registration"],
                "children": [],
            },
            {
                "owner": "runtime_event_stream",
                "lifecycle": "active",
                "effects": ["capability:runtime_event_stream"],
                "children": [],
            },
            {
                "owner": "runtime_wake_binding",
                "lifecycle": "active",
                "effects": [],
                "children": [],
            },
            {
                "owner": "session_persistence",
                "lifecycle": "active",
                "effects": ["backend_registration", "capability:session_persistence", "dependency:runtime_event_stream"],
                "children": [],
            },
            {
                "owner": "tool_catalog",
                "lifecycle": "active",
                "effects": ["capability:tool_catalog", "tool_registrations"],
                "children": [],
            },
        ])
    );

    let workspace_names = names(&snapshot["workspace_tools"]);
    let mut sorted_workspace_names = workspace_names.clone();
    sorted_workspace_names.sort();
    assert_eq!(workspace_names, sorted_workspace_names);
    // spawn_agents 在所有 composition 都注册为 metadata-only workspace tool：锁定其
    // 存在与 closed metadata 字段，防止 catalog 变更引入 UI/runtime implementation 依赖。
    let spawn_agents_snapshot = snapshot["workspace_tools"]
        .as_array()
        .expect("workspace tools should be an array")
        .iter()
        .find(|tool| tool["name"] == "spawn_agents")
        .expect("spawn_agents should stay registered in the default composition");
    assert_eq!(spawn_agents_snapshot["kind"], "other");
    assert_eq!(spawn_agents_snapshot["permission_policy"], "never");
    assert_eq!(spawn_agents_snapshot["has_prompt_guidelines"], false);
    assert!(
        !snapshot["session_tools"]
            .as_array()
            .expect("session tool names should be an array")
            .iter()
            .any(|name| name == &disabled_session_tool)
    );
    let disabled_prompt_tool = snapshot["prompt_tools"]
        .as_array()
        .expect("prompt tools should be an array")
        .iter()
        .find(|tool| tool["name"] == disabled_session_tool)
        .expect("prompt inventory should retain tools disabled for the session");
    assert_eq!(disabled_prompt_tool["tool_enabled"], true);
    assert_eq!(disabled_prompt_tool["session_enabled"], false);
    assert_eq!(
        snapshot["prompt_sources"],
        serde_json::json!([
            {
                "effective_order": 0,
                "stable_id": "runtime-private",
                "kind": "extra_prompt",
                "origin": "builtin",
                "scope": "global",
                "priority": -1,
                "is_trusted": true,
                "estimated_tokens": 7,
            },
            {
                "effective_order": 1,
                "stable_id": "private-instruction",
                "kind": "core_system_prompt",
                "origin": null,
                "scope": null,
                "priority": 0,
                "is_trusted": false,
                "estimated_tokens": null,
            }
        ])
    );
}

#[test]
fn composition_snapshot_projects_child_agent_scopes_without_instruction_bodies() {
    /// inspection scope 斋试的最小 child runtime：无事件、无 session capability。
    struct InspectionScopeChildRuntime;

    impl AgentRuntime for InspectionScopeChildRuntime {
        fn dispatch(
            &mut self,
            _command: AgentCommand,
        ) -> Result<AgentCommandReceipt, AgentRuntimeError> {
            Ok(AgentCommandReceipt::Accepted)
        }

        fn drain_events(&mut self) -> Vec<AgentEvent> {
            Vec::new()
        }

        fn shutdown(&mut self) -> Result<(), AgentRuntimeError> {
            Ok(())
        }
    }

    impl AgentRuntimePort for InspectionScopeChildRuntime {
        fn activate(&mut self, _grants: AgentRuntimeActivationGrants) -> Result<(), String> {
            Ok(())
        }

        fn suspend(&mut self) -> Result<(), AgentRuntimeError> {
            Ok(())
        }

        fn activity(&self) -> AgentRuntimeActivity {
            AgentRuntimeActivity::Idle
        }

        fn session(&self) -> Option<&dyn AgentSessionCapability> {
            None
        }

        fn session_mut(&mut self) -> Option<&mut dyn AgentSessionCapability> {
            None
        }

        fn has_pending_work(&self) -> bool {
            false
        }
    }

    let mut coordinator = runtime_coordinator(AppRuntimeOptions::default());
    let root_context = coordinator
        .components
        .agent_root_context_for_test()
        .expect("Native Agent should own a root context");
    // owner 命名与 stage_child_record 的 production 约定一致（child-agent-{id}）。
    let child_context = root_context
        .child(
            AgentContextOwner::try_new("child-agent-2").expect("child owner should validate"),
            AgentChildCapabilityGrants::empty()
                .inherit_tools()
                .inherit_prompt(),
        )
        .expect("child context should attach to the root scope");
    // title 携带 SECRET sentinel：scope 投影只允许 closed owner/label，不允许任何内容体。
    let title = AgentTitle::resolve(
        &AgentObjective::new(format!("child task {SECRET_SENTINEL}"))
            .expect("objective should be valid"),
        None,
    )
    .expect("title should resolve");
    coordinator.components.register_child_agent_for_test(
        AgentId::new(2),
        AgentId::MAIN,
        AgentTurnId::new(1),
        title,
        child_context,
        Box::new(InspectionScopeChildRuntime),
    );

    let snapshot = composition_snapshot(&coordinator);
    let child_scope = snapshot["effect_scopes"]
        .as_array()
        .expect("effect scopes should be an array")
        .iter()
        .find(|scope| scope["owner"] == "agent_runtime")
        .expect("the agent component scope should be present")["children"]
        .as_array()
        .expect("main-agent scope children should be an array")
        .iter()
        .find(|scope| scope["owner"] == "main-agent")
        .expect("the main-agent scope should be present")["children"]
        .as_array()
        .expect("child scopes should be an array")
        .iter()
        .find(|scope| scope["owner"] == "child-agent-2")
        .expect("the child scope should appear under the main-agent scope");
    // 只有 closed owner/lifecycle/effects/children。effect label 是固定控制格式
    // `agent_effect:{kind}:{per-context id}`：child context 的首个 effect 就是其 worker。
    assert_eq!(child_scope["lifecycle"], "active");
    assert_eq!(
        child_scope["effects"],
        serde_json::json!(["agent_effect:worker:0"])
    );
    assert_eq!(child_scope["children"], serde_json::json!([]));

    let json = serde_json::to_string(&snapshot).expect("snapshot should serialize");
    assert!(
        !json.contains(SECRET_SENTINEL),
        "child scope projection must not leak instruction or objective bodies: {json}"
    );

    // child scope 随 child tree 回收从 snapshot 消失（closed counts 归零语义）。
    coordinator
        .components
        .dispose_child_agents_for_session_transition()
        .expect("session transition should retire the child tree");
    let snapshot = composition_snapshot(&coordinator);
    let main_agent_scope_children = snapshot["effect_scopes"]
        .as_array()
        .expect("effect scopes should be an array")
        .iter()
        .find(|scope| scope["owner"] == "agent_runtime")
        .expect("the agent component scope should be present")["children"]
        .as_array()
        .expect("main-agent scope children should be an array")
        .iter()
        .find(|scope| scope["owner"] == "main-agent")
        .expect("the main-agent scope should be present")["children"]
        .as_array()
        .expect("child scopes should be an array");
    assert!(
        main_agent_scope_children
            .iter()
            .all(|scope| scope["owner"] != "child-agent-2"),
        "a disposed child scope must disappear from the inspection snapshot"
    );
}

#[test]
fn composition_snapshot_projects_safe_failure_and_pending_diagnostics() {
    let mut coordinator = runtime_coordinator(AppRuntimeOptions::default());
    let deactivation = coordinator
        .components
        .lifecycle
        .suspend("model_refresh")
        .expect("model refresh should begin suspension");
    coordinator
        .components
        .lifecycle
        .complete_deactivation(deactivation.deactivation_requests[0].clone())
        .expect("model refresh suspension should complete");
    let activation = coordinator
        .components
        .lifecycle
        .activate("model_refresh")
        .expect("model refresh should begin reactivation");
    coordinator
        .components
        .lifecycle
        .fail_activation(
            activation.activation_requests[0].clone(),
            ComponentFailureReason::ActivationRejected,
            true,
        )
        .expect("model refresh activation should fail at the current epoch");
    let deactivation = coordinator
        .components
        .lifecycle
        .deactivate("context_budget")
        .expect("context budget should begin deactivation");
    coordinator
        .components
        .lifecycle
        .fail_deactivation(
            deactivation.deactivation_requests[0].clone(),
            ComponentFailureReason::EffectDisposalRejected,
            false,
        )
        .expect("context budget deactivation should fail at the current epoch");

    let snapshot = coordinator.inspect_composition();
    snapshot
        .validate()
        .expect("failure-aware composition snapshot should validate");
    let json = serde_json::to_string(&snapshot).expect("snapshot should serialize");
    assert!(!json.contains(SECRET_SENTINEL));
    let snapshot: serde_json::Value = serde_json::from_str(&json).expect("snapshot should decode");

    assert_eq!(
        snapshot["pending"],
        serde_json::json!([{
            "component_id": "ui_runtime_bridge",
            "missing_dependencies": ["runtime_wake"],
        }])
    );
    assert_eq!(
        snapshot["failures"],
        serde_json::json!([
            {
                "component_id": "context_budget",
                "operation": "deactivation",
                "code": "effect_disposal_rejected",
                "message": "component effect disposal was rejected",
                "recoverable": false,
                "epoch": 1,
            },
            {
                "component_id": "model_refresh",
                "operation": "activation",
                "code": "activation_rejected",
                "message": "component activation was rejected",
                "recoverable": true,
                "epoch": 2,
            },
        ])
    );
}

#[test]
fn ui_runtime_bridge_reacts_to_wake_binding_lifecycle() {
    let mut prompt_assembly_manager = PromptAssemblyManagerSnapshot::default();
    prompt_assembly_manager.candidates.tools = vec![PromptAssemblyToolCandidate {
        name: "bash".to_string(),
        label: None,
        description: None,
        prompt_guidelines: Some("private tool guidance".to_string()),
        origin: PromptSourceOrigin::Builtin,
        selection_scope: PromptAssemblyScope::Global,
        tool_enabled: true,
        selection: PromptAssemblySelectionState::Selected { order: None },
    }];
    let mut coordinator = runtime_coordinator(AppRuntimeOptions {
        loaded_models: conversation_runtime::models::LoadedModelCatalog {
            provider_configs: vec![conversation_runtime::models::LoadedProviderConfig::new(
                "local",
                ProviderKind::OpenAiCompatible,
                Some("http://localhost:11434/v1".to_string()),
                None,
                None,
                true,
            )],
            ..conversation_runtime::models::LoadedModelCatalog::default()
        },
        session_store: Some(Arc::new(InMemorySessionStore::new())),
        initial_prompt_assembly: Some(prompt_assembly_manager),
        ..AppRuntimeOptions::default()
    });
    assert_eq!(
        component_state(&coordinator, "ui_runtime_bridge"),
        "pending"
    );

    let wake_count = Arc::new(AtomicUsize::new(0));
    let wake_count_for_callback = Arc::clone(&wake_count);
    RuntimeEventPort::bind_runtime_wake(
        &mut coordinator,
        RuntimeWake::new(move || {
            wake_count_for_callback.fetch_add(1, Ordering::SeqCst);
        }),
    )
    .expect("runtime wake should bind");

    assert_eq!(component_state(&coordinator, "ui_runtime_bridge"), "active");
    assert_eq!(
        composition_snapshot(&coordinator)["effect_scopes"]
            .as_array()
            .expect("effect scopes should be an array")
            .iter()
            .find(|scope| scope["owner"] == "ui_runtime_bridge")
            .expect("wake consumer should own its binding effect")["effects"],
        serde_json::json!([
            "dependency:runtime_event_stream",
            "dependency:runtime_wake",
            "runtime_wake_binding"
        ])
    );
    assert_eq!(
        names(&composition_snapshot(&coordinator)["providers"]),
        vec!["local"]
    );
    assert_eq!(
        composition_snapshot(&coordinator)["prompt_tools"],
        serde_json::json!([{
            "name": "bash",
            "tool_enabled": true,
            "session_enabled": true,
            "guidelines_available": true,
            "guidelines_selected": true,
        }])
    );
    coordinator.components.notify_runtime_event();
    assert_eq!(wake_count.load(Ordering::SeqCst), 1);

    coordinator.shutdown().expect("runtime should shut down");
    assert_eq!(
        component_state(&coordinator, "ui_runtime_bridge"),
        "disposed"
    );
    coordinator.components.notify_runtime_event();
    assert_eq!(
        wake_count.load(Ordering::SeqCst),
        1,
        "disposing the runtime owner must remove the old wake callback"
    );
    let snapshot = composition_snapshot(&coordinator);
    assert_eq!(snapshot["session_persistence"]["available"], false);
    assert_eq!(snapshot["session_persistence"]["mounted"], false);
    assert_eq!(
        snapshot["session_persistence"]["backend_id"],
        serde_json::Value::Null
    );
    assert_eq!(snapshot["workspace_tools"], serde_json::json!([]));
    assert_eq!(snapshot["session_tools"], serde_json::json!([]));
    assert_eq!(snapshot["prompt_tools"], serde_json::json!([]));
    assert_eq!(snapshot["prompt_sources"], serde_json::json!([]));
    assert_eq!(snapshot["providers"], serde_json::json!([]));
    assert_eq!(snapshot["approval_providers"], serde_json::json!([]));
    assert_eq!(snapshot["effect_scopes"], serde_json::json!([]));
    assert_eq!(component_state(&coordinator, "agent_runtime"), "disposed");
}

#[test]
fn rebinding_ui_runtime_bridge_disposes_the_previous_wake_effect() {
    let mut coordinator = runtime_coordinator(AppRuntimeOptions::default());
    let first_wake_count = Arc::new(AtomicUsize::new(0));
    let first_wake_count_for_callback = Arc::clone(&first_wake_count);
    RuntimeEventPort::bind_runtime_wake(
        &mut coordinator,
        RuntimeWake::new(move || {
            first_wake_count_for_callback.fetch_add(1, Ordering::SeqCst);
        }),
    )
    .expect("first runtime wake should bind");
    let first_generation =
        capability_generation(&composition_snapshot(&coordinator), "runtime_wake");
    assert_eq!(first_generation, 0);

    let second_wake_count = Arc::new(AtomicUsize::new(0));
    let second_wake_count_for_callback = Arc::clone(&second_wake_count);
    RuntimeEventPort::bind_runtime_wake(
        &mut coordinator,
        RuntimeWake::new(move || {
            second_wake_count_for_callback.fetch_add(1, Ordering::SeqCst);
        }),
    )
    .expect("replacement runtime wake should bind");

    coordinator.components.notify_runtime_event();
    assert_eq!(first_wake_count.load(Ordering::SeqCst), 0);
    assert_eq!(second_wake_count.load(Ordering::SeqCst), 1);
    assert_eq!(
        capability_generation(&composition_snapshot(&coordinator), "runtime_wake",),
        first_generation + 1
    );
    assert_eq!(component_state(&coordinator, "ui_runtime_bridge"), "active");
}

#[test]
fn reset_replaces_session_component_generations_without_rebuilding_the_ui_bridge() {
    let mut coordinator = runtime_coordinator(AppRuntimeOptions::default());
    RuntimeEventPort::bind_runtime_wake(&mut coordinator, RuntimeWake::new(|| {}))
        .expect("runtime wake should bind");
    let before = composition_snapshot(&coordinator);
    let workspace_tools_before = before["workspace_tools"].clone();
    let session_tools_before = before["session_tools"].clone();

    coordinator
        .handle_runtime_command(runtime_domain::session::RuntimeCommand::Reset)
        .expect("runtime reset should succeed");
    let after = composition_snapshot(&coordinator);

    for capability in [
        "llm_port",
        "model_catalog",
        "prompt_assembly",
        "tool_catalog",
    ] {
        assert_eq!(
            capability_generation(&after, capability),
            capability_generation(&before, capability) + 1,
            "reset should replace {capability}"
        );
    }
    assert_eq!(
        capability_generation(&after, "runtime_event_stream"),
        capability_generation(&before, "runtime_event_stream")
    );
    assert_eq!(
        capability_generation(&after, "runtime_wake"),
        capability_generation(&before, "runtime_wake")
    );
    assert_eq!(component_state(&coordinator, "agent_runtime"), "active");
    assert_eq!(component_state(&coordinator, "ui_runtime_bridge"), "active");
    assert_eq!(after["workspace_tools"], workspace_tools_before);
    assert_eq!(after["session_tools"], session_tools_before);
    assert_eq!(after["effect_scopes"], before["effect_scopes"]);
    assert!(component_required(&after, "agent_runtime").contains(&"llm_port".to_string()));
    assert_eq!(
        component_required(&after, "model_refresh"),
        vec![
            "llm_port".to_string(),
            "model_catalog".to_string(),
            "runtime_event_stream".to_string(),
        ]
    );
}

fn names(value: &serde_json::Value) -> Vec<&str> {
    value
        .as_array()
        .expect("snapshot field should be an array")
        .iter()
        .map(|entry| {
            entry["id"]
                .as_str()
                .or_else(|| entry["name"].as_str())
                .expect("snapshot entry should have an id or name")
        })
        .collect()
}

fn component_state(coordinator: &AppRuntimeCoordinator, component_id: &str) -> String {
    let snapshot = composition_snapshot(coordinator);
    snapshot["components"]
        .as_array()
        .expect("components should be an array")
        .iter()
        .find(|component| component["id"] == component_id)
        .and_then(|component| component["state"].as_str())
        .expect("component should have a lifecycle state")
        .to_string()
}

fn component_required(snapshot: &serde_json::Value, component_id: &str) -> Vec<String> {
    snapshot["components"]
        .as_array()
        .expect("components should be an array")
        .iter()
        .find(|component| component["id"] == component_id)
        .and_then(|component| component["required"].as_array())
        .expect("component should expose required capabilities")
        .iter()
        .map(|key| {
            key.as_str()
                .expect("required capability should be a string")
                .to_string()
        })
        .collect()
}

fn component_provides(snapshot: &serde_json::Value, component_id: &str) -> Vec<String> {
    snapshot["components"]
        .as_array()
        .expect("components should be an array")
        .iter()
        .find(|component| component["id"] == component_id)
        .and_then(|component| component["provides"].as_array())
        .expect("component should expose provided capabilities")
        .iter()
        .map(|key| {
            key.as_str()
                .expect("provided capability should be a string")
                .to_string()
        })
        .collect()
}

fn composition_snapshot(coordinator: &AppRuntimeCoordinator) -> serde_json::Value {
    let snapshot = coordinator.inspect_composition();
    snapshot
        .validate()
        .expect("composition snapshot should be internally consistent");
    serde_json::to_value(snapshot).expect("composition snapshot should serialize")
}

fn capability_generation(snapshot: &serde_json::Value, capability_key: &str) -> u64 {
    snapshot["capabilities"]
        .as_array()
        .expect("capabilities should be an array")
        .iter()
        .find(|capability| capability["key"] == capability_key)
        .and_then(|capability| capability["generation"].as_u64())
        .expect("capability should have a generation")
}
