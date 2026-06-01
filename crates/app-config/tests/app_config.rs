use std::{
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use app_config::appconfig::{
    AppConfigError, ReasoningContentDisplay, UserInputStyle, load_from_paths,
    persist_managed_search_tool_authorization_to_path,
};
use runtime_domain::session::ManagedSearchTool;

#[test]
fn load_defaults_to_cx_when_no_config_exists() {
    let working_dir = temp_test_dir("load-defaults-working");
    let user_config_dir = temp_test_dir("load-defaults-config");

    let config = load_from_paths(Some(working_dir.as_path()), Some(user_config_dir.as_path()))
        .expect("missing config files should fall back to defaults");

    assert_eq!(config.tui.user_input_style, UserInputStyle::Cx);
    assert!(config.tui.status_line.is_empty());
    assert!(config.tui.status_line_2.is_empty());
    assert_eq!(config.tui.file_picker_popup_height, 7);
    assert!(!config.debug.enabled);
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
        &working_dir.join(".hunea").join("config.toml"),
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
        &working_dir.join(".hunea").join("config.toml"),
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
        &working_dir.join(".hunea").join("config.toml"),
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
        &working_dir.join(".hunea").join("config.toml"),
        "[tui]\nstatus_line = [\"current-dir\"]\n",
    );

    let config = load_from_paths(Some(working_dir.as_path()), None)
        .expect("current-dir should be accepted as a valid status line item");

    assert_eq!(config.tui.status_line, vec!["current-dir"]);
}

#[test]
fn load_accepts_current_model_status_line() {
    let working_dir = temp_test_dir("load-accepts-current-model-working");
    write_config(
        &working_dir.join(".hunea").join("config.toml"),
        "[tui]\nstatus_line = [\"current-model\"]\n",
    );

    let config = load_from_paths(Some(working_dir.as_path()), None)
        .expect("current-model should be accepted as a valid status line item");

    assert_eq!(config.tui.status_line, vec!["current-model"]);
}

#[test]
fn load_accepts_throughput_status_line() {
    let working_dir = temp_test_dir("load-accepts-throughput-working");
    write_config(
        &working_dir.join(".hunea").join("config.toml"),
        "[tui]\nstatus_line = [\"throughput\"]\n",
    );

    let config = load_from_paths(Some(working_dir.as_path()), None)
        .expect("throughput should be accepted as a valid status line item");

    assert_eq!(config.tui.status_line, vec!["throughput"]);
}

#[test]
fn load_accepts_latency_status_line() {
    let working_dir = temp_test_dir("load-accepts-latency-working");
    write_config(
        &working_dir.join(".hunea").join("config.toml"),
        "[tui]\nstatus_line = [\"latency\"]\n",
    );

    let config = load_from_paths(Some(working_dir.as_path()), None)
        .expect("latency should be accepted as a valid status line item");

    assert_eq!(config.tui.status_line, vec!["latency"]);
}

#[test]
fn load_accepts_second_status_line() {
    let working_dir = temp_test_dir("load-accepts-second-status-line-working");
    write_config(
        &working_dir.join(".hunea").join("config.toml"),
        "[tui]\nstatus_line_2 = [\"current-dir\", \"git-branch\"]\n",
    );

    let config = load_from_paths(Some(working_dir.as_path()), None)
        .expect("status_line_2 should accept the same item format as status_line");

    assert_eq!(config.tui.status_line_2, vec!["current-dir", "git-branch"]);
}

#[test]
fn load_project_config_can_clear_user_second_status_line() {
    let working_dir = temp_test_dir("load-clears-second-status-line-working");
    let user_config_dir = temp_test_dir("load-clears-second-status-line-config");
    write_config(
        &user_config_dir.join("config.toml"),
        "[tui]\nstatus_line_2 = [\"current-dir\"]\n",
    );
    write_config(
        &working_dir.join(".hunea").join("config.toml"),
        "[tui]\nstatus_line_2 = []\n",
    );

    let config = load_from_paths(Some(working_dir.as_path()), Some(user_config_dir.as_path()))
        .expect("project config should be able to clear user-level second status line items");

    assert!(config.tui.status_line_2.is_empty());
}

