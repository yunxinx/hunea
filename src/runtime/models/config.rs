use std::{
    collections::BTreeMap,
    env, fmt, fs, io,
    path::{Path, PathBuf},
    time::Duration,
};

use directories::ProjectDirs;
use reqwest::blocking::Client;
use serde::Deserialize;
use toml_edit::DocumentMut;

use super::{ModelCatalog, ModelEntry, ModelProvider, ModelSelection, ModelSource};

const MODELS_FILE_NAME: &str = "models.toml";
const MODEL_SYNC_TIMEOUT: Duration = Duration::from_secs(3);
type ModelSyncResult = Result<Vec<String>, String>;

/// `LoadedModelCatalog` 是从 `models.toml` 得到的 TUI 模型目录与默认选择。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LoadedModelCatalog {
    pub catalog: ModelCatalog,
    pub selected_model: Option<ModelSelection>,
    pub source_path: Option<PathBuf>,
    pub requires_model_selection: bool,
}

/// `ProviderSyncRequest` 描述一次 OpenAI-compatible `/models` 同步请求。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderSyncRequest {
    pub provider_id: String,
    pub display_name: String,
    pub base_url: String,
    pub api_key_env: Option<String>,
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
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileModelsConfig {
    default: Option<String>,
    #[serde(default)]
    providers: BTreeMap<String, FileModelProviderConfig>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileModelProviderConfig {
    enabled: Option<bool>,
    kind: Option<String>,
    display_name: Option<String>,
    base_url: Option<String>,
    api_key_env: Option<String>,
    models: Option<Vec<String>>,
}

#[derive(Debug, Clone, Default)]
struct MergedModelsConfig {
    default: Option<String>,
    providers: BTreeMap<String, FileModelProviderConfig>,
}

#[derive(Debug, Deserialize)]
struct OpenAiModelsResponse {
    data: Vec<OpenAiModel>,
}

#[derive(Debug, Deserialize)]
struct OpenAiModel {
    id: String,
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
            Self::InvalidProviderKind { .. } => None,
        }
    }
}

/// `load` 从用户配置目录与当前工作目录加载 `models.toml`。
pub fn load() -> Result<LoadedModelCatalog, ModelsConfigError> {
    let working_dir = env::current_dir().ok();
    load_from_paths(working_dir.as_deref(), user_config_directory().as_deref())
}

/// `load_from_paths` 从指定目录加载模型配置，真实同步 OpenAI-compatible `/models`。
pub fn load_from_paths(
    working_dir: Option<&Path>,
    user_config_dir: Option<&Path>,
) -> Result<LoadedModelCatalog, ModelsConfigError> {
    load_from_paths_with_sync(working_dir, user_config_dir, sync_openai_compatible_models)
}

