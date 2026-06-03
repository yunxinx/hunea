use std::{
    env, fs,
    path::{Path, PathBuf},
};

/// `git_branch` 返回当前工作目录所属 Git 仓库的分支名。
/// 当前目录不在 Git 仓库中、`HEAD` 不是分支引用或读取失败时返回空字符串。
pub fn git_branch() -> String {
    let Ok(working_dir) = env::current_dir() else {
        return String::new();
    };

    git_branch_from_working_dir(&working_dir)
}

fn git_branch_from_working_dir(working_dir: &Path) -> String {
    let canonical = canonicalize_working_dir_for_git_lookup(working_dir);
    let Some(git_dir) = find_git_dir(&canonical) else {
        return String::new();
    };

    let Ok(head) = fs::read_to_string(git_dir.join("HEAD")) else {
        return String::new();
    };

    let reference = head
        .trim()
        .strip_prefix("ref:")
        .map(str::trim)
        .unwrap_or("");
    reference
        .strip_prefix("refs/heads/")
        .unwrap_or("")
        .to_string()
}

fn canonicalize_working_dir_for_git_lookup(working_dir: &Path) -> PathBuf {
    fs::canonicalize(working_dir).unwrap_or_else(|_| working_dir.to_path_buf())
}

fn find_git_dir(start_dir: &Path) -> Option<PathBuf> {
    let mut directory = start_dir.to_path_buf();

    loop {
        let git_path = directory.join(".git");
        match fs::metadata(&git_path) {
            Ok(metadata) if metadata.is_dir() => return Some(git_path),
            Ok(_) => return resolve_git_dir_file(&directory, &git_path),
            Err(_) => {}
        }

        let parent = directory.parent()?;
        if parent == directory {
            return None;
        }
        directory = parent.to_path_buf();
    }
}

fn resolve_git_dir_file(base_dir: &Path, git_path: &Path) -> Option<PathBuf> {
    let content = fs::read_to_string(git_path).ok()?;
    let target = content.trim().strip_prefix("gitdir:")?.trim();
    if target.is_empty() {
        return None;
    }

    let resolved = if Path::new(target).is_absolute() {
        PathBuf::from(target)
    } else {
        base_dir.join(target)
    };

    Some(resolved)
}
