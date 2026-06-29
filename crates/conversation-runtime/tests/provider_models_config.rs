use std::fs;

use conversation_runtime::{
    ProviderApiKey, ProviderKind,
    models::{load_from_paths, write_default_model},
};
use runtime_domain::model_catalog::{ModelSelection, ModelSource};

#[test]
fn models_config_keeps_provider_local_when_allowlist_is_omitted() {
    let working_dir = temp_test_dir("local-provider-models");
    fs::write(
        working_dir.join("models.toml"),
        r#"
default = "local/qwen3"

[providers.local]
enabled = true
kind = "openai_compatible"
display_name = "LM Studio"
base_url = "http://192.168.1.71:1234/v1"
"#,
    )
    .expect("models config should be written");

    let loaded = load_from_paths(Some(&working_dir), None).expect("models config should load");

    assert!(loaded.requires_model_selection);
    assert_eq!(
        loaded.selected_model,
        Some(ModelSelection::new("local", "qwen3"))
    );

    let provider = loaded
        .catalog
        .enabled_provider_at(0)
        .expect("enabled provider should be visible");
    assert_eq!(provider.id, "local");
    assert_eq!(provider.display_name, "LM Studio");
    assert_eq!(provider.source, ModelSource::NotLoaded);
    assert!(provider.models.is_empty());
    assert_eq!(provider.sync_error, None);
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

    let loaded = load_from_paths(Some(&working_dir), None).expect("models config should load");

    let provider = loaded
        .catalog
        .enabled_provider_by_id("remote")
        .expect("enabled provider should be visible");
    let connection = provider.connection();
    assert_eq!(
        connection.api_key.as_ref().map(ProviderApiKey::as_str),
        Some("sk-test-direct")
    );
    assert_eq!(connection.api_key_env, None);
}

