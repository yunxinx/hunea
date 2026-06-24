use super::{
    BRANCH_PICKER_LIST_ROWS_DEFAULT, COMPOSER_UNDO_DEFAULT_LIMIT, Config, DebugConfig,
    MESSAGE_HISTORY_LIMIT_DEFAULT, ReasoningContentDisplay, RuntimeConfig, TuiConfig,
    UserInputStyle,
};

impl Config {
    pub(super) fn default_config() -> Self {
        Self {
            tui: TuiConfig {
                user_input_style: UserInputStyle::Cx,
                status_line: Vec::new(),
                status_line_2: Vec::new(),
                external_editor: Vec::new(),
                show_external_editor_helper: true,
                copy_on_mouse_selection_release: false,
                swap_enter_and_send: false,
                ctrl_c_clears_input: true,
                esc_interrupt_presses: 2,
                esc_rewind_mode: super::EscRewindMode::Coarse,
                show_esc_interrupt_hint: true,
                file_picker_popup_height: 7,
                branch_picker_list_rows: BRANCH_PICKER_LIST_ROWS_DEFAULT,
                composer_undo_limit: COMPOSER_UNDO_DEFAULT_LIMIT,
                message_history_limit: MESSAGE_HISTORY_LIMIT_DEFAULT,
                print_transcript_on_exit: false,
                show_reasoning_content: false,
                reasoning_content_display: ReasoningContentDisplay::Collapsed,
            },
            runtime: RuntimeConfig {
                request_retry_attempts: 3,
                request_retry_delays: vec![1, 2, 3],
                request_timeout_seconds: 120,
                tool_max_turns: None,
                allow_managed_rg: None,
                allow_managed_fd: None,
            },
            debug: DebugConfig { enabled: false },
        }
    }
}
