use std::{
    fs,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use crate::envinfo;

use super::Model;

pub(crate) const EXTERNAL_EDITOR_HELPER_WINDOW: Duration = Duration::from_secs(3);

/// `ExternalEditorLaunch` 描述 runner 需要执行的一次外部编辑器启动请求。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalEditorLaunch {
    pub command: Vec<String>,
    pub draft_path: PathBuf,
    pub original_draft: String,
}

impl Model {
    pub(crate) fn prepare_external_editor_launch(&mut self) -> Option<ExternalEditorLaunch> {
        let editor = match envinfo::resolve_external_editor(&self.external_editor) {
            Ok(editor) => editor,
            Err(error) => {
                self.show_transient_status_notice(external_editor_unavailable_text(&error));
                return None;
            }
        };

        let original_draft = self.composer_text().to_string();
        let draft_path = match write_external_editor_draft(&original_draft) {
            Ok(path) => path,
            Err(_) => {
                self.show_transient_status_notice("Failed to prepare external editor");
                return None;
            }
        };

        self.clear_status_notice();

        let mut command = vec![editor.command.to_string_lossy().into_owned()];
        command.extend(editor.args);

        Some(ExternalEditorLaunch {
            command: external_editor_command_with_draft(&command, &draft_path),
            draft_path,
            original_draft,
        })
    }

    pub(crate) fn apply_external_editor_finished(
        &mut self,
        draft_path: &Path,
        original_draft: &str,
        failed: bool,
    ) {
        let old_value = self.composer_text().to_string();
        let old_line = self.composer.line();
        let old_column = self.composer.column();

        if failed {
            self.sync_composer_height();
            self.sync_document_viewport_after_composer_interaction(
                &old_value, old_line, old_column,
            );
            self.show_transient_status_notice("External editor failed");
            let _ = fs::remove_file(draft_path);
            return;
        }

        let content = match fs::read_to_string(draft_path) {
            Ok(content) => content,
            Err(_) => {
                self.sync_composer_height();
                self.sync_document_viewport_after_composer_interaction(
                    &old_value, old_line, old_column,
                );
                self.show_transient_status_notice("Failed to read external editor draft");
                let _ = fs::remove_file(draft_path);
                return;
            }
        };

        let next_draft = normalize_external_editor_draft(&content);
        if next_draft != original_draft {
            self.composer
                .replace_text_and_move_to_end(next_draft.clone());
        }

        self.sync_command_panel_navigation();
        self.sync_external_editor_helper_after_draft_change(&old_value);
        self.sync_composer_height();
        self.sync_document_viewport_after_composer_interaction(&old_value, old_line, old_column);
        let _ = fs::remove_file(draft_path);
    }

    pub(crate) fn sync_external_editor_helper_after_draft_change(&mut self, old_value: &str) {
        let old_eligible =
            self.external_editor_helper_eligible_for_value_at_width(old_value, self.width);
        let new_eligible = self
            .external_editor_helper_eligible_for_value_at_width(self.composer_text(), self.width);

        match (old_eligible, new_eligible) {
            (_, false) => self.set_external_editor_helper_visible(false),
            (false, true) => self.show_external_editor_helper(),
            (true, true) => self.refresh_external_editor_helper_deadline(),
        }
    }

    pub(crate) fn sync_external_editor_helper_after_resize(&mut self, previous_width: u16) {
        let old_eligible = self.external_editor_helper_eligible_for_value_at_width(
            self.composer_text(),
            previous_width,
        );
        let new_eligible = self
            .external_editor_helper_eligible_for_value_at_width(self.composer_text(), self.width);

        match (old_eligible, new_eligible) {
            (_, false) => self.set_external_editor_helper_visible(false),
            (false, true) => self.show_external_editor_helper(),
            (true, true) => self.refresh_external_editor_helper_deadline(),
        }
    }

    pub(crate) fn dismiss_external_editor_helper(&mut self, token: usize) {
        if !self.notice_state.external_editor_helper_visible
            || token != self.notice_state.external_editor_helper_token
        {
            return;
        }

        self.set_external_editor_helper_visible(false);
    }

    pub(crate) fn current_external_editor_helper_text(&self) -> String {
        if !self.notice_state.external_editor_helper_visible || self.external_editor_hint.is_empty()
        {
            return String::new();
        }

        format!("ctrl+g to edit in {}", self.external_editor_hint)
    }

    fn external_editor_helper_eligible_for_value_at_width(&self, value: &str, width: u16) -> bool {
        if !self.external_editor_helper_enabled || self.external_editor_hint.is_empty() {
            return false;
        }
        if value.trim().is_empty() {
            return false;
        }

        self.composer.full_height_for_value_at_width(value, width) > 2
    }

    fn show_external_editor_helper(&mut self) {
        self.notice_state.external_editor_helper_token += 1;
        self.notice_state.external_editor_helper_deadline =
            Some(Instant::now() + EXTERNAL_EDITOR_HELPER_WINDOW);
        self.set_external_editor_helper_visible(true);
    }

    fn refresh_external_editor_helper_deadline(&mut self) {
        if !self.notice_state.external_editor_helper_visible {
            return;
        }

        self.notice_state.external_editor_helper_deadline =
            Some(Instant::now() + EXTERNAL_EDITOR_HELPER_WINDOW);
    }

