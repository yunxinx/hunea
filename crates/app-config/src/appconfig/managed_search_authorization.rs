//! Managed search 工具授权读回与写回用户配置。

use std::{fs, io, path::Path};

use runtime_domain::session::ManagedSearchTool;

use super::AppConfigError;

/// 轻量读取的受管搜索工具授权状态。
///
/// 与 tool-runtime 的 `ManagedSearchToolConfig` 字段一致，但独立定义以保持
/// app-config 不依赖 tool-runtime。terminal-app 层做转换。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ManagedSearchAuthorization {
    pub allow_managed_rg: Option<bool>,
    pub allow_managed_fd: Option<bool>,
}

/// `read_managed_search_authorization` 轻量读取 config.toml 的
/// `runtime.allow_managed_rg` / `runtime.allow_managed_fd`。
///
/// 供 precheck 在完整 config 加载前使用。容错：文件不存在/解析失败/缺字段返回 default。
/// 字段在 `[runtime]` 表下（与 `persist_managed_search_tool_*_to_path` 写入路径一致）。
pub fn read_managed_search_authorization(config_path: &Path) -> ManagedSearchAuthorization {
    let Ok(content) = fs::read_to_string(config_path) else {
        return ManagedSearchAuthorization::default();
    };
    let Ok(value) = toml::from_str::<toml::Value>(&content) else {
        return ManagedSearchAuthorization::default();
    };
    let Some(runtime) = value.get("runtime") else {
        return ManagedSearchAuthorization::default();
    };
    ManagedSearchAuthorization {
        allow_managed_rg: runtime.get("allow_managed_rg").and_then(|v| v.as_bool()),
        allow_managed_fd: runtime.get("allow_managed_fd").and_then(|v| v.as_bool()),
    }
}

/// `persist_managed_search_tool_authorization_to_path` 将受管搜索工具授权写入指定配置文件。
pub fn persist_managed_search_tool_authorization_to_path(
    path: &Path,
    tool: ManagedSearchTool,
) -> Result<(), AppConfigError> {
    persist_managed_search_tool_field(path, tool, true)
}

/// `persist_managed_search_tool_rejection_to_path` 将受管搜索工具的拒绝（`false`）写入指定配置文件。
///
/// 用户在 precheck 选择 fallback 后调用，避免下次启动重复询问。
pub fn persist_managed_search_tool_rejection_to_path(
    path: &Path,
    tool: ManagedSearchTool,
) -> Result<(), AppConfigError> {
    persist_managed_search_tool_field(path, tool, false)
}

fn persist_managed_search_tool_field(
    path: &Path,
    tool: ManagedSearchTool,
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
    document["runtime"][managed_search_tool_authorization_field(tool)] = toml_edit::value(value);
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
