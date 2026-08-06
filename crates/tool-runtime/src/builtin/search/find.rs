use std::{
    fs,
    path::{Path, PathBuf},
};

use globset::{GlobBuilder, GlobMatcher};
use serde::Deserialize;
use serde_json::json;
use tokio::{task, task::JoinError};
use tokio_util::sync::CancellationToken;

use crate::{
    Tool, ToolCall, ToolDefinition, ToolExecutionFuture, ToolKind, ToolPermissionPolicy, ToolResult,
};

use super::super::workspace_file::{
    workspace::resolve_read_path, workspace_access::local_workspace_access,
};
use super::{
    error::SearchToolError,
    search_fallback::{
        BoundedSortedPaths, SEARCH_MAX_OUTPUT_BYTES, TOOL_CALL_INTERRUPTED, build_search_walker,
        format_bytes, model_search_path, path_has_vcs_component, search_relative_path,
    },
};

const FIND_TOOL_NAME: &str = "find";
const DEFAULT_ENTRY_LIMIT: usize = 1_000;
const MAX_ENTRY_LIMIT: usize = 10_000;

/// `find_tool` 创建确定性递归路径发现工具。
pub fn find_tool(root: impl AsRef<Path>) -> impl Tool + 'static {
    FindTool {
        root: root.as_ref().to_path_buf(),
    }
}

