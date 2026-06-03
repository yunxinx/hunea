use std::path::{Path, PathBuf};

/// `resolve_configured_current_dir` 把配置中的 current_dir 解析为实际目录基准。
pub(crate) fn resolve_configured_current_dir(value: &str) -> PathBuf {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    }
    if trimmed == "~" {
        return home_dir().unwrap_or_else(|| PathBuf::from("."));
    }
    if let Some(rest) = trimmed.strip_prefix("~/") {
        return home_dir()
            .map(|home| home.join(rest))
            .unwrap_or_else(|| PathBuf::from(trimmed));
    }

    let path = PathBuf::from(trimmed);
    if path.is_absolute() {
        path
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(path))
            .unwrap_or_else(|_| PathBuf::from(trimmed))
    }
}

/// `resolve_path_token` 以给定根目录解析 `@token` 中的路径文本。
pub(crate) fn resolve_path_token(root: &Path, token: &str) -> PathBuf {
    if token == "~" {
        return home_dir().unwrap_or_else(|| PathBuf::from(token));
    }
    if let Some(rest) = token.strip_prefix("~/") {
        return home_dir()
            .map(|home| home.join(rest))
            .unwrap_or_else(|| PathBuf::from(token));
    }

    let path = Path::new(token);
    if path.is_absolute() {
        return path.to_path_buf();
    }
    root.join(path)
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("USERPROFILE").map(PathBuf::from))
}
