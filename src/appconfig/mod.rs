use std::{
    env, fmt, fs, io,
    path::{Path, PathBuf},
};

use directories::ProjectDirs;
use serde::Deserialize;

use crate::envinfo;

/// `Config` 表示当前 lumos 支持的启动配置。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub tui: TuiConfig,
}

/// `TuiConfig` 表示 TUI 相关配置。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TuiConfig {
    pub user_input_style: UserInputStyle,
    pub status_line: Vec<String>,
    pub external_editor: Vec<String>,
    pub show_external_editor_helper: bool,
    pub copy_on_mouse_selection_release: bool,
    pub swap_enter_and_send: bool,
    pub ctrl_c_clears_input: bool,
}

/// `UserInputStyle` 表示用户输入区与用户消息的展示模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserInputStyle {
    Cx,
    Cc,
    Ms,
}

/// `AppConfigError` 描述配置加载或校验失败。
#[derive(Debug)]
pub enum AppConfigError {
    Read {
        path: PathBuf,
        source: io::Error,
    },
    Decode {
        path: PathBuf,
        source: toml::de::Error,
    },
    InvalidStyleMode {
        path: Option<PathBuf>,
        value: String,
    },
    InvalidStatusLineItem {
        path: Option<PathBuf>,
        value: String,
    },
    InvalidExternalEditorCommand {
        path: Option<PathBuf>,
    },
    ExternalEditorMustWait {
        path: Option<PathBuf>,
        command: String,
    },
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileConfig {
    #[serde(default)]
    tui: FileTuiConfig,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileTuiConfig {
    user_input_style: Option<String>,
    status_line: Option<Vec<String>>,
    external_editor: Option<Vec<String>>,
    show_external_editor_helper: Option<bool>,
    copy_on_mouse_selection_release: Option<bool>,
    swap_enter_and_send: Option<bool>,
    ctrl_c_clears_input: Option<bool>,
}

impl Config {
    fn default_config() -> Self {
        Self {
            tui: TuiConfig {
                user_input_style: UserInputStyle::Cx,
                status_line: Vec::new(),
                external_editor: Vec::new(),
                show_external_editor_helper: true,
                copy_on_mouse_selection_release: false,
                swap_enter_and_send: false,
                ctrl_c_clears_input: true,
            },
        }
    }
}

impl UserInputStyle {
    fn parse(value: &str) -> Result<Self, AppConfigError> {
        match value {
            "cx" => Ok(Self::Cx),
            "cc" => Ok(Self::Cc),
            "ms" => Ok(Self::Ms),
            other => Err(AppConfigError::InvalidStyleMode {
                path: None,
                value: other.to_string(),
            }),
        }
    }
}

impl fmt::Display for AppConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read { path, source } => {
                write!(f, "read config file {}: {source}", path.display())
            }
            Self::Decode { path, source } => {
                write!(f, "decode config file {}: {source}", path.display())
            }
            Self::InvalidStyleMode {
                path: Some(path),
                value,
            } => write!(
                f,
                "validate config file {}: unknown tui.user_input_style {:?}",
                path.display(),
                value
            ),
            Self::InvalidStyleMode { path: None, value } => {
                write!(f, "unknown tui.user_input_style {:?}", value)
            }
            Self::InvalidStatusLineItem {
                path: Some(path),
                value,
            } => write!(
                f,
                "validate config file {}: unknown tui.status_line item {:?}",
                path.display(),
                value
            ),
            Self::InvalidStatusLineItem { path: None, value } => {
                write!(f, "unknown tui.status_line item {:?}", value)
            }
            Self::InvalidExternalEditorCommand { path: Some(path) } => write!(
                f,
                "validate config file {}: invalid tui.external_editor command",
                path.display()
            ),
            Self::InvalidExternalEditorCommand { path: None } => {
                write!(f, "invalid tui.external_editor command")
            }
            Self::ExternalEditorMustWait {
                path: Some(path),
                command,
            } => write!(
                f,
                "validate config file {}: external editor must wait for close: {}",
                path.display(),
                command
            ),
            Self::ExternalEditorMustWait {
                path: None,
                command,
            } => {
                write!(f, "external editor must wait for close: {command}")
            }
        }
    }
}

impl std::error::Error for AppConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Read { source, .. } => Some(source),
            Self::Decode { source, .. } => Some(source),
            Self::InvalidStyleMode { .. }
            | Self::InvalidStatusLineItem { .. }
            | Self::InvalidExternalEditorCommand { .. }
            | Self::ExternalEditorMustWait { .. } => None,
        }
    }
}

