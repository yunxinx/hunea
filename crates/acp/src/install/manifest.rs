use std::{collections::BTreeMap, fs, io, path::Path};

use serde::{Deserialize, Serialize};

/// `InstallManifest` 记录一个 ACP binary 的本地安装结果。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InstallManifest {
    pub agent_id: String,
    pub agent_version: String,
    pub archive_url: String,
    pub command: String,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub checksum_verified: bool,
    pub installed_at: String,
}

/// `read_install_manifest` 从磁盘读取安装 manifest。
pub fn read_install_manifest(path: &Path) -> Result<InstallManifest, ManifestError> {
    let content = fs::read_to_string(path).map_err(|source| ManifestError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    toml::from_str(&content).map_err(|source| ManifestError::Decode {
        path: path.to_path_buf(),
        source,
    })
}

/// `write_install_manifest` 将安装 manifest 写入磁盘。
pub fn write_install_manifest(
    path: &Path,
    manifest: &InstallManifest,
) -> Result<(), ManifestError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|source| ManifestError::Write {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    let content = toml::to_string_pretty(manifest).map_err(ManifestError::Encode)?;
    fs::write(path, content).map_err(|source| ManifestError::Write {
        path: path.to_path_buf(),
        source,
    })
}

/// `ManifestError` 描述安装 manifest 读写失败。
#[derive(Debug)]
pub enum ManifestError {
    Read {
        path: std::path::PathBuf,
        source: io::Error,
    },
    Write {
        path: std::path::PathBuf,
        source: io::Error,
    },
    Decode {
        path: std::path::PathBuf,
        source: toml::de::Error,
    },
    Encode(toml::ser::Error),
}

impl std::fmt::Display for ManifestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Read { path, source } => {
                write!(f, "read install manifest {}: {source}", path.display())
            }
            Self::Write { path, source } => {
                write!(f, "write install manifest {}: {source}", path.display())
            }
            Self::Decode { path, source } => {
                write!(f, "decode install manifest {}: {source}", path.display())
            }
            Self::Encode(source) => write!(f, "encode install manifest: {source}"),
        }
    }
}

impl std::error::Error for ManifestError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Read { source, .. } | Self::Write { source, .. } => Some(source),
            Self::Decode { source, .. } => Some(source),
            Self::Encode(source) => Some(source),
        }
    }
}
