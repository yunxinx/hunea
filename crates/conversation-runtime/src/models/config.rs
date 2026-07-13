use std::{
    collections::BTreeMap,
    fmt, fs, io,
    path::{Path, PathBuf},
};

use runtime_domain::paths::{DataDirResolution, MODELS_FILE_NAME, WORKSPACE_HUNEA_DIRNAME};
use serde::Deserialize;
use toml_edit::DocumentMut;

use crate::list_provider_models;
use runtime_domain::{
    context_budget::ContextTokenLimit,
    model_catalog::{
        ModelCatalog, ModelEntry, ModelProvider, ModelSelection, ModelSource, ProviderSyncRequest,
    },
    model_context_limit::ModelContextLimits,
    provider::{ProviderApiKey, ProviderKind},
    session::ProviderRequest,
};

type ModelSyncResult = Result<Vec<String>, String>;

/// `LoadedModelCatalog` 是从 `models.toml` 得到的 TUI 模型目录与默认选择。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LoadedModelCatalog {
    pub catalog: ModelCatalog,
    pub context_limits: ModelContextLimits,
    pub selected_model: Option<ModelSelection>,
    pub source_path: Option<PathBuf>,
}

impl LoadedModelCatalog {
    /// `context_limit_for` 解析指定模型选择的 context limit（tokens）。
    pub fn context_limit_for(&self, selection: &ModelSelection) -> ContextTokenLimit {
        self.context_limits.resolve(&self.catalog, selection)
    }
}

/// `ModelsConfigError` 描述模型配置读取或校验失败。
#[derive(Debug)]
pub enum ModelsConfigError {
    Read {
        path: PathBuf,
        source: io::Error,
    },
    Decode {
        path: PathBuf,
        source: toml::de::Error,
    },
    Edit {
        path: PathBuf,
        source: toml_edit::TomlError,
    },
    Write {
        path: PathBuf,
        source: io::Error,
    },
    InvalidProviderKind {
        path: PathBuf,
        provider: String,
        value: String,
    },
    InvalidContextWindow {
        path: PathBuf,
        field: String,
        value: u64,
    },
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileModelsConfig {
    default: Option<String>,
    defaults: Option<FileModelsDefaults>,
    #[serde(default)]
    providers: BTreeMap<String, FileModelProviderConfig>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileModelsDefaults {
    context_window: Option<u64>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileModelProviderConfig {
    enabled: Option<bool>,
    kind: Option<String>,
    display_name: Option<String>,
    base_url: Option<String>,
    api_key: Option<String>,
    api_key_env: Option<String>,
    models: Option<Vec<String>>,
    #[serde(default)]
    model_profiles: BTreeMap<String, FileModelProfileConfig>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileModelProfileConfig {
    context_window: Option<u64>,
}

#[derive(Debug, Clone, Default)]
struct MergedModelsConfig {
    default: Option<String>,
    defaults: Option<FileModelsDefaults>,
    defaults_source_path: Option<PathBuf>,
    providers: BTreeMap<String, SourcedFileModelProviderConfig>,
}

#[derive(Debug, Clone)]
struct SourcedFileModelProviderConfig {
    config: FileModelProviderConfig,
    source_path: PathBuf,
}

impl fmt::Display for ModelsConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read { path, source } => {
                write!(f, "read model config file {}: {source}", path.display())
            }
            Self::Decode { path, source } => {
                write!(f, "decode model config file {}: {source}", path.display())
            }
            Self::Edit { path, source } => {
                write!(f, "edit model config file {}: {source}", path.display())
            }
            Self::Write { path, source } => {
                write!(f, "write model config file {}: {source}", path.display())
            }
            Self::InvalidProviderKind {
                path,
                provider,
                value,
            } => write!(
                f,
                "validate model config file {}: unknown providers.{}.kind {:?}",
                path.display(),
                provider,
                value
            ),
            Self::InvalidContextWindow { path, field, value } => write!(
                f,
                "validate model config file {}: invalid {} context_window {value}",
                path.display(),
                field
            ),
        }
    }
}

impl std::error::Error for ModelsConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Read { source, .. } => Some(source),
            Self::Decode { source, .. } => Some(source),
            Self::Edit { source, .. } => Some(source),
            Self::Write { source, .. } => Some(source),
            Self::InvalidProviderKind { .. } | Self::InvalidContextWindow { .. } => None,
        }
    }
}

