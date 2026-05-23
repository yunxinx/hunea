use std::{
    env, fs,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
    time::{SystemTime, UNIX_EPOCH},
};

use runtime_domain::envinfo::git_branch;

#[test]
fn git_branch_returns_current_branch_name() {
    let _guard = test_environment_lock()
        .lock()
        .expect("lock should not be poisoned");
    let original_dir = env::current_dir().expect("current directory should be available");

    let repo_dir = temp_test_dir("git-branch-current");
    write_git_head(&repo_dir, "ref: refs/heads/feature/status-line\n");
    fs::create_dir_all(repo_dir.join("nested")).expect("nested directory should exist");
    env::set_current_dir(repo_dir.join("nested")).expect("should switch into nested workdir");

    assert_eq!(git_branch(), "feature/status-line");

    env::set_current_dir(original_dir).expect("should restore original directory");
}

#[test]
fn git_branch_returns_empty_outside_repository() {
    let _guard = test_environment_lock()
        .lock()
        .expect("lock should not be poisoned");
    let original_dir = env::current_dir().expect("current directory should be available");

    let working_dir = temp_test_dir("git-branch-outside");
    env::set_current_dir(&working_dir).expect("should switch into temp working directory");

    assert_eq!(git_branch(), "");

    env::set_current_dir(original_dir).expect("should restore original directory");
}

#[test]
fn git_branch_supports_git_dir_file() {
    let _guard = test_environment_lock()
        .lock()
        .expect("lock should not be poisoned");
    let original_dir = env::current_dir().expect("current directory should be available");

    let repo_dir = temp_test_dir("git-branch-gitdir-file");
    let git_dir = temp_test_dir("git-branch-worktree").join("feature");
    fs::create_dir_all(&git_dir).expect("git dir should exist");
    fs::write(
        repo_dir.join(".git"),
        format!("gitdir: {}\n", git_dir.display()),
    )
    .expect(".git indirection file should be written");
    fs::write(git_dir.join("HEAD"), "ref: refs/heads/main\n").expect("HEAD should be written");
    env::set_current_dir(&repo_dir).expect("should switch into gitdir-file workdir");

    assert_eq!(git_branch(), "main");

    env::set_current_dir(original_dir).expect("should restore original directory");
}

#[test]
fn git_branch_resolves_symlinked_working_dir() {
    let _guard = test_environment_lock()
        .lock()
        .expect("lock should not be poisoned");
    let original_dir = env::current_dir().expect("current directory should be available");
    let original_pwd = env::var_os("PWD");

    let repo_dir = temp_test_dir("git-branch-symlink-repo");
    write_git_head(&repo_dir, "ref: refs/heads/feature/symlink\n");
    fs::create_dir_all(repo_dir.join("nested").join("deeper"))
        .expect("nested repo path should exist");

    let link_root = temp_test_dir("git-branch-symlink-root");
    let link_path = link_root.join("worktree");
    #[cfg(unix)]
    std::os::unix::fs::symlink(repo_dir.join("nested"), &link_path)
        .expect("symlink should be created");
    #[cfg(windows)]
    std::os::windows::fs::symlink_dir(repo_dir.join("nested"), &link_path)
        .expect("symlink should be created");

    let working_dir = link_path.join("deeper");
    env::set_current_dir(&working_dir).expect("should switch into symlinked workdir");
    unsafe {
        env::set_var("PWD", &working_dir);
    }

    assert_eq!(git_branch(), "feature/symlink");

    env::set_current_dir(original_dir).expect("should restore original directory");
    restore_env_var("PWD", original_pwd);
}

fn test_environment_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

fn temp_test_dir(prefix: &str) -> PathBuf {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time should be after unix epoch")
        .as_nanos();
    let path = env::temp_dir().join(format!("lumos-rust-{prefix}-{unique}"));
    fs::create_dir_all(&path).expect("temp test dir should be created");
    path
}

fn write_git_head(repo_dir: &Path, head: &str) {
    fs::create_dir_all(repo_dir.join(".git")).expect("git dir should exist");
    fs::write(repo_dir.join(".git").join("HEAD"), head).expect("HEAD should be written");
}

fn restore_env_var(key: &str, value: Option<std::ffi::OsString>) {
    match value {
        Some(value) => unsafe {
            env::set_var(key, value);
        },
        None => unsafe {
            env::remove_var(key);
        },
    }
}
