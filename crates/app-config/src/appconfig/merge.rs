use std::{fs, io, path::Path};

use super::{
    error::AppConfigError,
    file_config::{FileConfig, FileRuntimeConfig},
    types::{
        Config, EscRewindMode, KeyboardEnhancementMode, MotionMode, ReasoningContentDisplay,
        RuntimeConfig, ScrollAnimationMode, UserInputStyle,
    },
    validate::{
        normalize_request_retry_delays, validate_branch_picker_list_rows,
        validate_composer_undo_limit, validate_external_editor, validate_file_picker_popup_height,
        validate_message_history_limit, validate_request_retry_attempts,
        validate_request_timeout_seconds, validate_status_line_items_for_path,
        validate_tool_max_turns,
    },
};

/// 配置文件加载结果，区分成功、跳过、可降级错误。
///
/// 用 outcome 而不是直接 `Err` 返回 Read 失败，是为了让上层在多源 merge 时
/// 能“收集 warning 后继续”，而不是第一个不可读文件就中断整个加载链。
/// Decode/Validation 仍走 `Err`：那是配置内容错误，继续 merge 只会掩盖问题。
pub(super) enum ConfigFileLoadOutcome {
    /// 成功加载并 merge 到 config
    Loaded,
    /// 文件不存在，跳过（config 未被修改）
    Skipped,
    /// 可降级错误：权限/IO 错误，config 未被修改，收集后继续尝试其他源
    Downgradable(AppConfigError),
}

pub(super) fn merge_config_file(
    config: &mut Config,
    path: &Path,
    reasoning_content_display_configured: &mut bool,
) -> Result<ConfigFileLoadOutcome, AppConfigError> {
    let content = match fs::read_to_string(path) {
        Ok(content) => content,
        // 缺失是常态（用户可能只配了全局或只配了工作区），不是错误。
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(ConfigFileLoadOutcome::Skipped);
        }
        // 权限/磁盘等环境问题：降级，让其他配置源或默认值接管。
        Err(source) => {
            return Ok(ConfigFileLoadOutcome::Downgradable(AppConfigError::Read {
                path: path.to_path_buf(),
                source,
            }));
        }
    };

    let file_config: FileConfig =
        toml::from_str(&content).map_err(|source| AppConfigError::Decode {
            path: path.to_path_buf(),
            source,
        })?;
    let enables_reasoning_without_display =
        matches!(file_config.tui.show_reasoning_content, Some(true))
            && file_config.tui.reasoning_content_display.is_none();

    if let Some(style) = file_config.tui.user_input_style {
        config.tui.user_input_style =
            UserInputStyle::parse(&style).map_err(|error| match error {
                AppConfigError::InvalidStyleMode { value, .. } => {
                    AppConfigError::InvalidStyleMode {
                        path: Some(path.to_path_buf()),
                        value,
                    }
                }
                other => other,
            })?;
    }

    if let Some(motion) = file_config.tui.motion {
        config.tui.motion = MotionMode::parse(&motion).map_err(|error| match error {
            AppConfigError::InvalidMotionMode { value, .. } => AppConfigError::InvalidMotionMode {
                path: Some(path.to_path_buf()),
                value,
            },
            other => other,
        })?;
    }

    if let Some(items) = file_config.tui.status_line {
        validate_status_line_items_for_path(&items, path)?;
        config.tui.status_line = items;
    }

    if let Some(items) = file_config.tui.status_line_2 {
        validate_status_line_items_for_path(&items, path)?;
        config.tui.status_line_2 = items;
    }

    if let Some(command) = file_config.tui.external_editor {
        validate_external_editor(&command).map_err(|error| match error {
            AppConfigError::InvalidExternalEditorCommand { .. } => {
                AppConfigError::InvalidExternalEditorCommand {
                    path: Some(path.to_path_buf()),
                }
            }
            AppConfigError::ExternalEditorMustWait { command, .. } => {
                AppConfigError::ExternalEditorMustWait {
                    path: Some(path.to_path_buf()),
                    command,
                }
            }
            other => other,
        })?;
        config.tui.external_editor = command;
    }

    if let Some(show_helper) = file_config.tui.show_external_editor_helper {
        config.tui.show_external_editor_helper = show_helper;
    }

    if let Some(copy_on_release) = file_config.tui.copy_on_mouse_selection_release {
        config.tui.copy_on_mouse_selection_release = copy_on_release;
    }

    if let Some(swap_enter_and_send) = file_config.tui.swap_enter_and_send {
        config.tui.swap_enter_and_send = swap_enter_and_send;
    }

    if let Some(ctrl_c_clears_input) = file_config.tui.ctrl_c_clears_input {
        config.tui.ctrl_c_clears_input = ctrl_c_clears_input;
    }

    if let Some(esc_interrupt_presses) = file_config.tui.esc_interrupt_presses {
        if !(1..=3).contains(&esc_interrupt_presses) {
            return Err(AppConfigError::InvalidEscInterruptPresses {
                path: Some(path.to_path_buf()),
                value: esc_interrupt_presses,
            });
        }
        config.tui.esc_interrupt_presses = esc_interrupt_presses;
    }

    if let Some(esc_rewind_mode) = file_config.tui.esc_rewind_mode {
        config.tui.esc_rewind_mode =
            EscRewindMode::parse(&esc_rewind_mode).map_err(|error| match error {
                AppConfigError::InvalidEscRewindMode { value, .. } => {
                    AppConfigError::InvalidEscRewindMode {
                        path: Some(path.to_path_buf()),
                        value,
                    }
                }
                other => other,
            })?;
    }

    if let Some(keyboard_enhancement) = file_config.tui.keyboard_enhancement {
        config.tui.keyboard_enhancement = KeyboardEnhancementMode::parse(&keyboard_enhancement)
            .map_err(|error| match error {
                AppConfigError::InvalidKeyboardEnhancementMode { value, .. } => {
                    AppConfigError::InvalidKeyboardEnhancementMode {
                        path: Some(path.to_path_buf()),
                        value,
                    }
                }
                other => other,
            })?;
    }

    if let Some(show_esc_interrupt_hint) = file_config.tui.show_esc_interrupt_hint {
        config.tui.show_esc_interrupt_hint = show_esc_interrupt_hint;
    }

    if let Some(height) = file_config.tui.file_picker_popup_height {
        config.tui.file_picker_popup_height = validate_file_picker_popup_height(height, path)?;
    }

    if let Some(rows) = file_config.tui.branch_picker_list_rows {
        config.tui.branch_picker_list_rows = validate_branch_picker_list_rows(rows, path)?;
    }

    if let Some(limit) = file_config.tui.composer_undo_limit {
        config.tui.composer_undo_limit = validate_composer_undo_limit(limit, path)?;
    }

    if let Some(limit) = file_config.tui.message_history_limit {
        config.tui.message_history_limit = validate_message_history_limit(limit, path)?;
    }

    if let Some(print_transcript_on_exit) = file_config.tui.print_transcript_on_exit {
        config.tui.print_transcript_on_exit = print_transcript_on_exit;
    }

    if let Some(show_reasoning_content) = file_config.tui.show_reasoning_content {
        config.tui.show_reasoning_content = show_reasoning_content;
    }

    if let Some(reasoning_content_display) = file_config.tui.reasoning_content_display {
        config.tui.reasoning_content_display = ReasoningContentDisplay::parse(
            &reasoning_content_display,
        )
        .map_err(|error| match error {
            AppConfigError::InvalidReasoningContentDisplay { value, .. } => {
                AppConfigError::InvalidReasoningContentDisplay {
                    path: Some(path.to_path_buf()),
                    value,
                }
            }
            other => other,
        })?;
        *reasoning_content_display_configured = true;
    } else if enables_reasoning_without_display && !*reasoning_content_display_configured {
        config.tui.reasoning_content_display = ReasoningContentDisplay::Expanded;
    }

    if let Some(scroll_animation) = file_config.tui.scroll_animation {
        config.tui.scroll_animation =
            ScrollAnimationMode::parse(&scroll_animation).map_err(|error| match error {
                AppConfigError::InvalidScrollAnimation { value, .. } => {
                    AppConfigError::InvalidScrollAnimation {
                        path: Some(path.to_path_buf()),
                        value,
                    }
                }
                other => other,
            })?;
    }

    merge_runtime_config(&mut config.runtime, file_config.runtime, path)?;

    if let Some(enabled) = file_config.debug.enabled {
        config.debug.enabled = enabled;
    }

    Ok(ConfigFileLoadOutcome::Loaded)
}

