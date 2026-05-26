use std::{
    env, fmt, fs, io,
    path::{Path, PathBuf},
};

use directories::ProjectDirs;
use serde::Deserialize;

use runtime_domain::{envinfo, session::ManagedSearchTool};

/// @ 文件选择浮窗至少需要 3 行，避免列表在导航时过于局促。
pub const FILE_PICKER_POPUP_MIN_HEIGHT: u16 = 3;
/// @ 文件选择浮窗最多显示 21 行，避免覆盖过多上下文。
pub const FILE_PICKER_POPUP_MAX_HEIGHT: u16 = 21;

/// `Config` 表示当前 lumos 支持的启动配置。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub tui: TuiConfig,
    pub runtime: RuntimeConfig,
    pub debug: DebugConfig,
}

/// `TuiConfig` 表示 TUI 相关配置。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TuiConfig {
    pub user_input_style: UserInputStyle,
    pub status_line: Vec<String>,
    pub status_line_2: Vec<String>,
    pub external_editor: Vec<String>,
    pub show_external_editor_helper: bool,
    pub copy_on_mouse_selection_release: bool,
    pub swap_enter_and_send: bool,
    pub ctrl_c_clears_input: bool,
    pub esc_interrupt_presses: u8,
    pub show_esc_interrupt_hint: bool,
    pub file_picker_popup_height: u16,
    pub print_transcript_on_exit: bool,
    pub show_reasoning_content: bool,
    pub reasoning_content_display: ReasoningContentDisplay,
}

/// `DebugConfig` 表示仅用于本地调试与界面预览的配置。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DebugConfig {
    pub enabled: bool,
}

/// `UserInputStyle` 表示用户输入区与用户消息的展示模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserInputStyle {
    Cx,
    Cc,
    Ms,
}

/// `ReasoningContentDisplay` 表示思维链内容的默认展示方式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReasoningContentDisplay {
    Collapsed,
    Expanded,
    Snippet,
}