/// `load_with_resolution` 根据预检阶段决定的数据目录解析结果加载 `models.toml`。
///
/// 后加载覆盖先加载。路径由 `DataDirResolution::layered_config_file_paths` 统一决议。
///
/// 错误策略与 `app-config` 一致：
/// - NotFound → 跳过（可无 models.toml，走空目录默认）
/// - Read 权限/IO → 降级为 warning，继续下一个源
/// - 全部源均为 Read 错误 → 空目录默认 + warnings（内置默认足够启动）
/// - Decode/Validation → fatal
pub fn load_with_resolution(
    working_dir: Option<&Path>,
    resolution: &DataDirResolution,
) -> Result<(LoadedModelCatalog, Vec<ModelsConfigError>), ModelsConfigError> {
    let paths = resolution.layered_config_file_paths(working_dir, MODELS_FILE_NAME);
    load_from_explicit_paths(&paths)
}

/// `load_from_paths` 从指定目录加载本地模型配置，不在启动路径同步 provider 模型列表。
pub fn load_from_paths(
    working_dir: Option<&Path>,
    user_config_dir: Option<&Path>,
) -> Result<LoadedModelCatalog, ModelsConfigError> {
    let (loaded, _warnings) =
        load_from_explicit_paths(&model_config_paths(working_dir, user_config_dir))?;
    Ok(loaded)
}

fn load_from_explicit_paths(
    paths: &[PathBuf],
) -> Result<(LoadedModelCatalog, Vec<ModelsConfigError>), ModelsConfigError> {
    let mut merged = MergedModelsConfig::default();
    let mut source_path = None;
    let mut warnings = Vec::new();

    for path in paths {
        match read_models_config(path) {
            Ok(Some(file_config)) => {
                merge_models_config(&mut merged, file_config, path);
                source_path = Some(path.clone());
            }
            // models.toml 可选：缺文件用空目录默认，不阻塞启动。
            Ok(None) => {}
            // 文件级权限/IO：降级为 warning。目录能否用由预检决定。
            Err(error @ ModelsConfigError::Read { .. }) => warnings.push(error),
            // Decode/Validation：内容错误，立即 fatal。
            Err(other) => return Err(other),
        }
    }

    // 没有任何源成功加载时返回空目录 + warnings，而不是 fatal。
    if source_path.is_none() {
        return Ok((LoadedModelCatalog::default(), warnings));
    }

    let catalog = catalog_from_config(&merged, source_path.as_deref())?;
    let context_limits = context_limits_from_merged(&merged, source_path.as_deref())?;
    let selected_model = selection_from_default(merged.default.as_deref(), &catalog);

    Ok((
        LoadedModelCatalog {
            catalog,
            context_limits,
            selected_model,
            source_path,
        },
        warnings,
    ))
}

/// `write_default_model` 将用户最后一次选择写回 `models.toml` 的 `default` 字段。
pub fn write_default_model(
    source_path: Option<&Path>,
    selection: &ModelSelection,
) -> Result<PathBuf, ModelsConfigError> {
    let path = source_path
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from(WORKSPACE_HUNEA_DIRNAME).join(MODELS_FILE_NAME));
    let content = match fs::read_to_string(&path) {
        Ok(content) => content,
        Err(error) if error.kind() == io::ErrorKind::NotFound => String::new(),
        Err(source) => {
            return Err(ModelsConfigError::Read {
                path: path.clone(),
                source,
            });
        }
    };
    let mut document =
        content
            .parse::<DocumentMut>()
            .map_err(|source| ModelsConfigError::Edit {
                path: path.clone(),
                source,
            })?;
    document["default"] = toml_edit::value(selection.display_name());

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|source| ModelsConfigError::Write {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    fs::write(&path, document.to_string()).map_err(|source| ModelsConfigError::Write {
        path: path.clone(),
        source,
    })?;

    Ok(path)
}

/// `sync_provider_models_once` 立即刷新指定 provider 的模型列表。
pub fn sync_provider_models_once(request: &ProviderSyncRequest) -> Result<Vec<String>, String> {
    sync_provider_models(request)
}

