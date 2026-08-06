//! Managed ripgrep 授权读回与写回用户配置。

use std::{fs, io, path::Path};

use super::AppConfigError;

/// 轻量读取的 managed ripgrep 授权状态。
///
/// 与 tool-runtime 的 `ManagedRipgrepConfig` 字段一致，但独立定义以保持
/// app-config 不依赖 tool-runtime。terminal-app 层做转换。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ManagedRipgrepAuthorization {
    pub allow_managed_rg: Option<bool>,
}

/// `read_managed_ripgrep_authorization` 轻量读取 config.toml 的
/// `runtime.allow_managed_rg`。
///
/// 供 precheck 在完整 config 加载前使用。容错：文件不存在/解析失败/缺字段返回 default。
/// 字段在 `[runtime]` 表下（与 `persist_managed_ripgrep_*_to_path` 写入路径一致）。
pub fn read_managed_ripgrep_authorization(config_path: &Path) -> ManagedRipgrepAuthorization {
    let Ok(content) = fs::read_to_string(config_path) else {
        return ManagedRipgrepAuthorization::default();
    };
    let Ok(value) = toml::from_str::<toml::Value>(&content) else {
        return ManagedRipgrepAuthorization::default();
    };
    let Some(runtime) = value.get("runtime") else {
        return ManagedRipgrepAuthorization::default();
    };
    ManagedRipgrepAuthorization {
        allow_managed_rg: runtime.get("allow_managed_rg").and_then(|v| v.as_bool()),
    }
}

/// `persist_managed_ripgrep_authorization_to_path` 将 managed ripgrep 授权写入指定配置文件。
pub fn persist_managed_ripgrep_authorization_to_path(path: &Path) -> Result<(), AppConfigError> {
    persist_managed_ripgrep_authorization_field(path, true)
}

/// `persist_managed_ripgrep_rejection_to_path` 将 managed ripgrep 的拒绝（`false`）写入指定配置文件。
///
/// 用户在 precheck 选择 fallback 后调用，避免下次启动重复询问。
pub fn persist_managed_ripgrep_rejection_to_path(path: &Path) -> Result<(), AppConfigError> {
    persist_managed_ripgrep_authorization_field(path, false)
}

fn persist_managed_ripgrep_authorization_field(
    path: &Path,
    value: bool,
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
    document["runtime"]["allow_managed_rg"] = toml_edit::value(value);
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
