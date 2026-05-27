use std::path::PathBuf;

use directories::ProjectDirs;

/// `user_config_file_path` 返回用户级 `config.toml` 的默认写入位置。
pub fn user_config_file_path() -> Option<PathBuf> {
    user_config_directory().map(|path| path.join("config.toml"))
}

pub(super) fn user_config_directory() -> Option<PathBuf> {
    ProjectDirs::from("", "", "hunea").map(|dirs| dirs.config_dir().to_path_buf())
}
