use std::path::Path;

use runtime_domain::envinfo;

use super::{
    COMPOSER_UNDO_MAX_LIMIT, COMPOSER_UNDO_MIN_LIMIT, MESSAGE_HISTORY_LIMIT_MAX,
    MESSAGE_HISTORY_LIMIT_MIN,
    error::AppConfigError,
    types::{FILE_PICKER_POPUP_MAX_HEIGHT, FILE_PICKER_POPUP_MIN_HEIGHT},
};

pub(super) fn validate_status_line_items_for_path(
    items: &[String],
    path: &Path,
) -> Result<(), AppConfigError> {
    validate_status_line_items(items).map_err(|error| match error {
        AppConfigError::InvalidStatusLineItem { value, .. } => {
            AppConfigError::InvalidStatusLineItem {
                path: Some(path.to_path_buf()),
                value,
            }
        }
        other => other,
    })
}

pub(super) fn validate_request_retry_attempts(
    attempts: usize,
    path: &Path,
) -> Result<(), AppConfigError> {
    if (1..=10).contains(&attempts) {
        return Ok(());
    }

    Err(AppConfigError::InvalidRuntimeRequestPolicy {
        path: Some(path.to_path_buf()),
        reason: format!("runtime.request_retry_attempts must be between 1 and 10, got {attempts}"),
    })
}

pub(super) fn validate_file_picker_popup_height(
    value: usize,
    path: &Path,
) -> Result<u16, AppConfigError> {
    if !(usize::from(FILE_PICKER_POPUP_MIN_HEIGHT)..=usize::from(FILE_PICKER_POPUP_MAX_HEIGHT))
        .contains(&value)
    {
        return Err(AppConfigError::InvalidFilePickerPopupHeight {
            path: Some(path.to_path_buf()),
            value,
        });
    }

    Ok(value as u16)
}

pub(super) fn validate_branch_picker_list_rows(
    value: usize,
    path: &Path,
) -> Result<u16, AppConfigError> {
    if !(usize::from(super::BRANCH_PICKER_LIST_ROWS_MIN)
        ..=usize::from(super::BRANCH_PICKER_LIST_ROWS_MAX))
        .contains(&value)
    {
        return Err(AppConfigError::InvalidBranchPickerListRows {
            path: Some(path.to_path_buf()),
            value,
        });
    }

    Ok(value as u16)
}

pub(super) fn validate_composer_undo_limit(
    value: usize,
    path: &Path,
) -> Result<usize, AppConfigError> {
    if !(COMPOSER_UNDO_MIN_LIMIT..=COMPOSER_UNDO_MAX_LIMIT).contains(&value) {
        return Err(AppConfigError::InvalidComposerUndoLimit {
            path: Some(path.to_path_buf()),
            value,
        });
    }

    Ok(value)
}

pub(super) fn validate_message_history_limit(
    value: usize,
    path: &Path,
) -> Result<usize, AppConfigError> {
    if !(MESSAGE_HISTORY_LIMIT_MIN..=MESSAGE_HISTORY_LIMIT_MAX).contains(&value) {
        return Err(AppConfigError::InvalidMessageHistoryLimit {
            path: Some(path.to_path_buf()),
            value,
        });
    }

    Ok(value)
}

pub(super) fn validate_request_timeout_seconds(
    timeout_seconds: u64,
    path: &Path,
) -> Result<(), AppConfigError> {
    if (1..=7200).contains(&timeout_seconds) {
        return Ok(());
    }

    Err(AppConfigError::InvalidRuntimeRequestPolicy {
        path: Some(path.to_path_buf()),
        reason: format!(
            "runtime.request_timeout_seconds must be between 1 and 7200, got {timeout_seconds}"
        ),
    })
}

pub(super) fn validate_tool_max_turns(
    tool_max_turns: usize,
    path: &Path,
) -> Result<(), AppConfigError> {
    if tool_max_turns > 0 {
        return Ok(());
    }

    Err(AppConfigError::InvalidRuntimeRequestPolicy {
        path: Some(path.to_path_buf()),
        reason: "runtime.tool_max_turns must be at least 1 when configured".to_string(),
    })
}

pub(super) fn normalize_request_retry_delays(
    delays: &mut Vec<u64>,
    attempts: usize,
    has_explicit_delays: bool,
    path: &Path,
) -> Result<(), AppConfigError> {
    if delays.is_empty() {
        return Err(AppConfigError::InvalidRuntimeRequestPolicy {
            path: Some(path.to_path_buf()),
            reason: "runtime.request_retry_delays must not be empty".to_string(),
        });
    }

    if let Some(delay) = delays.iter().find(|delay| !(1..=1800).contains(*delay)) {
        return Err(AppConfigError::InvalidRuntimeRequestPolicy {
            path: Some(path.to_path_buf()),
            reason: format!(
                "runtime.request_retry_delays items must be between 1 and 1800 seconds, got {delay}"
            ),
        });
    }

    if delays.len() > attempts && has_explicit_delays {
        return Err(AppConfigError::InvalidRuntimeRequestPolicy {
            path: Some(path.to_path_buf()),
            reason: format!(
                "runtime.request_retry_delays has {} items but runtime.request_retry_attempts is {attempts}",
                delays.len()
            ),
        });
    }

    delays.truncate(attempts);

    if delays.len() < attempts {
        let last_delay = *delays
            .last()
            .expect("empty retry delay list is rejected before extension");
        delays.resize(attempts, last_delay);
    }

    Ok(())
}

fn validate_status_line_items(items: &[String]) -> Result<(), AppConfigError> {
    for item in items {
        match item.as_str() {
            "git-branch" | "current-dir" | "current-model" | "throughput" | "latency" => {}
            other => {
                return Err(AppConfigError::InvalidStatusLineItem {
                    path: None,
                    value: other.to_string(),
                });
            }
        }
    }

    Ok(())
}

pub(super) fn validate_external_editor(command: &[String]) -> Result<(), AppConfigError> {
    if command.is_empty() {
        return Ok(());
    }

    if command[0].trim().is_empty() {
        return Err(AppConfigError::InvalidExternalEditorCommand { path: None });
    }

    envinfo::validate_configured_external_editor(command).map_err(|error| match error {
        envinfo::ExternalEditorError::ExternalEditorMustWait { command } => {
            AppConfigError::ExternalEditorMustWait {
                path: None,
                command,
            }
        }
        _ => AppConfigError::InvalidExternalEditorCommand { path: None },
    })
}