#[test]
fn models_config_accepts_provider_kinds() {
    let working_dir = temp_test_dir("provider-kinds");
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

    let loaded = load_from_paths(Some(&working_dir), None).expect("provider kinds should load");

    assert_eq!(
        loaded.selected_model,
        Some(ModelSelection::new("anthropic", "claude-sonnet-4-5"))
    );
    let kinds = loaded
        .catalog
        .enabled_providers()
        .map(|provider| (provider.id.as_str(), provider.connection().kind))
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
fn models_config_does_not_record_sync_error_during_startup_load() {
    let working_dir = temp_test_dir("startup-load-without-sync-error");
    fs::write(
        working_dir.join("models.toml"),
        r#"
default = "local/qwen3"

[providers.local]
enabled = true
kind = "openai_compatible"
display_name = "LM Studio"
base_url = "http://192.168.1.71:1234/v1"
"#,
    )
    .expect("models config should be written");

    let loaded = load_from_paths(Some(&working_dir), None)
        .expect("models config should load without syncing provider models");

    let provider = loaded
        .catalog
        .enabled_provider_at(0)
        .expect("enabled provider should be visible");
    assert_eq!(provider.models.len(), 0);
    assert_eq!(provider.sync_error, None);
    assert_eq!(
        loaded.selected_model,
        Some(ModelSelection::new("local", "qwen3"))
    );
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

    let loaded = load_from_paths(Some(&working_dir), None).expect("models config should load");

    assert_eq!(
        loaded.selected_model,
        Some(ModelSelection::new("local", "qwen3"))
    );

    let provider = loaded
        .catalog
        .enabled_provider_at(0)
        .expect("enabled provider should be visible");
    assert_eq!(
        provider.connection().api_key_env.as_deref(),
        Some("DEEPSEEK_API_KEY")
    );
}

#[test]
fn models_config_trusts_default_model_outside_configured_allowlist() {
    let working_dir = temp_test_dir("default-model-outside-allowlist");
    fs::write(
        working_dir.join("models.toml"),
        r#"
default = "local/qwen4"

[providers.local]
enabled = true
kind = "openai_compatible"
display_name = "Local"
base_url = "http://127.0.0.1:1234/v1"
models = ["qwen3"]
"#,
    )
    .expect("models config should be written");

    let loaded = load_from_paths(Some(&working_dir), None).expect("models config should load");

    assert_eq!(
        loaded.selected_model,
        Some(ModelSelection::new("local", "qwen4"))
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

#[test]
fn models_config_resolves_default_context_window() {
    let working_dir = temp_test_dir("default-context-window");
    fs::write(
        working_dir.join("models.toml"),
        r#"
default = "local/qwen3"

[defaults]
context_window = 128000

[providers.local]
enabled = true
kind = "openai_compatible"
base_url = "http://127.0.0.1:1234/v1"
models = ["qwen3"]
"#,
    )
    .expect("models config should be written");

    let loaded = load_from_paths(Some(&working_dir), None).expect("models config should load");
    let selection = ModelSelection::new("local", "qwen3");

    assert_eq!(loaded.context_limit_for(&selection), 128_000);
}

#[test]
fn models_config_resolves_per_provider_model_profile() {
    let working_dir = temp_test_dir("provider-model-profile");
    fs::write(
        working_dir.join("models.toml"),
        r#"
default = "local/qwen3"

[defaults]
context_window = 128000

[providers.local]
enabled = true
kind = "openai_compatible"
base_url = "http://127.0.0.1:1234/v1"
models = ["qwen3"]

[providers.local.model_profiles.qwen3]
context_window = 32768
"#,
    )
    .expect("models config should be written");

    let loaded = load_from_paths(Some(&working_dir), None).expect("models config should load");
    let selection = ModelSelection::new("local", "qwen3");

    assert_eq!(loaded.context_limit_for(&selection), 32_768);
}

#[test]
fn models_config_project_overlay_overrides_user_defaults_context_window() {
    let user_dir = temp_test_dir("user-config-dir");
    let working_dir = temp_test_dir("project-overlay-context");
    fs::write(
        user_dir.join("models.toml"),
        r#"
[defaults]
context_window = 64000

[providers.local]
enabled = true
kind = "openai_compatible"
base_url = "http://127.0.0.1:1234/v1"
models = ["qwen3"]
"#,
    )
    .expect("user models config should be written");
    fs::create_dir_all(working_dir.join(".hunea")).expect("project config dir");
    fs::write(
        working_dir.join(".hunea").join("models.toml"),
        r#"
[defaults]
context_window = 200000
"#,
    )
    .expect("project models config should be written");

    let loaded = load_from_paths(Some(&working_dir), Some(&user_dir))
        .expect("merged models config should load");
    let selection = ModelSelection::new("local", "qwen3");

    assert_eq!(loaded.context_limit_for(&selection), 200_000);
}

#[test]
fn models_config_rejects_invalid_context_window() {
    let working_dir = temp_test_dir("invalid-context-window");
    fs::write(
        working_dir.join("models.toml"),
        r#"
[defaults]
context_window = 0

[providers.local]
enabled = true
kind = "openai_compatible"
base_url = "http://127.0.0.1:1234/v1"
"#,
    )
    .expect("models config should be written");

    let error = load_from_paths(Some(&working_dir), None)
        .expect_err("zero context_window should fail validation");
    let message = error.to_string();
    assert!(
        message.contains("invalid") && message.contains("context_window"),
        "unexpected error: {message}"
    );
}

#[test]
fn models_config_invalid_context_window_reports_the_real_source_file() {
    let user_dir = temp_test_dir("invalid-context-window-user-source");
    let working_dir = temp_test_dir("invalid-context-window-project-overlay");
    fs::write(
        user_dir.join("models.toml"),
        r#"
[defaults]
context_window = 0
"#,
    )
    .expect("user models config should be written");
    fs::create_dir_all(working_dir.join(".hunea")).expect("project config dir");
    fs::write(
        working_dir.join(".hunea").join("models.toml"),
        r#"
[providers.local]
enabled = true
kind = "openai_compatible"
base_url = "http://127.0.0.1:1234/v1"
models = ["qwen3"]
"#,
    )
    .expect("project models config should be written");

    let error = load_from_paths(Some(&working_dir), Some(&user_dir))
        .expect_err("invalid user config should keep its original source path");
    let message = error.to_string();
    let user_path = user_dir.join("models.toml");

    assert!(
        message.contains(&user_path.display().to_string()),
        "invalid context_window should report the user config path, got: {message}"
    );
}

#[test]
fn models_config_resolution_prefers_profile_over_defaults_and_builtin() {
    let working_dir = temp_test_dir("resolution-order");
    fs::write(
        working_dir.join("models.toml"),
        r#"
[defaults]
context_window = 64000

[providers.openai]
enabled = true
kind = "openai"
models = ["gpt-4o"]

[providers.openai.model_profiles."gpt-4o"]
context_window = 100000
"#,
    )
    .expect("models config should be written");

    let loaded = load_from_paths(Some(&working_dir), None).expect("models config should load");
    let selection = ModelSelection::new("openai", "gpt-4o");

    assert_eq!(loaded.context_limit_for(&selection), 100_000);
}

#[test]
fn models_config_unknown_model_without_defaults_uses_default_fallback() {
    let working_dir = temp_test_dir("unknown-no-defaults");
    fs::write(
        working_dir.join("models.toml"),
        r#"
[providers.local]
enabled = true
kind = "openai_compatible"
base_url = "http://127.0.0.1:1234/v1"
models = ["custom-local-7b"]
"#,
    )
    .expect("models config should be written");

    let loaded = load_from_paths(Some(&working_dir), None).expect("models config should load");
    let selection = ModelSelection::new("local", "custom-local-7b");

    assert_eq!(loaded.context_limit_for(&selection), 256_000);
}

#[test]
fn models_config_rejects_unknown_provider_field() {
    let working_dir = temp_test_dir("deny-unknown-provider");
    fs::write(
        working_dir.join("models.toml"),
        r#"
[providers.local]
enabled = true
kind = "openai_compatible"
base_url = "http://127.0.0.1:1234/v1"
unexpected_field = true
"#,
    )
    .expect("models config should be written");

    load_from_paths(Some(&working_dir), None).expect_err("unknown provider keys should fail");
}

#[test]
fn models_config_invalid_provider_kind_reports_the_real_source_file() {
    let user_dir = temp_test_dir("invalid-provider-kind-user-source");
    let working_dir = temp_test_dir("invalid-provider-kind-project-overlay");
    fs::write(
        user_dir.join("models.toml"),
        r#"
[providers.remote]
enabled = true
kind = "definitely_not_real"
base_url = "http://127.0.0.1:1234/v1"
"#,
    )
    .expect("user models config should be written");
    fs::create_dir_all(working_dir.join(".hunea")).expect("project config dir");
    fs::write(
        working_dir.join(".hunea").join("models.toml"),
        r#"
[defaults]
context_window = 128000
"#,
    )
    .expect("project models config should be written");

    let error = load_from_paths(Some(&working_dir), Some(&user_dir))
        .expect_err("invalid provider kind should keep its original source path");
    let message = error.to_string();
    let user_path = user_dir.join("models.toml");

    assert!(
        message.contains(&user_path.display().to_string()),
        "invalid provider kind should report the user config path, got: {message}"
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
        .join("hunea-runtime-models-config-tests")
        .join(unique);
    fs::create_dir_all(&path).expect("temp dir should be created");
    path
}
