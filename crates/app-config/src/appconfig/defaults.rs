use super::{
    Config, DebugConfig, ReasoningContentDisplay, RuntimeConfig, TuiConfig, UserInputStyle,
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
                show_esc_interrupt_hint: true,
                file_picker_popup_height: 7,
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
