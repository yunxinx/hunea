use std::fs;

use lumos::runtime::{
    model_catalog::{ModelSelection, ModelSource},
    native::{
        ProviderApiKey, ProviderKind,
        models::{ProviderSyncRequest, load_from_paths_with_sync, write_default_model},
    },
};

#[test]
fn models_config_syncs_provider_models_when_allowlist_is_omitted() {
    let working_dir = temp_test_dir("sync-provider-models");
    fs::write(
        working_dir.join("models.toml"),
        r#"
default = ""

[providers.local]
enabled = true
kind = "openai_compatible"
display_name = "LM Studio"
base_url = "http://192.168.1.71:1234/v1"
"#,
    )
    .expect("models config should be written");

    let loaded = load_from_paths_with_sync(Some(&working_dir), None, |request| {
        assert_eq!(
            request,
            &ProviderSyncRequest {
                provider_id: "local".to_string(),
                kind: ProviderKind::OpenAiCompatible,
                display_name: "LM Studio".to_string(),
                base_url: Some("http://192.168.1.71:1234/v1".to_string()),
                api_key: None,
                api_key_env: None,
            }
        );
        Ok(vec!["qwen3".to_string(), "deepseek-chat".to_string()])
    })
    .expect("models config should load");

    assert!(loaded.requires_model_selection);
    assert_eq!(loaded.selected_model, None);

    let provider = loaded
        .catalog
        .enabled_provider_at(0)
        .expect("enabled provider should be visible");
    assert_eq!(provider.id, "local");
    assert_eq!(provider.display_name, "LM Studio");
    assert_eq!(provider.source, ModelSource::Synced);
    assert_eq!(
        provider
            .models
            .iter()
            .map(|model| model.id.as_str())
            .collect::<Vec<_>>(),
        vec!["qwen3", "deepseek-chat"]
    );
}

#[test]
fn models_config_accepts_direct_provider_api_key() {
    let working_dir = temp_test_dir("direct-provider-api-key");
    fs::write(
        working_dir.join("models.toml"),
        r#"
default = "remote/qwen3"

[providers.remote]
enabled = true
kind = "openai_compatible"
display_name = "Remote"
base_url = "https://api.example.com/v1"
api_key = "sk-test-direct"
"#,
    )
    .expect("models config should be written");

    let loaded = load_from_paths_with_sync(Some(&working_dir), None, |request| {
        assert_eq!(
            request,
            &ProviderSyncRequest {
                provider_id: "remote".to_string(),
                kind: ProviderKind::OpenAiCompatible,
                display_name: "Remote".to_string(),
                base_url: Some("https://api.example.com/v1".to_string()),
                api_key: Some(ProviderApiKey::new("sk-test-direct")),
                api_key_env: None,
            }
        );
        Ok(vec!["qwen3".to_string()])
    })
    .expect("models config should load");

    let provider = loaded
        .catalog
        .enabled_provider_by_id("remote")
        .expect("enabled provider should be visible");
    let native_runtime = provider
        .native_runtime()
        .expect("remote provider should be native");
    assert_eq!(
        native_runtime.api_key.as_ref().map(ProviderApiKey::as_str),
        Some("sk-test-direct")
    );
    assert_eq!(native_runtime.api_key_env, None);
}

