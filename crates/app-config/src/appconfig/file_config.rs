use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct FileConfig {
    #[serde(default)]
    pub(super) tui: FileTuiConfig,
    #[serde(default)]
    pub(super) runtime: FileRuntimeConfig,
    #[serde(default)]
    pub(super) debug: FileDebugConfig,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct FileTuiConfig {
    pub(super) user_input_style: Option<String>,
    pub(super) motion: Option<String>,
    pub(super) status_line: Option<Vec<String>>,
    pub(super) status_line_2: Option<Vec<String>>,
    pub(super) external_editor: Option<Vec<String>>,
    pub(super) show_external_editor_helper: Option<bool>,
    pub(super) copy_on_mouse_selection_release: Option<bool>,
    pub(super) swap_enter_and_send: Option<bool>,
    pub(super) ctrl_c_clears_input: Option<bool>,
    pub(super) esc_interrupt_presses: Option<u8>,
    pub(super) esc_rewind_mode: Option<String>,
    pub(super) command_menu_mode: Option<String>,
    pub(super) command_menu_rows: Option<usize>,
    pub(super) keyboard_enhancement: Option<String>,
    pub(super) show_esc_interrupt_hint: Option<bool>,
    pub(super) file_picker_popup_height: Option<usize>,
    pub(super) branch_picker_list_rows: Option<usize>,
    pub(super) composer_undo_limit: Option<usize>,
    pub(super) message_history_limit: Option<usize>,
    pub(super) print_transcript_on_exit: Option<bool>,
    pub(super) show_reasoning_content: Option<bool>,
    pub(super) reasoning_content_display: Option<String>,
    pub(super) scroll_animation: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct FileRuntimeConfig {
    pub(super) request_retry_attempts: Option<usize>,
    pub(super) request_retry_delays: Option<Vec<u64>>,
    pub(super) request_timeout_seconds: Option<u64>,
    pub(super) tool_max_turns: Option<usize>,
    pub(super) allow_managed_rg: Option<bool>,
    pub(super) allow_managed_fd: Option<bool>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct FileDebugConfig {
    pub(super) enabled: Option<bool>,
}
