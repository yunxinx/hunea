use crate::appconfig::{
    self, AppConfigError, FILE_PICKER_POPUP_MAX_HEIGHT, FILE_PICKER_POPUP_MIN_HEIGHT,
};
use unicode_width::UnicodeWidthStr;

/// `format_config_error` 将启动阶段配置错误渲染成面向用户的诊断表。
pub fn format_config_error(error: &appconfig::AppConfigError) -> String {
    let rows: Vec<_> = config_error_rows(error)
        .into_iter()
        .map(|(label, value)| (label, table_cell(&value)))
        .collect();
    let label_width = rows
        .iter()
        .map(|(label, _)| display_width(label))
        .max()
        .unwrap_or_else(|| display_width("Field"))
        .max(display_width("Field"));
    let value_width = rows
        .iter()
        .map(|(_, value)| display_width(value))
        .max()
        .unwrap_or_else(|| display_width("Details"))
        .max(display_width("Details"));
    let border = format!(
        "+-{}-+-{}-+",
        "-".repeat(label_width),
        "-".repeat(value_width)
    );

    let mut report = String::new();
    report.push_str("Configuration error\n\n");
    report.push_str(&border);
    report.push('\n');
    report.push_str(&table_row("Field", "Details", label_width, value_width));
    report.push_str(&border);
    report.push('\n');
    for (label, value) in rows {
        report.push_str(&table_row(label, &value, label_width, value_width));
    }
    report.push_str(&border);
    report.push_str("\n\nFix the configuration file and restart Lumos.");
    report
}

fn table_row(label: &str, value: &str, label_width: usize, value_width: usize) -> String {
    format!(
        "| {} | {} |\n",
        padded_cell(label, label_width),
        padded_cell(value, value_width)
    )
}

fn padded_cell(value: &str, width: usize) -> String {
    let padding = width.saturating_sub(display_width(value));
    format!("{value}{}", " ".repeat(padding))
}

fn display_width(value: &str) -> usize {
    UnicodeWidthStr::width(value)
}

fn table_cell(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .replace('|', "/")
}

fn config_error_rows(error: &appconfig::AppConfigError) -> Vec<(&'static str, String)> {
    match error {
        AppConfigError::Read { path, source } => vec![
            ("File", path.display().to_string()),
            (
                "Reason",
                "Could not read the configuration file".to_string(),
            ),
            ("Details", source.to_string()),
        ],
        AppConfigError::Decode { path, source } => decode_rows(path, source),
        AppConfigError::Edit { path, source } => vec![
            ("File", path.display().to_string()),
            (
                "Reason",
                "Could not edit the configuration file".to_string(),
            ),
            ("Details", source.to_string()),
        ],
        AppConfigError::Write { path, source } => vec![
            ("File", path.display().to_string()),
            (
                "Reason",
                "Could not write the configuration file".to_string(),
            ),
            ("Details", source.to_string()),
        ],
        AppConfigError::InvalidStyleMode { path, value } => validation_rows(
            path,
            "tui.user_input_style",
            value,
            "Unknown user input style",
            "cx, cc, ms",
        ),
        AppConfigError::InvalidStatusLineItem { path, value } => validation_rows(
            path,
            "tui.status_line",
            value,
            "Unknown status line item",
            "git-branch, current-dir, current-model",
        ),
        AppConfigError::InvalidExternalEditorCommand { path } => rows_with_optional_file(
            path,
            vec![
                ("Setting", "tui.external_editor".to_string()),
                ("Reason", "Invalid external editor command".to_string()),
                (
                    "Expected",
                    "A non-empty command array, for example [\"code\", \"--wait\"]".to_string(),
                ),
            ],
        ),
        AppConfigError::ExternalEditorMustWait { path, command } => rows_with_optional_file(
            path,
            vec![
                ("Setting", "tui.external_editor".to_string()),
                ("Value", command.clone()),
                (
                    "Reason",
                    "The external editor command must wait until the editor closes".to_string(),
                ),
                (
                    "Expected",
                    "Use a blocking editor flag such as --wait".to_string(),
                ),
            ],
        ),
        AppConfigError::InvalidEscInterruptPresses { path, value } => rows_with_optional_file(
            path,
            vec![
                ("Setting", "tui.esc_interrupt_presses".to_string()),
                ("Value", value.to_string()),
                ("Reason", "Invalid interrupt press count".to_string()),
                ("Expected", "1, 2, or 3".to_string()),
            ],
        ),
        AppConfigError::InvalidFilePickerPopupHeight { path, value } => rows_with_optional_file(
            path,
            vec![
                ("Setting", "tui.file_picker_popup_height".to_string()),
                ("Value", value.to_string()),
                ("Reason", "Invalid file picker popup height".to_string()),
                (
                    "Expected",
                    format!("{FILE_PICKER_POPUP_MIN_HEIGHT}..{FILE_PICKER_POPUP_MAX_HEIGHT}"),
                ),
            ],
        ),
        AppConfigError::InvalidReasoningContentDisplay { path, value } => validation_rows(
            path,
            "tui.reasoning_content_display",
            value,
            "Unknown reasoning content display mode",
            "collapsed, expanded, snippet",
        ),
        AppConfigError::InvalidRuntimeRequestPolicy { path, reason } => rows_with_optional_file(
            path,
            vec![
                ("Setting", "runtime.request".to_string()),
                ("Reason", reason.clone()),
                (
                    "Expected",
                    "request_retry_attempts: 1..10; request_retry_delays: 1..1800 seconds; request_timeout_seconds: 1..7200; tool_max_turns: unset or >= 1".to_string(),
                ),
            ],
        ),
    }
}