#[test]
fn load_rejects_unknown_second_status_line_item() {
    let working_dir = temp_test_dir("load-rejects-second-status-line-working");
    write_config(
        &working_dir.join(".hunea").join("config.toml"),
        "[tui]\nstatus_line_2 = [\"weird-item\"]\n",
    );

    let error = load_from_paths(Some(working_dir.as_path()), None)
        .expect_err("unknown second status line item should be rejected");

    assert!(
        error.to_string().contains("unknown tui.status_line item"),
        "unexpected error: {error}"
    );
}

#[test]
fn load_rejects_underscore_current_model_status_line() {
    let working_dir = temp_test_dir("load-rejects-underscore-current-model-working");
    write_config(
        &working_dir.join(".hunea").join("config.toml"),
        "[tui]\nstatus_line = [\"current_model\"]\n",
    );

    let error = load_from_paths(Some(working_dir.as_path()), None)
        .expect_err("current_model should not be accepted as a status line item");

    assert!(
        error.to_string().contains("unknown tui.status_line item"),
        "unexpected error: {error}"
    );
}

#[test]
fn load_accepts_external_editor_command() {
    let working_dir = temp_test_dir("load-accepts-external-editor-working");
    write_config(
        &working_dir.join(".hunea").join("config.toml"),
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
        &working_dir.join(".hunea").join("config.toml"),
        "[tui]\nshow_external_editor_helper = false\n",
    );

    let config = load_from_paths(Some(working_dir.as_path()), None)
        .expect("show_external_editor_helper should accept false");

    assert!(!config.tui.show_external_editor_helper);
}

#[test]
fn load_defaults_copy_on_mouse_selection_release_to_false() {
    let working_dir = temp_test_dir("load-default-copy-on-selection-working");
    let user_config_dir = temp_test_dir("load-default-copy-on-selection-config");

    let config = load_from_paths(Some(working_dir.as_path()), Some(user_config_dir.as_path()))
        .expect("missing config files should keep selection copy disabled");

    assert!(!config.tui.copy_on_mouse_selection_release);
}

#[test]
fn load_defaults_swap_enter_and_send_to_false() {
    let working_dir = temp_test_dir("load-default-swap-enter-working");
    let user_config_dir = temp_test_dir("load-default-swap-enter-config");

    let config = load_from_paths(Some(working_dir.as_path()), Some(user_config_dir.as_path()))
        .expect("missing config files should keep swapped enter disabled");

    assert!(!config.tui.swap_enter_and_send);
}

#[test]
fn load_defaults_ctrl_c_clears_input_to_true() {
    let working_dir = temp_test_dir("load-default-ctrl-c-clears-input-working");
    let user_config_dir = temp_test_dir("load-default-ctrl-c-clears-input-config");

    let config = load_from_paths(Some(working_dir.as_path()), Some(user_config_dir.as_path()))
        .expect("missing config files should keep ctrl+c draft clear enabled");

    assert!(config.tui.ctrl_c_clears_input);
}

#[test]
fn load_defaults_esc_interrupt_presses_to_two() {
    let working_dir = temp_test_dir("load-default-esc-interrupt-working");
    let user_config_dir = temp_test_dir("load-default-esc-interrupt-config");

    let config = load_from_paths(Some(working_dir.as_path()), Some(user_config_dir.as_path()))
        .expect("missing config files should keep esc interrupt presses at two");

    assert_eq!(config.tui.esc_interrupt_presses, 2);
}

#[test]
fn load_defaults_show_esc_interrupt_hint_to_true() {
    let working_dir = temp_test_dir("load-default-show-esc-hint-working");
    let user_config_dir = temp_test_dir("load-default-show-esc-hint-config");

    let config = load_from_paths(Some(working_dir.as_path()), Some(user_config_dir.as_path()))
        .expect("missing config files should keep esc interrupt hint enabled");

    assert!(config.tui.show_esc_interrupt_hint);
}

