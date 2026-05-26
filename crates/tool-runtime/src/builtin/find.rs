use std::{
    path::{Path, PathBuf},
    process::Stdio,
};

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

use super::{
    external_tool::{
        ExternalCommand, ManagedSearchToolConfig, ManagedToolKind, resolve_external_command,
    },
    search_fallback::{
        BoundedSortedPaths, SEARCH_MAX_OUTPUT_BYTES, TOOL_CALL_INTERRUPTED,
        VCS_DIRECTORIES_TO_EXCLUDE, build_workspace_walker, collect_capped_stderr, compile_glob,
        format_bytes, path_has_vcs_component, path_text_has_vcs_component, stderr_task_output,
        truncate_head_by_bytes, workspace_relative_cli_path, workspace_relative_path,
    },
    workspace::resolve_workspace_path,
    workspace_access::local_workspace_access,
};

const FIND_TOOL_NAME: &str = "find";
const DEFAULT_ENTRY_LIMIT: usize = 1_000;
const MAX_ENTRY_LIMIT: usize = 10_000;

/// `find_tool` 创建 workspace 路径发现工具。
pub fn find_tool(root: impl AsRef<Path>) -> impl Tool + 'static {
    find_tool_with_config(root, ManagedSearchToolConfig::default())
}

pub(crate) fn find_tool_with_config(
    root: impl AsRef<Path>,
    managed_tools: ManagedSearchToolConfig,
) -> impl Tool + 'static {
    FindTool {
        root: root.as_ref().to_path_buf(),
        managed_tools,
    }
}

#[derive(Clone)]
struct FindTool {
    root: PathBuf,
    managed_tools: ManagedSearchToolConfig,
}

impl std::fmt::Debug for FindTool {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("FindTool")
            .field("root", &self.root)
            .finish_non_exhaustive()
    }
}

impl Tool for FindTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(FIND_TOOL_NAME)
            .with_label("Find")
            .with_kind(ToolKind::Search)
            .with_description(
                "Find files or directories recursively inside the current workspace by glob pattern. Results are deterministic workspace-relative paths sorted alphabetically. Respects .gitignore and searches hidden paths.",
            )
            .with_input_schema(json!({
                "type": "object",
                "properties": {
                    "pattern": {
                        "type": "string",
                        "description": "Glob pattern to match paths, for example \"*.rs\", \"**/*.json\", or \"src/**/*.spec.ts\""
                    },
                    "path": {
                        "type": "string",
                        "description": "Workspace-relative or workspace-contained absolute directory path; defaults to the workspace root"
                    },
                    "limit": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": MAX_ENTRY_LIMIT,
                        "description": "Maximum number of paths to return"
                    }
                },
                "required": ["pattern"],
                "additionalProperties": false
            }))
            .with_permission_policy(ToolPermissionPolicy::Always)
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
        Box::pin(async move { execute_find(root, managed_tools, call, context).await })
    }
}

#[derive(Debug, Deserialize)]
struct FindArguments {
    pattern: String,
    path: Option<String>,
    limit: Option<usize>,
}

