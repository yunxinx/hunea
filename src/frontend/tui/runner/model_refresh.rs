use crate::frontend::tui::Model;
use crate::runtime::native::models as native_models;
use crate::runtime::native::models::ProviderSyncRequest;
use crate::runtime::native::{ModelProviderRefreshEvent, ModelProviderRefreshRuntimeState};

pub(super) fn drain_model_refresh_runtime_events(
    model: &mut Model,
    model_refresh_runtime: &mut ModelProviderRefreshRuntimeState,
) -> bool {
    let mut changed = false;
    while let Some(event) = model_refresh_runtime.try_recv_event() {
        apply_model_provider_refresh_event(model, event);
        changed = true;
    }
    changed
}

fn apply_model_provider_refresh_event(model: &mut Model, event: ModelProviderRefreshEvent) {
    match event {
        ModelProviderRefreshEvent::Finished {
            provider_id,
            model_ids,
        } => model.apply_model_provider_refresh_success(&provider_id, model_ids),
        ModelProviderRefreshEvent::Failed {
            provider_id,
            message,
        } => model.apply_model_provider_refresh_failure(&provider_id, message),
    }
}

pub(super) fn run_refresh_model_provider_effect(
    model: &mut Model,
    model_refresh_runtime: &mut ModelProviderRefreshRuntimeState,
    request: ProviderSyncRequest,
) {
    if model_refresh_runtime.is_running() {
        model.show_transient_status_notice("Model refresh is already running");
        return;
    }

    model_refresh_runtime.start(request);
}

pub(super) fn persist_selected_model(
    model: &mut Model,
    model_config_path: Option<&std::path::Path>,
    selection: &crate::runtime::model_catalog::ModelSelection,
) {
    if let Err(error) = native_models::write_default_model(model_config_path, selection) {
        model.show_transient_status_notice(&format!("Failed to save default model: {error}"));
    }
}
