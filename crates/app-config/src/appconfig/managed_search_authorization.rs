//! Managed search 工具授权写回用户配置。

use std::{fs, io, path::Path};

use runtime_domain::session::ManagedSearchTool;

use super::AppConfigError;

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
