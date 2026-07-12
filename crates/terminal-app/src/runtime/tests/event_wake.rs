use std::{sync::mpsc, time::Duration};

use runtime_domain::{
    model_catalog::ProviderSyncRequest,
    provider::ProviderKind,
    session::{RuntimeCommand, RuntimeEvent, RuntimePermissionRequest, RuntimeTarget},
};

use super::support::*;

#[test]
fn model_refresh_completion_notifies_the_coordinator_consumer() {
    let mut coordinator = runtime_coordinator(AppRuntimeOptions::default());
    let (wake_sender, wake_receiver) = mpsc::channel();
    coordinator
        .runtime_event_notifier
        .replace_callback(move || {
            let _ = wake_sender.send(());
        });

    RuntimeCoordinator::refresh_model_provider(
        &mut coordinator,
        ProviderSyncRequest {
            provider_id: "anthropic".to_string(),
            kind: ProviderKind::Anthropic,
            display_name: "Anthropic".to_string(),
            base_url: None,
            api_key: None,
            api_key_env: None,
        },
    )
    .expect("model refresh should start");

    wake_receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("refresh completion should wake the coordinator consumer");
    assert_eq!(
        RuntimeCoordinator::drain_model_provider_refresh_events(&mut coordinator).len(),
        1
    );
}

#[test]
fn render_barrier_deferral_rearms_the_coordinator_consumer() {
    let mut coordinator = runtime_coordinator(AppRuntimeOptions::default());
    let (wake_sender, wake_receiver) = mpsc::channel();
    coordinator
        .runtime_event_notifier
        .replace_callback(move || {
            let _ = wake_sender.send(());
        });
    let deferred = RuntimeEvent::PermissionRequested {
        target: RuntimeTarget::provider("local", "qwen3"),
        request: RuntimePermissionRequest::new("permission-1", None, Vec::new()),
    };

    coordinator.defer_runtime_event_until_next_render(deferred.clone());

    wake_receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("deferred runtime event should rearm the loop after the render barrier");
    assert_eq!(coordinator.pending_runtime_events, vec![deferred]);
}

#[test]
fn deferred_event_does_not_skip_ready_worker_payloads() {
    let mut coordinator = runtime_coordinator(AppRuntimeOptions {
        loaded_models: conversation_runtime::models::LoadedModelCatalog {
            catalog: runtime_domain::model_catalog::ModelCatalog::new(vec![
                runtime_domain::model_catalog::ModelProvider::new(
                    "local",
                    ProviderKind::OpenAiCompatible,
                    "Local",
                    Some("http://127.0.0.1:1234/v1".to_string()),
                    runtime_domain::model_catalog::ModelSource::Configured,
                    vec![runtime_domain::model_catalog::ModelEntry::new(
                        "qwen3",
                        None,
                        runtime_domain::model_catalog::ModelSource::Configured,
                    )],
                ),
            ]),
            ..conversation_runtime::models::LoadedModelCatalog::default()
        },
        ..AppRuntimeOptions::default()
    });
    let (wake_sender, wake_receiver) = mpsc::channel();
    coordinator
        .runtime_event_notifier
        .replace_callback(move || {
            let _ = wake_sender.send(());
        });
    let context_request_id = request_id(91);

    RuntimeCoordinator::dispatch_runtime_command(
        &mut coordinator,
        RuntimeCommand::LoadContextBudgetSnapshot {
            request_id: context_request_id,
            selection: ModelSelection::new("local", "qwen3"),
        },
    )
    .expect("context budget request should start");
    wake_receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("ready context budget payload should notify the consumer");

    let deferred = RuntimeEvent::PermissionRequested {
        target: RuntimeTarget::provider("local", "qwen3"),
        request: RuntimePermissionRequest::new("permission-2", None, Vec::new()),
    };
    coordinator.defer_runtime_event_until_next_render(deferred.clone());

    let events = RuntimeCoordinator::drain_runtime_events(&mut coordinator);

    assert_eq!(events.first(), Some(&deferred));
    assert!(events.iter().any(|event| matches!(
        event,
        RuntimeEvent::ContextBudgetSnapshotLoaded { request_id, .. }
            if *request_id == context_request_id
    )));
}
