use std::{
    fmt, fs, io,
    path::{Path, PathBuf},
};

use serde::Deserialize;

use crate::paths::{DataDirResolution, PHRASES_FILE_NAME, WORKSPACE_HUNEA_DIRNAME};

/// `DEFAULT_STATUS_PHRASES` 是等待行在没有模型 reasoning header 时使用的内置文案。
pub const DEFAULT_STATUS_PHRASES: &[&str] = &[
    "Cooking",
    "Preparing",
    "Crafting",
    "Brewing",
    "Composing",
    "Weaving",
    "Conjuring",
    "Summoning",
    "Generating",
    "Processing",
];

/// `StatusPhraseMode` 描述用户短语如何与内置短语合并。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StatusPhraseMode {
    #[default]
    Append,
    Override,
}

/// `StatusPhraseOrder` 描述等待行短语的选择策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StatusPhraseOrder {
    #[default]
    Random,
    Cycle,
}

/// `LoadedStatusPhrases` 是从 `phrases.toml` 合并后的等待行文案配置。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedStatusPhrases {
    pub phrases: Vec<String>,
    pub mode: StatusPhraseMode,
    pub order: StatusPhraseOrder,
    pub source_path: Option<PathBuf>,
}

impl Default for LoadedStatusPhrases {
    fn default() -> Self {
        Self {
            phrases: default_status_phrases(),
            mode: StatusPhraseMode::Append,
            order: StatusPhraseOrder::Random,
            source_path: None,
        }
    }
}

/// `PhrasesConfigError` 描述等待行文案配置读取或解析失败。
#[derive(Debug)]
pub enum PhrasesConfigError {
    Read {
        path: PathBuf,
        source: io::Error,
    },
    Decode {
        path: PathBuf,
        source: toml::de::Error,
    },
    InvalidMode {
        path: PathBuf,
        value: String,
    },
    InvalidOrder {
        path: PathBuf,
        value: String,
    },
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct FilePhrasesConfig {
    mode: Option<String>,
    order: Option<String>,
    phrases: Option<Vec<String>>,
}

impl fmt::Display for PhrasesConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read { path, source } => {
                write!(f, "read phrase config file {}: {source}", path.display())
            }
            Self::Decode { path, source } => {
                write!(f, "decode phrase config file {}: {source}", path.display())
            }
            Self::InvalidMode { path, value } => write!(
                f,
                "validate phrase config file {}: unknown mode {:?}",
                path.display(),
                value
            ),
            Self::InvalidOrder { path, value } => write!(
                f,
                "validate phrase config file {}: unknown order {:?}",
                path.display(),
                value
            ),
        }
    }
}

impl std::error::Error for PhrasesConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Read { source, .. } => Some(source),
            Self::Decode { source, .. } => Some(source),
            Self::InvalidMode { .. } | Self::InvalidOrder { .. } => None,
        }
    }
}

/// `load_with_resolution` 根据预检阶段决定的数据目录解析结果加载 `phrases.toml`。
///
/// 后加载覆盖先加载。路径由 `DataDirResolution::layered_config_file_paths` 统一决议。
///
/// 错误策略与 `app-config` 一致：
/// - NotFound → 跳过（phrases 可选，缺文件用内置默认）
/// - Read 权限/IO → 降级为 warning，继续下一个源
/// - 全部源均为 Read 错误 → 内置默认 + warnings
/// - Decode/Validation → fatal
pub fn load_with_resolution(
    working_dir: Option<&Path>,
    resolution: &DataDirResolution,
) -> Result<(LoadedStatusPhrases, Vec<PhrasesConfigError>), PhrasesConfigError> {
    let paths = resolution.layered_config_file_paths(working_dir, PHRASES_FILE_NAME);
    load_from_explicit_paths(&paths)
}

/// `load_from_paths` 从指定目录加载并合并等待行文案配置。
pub fn load_from_paths(
    working_dir: Option<&Path>,
    user_config_dir: Option<&Path>,
) -> Result<LoadedStatusPhrases, PhrasesConfigError> {
    let (loaded, _warnings) =
        load_from_explicit_paths(&phrase_config_paths(working_dir, user_config_dir))?;
    Ok(loaded)
}