#[test]
fn models_config_accepts_native_genai_provider_kinds() {
    let working_dir = temp_test_dir("native-genai-provider-kinds");
    fs::write(
        working_dir.join("models.toml"),
        r#"
default = "anthropic/claude-sonnet-4-5"

[providers.openai]
enabled = true
kind = "openai"
display_name = "OpenAI"
api_key_env = "OPENAI_API_KEY"
models = ["gpt-4.1"]

[providers.anthropic]
enabled = true
kind = "anthropic"
display_name = "Anthropic"
api_key_env = "ANTHROPIC_API_KEY"
models = ["claude-sonnet-4-5"]

[providers.gemini]
enabled = true
kind = "gemini"
display_name = "Gemini"
api_key_env = "GEMINI_API_KEY"
models = ["gemini-2.5-pro"]
"#,
    )
    .expect("models config should be written");

    let loaded = load_from_paths_with_sync(Some(&working_dir), None, |_| {
        panic!("configured model allowlists should not sync")
    })
    .expect("native provider kinds should load");

    assert_eq!(
        loaded.selected_model,
        Some(ModelSelection::new("anthropic", "claude-sonnet-4-5"))
    );
    let kinds = loaded
        .catalog
        .enabled_providers()
        .map(|provider| {
            (
                provider.id.as_str(),
                provider
                    .native_runtime()
                    .expect("configured provider should be native")
                    .kind,
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        kinds,
        vec![
            ("anthropic", ProviderKind::Anthropic),
            ("gemini", ProviderKind::Gemini),
            ("openai", ProviderKind::OpenAi),
        ]
    );
}

#[test]
fn models_config_keeps_sync_error_when_provider_models_fail_to_sync() {
    let working_dir = temp_test_dir("sync-provider-models-error");
    fs::write(
        working_dir.join("models.toml"),
        r#"
default = ""

[providers.local]
enabled = true
kind = "openai_compatible"
display_name = "LM Studio"
base_url = "http://192.168.1.71:1234/v1"
"#,
    )
    .expect("models config should be written");

    let loaded = load_from_paths_with_sync(Some(&working_dir), None, |_| {
        Err("connection refused".to_string())
    })
    .expect("models config should load even when model sync fails");

    let provider = loaded
        .catalog
        .enabled_provider_at(0)
        .expect("enabled provider should be visible");
    assert_eq!(provider.models.len(), 0);
    assert_eq!(provider.sync_error.as_deref(), Some("connection refused"));
}

#[test]
fn models_config_uses_explicit_default_selection_when_present() {
    let working_dir = temp_test_dir("default-model-selection");
    fs::write(
        working_dir.join("models.toml"),
        r#"
default = "local/qwen3"

[providers.local]
enabled = true
kind = "openai_compatible"
display_name = "Local"
base_url = "http://127.0.0.1:1234/v1"
api_key_env = "DEEPSEEK_API_KEY"
models = ["qwen3"]
"#,
    )
    .expect("models config should be written");

    let loaded = load_from_paths_with_sync(Some(&working_dir), None, |_| {
        panic!("configured model allowlist should not sync /models")
    })
    .expect("models config should load");

    assert_eq!(
        loaded.selected_model,
        Some(ModelSelection::new("local", "qwen3"))
    );

    let provider = loaded
        .catalog
        .enabled_provider_at(0)
        .expect("enabled provider should be visible");
    assert_eq!(
        provider
            .native_runtime()
            .expect("local provider should be native")
            .api_key_env
            .as_deref(),
        Some("DEEPSEEK_API_KEY")
    );
}

#[test]
fn write_default_model_persists_last_selected_model() {
    let working_dir = temp_test_dir("persist-default-model");
    let config_path = working_dir.join("models.toml");
    fs::write(
        &config_path,
        r#"
default = ""

[providers.local]
enabled = true
kind = "openai_compatible"
display_name = "Local"
base_url = "http://127.0.0.1:1234/v1"
models = ["qwen3"]
"#,
    )
    .expect("models config should be written");

    write_default_model(Some(&config_path), &ModelSelection::new("local", "qwen3"))
        .expect("selected model should be persisted");

    let saved = fs::read_to_string(config_path).expect("models config should be readable");
    assert!(
        saved.contains("default = \"local/qwen3\""),
        "default should track the last selected model, got:\n{saved}"
    );
}

fn temp_test_dir(name: &str) -> std::path::PathBuf {
    let unique = format!(
        "{}-{}",
        name,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock should be after epoch")
            .as_nanos()
    );
    let path = std::env::temp_dir()
        .join("lumos-runtime-models-config-tests")
        .join(unique);
    fs::create_dir_all(&path).expect("temp dir should be created");
    path
}
