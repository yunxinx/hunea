use std::path::PathBuf;

use directories::ProjectDirs;

/// `hunea_config_dir` 返回 hunea 用户级配置根目录。
pub fn hunea_config_dir() -> Option<PathBuf> {
    ProjectDirs::from("", "", "hunea").map(|dirs| dirs.config_dir().to_path_buf())
}
