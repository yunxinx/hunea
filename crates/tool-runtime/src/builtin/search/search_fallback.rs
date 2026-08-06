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

pub(crate) fn search_relative_path<'a>(search_root: &Path, path: &'a Path) -> &'a Path {
    path.strip_prefix(search_root).unwrap_or(path)
}

pub(crate) fn model_search_path(workspace_root: &Path, path: &Path) -> String {
    if let Ok(relative) = path.strip_prefix(workspace_root)
        && !relative.as_os_str().is_empty()
    {
        normalized_path_text(relative)
    } else {
        normalized_path_text(path)
    }
}

fn normalized_path_text(path: &Path) -> String {
    let normalized = path.components().collect::<PathBuf>();
    let text = normalized.to_string_lossy();
    if std::path::MAIN_SEPARATOR == '/' {
        text.into_owned()
    } else {
        text.replace(std::path::MAIN_SEPARATOR, "/")
    }
}

pub(crate) fn build_search_walker(start_path: &Path, include_hidden: bool) -> ignore::Walk {
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

pub(crate) fn path_matches_glob(
    search_root: &Path,
    search_root_is_file: bool,
    path: &Path,
    matcher: &GlobMatcher,
) -> bool {
    let target = if search_root_is_file {
        path.file_name().map(Path::new).unwrap_or(path)
    } else {
        search_relative_path(search_root, path)
    };
    matcher.is_match(target)
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
    fn model_search_paths_preserve_workspace_and_external_identity() {
        let root = Path::new("/workspace");

        assert_eq!(
            model_search_path(root, &root.join("src/lib.rs")),
            "src/lib.rs"
        );
        assert_eq!(
            model_search_path(root, Path::new("/external/lib.rs")),
            "/external/lib.rs"
        );
        assert_eq!(
            model_search_path(root, Path::new("/external/./lib.rs")),
            "/external/lib.rs"
        );
        assert_eq!(
            search_relative_path(&root.join("src"), &root.join("src/lib.rs")),
            Path::new("lib.rs")
        );
    }

    #[tokio::test]
    async fn stderr_task_output_preserves_join_error_message() {
        let task = tokio::spawn(async { panic!("stderr reader failed") });

        let output = stderr_task_output(Some(task)).await;

        assert!(output.contains("stderr reader panicked:"));
        assert!(output.contains("stderr reader failed"));
    }
}
