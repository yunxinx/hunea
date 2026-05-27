/// @ 文件选择浮窗至少需要 3 行，避免列表在导航时过于局促。
pub const FILE_PICKER_POPUP_MIN_HEIGHT: u16 = 3;
/// @ 文件选择浮窗最多显示 21 行，避免覆盖过多上下文。
pub const FILE_PICKER_POPUP_MAX_HEIGHT: u16 = 21;

/// `Config` 表示当前 hunea 支持的启动配置。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub tui: TuiConfig,
    pub runtime: RuntimeConfig,
    pub debug: DebugConfig,
}

/// `TuiConfig` 表示 TUI 相关配置。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TuiConfig {
    pub user_input_style: UserInputStyle,
    pub status_line: Vec<String>,
    pub status_line_2: Vec<String>,
    pub external_editor: Vec<String>,
    pub show_external_editor_helper: bool,
    pub copy_on_mouse_selection_release: bool,
    pub swap_enter_and_send: bool,
    pub ctrl_c_clears_input: bool,
    pub esc_interrupt_presses: u8,
    pub show_esc_interrupt_hint: bool,
    pub file_picker_popup_height: u16,
    pub print_transcript_on_exit: bool,
    pub show_reasoning_content: bool,
    pub reasoning_content_display: ReasoningContentDisplay,
}

/// `DebugConfig` 表示仅用于本地调试与界面预览的配置。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DebugConfig {
    pub enabled: bool,
}

/// `UserInputStyle` 表示用户输入区与用户消息的展示模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserInputStyle {
    Cx,
    Cc,
    Ms,
}

/// `ReasoningContentDisplay` 表示思维链内容的默认展示方式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReasoningContentDisplay {
    Collapsed,
    Expanded,
    Snippet,
}

/// `RuntimeConfig` 表示可被多个 runtime 复用的执行策略。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeConfig {
    pub request_retry_attempts: usize,
    pub request_retry_delays: Vec<u64>,
    pub request_timeout_seconds: u64,
    pub tool_max_turns: Option<usize>,
    pub allow_managed_rg: Option<bool>,
    pub allow_managed_fd: Option<bool>,
}

impl ReasoningContentDisplay {
    pub(super) fn parse(value: &str) -> Result<Self, super::AppConfigError> {
        match value {
            "collapsed" => Ok(Self::Collapsed),
            "expanded" => Ok(Self::Expanded),
            "snippet" => Ok(Self::Snippet),
            other => Err(super::AppConfigError::InvalidReasoningContentDisplay {
                path: None,
                value: other.to_string(),
            }),
        }
    }
}

impl UserInputStyle {
    pub(super) fn parse(value: &str) -> Result<Self, super::AppConfigError> {
        match value {
            "cx" => Ok(Self::Cx),
            "cc" => Ok(Self::Cc),
            "ms" => Ok(Self::Ms),
            other => Err(super::AppConfigError::InvalidStyleMode {
                path: None,
                value: other.to_string(),
            }),
        }
    }
}