/// `load` 按“用户级配置 -> 当前目录覆盖”的顺序加载配置。
pub fn load() -> Result<Config, AppConfigError> {
    load_with_lookups(env::current_dir, user_config_directory)
}

/// `load_from_paths` 使用给定目录快照加载配置，便于测试与非标准启动入口复用。
pub fn load_from_paths(
    working_dir: Option<&Path>,
    user_config_dir: Option<&Path>,
) -> Result<Config, AppConfigError> {
    load_from_base_config(
        Config::default_config(),
        working_dir.map(Path::to_path_buf),
        user_config_dir.map(Path::to_path_buf),
    )
}

fn load_with_lookups(
    get_working_dir: impl FnOnce() -> io::Result<PathBuf>,
    get_user_config_dir: impl FnOnce() -> Option<PathBuf>,
) -> Result<Config, AppConfigError> {
    let mut config = Config::default_config();
    let working_dir = match get_working_dir() {
        Ok(path) => Some(path),
        Err(_) => {
            config.tui.user_input_style = UserInputStyle::Ms;
            None
        }
    };

    load_from_base_config(config, working_dir, get_user_config_dir())
}

fn load_from_base_config(
    mut config: Config,
    working_dir: Option<PathBuf>,
    user_config_dir: Option<PathBuf>,
) -> Result<Config, AppConfigError> {
    let mut config_paths = Vec::with_capacity(2);
    if let Some(path) = user_config_dir {
        config_paths.push(path.join("config.toml"));
    }
    if let Some(path) = working_dir {
        config_paths.push(path.join(".lumos").join("config.toml"));
    }

    for path in config_paths {
        config = merge_config_file(config, &path)?;
    }

    Ok(config)
}

fn merge_config_file(mut config: Config, path: &Path) -> Result<Config, AppConfigError> {
    let content = match fs::read_to_string(path) {
        Ok(content) => content,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(config),
        Err(source) => {
            return Err(AppConfigError::Read {
                path: path.to_path_buf(),
                source,
            });
        }
    };

    let file_config: FileConfig =
        toml::from_str(&content).map_err(|source| AppConfigError::Decode {
            path: path.to_path_buf(),
            source,
        })?;

    if let Some(style) = file_config.tui.user_input_style {
        config.tui.user_input_style =
            UserInputStyle::parse(&style).map_err(|error| match error {
                AppConfigError::InvalidStyleMode { value, .. } => {
                    AppConfigError::InvalidStyleMode {
                        path: Some(path.to_path_buf()),
                        value,
                    }
                }
                other => other,
            })?;
    }

    if let Some(items) = file_config.tui.status_line {
        validate_status_line_items(&items).map_err(|error| match error {
            AppConfigError::InvalidStatusLineItem { value, .. } => {
                AppConfigError::InvalidStatusLineItem {
                    path: Some(path.to_path_buf()),
                    value,
                }
            }
            other => other,
        })?;
        config.tui.status_line = items;
    }

    if let Some(command) = file_config.tui.external_editor {
        validate_external_editor(&command).map_err(|error| match error {
            AppConfigError::InvalidExternalEditorCommand { .. } => {
                AppConfigError::InvalidExternalEditorCommand {
                    path: Some(path.to_path_buf()),
                }
            }
            AppConfigError::ExternalEditorMustWait { command, .. } => {
                AppConfigError::ExternalEditorMustWait {
                    path: Some(path.to_path_buf()),
                    command,
                }
            }
            other => other,
        })?;
        config.tui.external_editor = command;
    }

    if let Some(show_helper) = file_config.tui.show_external_editor_helper {
        config.tui.show_external_editor_helper = show_helper;
    }

    if let Some(copy_on_release) = file_config.tui.copy_on_mouse_selection_release {
        config.tui.copy_on_mouse_selection_release = copy_on_release;
    }

    if let Some(swap_enter_and_send) = file_config.tui.swap_enter_and_send {
        config.tui.swap_enter_and_send = swap_enter_and_send;
    }

    if let Some(ctrl_c_clears_input) = file_config.tui.ctrl_c_clears_input {
        config.tui.ctrl_c_clears_input = ctrl_c_clears_input;
    }

    Ok(config)
}

fn user_config_directory() -> Option<PathBuf> {
    ProjectDirs::from("", "", "lumos").map(|dirs| dirs.config_dir().to_path_buf())
}

fn validate_status_line_items(items: &[String]) -> Result<(), AppConfigError> {
    for item in items {
        match item.as_str() {
            "git-branch" | "current-dir" => {}
            other => {
                return Err(AppConfigError::InvalidStatusLineItem {
                    path: None,
                    value: other.to_string(),
                });
            }
        }
    }

    Ok(())
}

