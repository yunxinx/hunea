use std::{
    env, fmt, fs, io,
    path::{Path, PathBuf},
};

use directories::ProjectDirs;
use serde::Deserialize;

const PHRASES_FILE_NAME: &str = "phrases.toml";

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

/// `load` 从用户配置目录与当前工作目录加载 `phrases.toml`。
pub fn load() -> Result<LoadedStatusPhrases, PhrasesConfigError> {
    let working_dir = env::current_dir().ok();
    load_from_paths(working_dir.as_deref(), user_config_directory().as_deref())
}

/// `load_from_paths` 从指定目录加载并合并等待行文案配置。
pub fn load_from_paths(
    working_dir: Option<&Path>,
    user_config_dir: Option<&Path>,
) -> Result<LoadedStatusPhrases, PhrasesConfigError> {
    let mut loaded = LoadedStatusPhrases::default();
    let mut saw_config = false;

    for path in phrase_config_paths(working_dir, user_config_dir) {
        let Some(config) = read_phrases_config(&path)? else {
            continue;
        };
        merge_phrases_config(&mut loaded, config, &path)?;
        loaded.source_path = Some(path);
        saw_config = true;
    }

    if !saw_config {
        loaded.source_path = None;
    }
    if loaded.phrases.is_empty() {
        loaded.phrases.push("Generating".to_string());
    }

    Ok(loaded)
}

fn default_status_phrases() -> Vec<String> {
    DEFAULT_STATUS_PHRASES
        .iter()
        .map(|phrase| (*phrase).to_string())
        .collect()
}

fn phrase_config_paths(working_dir: Option<&Path>, user_config_dir: Option<&Path>) -> Vec<PathBuf> {
    let mut paths = Vec::with_capacity(3);
    if let Some(path) = user_config_dir {
        paths.push(path.join(PHRASES_FILE_NAME));
    }
    if let Some(path) = working_dir {
        paths.push(path.join(PHRASES_FILE_NAME));
        paths.push(path.join(".hunea").join(PHRASES_FILE_NAME));
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

fn user_config_directory() -> Option<PathBuf> {
    ProjectDirs::from("", "", "hunea").map(|dirs| dirs.config_dir().to_path_buf())
}