/// `load_from_paths_with_sync` 使用注入的同步函数加载模型配置，便于测试。
pub fn load_from_paths_with_sync(
    working_dir: Option<&Path>,
    user_config_dir: Option<&Path>,
    mut sync_models: impl FnMut(&ProviderSyncRequest) -> ModelSyncResult,
) -> Result<LoadedModelCatalog, ModelsConfigError> {
    let mut merged = MergedModelsConfig::default();
    let mut source_path = None;

    for path in model_config_paths(working_dir, user_config_dir) {
        let Some(file_config) = read_models_config(&path)? else {
            continue;
        };
        merge_models_config(&mut merged, file_config);
        source_path = Some(path);
    }

    if source_path.is_none() {
        return Ok(LoadedModelCatalog::default());
    }

    let catalog = catalog_from_config(&merged, source_path.as_deref(), &mut sync_models)?;
    let selected_model = selection_from_default(merged.default.as_deref(), &catalog);

    Ok(LoadedModelCatalog {
        catalog,
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

fn model_config_paths(working_dir: Option<&Path>, user_config_dir: Option<&Path>) -> Vec<PathBuf> {
    let mut paths = Vec::with_capacity(3);
    if let Some(path) = user_config_dir {
        paths.push(path.join(MODELS_FILE_NAME));
    }
    if let Some(path) = working_dir {
        paths.push(path.join(MODELS_FILE_NAME));
        paths.push(path.join(".lumos").join(MODELS_FILE_NAME));
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

fn merge_models_config(target: &mut MergedModelsConfig, source: FileModelsConfig) {
    if let Some(default) = source.default {
        target.default = Some(default);
    }

    for (provider_id, provider) in source.providers {
        target.providers.insert(provider_id, provider);
    }
}

fn catalog_from_config(
    config: &MergedModelsConfig,
    source_path: Option<&Path>,
    sync_models: &mut impl FnMut(&ProviderSyncRequest) -> ModelSyncResult,
) -> Result<ModelCatalog, ModelsConfigError> {
    let mut providers = Vec::with_capacity(config.providers.len());
    for (provider_id, provider) in &config.providers {
        validate_provider_kind(provider_id, provider, source_path)?;
        providers.push(provider_from_config(provider_id, provider, sync_models));
    }

    Ok(ModelCatalog::new(providers))
}

fn validate_provider_kind(
    provider_id: &str,
    provider: &FileModelProviderConfig,
    source_path: Option<&Path>,
) -> Result<(), ModelsConfigError> {
    let Some(kind) = provider.kind.as_deref() else {
        return Ok(());
    };
    if kind == "openai_compatible" {
        return Ok(());
    }

    Err(ModelsConfigError::InvalidProviderKind {
        path: source_path
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from(MODELS_FILE_NAME)),
        provider: provider_id.to_string(),
        value: kind.to_string(),
    })
}

fn provider_from_config(
    provider_id: &str,
    provider: &FileModelProviderConfig,
    sync_models: &mut impl FnMut(&ProviderSyncRequest) -> ModelSyncResult,
) -> ModelProvider {
    let display_name = provider
        .display_name
        .clone()
        .unwrap_or_else(|| provider_id.to_string());
    let base_url = provider.base_url.clone();
    let enabled = provider.enabled.unwrap_or(true);
    let (source, models, sync_error) = match provider.models.as_ref() {
        Some(models) => (
            ModelSource::Configured,
            models
                .iter()
                .map(|model| ModelEntry::new(model.clone(), None, ModelSource::Configured))
                .collect(),
            None,
        ),
        None if enabled => {
            let sync_result = base_url
                .as_ref()
                .map(|base_url| {
                    sync_models(&ProviderSyncRequest {
                        provider_id: provider_id.to_string(),
                        display_name: display_name.clone(),
                        base_url: base_url.clone(),
                        api_key_env: provider.api_key_env.clone(),
                    })
                })
                .unwrap_or_else(|| Err("base_url is not configured".to_string()));
            let (synced, sync_error) = match sync_result {
                Ok(models) => (models, None),
                Err(error) => (Vec::new(), Some(error)),
            };
            (
                ModelSource::Synced,
                synced
                    .into_iter()
                    .map(|model| ModelEntry::new(model, None, ModelSource::Synced))
                    .collect(),
                sync_error,
            )
        }
        None => (ModelSource::Synced, Vec::new(), None),
    };

    let mut model_provider =
        ModelProvider::new(provider_id, display_name, base_url, source, models);
    model_provider.sync_error = sync_error;
    if enabled {
        model_provider
    } else {
        ModelProvider::disabled(
            model_provider.id,
            model_provider.display_name,
            model_provider.base_url,
            model_provider.source,
            model_provider.models,
        )
    }
}

fn selection_from_default(default: Option<&str>, catalog: &ModelCatalog) -> Option<ModelSelection> {
    let default = default?.trim();
    if default.is_empty() {
        return None;
    }

    let selection = if let Some((provider_id, model_id)) = default.split_once('/') {
        ModelSelection::new(provider_id.trim(), model_id.trim())
    } else {
        catalog
            .enabled_providers()
            .find(|provider| provider.models.iter().any(|model| model.id == default))
            .map(|provider| ModelSelection::new(provider.id.clone(), default.to_string()))?
    };

    catalog.contains_selection(&selection).then_some(selection)
}

fn sync_openai_compatible_models(request: &ProviderSyncRequest) -> ModelSyncResult {
    let client = Client::builder()
        .timeout(MODEL_SYNC_TIMEOUT)
        .build()
        .map_err(|source| format!("create HTTP client: {source}"))?;
    let endpoint = format!("{}/models", request.base_url.trim_end_matches('/'));
    let mut builder = client.get(&endpoint);
    if let Some(api_key) = request
        .api_key_env
        .as_deref()
        .and_then(|name| env::var(name).ok())
        .filter(|value| !value.trim().is_empty())
    {
        builder = builder.bearer_auth(api_key);
    }

    let response = builder
        .send()
        .map_err(|_| format!("cannot reach {endpoint}"))?;
    let status = response.status();
    if !status.is_success() {
        return Err(format!("HTTP {status} from {endpoint}"));
    }
    let body = response
        .json::<OpenAiModelsResponse>()
        .map_err(|_| format!("invalid response from {endpoint}"))?;

    Ok(body.data.into_iter().map(|model| model.id).collect())
}

fn user_config_directory() -> Option<PathBuf> {
    ProjectDirs::from("", "", "lumos").map(|dirs| dirs.config_dir().to_path_buf())
}
