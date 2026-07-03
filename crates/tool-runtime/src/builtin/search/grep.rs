use std::{
    fs,
    path::{Path, PathBuf},
    process::Stdio,
};

use regex::RegexBuilder;
use serde::Deserialize;
use serde_json::json;
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::Command,
    task,
    task::JoinError,
};
use tokio_util::sync::CancellationToken;

use crate::{
    Tool, ToolCall, ToolDefinition, ToolExecutionContext, ToolExecutionFuture, ToolKind,
    ToolPermissionPolicy, ToolResult,
};

use super::super::workspace_file::{
    workspace::resolve_workspace_path, workspace_access::local_workspace_access,
};
use super::{
    external_tool::{
        ExternalCommand, ManagedSearchToolConfig, ManagedToolKind, resolve_external_command,
    },
    search_fallback::{
        GREP_MAX_LINE_CHARS, SEARCH_MAX_OUTPUT_BYTES, TOOL_CALL_INTERRUPTED,
        VCS_DIRECTORIES_TO_EXCLUDE, build_workspace_walker, collect_capped_stderr, compile_glob,
        format_bytes, path_has_vcs_component, path_matches_glob, path_text_has_vcs_component,
        stderr_task_output, truncate_head_by_bytes, truncate_line, workspace_relative_cli_path,
        workspace_relative_path,
    },
};

const GREP_TOOL_NAME: &str = "grep";
const DEFAULT_MATCH_LIMIT: usize = 100;
const MAX_MATCH_LIMIT: usize = 1_000;
const MAX_CONTEXT_LINES: usize = 20;

/// `grep_tool` 创建 workspace 内容搜索工具。
pub fn grep_tool(root: impl AsRef<Path>) -> impl Tool + 'static {
    grep_tool_with_config(root, ManagedSearchToolConfig::default())
}

pub(crate) fn grep_tool_with_config(
    root: impl AsRef<Path>,
    managed_tools: ManagedSearchToolConfig,
) -> impl Tool + 'static {
    GrepTool {
        root: root.as_ref().to_path_buf(),
        managed_tools,
    }
}

#[derive(Clone)]
struct GrepTool {
    root: PathBuf,
    managed_tools: ManagedSearchToolConfig,
}

impl std::fmt::Debug for GrepTool {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("GrepTool")
            .field("root", &self.root)
            .finish_non_exhaustive()
    }
}

impl Tool for GrepTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(GREP_TOOL_NAME)
            .with_label("Grep")
            .with_kind(ToolKind::Search)
            .with_description(
                "Search file contents recursively inside the current workspace. Returns file paths, 1-based line numbers, matching text, and truncation notes when match, line, or byte limits are reached. Respects .gitignore and searches hidden files.",
            )
            .with_input_schema(json!({
                "type": "object",
                "properties": {
                    "pattern": {
                        "type": "string",
                        "description": "Regex or literal pattern to search for"
                    },
                    "path": {
                        "type": "string",
                        "description": "Workspace-relative or workspace-contained absolute file or directory path; defaults to the workspace root"
                    },
                    "glob": {
                        "type": "string",
                        "description": "Optional glob filter for workspace-relative paths, for example \"*.rs\" or \"crates/**/Cargo.toml\""
                    },
                    "ignore_case": {
                        "type": "boolean",
                        "description": "Match case-insensitively"
                    },
                    "literal": {
                        "type": "boolean",
                        "description": "Treat pattern as plain text instead of regex"
                    },
                    "context": {
                        "type": "integer",
                        "minimum": 0,
                        "maximum": MAX_CONTEXT_LINES,
                        "description": "Number of context lines before and after each match"
                    },
                    "limit": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": MAX_MATCH_LIMIT,
                        "description": "Maximum number of matching lines to return"
                    }
                },
                "required": ["pattern"],
                "additionalProperties": false
            }))
            .with_permission_policy(ToolPermissionPolicy::Always)
            .with_prompt_guidelines(
                "Uses ripgrep internally but handles permissions and .gitignore — prefer over running rg in bash.",
            )
    }

    fn execute<'a>(
        &'a self,
        call: ToolCall,
        cancellation: &'a CancellationToken,
    ) -> ToolExecutionFuture<'a> {
        self.execute_with_context(call, ToolExecutionContext::new(cancellation))
    }

    fn execute_with_context<'a>(
        &'a self,
        call: ToolCall,
        context: ToolExecutionContext<'a>,
    ) -> ToolExecutionFuture<'a> {
        let root = self.root.clone();
        let managed_tools = self.managed_tools.clone();
        Box::pin(async move { execute_grep(root, managed_tools, call, context).await })
    }
}

