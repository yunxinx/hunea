use std::{
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use lumos::appconfig::{UserInputStyle, load_from_paths};

#[test]
fn load_defaults_to_cx_when_no_config_exists() {
    let working_dir = temp_test_dir("load-defaults-working");
    let user_config_dir = temp_test_dir("load-defaults-config");

    let config = load_from_paths(Some(working_dir.as_path()), Some(user_config_dir.as_path()))
        .expect("missing config files should fall back to defaults");

    assert_eq!(config.tui.user_input_style, UserInputStyle::Cx);
    assert!(config.tui.status_line.is_empty());
}

#[test]
fn load_project_config_overrides_user_config() {
    let working_dir = temp_test_dir("load-project-overrides-working");
    let user_config_dir = temp_test_dir("load-project-overrides-config");
    write_config(
        &user_config_dir.join("config.toml"),
        "[tui]\nuser_input_style = \"ms\"\n",
    );
    write_config(
        &working_dir.join(".lumos").join("config.toml"),
        "[tui]\nuser_input_style = \"cx\"\n",
    );

    let config = load_from_paths(Some(working_dir.as_path()), Some(user_config_dir.as_path()))
        .expect("project config should override the user config");

    assert_eq!(config.tui.user_input_style, UserInputStyle::Cx);
}

#[test]
fn load_accepts_cc_style_mode() {
    let working_dir = temp_test_dir("load-accepts-cc-working");
    write_config(
        &working_dir.join(".lumos").join("config.toml"),
        "[tui]\nuser_input_style = \"cc\"\n",
    );

    let config = load_from_paths(Some(working_dir.as_path()), None)
        .expect("cc should be accepted as a valid style mode");

    assert_eq!(config.tui.user_input_style, UserInputStyle::Cc);
}

#[test]
fn load_accepts_git_branch_status_line() {
    let working_dir = temp_test_dir("load-accepts-git-branch-working");
    write_config(
        &working_dir.join(".lumos").join("config.toml"),
        "[tui]\nstatus_line = [\"git-branch\"]\n",
    );

    let config = load_from_paths(Some(working_dir.as_path()), None)
        .expect("git-branch should be accepted as a valid status line item");

    assert_eq!(config.tui.status_line, vec!["git-branch"]);
}

#[test]
fn load_accepts_current_dir_status_line() {
    let working_dir = temp_test_dir("load-accepts-current-dir-working");
    write_config(
        &working_dir.join(".lumos").join("config.toml"),
        "[tui]\nstatus_line = [\"current-dir\"]\n",
    );

    let config = load_from_paths(Some(working_dir.as_path()), None)
        .expect("current-dir should be accepted as a valid status line item");

    assert_eq!(config.tui.status_line, vec!["current-dir"]);
}

#[test]
fn load_accepts_external_editor_command() {
    let working_dir = temp_test_dir("load-accepts-external-editor-working");
    write_config(
        &working_dir.join(".lumos").join("config.toml"),
        "[tui]\nexternal_editor = [\"code\", \"--wait\"]\n",
    );

    let config = load_from_paths(Some(working_dir.as_path()), None)
        .expect("external editor command should be accepted");

    assert_eq!(config.tui.external_editor, vec!["code", "--wait"]);
}

#[test]
fn load_accepts_disabling_external_editor_helper() {
    let working_dir = temp_test_dir("load-disable-external-editor-helper-working");
    write_config(
        &working_dir.join(".lumos").join("config.toml"),
        "[tui]\nshow_external_editor_helper = false\n",
    );

    let config = load_from_paths(Some(working_dir.as_path()), None)
        .expect("show_external_editor_helper should accept false");

    assert!(!config.tui.show_external_editor_helper);
}

#[test]
fn load_project_config_can_clear_user_status_line() {
    let working_dir = temp_test_dir("load-clears-status-line-working");
    let user_config_dir = temp_test_dir("load-clears-status-line-config");
    write_config(
        &user_config_dir.join("config.toml"),
        "[tui]\nstatus_line = [\"git-branch\"]\n",
    );
    write_config(
        &working_dir.join(".lumos").join("config.toml"),
        "[tui]\nstatus_line = []\n",
    );

    let config = load_from_paths(Some(working_dir.as_path()), Some(user_config_dir.as_path()))
        .expect("project config should be able to clear user-level status line items");

    assert!(config.tui.status_line.is_empty());
}

#[test]
fn load_project_config_can_clear_user_external_editor() {
    let working_dir = temp_test_dir("load-clears-external-editor-working");
    let user_config_dir = temp_test_dir("load-clears-external-editor-config");
    write_config(
        &user_config_dir.join("config.toml"),
        "[tui]\nexternal_editor = [\"code\", \"--wait\"]\n",
    );
    write_config(
        &working_dir.join(".lumos").join("config.toml"),
        "[tui]\nexternal_editor = []\n",
    );

    let config = load_from_paths(Some(working_dir.as_path()), Some(user_config_dir.as_path()))
        .expect("project config should be able to clear user-level external editor");

    assert!(config.tui.external_editor.is_empty());
}

#[test]
fn load_rejects_unknown_status_line_item() {
    let working_dir = temp_test_dir("load-rejects-status-line-working");
    write_config(
        &working_dir.join(".lumos").join("config.toml"),
        "[tui]\nstatus_line = [\"weird-item\"]\n",
    );

    let error = load_from_paths(Some(working_dir.as_path()), None)
        .expect_err("unknown status line item should be rejected");

    assert!(
        error.to_string().contains("unknown tui.status_line item"),
        "unexpected error: {error}"
    );
}

#[test]
fn load_rejects_unknown_style_mode() {
    let working_dir = temp_test_dir("load-rejects-style-working");
    write_config(
        &working_dir.join(".lumos").join("config.toml"),
        "[tui]\nuser_input_style = \"weird\"\n",
    );

    let error = load_from_paths(Some(working_dir.as_path()), None)
        .expect_err("unknown style mode should be rejected");

    assert!(
        error.to_string().contains("unknown tui.user_input_style"),
        "unexpected error: {error}"
    );
}

#[test]
fn load_rejects_unknown_keys() {
    let working_dir = temp_test_dir("load-rejects-keys-working");
    write_config(
        &working_dir.join(".lumos").join("config.toml"),
        "[tui]\nunknown = true\n",
    );

    let error =
        load_from_paths(Some(working_dir.as_path()), None).expect_err("unknown keys should fail");

    assert!(
        error.to_string().contains("unknown field"),
        "unexpected error: {error}"
    );
}

#[test]
fn load_rejects_external_editor_without_command() {
    let working_dir = temp_test_dir("load-rejects-empty-external-editor-working");
    write_config(
        &working_dir.join(".lumos").join("config.toml"),
        "[tui]\nexternal_editor = [\"\"]\n",
    );

    let error = load_from_paths(Some(working_dir.as_path()), None)
        .expect_err("external editor command should reject empty executable");

    assert!(
        error.to_string().contains("invalid tui.external_editor"),
        "unexpected error: {error}"
    );
}

#[test]
fn load_rejects_non_blocking_external_editor() {
    let working_dir = temp_test_dir("load-rejects-non-blocking-external-editor-working");
    write_config(
        &working_dir.join(".lumos").join("config.toml"),
        "[tui]\nexternal_editor = [\"code\"]\n",
    );

    let error = load_from_paths(Some(working_dir.as_path()), None)
        .expect_err("GUI editors without wait flags should be rejected");

    assert!(
        error
            .to_string()
            .contains("external editor must wait for close"),
        "unexpected error: {error}"
    );
}

fn temp_test_dir(prefix: &str) -> PathBuf {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time should be after unix epoch")
        .as_nanos();
    let path = std::env::temp_dir().join(format!("lumos-rust-{prefix}-{unique}"));
    fs::create_dir_all(&path).expect("temp test dir should be created");
    path
}

fn write_config(path: &Path, content: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("config parent dir should exist");
    }
    fs::write(path, content).expect("config file should be written");
}
