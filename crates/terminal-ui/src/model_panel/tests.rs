use super::*;
use crate::{
    ModelOptions, StartupBannerOptions, text_search::CaseInsensitiveQuery, theme::default_palette,
    tool_approval_panel::ToolApprovalSource,
};
use ratatui::style::Modifier;
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
    assert_eq!(model.current_status_notice_text(), "");
    assert_eq!(
        model.active_toast_text_for_test(),
        Some("Models refreshed: Local")
    );
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
fn opening_tool_approval_panel_clears_open_model_panel_state() {
    let mut model = model_with_single_provider();
    model.open_model_panel();
    model.push_model_panel_search_character('q');
    assert!(model.model_panel.is_open);
    assert_eq!(model.model_panel.search_query, "q");
    assert!(!model.model_panel.filtered_model_indices.is_empty());

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

    assert!(!model.model_panel.is_open);
    assert_eq!(model.model_panel.search_query, "");
    assert!(model.model_panel.filtered_model_indices.is_empty());
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
    assert_eq!(model.current_status_notice_text(), "");
    assert_eq!(
        model.active_toast_text_for_test(),
        Some("Failed to refresh models for Local: connection refused")
    );
}

#[test]
fn model_panel_selection_uses_toast_not_status_notice() {
    let mut model = model_with_single_provider();
    model.selected_model = None;
    model.open_model_panel();

    let effect = model.handle_model_panel_key(KeyEvent::from(KeyCode::Enter));

    assert_eq!(
        effect.into_effect(),
        Some(AppEffect::PersistSelectedModel {
            selection: ModelSelection::new("local", "qwen3")
        })
    );
    assert_eq!(model.current_status_notice_text(), "");
    assert_eq!(
        model.active_toast_text_for_test(),
        Some("Model selected: [Local] qwen3")
    );
}

#[test]
fn model_entry_search_matches_ascii_without_case_sensitivity() {
    let entry = ModelEntry::new(
        "DeepSeek-Reasoner",
        Some("General Chat Model".to_string()),
        ModelSource::Configured,
    );

    let deepseek = CaseInsensitiveQuery::new("deepseek");
    let chat = CaseInsensitiveQuery::new("chat");
    let qwen = CaseInsensitiveQuery::new("qwen");
    assert!(model_entry_matches_search(&entry, &deepseek));
    assert!(model_entry_matches_search(&entry, &chat));
    assert!(!model_entry_matches_search(&entry, &qwen));
}

#[test]
fn model_entry_search_keeps_unicode_case_insensitive_matching() {
    let entry = ModelEntry::new(
        "Qwen",
        Some("İstanbul capable model".to_string()),
        ModelSource::Configured,
    );

    let istanbul = CaseInsensitiveQuery::new("i\u{307}stanbul");
    assert!(model_entry_matches_search(&entry, &istanbul));
}

#[test]
fn model_panel_highlights_matched_description_text() {
    let mut model = Model::new_with_options(
        StartupBannerOptions::default(),
        ModelOptions {
            model_catalog: ModelCatalog::new(vec![ModelProvider::new(
                "local",
                ProviderKind::OpenAiCompatible,
                "Local",
                Some("http://127.0.0.1:1234/v1".to_string()),
                ModelSource::Configured,
                vec![ModelEntry::new(
                    "qwen3",
                    Some("General Chat Model".to_string()),
                    ModelSource::Configured,
                )],
            )]),
            ..ModelOptions::default()
        },
    );
    model.set_palette(default_palette(), true);
    model.set_window(80, 20);
    model.open_model_panel();
    for character in "chat".chars() {
        model.push_model_panel_search_character(character);
    }

    let rendered = model.current_inline_model_panel_render_result();
    let line_index = rendered
        .plain_lines
        .iter()
        .position(|line| line.contains("General Chat Model"))
        .expect("matching model description row should render");
    let highlighted_span = rendered.lines[line_index]
        .spans
        .iter()
        .find(|span| span.content.eq_ignore_ascii_case("chat"))
        .expect("matched model description span should render separately");

    assert!(
        highlighted_span.style.bg == default_palette().surface
            || highlighted_span
                .style
                .add_modifier
                .contains(Modifier::REVERSED),
        "matched model description should use background-like highlight: {:?}",
        highlighted_span.style
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