#[test]
fn load_defaults_print_transcript_on_exit_to_false() {
    let working_dir = temp_test_dir("load-default-print-transcript-working");
    let user_config_dir = temp_test_dir("load-default-print-transcript-config");

    let config = load_from_paths(Some(working_dir.as_path()), Some(user_config_dir.as_path()))
        .expect("missing config files should keep terminal replay disabled");

    assert!(!config.tui.print_transcript_on_exit);
}

#[test]
fn load_defaults_show_reasoning_content_to_false() {
    let working_dir = temp_test_dir("load-default-show-reasoning-working");
    let user_config_dir = temp_test_dir("load-default-show-reasoning-config");

    let config = load_from_paths(Some(working_dir.as_path()), Some(user_config_dir.as_path()))
        .expect("missing config files should keep reasoning content hidden");

    assert!(!config.tui.show_reasoning_content);
}

#[test]
fn load_defaults_reasoning_content_display_to_collapsed() {
    let working_dir = temp_test_dir("load-default-reasoning-display-working");
    let user_config_dir = temp_test_dir("load-default-reasoning-display-config");

    let config = load_from_paths(Some(working_dir.as_path()), Some(user_config_dir.as_path()))
        .expect("missing config files should keep reasoning display collapsed");

    assert_eq!(
        config.tui.reasoning_content_display,
        ReasoningContentDisplay::Collapsed
    );
}

#[test]
fn load_accepts_enabling_show_reasoning_content() {
    let working_dir = temp_test_dir("load-enable-show-reasoning-working");
    write_config(
        &working_dir.join(".hunea").join("config.toml"),
        "[tui]\nshow_reasoning_content = true\n",
    );

    let config = load_from_paths(Some(working_dir.as_path()), None)
        .expect("show_reasoning_content should accept true");

    assert!(config.tui.show_reasoning_content);
}

#[test]
fn load_defaults_reasoning_content_display_to_expanded_when_reasoning_content_is_enabled() {
    let working_dir = temp_test_dir("load-show-reasoning-default-expanded-working");
    write_config(
        &working_dir.join(".hunea").join("config.toml"),
        "[tui]\nshow_reasoning_content = true\n",
    );

    let config = load_from_paths(Some(working_dir.as_path()), None)
        .expect("show_reasoning_content without display should default to expanded");

    assert_eq!(
        config.tui.reasoning_content_display,
        ReasoningContentDisplay::Expanded
    );
}

#[test]
fn load_keeps_explicit_reasoning_content_display_when_reasoning_content_is_enabled() {
    let working_dir = temp_test_dir("load-show-reasoning-keeps-explicit-display-working");
    write_config(
        &working_dir.join(".hunea").join("config.toml"),
        "[tui]\nshow_reasoning_content = true\nreasoning_content_display = \"collapsed\"\n",
    );

    let config = load_from_paths(Some(working_dir.as_path()), None)
        .expect("explicit reasoning_content_display should be preserved");

    assert_eq!(
        config.tui.reasoning_content_display,
        ReasoningContentDisplay::Collapsed
    );
}

#[test]
fn load_accepts_expanded_reasoning_content_display() {
    let working_dir = temp_test_dir("load-expanded-reasoning-display-working");
    write_config(
        &working_dir.join(".hunea").join("config.toml"),
        "[tui]\nreasoning_content_display = \"expanded\"\n",
    );

    let config = load_from_paths(Some(working_dir.as_path()), None)
        .expect("reasoning_content_display should accept expanded");

    assert_eq!(
        config.tui.reasoning_content_display,
        ReasoningContentDisplay::Expanded
    );
}

#[test]
fn load_accepts_expanded_simplified_reasoning_content_display() {
    let working_dir = temp_test_dir("load-expanded-simplified-reasoning-display-working");
    write_config(
        &working_dir.join(".hunea").join("config.toml"),
        "[tui]\nreasoning_content_display = \"expanded-simplified\"\n",
    );

    let config = load_from_paths(Some(working_dir.as_path()), None)
        .expect("reasoning_content_display should accept expanded-simplified");

    assert_eq!(
        config.tui.reasoning_content_display,
        ReasoningContentDisplay::ExpandedSimplified
    );
}

