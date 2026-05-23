use std::path::Path;

use runtime_domain::session::{
    RuntimeToolActivity, RuntimeToolActivityContent, RuntimeToolActivityRawValue,
    RuntimeToolActivityStatus, RuntimeToolActivityUpdate, RuntimeToolKind,
};

/// `ToolApprovalPreview` 表示审批面板中可直接展示的工具变更预览。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ToolApprovalPreview {
    action: ToolApprovalPreviewAction,
    path: String,
    content: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ToolApprovalPreviewAction {
    CreateFile,
    EditFile,
}

impl ToolApprovalPreview {
    pub(crate) fn create_file(path: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            action: ToolApprovalPreviewAction::CreateFile,
            path: path.into(),
            content: content.into(),
        }
    }

    pub(crate) fn from_runtime_tool_activity_update(
        update: &RuntimeToolActivityUpdate,
    ) -> Option<Self> {
        update
            .content
            .as_deref()
            .and_then(file_preview_from_runtime_content)
            .or_else(|| {
                let path = runtime_write_tool_activity_update_target(update)?;
                let content = update.raw_input.as_ref().and_then(raw_input_content)?;
                Some(Self::file_write(path, content))
            })
    }

    pub(crate) fn question(&self) -> String {
        let verb = match self.action {
            ToolApprovalPreviewAction::CreateFile => "create",
            ToolApprovalPreviewAction::EditFile => "edit",
        };
        format!("Do you want to {verb} {}?", self.path)
    }

    pub(crate) fn path(&self) -> &str {
        &self.path
    }

    pub(crate) fn content(&self) -> &str {
        &self.content
    }

    fn file_write(path: String, content: String) -> Self {
        let action = if Path::new(&path).exists() {
            ToolApprovalPreviewAction::EditFile
        } else {
            ToolApprovalPreviewAction::CreateFile
        };
        Self {
            action,
            path: runtime_display_path(&path),
            content,
        }
    }
}

pub(crate) fn runtime_display_path(path: &str) -> String {
    let path_ref = Path::new(path);
    if path_ref.is_absolute()
        && let Ok(cwd) = std::env::current_dir()
        && let Ok(stripped) = path_ref.strip_prefix(cwd)
        && !stripped.as_os_str().is_empty()
    {
        return stripped.display().to_string();
    }

    path_ref.display().to_string()
}

pub(crate) fn is_runtime_write_tool_activity(call: &RuntimeToolActivity) -> bool {
    runtime_write_tool_activity_title_target(&call.title).is_some()
        || call.kind == RuntimeToolKind::Write
        || (call.kind == RuntimeToolKind::Edit
            && call.raw_input.as_ref().is_some_and(|raw_input| {
                raw_input_path(raw_input).is_some() && raw_input_content(raw_input).is_some()
            }))
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

fn runtime_write_tool_activity_update_target(update: &RuntimeToolActivityUpdate) -> Option<String> {
    update
        .title
        .as_deref()
        .and_then(runtime_write_tool_activity_title_target)
        .or_else(|| update.raw_input.as_ref().and_then(raw_input_path))
        .map(|path| runtime_display_path(&path))
}

fn runtime_write_tool_activity_title_target(title: &str) -> Option<String> {
    let title = title.trim();
    ["WriteFile:", "Write File:", "Write:", "Write "]
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
        } = content
        else {
            return None;
        };
        let action = if old_text.as_deref().unwrap_or_default().is_empty() {
            ToolApprovalPreviewAction::CreateFile
        } else {
            ToolApprovalPreviewAction::EditFile
        };
        if action == ToolApprovalPreviewAction::CreateFile {
            return Some(ToolApprovalPreview::create_file(
                runtime_display_path(path),
                new_text.clone(),
            ));
        }

        Some(ToolApprovalPreview {
            action,
            path: runtime_display_path(path),
            content: new_text.clone(),
        })
    })
}

fn raw_input_path(raw_input: &RuntimeToolActivityRawValue) -> Option<String> {
    raw_input_string_field(raw_input, &["path", "file_path", "filePath"])
        .filter(|path| !path.trim().is_empty())
}

fn raw_input_content(raw_input: &RuntimeToolActivityRawValue) -> Option<String> {
    raw_input_string_field(
        raw_input,
        &[
            "content",
            "new_string",
            "newString",
            "new_text",
            "newText",
            "text",
        ],
    )
}

fn raw_input_string_field(
    raw_input: &RuntimeToolActivityRawValue,
    keys: &[&str],
) -> Option<String> {
    raw_input.string_field(keys)
}