    fn set_external_editor_helper_visible(&mut self, visible: bool) {
        if self.notice_state.external_editor_helper_visible == visible {
            if !visible {
                self.notice_state.external_editor_helper_deadline = None;
            }
            return;
        }

        self.maybe_clear_selection_for_bottom_status_slot_change();
        self.maybe_clear_pending_composer_cursor_click_for_bottom_status_slot_change();
        let preserved_viewport_state = if self.document_runtime.manual_scroll {
            Some(self.current_document_viewport_state())
        } else {
            None
        };

        self.notice_state.external_editor_helper_visible = visible;
        self.bump_status_line_revision();
        if !visible {
            self.notice_state.external_editor_helper_deadline = None;
        }
        self.sync_after_bottom_status_slot_change(preserved_viewport_state);
    }
}

pub(crate) fn external_editor_command_with_draft(
    command: &[String],
    draft_path: &Path,
) -> Vec<String> {
    if command.is_empty() {
        return Vec::new();
    }

    let draft_path = draft_path.to_string_lossy().into_owned();
    let mut args = command[1..].to_vec();
    if is_shell_wrapper_command(&command[0])
        && let Some(script_index) = shell_script_arg_index(&args)
    {
        let script = args[script_index].clone();
        if is_fish_shell(&command[0]) {
            args.insert(script_index + 1, draft_path);
            let mut full_command = vec![command[0].clone()];
            full_command.extend(args);
            return full_command;
        }
        if shell_script_uses_dollar_zero(&script) {
            let name_index = script_index + 1;
            if name_index >= args.len() {
                args.push(draft_path);
            } else {
                args[name_index] = draft_path;
            }
            let mut full_command = vec![command[0].clone()];
            full_command.extend(args);
            return full_command;
        }

        let name_index = script_index + 1;
        if name_index >= args.len() {
            args.push("lumos".to_string());
            args.push(draft_path);
        } else {
            args.insert(name_index + 1, draft_path);
        }

        let mut full_command = vec![command[0].clone()];
        full_command.extend(args);
        return full_command;
    }

    let mut full_command = command.to_vec();
    full_command.push(draft_path);
    full_command
}

fn write_external_editor_draft(content: &str) -> std::io::Result<PathBuf> {
    let mut counter = 0u64;
    loop {
        let candidate = std::env::temp_dir().join(format!(
            "lumos-draft-{}-{}-{counter}.md",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        match fs::write(&candidate, content) {
            Ok(_) => return Ok(candidate),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                counter += 1;
            }
            Err(error) => return Err(error),
        }
    }
}

fn normalize_external_editor_draft(content: &str) -> String {
    content.replace("\r\n", "\n").replace('\r', "\n")
}

fn external_editor_unavailable_text(error: &envinfo::ExternalEditorError) -> &'static str {
    match error {
        envinfo::ExternalEditorError::ExternalEditorNotFound { .. } => "External editor not found",
        envinfo::ExternalEditorError::ExternalEditorMustWait { .. } => {
            "External editor must wait for close"
        }
        envinfo::ExternalEditorError::NoExternalEditorAvailable => "No external editor available",
    }
}

fn is_shell_wrapper_command(command: &str) -> bool {
    matches!(
        shell_command_name(command).as_str(),
        "sh" | "bash" | "zsh" | "dash" | "ksh" | "fish"
    )
}

fn is_fish_shell(command: &str) -> bool {
    shell_command_name(command) == "fish"
}

fn shell_command_name(command: &str) -> String {
    command
        .trim_end_matches(['/', '\\'])
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(command)
        .to_ascii_lowercase()
        .trim_end_matches(".exe")
        .to_string()
}

fn shell_script_uses_dollar_zero(script: &str) -> bool {
    script.contains("$0") || script.contains("${0}")
}

fn shell_script_arg_index(args: &[String]) -> Option<usize> {
    for (index, arg) in args.iter().enumerate() {
        if arg == "-c" || arg == "--command" {
            return (index + 1 < args.len()).then_some(index + 1);
        }
        if arg.starts_with('-')
            && !arg.starts_with("--")
            && arg[1..].chars().any(|character| character == 'c')
        {
            return (index + 1 < args.len()).then_some(index + 1);
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn external_editor_command_with_draft_adds_shell_name_when_missing() {
        let command = vec![
            "sh".to_string(),
            "-c".to_string(),
            "nvim \"$1\"".to_string(),
        ];
        let full_command = external_editor_command_with_draft(&command, Path::new("/tmp/draft.md"));

        assert_eq!(
            full_command,
            vec![
                "sh".to_string(),
                "-c".to_string(),
                "nvim \"$1\"".to_string(),
                "lumos".to_string(),
                "/tmp/draft.md".to_string(),
            ]
        );
    }

    #[test]
    fn external_editor_command_with_draft_replaces_dollar_zero_when_requested() {
        let command = vec![
            "sh".to_string(),
            "-c".to_string(),
            "nvim \"$0\"".to_string(),
        ];
        let full_command = external_editor_command_with_draft(&command, Path::new("/tmp/draft.md"));

        assert_eq!(
            full_command,
            vec![
                "sh".to_string(),
                "-c".to_string(),
                "nvim \"$0\"".to_string(),
                "/tmp/draft.md".to_string(),
            ]
        );
    }

    #[test]
    fn external_editor_command_with_draft_does_not_inject_shell_name_for_fish() {
        let command = vec![
            "fish".to_string(),
            "-c".to_string(),
            "nvim $argv[1]".to_string(),
        ];
        let full_command = external_editor_command_with_draft(&command, Path::new("/tmp/draft.md"));

        assert_eq!(
            full_command,
            vec![
                "fish".to_string(),
                "-c".to_string(),
                "nvim $argv[1]".to_string(),
                "/tmp/draft.md".to_string(),
            ]
        );
    }

    #[test]
    fn normalize_external_editor_draft_normalizes_crlf() {
        assert_eq!(
            normalize_external_editor_draft("after\r\nmore\r\n"),
            "after\nmore\n"
        );
    }
}