#[test]
fn load_accepts_snippet_reasoning_content_display() {
    let working_dir = temp_test_dir("load-snippet-reasoning-display-working");
    write_config(
        &working_dir.join(".hunea").join("config.toml"),
        "[tui]\nreasoning_content_display = \"snippet\"\n",
    );

    let config = load_from_paths(Some(working_dir.as_path()), None)
        .expect("reasoning_content_display should accept snippet");

    assert_eq!(
        config.tui.reasoning_content_display,
        ReasoningContentDisplay::Snippet
    );
}

#[test]
fn load_rejects_unknown_reasoning_content_display() {
    let working_dir = temp_test_dir("load-invalid-reasoning-display-working");
    write_config(
        &working_dir.join(".hunea").join("config.toml"),
        "[tui]\nreasoning_content_display = \"always\"\n",
    );

    let error = load_from_paths(Some(working_dir.as_path()), None)
        .expect_err("unknown reasoning_content_display should be rejected");

    assert!(matches!(
        error,
        AppConfigError::InvalidReasoningContentDisplay { .. }
    ));
}

#[test]
fn load_accepts_enabling_copy_on_mouse_selection_release() {
    let working_dir = temp_test_dir("load-enable-copy-on-selection-working");
    write_config(
        &working_dir.join(".hunea").join("config.toml"),
        "[tui]\ncopy_on_mouse_selection_release = true\n",
    );

    let config = load_from_paths(Some(working_dir.as_path()), None)
        .expect("copy_on_mouse_selection_release should accept true");

    assert!(config.tui.copy_on_mouse_selection_release);
}

#[test]
fn load_accepts_swap_enter_and_send() {
    let working_dir = temp_test_dir("load-swap-enter-working");
    write_config(
        &working_dir.join(".hunea").join("config.toml"),
        "[tui]\nswap_enter_and_send = true\n",
    );

    let config = load_from_paths(Some(working_dir.as_path()), None)
        .expect("swap_enter_and_send should accept true");

    assert!(config.tui.swap_enter_and_send);
}

#[test]
fn load_accepts_disabling_ctrl_c_clears_input() {
    let working_dir = temp_test_dir("load-disable-ctrl-c-clears-input-working");
    write_config(
        &working_dir.join(".hunea").join("config.toml"),
        "[tui]\nctrl_c_clears_input = false\n",
    );

    let config = load_from_paths(Some(working_dir.as_path()), None)
        .expect("ctrl_c_clears_input should accept false");

    assert!(!config.tui.ctrl_c_clears_input);
}

#[test]
fn load_accepts_configured_esc_interrupt_presses() {
    let working_dir = temp_test_dir("load-esc-interrupt-working");
    write_config(
        &working_dir.join(".hunea").join("config.toml"),
        "[tui]\nesc_interrupt_presses = 3\n",
    );

    let config = load_from_paths(Some(working_dir.as_path()), None)
        .expect("esc_interrupt_presses should accept 3");

    assert_eq!(config.tui.esc_interrupt_presses, 3);
}

#[test]
fn load_accepts_configured_file_picker_popup_height() {
    let working_dir = temp_test_dir("load-file-picker-popup-height-working");
    write_config(
        &working_dir.join(".hunea").join("config.toml"),
        "[tui]\nfile_picker_popup_height = 21\n",
    );

    let config = load_from_paths(Some(working_dir.as_path()), None)
        .expect("file picker popup height should accept values up to 21");

    assert_eq!(config.tui.file_picker_popup_height, 21);
}

