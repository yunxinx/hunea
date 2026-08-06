//! Runtime tool activity preview helpers shared by approval and transcript rendering.

use std::{
    env,
    path::{Component, Path, PathBuf},
};

use runtime_domain::envinfo::shorten_home_prefix;
use runtime_domain::session::{
    RuntimeToolActivity, RuntimeToolActivityContent, RuntimeToolActivityRawValue,
    RuntimeToolActivityStatus, RuntimeToolActivityUpdate, RuntimeToolKind,
};

/// `ToolApprovalPreview` 表示审批面板中可直接展示的工具变更预览。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ToolApprovalPreview {
    path: String,
    old_text: Option<String>,
    new_text: String,
    is_truncated: bool,
}

impl ToolApprovalPreview {
    #[cfg(test)]
    pub(crate) fn create_file(path: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            old_text: None,
            new_text: content.into(),
            is_truncated: false,
        }
    }

    pub(crate) fn from_runtime_tool_activity_update(
        update: &RuntimeToolActivityUpdate,
    ) -> Option<Self> {
        // raw input 没有执行前文件快照，不能据此构造可信的 Added/Edited diff。
        update
            .content
            .as_deref()
            .and_then(file_preview_from_runtime_content)
    }

    pub(crate) fn question(&self) -> String {
        let verb = if self.old_text.is_some() {
            "edit"
        } else {
            "create"
        };
        format!("Do you want to {verb} {}?", self.path)
    }

    pub(crate) fn path(&self) -> &str {
        &self.path
    }

    pub(crate) fn content(&self) -> &str {
        &self.new_text
    }

    pub(crate) fn old_text(&self) -> Option<&str> {
        self.old_text.as_deref()
    }

    pub(crate) const fn is_truncated(&self) -> bool {
        self.is_truncated
    }
}

pub(crate) fn runtime_display_path(path: &str) -> String {
    let cwd = env::current_dir().ok();
    let home = detect_home_dir();
    runtime_display_path_with_roots(path, cwd.as_deref(), home.as_deref())
}

/// `runtime_display_path_with_roots` 只改变 TUI 展示文本，不改写 runtime 保存的原始路径。
/// cwd 内显示相对路径，cwd 外但 home 内显示 `~/...`，其余显示绝对路径。
pub(crate) fn runtime_display_path_with_roots(
    path: &str,
    cwd: Option<&Path>,
    home: Option<&Path>,
) -> String {
    let path = path.trim();
    if path.is_empty() {
        return ".".to_string();
    }

    let normalized_path = lexical_path(Path::new(path));
    let path_ref = normalized_path.as_path();
    if !path_ref.is_absolute() {
        return relative_display_path(path_ref);
    }

    if let Some(cwd) = cwd {
        let normalized_cwd = lexical_path(cwd);
        let cwd = normalized_cwd.as_path();
        if path_ref == cwd {
            return ".".to_string();
        }
        if let Ok(stripped) = path_ref.strip_prefix(cwd)
            && !stripped.as_os_str().is_empty()
        {
            return relative_display_path(stripped);
        }
    }

    if let Some(home) = home {
        return shorten_home_prefix(path_ref, &lexical_path(home));
    }

    path_ref.display().to_string()
}

fn relative_display_path(path: &Path) -> String {
    let normalized = lexical_path(path);

    if normalized.as_os_str().is_empty() {
        ".".to_string()
    } else {
        normalized.display().to_string()
    }
}

fn lexical_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(part) => normalized.push(part),
            Component::ParentDir => normalized.push(".."),
            Component::RootDir | Component::Prefix(_) => normalized.push(component.as_os_str()),
        }
    }

    normalized
}

