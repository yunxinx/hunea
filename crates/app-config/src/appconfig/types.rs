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
/// 悬浮命令菜单至少显示 7 个命令行，与内联斜杠菜单默认密度一致。
///
/// 与 `terminal-ui` 的 `COMMAND_MENU_ROWS_*` 数值必须保持一致；
/// `terminal-app` 有同步测试防止两边漂移。
pub const COMMAND_MENU_ROWS_MIN: u16 = 7;
/// 悬浮命令菜单最多显示 21 个命令行，避免遮挡过多上下文。
pub const COMMAND_MENU_ROWS_MAX: u16 = 21;
/// 悬浮命令菜单默认显示 7 个命令行。
pub const COMMAND_MENU_ROWS_DEFAULT: u16 = 7;
/// Composer undo 至少保留 1 条，确保开启撤回时有明确效果。
pub const COMPOSER_UNDO_MIN_LIMIT: usize = 1;
/// Composer undo 最多保留 200 条，避免配置误填导致草稿快照无限增长。
pub const COMPOSER_UNDO_MAX_LIMIT: usize = 200;
/// Composer undo 默认保留 50 条，覆盖常见短编辑而不制造过多隐藏状态。
pub const COMPOSER_UNDO_DEFAULT_LIMIT: usize = 50;
/// Message history 至少保留 100 条。
pub const MESSAGE_HISTORY_LIMIT_MIN: usize = 100;
/// Message history 最多保留 1000 条。
pub const MESSAGE_HISTORY_LIMIT_MAX: usize = 1000;
/// Message history 默认保留 100 条。
pub const MESSAGE_HISTORY_LIMIT_DEFAULT: usize = 100;

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
    pub motion: MotionMode,
    pub status_line: Vec<String>,
    pub status_line_2: Vec<String>,
    pub external_editor: Vec<String>,
    pub show_external_editor_helper: bool,
    pub copy_on_mouse_selection_release: bool,
    pub swap_enter_and_send: bool,
    pub ctrl_c_clears_input: bool,
    pub esc_interrupt_presses: u8,
    pub esc_rewind_mode: EscRewindMode,
    pub command_menu_mode: CommandMenuMode,
    pub command_menu_rows: u16,
    pub keyboard_enhancement: KeyboardEnhancementMode,
    pub show_esc_interrupt_hint: bool,
    pub file_picker_popup_height: u16,
    pub branch_picker_list_rows: u16,
    pub composer_undo_limit: usize,
    pub message_history_limit: usize,
    pub print_transcript_on_exit: bool,
    pub show_reasoning_content: bool,
    pub reasoning_content_display: ReasoningContentDisplay,
    /// 滚轮平滑滚动的手感档位；`Off` 恢复固定步长瞬时跳变（无加速度），
    /// 是平滑滚动的完整逃生通道；`motion = "reduced"` 时无论取值均为瞬时。
    pub scroll_animation: ScrollAnimationMode,
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

/// `MotionMode` 控制TUI装饰性动画是否以完整形式运行。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MotionMode {
    Full,
    Reduced,
}

impl MotionMode {
    pub(super) fn parse(value: &str) -> Result<Self, super::AppConfigError> {
        match value {
            "full" => Ok(Self::Full),
            "reduced" => Ok(Self::Reduced),
            other => Err(super::AppConfigError::InvalidMotionMode {
                path: None,
                value: other.to_string(),
            }),
        }
    }
}

/// `ScrollAnimationMode` 表示滚轮平滑滚动的手感档位。
///
/// 五个动画档位参数单调有序、观感可区分（档位表在 terminal-ui 的
/// `smooth_scroll.rs`）；语义化档位替代裸参数暴露，覆盖不同终端/鼠标驱动
/// 组合的滚轮事件流特征差异。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrollAnimationMode {
    Off,
    Snappy,
    Fast,
    Smooth,
    Gentle,
    Glide,
}

impl ScrollAnimationMode {
    pub(super) fn parse(value: &str) -> Result<Self, super::AppConfigError> {
        match value {
            "off" => Ok(Self::Off),
            "snappy" => Ok(Self::Snappy),
            "fast" => Ok(Self::Fast),
            "smooth" => Ok(Self::Smooth),
            "gentle" => Ok(Self::Gentle),
            "glide" => Ok(Self::Glide),
            other => Err(super::AppConfigError::InvalidScrollAnimation {
                path: None,
                value: other.to_string(),
            }),
        }
    }
}

/// `EscRewindMode` 表示空 composer 下 `Esc` 进入哪类回溯交互。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EscRewindMode {
    Coarse,
    Entry,
}

/// `CommandMenuMode` 表示命令菜单的触发方式：
/// `Slash` 仅 `/` 开头内联斜杠菜单；`Floating` 仅 `Ctrl+O` 悬浮命令菜单
/// （`/` 回落为普通文本）；`Both` 两种方式同时可用。
///
/// 与 `terminal-ui::CommandMenuMode` 变体一一对应，由 `terminal-app` 映射；
/// 新增变体时两边与映射必须同步更新。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandMenuMode {
    Slash,
    Floating,
    Both,
}

/// `KeyboardEnhancementMode` 控制 kitty keyboard enhancement 的启用策略：
/// `Auto` 按环境自动判定（WSL 内的 VSCode 终端禁用），`On`/`Off` 强制指定。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyboardEnhancementMode {
    Auto,
    On,
    Off,
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
    /// 单次 provider HTTP 请求的 idle timeout（秒）：约束建连等待与流式响应
    /// 相邻数据块的空闲间隔，收到数据即重置；不限制一个 turn 的总时长。
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

impl CommandMenuMode {
    pub(super) fn parse(value: &str) -> Result<Self, super::AppConfigError> {
        match value {
            "slash" => Ok(Self::Slash),
            "floating" => Ok(Self::Floating),
            "both" => Ok(Self::Both),
            other => Err(super::AppConfigError::InvalidCommandMenuMode {
                path: None,
                value: other.to_string(),
            }),
        }
    }
}

impl KeyboardEnhancementMode {
    pub(super) fn parse(value: &str) -> Result<Self, super::AppConfigError> {
        match value {
            "auto" => Ok(Self::Auto),
            "on" => Ok(Self::On),
            "off" => Ok(Self::Off),
            other => Err(super::AppConfigError::InvalidKeyboardEnhancementMode {
                path: None,
                value: other.to_string(),
            }),
        }
    }
}