#[test]
fn load_accepts_minimum_file_picker_popup_height() {
    let working_dir = temp_test_dir("load-min-file-picker-popup-height-working");
    write_config(
        &working_dir.join(".hunea").join("config.toml"),
        "[tui]\nfile_picker_popup_height = 3\n",
    );

    let config = load_from_paths(Some(working_dir.as_path()), None)
        .expect("file picker popup height should accept the minimum value");

    assert_eq!(config.tui.file_picker_popup_height, 3);
}

#[test]
fn load_rejects_file_picker_popup_height_below_minimum() {
    let working_dir = temp_test_dir("load-low-file-picker-popup-height-working");
    write_config(
        &working_dir.join(".hunea").join("config.toml"),
        "[tui]\nfile_picker_popup_height = 2\n",
    );

    let error = load_from_paths(Some(working_dir.as_path()), None)
        .expect_err("file picker popup height should reject values below 3");

    assert!(
        error
            .to_string()
            .contains("tui.file_picker_popup_height must be between 3 and 21"),
        "unexpected error: {error}"
    );
}

#[test]
fn load_rejects_file_picker_popup_height_above_maximum() {
    let working_dir = temp_test_dir("load-high-file-picker-popup-height-working");
    write_config(
        &working_dir.join(".hunea").join("config.toml"),
        "[tui]\nfile_picker_popup_height = 22\n",
    );

    let error = load_from_paths(Some(working_dir.as_path()), None)
        .expect_err("file picker popup height should reject values above 21");

    assert!(
        error
            .to_string()
            .contains("tui.file_picker_popup_height must be between 3 and 21"),
        "unexpected error: {error}"
    );
}

#[test]
fn load_defaults_runtime_request_policy() {
    let working_dir = temp_test_dir("load-default-runtime-retry-working");

    let config = load_from_paths(Some(working_dir.as_path()), None)
        .expect("default runtime retry policy should load");

    assert_eq!(config.runtime.request_retry_attempts, 3);
    assert_eq!(config.runtime.request_retry_delays, vec![1, 2, 3]);
    assert_eq!(config.runtime.request_timeout_seconds, 120);
    assert_eq!(config.runtime.tool_max_turns, None);
    assert_eq!(config.runtime.allow_managed_rg, None);
    assert_eq!(config.runtime.allow_managed_fd, None);
}

#[test]
fn load_accepts_configured_runtime_request_policy() {
    let working_dir = temp_test_dir("load-runtime-retry-working");
    write_config(
        &working_dir.join(".hunea").join("config.toml"),
        "[runtime]\nrequest_retry_attempts = 5\nrequest_retry_delays = [1, 3]\nrequest_timeout_seconds = 240\ntool_max_turns = 11\n",
    );

    let config = load_from_paths(Some(working_dir.as_path()), None)
        .expect("runtime request policy should accept configured values");

    assert_eq!(config.runtime.request_retry_attempts, 5);
    assert_eq!(config.runtime.request_retry_delays, vec![1, 3, 3, 3, 3]);
    assert_eq!(config.runtime.request_timeout_seconds, 240);
    assert_eq!(config.runtime.tool_max_turns, Some(11));
}

#[test]
fn load_accepts_managed_search_tool_authorization_flags() {
    let working_dir = temp_test_dir("load-runtime-managed-search-tools-working");
    write_config(
        &working_dir.join(".hunea").join("config.toml"),
        "[runtime]\nallow_managed_rg = true\nallow_managed_fd = false\n",
    );

    let config = load_from_paths(Some(working_dir.as_path()), None)
        .expect("runtime managed search tool flags should load");

    assert_eq!(config.runtime.allow_managed_rg, Some(true));
    assert_eq!(config.runtime.allow_managed_fd, Some(false));
}

#[test]
fn persists_managed_search_tool_authorization_to_user_config() {
    let working_dir = temp_test_dir("persist-managed-search-authorization");
    let config_path = working_dir.join("config.toml");
    write_config(&config_path, "[runtime]\nrequest_timeout_seconds = 240\n");

    persist_managed_search_tool_authorization_to_path(&config_path, ManagedSearchTool::Ripgrep)
        .expect("authorization should be written");

    let content = fs::read_to_string(&config_path).expect("config should be readable");
    assert!(content.contains("request_timeout_seconds = 240"));
    assert!(content.contains("allow_managed_rg = true"));
    assert!(!content.contains("allow_managed_fd = true"));
}

