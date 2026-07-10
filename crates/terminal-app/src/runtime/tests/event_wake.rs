use std::{sync::mpsc, time::Duration};

use runtime_domain::{
    model_catalog::ProviderSyncRequest,
    provider::ProviderKind,
    session::{RuntimeEvent, RuntimePermissionRequest, RuntimeTarget},
};

use super::support::*;

#[test]
fn model_refresh_completion_notifies_the_coordinator_consumer() {
    let mut coordinator = runtime_coordinator(AppRuntimeOptions::default());
    let (wake_sender, wake_receiver) = mpsc::channel();
    coordinator
        .runtime_event_notifier
        .install(move || {
            let _ = wake_sender.send(());
        })
        .expect("test notifier should install once");

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
        .install(move || {
            let _ = wake_sender.send(());
        })
        .expect("test notifier should install once");
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