fn decode_rows(path: &std::path::Path, source: &toml::de::Error) -> Vec<(&'static str, String)> {
    let mut rows = vec![
        ("File", path.display().to_string()),
        ("Reason", "Invalid TOML configuration".to_string()),
    ];
    if let Some(location) = decode_error_location(source) {
        rows.push(("Location", location));
    }
    rows.push(("Details", source.message().to_string()));
    rows
}

fn decode_error_location(source: &toml::de::Error) -> Option<String> {
    source
        .to_string()
        .lines()
        .next()
        .filter(|line| line.starts_with("TOML parse error at "))
        .map(|line| line.trim_start_matches("TOML parse error at ").to_string())
}

fn validation_rows(
    path: &Option<std::path::PathBuf>,
    setting: &str,
    value: &str,
    reason: &str,
    expected: &str,
) -> Vec<(&'static str, String)> {
    rows_with_optional_file(
        path,
        vec![
            ("Setting", setting.to_string()),
            ("Value", format!("{value:?}")),
            ("Reason", reason.to_string()),
            ("Expected", expected.to_string()),
        ],
    )
}

fn rows_with_optional_file(
    path: &Option<std::path::PathBuf>,
    mut rows: Vec<(&'static str, String)>,
) -> Vec<(&'static str, String)> {
    if let Some(path) = path {
        rows.insert(0, ("File", path.display().to_string()));
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_error_report_formats_status_line_validation_as_table() {
        let error = AppConfigError::InvalidStatusLineItem {
            path: Some("/tmp/lumos/.lumos/config.toml".into()),
            value: "current-mode".to_string(),
        };

        let report = format_config_error(&error);

        assert!(report.contains("Configuration error"));
        assert!(report.contains("| Field"));
        assert!(report.contains("| File"));
        assert!(report.contains("/tmp/lumos/.lumos/config.toml"));
        assert!(report.contains("| Setting"));
        assert!(report.contains("tui.status_line"));
        assert!(report.contains("| Value"));
        assert!(report.contains("\"current-mode\""));
        assert!(report.contains("| Expected"));
        assert!(report.contains("git-branch, current-dir, current-model"));
        assert!(!report.contains("Backtrace"));
        assert!(!report.contains("Location:"));
    }

    #[test]
    fn config_error_report_flattens_multiline_details_for_table_cells() {
        assert_eq!(
            table_cell("line one\nline two\tline three | line four"),
            "line one line two line three / line four"
        );
    }

    #[test]
    fn config_error_report_formats_toml_decode_error_without_snippet_pipes() {
        let source = toml::from_str::<toml::Value>("[tui")
            .expect_err("invalid TOML should produce a decode error");
        let error = AppConfigError::Decode {
            path: "/tmp/lumos/.lumos/config.toml".into(),
            source,
        };

        let report = format_config_error(&error);

        assert!(report.contains("| Location"));
        assert!(report.contains("line 1, column"));
        assert!(report.contains("| Details"));
        assert!(report.contains("unclosed table"));
        assert!(!report.contains("1 | [tui"));
        assert!(!report.contains("| ^"));
    }

    #[test]
    fn table_row_pads_by_terminal_display_width() {
        let row = table_row("字段", "值", 6, 4);

        assert_eq!(row, "| 字段   | 值   |\n");
    }
}