/// `RuntimeConfig` 表示可被多个 runtime 复用的执行策略。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeConfig {
    pub request_retry_attempts: usize,
    pub request_retry_delays: Vec<u64>,
    pub request_timeout_seconds: u64,
    pub tool_max_turns: Option<usize>,
    pub allow_managed_rg: Option<bool>,
    pub allow_managed_fd: Option<bool>,
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
    Edit {
        path: PathBuf,
        source: toml_edit::TomlError,
    },
    Write {
        path: PathBuf,
        source: io::Error,
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
    InvalidEscInterruptPresses {
        path: Option<PathBuf>,
        value: u8,
    },
    InvalidFilePickerPopupHeight {
        path: Option<PathBuf>,
        value: usize,
    },
    InvalidReasoningContentDisplay {
        path: Option<PathBuf>,
        value: String,
    },
    InvalidRuntimeRequestPolicy {
        path: Option<PathBuf>,
        reason: String,
    },
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileConfig {
    #[serde(default)]
    tui: FileTuiConfig,
    #[serde(default)]
    runtime: FileRuntimeConfig,
    #[serde(default)]
    debug: FileDebugConfig,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileTuiConfig {
    user_input_style: Option<String>,
    status_line: Option<Vec<String>>,
    status_line_2: Option<Vec<String>>,
    external_editor: Option<Vec<String>>,
    show_external_editor_helper: Option<bool>,
    copy_on_mouse_selection_release: Option<bool>,
    swap_enter_and_send: Option<bool>,
    ctrl_c_clears_input: Option<bool>,
    esc_interrupt_presses: Option<u8>,
    show_esc_interrupt_hint: Option<bool>,
    file_picker_popup_height: Option<usize>,
    print_transcript_on_exit: Option<bool>,
    show_reasoning_content: Option<bool>,
    reasoning_content_display: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileRuntimeConfig {
    request_retry_attempts: Option<usize>,
    request_retry_delays: Option<Vec<u64>>,
    request_timeout_seconds: Option<u64>,
    tool_max_turns: Option<usize>,
    allow_managed_rg: Option<bool>,
    allow_managed_fd: Option<bool>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileDebugConfig {
    enabled: Option<bool>,
}

impl Config {
    fn default_config() -> Self {
        Self {
            tui: TuiConfig {
                user_input_style: UserInputStyle::Cx,
                status_line: Vec::new(),
                status_line_2: Vec::new(),
                external_editor: Vec::new(),
                show_external_editor_helper: true,
                copy_on_mouse_selection_release: false,
                swap_enter_and_send: false,
                ctrl_c_clears_input: true,
                esc_interrupt_presses: 2,
                show_esc_interrupt_hint: true,
                file_picker_popup_height: 7,
                print_transcript_on_exit: false,
                show_reasoning_content: false,
                reasoning_content_display: ReasoningContentDisplay::Collapsed,
            },
            runtime: RuntimeConfig {
                request_retry_attempts: 3,
                request_retry_delays: vec![1, 2, 3],
                request_timeout_seconds: 120,
                tool_max_turns: None,
                allow_managed_rg: None,
                allow_managed_fd: None,
            },
            debug: DebugConfig { enabled: false },
        }
    }
}

impl ReasoningContentDisplay {
    fn parse(value: &str) -> Result<Self, AppConfigError> {
        match value {
            "collapsed" => Ok(Self::Collapsed),
            "expanded" => Ok(Self::Expanded),
            "snippet" => Ok(Self::Snippet),
            other => Err(AppConfigError::InvalidReasoningContentDisplay {
                path: None,
                value: other.to_string(),
            }),
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
            Self::Edit { path, source } => {
                write!(f, "edit config file {}: {source}", path.display())
            }
            Self::Write { path, source } => {
                write!(f, "write config file {}: {source}", path.display())
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
            Self::InvalidEscInterruptPresses {
                path: Some(path),
                value,
            } => write!(
                f,
                "validate config file {}: tui.esc_interrupt_presses must be 1, 2, or 3, got {}",
                path.display(),
                value
            ),
            Self::InvalidEscInterruptPresses { path: None, value } => write!(
                f,
                "tui.esc_interrupt_presses must be 1, 2, or 3, got {value}"
            ),
            Self::InvalidFilePickerPopupHeight {
                path: Some(path),
                value,
            } => write!(
                f,
                "validate config file {}: tui.file_picker_popup_height must be between {} and {}, got {}",
                path.display(),
                FILE_PICKER_POPUP_MIN_HEIGHT,
                FILE_PICKER_POPUP_MAX_HEIGHT,
                value
            ),
            Self::InvalidFilePickerPopupHeight { path: None, value } => write!(
                f,
                "tui.file_picker_popup_height must be between {} and {}, got {value}",
                FILE_PICKER_POPUP_MIN_HEIGHT, FILE_PICKER_POPUP_MAX_HEIGHT
            ),
            Self::InvalidReasoningContentDisplay {
                path: Some(path),
                value,
            } => write!(
                f,
                "validate config file {}: unknown tui.reasoning_content_display {:?}",
                path.display(),
                value
            ),
            Self::InvalidReasoningContentDisplay { path: None, value } => {
                write!(f, "unknown tui.reasoning_content_display {:?}", value)
            }
            Self::InvalidRuntimeRequestPolicy {
                path: Some(path),
                reason,
            } => write!(
                f,
                "validate config file {}: invalid runtime.request policy: {}",
                path.display(),
                reason
            ),
            Self::InvalidRuntimeRequestPolicy { path: None, reason } => {
                write!(f, "invalid runtime.request policy: {reason}")
            }
        }
    }
}

impl std::error::Error for AppConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Read { source, .. } => Some(source),
            Self::Decode { source, .. } => Some(source),
            Self::Edit { source, .. } => Some(source),
            Self::Write { source, .. } => Some(source),
            Self::InvalidStyleMode { .. }
            | Self::InvalidStatusLineItem { .. }
            | Self::InvalidExternalEditorCommand { .. }
            | Self::ExternalEditorMustWait { .. }
            | Self::InvalidEscInterruptPresses { .. }
            | Self::InvalidFilePickerPopupHeight { .. }
            | Self::InvalidReasoningContentDisplay { .. }
            | Self::InvalidRuntimeRequestPolicy { .. } => None,
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

/// `user_config_file_path` 返回用户级 `config.toml` 的默认写入位置。
pub fn user_config_file_path() -> Option<PathBuf> {
    user_config_directory().map(|path| path.join("config.toml"))
}

/// `persist_managed_search_tool_authorization_to_path` 将受管搜索工具授权写入指定配置文件。
pub fn persist_managed_search_tool_authorization_to_path(
    path: &Path,
    tool: ManagedSearchTool,
) -> Result<(), AppConfigError> {
    let content = match fs::read_to_string(path) {
        Ok(content) => content,
        Err(error) if error.kind() == io::ErrorKind::NotFound => String::new(),
        Err(error) => {
            return Err(AppConfigError::Read {
                path: path.to_path_buf(),
                source: error,
            });
        }
    };
    let mut document =
        content
            .parse::<toml_edit::DocumentMut>()
            .map_err(|error| AppConfigError::Edit {
                path: path.to_path_buf(),
                source: error,
            })?;
    document["runtime"][managed_search_tool_authorization_field(tool)] = toml_edit::value(true);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| AppConfigError::Write {
            path: path.to_path_buf(),
            source: error,
        })?;
    }
    fs::write(path, document.to_string()).map_err(|error| AppConfigError::Write {
        path: path.to_path_buf(),
        source: error,
    })
}

fn managed_search_tool_authorization_field(tool: ManagedSearchTool) -> &'static str {
    match tool {
        ManagedSearchTool::Ripgrep => "allow_managed_rg",
        ManagedSearchTool::Fd => "allow_managed_fd",
    }
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
    if let Some(path) = working_dir.as_ref() {
        config_paths.push(path.join(".lumos").join("config.toml"));
    }

    let mut reasoning_content_display_configured = false;
    for path in config_paths {
        config = merge_config_file(config, &path, &mut reasoning_content_display_configured)?;
    }

    Ok(config)
}

fn merge_config_file(
    mut config: Config,
    path: &Path,
    reasoning_content_display_configured: &mut bool,
) -> Result<Config, AppConfigError> {
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
    let enables_reasoning_without_display =
        matches!(file_config.tui.show_reasoning_content, Some(true))
            && file_config.tui.reasoning_content_display.is_none();

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
        validate_status_line_items_for_path(&items, path)?;
        config.tui.status_line = items;
    }

    if let Some(items) = file_config.tui.status_line_2 {
        validate_status_line_items_for_path(&items, path)?;
        config.tui.status_line_2 = items;
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

    if let Some(esc_interrupt_presses) = file_config.tui.esc_interrupt_presses {
        if !(1..=3).contains(&esc_interrupt_presses) {
            return Err(AppConfigError::InvalidEscInterruptPresses {
                path: Some(path.to_path_buf()),
                value: esc_interrupt_presses,
            });
        }
        config.tui.esc_interrupt_presses = esc_interrupt_presses;
    }

    if let Some(show_esc_interrupt_hint) = file_config.tui.show_esc_interrupt_hint {
        config.tui.show_esc_interrupt_hint = show_esc_interrupt_hint;
    }

    if let Some(height) = file_config.tui.file_picker_popup_height {
        config.tui.file_picker_popup_height = validate_file_picker_popup_height(height, path)?;
    }

    if let Some(print_transcript_on_exit) = file_config.tui.print_transcript_on_exit {
        config.tui.print_transcript_on_exit = print_transcript_on_exit;
    }

    if let Some(show_reasoning_content) = file_config.tui.show_reasoning_content {
        config.tui.show_reasoning_content = show_reasoning_content;
    }

    if let Some(reasoning_content_display) = file_config.tui.reasoning_content_display {
        config.tui.reasoning_content_display = ReasoningContentDisplay::parse(
            &reasoning_content_display,
        )
        .map_err(|error| match error {
            AppConfigError::InvalidReasoningContentDisplay { value, .. } => {
                AppConfigError::InvalidReasoningContentDisplay {
                    path: Some(path.to_path_buf()),
                    value,
                }
            }
            other => other,
        })?;
        *reasoning_content_display_configured = true;
    } else if enables_reasoning_without_display && !*reasoning_content_display_configured {
        config.tui.reasoning_content_display = ReasoningContentDisplay::Expanded;
    }

    merge_runtime_config(&mut config.runtime, file_config.runtime, path)?;

    if let Some(enabled) = file_config.debug.enabled {
        config.debug.enabled = enabled;
    }

    Ok(config)
}

fn validate_status_line_items_for_path(
    items: &[String],
    path: &Path,
) -> Result<(), AppConfigError> {
    validate_status_line_items(items).map_err(|error| match error {
        AppConfigError::InvalidStatusLineItem { value, .. } => {
            AppConfigError::InvalidStatusLineItem {
                path: Some(path.to_path_buf()),
                value,
            }
        }
        other => other,
    })
}

fn merge_runtime_config(
    config: &mut RuntimeConfig,
    file_config: FileRuntimeConfig,
    path: &Path,
) -> Result<(), AppConfigError> {
    if file_config.request_retry_attempts.is_none()
        && file_config.request_retry_delays.is_none()
        && file_config.request_timeout_seconds.is_none()
        && file_config.tool_max_turns.is_none()
        && file_config.allow_managed_rg.is_none()
        && file_config.allow_managed_fd.is_none()
    {
        return Ok(());
    }

    let has_explicit_delays = file_config.request_retry_delays.is_some();
    let attempts = match file_config.request_retry_attempts {
        Some(attempts) => attempts,
        None => file_config
            .request_retry_delays
            .as_ref()
            .map(Vec::len)
            .unwrap_or(config.request_retry_attempts),
    };
    validate_request_retry_attempts(attempts, path)?;

    let mut delays = file_config
        .request_retry_delays
        .unwrap_or_else(|| config.request_retry_delays.clone());
    normalize_request_retry_delays(&mut delays, attempts, has_explicit_delays, path)?;

    let timeout_seconds = file_config
        .request_timeout_seconds
        .unwrap_or(config.request_timeout_seconds);
    validate_request_timeout_seconds(timeout_seconds, path)?;

    let tool_max_turns = file_config.tool_max_turns.or(config.tool_max_turns);
    if let Some(tool_max_turns) = tool_max_turns {
        validate_tool_max_turns(tool_max_turns, path)?;
    }

    config.request_retry_attempts = attempts;
    config.request_retry_delays = delays;
    config.request_timeout_seconds = timeout_seconds;
    config.tool_max_turns = tool_max_turns;
    if let Some(allow_managed_rg) = file_config.allow_managed_rg {
        config.allow_managed_rg = Some(allow_managed_rg);
    }
    if let Some(allow_managed_fd) = file_config.allow_managed_fd {
        config.allow_managed_fd = Some(allow_managed_fd);
    }
    Ok(())
}

fn validate_request_retry_attempts(attempts: usize, path: &Path) -> Result<(), AppConfigError> {
    if (1..=10).contains(&attempts) {
        return Ok(());
    }

    Err(AppConfigError::InvalidRuntimeRequestPolicy {
        path: Some(path.to_path_buf()),
        reason: format!("runtime.request_retry_attempts must be between 1 and 10, got {attempts}"),
    })
}

fn validate_file_picker_popup_height(value: usize, path: &Path) -> Result<u16, AppConfigError> {
    if !(usize::from(FILE_PICKER_POPUP_MIN_HEIGHT)..=usize::from(FILE_PICKER_POPUP_MAX_HEIGHT))
        .contains(&value)
    {
        return Err(AppConfigError::InvalidFilePickerPopupHeight {
            path: Some(path.to_path_buf()),
            value,
        });
    }

    Ok(value as u16)
}

fn validate_request_timeout_seconds(
    timeout_seconds: u64,
    path: &Path,
) -> Result<(), AppConfigError> {
    if (1..=7200).contains(&timeout_seconds) {
        return Ok(());
    }

    Err(AppConfigError::InvalidRuntimeRequestPolicy {
        path: Some(path.to_path_buf()),
        reason: format!(
            "runtime.request_timeout_seconds must be between 1 and 7200, got {timeout_seconds}"
        ),
    })
}

fn validate_tool_max_turns(tool_max_turns: usize, path: &Path) -> Result<(), AppConfigError> {
    if tool_max_turns > 0 {
        return Ok(());
    }

    Err(AppConfigError::InvalidRuntimeRequestPolicy {
        path: Some(path.to_path_buf()),
        reason: "runtime.tool_max_turns must be at least 1 when configured".to_string(),
    })
}

fn normalize_request_retry_delays(
    delays: &mut Vec<u64>,
    attempts: usize,
    has_explicit_delays: bool,
    path: &Path,
) -> Result<(), AppConfigError> {
    if delays.is_empty() {
        return Err(AppConfigError::InvalidRuntimeRequestPolicy {
            path: Some(path.to_path_buf()),
            reason: "runtime.request_retry_delays must not be empty".to_string(),
        });
    }

    if let Some(delay) = delays.iter().find(|delay| !(1..=1800).contains(*delay)) {
        return Err(AppConfigError::InvalidRuntimeRequestPolicy {
            path: Some(path.to_path_buf()),
            reason: format!(
                "runtime.request_retry_delays items must be between 1 and 1800 seconds, got {delay}"
            ),
        });
    }

    if delays.len() > attempts && has_explicit_delays {
        return Err(AppConfigError::InvalidRuntimeRequestPolicy {
            path: Some(path.to_path_buf()),
            reason: format!(
                "runtime.request_retry_delays has {} items but runtime.request_retry_attempts is {attempts}",
                delays.len()
            ),
        });
    }

    delays.truncate(attempts);

    if delays.len() < attempts {
        let last_delay = *delays
            .last()
            .expect("empty retry delay list is rejected before extension");
        delays.resize(attempts, last_delay);
    }

    Ok(())
}

fn user_config_directory() -> Option<PathBuf> {
    ProjectDirs::from("", "", "lumos").map(|dirs| dirs.config_dir().to_path_buf())
}

fn validate_status_line_items(items: &[String]) -> Result<(), AppConfigError> {
    for item in items {
        match item.as_str() {
            "git-branch" | "current-dir" | "current-model" | "throughput" | "latency" => {}
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
