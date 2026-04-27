use std::{
    collections::BTreeMap,
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
    pub acp: AcpConfig,
}

/// `LoadedConfig` 保留配置内容以及 ACP 字段来自哪个配置文件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedConfig {
    pub config: Config,
    pub acp_source: Option<PathBuf>,
    pub user_acp_path: Option<PathBuf>,
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
    pub esc_interrupt_presses: u8,
    pub show_esc_interrupt_hint: bool,
    pub print_transcript_on_exit: bool,
}

/// `UserInputStyle` 表示用户输入区与用户消息的展示模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserInputStyle {
    Cx,
    Cc,
    Ms,
}

/// `AcpConfig` 表示 ACP 层的启动配置。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcpConfig {
    pub enabled: bool,
    pub registry_url: String,
    pub install_root: AcpInstallRoot,
    pub custom_install_dir: PathBuf,
    pub distribution_preference: Vec<AcpDistribution>,
    pub auto_update_check: bool,
    pub agent_servers: BTreeMap<String, AgentServerConfig>,
}

/// `AgentServerConfig` 表示单个 ACP agent server 的来源与启动覆盖配置。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentServerConfig {
    pub server_type: AgentServerType,
    pub agent: String,
    pub command: String,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub default_model: Option<String>,
    pub default_mode: Option<String>,
}

/// `AgentServerType` 表示 agent server 的来源类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentServerType {
    Registry,
    Custom,
}

/// `AcpInstallRoot` 表示 ACP 包安装位置策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcpInstallRoot {
    Config,
    Data,
    Cache,
    Project,
    Custom,
}