#[derive(Clone)]
struct FindTool {
    root: PathBuf,
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
                "Find files or directories recursively under an existing relative or absolute directory path by glob pattern. Relative paths resolve from the current working directory and default to it. Patterns containing '/' are relative to the search path; other patterns match each entry name. Results are deterministic relative or absolute paths sorted alphabetically. Respects ignore files and searches hidden paths.",
            )
            .with_input_schema(json!({
                "type": "object",
                "properties": {
                    "pattern": {
                        "type": "string",
                        "description": "Glob pattern to match entry names or search-relative paths, for example \"*.rs\", \"**/*.json\", or \"src/**/*.spec.ts\""
                    },
                    "path": {
                        "type": "string",
                        "description": "Existing relative or absolute directory path; relative paths resolve from the current working directory and default to it"
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
            .with_prompt_guidelines("Prefer find over shell find.")
    }

    fn execute<'a>(
        &'a self,
        call: ToolCall,
        cancellation: &'a CancellationToken,
    ) -> ToolExecutionFuture<'a> {
        let root = self.root.clone();
        let call_id = call.call_id.clone();
        let cancellation = cancellation.clone();
        Box::pin(async move {
            if cancellation.is_cancelled() {
                return ToolResult::error(call.call_id, TOOL_CALL_INTERRUPTED);
            }
            match task::spawn_blocking(move || run_find(root, call, &cancellation)).await {
                Ok(Ok((call_id, outcome))) => find_result(call_id, outcome),
                Ok(Err((call_id, error))) => ToolResult::error(call_id, error.to_string()),
                Err(error) => join_error_result(call_id, error),
            }
        })
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

fn run_find(
    root: PathBuf,
    call: ToolCall,
    cancellation: &CancellationToken,
) -> Result<(String, FindOutcome), (String, SearchToolError)> {
    let call_id = call.call_id;
    let outcome = (|| {
        if cancellation.is_cancelled() {
            return Err(SearchToolError::Interrupted);
        }
        let arguments = parse_arguments(call.arguments)?;
        let access = local_workspace_access();
        let root = access
            .canonicalize(&root)
            .map_err(|source| SearchToolError::WorkspaceRoot { path: root, source })?;
        let search_path = resolve_read_path(access.as_ref(), &root, &arguments.requested_path)
            .map_err(|source| SearchToolError::WorkspacePath { source })?;
        let metadata = fs::metadata(&search_path).map_err(|source| SearchToolError::PathIo {
            operation: "read find search path metadata",
            path: search_path.clone(),
            source,
        })?;
        if !metadata.is_dir() {
            return Err(SearchToolError::SearchPathNotDirectory {
                path: search_path.clone(),
            });
        }
        rust_find(&root, &search_path, &arguments, cancellation)
    })();

    outcome
        .map(|outcome| (call_id.clone(), outcome))
        .map_err(|error| (call_id, error))
}

fn parse_arguments(value: serde_json::Value) -> Result<NormalizedFindArguments, SearchToolError> {
    let arguments = serde_json::from_value::<FindArguments>(value).map_err(|source| {
        SearchToolError::InvalidArguments {
            tool: FIND_TOOL_NAME,
            source,
        }
    })?;
    let pattern = arguments.pattern.trim();
    if pattern.is_empty() {
        return Err(SearchToolError::MissingPattern);
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

fn rust_find(
    workspace_root: &Path,
    search_root: &Path,
    arguments: &NormalizedFindArguments,
    cancellation: &CancellationToken,
) -> Result<FindOutcome, SearchToolError> {
    let matcher = FindMatcher::new(&arguments.pattern)?;
    let mut paths = BoundedSortedPaths::new(arguments.limit);
    for entry in build_search_walker(search_root, true) {
        if cancellation.is_cancelled() {
            return Err(SearchToolError::Interrupted);
        }
        let entry = entry.map_err(|source| SearchToolError::WalkSearchPath { source })?;
        let path = entry.path();
        if path == search_root {
            continue;
        }
        let search_relative = search_relative_path(search_root, path);
        if path_has_vcs_component(search_relative) || !matcher.matches(path, search_relative) {
            continue;
        }
        paths.push(model_search_path(workspace_root, path));
    }
    let total_entries = paths.total_entries();
    Ok(format_find_paths(
        paths.into_paths(),
        total_entries,
        arguments.limit,
    ))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FindMatchTarget {
    Basename,
    SearchRelativePath,
}

struct FindMatcher {
    matcher: GlobMatcher,
    target: FindMatchTarget,
}

impl FindMatcher {
    fn new(pattern: &str) -> Result<Self, SearchToolError> {
        let matcher = GlobBuilder::new(pattern)
            .literal_separator(true)
            .case_insensitive(false)
            .build()
            .map(|glob| glob.compile_matcher())
            .map_err(|source| SearchToolError::InvalidGlob {
                pattern: pattern.to_string(),
                source,
            })?;
        let target = if pattern.contains('/') {
            FindMatchTarget::SearchRelativePath
        } else {
            FindMatchTarget::Basename
        };
        Ok(Self { matcher, target })
    }

    fn matches(&self, path: &Path, search_relative: &Path) -> bool {
        let target = match self.target {
            FindMatchTarget::Basename => path.file_name().map(Path::new).unwrap_or(path),
            FindMatchTarget::SearchRelativePath => search_relative,
        };
        self.matcher.is_match(target)
    }
}

fn format_find_paths(paths: Vec<String>, total_entries: usize, limit: usize) -> FindOutcome {
    let retained_entries = paths.len().min(limit);
    let result_limit_reached = total_entries > retained_entries;
    let mut content = String::new();
    let mut shown_entries = 0usize;

    for path in paths.iter().take(retained_entries) {
        let separator_bytes = usize::from(!content.is_empty());
        if content.len() + separator_bytes + path.len() > SEARCH_MAX_OUTPUT_BYTES {
            break;
        }
        if !content.is_empty() {
            content.push('\n');
        }
        content.push_str(path);
        shown_entries += 1;
    }

    let byte_truncated = shown_entries < retained_entries;
    if total_entries == 0 {
        content = "No paths found.".to_string();
    }
    let mut notices = Vec::new();
    if result_limit_reached {
        notices.push(format!(
            "result limit {limit} reached ({total_entries} paths matched)"
        ));
    }
    if byte_truncated {
        notices.push(format!(
            "{} limit reached",
            format_bytes(SEARCH_MAX_OUTPUT_BYTES)
        ));
    }
    if !notices.is_empty() {
        if !content.is_empty() {
            content.push_str("\n\n");
        }
        content.push_str(&format!("[{}]", notices.join(". ")));
    }

    FindOutcome {
        content,
        total_entries,
        shown_entries,
        truncated: result_limit_reached || byte_truncated,
        byte_truncated,
        backend: "rust",
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
    ToolResult::error(
        call_id,
        SearchToolError::JoinTask {
            operation: "find",
            source: error,
        }
        .to_string(),
    )
}

#[cfg(test)]
mod tests {
    use std::{
        ffi::OsString,
        fs,
        path::{Path, PathBuf},
        time::{SystemTime, UNIX_EPOCH},
    };

    #[cfg(unix)]
    use std::os::unix::{ffi::OsStringExt, fs::symlink};

    use super::*;

    #[test]
    fn parse_arguments_preserves_find_deserialization_error() {
        let result = parse_arguments(json!({ "pattern": 12 }));

        assert!(matches!(
            result,
            Err(SearchToolError::InvalidArguments { tool: "find", .. })
        ));
    }

    #[test]
    fn parse_arguments_caps_find_limit() {
        let arguments = parse_arguments(json!({
            "pattern": "*",
            "limit": usize::MAX,
        }))
        .expect("arguments should parse");

        assert_eq!(arguments.limit, MAX_ENTRY_LIMIT);
    }

    #[test]
    fn find_matcher_rejects_invalid_glob_pattern() {
        let result = FindMatcher::new("[");

        assert!(matches!(
            result,
            Err(SearchToolError::InvalidGlob { pattern, .. }) if pattern == "["
        ));
    }

    #[test]
    fn path_glob_is_anchored_and_does_not_cross_separators() {
        let matcher = FindMatcher::new("src/*.rs").expect("glob should compile");

        assert!(matcher.matches(Path::new("/root/src/lib.rs"), Path::new("src/lib.rs")));
        assert!(!matcher.matches(
            Path::new("/root/src/bin/main.rs"),
            Path::new("src/bin/main.rs")
        ));
        assert!(!matcher.matches(
            Path::new("/root/nested/src/lib.rs"),
            Path::new("nested/src/lib.rs")
        ));
    }

    #[test]
    fn explicit_double_star_matches_nested_path() {
        let matcher = FindMatcher::new("**/src/*.rs").expect("glob should compile");

        assert!(matcher.matches(
            Path::new("/root/nested/src/lib.rs"),
            Path::new("nested/src/lib.rs")
        ));
    }

    #[test]
    fn basename_glob_is_recursive_and_case_sensitive() {
        let matcher = FindMatcher::new("*.rs").expect("glob should compile");

        assert!(matcher.matches(Path::new("/root/nested/lib.rs"), Path::new("nested/lib.rs")));
        assert!(!matcher.matches(Path::new("/root/nested/LIB.RS"), Path::new("nested/LIB.RS")));
    }

    #[test]
    fn rust_find_returns_search_results_relative_to_workspace() {
        let root = temp_root("find-rust-relative");
        fs::create_dir_all(root.join("src/bin")).expect("directories should exist");
        fs::write(root.join("src/lib.rs"), "").expect("file should exist");
        fs::write(root.join("src/bin/main.rs"), "").expect("file should exist");
        let arguments = normalized("src/*.rs", ".", 10);

        let outcome = rust_find(&root, &root, &arguments, &CancellationToken::new())
            .expect("find should succeed");

        assert_eq!(outcome.content, "src/lib.rs");
        assert_eq!(outcome.backend, "rust");
        cleanup(&root);
    }

    #[test]
    fn rust_find_returns_external_results_as_absolute_paths() {
        let workspace = temp_root("find-rust-workspace");
        let external = temp_root("find-rust-external");
        fs::write(external.join("match.rs"), "").expect("file should exist");
        let arguments = normalized("*.rs", external.to_string_lossy().as_ref(), 10);

        let outcome = rust_find(&workspace, &external, &arguments, &CancellationToken::new())
            .expect("find should succeed");

        assert_eq!(
            outcome.content,
            external.join("match.rs").display().to_string()
        );
        cleanup(&external);
        cleanup(&workspace);
    }

    #[test]
    fn rust_find_traversal_includes_hidden_files_and_directories_but_honors_ignores_and_vcs() {
        let root = temp_root("find-rust-traversal");
        fs::create_dir_all(root.join("src")).expect("source directory should exist");
        fs::create_dir_all(root.join("ignored")).expect("ignored directory should exist");
        fs::create_dir_all(root.join("ignored-by-dot-ignore"))
            .expect(".ignore directory should exist");
        fs::create_dir_all(root.join(".git")).expect("VCS directory should exist");
        fs::write(root.join(".hidden.rs"), "").expect("hidden file should exist");
        fs::write(root.join("src/lib.rs"), "").expect("source file should exist");
        fs::write(root.join("ignored/skip.rs"), "").expect("ignored file should exist");
        fs::write(root.join("ignored-by-dot-ignore/skip.rs"), "")
            .expect(".ignore file should exist");
        fs::write(root.join(".git/config"), "").expect("VCS file should exist");
        fs::write(root.join(".gitignore"), "ignored/\n").expect("ignore file should exist");
        fs::write(root.join(".ignore"), "ignored-by-dot-ignore/\n")
            .expect(".ignore rules should exist");

        let outcome = rust_find(
            &root,
            &root,
            &normalized("*", ".", 100),
            &CancellationToken::new(),
        )
        .expect("find should succeed");

        assert!(outcome.content.lines().any(|path| path == ".hidden.rs"));
        assert!(outcome.content.lines().any(|path| path == "src"));
        assert!(outcome.content.lines().any(|path| path == "src/lib.rs"));
        assert!(!outcome.content.contains("ignored"));
        assert!(!outcome.content.contains("ignored-by-dot-ignore"));
        assert!(!outcome.content.contains(".git/config"));
        assert!(!outcome.content.lines().any(|path| path == "."));
        cleanup(&root);
    }

    #[test]
    fn rust_find_search_subpath_keeps_workspace_relative_output_and_parent_ignore_rules() {
        let root = temp_root("find-rust-subpath");
        let search_root = root.join("src");
        fs::create_dir_all(&search_root).expect("source directory should exist");
        fs::write(root.join(".gitignore"), "ignored.rs\n").expect("ignore file should exist");
        fs::write(search_root.join("lib.rs"), "").expect("source file should exist");
        fs::write(search_root.join("ignored.rs"), "").expect("ignored file should exist");

        let outcome = rust_find(
            &root,
            &search_root,
            &normalized("*.rs", "src", 10),
            &CancellationToken::new(),
        )
        .expect("find should succeed");

        assert_eq!(outcome.content, "src/lib.rs");
        cleanup(&root);
    }

    #[test]
    fn rust_find_returns_deterministic_top_n_and_accurate_match_count() {
        let root = temp_root("find-rust-top-n");
        for name in ["c.txt", "A.txt", "b.txt"] {
            fs::write(root.join(name), "").expect("fixture should exist");
        }

        let outcome = rust_find(
            &root,
            &root,
            &normalized("*.txt", ".", 2),
            &CancellationToken::new(),
        )
        .expect("find should succeed");

        assert_eq!(outcome.total_entries, 3);
        assert_eq!(outcome.shown_entries, 2);
        assert!(outcome.content.starts_with("A.txt\nb.txt\n\n"));
        assert!(
            outcome
                .content
                .contains("result limit 2 reached (3 paths matched)")
        );
        cleanup(&root);
    }

    #[test]
    fn find_byte_limit_counts_only_complete_displayed_paths() {
        let paths = vec!["a".repeat(30_000), "b".repeat(30_000)];

        let outcome = format_find_paths(paths, 2, 10);

        assert_eq!(outcome.shown_entries, 1);
        assert!(outcome.byte_truncated);
        assert!(outcome.truncated);
        assert!(!outcome.content.contains(&"b".repeat(30_000)));
    }

    #[test]
    fn find_byte_limit_does_not_emit_a_partial_first_path() {
        let path = "a".repeat(SEARCH_MAX_OUTPUT_BYTES + 1);

        let outcome = format_find_paths(vec![path.clone()], 1, 10);

        assert_eq!(outcome.shown_entries, 0);
        assert!(outcome.byte_truncated);
        assert!(!outcome.content.contains(&path));
        assert!(outcome.content.contains("50.0KB limit reached"));
    }

    #[test]
    fn cancelled_find_stops_before_traversal() {
        let root = temp_root("find-rust-cancelled");
        let cancellation = CancellationToken::new();
        cancellation.cancel();

        let result = rust_find(&root, &root, &normalized("*", ".", 10), &cancellation);

        assert!(matches!(result, Err(SearchToolError::Interrupted)));
        cleanup(&root);
    }

    #[test]
    fn find_rejects_file_search_root() {
        let root = temp_root("find-file-root");
        let file = root.join("file.txt");
        fs::write(&file, "content").expect("file should exist");
        let call = ToolCall::new(
            "find-file-root",
            FIND_TOOL_NAME,
            json!({ "pattern": "*", "path": file }),
        );

        let error = run_find(root.clone(), call, &CancellationToken::new())
            .expect_err("file search root should fail")
            .1;

        assert!(matches!(
            error,
            SearchToolError::SearchPathNotDirectory { .. }
        ));
        cleanup(&root);
    }

    #[cfg(unix)]
    #[test]
    fn find_matches_non_utf8_filename_before_lossy_rendering() {
        let root = temp_root("find-non-utf8");
        let name = OsString::from_vec(vec![b'm', b'a', b't', b'c', b'h', 0xFF]);
        fs::write(root.join(&name), "").expect("file should exist");

        let outcome = rust_find(
            &root,
            &root,
            &normalized("match*", ".", 10),
            &CancellationToken::new(),
        )
        .expect("find should succeed");

        assert_eq!(outcome.total_entries, 1);
        assert!(outcome.content.starts_with("match"));
        cleanup(&root);
    }

    #[cfg(unix)]
    #[test]
    fn find_reports_symlinks_without_following_symlinked_directories() {
        let root = temp_root("find-symlinks");
        let outside = temp_root("find-symlinks-outside");
        fs::write(outside.join("nested.txt"), "").expect("file should exist");
        symlink(&outside, root.join("directory.link")).expect("directory symlink should exist");
        symlink(root.join("missing"), root.join("broken.link"))
            .expect("broken symlink should exist");

        let outcome = rust_find(
            &root,
            &root,
            &normalized("*.link", ".", 10),
            &CancellationToken::new(),
        )
        .expect("find should succeed");

        assert_eq!(outcome.content, "broken.link\ndirectory.link");
        assert!(!outcome.content.contains("nested.txt"));
        cleanup(&outside);
        cleanup(&root);
    }

    fn normalized(pattern: &str, requested_path: &str, limit: usize) -> NormalizedFindArguments {
        NormalizedFindArguments {
            pattern: pattern.to_string(),
            requested_path: requested_path.to_string(),
            limit,
        }
    }

    fn temp_root(label: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be after epoch")
            .as_nanos();
        let root =
            std::env::temp_dir().join(format!("hunea-{label}-{}-{nonce}", std::process::id()));
        fs::create_dir_all(&root).expect("temp root should be created");
        fs::canonicalize(root).expect("temp root should canonicalize")
    }

    fn cleanup(path: &Path) {
        let _ = fs::remove_dir_all(path);
    }
}