fn load_from_explicit_paths(
    paths: &[PathBuf],
) -> Result<(LoadedStatusPhrases, Vec<PhrasesConfigError>), PhrasesConfigError> {
    // Default 已含内置短语；Read 失败时保留默认，不 fatal。
    let mut loaded = LoadedStatusPhrases::default();
    let mut saw_config = false;
    let mut warnings = Vec::new();

    for path in paths {
        match read_phrases_config(path) {
            Ok(Some(config)) => {
                merge_phrases_config(&mut loaded, config, path)?;
                loaded.source_path = Some(path.clone());
                saw_config = true;
            }
            // phrases.toml 可选：缺文件用内置 DEFAULT_STATUS_PHRASES。
            Ok(None) => {}
            // 文件级权限/IO：降级为 warning。目录能否用由预检决定。
            Err(error @ PhrasesConfigError::Read { .. }) => warnings.push(error),
            // Decode/Validation：内容错误，立即 fatal。
            Err(other) => return Err(other),
        }
    }

    if !saw_config {
        loaded.source_path = None;
    }
    // override 模式可能把短语清空；保证至少有一条可展示文案。
    if loaded.phrases.is_empty() {
        loaded.phrases.push("Generating".to_string());
    }

    Ok((loaded, warnings))
}

fn default_status_phrases() -> Vec<String> {
    DEFAULT_STATUS_PHRASES
        .iter()
        .map(|phrase| (*phrase).to_string())
        .collect()
}

/// 测试 / 显式路径入口用的搜索列表；生产路径走 `DataDirResolution::layered_config_file_paths`。
fn phrase_config_paths(working_dir: Option<&Path>, user_config_dir: Option<&Path>) -> Vec<PathBuf> {
    let mut paths = Vec::with_capacity(2);
    if let Some(path) = user_config_dir {
        paths.push(path.join(PHRASES_FILE_NAME));
    }
    if let Some(path) = working_dir {
        // 只认 `.hunea/phrases.toml`，不读工作区根 `phrases.toml`（历史错误位置，已废弃）。
        paths.push(path.join(WORKSPACE_HUNEA_DIRNAME).join(PHRASES_FILE_NAME));
    }
    paths
}

fn read_phrases_config(path: &Path) -> Result<Option<FilePhrasesConfig>, PhrasesConfigError> {
    let content = match fs::read_to_string(path) {
        Ok(content) => content,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(PhrasesConfigError::Read {
                path: path.to_path_buf(),
                source,
            });
        }
    };

    let config = toml::from_str(&content).map_err(|source| PhrasesConfigError::Decode {
        path: path.to_path_buf(),
        source,
    })?;
    Ok(Some(config))
}

fn merge_phrases_config(
    target: &mut LoadedStatusPhrases,
    source: FilePhrasesConfig,
    path: &Path,
) -> Result<(), PhrasesConfigError> {
    let mode = match source.mode.as_deref() {
        Some("append") | None => StatusPhraseMode::Append,
        Some("override") => StatusPhraseMode::Override,
        Some(value) => {
            return Err(PhrasesConfigError::InvalidMode {
                path: path.to_path_buf(),
                value: value.to_string(),
            });
        }
    };
    if let Some(order) = source.order {
        target.order = match order.as_str() {
            "random" => StatusPhraseOrder::Random,
            "cycle" => StatusPhraseOrder::Cycle,
            value => {
                return Err(PhrasesConfigError::InvalidOrder {
                    path: path.to_path_buf(),
                    value: value.to_string(),
                });
            }
        };
    }
    if let Some(phrases) = source.phrases {
        let phrases = normalize_phrases(phrases);
        match mode {
            StatusPhraseMode::Append => target.phrases.extend(phrases),
            StatusPhraseMode::Override => target.phrases = phrases,
        }
        target.mode = mode;
    } else {
        target.mode = mode;
    }

    Ok(())
}

fn normalize_phrases(phrases: Vec<String>) -> Vec<String> {
    phrases
        .into_iter()
        .map(|phrase| phrase.trim().to_string())
        .filter(|phrase| !phrase.is_empty())
        .collect()
}
