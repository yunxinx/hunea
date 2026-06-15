/// @ 文件选择浮窗至少需要 3 行，避免列表在导航时过于局促。
pub const FILE_PICKER_POPUP_MIN_HEIGHT: u16 = 3;
/// @ 文件选择浮窗最多显示 21 行，避免覆盖过多上下文。
pub const FILE_PICKER_POPUP_MAX_HEIGHT: u16 = 21;
/// Branch picker 至少显示 3 个分支行，保证有可导航空间。
pub const BRANCH_PICKER_LIST_ROWS_MIN: u16 = 3;
/// Branch picker 最多显示 14 个分支行，避免遮挡 `/tree` 过多上下文。
pub const BRANCH_PICKER_LIST_ROWS_MAX: u16 = 14;
/// Branch picker 默认显示 7 个分支行，与 file picker 默认密度一致。
pub const BRANCH_PICKER_LIST_ROWS_DEFAULT: u16 = 7;
/// Composer undo 至少保留 1 条，确保开启撤回时有明确效果。
pub const COMPOSER_UNDO_MIN_LIMIT: usize = 1;
/// Composer undo 最多保留 200 条，避免配置误填导致草稿快照无限增长。
pub const COMPOSER_UNDO_MAX_LIMIT: usize = 200;
/// Composer undo 默认保留 50 条，覆盖常见短编辑而不制造过多隐藏状态。
pub const COMPOSER_UNDO_DEFAULT_LIMIT: usize = 50;

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
    pub esc_rewind_mode: EscRewindMode,
    pub show_esc_interrupt_hint: bool,
    pub file_picker_popup_height: u16,
    pub branch_picker_list_rows: u16,
    pub composer_undo_limit: usize,
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

/// `EscRewindMode` 表示空 composer 下 `Esc` 进入哪类回溯交互。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EscRewindMode {
    Coarse,
    Entry,
}

/// `ReasoningContentDisplay` 表示思维链内容的默认展示方式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReasoningContentDisplay {
    Collapsed,
    Expanded,
    ExpandedSimplified,
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
            "expanded-simplified" => Ok(Self::ExpandedSimplified),
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

impl EscRewindMode {
    pub(super) fn parse(value: &str) -> Result<Self, super::AppConfigError> {
        match value {
            "coarse" => Ok(Self::Coarse),
            "entry" => Ok(Self::Entry),
            other => Err(super::AppConfigError::InvalidEscRewindMode {
                path: None,
                value: other.to_string(),
            }),
        }
    }
}