fn validate_external_editor(command: &[String]) -> Result<(), AppConfigError> {
    if command.is_empty() {
        return Ok(());
    }

    if command[0].trim().is_empty() {
        return Err(AppConfigError::InvalidExternalEditorCommand { path: None });
    }

    envinfo::validate_configured_external_editor(command).map_err(|error| match error {
        envinfo::ExternalEditorError::ExternalEditorMustWait { command } => {
            AppConfigError::ExternalEditorMustWait {
                path: None,
                command,
            }
        }
        _ => AppConfigError::InvalidExternalEditorCommand { path: None },
    })
}

#[cfg(test)]
mod tests {
    use super::{UserInputStyle, load_from_paths, load_with_lookups};
    use std::{
        fs, io,
        path::{Path, PathBuf},
    };

    #[test]
    fn load_defaults_to_cx_when_no_config_exists() {
        let working_dir = temp_test_dir("load-defaults-working");
        let user_config_dir = temp_test_dir("load-defaults-config");

        let config = load_from_paths(Some(working_dir.as_path()), Some(user_config_dir.as_path()))
            .expect("missing config files should fall back to defaults");

        assert_eq!(config.tui.user_input_style, UserInputStyle::Cx);
    }

    #[test]
    fn load_project_config_overrides_user_config() {
        let working_dir = temp_test_dir("load-project-overrides-working");
        let user_config_dir = temp_test_dir("load-project-overrides-config");
        write_config(
            &user_config_dir.join("config.toml"),
            "[tui]\nuser_input_style = \"ms\"\n",
        );
        write_config(
            &working_dir.join(".lumos").join("config.toml"),
            "[tui]\nuser_input_style = \"cx\"\n",
        );

        let config = load_from_paths(Some(working_dir.as_path()), Some(user_config_dir.as_path()))
            .expect("project config should override the user config");

        assert_eq!(config.tui.user_input_style, UserInputStyle::Cx);
    }

    #[test]
    fn load_accepts_cc_style_mode() {
        let working_dir = temp_test_dir("load-accepts-cc-working");
        write_config(
            &working_dir.join(".lumos").join("config.toml"),
            "[tui]\nuser_input_style = \"cc\"\n",
        );

        let config = load_from_paths(Some(working_dir.as_path()), None)
            .expect("cc should be accepted as a valid style mode");

        assert_eq!(config.tui.user_input_style, UserInputStyle::Cc);
    }

    #[test]
    fn load_rejects_unknown_style_mode() {
        let working_dir = temp_test_dir("load-rejects-style-working");
        write_config(
            &working_dir.join(".lumos").join("config.toml"),
            "[tui]\nuser_input_style = \"weird\"\n",
        );

        let error = load_from_paths(Some(working_dir.as_path()), None)
            .expect_err("unknown style mode should be rejected");

        assert!(
            error.to_string().contains("unknown tui.user_input_style"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn load_rejects_unknown_keys() {
        let working_dir = temp_test_dir("load-rejects-keys-working");
        write_config(
            &working_dir.join(".lumos").join("config.toml"),
            "[tui]\nunknown = true\n",
        );

        let error = load_from_paths(Some(working_dir.as_path()), None)
            .expect_err("unknown keys should fail");

        assert!(
            error.to_string().contains("unknown field"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn load_falls_back_to_ms_when_working_directory_lookup_fails() {
        let config = load_with_lookups(
            || Err(io::Error::other("working directory unavailable")),
            || None,
        )
        .expect("missing working dir should fall back to ms");

        assert_eq!(config.tui.user_input_style, UserInputStyle::Ms);
    }

    #[test]
    fn load_still_uses_user_config_when_working_directory_lookup_fails() {
        let user_config_dir = temp_test_dir("load-user-config-after-cwd-failure");
        write_config(
            &user_config_dir.join("config.toml"),
            "[tui]\nuser_input_style = \"cc\"\n",
        );

        let config = load_with_lookups(
            || Err(io::Error::other("working directory unavailable")),
            || Some(user_config_dir.clone()),
        )
        .expect("user config should still be used");

        assert_eq!(config.tui.user_input_style, UserInputStyle::Cc);
    }

    fn temp_test_dir(prefix: &str) -> PathBuf {
        let unique = format!(
            "{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system time should be after unix epoch")
                .as_nanos()
        );
        let path = std::env::temp_dir().join(format!("lumos-rust-{prefix}-{unique}"));
        fs::create_dir_all(&path).expect("temp test dir should be created");
        path
    }

    fn write_config(path: &Path, content: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("config parent dir should exist");
        }
        fs::write(path, content).expect("config file should be written");
    }
}
