use super::*;

pub(in crate::prompt_assembly) fn path_component_key(path: &Path) -> String {
    let components = path
        .components()
        .filter_map(|component| match component {
            std::path::Component::Normal(value) => value.to_str(),
            _ => None,
        })
        .collect::<Vec<_>>();
    if components.is_empty() {
        ".".to_string()
    } else {
        components.join("/")
    }
}

pub(in crate::prompt_assembly) fn escape_xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

pub(in crate::prompt_assembly) fn git_root(start: &Path) -> Option<PathBuf> {
    let mut current = Some(start);
    while let Some(dir) = current {
        let git_path = dir.join(".git");
        if git_path.is_dir() || git_path.is_file() {
            return Some(dir.to_path_buf());
        }
        current = dir.parent();
    }
    None
}

pub(in crate::prompt_assembly) fn scope_reference_key(
    scope: PromptAssemblyScope,
    reference_id: &str,
) -> String {
    format!("{}::{reference_id}", scope.as_stored_value())
}
