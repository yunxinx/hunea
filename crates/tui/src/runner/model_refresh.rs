use crate::Model;
use mo_core::model_catalog::{ModelProviderRefreshEvent, ModelSelection, ProviderSyncRequest};

use super::RuntimeCoordinator;

pub(super) fn apply_model_provider_refresh_event(
    model: &mut Model,
    event: ModelProviderRefreshEvent,
) {
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
    runtime_coordinator: &mut impl RuntimeCoordinator,
    request: ProviderSyncRequest,
) {
    if let Err(message) = runtime_coordinator.refresh_model_provider(request) {
        model.show_transient_status_notice(&message);
    }
}

pub(super) fn persist_selected_model(
    model: &mut Model,
    runtime_coordinator: &mut impl RuntimeCoordinator,
    selection: &ModelSelection,
) {
    if let Err(message) = runtime_coordinator.persist_selected_model(selection) {
        model.show_transient_status_notice(&message);
    }
}
