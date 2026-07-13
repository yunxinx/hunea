use std::path::PathBuf;

use runtime_domain::{
    model_catalog::{ModelCatalog, ModelSelection},
    phrases::StatusPhraseOrder,
    prompt_assembly::PromptAssemblyManagerSnapshot,
};

use crate::{
    MotionMode, ReasoningDisplayMode,
    composer::DEFAULT_COMPOSER_UNDO_LIMIT,
    entry_tree::BRANCH_PICKER_LIST_ROWS_DEFAULT,
    file_picker::{FILE_PICKER_POPUP_MAX_HEIGHT, FILE_PICKER_POPUP_MIN_HEIGHT},
    status_line::StatusLineItem,
    status_phrases::default_status_phrases,
    style_mode::StyleMode,
};

/// `EscRewindMode` 表示空 composer 下 `Esc` 进入哪类回溯交互。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EscRewindMode {
    Coarse,
    Entry,
}

/// `KeyboardEnhancementPreference` 控制 kitty keyboard enhancement 的启用策略：
/// `Auto` 按环境自动判定（WSL 内的 VSCode 终端禁用），`On`/`Off` 强制指定。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyboardEnhancementPreference {
    Auto,
    On,
    Off,
}

/// `ModelOptions` 表示创建 TUI 模型时可配置的样式与状态行选项。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelOptions {
    pub working_dir: Option<PathBuf>,
    pub style_mode: StyleMode,
    pub motion_mode: MotionMode,
    pub status_line_items: Vec<StatusLineItem>,
    pub status_line_2_items: Vec<StatusLineItem>,
    pub external_editor: Vec<String>,
    pub external_editor_hint: String,
    pub show_external_editor_helper: bool,
    pub copy_on_mouse_selection_release: bool,
    pub swap_enter_and_send: bool,
    pub ctrl_c_clears_input: bool,
    pub esc_interrupt_presses: u8,
    pub esc_rewind_mode: EscRewindMode,
    /// 由 runner 在进入终端会话时消费，决定是否 Push keyboard enhancement flags；
    /// 不进入 `Model` 状态。
    pub keyboard_enhancement: KeyboardEnhancementPreference,
    pub show_esc_interrupt_hint: bool,
    pub file_picker_popup_height: u16,
    pub branch_picker_list_rows: u16,
    pub composer_undo_limit: usize,
    pub message_history_limit: usize,
    pub show_reasoning_content: bool,
    pub reasoning_display_mode: ReasoningDisplayMode,
    pub debug_commands_enabled: bool,
    pub model_catalog: ModelCatalog,
    pub selected_model: Option<ModelSelection>,
    pub status_phrases: Vec<String>,
    pub status_phrase_order: StatusPhraseOrder,
    pub prompt_assembly: Option<PromptAssemblyManagerSnapshot>,
}

impl Default for ModelOptions {
    fn default() -> Self {
        Self {
            working_dir: None,
            style_mode: StyleMode::default(),
            motion_mode: MotionMode::default(),
            status_line_items: Vec::new(),
            status_line_2_items: Vec::new(),
            external_editor: Vec::new(),
            external_editor_hint: String::new(),
            show_external_editor_helper: true,
            copy_on_mouse_selection_release: false,
            swap_enter_and_send: false,
            ctrl_c_clears_input: true,
            esc_interrupt_presses: 2,
            esc_rewind_mode: EscRewindMode::Coarse,
            keyboard_enhancement: KeyboardEnhancementPreference::Auto,
            show_esc_interrupt_hint: true,
            file_picker_popup_height: 7
                .clamp(FILE_PICKER_POPUP_MIN_HEIGHT, FILE_PICKER_POPUP_MAX_HEIGHT),
            branch_picker_list_rows: BRANCH_PICKER_LIST_ROWS_DEFAULT,
            composer_undo_limit: DEFAULT_COMPOSER_UNDO_LIMIT,
            message_history_limit: 100,
            show_reasoning_content: false,
            reasoning_display_mode: ReasoningDisplayMode::Collapsed,
            debug_commands_enabled: false,
            model_catalog: ModelCatalog::default(),
            selected_model: None,
            status_phrases: default_status_phrases(),
            status_phrase_order: StatusPhraseOrder::Random,
            prompt_assembly: None,
        }
    }
}
