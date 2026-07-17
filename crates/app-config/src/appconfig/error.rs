use std::{fmt, io, path::PathBuf};

use super::{
    BRANCH_PICKER_LIST_ROWS_MAX, BRANCH_PICKER_LIST_ROWS_MIN, COMPOSER_UNDO_MAX_LIMIT,
    COMPOSER_UNDO_MIN_LIMIT, FILE_PICKER_POPUP_MAX_HEIGHT, FILE_PICKER_POPUP_MIN_HEIGHT,
    MESSAGE_HISTORY_LIMIT_MAX, MESSAGE_HISTORY_LIMIT_MIN,
};

/// `AppConfigError` 描述配置加载或校验失败。
#[derive(Debug)]
pub enum AppConfigError {
    Read {
        path: PathBuf,
        source: io::Error,
    },
    Decode {
        path: PathBuf,
        source: toml::de::Error,
    },
    Edit {
        path: PathBuf,
        source: toml_edit::TomlError,
    },
    Write {
        path: PathBuf,
        source: io::Error,
    },
    InvalidStyleMode {
        path: Option<PathBuf>,
        value: String,
    },
    InvalidMotionMode {
        path: Option<PathBuf>,
        value: String,
    },
    InvalidScrollAnimation {
        path: Option<PathBuf>,
        value: String,
    },
    InvalidStatusLineItem {
        path: Option<PathBuf>,
        value: String,
    },
    InvalidExternalEditorCommand {
        path: Option<PathBuf>,
    },
    ExternalEditorMustWait {
        path: Option<PathBuf>,
        command: String,
    },
    InvalidEscInterruptPresses {
        path: Option<PathBuf>,
        value: u8,
    },
    InvalidEscRewindMode {
        path: Option<PathBuf>,
        value: String,
    },
    InvalidKeyboardEnhancementMode {
        path: Option<PathBuf>,
        value: String,
    },
    InvalidFilePickerPopupHeight {
        path: Option<PathBuf>,
        value: usize,
    },
    InvalidBranchPickerListRows {
        path: Option<PathBuf>,
        value: usize,
    },
    InvalidComposerUndoLimit {
        path: Option<PathBuf>,
        value: usize,
    },
    InvalidMessageHistoryLimit {
        path: Option<PathBuf>,
        value: usize,
    },
    InvalidReasoningContentDisplay {
        path: Option<PathBuf>,
        value: String,
    },
    InvalidRuntimeRequestPolicy {
        path: Option<PathBuf>,
        reason: String,
    },
}