#[derive(Debug, Clone)]
struct NormalizedFindArguments {
    pattern: String,
    requested_path: String,
    limit: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FindOutcome {
    content: String,
    total_entries: usize,
    shown_entries: usize,
    truncated: bool,
    byte_truncated: bool,
    backend: &'static str,
}

async fn execute_find(
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
        resolve_external_command(ManagedToolKind::Fd, &managed_tools, &context).await
        && let Ok(outcome) =
            run_external_find(&command, &root, &search_path, &arguments, &context).await
    {
        return find_result(call.call_id, outcome);
    }

    let call_id = call.call_id.clone();
    let cancellation = context.cancellation().clone();
    match task::spawn_blocking(move || rust_find(&root, &search_path, &arguments, &cancellation))
        .await
    {
        Ok(Ok(outcome)) => find_result(call.call_id, outcome),
        Ok(Err(message)) => ToolResult::error(call.call_id, message),
        Err(error) => join_error_result(call_id, error),
    }
}

fn parse_arguments(value: serde_json::Value) -> Result<NormalizedFindArguments, String> {
    let arguments = serde_json::from_value::<FindArguments>(value)
        .map_err(|error| format!("find arguments are invalid: {error}"))?;
    let pattern = arguments.pattern.trim();
    if pattern.is_empty() {
        return Err("'pattern' is required".to_string());
    }
    Ok(NormalizedFindArguments {
        pattern: pattern.to_string(),
        requested_path: arguments.path.unwrap_or_else(|| ".".to_string()),
        limit: arguments
            .limit
            .unwrap_or(DEFAULT_ENTRY_LIMIT)
            .clamp(1, MAX_ENTRY_LIMIT),
    })
}

async fn run_external_find(
    command: &ExternalCommand,
    root: &Path,
    search_path: &Path,
    arguments: &NormalizedFindArguments,
    context: &ToolExecutionContext<'_>,
) -> Result<FindOutcome, String> {
    let mut args = vec![
        "--glob".to_string(),
        "--color=never".to_string(),
        "--strip-cwd-prefix".to_string(),
        "--hidden".to_string(),
        "--no-require-git".to_string(),
    ];
    for directory in VCS_DIRECTORIES_TO_EXCLUDE {
        args.push("--exclude".to_string());
        args.push((*directory).to_string());
    }
    let mut effective_pattern = arguments.pattern.clone();
    if arguments.pattern.contains('/') {
        args.push("--full-path".to_string());
        if !arguments.pattern.starts_with('/') && !arguments.pattern.starts_with("**/") {
            effective_pattern = format!("**/{}", arguments.pattern);
        }
    }
    args.push("--".to_string());
    args.push(effective_pattern);
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
        .ok_or_else(|| "external find stdout is unavailable".to_string())?;
    let stderr_task = child
        .stderr
        .take()
        .map(|stderr| tokio::spawn(collect_capped_stderr(stderr)));
    let mut lines = BufReader::new(stdout).lines();
    let mut collector = BoundedSortedPaths::new(arguments.limit);

    loop {
        let line = tokio::select! {
            _ = context.cancellation().cancelled() => {
                let _ = child.start_kill();
                return Err(TOOL_CALL_INTERRUPTED.to_string());
            }
            line = lines.next_line() => {
                line.map_err(|error| format!("read external find output failed: {error}"))?
            }
        };
        let Some(line) = line else {
            break;
        };
        let path = line.trim().trim_end_matches('/').to_string();
        if path.is_empty() || path_text_has_vcs_component(&path) {
            continue;
        }
        collector.push(path);
    }

    let status = child
        .wait()
        .await
        .map_err(|error| format!("wait for external find failed: {error}"))?;
    let stderr = stderr_task_output(stderr_task).await;
    if !matches!(status.code(), Some(0)) {
        return Err(format!("external find failed: {}", stderr.trim()));
    }

    let total_entries = collector.total_entries();
    Ok(format_find_paths(
        collector.into_paths(),
        total_entries,
        arguments.limit,
        command.backend.as_str(),
    ))
}

fn rust_find(
    root: &Path,
    search_path: &Path,
    arguments: &NormalizedFindArguments,
    cancellation: &CancellationToken,
) -> Result<FindOutcome, String> {
    let matcher = FindMatcher::new(arguments)?;
    let mut paths = BoundedSortedPaths::new(arguments.limit);
    for entry in build_workspace_walker(root, search_path, true) {
        if cancellation.is_cancelled() {
            return Err(TOOL_CALL_INTERRUPTED.to_string());
        }
        let entry = entry.map_err(|error| format!("walk workspace failed: {error}"))?;
        let path = entry.path();
        if path == search_path {
            continue;
        }
        if path_has_vcs_component(path) {
            continue;
        }
        let relative_path = workspace_relative_path(root, path);
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| relative_path.clone());
        let target = if arguments.pattern.contains('/') {
            relative_path.as_str()
        } else {
            name.as_str()
        };
        if matcher.matches(target) {
            paths.push(relative_path);
        }
    }
    let total_entries = paths.total_entries();
    Ok(format_find_paths(
        paths.into_paths(),
        total_entries,
        arguments.limit,
        "rust_fallback",
    ))
}

enum FindMatcher {
    Glob {
        primary: globset::GlobMatcher,
        prefixed: Option<globset::GlobMatcher>,
    },
}

impl FindMatcher {
    fn new(arguments: &NormalizedFindArguments) -> Result<Self, String> {
        let primary = compile_glob(&arguments.pattern)?;
        let prefixed = if arguments.pattern.contains('/')
            && !arguments.pattern.starts_with('/')
            && !arguments.pattern.starts_with("**/")
        {
            Some(compile_glob(&format!("**/{}", arguments.pattern))?)
        } else {
            None
        };
        Ok(Self::Glob { primary, prefixed })
    }

    fn matches(&self, target: &str) -> bool {
        match self {
            Self::Glob { primary, prefixed } => {
                primary.is_match(target)
                    || prefixed
                        .as_ref()
                        .is_some_and(|matcher| matcher.is_match(target))
            }
        }
    }
}

fn format_find_paths(
    mut paths: Vec<String>,
    total_entries: usize,
    limit: usize,
    backend: &'static str,
) -> FindOutcome {
    paths.dedup();
    let shown_entries = total_entries.min(limit);
    let result_limit_reached = total_entries > shown_entries;
    let raw_content = if shown_entries == 0 {
        "No paths found.".to_string()
    } else {
        paths
            .iter()
            .take(limit)
            .map(String::as_str)
            .collect::<Vec<_>>()
            .join("\n")
    };
    let truncation = truncate_head_by_bytes(raw_content, SEARCH_MAX_OUTPUT_BYTES);
    let mut content = truncation.content;
    let mut notices = Vec::new();
    if result_limit_reached {
        notices.push(format!(
            "showing {shown_entries} of {total_entries} paths. Increase limit for more"
        ));
    }
    if truncation.is_truncated {
        notices.push(format!(
            "{} limit reached",
            format_bytes(SEARCH_MAX_OUTPUT_BYTES)
        ));
    }
    if !notices.is_empty() {
        content.push_str(&format!("\n\n[{}]", notices.join(". ")));
    }
    FindOutcome {
        content,
        total_entries,
        shown_entries: paths.len().min(limit),
        truncated: result_limit_reached || truncation.is_truncated,
        byte_truncated: truncation.is_truncated,
        backend,
    }
}

