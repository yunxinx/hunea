use std::{
    collections::BTreeSet,
    ffi::OsStr,
    path::{Path, PathBuf},
};

use globset::{Glob, GlobMatcher};
use ignore::WalkBuilder;
use tokio::{io::AsyncReadExt, task::JoinHandle};

use super::error::SearchToolError;

pub(crate) const TOOL_CALL_INTERRUPTED: &str = "Tool call interrupted";
pub(crate) const SEARCH_MAX_OUTPUT_BYTES: usize = 50 * 1024;
pub(crate) const GREP_MAX_LINE_CHARS: usize = 500;
pub(crate) const VCS_DIRECTORIES_TO_EXCLUDE: &[&str] =
    &[".git", ".svn", ".hg", ".bzr", ".jj", ".sl"];

#[derive(Debug, Clone)]
pub(crate) struct BoundedSortedPaths {
    limit: usize,
    total_entries: usize,
    entries: BTreeSet<(String, String)>,
}

impl BoundedSortedPaths {
    pub(crate) fn new(limit: usize) -> Self {
        Self {
            limit: limit.max(1),
            total_entries: 0,
            entries: BTreeSet::new(),
        }
    }

    pub(crate) fn push(&mut self, path: String) {
        self.total_entries += 1;
        self.entries.insert((path.to_lowercase(), path));
        if self.entries.len() > self.limit
            && let Some(last) = self.entries.iter().next_back().cloned()
        {
            self.entries.remove(&last);
        }
    }

    pub(crate) const fn total_entries(&self) -> usize {
        self.total_entries
    }

    pub(crate) fn into_paths(self) -> Vec<String> {
        self.entries.into_iter().map(|(_, path)| path).collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HeadTruncation {
    pub content: String,
    pub is_truncated: bool,
    pub total_bytes: usize,
    pub output_bytes: usize,
}

pub(crate) fn workspace_relative_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

pub(crate) fn workspace_relative_cli_path(root: &Path, path: &Path) -> String {
    let relative_path = workspace_relative_path(root, path);
    if relative_path.is_empty() {
        ".".to_string()
    } else {
        relative_path
    }
}

pub(crate) fn build_workspace_walker(
    root: &Path,
    start_path: &Path,
    include_hidden: bool,
) -> ignore::Walk {
    let mut builder = WalkBuilder::new(start_path);
    builder
        .standard_filters(true)
        .parents(true)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .require_git(false)
        .hidden(!include_hidden)
        .sort_by_file_name(|left, right| left.cmp(right))
        .filter_entry(|entry| !is_vcs_directory_name(entry.file_name()));
    if start_path != root {
        builder.add_custom_ignore_filename(".gitignore");
    }
    builder.build()
}

pub(crate) fn is_vcs_directory_name(name: &OsStr) -> bool {
    let name = name.to_string_lossy();
    VCS_DIRECTORIES_TO_EXCLUDE
        .iter()
        .any(|excluded| name == *excluded)
}

pub(crate) fn path_has_vcs_component(path: &Path) -> bool {
    path.components()
        .any(|component| is_vcs_directory_name(component.as_os_str()))
}

pub(crate) fn path_text_has_vcs_component(path: &str) -> bool {
    path.split(['/', '\\'])
        .any(|component| VCS_DIRECTORIES_TO_EXCLUDE.contains(&component))
}

pub(crate) fn compile_glob(pattern: &str) -> Result<GlobMatcher, SearchToolError> {
    Glob::new(pattern)
        .map(|glob| glob.compile_matcher())
        .map_err(|source| SearchToolError::InvalidGlob {
            pattern: pattern.to_string(),
            source,
        })
}

pub(crate) fn path_matches_glob(root: &Path, path: &Path, matcher: &GlobMatcher) -> bool {
    let relative = PathBuf::from(workspace_relative_path(root, path));
    matcher.is_match(relative)
}

pub(crate) fn truncate_line(text: &str, max_chars: usize) -> (String, bool) {
    let mut chars = text.chars();
    let truncated = chars.by_ref().take(max_chars).collect::<String>();
    if chars.next().is_some() {
        (format!("{truncated}..."), true)
    } else {
        (truncated, false)
    }
}

pub(crate) fn truncate_head_by_bytes(content: String, max_bytes: usize) -> HeadTruncation {
    let total_bytes = content.len();
    if total_bytes <= max_bytes {
        return HeadTruncation {
            output_bytes: total_bytes,
            content,
            is_truncated: false,
            total_bytes,
        };
    }

    let mut output = String::new();
    for line in content.split('\n') {
        let next_len = if output.is_empty() {
            line.len()
        } else {
            output.len() + 1 + line.len()
        };
        if next_len > max_bytes {
            break;
        }
        if !output.is_empty() {
            output.push('\n');
        }
        output.push_str(line);
    }

    HeadTruncation {
        output_bytes: output.len(),
        content: output,
        is_truncated: true,
        total_bytes,
    }
}

pub(crate) fn format_bytes(bytes: usize) -> String {
    if bytes < 1024 {
        format!("{bytes}B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1}KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1}MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

pub(crate) async fn collect_capped_stderr(mut stderr: tokio::process::ChildStderr) -> String {
    const MAX_STDERR_BYTES: usize = 8 * 1024;
    let mut output = Vec::new();
    let mut buffer = [0; 1024];
    while let Ok(read) = stderr.read(&mut buffer).await {
        if read == 0 {
            break;
        }
        let remaining = MAX_STDERR_BYTES.saturating_sub(output.len());
        if remaining > 0 {
            output.extend_from_slice(&buffer[..read.min(remaining)]);
        }
    }
    String::from_utf8_lossy(&output).to_string()
}

pub(crate) async fn stderr_task_output(stderr_task: Option<JoinHandle<String>>) -> String {
    let Some(task) = stderr_task else {
        return String::new();
    };

    task.await
        .unwrap_or_else(|error| format!("stderr reader panicked: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_root_becomes_dot_for_external_cli_arguments() {
        let root = Path::new("/workspace");

        assert_eq!(workspace_relative_cli_path(root, root), ".");
        assert_eq!(workspace_relative_cli_path(root, &root.join("src")), "src");
    }

    #[tokio::test]
    async fn stderr_task_output_preserves_join_error_message() {
        let task = tokio::spawn(async { panic!("stderr reader failed") });

        let output = stderr_task_output(Some(task)).await;

        assert!(output.contains("stderr reader panicked:"));
        assert!(output.contains("stderr reader failed"));
    }
}