impl fmt::Display for AppConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read { path, source } => {
                write!(f, "read config file {}: {source}", path.display())
            }
            Self::Decode { path, source } => {
                write!(f, "decode config file {}: {source}", path.display())
            }
            Self::Edit { path, source } => {
                write!(f, "edit config file {}: {source}", path.display())
            }
            Self::Write { path, source } => {
                write!(f, "write config file {}: {source}", path.display())
            }
            Self::InvalidStyleMode {
                path: Some(path),
                value,
            } => write!(
                f,
                "validate config file {}: unknown tui.user_input_style {:?}",
                path.display(),
                value
            ),
            Self::InvalidStyleMode { path: None, value } => {
                write!(f, "unknown tui.user_input_style {:?}", value)
            }
            Self::InvalidMotionMode {
                path: Some(path),
                value,
            } => write!(
                f,
                "validate config file {}: unknown tui.motion {:?}",
                path.display(),
                value
            ),
            Self::InvalidMotionMode { path: None, value } => {
                write!(f, "unknown tui.motion {:?}", value)
            }
            Self::InvalidScrollAnimation {
                path: Some(path),
                value,
            } => write!(
                f,
                "validate config file {}: tui.scroll_animation must be \"off\", \"snappy\", \"fast\", \"smooth\", \"gentle\", or \"glide\", got {:?}",
                path.display(),
                value
            ),
            Self::InvalidScrollAnimation { path: None, value } => write!(
                f,
                "tui.scroll_animation must be \"off\", \"snappy\", \"fast\", \"smooth\", \"gentle\", or \"glide\", got {value:?}"
            ),
            Self::InvalidStatusLineItem {
                path: Some(path),
                value,
            } => write!(
                f,
                "validate config file {}: unknown tui.status_line item {:?}",
                path.display(),
                value
            ),
            Self::InvalidStatusLineItem { path: None, value } => {
                write!(f, "unknown tui.status_line item {:?}", value)
            }
            Self::InvalidExternalEditorCommand { path: Some(path) } => write!(
                f,
                "validate config file {}: invalid tui.external_editor command",
                path.display()
            ),
            Self::InvalidExternalEditorCommand { path: None } => {
                write!(f, "invalid tui.external_editor command")
            }
            Self::ExternalEditorMustWait {
                path: Some(path),
                command,
            } => write!(
                f,
                "validate config file {}: external editor must wait for close: {}",
                path.display(),
                command
            ),
            Self::ExternalEditorMustWait {
                path: None,
                command,
            } => {
                write!(f, "external editor must wait for close: {command}")
            }
            Self::InvalidEscInterruptPresses {
                path: Some(path),
                value,
            } => write!(
                f,
                "validate config file {}: tui.esc_interrupt_presses must be 1, 2, or 3, got {}",
                path.display(),
                value
            ),
            Self::InvalidEscInterruptPresses { path: None, value } => write!(
                f,
                "tui.esc_interrupt_presses must be 1, 2, or 3, got {value}"
            ),
            Self::InvalidEscRewindMode {
                path: Some(path),
                value,
            } => write!(
                f,
                "validate config file {}: tui.esc_rewind_mode must be \"coarse\" or \"entry\", got {:?}",
                path.display(),
                value
            ),
            Self::InvalidEscRewindMode { path: None, value } => write!(
                f,
                "tui.esc_rewind_mode must be \"coarse\" or \"entry\", got {value:?}"
            ),
            Self::InvalidKeyboardEnhancementMode {
                path: Some(path),
                value,
            } => write!(
                f,
                "validate config file {}: tui.keyboard_enhancement must be \"auto\", \"on\", or \"off\", got {:?}",
                path.display(),
                value
            ),
            Self::InvalidKeyboardEnhancementMode { path: None, value } => write!(
                f,
                "tui.keyboard_enhancement must be \"auto\", \"on\", or \"off\", got {value:?}"
            ),
            Self::InvalidFilePickerPopupHeight {
                path: Some(path),
                value,
            } => write!(
                f,
                "validate config file {}: tui.file_picker_popup_height must be between {} and {}, got {}",
                path.display(),
                FILE_PICKER_POPUP_MIN_HEIGHT,
                FILE_PICKER_POPUP_MAX_HEIGHT,
                value
            ),
            Self::InvalidFilePickerPopupHeight { path: None, value } => write!(
                f,
                "tui.file_picker_popup_height must be between {} and {}, got {value}",
                FILE_PICKER_POPUP_MIN_HEIGHT, FILE_PICKER_POPUP_MAX_HEIGHT
            ),
            Self::InvalidBranchPickerListRows {
                path: Some(path),
                value,
            } => write!(
                f,
                "validate config file {}: tui.branch_picker_list_rows must be between {} and {}, got {}",
                path.display(),
                BRANCH_PICKER_LIST_ROWS_MIN,
                BRANCH_PICKER_LIST_ROWS_MAX,
                value
            ),
            Self::InvalidBranchPickerListRows { path: None, value } => write!(
                f,
                "tui.branch_picker_list_rows must be between {} and {}, got {value}",
                BRANCH_PICKER_LIST_ROWS_MIN, BRANCH_PICKER_LIST_ROWS_MAX
            ),
            Self::InvalidComposerUndoLimit {
                path: Some(path),
                value,
            } => write!(
                f,
                "validate config file {}: tui.composer_undo_limit must be between {} and {}, got {}",
                path.display(),
                COMPOSER_UNDO_MIN_LIMIT,
                COMPOSER_UNDO_MAX_LIMIT,
                value
            ),
            Self::InvalidComposerUndoLimit { path: None, value } => write!(
                f,
                "tui.composer_undo_limit must be between {} and {}, got {value}",
                COMPOSER_UNDO_MIN_LIMIT, COMPOSER_UNDO_MAX_LIMIT
            ),
            Self::InvalidMessageHistoryLimit {
                path: Some(path),
                value,
            } => write!(
                f,
                "validate config file {}: tui.message_history_limit must be between {} and {}, got {}",
                path.display(),
                MESSAGE_HISTORY_LIMIT_MIN,
                MESSAGE_HISTORY_LIMIT_MAX,
                value
            ),
            Self::InvalidMessageHistoryLimit { path: None, value } => write!(
                f,
                "tui.message_history_limit must be between {} and {}, got {value}",
                MESSAGE_HISTORY_LIMIT_MIN, MESSAGE_HISTORY_LIMIT_MAX
            ),
            Self::InvalidReasoningContentDisplay {
                path: Some(path),
                value,
            } => write!(
                f,
                "validate config file {}: unknown tui.reasoning_content_display {:?}",
                path.display(),
                value
            ),
            Self::InvalidReasoningContentDisplay { path: None, value } => {
                write!(f, "unknown tui.reasoning_content_display {:?}", value)
            }
            Self::InvalidRuntimeRequestPolicy {
                path: Some(path),
                reason,
            } => write!(
                f,
                "validate config file {}: invalid runtime.request policy: {}",
                path.display(),
                reason
            ),
            Self::InvalidRuntimeRequestPolicy { path: None, reason } => {
                write!(f, "invalid runtime.request policy: {reason}")
            }
        }
    }
}

impl std::error::Error for AppConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Read { source, .. } => Some(source),
            Self::Decode { source, .. } => Some(source),
            Self::Edit { source, .. } => Some(source),
            Self::Write { source, .. } => Some(source),
            Self::InvalidStyleMode { .. }
            | Self::InvalidMotionMode { .. }
            | Self::InvalidScrollAnimation { .. }
            | Self::InvalidStatusLineItem { .. }
            | Self::InvalidExternalEditorCommand { .. }
            | Self::ExternalEditorMustWait { .. }
            | Self::InvalidEscInterruptPresses { .. }
            | Self::InvalidEscRewindMode { .. }
            | Self::InvalidKeyboardEnhancementMode { .. }
            | Self::InvalidFilePickerPopupHeight { .. }
            | Self::InvalidBranchPickerListRows { .. }
            | Self::InvalidComposerUndoLimit { .. }
            | Self::InvalidMessageHistoryLimit { .. }
            | Self::InvalidReasoningContentDisplay { .. }
            | Self::InvalidRuntimeRequestPolicy { .. } => None,
        }
    }
}