/// 测试 / 显式路径入口用的搜索列表；生产路径走 `DataDirResolution::layered_config_file_paths`。
fn model_config_paths(working_dir: Option<&Path>, user_config_dir: Option<&Path>) -> Vec<PathBuf> {
    let mut paths = Vec::with_capacity(2);
    if let Some(path) = user_config_dir {
        paths.push(path.join(MODELS_FILE_NAME));
    }
    if let Some(path) = working_dir {
        // 只认 `.hunea/models.toml`，不读工作区根 `models.toml`（历史错误位置，已废弃）。
        paths.push(path.join(WORKSPACE_HUNEA_DIRNAME).join(MODELS_FILE_NAME));
    }
    paths
}

fn read_models_config(path: &Path) -> Result<Option<FileModelsConfig>, ModelsConfigError> {
    let content = match fs::read_to_string(path) {
        Ok(content) => content,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(ModelsConfigError::Read {
                path: path.to_path_buf(),
                source,
            });
        }
    };

    let config = toml::from_str(&content).map_err(|source| ModelsConfigError::Decode {
        path: path.to_path_buf(),
        source,
    })?;
    Ok(Some(config))
}

fn merge_models_config(target: &mut MergedModelsConfig, source: FileModelsConfig, path: &Path) {
    if let Some(default) = source.default {
        target.default = Some(default);
    }
    if let Some(defaults) = source.defaults {
        target.defaults = Some(defaults);
        target.defaults_source_path = Some(path.to_path_buf());
    }

    for (provider_id, provider) in source.providers {
        target.providers.insert(
            provider_id,
            SourcedFileModelProviderConfig {
                config: provider,
                source_path: path.to_path_buf(),
            },
        );
    }
}

fn context_limits_from_merged(
    config: &MergedModelsConfig,
    source_path: Option<&Path>,
) -> Result<ModelContextLimits, ModelsConfigError> {
    let defaults_path = config
        .defaults_source_path
        .clone()
        .or_else(|| source_path.map(Path::to_path_buf))
        .unwrap_or_else(|| PathBuf::from(MODELS_FILE_NAME));

    let defaults = match config.defaults.as_ref().and_then(|d| d.context_window) {
        Some(value) => Some(validate_positive_context_window(
            value,
            "defaults",
            &defaults_path,
        )?),
        None => None,
    };

    let mut by_provider_model = BTreeMap::new();
    for (provider_id, provider) in &config.providers {
        for (model_id, profile) in &provider.config.model_profiles {
            let Some(value) = profile.context_window else {
                continue;
            };
            let field = format!("providers.{provider_id}.model_profiles.{model_id}");
            let limit = validate_positive_context_window(value, &field, &provider.source_path)?;
            by_provider_model.insert((provider_id.clone(), model_id.clone()), limit);
        }
    }

    Ok(ModelContextLimits::new(defaults, by_provider_model))
}

fn validate_positive_context_window(
    value: u64,
    field: &str,
    path: &Path,
) -> Result<ContextTokenLimit, ModelsConfigError> {
    let value_usize =
        usize::try_from(value).map_err(|_| ModelsConfigError::InvalidContextWindow {
            path: path.to_path_buf(),
            field: field.to_string(),
            value,
        })?;

    ContextTokenLimit::try_from(value_usize).map_err(|_| ModelsConfigError::InvalidContextWindow {
        path: path.to_path_buf(),
        field: field.to_string(),
        value,
    })
}

fn catalog_from_config(
    config: &MergedModelsConfig,
    _source_path: Option<&Path>,
) -> Result<ModelCatalog, ModelsConfigError> {
    let mut providers = Vec::with_capacity(config.providers.len());
    for (provider_id, provider) in &config.providers {
        validate_provider_kind(provider_id, provider)?;
        providers.push(provider_from_config(provider_id, &provider.config));
    }

    Ok(ModelCatalog::new(providers))
}

fn validate_provider_kind(
    provider_id: &str,
    provider: &SourcedFileModelProviderConfig,
) -> Result<(), ModelsConfigError> {
    let kind = provider
        .config
        .kind
        .as_deref()
        .unwrap_or("openai_compatible");
    ProviderKind::from_config_value(kind)
        .map(|_| ())
        .ok_or_else(|| ModelsConfigError::InvalidProviderKind {
            path: provider.source_path.clone(),
            provider: provider_id.to_string(),
            value: kind.to_string(),
        })
}

