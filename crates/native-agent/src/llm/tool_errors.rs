use mo_tools::{ProcessedToolError, ToolErrorFormatter};

const TOOL_ERROR_HINT_SEPARATOR: &str = ". Hint: ";
const RIG_ERROR_PREFIXES: &[&str] = &[
    "Toolset error:",
    "ToolServerError:",
    "ToolSetError:",
    "ToolCallError:",
    "CallError:",
    "JsonError:",
];

#[derive(Debug, Clone, Copy, Default)]
pub(super) struct NativeAgentToolErrorFormatter;

impl ToolErrorFormatter for NativeAgentToolErrorFormatter {
    fn format_tool_error(&self, tool_name: &str, raw_error: &str) -> ProcessedToolError {
        let display_reason = tool_error_display_reason(tool_name, raw_error);
        let hint = tool_error_hint(tool_name, &display_reason);
        ProcessedToolError::new(
            format!("{display_reason}{TOOL_ERROR_HINT_SEPARATOR}{hint}"),
            display_reason,
        )
    }
}

pub(super) fn parse_rig_error_text(result: &str) -> Option<String> {
    if result.contains("Tool call interrupted") {
        return Some("Tool call interrupted".to_string());
    }

    let (unwrapped, had_rig_wrapper) = strip_rig_error_wrappers(result);
    let unwrapped = unwrapped.trim();
    if unwrapped.is_empty() {
        return None;
    }

    let reason = unwrapped
        .rsplit_once(TOOL_ERROR_HINT_SEPARATOR)
        .map(|(reason, _)| reason)
        .unwrap_or(unwrapped)
        .trim()
        .trim_end_matches('.');

    if had_rig_wrapper || looks_like_processed_tool_error(reason, unwrapped) {
        Some(reason.to_string())
    } else {
        None
    }
}

fn strip_rig_error_wrappers(result: &str) -> (&str, bool) {
    let mut text = result.trim();
    let mut stripped = false;

    while let Some(rest) = strip_any_prefix(text, RIG_ERROR_PREFIXES) {
        text = rest.trim_start();
        stripped = true;
    }

    (text, stripped)
}

fn strip_any_prefix<'a>(text: &'a str, prefixes: &[&str]) -> Option<&'a str> {
    prefixes
        .iter()
        .find_map(|prefix| text.strip_prefix(prefix).map(str::trim_start))
}

fn looks_like_processed_tool_error(reason: &str, full_text: &str) -> bool {
    full_text.contains(TOOL_ERROR_HINT_SEPARATOR)
        && (reason.starts_with("File not found:")
            || reason.starts_with("Directory not found:")
            || reason.starts_with("File requires explicit attachment:")
            || reason.starts_with("Path is ")
            || reason.starts_with("Path ")
            || reason.starts_with("Invalid arguments")
            || reason.starts_with("Workspace root is unavailable")
            || reason.starts_with("Tool failed:")
            || reason.starts_with("Could not ")
            || reason.starts_with("Read failed")
            || reason.starts_with("List directory failed"))
}

