use std::{
    env,
    path::Path,
    sync::{Mutex, OnceLock},
};

use lumos::envinfo::{short_work_dir, shorten_home_prefix};

#[test]
fn shorten_home_prefix_only_replaces_a_real_home_prefix() {
    let cases = [
        ("/home/archie", "/home/archie", "~"),
        ("/home/archie/project", "/home/archie", "~/project"),
        (
            "/tmp/home/archie/project",
            "/home/archie",
            "/tmp/home/archie/project",
        ),
        (
            "/home/archie-dev/project",
            "/home/archie",
            "/home/archie-dev/project",
        ),
    ];

    for (working_dir, home_dir, expected) in cases {
        let shortened = shorten_home_prefix(Path::new(working_dir), Path::new(home_dir));
        assert_eq!(shortened, expected);
    }
}

#[test]
fn short_work_dir_replaces_home_prefix_from_environment() {
    let _guard = test_environment_lock()
        .lock()
        .expect("lock should not be poisoned");
    let original_dir = env::current_dir().expect("current directory should be available");
    let original_home = env::var_os("HOME");

    let home_dir = tempfile_path("home");
    let work_dir = home_dir.join("project");
    std::fs::create_dir_all(&work_dir).expect("work directory should be creatable");

    env::set_current_dir(&work_dir).expect("should switch into temp workdir");
    unsafe {
        env::set_var("HOME", &home_dir);
    }

    assert_eq!(short_work_dir(), "~/project");

    env::set_current_dir(original_dir).expect("should restore original directory");
    restore_env_var("HOME", original_home);
}

#[test]
fn short_work_dir_keeps_path_outside_home() {
    let _guard = test_environment_lock()
        .lock()
        .expect("lock should not be poisoned");
    let original_dir = env::current_dir().expect("current directory should be available");
    let original_home = env::var_os("HOME");

    let home_dir = tempfile_path("home");
    let root_dir = tempfile_path("root");
    let work_dir = root_dir.join("project");
    std::fs::create_dir_all(&work_dir).expect("work directory should be creatable");

    env::set_current_dir(&work_dir).expect("should switch into temp workdir");
    unsafe {
        env::set_var("HOME", &home_dir);
    }

    assert_eq!(short_work_dir(), work_dir.display().to_string());

    env::set_current_dir(original_dir).expect("should restore original directory");
    restore_env_var("HOME", original_home);
}

fn test_environment_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

fn tempfile_path(prefix: &str) -> std::path::PathBuf {
    let unique = format!(
        "{prefix}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system time should be after epoch")
            .as_nanos()
    );
    std::env::temp_dir().join(unique)
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