fn find_result(call_id: String, outcome: FindOutcome) -> ToolResult {
    ToolResult::success(call_id, outcome.content).with_details(json!({
        "backend": outcome.backend,
        "total_entries": outcome.total_entries,
        "shown_entries": outcome.shown_entries,
        "truncated": outcome.truncated,
        "byte_truncated": outcome.byte_truncated,
    }))
}

fn join_error_result(call_id: String, error: JoinError) -> ToolResult {
    ToolResult::error(call_id, format!("find task failed: {error}"))
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

    use super::*;
    use crate::builtin::external_tool::ExternalToolBackend;

    #[cfg(unix)]
    #[tokio::test]
    async fn external_find_uses_dot_path_for_workspace_root() {
        let root = temp_root("external-find-root-path");
        let script = write_executable(
            &root,
            "fake-fd",
            r#"#!/bin/sh
last=""
for arg in "$@"; do
  last="$arg"
done
if [ "$last" != "." ]; then
  printf 'expected root path ".", got "%s"\n' "$last" >&2
  exit 2
fi
printf 'src/lib.rs\n'
"#,
        );
        let command = ExternalCommand {
            path: script,
            backend: ExternalToolBackend::SystemPath,
        };
        let arguments = NormalizedFindArguments {
            pattern: "*.rs".to_string(),
            requested_path: ".".to_string(),
            limit: 10,
        };

        let outcome = run_external_find(
            &command,
            &root,
            &root,
            &arguments,
            &ToolExecutionContext::new(&CancellationToken::new()),
        )
        .await
        .expect("root search should pass . to fd");

        assert_eq!(outcome.content, "src/lib.rs");
        cleanup(&root);
    }

    #[test]
    fn parse_arguments_caps_find_limit() {
        let arguments = parse_arguments(json!({
            "pattern": "*",
            "limit": usize::MAX
        }))
        .expect("arguments should parse");

        assert_eq!(arguments.limit, MAX_ENTRY_LIMIT);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn external_find_marks_limit_and_returns_deterministic_sorted_prefix() {
        let root = temp_root("external-find-limit-sort");
        let script = write_executable(
            &root,
            "fake-fd",
            r#"#!/bin/sh
limit=0
while [ "$#" -gt 0 ]; do
  if [ "$1" = "--max-results" ]; then
    limit="$2"
  fi
  shift
done
count=0
for path in z.rs a.rs m.rs c.rs; do
  count=$((count + 1))
  if [ "$limit" -gt 0 ] && [ "$count" -gt "$limit" ]; then
    exit 0
  fi
  printf '%s\n' "$path"
done
"#,
        );
        let command = ExternalCommand {
            path: script,
            backend: ExternalToolBackend::SystemPath,
        };
        let arguments = NormalizedFindArguments {
            pattern: "*.rs".to_string(),
            requested_path: ".".to_string(),
            limit: 2,
        };

        let outcome = run_external_find(
            &command,
            &root,
            &root,
            &arguments,
            &ToolExecutionContext::new(&CancellationToken::new()),
        )
        .await
        .expect("fake fd should succeed");

        assert_eq!(
            outcome.content,
            "a.rs\nc.rs\n\n[showing 2 of 4 paths. Increase limit for more]"
        );
        assert_eq!(outcome.total_entries, 4);
        assert_eq!(outcome.shown_entries, 2);
        assert!(outcome.truncated);
        cleanup(&root);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn external_find_excludes_vcs_directories_but_keeps_hidden_files() {
        let root = temp_root("external-find-vcs");
        let script = write_executable(
            &root,
            "fake-fd",
            r#"#!/bin/sh
exclude_vcs=0
while [ "$#" -gt 0 ]; do
  if [ "$1" = "--exclude" ] && [ "${2:-}" = ".git" ]; then
    exclude_vcs=1
  fi
  shift
done
printf '.hidden.rs\n'
printf 'src/lib.rs\n'
if [ "$exclude_vcs" -eq 0 ]; then
  printf '.git/config\n'
fi
"#,
        );
        let command = ExternalCommand {
            path: script,
            backend: ExternalToolBackend::SystemPath,
        };
        let arguments = NormalizedFindArguments {
            pattern: "*.rs".to_string(),
            requested_path: ".".to_string(),
            limit: 10,
        };

        let outcome = run_external_find(
            &command,
            &root,
            &root,
            &arguments,
            &ToolExecutionContext::new(&CancellationToken::new()),
        )
        .await
        .expect("fake fd should succeed");

        assert!(outcome.content.contains(".hidden.rs"));
        assert!(outcome.content.contains("src/lib.rs"));
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
            std::env::temp_dir().join(format!("lumos-{prefix}-{}-{stamp}", std::process::id()));
        fs::create_dir_all(&root).expect("create temp root");
        root
    }

    fn cleanup(path: &Path) {
        let _ = fs::remove_dir_all(path);
    }
}
