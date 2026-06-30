use std::{
    collections::BTreeMap,
    env, fmt, fs, io,
    path::{Path, PathBuf},
};

use directories::ProjectDirs;
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

const MODELS_FILE_NAME: &str = "models.toml";
type ModelSyncResult = Result<Vec<String>, String>;

/// `LoadedModelCatalog` 是从 `models.toml` 得到的 TUI 模型目录与默认选择。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LoadedModelCatalog {
    pub catalog: ModelCatalog,
    pub context_limits: ModelContextLimits,
    pub selected_model: Option<ModelSelection>,
    pub source_path: Option<PathBuf>,
    pub requires_model_selection: bool,
}

impl LoadedModelCatalog {
    /// `context_limit_for` 解析指定模型选择的 context limit（tokens）。
    pub fn context_limit_for(&self, selection: &ModelSelection) -> ContextTokenLimit {
        self.catalog
            .context_limit_for(&self.context_limits, selection)
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

/// `load` 从用户配置目录与当前工作目录加载 `models.toml`。
pub fn load() -> Result<LoadedModelCatalog, ModelsConfigError> {
    let working_dir = env::current_dir().ok();
    load_from_paths(working_dir.as_deref(), user_config_directory().as_deref())
}

/// `load_from_paths` 从指定目录加载本地模型配置，不在启动路径同步 provider 模型列表。
pub fn load_from_paths(
    working_dir: Option<&Path>,
    user_config_dir: Option<&Path>,
) -> Result<LoadedModelCatalog, ModelsConfigError> {
    let mut merged = MergedModelsConfig::default();
    let mut source_path = None;

    for path in model_config_paths(working_dir, user_config_dir) {
        let Some(file_config) = read_models_config(&path)? else {
            continue;
        };
        merge_models_config(&mut merged, file_config, &path);
        source_path = Some(path);
    }

    if source_path.is_none() {
        return Ok(LoadedModelCatalog::default());
    }

    let catalog = catalog_from_config(&merged, source_path.as_deref())?;
    let context_limits = context_limits_from_merged(&merged, source_path.as_deref())?;
    let selected_model = selection_from_default(merged.default.as_deref(), &catalog);

    Ok(LoadedModelCatalog {
        catalog,
        context_limits,
        selected_model,
        source_path,
        requires_model_selection: true,
    })
}

/// `write_default_model` 将用户最后一次选择写回 `models.toml` 的 `default` 字段。
pub fn write_default_model(
    source_path: Option<&Path>,
    selection: &ModelSelection,
) -> Result<PathBuf, ModelsConfigError> {
    let path = source_path
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from(MODELS_FILE_NAME));
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

fn model_config_paths(working_dir: Option<&Path>, user_config_dir: Option<&Path>) -> Vec<PathBuf> {
    let mut paths = Vec::with_capacity(3);
    if let Some(path) = user_config_dir {
        paths.push(path.join(MODELS_FILE_NAME));
    }
    if let Some(path) = working_dir {
        paths.push(path.join(MODELS_FILE_NAME));
        paths.push(path.join(".hunea").join(MODELS_FILE_NAME));
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
    if value == 0 || value > u32::MAX as u64 {
        return Err(ModelsConfigError::InvalidContextWindow {
            path: path.to_path_buf(),
            field: field.to_string(),
            value,
        });
    }

    ContextTokenLimit::try_from(value as u32).map_err(|_| ModelsConfigError::InvalidContextWindow {
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

fn user_config_directory() -> Option<PathBuf> {
    ProjectDirs::from("", "", "hunea").map(|dirs| dirs.config_dir().to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
