use std::{fmt, path::PathBuf};

/// `InstallPathInputs` 是安装目录解析所需的环境路径快照。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallPathInputs {
    pub user_config_dir: PathBuf,
    pub user_data_dir: Option<PathBuf>,
    pub user_cache_dir: Option<PathBuf>,
    pub project_dir: Option<PathBuf>,
    pub custom_install_dir: Option<PathBuf>,
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

/// `InstallPaths` 表示某个 agent 版本最终使用的安装路径集合。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallPaths {
    pub root: PathBuf,
    pub agent_version_dir: PathBuf,
    pub manifest_path: PathBuf,
}

/// `InstallPathError` 描述安装路径策略无法解析的原因。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallPathError {
    MissingDataDir,
    MissingCacheDir,
    MissingProjectDir,
    MissingCustomInstallDir,
}

impl fmt::Display for InstallPathError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingDataDir => {
                write!(f, "acp install_root=data requires a data directory")
            }
            Self::MissingCacheDir => {
                write!(f, "acp install_root=cache requires a cache directory")
            }
            Self::MissingProjectDir => {
                write!(f, "acp install_root=project requires a project directory")
            }
            Self::MissingCustomInstallDir => {
                write!(f, "acp install_root=custom requires custom_install_dir")
            }
        }
    }
}

impl std::error::Error for InstallPathError {}

impl InstallPathInputs {
    pub fn new(user_config_dir: PathBuf) -> Self {
        Self {
            user_config_dir,
            user_data_dir: None,
            user_cache_dir: None,
            project_dir: None,
            custom_install_dir: None,
        }
    }
}

/// `resolve_install_paths` 根据配置策略解析 agent 版本安装目录。
pub fn resolve_install_paths(
    inputs: &InstallPathInputs,
    install_root: AcpInstallRoot,
    agent_id: &str,
    version: &str,
) -> Result<InstallPaths, InstallPathError> {
    let root = match install_root {
        AcpInstallRoot::Config => inputs.user_config_dir.join(".acpclient"),
        AcpInstallRoot::Data => inputs
            .user_data_dir
            .clone()
            .ok_or(InstallPathError::MissingDataDir)?
            .join(".acpclient"),
        AcpInstallRoot::Cache => inputs
            .user_cache_dir
            .clone()
            .ok_or(InstallPathError::MissingCacheDir)?
            .join(".acpclient"),
        AcpInstallRoot::Project => inputs
            .project_dir
            .clone()
            .ok_or(InstallPathError::MissingProjectDir)?
            .join(".lumos")
            .join(".acpclient"),
        AcpInstallRoot::Custom => inputs
            .custom_install_dir
            .clone()
            .ok_or(InstallPathError::MissingCustomInstallDir)?,
    };
    let agent_version_dir = root.join("installs").join(agent_id).join(version);
    let manifest_path = agent_version_dir.join("manifest.toml");

    Ok(InstallPaths {
        root,
        agent_version_dir,
        manifest_path,
    })
}
