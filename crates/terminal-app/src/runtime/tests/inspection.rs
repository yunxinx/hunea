use std::sync::atomic::{AtomicUsize, Ordering};

use runtime_domain::{
    model_catalog::{ModelCatalog, ModelEntry, ModelProvider, ModelSelection, ModelSource},
    prompt_assembly::{PromptPreludeSection, PromptPreludeSnapshot, PromptSourceKind},
    provider::{ProviderApiKey, ProviderKind},
};
use terminal_ui::{RuntimeWake, UiRuntimePort};

use super::support::*;

const SECRET_SENTINEL: &str = "inspection-secret-sentinel";

#[test]
fn composition_snapshot_is_deterministic_and_redacted() {
    let store: Arc<dyn SessionStore> = Arc::new(InMemorySessionStore::new());
    let provider = ModelProvider::new(
        "z-provider",
        ProviderKind::OpenAiCompatible,
        "Z Provider",
        Some(format!("https://{SECRET_SENTINEL}@example.test/v1")),
        ModelSource::Configured,
        vec![
            ModelEntry::new("z-model", None, ModelSource::Configured),
            ModelEntry::new("a-model", None, ModelSource::Configured),
        ],
    )
    .with_api_key(Some(ProviderApiKey::new(SECRET_SENTINEL)))
    .with_api_key_env(Some(SECRET_SENTINEL.to_string()));
    let disabled_provider = ModelProvider::disabled(
        "a-provider",
        ProviderKind::Anthropic,
        "A Provider",
        None,
        ModelSource::Configured,
        vec![ModelEntry::new(
            "disabled-model",
            None,
            ModelSource::Configured,
        )],
    );
    let mut coordinator = runtime_coordinator(AppRuntimeOptions {
        loaded_models: conversation_runtime::models::LoadedModelCatalog {
            catalog: ModelCatalog::new(vec![provider, disabled_provider]),
            selected_model: Some(ModelSelection::new("z-provider", "z-model")),
            ..conversation_runtime::models::LoadedModelCatalog::default()
        },
        session_store: Some(store),
        initial_prompt_prelude: Some(PromptPreludeSnapshot {
            sections: vec![PromptPreludeSection {
                reference_id: "private-instruction".to_string(),
                kind: PromptSourceKind::CoreSystemPrompt,
                title: "private instruction".to_string(),
                origin: None,
                body: SECRET_SENTINEL.to_string(),
            }],
        }),
        ..AppRuntimeOptions::default()
    });

    let workspace_tool_names = coordinator
        .components
        .workspace_tools
        .definitions()
        .definitions()
        .map(|definition| definition.name.clone())
        .collect::<Vec<_>>();
    let disabled_session_tool = workspace_tool_names
        .first()
        .expect("default composition should expose workspace tools")
        .clone();
    coordinator.components.session_workspace_tools = coordinator
        .components
        .workspace_tools
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

    let snapshot: serde_json::Value =
        serde_json::from_str(&json).expect("snapshot JSON should decode");
    assert_eq!(snapshot["session_persistence"]["available"], true);
    assert_eq!(
        snapshot["selected_model"],
        serde_json::json!({"provider_id": "z-provider", "model_id": "z-model"})
    );
    assert_eq!(
        names(&snapshot["providers"]),
        vec!["a-provider", "z-provider"]
    );
    assert_eq!(
        snapshot["providers"][1]["model_ids"],
        serde_json::json!(["a-model", "z-model"])
    );

    let workspace_names = names(&snapshot["workspace_tools"]);
    let mut sorted_workspace_names = workspace_names.clone();
    sorted_workspace_names.sort();
    assert_eq!(workspace_names, sorted_workspace_names);
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
}

#[test]
fn ui_runtime_bridge_reacts_to_wake_binding_lifecycle() {
    let mut coordinator = runtime_coordinator(AppRuntimeOptions {
        session_store: Some(Arc::new(InMemorySessionStore::new())),
        ..AppRuntimeOptions::default()
    });
    assert_eq!(
        component_state(&coordinator, "ui_runtime_bridge"),
        "pending"
    );

    let wake_count = Arc::new(AtomicUsize::new(0));
    let wake_count_for_callback = Arc::clone(&wake_count);
    UiRuntimePort::bind_runtime_wake(
        &mut coordinator,
        RuntimeWake::new(move || {
            wake_count_for_callback.fetch_add(1, Ordering::SeqCst);
        }),
    )
    .expect("runtime wake should bind");

    assert_eq!(component_state(&coordinator, "ui_runtime_bridge"), "active");
    coordinator.components.runtime_event_notifier.notify();
    assert_eq!(wake_count.load(Ordering::SeqCst), 1);

    coordinator.shutdown().expect("runtime should shut down");
    assert_eq!(
        component_state(&coordinator, "ui_runtime_bridge"),
        "pending"
    );
    coordinator.components.runtime_event_notifier.notify();
    assert_eq!(
        wake_count.load(Ordering::SeqCst),
        1,
        "disposing the runtime owner must remove the old wake callback"
    );
    let snapshot = composition_snapshot(&coordinator);
    assert_eq!(snapshot["session_persistence"]["available"], false);
}

#[test]
fn rebinding_ui_runtime_bridge_disposes_the_previous_wake_effect() {
    let mut coordinator = runtime_coordinator(AppRuntimeOptions::default());
    let first_wake_count = Arc::new(AtomicUsize::new(0));
    let first_wake_count_for_callback = Arc::clone(&first_wake_count);
    UiRuntimePort::bind_runtime_wake(
        &mut coordinator,
        RuntimeWake::new(move || {
            first_wake_count_for_callback.fetch_add(1, Ordering::SeqCst);
        }),
    )
    .expect("first runtime wake should bind");

    let second_wake_count = Arc::new(AtomicUsize::new(0));
    let second_wake_count_for_callback = Arc::clone(&second_wake_count);
    UiRuntimePort::bind_runtime_wake(
        &mut coordinator,
        RuntimeWake::new(move || {
            second_wake_count_for_callback.fetch_add(1, Ordering::SeqCst);
        }),
    )
    .expect("replacement runtime wake should bind");

    coordinator.components.runtime_event_notifier.notify();
    assert_eq!(first_wake_count.load(Ordering::SeqCst), 0);
    assert_eq!(second_wake_count.load(Ordering::SeqCst), 1);
    assert_eq!(component_state(&coordinator, "ui_runtime_bridge"), "active");
}

#[test]
fn reset_replaces_session_component_generations_without_rebuilding_the_ui_bridge() {
    let mut coordinator = runtime_coordinator(AppRuntimeOptions::default());
    UiRuntimePort::bind_runtime_wake(&mut coordinator, RuntimeWake::new(|| {}))
        .expect("runtime wake should bind");
    let before = composition_snapshot(&coordinator);

    coordinator
        .handle_runtime_command(runtime_domain::session::RuntimeCommand::Reset)
        .expect("runtime reset should succeed");
    let after = composition_snapshot(&coordinator);

    for capability in [
        "conversation_worker",
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
    assert_eq!(
        component_state(&coordinator, "native_agent_runtime"),
        "active"
    );
    assert_eq!(component_state(&coordinator, "ui_runtime_bridge"), "active");
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

fn composition_snapshot(coordinator: &AppRuntimeCoordinator) -> serde_json::Value {
    serde_json::to_value(coordinator.inspect_composition())
        .expect("composition snapshot should serialize")
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
