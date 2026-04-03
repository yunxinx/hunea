use std::{
    env,
    path::{MAIN_SEPARATOR, Path, PathBuf},
};

/// `short_work_dir` 返回当前工作目录，并在位于 home 目录下时使用 `~` 缩写。
/// 获取工作目录失败时返回空字符串。
pub fn short_work_dir() -> String {
    let Ok(working_dir) = env::current_dir() else {
        return String::new();
    };

    let Some(home_dir) = detect_home_dir() else {
        return normalize_path(&working_dir).display().to_string();
    };

    shorten_home_prefix(&working_dir, &home_dir)
}

/// `shorten_home_prefix` 仅在路径真实位于 home 下时把前缀替换为 `~`。
pub fn shorten_home_prefix(working_dir: &Path, home_dir: &Path) -> String {
    let clean_working_dir = normalize_path(working_dir);
    let clean_home_dir = normalize_path(home_dir);

    if clean_working_dir == clean_home_dir {
        return String::from("~");
    }

    let Ok(relative_path) = clean_working_dir.strip_prefix(&clean_home_dir) else {
        return clean_working_dir.display().to_string();
    };

    if relative_path.as_os_str().is_empty() {
        return String::from("~");
    }

    format!("~{MAIN_SEPARATOR}{}", relative_path.display())
}

fn detect_home_dir() -> Option<PathBuf> {
    env::var_os("HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("USERPROFILE").map(PathBuf::from))
        .or_else(|| {
            let home_drive = env::var_os("HOMEDRIVE")?;
            let home_path = env::var_os("HOMEPATH")?;
            let mut path = PathBuf::from(home_drive);
            path.push(home_path);
            Some(path)
        })
}

fn normalize_path(path: &Path) -> PathBuf {
    path.components().collect()
}