#[derive(Debug, Deserialize)]
struct GrepArguments {
    pattern: String,
    path: Option<String>,
    glob: Option<String>,
    ignore_case: Option<bool>,
    literal: Option<bool>,
    context: Option<usize>,
    limit: Option<usize>,
}

#[derive(Debug, Clone)]
struct NormalizedGrepArguments {
    pattern: String,
    requested_path: String,
    glob: Option<String>,
    ignore_case: bool,
    literal: bool,
    context: usize,
    limit: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct GrepOutcome {
    content: String,
    total_matches: usize,
    shown_matches: usize,
    truncated: bool,
    match_truncated: bool,
    byte_truncated: bool,
    lines_truncated: bool,
    backend: &'static str,
}

async fn execute_grep(
    root: PathBuf,
    managed_tools: ManagedSearchToolConfig,
    call: ToolCall,
    context: ToolExecutionContext<'_>,
) -> ToolResult {
    if context.cancellation().is_cancelled() {
        return ToolResult::error(call.call_id, TOOL_CALL_INTERRUPTED);
    }
    let arguments = match parse_arguments(call.arguments) {
        Ok(arguments) => arguments,
        Err(message) => return ToolResult::error(call.call_id, message),
    };
    let access = local_workspace_access();
    let root = match access.as_ref().canonicalize(&root) {
        Ok(root) => root,
        Err(error) => {
            return ToolResult::error(
                call.call_id,
                format!("workspace root is unavailable: {error}"),
            );
        }
    };
    let search_path =
        match resolve_workspace_path(access.as_ref(), &root, &arguments.requested_path) {
            Ok(path) => path,
            Err(message) => return ToolResult::error(call.call_id, message),
        };

    if let Some(command) =
        resolve_external_command(ManagedToolKind::Ripgrep, &managed_tools, &context).await
        && let Ok(outcome) =
            run_external_grep(&command, &root, &search_path, &arguments, &context).await
    {
        return grep_result(call.call_id, outcome);
    }

    let call_id = call.call_id.clone();
    let cancellation = context.cancellation().clone();
    match task::spawn_blocking(move || rust_grep(&root, &search_path, &arguments, &cancellation))
        .await
    {
        Ok(Ok(outcome)) => grep_result(call.call_id, outcome),
        Ok(Err(message)) => ToolResult::error(call.call_id, message),
        Err(error) => join_error_result(call_id, error),
    }
}

fn parse_arguments(value: serde_json::Value) -> Result<NormalizedGrepArguments, String> {
    let arguments = serde_json::from_value::<GrepArguments>(value)
        .map_err(|error| format!("grep arguments are invalid: {error}"))?;
    let pattern = arguments.pattern.trim();
    if pattern.is_empty() {
        return Err("'pattern' is required".to_string());
    }
    Ok(NormalizedGrepArguments {
        pattern: pattern.to_string(),
        requested_path: arguments.path.unwrap_or_else(|| ".".to_string()),
        glob: arguments
            .glob
            .map(|glob| glob.trim().to_string())
            .filter(|glob| !glob.is_empty()),
        ignore_case: arguments.ignore_case.unwrap_or(false),
        literal: arguments.literal.unwrap_or(false),
        context: arguments.context.unwrap_or(0).min(MAX_CONTEXT_LINES),
        limit: arguments
            .limit
            .unwrap_or(DEFAULT_MATCH_LIMIT)
            .clamp(1, MAX_MATCH_LIMIT),
    })
}

async fn run_external_grep(
    command: &ExternalCommand,
    root: &Path,
    search_path: &Path,
    arguments: &NormalizedGrepArguments,
    context: &ToolExecutionContext<'_>,
) -> Result<GrepOutcome, String> {
    let mut args = vec![
        "--json".to_string(),
        "--line-number".to_string(),
        "--color=never".to_string(),
        "--sort".to_string(),
        "path".to_string(),
        "--hidden".to_string(),
    ];
    for directory in VCS_DIRECTORIES_TO_EXCLUDE {
        args.push("--glob".to_string());
        args.push(format!("!{directory}"));
    }
    if arguments.ignore_case {
        args.push("--ignore-case".to_string());
    }
    if arguments.literal {
        args.push("--fixed-strings".to_string());
    }
    if arguments.context > 0 {
        args.push("--context".to_string());
        args.push(arguments.context.to_string());
    }
    if let Some(glob) = arguments.glob.as_ref() {
        args.push("--glob".to_string());
        args.push(glob.clone());
    }
    args.push("--".to_string());
    args.push(arguments.pattern.clone());
    args.push(workspace_relative_cli_path(root, search_path));

    let mut process = Command::new(&command.path);
    process.args(&args);
    process.current_dir(root);
    process.stdin(Stdio::null());
    process.stdout(Stdio::piped());
    process.stderr(Stdio::piped());
    process.kill_on_drop(true);

    let mut child = process
        .spawn()
        .map_err(|error| format!("spawn external search tool failed: {error}"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "external grep stdout is unavailable".to_string())?;
    let stderr_task = child
        .stderr
        .take()
        .map(|stderr| tokio::spawn(collect_capped_stderr(stderr)));
    let mut lines = BufReader::new(stdout).lines();
    let mut matches = Vec::new();
    let mut observed_matches = 0usize;
    let mut match_truncated = false;
    let mut killed_due_to_limit = false;

    loop {
        let line = tokio::select! {
            _ = context.cancellation().cancelled() => {
                let _ = child.start_kill();
                return Err(TOOL_CALL_INTERRUPTED.to_string());
            }
            line = lines.next_line() => {
                line.map_err(|error| format!("read external grep output failed: {error}"))?
            }
        };
        let Some(line) = line else {
            break;
        };
        let Some(match_event) = parse_rg_match_event(&line) else {
            continue;
        };
        if path_text_has_vcs_component(&match_event.path) {
            continue;
        }
        if observed_matches >= arguments.limit {
            match_truncated = true;
            killed_due_to_limit = true;
            observed_matches += 1;
            let _ = child.start_kill();
            break;
        }
        observed_matches += 1;
        matches.push(match_event);
    }

    let status = child
        .wait()
        .await
        .map_err(|error| format!("wait for external grep failed: {error}"))?;
    let stderr = stderr_task_output(stderr_task).await;
    if !killed_due_to_limit && !matches!(status.code(), Some(0) | Some(1)) {
        return Err(format!("external grep failed: {}", stderr.trim()));
    }

    let (lines, lines_truncated) = format_external_grep_matches(root, arguments, &matches).await;
    let formatted = format_grep_content(lines, matches.len(), match_truncated, lines_truncated);
    Ok(GrepOutcome {
        content: formatted.content,
        total_matches: observed_matches,
        shown_matches: matches.len(),
        truncated: match_truncated || formatted.byte_truncated || formatted.lines_truncated,
        match_truncated,
        byte_truncated: formatted.byte_truncated,
        lines_truncated: formatted.lines_truncated,
        backend: command.backend.as_str(),
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ExternalGrepMatch {
    path: String,
    line_number: usize,
    line_text: String,
}

#[derive(Debug, Deserialize)]
struct RipgrepJsonEvent {
    #[serde(rename = "type")]
    event_type: String,
    data: Option<RipgrepMatchData>,
}

#[derive(Debug, Deserialize)]
struct RipgrepMatchData {
    path: RipgrepText,
    line_number: usize,
    lines: RipgrepText,
}

#[derive(Debug, Deserialize)]
struct RipgrepText {
    text: String,
}

fn parse_rg_match_event(line: &str) -> Option<ExternalGrepMatch> {
    let event = serde_json::from_str::<RipgrepJsonEvent>(line).ok()?;
    if event.event_type != "match" {
        return None;
    }
    let data = event.data?;
    Some(ExternalGrepMatch {
        path: data.path.text,
        line_number: data.line_number,
        line_text: sanitize_rg_line_text(&data.lines.text),
    })
}

fn sanitize_rg_line_text(text: &str) -> String {
    text.replace("\r\n", "\n")
        .replace('\r', "")
        .trim_end_matches('\n')
        .to_string()
}

async fn format_external_grep_matches(
    root: &Path,
    arguments: &NormalizedGrepArguments,
    matches: &[ExternalGrepMatch],
) -> (Vec<String>, bool) {
    let mut output = Vec::with_capacity(matches.len());
    let mut lines_truncated = false;
    for match_event in matches {
        if arguments.context == 0 {
            let (line, was_truncated) = truncate_line(&match_event.line_text, GREP_MAX_LINE_CHARS);
            lines_truncated |= was_truncated;
            output.push(format!(
                "{}:{}:{}",
                match_event.path, match_event.line_number, line
            ));
            continue;
        }

        let path = external_match_path(root, &match_event.path);
        let Ok(content) = tokio::fs::read_to_string(&path).await else {
            let (line, was_truncated) = truncate_line(&match_event.line_text, GREP_MAX_LINE_CHARS);
            lines_truncated |= was_truncated;
            output.push(format!(
                "{}:{}:{}",
                match_event.path, match_event.line_number, line
            ));
            continue;
        };
        let lines = content.lines().collect::<Vec<_>>();
        let match_index = match_event.line_number.saturating_sub(1);
        if match_index >= lines.len() {
            let (line, was_truncated) = truncate_line(&match_event.line_text, GREP_MAX_LINE_CHARS);
            lines_truncated |= was_truncated;
            output.push(format!(
                "{}:{}:{}",
                match_event.path, match_event.line_number, line
            ));
            continue;
        }
        let (block, was_truncated) =
            format_match_block(root, &path, &lines, match_index, arguments.context);
        lines_truncated |= was_truncated;
        output.push(block);
    }
    (output, lines_truncated)
}

fn external_match_path(root: &Path, path: &str) -> PathBuf {
    let path = Path::new(path);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    }
}

fn rust_grep(
    root: &Path,
    search_path: &Path,
    arguments: &NormalizedGrepArguments,
    cancellation: &CancellationToken,
) -> Result<GrepOutcome, String> {
    if cancellation.is_cancelled() {
        return Err(TOOL_CALL_INTERRUPTED.to_string());
    }
    let pattern = if arguments.literal {
        regex::escape(&arguments.pattern)
    } else {
        arguments.pattern.clone()
    };
    let regex = RegexBuilder::new(&pattern)
        .case_insensitive(arguments.ignore_case)
        .build()
        .map_err(|error| format!("invalid grep pattern: {error}"))?;
    let glob = arguments.glob.as_deref().map(compile_glob).transpose()?;
    let mut matches = Vec::new();
    let mut observed_matches = 0usize;
    let mut match_truncated = false;
    let mut lines_truncated = false;

    'walk: for entry in build_workspace_walker(root, search_path, true) {
        if cancellation.is_cancelled() {
            return Err(TOOL_CALL_INTERRUPTED.to_string());
        }
        let entry = entry.map_err(|error| format!("walk workspace failed: {error}"))?;
        let path = entry.path();
        if path_has_vcs_component(path) {
            continue;
        }
        let file_type = entry
            .file_type()
            .ok_or_else(|| format!("read file type failed for '{}'", path.display()))?;
        if !file_type.is_file() {
            continue;
        }
        if let Some(glob) = glob.as_ref()
            && !path_matches_glob(root, path, glob)
        {
            continue;
        }
        let Ok(content) = fs::read_to_string(path) else {
            continue;
        };
        let lines = content.lines().collect::<Vec<_>>();
        for (index, line) in lines.iter().enumerate() {
            if regex.is_match(line) {
                if observed_matches >= arguments.limit {
                    observed_matches += 1;
                    match_truncated = true;
                    break 'walk;
                }
                observed_matches += 1;
                let (match_block, was_truncated) =
                    format_match_block(root, path, &lines, index, arguments.context);
                lines_truncated |= was_truncated;
                matches.push(match_block);
            }
        }
    }

    let shown_matches = matches.len();
    let formatted = format_grep_content(matches, shown_matches, match_truncated, lines_truncated);
    Ok(GrepOutcome {
        content: formatted.content,
        total_matches: observed_matches,
        shown_matches,
        truncated: match_truncated || formatted.byte_truncated || formatted.lines_truncated,
        match_truncated,
        byte_truncated: formatted.byte_truncated,
        lines_truncated: formatted.lines_truncated,
        backend: "rust_fallback",
    })
}

fn format_match_block(
    root: &Path,
    path: &Path,
    lines: &[&str],
    match_index: usize,
    context: usize,
) -> (String, bool) {
    let relative = workspace_relative_path(root, path);
    if context == 0 {
        let (line, was_truncated) = truncate_line(lines[match_index], GREP_MAX_LINE_CHARS);
        return (
            format!("{}:{}:{}", relative, match_index + 1, line),
            was_truncated,
        );
    }
    let start = match_index.saturating_sub(context);
    let end = (match_index + context + 1).min(lines.len());
    let mut was_any_line_truncated = false;
    let content = (start..end)
        .map(|index| {
            let separator = if index == match_index { ":" } else { "-" };
            let (line, was_truncated) = truncate_line(lines[index], GREP_MAX_LINE_CHARS);
            was_any_line_truncated |= was_truncated;
            format!("{}{separator}{}:{}", relative, index + 1, line)
        })
        .collect::<Vec<_>>()
        .join("\n");
    (content, was_any_line_truncated)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FormattedGrepContent {
    content: String,
    byte_truncated: bool,
    lines_truncated: bool,
}

fn format_grep_content(
    lines: Vec<String>,
    shown_matches: usize,
    match_truncated: bool,
    lines_truncated: bool,
) -> FormattedGrepContent {
    if shown_matches == 0 {
        return FormattedGrepContent {
            content: "No matches found.".to_string(),
            byte_truncated: false,
            lines_truncated,
        };
    }
    let raw_content = lines.join("\n");
    let truncation = truncate_head_by_bytes(raw_content, SEARCH_MAX_OUTPUT_BYTES);
    let mut content = truncation.content;
    let mut notices = Vec::new();
    if match_truncated {
        notices.push(format!(
            "{shown_matches} matches limit reached. Use limit={} for more, or refine pattern",
            shown_matches.saturating_mul(2).max(shown_matches + 1)
        ));
    }
    if truncation.is_truncated {
        notices.push(format!(
            "{} limit reached",
            format_bytes(SEARCH_MAX_OUTPUT_BYTES)
        ));
    }
    if lines_truncated {
        notices.push(format!(
            "some lines truncated to {GREP_MAX_LINE_CHARS} chars. Use read tool to see full lines"
        ));
    }
    if !notices.is_empty() {
        content.push_str(&format!("\n\n[{}]", notices.join(". ")));
    }
    FormattedGrepContent {
        content,
        byte_truncated: truncation.is_truncated,
        lines_truncated,
    }
}

fn grep_result(call_id: String, outcome: GrepOutcome) -> ToolResult {
    ToolResult::success(call_id, outcome.content).with_details(json!({
        "backend": outcome.backend,
        "total_matches": outcome.total_matches,
        "shown_matches": outcome.shown_matches,
        "truncated": outcome.truncated,
        "match_truncated": outcome.match_truncated,
        "byte_truncated": outcome.byte_truncated,
        "lines_truncated": outcome.lines_truncated,
    }))
}

fn join_error_result(call_id: String, error: JoinError) -> ToolResult {
    ToolResult::error(call_id, format!("grep task failed: {error}"))
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::{Path, PathBuf},
        time::{SystemTime, UNIX_EPOCH},
    };

    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    use tokio_util::sync::CancellationToken;

    use super::super::external_tool::ExternalToolBackend;
    use super::*;

    #[cfg(unix)]
    #[tokio::test]
    async fn external_grep_uses_dot_path_for_workspace_root() {
        let root = temp_root("external-grep-root-path");
        let script = write_executable(
            &root,
            "fake-rg",
            r#"#!/bin/sh
last=""
for arg in "$@"; do
  last="$arg"
done
if [ "$last" != "." ]; then
  printf 'expected root path ".", got "%s"\n' "$last" >&2
  exit 2
fi
printf '{"type":"match","data":{"path":{"text":"src/lib.rs"},"line_number":1,"lines":{"text":"needle\\n"}}}\n'
"#,
        );
        let command = ExternalCommand {
            path: script,
            backend: ExternalToolBackend::SystemPath,
        };
        let arguments = NormalizedGrepArguments {
            pattern: "needle".to_string(),
            requested_path: ".".to_string(),
            glob: None,
            ignore_case: false,
            literal: false,
            context: 0,
            limit: 10,
        };

        let outcome = run_external_grep(
            &command,
            &root,
            &root,
            &arguments,
            &ToolExecutionContext::new(&CancellationToken::new()),
        )
        .await
        .expect("root search should pass . to rg");

        assert_eq!(outcome.content, "src/lib.rs:1:needle");
        cleanup(&root);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn external_grep_stops_after_limit_when_streaming_json_matches() {
        let root = temp_root("external-grep-limit");
        let script = write_executable(
            &root,
            "fake-rg",
            r#"#!/bin/sh
i=1
while [ "$i" -le 500 ]; do
  printf '{"type":"match","data":{"path":{"text":"src/lib.rs"},"line_number":%s,"lines":{"text":"needle-%s\\n"}}}\n' "$i" "$i"
  i=$((i + 1))
done
"#,
        );
        let command = ExternalCommand {
            path: script,
            backend: ExternalToolBackend::SystemPath,
        };
        let arguments = NormalizedGrepArguments {
            pattern: "needle".to_string(),
            requested_path: ".".to_string(),
            glob: None,
            ignore_case: false,
            literal: false,
            context: 0,
            limit: 3,
        };

        let outcome = run_external_grep(
            &command,
            &root,
            &root,
            &arguments,
            &ToolExecutionContext::new(&CancellationToken::new()),
        )
        .await
        .expect("fake rg should succeed");

        assert_eq!(outcome.shown_matches, 3);
        assert!(outcome.match_truncated);
        assert!(outcome.total_matches <= 4);
        assert!(outcome.content.contains("src/lib.rs:1:needle-1"));
        assert!(!outcome.content.contains("needle-4"));
        cleanup(&root);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn external_grep_excludes_vcs_directories_but_keeps_hidden_files() {
        let root = temp_root("external-grep-vcs");
        let script = write_executable(
            &root,
            "fake-rg",
            r#"#!/bin/sh
exclude_vcs=0
while [ "$#" -gt 0 ]; do
  if [ "$1" = "--glob" ] && [ "${2:-}" = "!.git" ]; then
    exclude_vcs=1
  fi
  shift
done
printf '{"type":"match","data":{"path":{"text":".hidden.rs"},"line_number":1,"lines":{"text":"needle hidden\\n"}}}\n'
if [ "$exclude_vcs" -eq 0 ]; then
  printf '{"type":"match","data":{"path":{"text":".git/config"},"line_number":1,"lines":{"text":"needle vcs\\n"}}}\n'
fi
"#,
        );
        let command = ExternalCommand {
            path: script,
            backend: ExternalToolBackend::SystemPath,
        };
        let arguments = NormalizedGrepArguments {
            pattern: "needle".to_string(),
            requested_path: ".".to_string(),
            glob: None,
            ignore_case: false,
            literal: false,
            context: 0,
            limit: 10,
        };

        let outcome = run_external_grep(
            &command,
            &root,
            &root,
            &arguments,
            &ToolExecutionContext::new(&CancellationToken::new()),
        )
        .await
        .expect("fake rg should succeed");

        assert!(outcome.content.contains(".hidden.rs:1:needle hidden"));
        assert!(!outcome.content.contains(".git/config"));
        cleanup(&root);
    }

    #[cfg(unix)]
    fn write_executable(root: &Path, name: &str, content: &str) -> PathBuf {
        let path = root.join(name);
        {
            use std::io::Write;

            let mut file = fs::File::create(&path).expect("create fake executable");
            file.write_all(content.as_bytes())
                .expect("write fake executable");
            file.sync_all().expect("sync fake executable");
        }
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755))
            .expect("chmod fake executable");
        path
    }

    fn temp_root(prefix: &str) -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after unix epoch")
            .as_nanos();
        let root =
            std::env::temp_dir().join(format!("hunea-{prefix}-{}-{stamp}", std::process::id()));
        fs::create_dir_all(&root).expect("create temp root");
        root
    }

    fn cleanup(path: &Path) {
        let _ = fs::remove_dir_all(path);
    }
}
