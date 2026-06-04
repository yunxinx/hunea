use std::path::PathBuf;

use directories::ProjectDirs;

/// `config_dir` 返回 hunea 用户级配置根目录。
pub fn config_dir() -> Option<PathBuf> {
    ProjectDirs::from("", "", "hunea").map(|dirs| dirs.config_dir().to_path_buf())
}

/// `user_config_file_path` 返回用户级 `config.toml` 的默认写入位置。
pub fn user_config_file_path() -> Option<PathBuf> {
    config_dir().map(|path| path.join("config.toml"))
}
