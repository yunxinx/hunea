use super::*;
use crate::{ModelOptions, StartupBannerOptions, tool_approval_panel::ToolApprovalSource};
use runtime_domain::model_catalog::{ModelCatalog, ModelProvider, ModelSource};
use runtime_domain::provider::ProviderKind;

#[test]
fn provider_refresh_success_replaces_models_and_drops_stale_selection() {
    let mut model = model_with_single_provider();
    model.open_model_panel();

    model.apply_model_provider_refresh_success(
        "local",
        vec!["fresh-a".to_string(), "fresh-b".to_string()],
    );

    let provider = model
        .model_catalog
        .enabled_provider_by_id("local")
        .expect("refreshed provider should remain enabled");
    assert_eq!(provider.source, ModelSource::Synced);
    assert_eq!(provider.sync_error, None);
    assert_eq!(
        provider
            .models
            .iter()
            .map(|entry| entry.id.as_str())
            .collect::<Vec<_>>(),
        vec!["fresh-a", "fresh-b"]
    );
    assert_eq!(model.selected_model, None);
    assert_eq!(model.model_panel.model_index, 0);
    assert_eq!(model.model_panel.scroll, 0);
}

#[test]
fn open_model_panel_closes_tool_approval_panel_and_resumes_stream_activity() {
    let mut model = model_with_single_provider();
    model.show_stream_activity_with_header("Working");
    model.open_tool_approval_panel(
        ToolApprovalSource::RuntimePermission {
            target: runtime_domain::session::RuntimeTarget::provider("local", "qwen3"),
            request_id: "permission-1".to_string(),
            allow_option_id: Some("allow-once".to_string()),
            allow_always_option_id: None,
            reject_option_id: Some("reject-once".to_string()),
            reject_always_option_id: None,
        },
        "Write file".to_string(),
        Vec::new(),
    );
    assert!(!model.current_stream_activity_render_result().has_content);

    model.open_model_panel();

    assert!(model.current_stream_activity_render_result().has_content);
}

#[test]
fn provider_refresh_failure_keeps_existing_models_and_records_error() {
    let mut model = model_with_single_provider();
    model.open_model_panel();

    model.apply_model_provider_refresh_failure("local", "connection refused");

    let provider = model
        .model_catalog
        .enabled_provider_by_id("local")
        .expect("provider should remain visible after failed refresh");
    assert_eq!(provider.source, ModelSource::Configured);
    assert_eq!(provider.sync_error.as_deref(), Some("connection refused"));
    assert_eq!(
        provider
            .models
            .iter()
            .map(|entry| entry.id.as_str())
            .collect::<Vec<_>>(),
        vec!["qwen3"]
    );
    assert_eq!(
        model.selected_model,
        Some(ModelSelection::new("local", "qwen3"))
    );
}

fn model_with_single_provider() -> Model {
    Model::new_with_options(
        StartupBannerOptions::default(),
        ModelOptions {
            model_catalog: ModelCatalog::new(vec![ModelProvider::new(
                "local",
                ProviderKind::OpenAiCompatible,
                "Local",
                Some("http://127.0.0.1:1234/v1".to_string()),
                ModelSource::Configured,
                vec![ModelEntry::new("qwen3", None, ModelSource::Configured)],
            )]),
            selected_model: Some(ModelSelection::new("local", "qwen3")),
            ..ModelOptions::default()
        },
    )
}