fn detect_home_dir() -> Option<PathBuf> {
    env::var_os("HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("USERPROFILE").map(PathBuf::from))
        .or_else(|| {
            let home_drive = env::var_os("HOMEDRIVE")?;
            let home_path = env::var_os("HOMEPATH")?;
            let mut path = PathBuf::from(home_drive);
            path.push(home_path);
            Some(path)
        })
}

pub(crate) fn is_runtime_write_tool_activity(call: &RuntimeToolActivity) -> bool {
    matches!(call.kind, RuntimeToolKind::Write | RuntimeToolKind::Edit)
        || runtime_write_tool_activity_title_target(&call.title).is_some()
}

pub(crate) fn should_collapse_runtime_write_tool_activity(call: &RuntimeToolActivity) -> bool {
    is_runtime_write_tool_activity(call)
        && call.status != RuntimeToolActivityStatus::Failed
        && !call
            .content
            .iter()
            .any(|content| matches!(content, RuntimeToolActivityContent::Diff { .. }))
}

pub(crate) fn runtime_write_tool_activity_target(call: &RuntimeToolActivity) -> Option<String> {
    runtime_write_tool_activity_title_target(&call.title)
        .or_else(|| call.raw_input.as_ref().and_then(raw_input_path))
        .map(|path| runtime_display_path(&path))
}

fn runtime_write_tool_activity_title_target(title: &str) -> Option<String> {
    let title = title.trim();
    [
        "WriteFile:",
        "Write File:",
        "Write:",
        "Write ",
        "Edit:",
        "Edit ",
    ]
    .iter()
    .find_map(|prefix| {
        title.strip_prefix(prefix).and_then(|target| {
            let target = target.trim();
            (!target.is_empty()).then(|| target.to_string())
        })
    })
}

fn file_preview_from_runtime_content(
    content: &[RuntimeToolActivityContent],
) -> Option<ToolApprovalPreview> {
    content.iter().find_map(|content| {
        let RuntimeToolActivityContent::Diff {
            path,
            old_text,
            new_text,
            is_truncated,
        } = content
        else {
            return None;
        };
        Some(ToolApprovalPreview {
            path: runtime_display_path(path),
            old_text: old_text.clone(),
            new_text: new_text.clone(),
            is_truncated: *is_truncated,
        })
    })
}

fn raw_input_path(raw_input: &RuntimeToolActivityRawValue) -> Option<String> {
    raw_input_string_field(raw_input, &["path", "file_path", "filePath"])
        .filter(|path| !path.trim().is_empty())
}

fn raw_input_string_field(
    raw_input: &RuntimeToolActivityRawValue,
    keys: &[&str],
) -> Option<String> {
    raw_input.string_field(keys)
}

#[cfg(test)]
mod tests {
    use super::runtime_display_path_with_roots;
    use std::path::Path;

    #[test]
    fn display_path_normalizes_relative_paths() {
        assert_eq!(runtime_display_path_with_roots("", None, None), ".");
        assert_eq!(
            runtime_display_path_with_roots("./src/main.rs", None, None),
            "src/main.rs"
        );
        assert_eq!(runtime_display_path_with_roots(".", None, None), ".");
    }

    #[cfg(unix)]
    #[test]
    fn display_path_uses_cwd_relative_home_relative_then_absolute_precedence() {
        let cwd = Path::new("/home/ziply/project");
        let home = Path::new("/home/ziply");

        assert_eq!(
            runtime_display_path_with_roots("/home/ziply/project", Some(cwd), Some(home)),
            "."
        );
        assert_eq!(
            runtime_display_path_with_roots(
                "/home/ziply/project/src/main.rs",
                Some(cwd),
                Some(home)
            ),
            "src/main.rs"
        );
        assert_eq!(
            runtime_display_path_with_roots(
                "/home/ziply/reference/README.md",
                Some(cwd),
                Some(home)
            ),
            "~/reference/README.md"
        );
        assert_eq!(
            runtime_display_path_with_roots("/opt/./reference/README.md", Some(cwd), Some(home)),
            "/opt/reference/README.md"
        );
    }
}
