#[cfg(test)]
fn model_list_plain_lines(plain_lines: &[String]) -> Vec<&str> {
    let start = plain_lines
        .iter()
        .position(|line| line.contains("Available Models:"))
        .map(|index| index + 1)
        .unwrap_or(plain_lines.len());

    plain_lines[start..]
        .iter()
        .take_while(|line| !line.trim_start().starts_with("Enter select"))
        .map(String::as_str)
        .filter(|line| !line.trim().is_empty())
        .collect()
}

#[cfg(test)]
fn model_row_label(line: &str) -> &str {
    let line = line.trim_start();
    line.strip_prefix("➜ ").unwrap_or(line).trim()
}

use super::*;
use crate::{AppEvent, HeroOptions, ModelOptions};
use mo_core::model_catalog::{ModelCatalog, ModelProvider, ModelSource};
use mo_core::provider::ProviderKind;
use mo_core::session::{RuntimeModelConfig, RuntimeModelOption};

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
    model.update(AppEvent::AcpPermissionRequested {
        request_id: "permission-1".to_string(),
        title: Some("Write file".to_string()),
        allow_option_id: Some("allow-once".to_string()),
        allow_always_option_id: None,
        reject_option_id: Some("reject-once".to_string()),
        reject_always_option_id: None,
    });
    assert!(!model.current_stream_activity_render_result().has_content);

    model.open_model_panel();

    assert!(model.current_stream_activity_render_result().has_content);
}

#[test]
fn acp_model_panel_selects_agent_model_without_persisting_native_default() {
    let mut model = Model::new_with_options(
        HeroOptions::default(),
        ModelOptions {
            model_catalog: ModelCatalog::new(vec![ModelProvider::native(
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
    );
    model.selected_acp_agent = Some("Kimi Code CLI".to_string());
    model.apply_acp_model_config(
        "Kimi Code CLI",
        RuntimeModelConfig {
            config_id: Some("model".to_string()),
            current_value: "kimi-k2".to_string(),
            current_name: "Kimi K2".to_string(),
            options: vec![
                RuntimeModelOption {
                    value: "kimi-k2".to_string(),
                    name: "Kimi K2".to_string(),
                },
                RuntimeModelOption {
                    value: "kimi-k1.5".to_string(),
                    name: "Kimi K1.5".to_string(),
                },
            ],
        },
    );
    assert!(
        model
            .model_catalog
            .enabled_provider_by_id("acp:Kimi Code CLI")
            .expect("ACP model provider should be visible")
            .native_runtime()
            .is_none()
    );
    model.open_model_panel();
    model.move_model_panel_model(1);

    let effect = model.select_current_model_panel_model();

    assert_eq!(
        effect,
        Some(AppEffect::SetAcpModel {
            config_id: Some("model".to_string()),
            value: "kimi-k1.5".to_string(),
        })
    );
    assert_eq!(
        model.selected_model,
        Some(ModelSelection::new("acp:Kimi Code CLI", "kimi-k1.5"))
    );
    assert_eq!(model.acp_current_model.as_deref(), Some("Kimi K1.5"));
}

#[test]
fn acp_model_panel_selects_legacy_agent_model_without_config_id() {
    let mut model = Model::new_with_options(
        HeroOptions::default(),
        ModelOptions {
            model_catalog: ModelCatalog::new(vec![ModelProvider::native(
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
    );
    model.selected_acp_agent = Some("Kimi Code CLI".to_string());
    model.apply_acp_model_config(
        "Kimi Code CLI",
        RuntimeModelConfig {
            config_id: None,
            current_value: "kimi-for-coding".to_string(),
            current_name: "Kimi for Coding".to_string(),
            options: vec![
                RuntimeModelOption {
                    value: "kimi-for-coding".to_string(),
                    name: "Kimi for Coding".to_string(),
                },
                RuntimeModelOption {
                    value: "kimi-for-coding(thinking)".to_string(),
                    name: "Kimi for Coding (thinking)".to_string(),
                },
            ],
        },
    );
    model.open_model_panel();
    model.move_model_panel_model(1);

    let effect = model.select_current_model_panel_model();

    assert_eq!(
        effect,
        Some(AppEffect::SetAcpModel {
            config_id: None,
            value: "kimi-for-coding(thinking)".to_string(),
        })
    );
    assert_eq!(
        model.selected_model,
        Some(ModelSelection::new(
            "acp:Kimi Code CLI",
            "kimi-for-coding(thinking)"
        ))
    );
    assert_eq!(
        model.acp_current_model.as_deref(),
        Some("Kimi for Coding (thinking)")
    );
}

#[test]
fn acp_model_panel_prefers_model_name_and_falls_back_to_id() {
    let mut model = Model::new_with_options(
        HeroOptions::default(),
        ModelOptions {
            model_catalog: ModelCatalog::new(vec![ModelProvider::native(
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
    );
    model.selected_acp_agent = Some("Kimi Code CLI".to_string());
    model.apply_acp_model_config(
        "Kimi Code CLI",
        RuntimeModelConfig {
            config_id: Some("models".to_string()),
            current_value: "kimi-code/kimi-for-coding".to_string(),
            current_name: "kimi-for-coding".to_string(),
            options: vec![
                RuntimeModelOption {
                    value: "kimi-code/kimi-for-coding".to_string(),
                    name: "Kimi for Coding".to_string(),
                },
                RuntimeModelOption {
                    value: "kimi-code/kimi-for-coding,thinking".to_string(),
                    name: String::new(),
                },
            ],
        },
    );
    model.set_window(80, 24);
    model.open_model_panel();

    let render = model.current_inline_model_panel_render_result();
    let plain_lines = render.plain_lines;
    let model_lines = model_list_plain_lines(&plain_lines);

    assert_eq!(
        model_lines
            .iter()
            .filter(|line| line.contains("Kimi for Coding"))
            .count(),
        1
    );
    assert!(
        model_lines
            .iter()
            .any(|line| model_row_label(line) == "kimi-code/kimi-for-coding,thinking")
    );
    assert!(
        !model_lines
            .iter()
            .any(|line| model_row_label(line) == "kimi-code/kimi-for-coding")
    );

    model.move_model_panel_model(1);
    let effect = model.select_current_model_panel_model();

    assert_eq!(
        effect,
        Some(AppEffect::SetAcpModel {
            config_id: Some("models".to_string()),
            value: "kimi-code/kimi-for-coding,thinking".to_string(),
        })
    );
    assert_eq!(
        model.acp_current_model.as_deref(),
        Some("kimi-code/kimi-for-coding,thinking")
    );
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
        HeroOptions::default(),
        ModelOptions {
            model_catalog: ModelCatalog::new(vec![ModelProvider::native(
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
