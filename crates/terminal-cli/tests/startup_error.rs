use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

#[test]
fn invalid_app_config_exits_with_user_facing_table() {
    let working_dir = temp_test_dir("invalid-app-config-table-working");
    let config_path = working_dir.join(".hunea").join("config.toml");
    write_config(&config_path, "[tui]\nstatus_line = [\"current-mode\"]\n");

    let output = Command::new(env!("CARGO_BIN_EXE_hunea"))
        .current_dir(&working_dir)
        .output()
        .expect("hunea binary should run");

    assert!(
        !output.status.success(),
        "invalid config should exit with a failure status"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(stderr.contains("Configuration error"));
    assert!(stderr.contains("| Field"));
    assert!(stderr.contains("| File"));
    assert!(stderr.contains(config_path.to_string_lossy().as_ref()));
    assert!(stderr.contains("| Setting"));
    assert!(stderr.contains("tui.status_line"));
    assert!(stderr.contains("| Value"));
    assert!(stderr.contains("\"current-mode\""));
    assert!(stderr.contains("| Expected"));
    assert!(stderr.contains("git-branch, current-dir, current-model"));
    assert!(
        !stderr.contains("Backtrace"),
        "config errors should not print developer backtrace hints: {stderr}"
    );
    assert!(
        !stderr.contains("Location:"),
        "config errors should not print source locations: {stderr}"
    );
}

#[test]
fn version_flag_prints_product_version_without_starting_tui() {
    let output = Command::new(env!("CARGO_BIN_EXE_hunea"))
        .arg("--version")
        .output()
        .expect("hunea binary should run");

    assert!(output.status.success(), "--version should succeed");
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        format!("hunea {}\n", env!("CARGO_PKG_VERSION"))
    );
    assert!(
        output.stderr.is_empty(),
        "--version should not initialize the TUI: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn write_config(path: &Path, content: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("test config parent directory should be created");
    }
    fs::write(path, content).expect("test config should be written");
}

fn temp_test_dir(name: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time should be after epoch")
        .as_nanos();
    let path = std::env::temp_dir().join(format!("hunea-{name}-{nanos}"));
    fs::create_dir_all(&path).expect("test directory should be created");
    path
}