#[test]
fn load_rejects_zero_runtime_tool_max_turns() {
    let working_dir = temp_test_dir("load-runtime-tool-max-turns-zero-working");
    write_config(
        &working_dir.join(".hunea").join("config.toml"),
        "[runtime]\ntool_max_turns = 0\n",
    );

    let error = load_from_paths(Some(working_dir.as_path()), None)
        .expect_err("runtime tool_max_turns should reject zero");

    assert!(
        error
            .to_string()
            .contains("runtime.tool_max_turns must be at least 1 when configured"),
        "unexpected error: {error}"
    );
}

#[test]
fn load_uses_runtime_request_retry_delay_count_when_attempts_are_omitted() {
    let working_dir = temp_test_dir("load-runtime-retry-delays-only-working");
    write_config(
        &working_dir.join(".hunea").join("config.toml"),
        "[runtime]\nrequest_retry_delays = [1, 3, 5]\n",
    );

    let config = load_from_paths(Some(working_dir.as_path()), None)
        .expect("runtime request retry delays should imply retry attempts");

    assert_eq!(config.runtime.request_retry_attempts, 3);
    assert_eq!(config.runtime.request_retry_delays, vec![1, 3, 5]);
}

#[test]
fn load_truncates_default_runtime_request_retry_delays_when_only_attempts_are_configured() {
    let working_dir = temp_test_dir("load-runtime-retry-attempts-only-working");
    write_config(
        &working_dir.join(".hunea").join("config.toml"),
        "[runtime]\nrequest_retry_attempts = 2\n",
    );

    let config = load_from_paths(Some(working_dir.as_path()), None)
        .expect("runtime request retry attempts should truncate default delays");

    assert_eq!(config.runtime.request_retry_attempts, 2);
    assert_eq!(config.runtime.request_retry_delays, vec![1, 2]);
}

#[test]
fn load_rejects_invalid_runtime_request_retry_policy_shape() {
    let working_dir = temp_test_dir("load-invalid-runtime-retry-working");
    write_config(
        &working_dir.join(".hunea").join("config.toml"),
        "[runtime]\nrequest_retry_attempts = 3\nrequest_retry_delays = [1, 3, 5, 8]\n",
    );

    let error = load_from_paths(Some(working_dir.as_path()), None)
        .expect_err("runtime retry delays longer than attempts should be rejected");

    assert!(error.to_string().contains("runtime.request_retry"));
}

#[test]
fn load_rejects_out_of_range_runtime_request_retry_values() {
    let attempts_dir = temp_test_dir("load-invalid-runtime-retry-attempts-working");
    write_config(
        &attempts_dir.join(".hunea").join("config.toml"),
        "[runtime]\nrequest_retry_attempts = 11\n",
    );
    let attempts_error = load_from_paths(Some(attempts_dir.as_path()), None)
        .expect_err("request_retry_attempts should reject values above 10");
    assert!(attempts_error.to_string().contains("between 1 and 10"));

    let delays_dir = temp_test_dir("load-invalid-runtime-retry-delays-working");
    write_config(
        &delays_dir.join(".hunea").join("config.toml"),
        "[runtime]\nrequest_retry_delays = [1, 1801]\n",
    );
    let delays_error = load_from_paths(Some(delays_dir.as_path()), None)
        .expect_err("request_retry_delays should reject values above 1800 seconds");
    assert!(delays_error.to_string().contains("between 1 and 1800"));
}