fn merge_runtime_config(
    config: &mut RuntimeConfig,
    file_config: FileRuntimeConfig,
    path: &Path,
) -> Result<(), AppConfigError> {
    if file_config.request_retry_attempts.is_none()
        && file_config.request_retry_delays.is_none()
        && file_config.request_timeout_seconds.is_none()
        && file_config.tool_max_turns.is_none()
        && file_config.allow_managed_rg.is_none()
        && file_config.allow_managed_fd.is_none()
    {
        return Ok(());
    }

    let has_explicit_delays = file_config.request_retry_delays.is_some();
    let attempts = match file_config.request_retry_attempts {
        Some(attempts) => attempts,
        None => file_config
            .request_retry_delays
            .as_ref()
            .map(Vec::len)
            .unwrap_or(config.request_retry_attempts),
    };
    validate_request_retry_attempts(attempts, path)?;

    let mut delays = file_config
        .request_retry_delays
        .unwrap_or_else(|| config.request_retry_delays.clone());
    normalize_request_retry_delays(&mut delays, attempts, has_explicit_delays, path)?;

    let timeout_seconds = file_config
        .request_timeout_seconds
        .unwrap_or(config.request_timeout_seconds);
    validate_request_timeout_seconds(timeout_seconds, path)?;

    let tool_max_turns = file_config.tool_max_turns.or(config.tool_max_turns);
    if let Some(tool_max_turns) = tool_max_turns {
        validate_tool_max_turns(tool_max_turns, path)?;
    }

    config.request_retry_attempts = attempts;
    config.request_retry_delays = delays;
    config.request_timeout_seconds = timeout_seconds;
    config.tool_max_turns = tool_max_turns;
    if let Some(allow_managed_rg) = file_config.allow_managed_rg {
        config.allow_managed_rg = Some(allow_managed_rg);
    }
    if let Some(allow_managed_fd) = file_config.allow_managed_fd {
        config.allow_managed_fd = Some(allow_managed_fd);
    }
    Ok(())
}
