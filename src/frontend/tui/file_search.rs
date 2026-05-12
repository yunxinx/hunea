use std::{
    fs,
    path::{Path, PathBuf},
};

const MAX_SCAN_FILES: usize = 8_000;
const MAX_SCAN_DEPTH: usize = 20;

/// `FileSearchMatch` 表示一个相对工作目录的文件搜索结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FileSearchMatch {
    pub(crate) path: String,
}

impl FileSearchMatch {
    #[cfg(test)]
    fn new_for_test(path: &str) -> Self {
        Self {
            path: path.to_string(),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub(crate) struct FileSearchCache {
    root: Option<PathBuf>,
    files: Vec<String>,
}

impl FileSearchCache {
    /// `search` 复用当前目录快照进行文件过滤，避免每次按键都重新遍历目录。
    pub(crate) fn search(&mut self, root: &Path, query: &str) -> Vec<FileSearchMatch> {
        let root = normalized_root(root);
        if self.root.as_deref() != Some(root.as_path()) {
            self.root = Some(root.clone());
            self.files = scan_files(&root);
        }

        search_paths(&self.files, query)
    }
}

#[cfg(test)]
pub(crate) fn common_match_prefix(matches: &[FileSearchMatch]) -> String {
    let Some(first) = matches.first() else {
        return String::new();
    };

    let mut prefix = first.path.clone();
    for item in matches.iter().skip(1) {
        prefix = common_char_prefix(&prefix, &item.path);
        if prefix.is_empty() {
            break;
        }
    }
    prefix
}

pub(crate) fn common_path_completion_prefix(matches: &[FileSearchMatch], query: &str) -> String {
    let query = query.trim();
    let lower_query = query.to_lowercase();
    let mut prefix = None::<String>;

    for item in matches {
        let is_prefix_candidate =
            query.is_empty() || item.path.to_lowercase().starts_with(&lower_query);
        if !is_prefix_candidate {
            continue;
        }

        prefix = Some(match prefix {
            Some(current) => common_char_prefix(&current, &item.path),
            None => item.path.clone(),
        });
        if prefix.as_deref() == Some("") {
            break;
        }
    }

    prefix.unwrap_or_default()
}

fn search_paths(paths: &[String], query: &str) -> Vec<FileSearchMatch> {
    let query = query.trim();
    if query.is_empty() {
        return paths
            .iter()
            .map(|path| FileSearchMatch { path: path.clone() })
            .collect();
    }

    let mut scored = paths
        .iter()
        .filter_map(|path| score_path(path, query).map(|score| (score, path.clone())))
        .collect::<Vec<_>>();
    scored.sort_by(|(left_score, left_path), (right_score, right_path)| {
        left_score
            .cmp(right_score)
            .then_with(|| left_path.len().cmp(&right_path.len()))
            .then_with(|| left_path.cmp(right_path))
    });

    scored
        .into_iter()
        .map(|(_, path)| FileSearchMatch { path })
        .collect()
}

fn score_path(path: &str, query: &str) -> Option<usize> {
    let lower_path = path.to_lowercase();
    let lower_query = query.to_lowercase();
    if lower_path.starts_with(&lower_query) {
        return Some(lower_path.len());
    }

    if let Some(offset) = lower_path
        .split('/')
        .scan(0usize, |start, component| {
            let current_start = *start;
            *start += component.len() + 1;
            Some((current_start, component))
        })
        .find_map(|(start, component)| component.starts_with(&lower_query).then_some(start))
    {
        return Some(10_000 + offset + lower_path.len());
    }

    fuzzy_subsequence_score(&lower_path, &lower_query).map(|score| 20_000 + score)
}

fn fuzzy_subsequence_score(path: &str, query: &str) -> Option<usize> {
    let mut score = 0usize;
    let mut last_match = None;
    let mut path_indices = path.char_indices().peekable();

    for query_char in query.chars() {
        let mut found = None;
        for (index, path_char) in path_indices.by_ref() {
            if path_char == query_char {
                found = Some(index);
                break;
            }
        }
        let index = found?;
        if let Some(previous) = last_match {
            score += index.saturating_sub(previous + 1);
        } else {
            score += index;
        }
        last_match = Some(index);
    }

    Some(score + path.len())
}

fn scan_files(root: &Path) -> Vec<String> {
    let mut files = Vec::new();
    scan_dir(root, root, 0, &mut files);
    files.sort();
    files.dedup();
    files
}

fn scan_dir(root: &Path, dir: &Path, depth: usize, files: &mut Vec<String>) {
    if files.len() >= MAX_SCAN_FILES || depth > MAX_SCAN_DEPTH {
        return;
    }

    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut entries = entries.filter_map(Result::ok).collect::<Vec<_>>();
    entries.sort_by(|left, right| {
        entry_name(left)
            .cmp(&entry_name(right))
            .then_with(|| entry_path(left).cmp(&entry_path(right)))
    });

    for entry in entries {
        if files.len() >= MAX_SCAN_FILES {
            return;
        }
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };

        if file_type.is_dir() {
            if should_skip_dir(&path) {
                continue;
            }
            scan_dir(root, &path, depth + 1, files);
            continue;
        }

        if !file_type.is_file() {
            continue;
        }

        let Ok(relative) = path.strip_prefix(root) else {
            continue;
        };
        let Some(relative) = relative_path_string(relative) else {
            continue;
        };
        files.push(relative);
    }
}

fn should_skip_dir(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };

    matches!(
        name,
        ".git" | "target" | "node_modules" | ".cache" | "dist" | "build"
    )
}

fn relative_path_string(path: &Path) -> Option<String> {
    let mut parts = Vec::new();
    for component in path.components() {
        parts.push(component.as_os_str().to_str()?);
    }
    Some(parts.join("/"))
}

fn normalized_root(root: &Path) -> PathBuf {
    root.components().collect()
}

fn common_char_prefix(left: &str, right: &str) -> String {
    left.chars()
        .zip(right.chars())
        .take_while(|(left, right)| left == right)
        .map(|(left, _)| left)
        .collect()
}

fn entry_name(entry: &fs::DirEntry) -> String {
    entry.file_name().to_string_lossy().to_string()
}

fn entry_path(entry: &fs::DirEntry) -> PathBuf {
    entry.path()
}

#[cfg(test)]
fn search_paths_for_test(paths: &[String], query: &str) -> Vec<FileSearchMatch> {
    search_paths(paths, query)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_prefers_path_prefix_then_component_prefix_then_fuzzy_match() {
        let paths = vec![
            "README.md".to_string(),
            "src/frontend/tui/mod.rs".to_string(),
            "src/main.rs".to_string(),
            "tests/file_picker.rs".to_string(),
        ];

        let results = search_paths_for_test(&paths, "src");

        assert_eq!(
            result_paths(&results),
            vec!["src/main.rs", "src/frontend/tui/mod.rs"]
        );
    }

    #[test]
    fn common_prefix_uses_every_matching_path() {
        let paths = vec![
            FileSearchMatch::new_for_test("src/lib.rs"),
            FileSearchMatch::new_for_test("src/main.rs"),
            FileSearchMatch::new_for_test("src/model.rs"),
        ];

        assert_eq!(common_match_prefix(&paths), "src/");
    }

    #[test]
    fn common_path_completion_prefix_ignores_fuzzy_matches() {
        let paths = vec![
            FileSearchMatch::new_for_test("src/dir1/docs.md"),
            FileSearchMatch::new_for_test("src/dir1/readme.md"),
            FileSearchMatch::new_for_test("docs/status.md"),
        ];

        assert_eq!(common_path_completion_prefix(&paths, "s"), "src/dir1/");
    }

    fn result_paths(results: &[FileSearchMatch]) -> Vec<&str> {
        results.iter().map(|item| item.path.as_str()).collect()
    }
}