/// `AcpDistribution` 表示 registry 分发类型偏好。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcpDistribution {
    Binary,
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
    InvalidAgentServerType {
        path: Option<PathBuf>,
        server: String,
        value: String,
    },
    InvalidAcpInstallRoot {
        path: Option<PathBuf>,
        value: String,
    },
    InvalidAcpDistribution {
        path: Option<PathBuf>,
        value: String,
    },
    InvalidEscInterruptPresses {
        path: Option<PathBuf>,
        value: u8,
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
    esc_interrupt_presses: Option<u8>,
    show_esc_interrupt_hint: Option<bool>,
    print_transcript_on_exit: Option<bool>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileAcpConfig {
    enabled: Option<bool>,
    registry_url: Option<String>,
    install_root: Option<String>,
    custom_install_dir: Option<PathBuf>,
    distribution_preference: Option<Vec<String>>,
    auto_update_check: Option<bool>,
    #[serde(default)]
    agent_servers: BTreeMap<String, FileAgentServerConfig>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileAgentServerConfig {
    #[serde(rename = "type")]
    server_type: Option<String>,
    agent: Option<String>,
    command: Option<String>,
    args: Option<Vec<String>>,
    env: Option<BTreeMap<String, String>>,
    default_model: Option<String>,
    default_mode: Option<String>,
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
                esc_interrupt_presses: 2,
                show_esc_interrupt_hint: true,
                print_transcript_on_exit: false,
            },
            acp: AcpConfig {
                enabled: false,
                registry_url:
                    "https://cdn.agentclientprotocol.com/registry/v1/latest/registry.json"
                        .to_string(),
                install_root: AcpInstallRoot::Config,
                custom_install_dir: PathBuf::new(),
                distribution_preference: vec![AcpDistribution::Binary],
                auto_update_check: true,
                agent_servers: BTreeMap::new(),
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

impl AgentServerConfig {
    fn new(server_id: &str) -> Self {
        Self {
            server_type: AgentServerType::Registry,
            agent: server_id.to_string(),
            command: String::new(),
            args: Vec::new(),
            env: BTreeMap::new(),
            default_model: None,
            default_mode: None,
        }
    }
}

impl AgentServerType {
    fn parse(value: &str) -> Result<Self, AppConfigError> {
        match value {
            "registry" => Ok(Self::Registry),
            "custom" => Ok(Self::Custom),
            other => Err(AppConfigError::InvalidAgentServerType {
                path: None,
                server: String::new(),
                value: other.to_string(),
            }),
        }
    }
}

impl AcpInstallRoot {
    fn parse(value: &str) -> Result<Self, AppConfigError> {
        match value {
            "config" => Ok(Self::Config),
            "data" => Ok(Self::Data),
            "cache" => Ok(Self::Cache),
            "project" => Ok(Self::Project),
            "custom" => Ok(Self::Custom),
            other => Err(AppConfigError::InvalidAcpInstallRoot {
                path: None,
                value: other.to_string(),
            }),
        }
    }
}

impl AcpDistribution {
    fn parse(value: &str) -> Result<Self, AppConfigError> {
        match value {
            "binary" => Ok(Self::Binary),
            other => Err(AppConfigError::InvalidAcpDistribution {
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
            Self::InvalidAgentServerType {
                path: Some(path),
                server,
                value,
            } => write!(
                f,
                "validate config file {}: unknown agent_servers.{}.type {:?}",
                path.display(),
                server,
                value
            ),
            Self::InvalidAgentServerType {
                path: None,
                server,
                value,
            } => write!(f, "unknown agent_servers.{}.type {:?}", server, value),
            Self::InvalidAcpInstallRoot {
                path: Some(path),
                value,
            } => write!(
                f,
                "validate config file {}: unknown acp.install_root {:?}",
                path.display(),
                value
            ),
            Self::InvalidAcpInstallRoot { path: None, value } => {
                write!(f, "unknown acp.install_root {:?}", value)
            }
            Self::InvalidAcpDistribution {
                path: Some(path),
                value,
            } => write!(
                f,
                "validate config file {}: unknown acp.distribution_preference item {:?}",
                path.display(),
                value
            ),
            Self::InvalidAcpDistribution { path: None, value } => {
                write!(f, "unknown acp.distribution_preference item {:?}", value)
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
            | Self::InvalidAgentServerType { .. }
            | Self::InvalidAcpInstallRoot { .. }
            | Self::InvalidAcpDistribution { .. }
            | Self::InvalidEscInterruptPresses { .. } => None,
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
    .map(|loaded| loaded.config)
}

/// `load_with_sources_from_paths` 加载配置并保留 acp 来源文件，便于后续写回。
pub fn load_with_sources_from_paths(
    working_dir: Option<&Path>,
    user_config_dir: Option<&Path>,
) -> Result<LoadedConfig, AppConfigError> {
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

    load_from_base_config(config, working_dir, get_user_config_dir()).map(|loaded| loaded.config)
}

fn load_from_base_config(
    mut config: Config,
    working_dir: Option<PathBuf>,
    user_config_dir: Option<PathBuf>,
) -> Result<LoadedConfig, AppConfigError> {
    let mut config_paths = Vec::with_capacity(2);
    let user_acp_path = user_config_dir.as_ref().map(|path| path.join("acp.toml"));
    if let Some(path) = user_config_dir {
        config_paths.push(path.join("config.toml"));
    }
    if let Some(path) = working_dir.as_ref() {
        config_paths.push(path.join(".lumos").join("config.toml"));
    }

    for path in config_paths {
        config = merge_config_file(config, &path)?;
    }

    let mut acp_source = None;
    if let Some(path) = user_acp_path.clone() {
        let merge_result = merge_acp_config_file(&mut config.acp, &path)?;
        if merge_result.has_acp {
            acp_source = Some(path);
        }
    }
    if let Some(path) = working_dir.as_ref() {
        let path = path.join(".lumos").join("acp.toml");
        let merge_result = merge_acp_config_file(&mut config.acp, &path)?;
        if merge_result.has_acp {
            acp_source = Some(path);
        }
    }

    Ok(LoadedConfig {
        config,
        acp_source,
        user_acp_path,
    })
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

    if let Some(print_transcript_on_exit) = file_config.tui.print_transcript_on_exit {
        config.tui.print_transcript_on_exit = print_transcript_on_exit;
    }

    Ok(config)
}

struct AcpConfigMergeResult {
    has_acp: bool,
}

fn merge_acp_config_file(
    config: &mut AcpConfig,
    path: &Path,
) -> Result<AcpConfigMergeResult, AppConfigError> {
    let content = match fs::read_to_string(path) {
        Ok(content) => content,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(AcpConfigMergeResult { has_acp: false });
        }
        Err(source) => {
            return Err(AppConfigError::Read {
                path: path.to_path_buf(),
                source,
            });
        }
    };

    let file_config: FileAcpConfig =
        toml::from_str(&content).map_err(|source| AppConfigError::Decode {
            path: path.to_path_buf(),
            source,
        })?;
    merge_acp_config(config, file_config, path)?;

    Ok(AcpConfigMergeResult { has_acp: true })
}

fn merge_acp_config(
    config: &mut AcpConfig,
    file_config: FileAcpConfig,
    path: &Path,
) -> Result<(), AppConfigError> {
    if let Some(enabled) = file_config.enabled {
        config.enabled = enabled;
    }

    if let Some(registry_url) = file_config.registry_url {
        config.registry_url = registry_url;
    }

    if let Some(install_root) = file_config.install_root {
        config.install_root =
            AcpInstallRoot::parse(&install_root).map_err(|error| match error {
                AppConfigError::InvalidAcpInstallRoot { value, .. } => {
                    AppConfigError::InvalidAcpInstallRoot {
                        path: Some(path.to_path_buf()),
                        value,
                    }
                }
                other => other,
            })?;
    }

    if let Some(custom_install_dir) = file_config.custom_install_dir {
        config.custom_install_dir = custom_install_dir;
    }

    if let Some(preference) = file_config.distribution_preference {
        let mut parsed = Vec::with_capacity(preference.len());
        for item in preference {
            parsed.push(AcpDistribution::parse(&item).map_err(|error| match error {
                AppConfigError::InvalidAcpDistribution { value, .. } => {
                    AppConfigError::InvalidAcpDistribution {
                        path: Some(path.to_path_buf()),
                        value,
                    }
                }
                other => other,
            })?);
        }
        config.distribution_preference = parsed;
    }

    if let Some(auto_update_check) = file_config.auto_update_check {
        config.auto_update_check = auto_update_check;
    }

    for (server_id, file_server) in file_config.agent_servers {
        merge_agent_server_config(config, server_id, file_server, path)?;
    }

    Ok(())
}

fn merge_agent_server_config(
    config: &mut AcpConfig,
    server_id: String,
    file_server: FileAgentServerConfig,
    path: &Path,
) -> Result<(), AppConfigError> {
    let had_existing = config.agent_servers.contains_key(&server_id);
    let mut server = config
        .agent_servers
        .remove(&server_id)
        .unwrap_or_else(|| AgentServerConfig::new(&server_id));

    if let Some(server_type) = file_server.server_type {
        server.server_type = AgentServerType::parse(&server_type).map_err(|error| match error {
            AppConfigError::InvalidAgentServerType { value, .. } => {
                AppConfigError::InvalidAgentServerType {
                    path: Some(path.to_path_buf()),
                    server: server_id.clone(),
                    value,
                }
            }
            other => other,
        })?;
    } else if !had_existing && file_server.command.is_some() {
        server.server_type = AgentServerType::Custom;
    }

    if let Some(agent) = file_server.agent {
        server.agent = agent;
    }

    if let Some(command) = file_server.command {
        server.command = command;
    }

    if let Some(args) = file_server.args {
        server.args = args;
    }

    if let Some(env) = file_server.env {
        server.env = env;
    }

    if let Some(default_model) = file_server.default_model {
        server.default_model = Some(default_model);
    }

    if let Some(default_mode) = file_server.default_mode {
        server.default_mode = Some(default_mode);
    }

    config.agent_servers.insert(server_id, server);

    Ok(())
}

fn user_config_directory() -> Option<PathBuf> {
    ProjectDirs::from("", "", "lumos").map(|dirs| dirs.config_dir().to_path_buf())
}

/// `write_acp_enabled` 将 acp enabled 开关写回来源配置。
pub fn write_acp_enabled(source: &LoadedConfig, enabled: bool) -> Result<PathBuf, AppConfigError> {
    write_acp_bool(source, "enabled", enabled)
}

/// `write_acp_auto_update_check` 将自动更新检查开关写回来源配置。
pub fn write_acp_auto_update_check(
    source: &LoadedConfig,
    enabled: bool,
) -> Result<PathBuf, AppConfigError> {
    write_acp_bool(source, "auto_update_check", enabled)
}

fn write_acp_bool(
    source: &LoadedConfig,
    key: &str,
    enabled: bool,
) -> Result<PathBuf, AppConfigError> {
    let path = source
        .acp_source
        .clone()
        .or_else(|| source.user_acp_path.clone())
        .or_else(default_user_config_path)
        .unwrap_or_else(|| PathBuf::from("acp.toml"));

    let content = match fs::read_to_string(&path) {
        Ok(content) => content,
        Err(error) if error.kind() == io::ErrorKind::NotFound => String::new(),
        Err(source) => {
            return Err(AppConfigError::Read { path, source });
        }
    };
    let mut document = content
        .parse::<toml_edit::DocumentMut>()
        .map_err(|source| AppConfigError::Edit {
            path: path.clone(),
            source,
        })?;
    document[key] = toml_edit::value(enabled);

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|source| AppConfigError::Write {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    fs::write(&path, document.to_string()).map_err(|source| AppConfigError::Write {
        path: path.clone(),
        source,
    })?;

    Ok(path)
}

fn default_user_config_path() -> Option<PathBuf> {
    user_config_directory().map(|path| path.join("acp.toml"))
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
    use super::{
        AcpDistribution, AcpInstallRoot, UserInputStyle, load_from_paths, load_with_lookups,
        load_with_sources_from_paths, write_acp_auto_update_check, write_acp_enabled,
    };
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
    fn load_defaults_to_disabled_acp() {
        let working_dir = temp_test_dir("load-acp-default-working");
        let user_config_dir = temp_test_dir("load-acp-default-config");

        let config = load_from_paths(Some(working_dir.as_path()), Some(user_config_dir.as_path()))
            .expect("missing config files should fall back to defaults");

        assert!(!config.acp.enabled);
        assert_eq!(config.acp.install_root, AcpInstallRoot::Config);
        assert_eq!(
            config.acp.distribution_preference,
            vec![AcpDistribution::Binary]
        );
        assert!(config.acp.auto_update_check);
        assert_eq!(
            config.acp.registry_url,
            "https://cdn.agentclientprotocol.com/registry/v1/latest/registry.json"
        );
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
    fn load_acp_config_from_project_acp_config() {
        let working_dir = temp_test_dir("load-acp-project-working");
        write_config(
            &working_dir.join(".lumos").join("acp.toml"),
            r#"
enabled = true
install_root = "project"
auto_update_check = false
distribution_preference = ["binary"]
registry_url = "https://example.test/registry.json"

[agent_servers.kimi]
type = "registry"
agent = "kimi"
command = "kimi-dev"
args = ["acp", "--verbose"]
env = { LUMOS_TEST = "1" }
"#,
        );

        let config = load_from_paths(Some(working_dir.as_path()), None)
            .expect("acp config should be loaded");

        let server = config
            .acp
            .agent_servers
            .get("kimi")
            .expect("kimi server should be configured");

        assert!(config.acp.enabled);
        assert_eq!(config.acp.install_root, AcpInstallRoot::Project);
        assert!(!config.acp.auto_update_check);
        assert_eq!(
            config.acp.registry_url,
            "https://example.test/registry.json"
        );
        assert_eq!(server.agent, "kimi");
        assert_eq!(server.command, "kimi-dev");
        assert_eq!(server.args, vec!["acp", "--verbose"]);
        assert_eq!(server.env.get("LUMOS_TEST"), Some(&"1".to_string()));
    }

    #[test]
    fn load_rejects_acp_table_in_main_config() {
        let working_dir = temp_test_dir("load-acp-main-config-ignored");
        write_config(
            &working_dir.join(".lumos").join("config.toml"),
            "[acp]\nenabled = true\nagent = \"kimi\"\n",
        );

        let error = load_from_paths(Some(working_dir.as_path()), None)
            .expect_err("acp table in main config should be rejected");

        assert!(
            error.to_string().contains("unknown field `acp`"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn load_tracks_acp_source() {
        let working_dir = temp_test_dir("load-acp-source-working");
        let user_config_dir = temp_test_dir("load-acp-source-config");
        let user_config = user_config_dir.join("acp.toml");
        let project_config = working_dir.join(".lumos").join("acp.toml");
        write_config(&user_config, "[agent_servers.kimi]\ntype = \"registry\"\n");
        write_config(
            &project_config,
            "[agent_servers.codex-acp]\ntype = \"registry\"\n",
        );

        let loaded = load_with_sources_from_paths(
            Some(working_dir.as_path()),
            Some(user_config_dir.as_path()),
        )
        .expect("config should load with source metadata");

        assert_eq!(loaded.acp_source, Some(project_config));
        assert_eq!(loaded.user_acp_path, Some(user_config));
        assert!(loaded.config.acp.agent_servers.contains_key("codex-acp"));
    }

    #[test]
    fn write_acp_enabled_preserves_existing_toml() {
        let working_dir = temp_test_dir("write-acp-enabled-working");
        let project_config = working_dir.join(".lumos").join("acp.toml");
        write_config(
            &project_config,
            "# keep me\nenabled = true\n[agent_servers.kimi]\ntype = \"registry\"\n",
        );
        let loaded = load_with_sources_from_paths(Some(working_dir.as_path()), None)
            .expect("config should load");

        let written_path =
            write_acp_enabled(&loaded, false).expect("acp enabled should be written back");

        assert_eq!(written_path, project_config);
        let content = fs::read_to_string(written_path).expect("config should be readable");
        assert!(content.contains("# keep me"));
        assert!(content.contains("[agent_servers.kimi]"));
        assert!(content.contains("enabled = false"));
    }

    #[test]
    fn write_acp_auto_update_check_uses_acp_source() {
        let working_dir = temp_test_dir("write-acp-update-working");
        let user_config_dir = temp_test_dir("write-acp-update-config");
        let user_config = user_config_dir.join("acp.toml");
        write_config(&user_config, "auto_update_check = true\n");
        let loaded = load_with_sources_from_paths(
            Some(working_dir.as_path()),
            Some(user_config_dir.as_path()),
        )
        .expect("config should load");

        let written_path = write_acp_auto_update_check(&loaded, false)
            .expect("acp auto update should be written back");

        assert_eq!(written_path, user_config);
        let content = fs::read_to_string(written_path).expect("config should be readable");
        assert!(content.contains("auto_update_check = false"));
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