fn provider_from_config(provider_id: &str, provider: &FileModelProviderConfig) -> ModelProvider {
    let kind = provider
        .kind
        .as_deref()
        .and_then(ProviderKind::from_config_value)
        .unwrap_or_default();
    let display_name = provider
        .display_name
        .clone()
        .unwrap_or_else(|| provider_id.to_string());
    let base_url = provider.base_url.clone();
    let api_key = ProviderApiKey::from_optional_config(provider.api_key.clone());
    let enabled = provider.enabled.unwrap_or(true);
    let (source, models) = match provider.models.as_ref() {
        Some(models) => (
            ModelSource::Configured,
            models
                .iter()
                .map(|model| ModelEntry::new(model.clone(), None, ModelSource::Configured))
                .collect(),
        ),
        None => (ModelSource::NotLoaded, Vec::new()),
    };

    let mut model_provider =
        ModelProvider::new(provider_id, kind, display_name, base_url, source, models)
            .with_api_key(api_key)
            .with_api_key_env(provider.api_key_env.clone());
    model_provider.enabled = enabled;
    model_provider
}

fn selection_from_default(default: Option<&str>, catalog: &ModelCatalog) -> Option<ModelSelection> {
    let default = default?.trim();
    if default.is_empty() {
        return None;
    }

    if let Some((provider_id, model_id)) = default.split_once('/') {
        let provider_id = provider_id.trim();
        let model_id = model_id.trim();
        if provider_id.is_empty() || model_id.is_empty() {
            return None;
        }
        return Some(ModelSelection::new(provider_id, model_id));
    }

    let mut matches = catalog
        .enabled_providers()
        .filter(|provider| provider.models.iter().any(|model| model.id == default));
    if let Some(provider) = matches.next() {
        if matches.next().is_none() {
            return Some(ModelSelection::new(
                provider.id.clone(),
                default.to_string(),
            ));
        }
        return None;
    }

    let mut enabled_providers = catalog.enabled_providers();
    let provider = enabled_providers.next()?;
    if enabled_providers.next().is_some() {
        None
    } else {
        Some(ModelSelection::new(
            provider.id.clone(),
            default.to_string(),
        ))
    }
}