#[test]
fn load_rejects_out_of_range_runtime_request_timeout_values() {
    let zero_dir = temp_test_dir("load-invalid-runtime-timeout-zero-working");
    write_config(
        &zero_dir.join(".hunea").join("config.toml"),
        "[runtime]\nrequest_timeout_seconds = 0\n",
    );
    let zero_error = load_from_paths(Some(zero_dir.as_path()), None)
        .expect_err("request_timeout_seconds should reject zero");
    assert!(zero_error.to_string().contains("between 1 and 7200"));

    let too_large_dir = temp_test_dir("load-invalid-runtime-timeout-large-working");
    write_config(
        &too_large_dir.join(".hunea").join("config.toml"),
        "[runtime]\nrequest_timeout_seconds = 7201\n",
    );
    let too_large_error = load_from_paths(Some(too_large_dir.as_path()), None)
        .expect_err("request_timeout_seconds should reject values above 7200 seconds");
    assert!(too_large_error.to_string().contains("between 1 and 7200"));
}

#[test]
fn load_accepts_disabling_show_esc_interrupt_hint() {
    let working_dir = temp_test_dir("load-disable-show-esc-hint-working");
    write_config(
        &working_dir.join(".hunea").join("config.toml"),
        "[tui]\nshow_esc_interrupt_hint = false\n",
    );

    let config = load_from_paths(Some(working_dir.as_path()), None)
        .expect("show_esc_interrupt_hint should accept false");

    assert!(!config.tui.show_esc_interrupt_hint);
}

#[test]
fn load_accepts_enabling_debug_commands() {
    let working_dir = temp_test_dir("load-enable-debug-working");
    write_config(
        &working_dir.join(".hunea").join("config.toml"),
        "[debug]\nenabled = true\n",
    );

    let config =
        load_from_paths(Some(working_dir.as_path()), None).expect("debug should accept true");

    assert!(config.debug.enabled);
}

#[test]
fn load_rejects_invalid_esc_interrupt_presses() {
    let working_dir = temp_test_dir("load-invalid-esc-interrupt-working");
    write_config(
        &working_dir.join(".hunea").join("config.toml"),
        "[tui]\nesc_interrupt_presses = 4\n",
    );

    let error = load_from_paths(Some(working_dir.as_path()), None)
        .expect_err("esc_interrupt_presses should only accept 1, 2, or 3");

    assert!(error.to_string().contains("tui.esc_interrupt_presses"));
}

#[test]
fn load_accepts_enabling_print_transcript_on_exit() {
    let working_dir = temp_test_dir("load-enable-print-transcript-working");
    write_config(
        &working_dir.join(".hunea").join("config.toml"),
        "[tui]\nprint_transcript_on_exit = true\n",
    );

    let config = load_from_paths(Some(working_dir.as_path()), None)
        .expect("print_transcript_on_exit should accept true");

    assert!(config.tui.print_transcript_on_exit);
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
        &working_dir.join(".hunea").join("config.toml"),
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
        &working_dir.join(".hunea").join("config.toml"),
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
        &working_dir.join(".hunea").join("config.toml"),
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
        &working_dir.join(".hunea").join("config.toml"),
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
fn load_rejects_legacy_command_panel_mode_key() {
    let working_dir = temp_test_dir("load-rejects-legacy-command-panel-mode-working");
    write_config(
        &working_dir.join(".hunea").join("config.toml"),
        "[tui]\ncommand_panel_mode = \"inline\"\n",
    );

    let error = load_from_paths(Some(working_dir.as_path()), None)
        .expect_err("legacy command_panel_mode should be rejected explicitly");

    assert!(
        error.to_string().contains("command_panel_mode"),
        "unexpected error: {error}"
    );
}

#[test]
fn load_rejects_unknown_keys() {
    let working_dir = temp_test_dir("load-rejects-keys-working");
    write_config(
        &working_dir.join(".hunea").join("config.toml"),
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
        &working_dir.join(".hunea").join("config.toml"),
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
        &working_dir.join(".hunea").join("config.toml"),
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
    let path = std::env::temp_dir().join(format!("hunea-rust-{prefix}-{unique}"));
    fs::create_dir_all(&path).expect("temp test dir should be created");
    path
}

fn write_config(path: &Path, content: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("config parent dir should exist");
    }
    fs::write(path, content).expect("config file should be written");
}