fn tool_error_display_reason(tool_name: &str, raw_error: &str) -> String {
    let raw_error = strip_os_error_suffix(raw_error.trim());
    if raw_error.is_empty() {
        return format!("Tool failed: {tool_name}");
    }

    if let Some(path) = path_not_found_target(raw_error) {
        return match tool_name {
            "list_dir" => format!("Directory not found: {path}"),
            _ => format!("File not found: {path}"),
        };
    }

    if let Some(path) = quoted_target(raw_error, "stat failed for ") {
        if raw_error.contains("No such file or directory") {
            return match tool_name {
                "list_dir" => format!("Directory not found: {path}"),
                _ => format!("File not found: {path}"),
            };
        }
        return format!("Could not inspect path: {path}");
    }

    if let Some(path) = quoted_target(raw_error, "read failed for ") {
        if raw_error
            .contains("must be attached explicitly in the user prompt instead of using read")
        {
            return format!("File requires explicit attachment: {path}");
        }
        if raw_error.contains("No such file or directory") {
            return format!("File not found: {path}");
        }
        return format!("Could not read file: {path}");
    }

    if let Some(path) = quoted_target(raw_error, "read directory failed for ") {
        if raw_error.contains("No such file or directory") {
            return format!("Directory not found: {path}");
        }
        return format!("Could not list directory: {path}");
    }

    if let Some(path) = raw_error.strip_prefix("path is outside workspace: ") {
        return format!("Path is outside workspace: {}", path.trim());
    }

    if raw_error == "'path' is required" {
        return "Path is required".to_string();
    }

    if let Some(path) =
        quoted_subject_with_suffix(raw_error, " is a directory, use list_dir instead")
    {
        return format!("Path is a directory: {path}");
    }

    if let Some(path) = quoted_subject_with_suffix(raw_error, " is not a regular file") {
        return format!("Path is not a regular file: {path}");
    }

    if let Some(path) = quoted_subject_with_suffix(raw_error, " is a file, use read instead") {
        return format!("Path is a file: {path}");
    }

    if raw_error.starts_with("read arguments are invalid:")
        || raw_error.starts_with("list_dir arguments are invalid:")
        || raw_error.contains("arguments do not match schema:")
    {
        return format!("Invalid arguments for {tool_name}");
    }

    if raw_error.starts_with("workspace root is unavailable:") {
        return "Workspace root is unavailable".to_string();
    }

    format!("Tool failed: {raw_error}")
}

fn tool_error_hint(tool_name: &str, display_reason: &str) -> &'static str {
    if display_reason.starts_with("File not found:") {
        return "Use list_dir to verify the file exists before reading.";
    }
    if display_reason.starts_with("File requires explicit attachment:") {
        return "Ask the user to attach the file explicitly instead of using read.";
    }
    if display_reason.starts_with("Directory not found:") {
        return "Use list_dir on the nearest existing parent directory.";
    }
    if display_reason.starts_with("Path is a directory:") {
        return "Use list_dir to inspect the directory before reading a file.";
    }
    if display_reason.starts_with("Path is a file:") {
        return "Use read to read file contents.";
    }
    if display_reason.starts_with("Path is outside workspace:") {
        return "Use a path inside the current workspace.";
    }
    if display_reason == "Path is required" {
        return "Provide a workspace-relative path.";
    }
    if display_reason.starts_with("Invalid arguments") {
        return "Check the tool schema and retry with valid arguments.";
    }

    match tool_name {
        "read" => "Use list_dir to verify the path, then retry read.",
        "list_dir" => "Verify the path is a directory inside the workspace.",
        _ => "Check the tool input and try again.",
    }
}

fn path_not_found_target(raw_error: &str) -> Option<String> {
    let rest = raw_error.strip_prefix("path not found: ")?;
    let (target, _) = rest.rsplit_once(": ")?;
    let target = target.trim();
    (!target.is_empty()).then(|| target.to_string())
}

fn quoted_target(raw_error: &str, prefix: &str) -> Option<String> {
    let rest = raw_error.strip_prefix(prefix)?.strip_prefix('\'')?;
    let end = rest.find('\'')?;
    let target = rest[..end].trim();
    (!target.is_empty()).then(|| target.to_string())
}

fn quoted_subject_with_suffix(raw_error: &str, suffix: &str) -> Option<String> {
    let rest = raw_error.strip_prefix('\'')?;
    let (target, tail) = rest.split_once('\'')?;
    (tail == suffix && !target.trim().is_empty()).then(|| target.trim().to_string())
}

fn strip_os_error_suffix(text: &str) -> &str {
    let Some(index) = text.rfind(" (os error ") else {
        return text;
    };
    if text[index..].ends_with(')') {
        text[..index].trim_end()
    } else {
        text
    }
}

#[cfg(test)]
mod tests {
    use mo_tools::ToolErrorFormatter;

    use super::{NativeAgentToolErrorFormatter, parse_rig_error_text};