fn sync_provider_models(request: &ProviderSyncRequest) -> ModelSyncResult {
    if !request.kind.uses_openai_compatible_endpoint() && request.kind != ProviderKind::OpenAi {
        return Err(format!(
            "model sync for {} is not supported; configure models = [...]",
            request.kind
        ));
    }

    let request = ProviderRequest::new(
        request.provider_id.clone(),
        request.kind,
        "__model_sync__",
        request.base_url.clone(),
        request.api_key.clone(),
        request.api_key_env.clone(),
        Vec::new(),
    );
    list_provider_models(&request).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    fn process_euid_is_root() -> bool {
        // SAFETY: geteuid 无参数、无内存副作用；仅测试用。
        unsafe { libc::geteuid() == 0 }
    }
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn non_openai_provider_requires_configured_model_allowlist_for_sync() {
        let request = ProviderSyncRequest {
            provider_id: "anthropic_proxy".to_string(),
            kind: ProviderKind::Anthropic,
            display_name: "Anthropic Proxy".to_string(),
            base_url: Some("http://127.0.0.1:9/v1".to_string()),
            api_key: None,
            api_key_env: Some("ANTHROPIC_API_KEY".to_string()),
        };

        let error = sync_provider_models(&request)
            .expect_err("custom endpoint model sync should be explicit");

        assert_eq!(
            error,
            "model sync for anthropic is not supported; configure models = [...]"
        );
    }

    #[test]
    fn config_accepts_openai_responses_provider_kind() {
        let root = tempdir_path("models-openai-responses");
        let models_path = root.join(".hunea").join("models.toml");
        fs::create_dir_all(models_path.parent().expect("models parent should exist"))
            .expect("models parent should be creatable");
        fs::write(
            &models_path,
            r#"
default = "responses/fast-responses-model"

[providers.responses]
kind = "openai_responses"
base_url = "https://responses.example.com/v1"
models = ["fast-responses-model"]
"#,
        )
        .expect("models config should be writable");

        let loaded = load_from_paths(Some(&root), None).expect("models config should load");
        let provider = loaded
            .catalog
            .enabled_provider_by_id("responses")
            .expect("provider should exist");

        assert_eq!(provider.connection.kind, ProviderKind::OpenAiResponses);
        assert_eq!(
            loaded.selected_model,
            Some(runtime_domain::model_catalog::ModelSelection::new(
                "responses",
                "fast-responses-model"
            ))
        );
    }

    #[test]
    fn openai_custom_base_url_syncs_through_models_endpoint() {
        let request = ProviderSyncRequest {
            provider_id: "openai_proxy".to_string(),
            kind: ProviderKind::OpenAi,
            display_name: "OpenAI Proxy".to_string(),
            base_url: Some("http://127.0.0.1:9/v1".to_string()),
            api_key: Some(ProviderApiKey::new("test-key")),
            api_key_env: Some("OPENAI_API_KEY".to_string()),
        };

        let error = sync_provider_models(&request)
            .expect_err("unreachable endpoint should fail after choosing /models sync");

        assert!(error.contains("transport error"));
    }

    #[test]
    fn load_with_resolution_global_merges_global_and_workspace() {
        let working_dir = tempdir_path("resolution-global-merge-working");
        let global_dir = tempdir_path("resolution-global-merge-global");
        fs::create_dir_all(&global_dir).expect("global dir should be created");
        fs::write(
            global_dir.join("models.toml"),
            r#"
[providers.global]
kind = "openai_compatible"
base_url = "http://127.0.0.1:9/v1"
models = ["global-model"]
"#,
        )
        .expect("global models should be written");
        fs::create_dir_all(working_dir.join(".hunea")).expect("hunea dir should be created");
        fs::write(
            working_dir.join(".hunea").join("models.toml"),
            r#"
[providers.workspace]
kind = "openai_compatible"
base_url = "http://127.0.0.1:9/v1"
models = ["workspace-model"]
"#,
        )
        .expect("workspace models should be written");

        let resolution = DataDirResolution::Global(global_dir);
        let (loaded, _warnings) = load_with_resolution(Some(&working_dir), &resolution)
            .expect("global resolution should load");

        assert!(loaded.catalog.enabled_provider_by_id("global").is_some());
        assert!(loaded.catalog.enabled_provider_by_id("workspace").is_some());
    }

    #[test]
    fn load_with_resolution_portable_skips_global() {
        let working_dir = tempdir_path("resolution-portable-skip-working");
        let global_dir = tempdir_path("resolution-portable-skip-global");
        fs::create_dir_all(&global_dir).expect("global dir should be created");
        fs::write(
            global_dir.join("models.toml"),
            r#"
[providers.global]
kind = "openai_compatible"
base_url = "http://127.0.0.1:9/v1"
"#,
        )
        .expect("global models should be written");
        fs::create_dir_all(working_dir.join(".hunea")).expect("hunea dir should be created");
        fs::write(
            working_dir.join(".hunea").join("models.toml"),
            r#"
[providers.portable]
kind = "openai_compatible"
base_url = "http://127.0.0.1:9/v1"
"#,
        )
        .expect("portable models should be written");

        let resolution = DataDirResolution::Portable(working_dir.join(".hunea"));
        let (loaded, _warnings) = load_with_resolution(Some(&working_dir), &resolution)
            .expect("portable resolution should load");

        assert!(loaded.catalog.enabled_provider_by_id("portable").is_some());
        assert!(loaded.catalog.enabled_provider_by_id("global").is_none());
    }

    #[test]
    fn load_with_resolution_ignores_workspace_root_models_toml() {
        let working_dir = tempdir_path("resolution-ignore-root-working");
        fs::create_dir_all(&working_dir).expect("working dir should be created");
        fs::write(
            working_dir.join("models.toml"),
            r#"
[providers.root]
kind = "openai_compatible"
base_url = "http://127.0.0.1:9/v1"
models = ["root-model"]
"#,
        )
        .expect("workspace-root models should be written");
        fs::create_dir_all(working_dir.join(".hunea")).expect("hunea dir should be created");
        fs::write(
            working_dir.join(".hunea").join("models.toml"),
            r#"
[providers.project]
kind = "openai_compatible"
base_url = "http://127.0.0.1:9/v1"
models = ["project-model"]
"#,
        )
        .expect("project models should be written");

        let resolution = DataDirResolution::Portable(working_dir.join(".hunea"));
        let (loaded, _warnings) = load_with_resolution(Some(&working_dir), &resolution)
            .expect("portable resolution should load");

        assert!(loaded.catalog.enabled_provider_by_id("root").is_none());
        assert!(loaded.catalog.enabled_provider_by_id("project").is_some());
        assert_eq!(
            loaded.source_path.as_deref(),
            Some(working_dir.join(".hunea").join("models.toml").as_path()),
        );
    }

    #[cfg(unix)]
    #[test]
    fn load_with_resolution_skips_unreadable_global_and_uses_workspace() {
        if process_euid_is_root() {
            eprintln!("skipping permission test under root");
            return;
        }

        let working_dir = tempdir_path("resolution-skip-read-working");
        let global_dir = tempdir_path("resolution-skip-read-global");
        fs::create_dir_all(&global_dir).expect("global dir should be created");
        fs::write(
            global_dir.join("models.toml"),
            r#"
[providers.global]
kind = "openai_compatible"
base_url = "http://127.0.0.1:9/v1"
"#,
        )
        .expect("global models should be written");
        fs::create_dir_all(working_dir.join(".hunea")).expect("hunea dir should be created");
        fs::write(
            working_dir.join(".hunea").join("models.toml"),
            r#"
[providers.workspace]
kind = "openai_compatible"
base_url = "http://127.0.0.1:9/v1"
"#,
        )
        .expect("workspace models should be written");

        use std::os::unix::fs::PermissionsExt;
        let unreadable_path = global_dir.join("models.toml");
        fs::set_permissions(&unreadable_path, fs::Permissions::from_mode(0o000))
            .expect("chmod should work");

        let resolution = DataDirResolution::Global(global_dir);
        let (loaded, warnings) = load_with_resolution(Some(&working_dir), &resolution)
            .expect("should skip unreadable global and load workspace");

        // 恢复权限以便 tempdir 清理
        let _ = fs::set_permissions(&unreadable_path, fs::Permissions::from_mode(0o644));

        assert!(loaded.catalog.enabled_provider_by_id("workspace").is_some());
        assert!(loaded.catalog.enabled_provider_by_id("global").is_none());
        assert_eq!(
            warnings.len(),
            1,
            "unreadable global should surface as warning"
        );
    }

    #[cfg(unix)]
    #[test]
    fn load_with_resolution_all_sources_unreadable_uses_defaults_with_warnings() {
        if process_euid_is_root() {
            eprintln!("skipping permission test under root");
            return;
        }

        let working_dir = tempdir_path("resolution-all-unreadable-working");
        let global_dir = tempdir_path("resolution-all-unreadable-global");
        fs::create_dir_all(&global_dir).expect("global dir should be created");
        fs::write(
            global_dir.join("models.toml"),
            r#"
[providers.global]
kind = "openai_compatible"
base_url = "http://127.0.0.1:9/v1"
"#,
        )
        .expect("global models should be written");
        fs::create_dir_all(working_dir.join(".hunea")).expect("hunea dir should be created");
        fs::write(
            working_dir.join(".hunea").join("models.toml"),
            r#"
[providers.workspace]
kind = "openai_compatible"
base_url = "http://127.0.0.1:9/v1"
"#,
        )
        .expect("workspace models should be written");

        use std::os::unix::fs::PermissionsExt;
        let global_path = global_dir.join("models.toml");
        let workspace_path = working_dir.join(".hunea").join("models.toml");
        fs::set_permissions(&global_path, fs::Permissions::from_mode(0o000))
            .expect("chmod should work");
        fs::set_permissions(&workspace_path, fs::Permissions::from_mode(0o000))
            .expect("chmod should work");

        let resolution = DataDirResolution::Global(global_dir);
        let (loaded, warnings) = load_with_resolution(Some(&working_dir), &resolution)
            .expect("unreadable files should fall back to defaults");

        let _ = fs::set_permissions(&global_path, fs::Permissions::from_mode(0o644));
        let _ = fs::set_permissions(&workspace_path, fs::Permissions::from_mode(0o644));

        assert_eq!(loaded.catalog.enabled_provider_count(), 0);
        assert_eq!(warnings.len(), 2, "expected two warnings: {warnings:?}");
        assert!(
            warnings
                .iter()
                .all(|w| matches!(w, ModelsConfigError::Read { .. })),
            "expected Read warnings, got: {warnings:?}"
        );
    }

    fn tempdir_path(label: &str) -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after epoch")
            .as_nanos();
        std::env::temp_dir().join(format!(
            "hunea-models-config-{label}-{}-{stamp}",
            std::process::id()
        ))
    }
}
