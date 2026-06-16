use std::path::Path;

pub(crate) fn normalize_project_dir(work_dir: &Path) -> String {
    work_dir
        .canonicalize()
        .unwrap_or_else(|_| work_dir.to_path_buf())
        .to_string_lossy()
        .into_owned()
}

pub(crate) fn truncate_chars(text: &str, limit: usize) -> String {
    text.chars().take(limit).collect()
}

pub(crate) fn truncate_chars_with_ellipsis(text: &str, limit: usize) -> String {
    let mut output = String::new();
    for (index, character) in text.chars().enumerate() {
        if index >= limit {
            output.push('…');
            break;
        }
        output.push(character);
    }
    output
}