    #[test]
    fn native_tool_error_formatter_cleans_common_builtin_tool_errors() {
        struct Case {
            tool_name: &'static str,
            raw_error: &'static str,
            display_reason: &'static str,
            hint: &'static str,
        }

        let cases = [
            Case {
                tool_name: "read",
                raw_error: "path not found: AGENTS.md: No such file or directory (os error 2)",
                display_reason: "File not found: AGENTS.md",
                hint: "Use list_dir to verify the file exists before reading.",
            },
            Case {
                tool_name: "list_dir",
                raw_error: "path not found: missing: No such file or directory (os error 2)",
                display_reason: "Directory not found: missing",
                hint: "Use list_dir on the nearest existing parent directory.",
            },
            Case {
                tool_name: "read",
                raw_error: "stat failed for 'secret.txt': Permission denied (os error 13)",
                display_reason: "Could not inspect path: secret.txt",
                hint: "Use list_dir to verify the path, then retry read.",
            },
            Case {
                tool_name: "read",
                raw_error: "read failed for '/workspace/unreadable.txt': Permission denied (os error 13)",
                display_reason: "Could not read file: /workspace/unreadable.txt",
                hint: "Use list_dir to verify the path, then retry read.",
            },
            Case {
                tool_name: "read",
                raw_error: "read failed for '/workspace/assets/sample.png': image/png files must be attached explicitly in the user prompt instead of using read",
                display_reason: "File requires explicit attachment: /workspace/assets/sample.png",
                hint: "Ask the user to attach the file explicitly instead of using read.",
            },
            Case {
                tool_name: "list_dir",
                raw_error: "read directory failed for '/workspace/private': Permission denied (os error 13)",
                display_reason: "Could not list directory: /workspace/private",
                hint: "Verify the path is a directory inside the workspace.",
            },
            Case {
                tool_name: "read",
                raw_error: "path is outside workspace: ../secret.txt",
                display_reason: "Path is outside workspace: ../secret.txt",
                hint: "Use a path inside the current workspace.",
            },
            Case {
                tool_name: "read",
                raw_error: "'path' is required",
                display_reason: "Path is required",
                hint: "Provide a workspace-relative path.",
            },
            Case {
                tool_name: "read",
                raw_error: "'src' is a directory, use list_dir instead",
                display_reason: "Path is a directory: src",
                hint: "Use list_dir to inspect the directory before reading a file.",
            },
            Case {
                tool_name: "read",
                raw_error: "'socket' is not a regular file",
                display_reason: "Path is not a regular file: socket",
                hint: "Use list_dir to verify the path, then retry read.",
            },
            Case {
                tool_name: "list_dir",
                raw_error: "'Cargo.toml' is a file, use read instead",
                display_reason: "Path is a file: Cargo.toml",
                hint: "Use read to read file contents.",
            },
            Case {
                tool_name: "read",
                raw_error: "read arguments are invalid: invalid type",
                display_reason: "Invalid arguments for read",
                hint: "Check the tool schema and retry with valid arguments.",
            },
            Case {
                tool_name: "list_dir",
                raw_error: "workspace root is unavailable: No such file or directory (os error 2)",
                display_reason: "Workspace root is unavailable",
                hint: "Verify the path is a directory inside the workspace.",
            },
        ];

        for case in cases {
            let formatted =
                NativeAgentToolErrorFormatter.format_tool_error(case.tool_name, case.raw_error);

            assert_eq!(formatted.display_reason, case.display_reason);
            assert_eq!(
                formatted.assistant_message,
                format!("{}. Hint: {}", case.display_reason, case.hint)
            );
        }
    }

    #[test]
    fn parse_rig_error_text_strips_legacy_prefixes_and_hints() {
        assert_eq!(
            parse_rig_error_text(
                "Toolset error: ToolCallError: ToolCallError: File not found: AGENTS.md. Hint: Use list_dir to verify the file exists before reading."
            ),
            Some("File not found: AGENTS.md".to_string())
        );
        assert_eq!(
            parse_rig_error_text(
                "File not found: AGENTS.md. Hint: Use list_dir to verify the file exists before reading."
            ),
            Some("File not found: AGENTS.md".to_string())
        );
        assert_eq!(parse_rig_error_text("normal tool output"), None);
    }

    #[test]
    fn parse_rig_error_text_removes_the_trailing_formatter_hint_only() {
        assert_eq!(
            parse_rig_error_text(
                "Tool failed: path contains . Hint: literal text. Hint: Check the tool input and try again."
            ),
            Some("Tool failed: path contains . Hint: literal text".to_string())
        );
    }
}
