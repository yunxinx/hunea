use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    style::Modifier,
    text::{Line, Span},
};

mod file_preview;

use super::{
    AppEffect, Model,
    acp_tool_preview::ToolApprovalPreview,
    inline_panel::{
        InlinePanelRenderResult, append_wrapped_inline_value, inline_panel_render_result,
        inline_panel_rule_line, wrap_inline_text,
    },
    theme::{primary_text_style, secondary_text_style, tertiary_text_style},
    tool_result::ToolResultKind,
    transcript::markdown_highlight::{highlight_code_chunks, wrap_highlight_chunks},
};
use file_preview::build_file_preview_panel_lines;

/// `ToolApprovalPanelState` 保存通用工具审批面板的展示与导航状态。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(super) struct ToolApprovalPanelState {
    pub(super) is_open: bool,
    pub(super) selected: usize,
    pub(super) source: Option<ToolApprovalSource>,
    pub(super) title: String,
    pub(super) details: Vec<ToolApprovalDetail>,
    pub(super) preview: Option<ToolApprovalPreview>,
    pub(super) suspended_acp_tool_call_item_index: Option<usize>,
}

/// `ToolApprovalSource` 描述工具审批确认后需要回到哪个运行时来源。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ToolApprovalSource {
    AcpPermission {
        request_id: String,
        allow_option_id: Option<String>,
        allow_always_option_id: Option<String>,
        reject_option_id: Option<String>,
        reject_always_option_id: Option<String>,
    },
    Preview,
}

/// `ToolApprovalDetail` 表示审批面板中的一行说明信息。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ToolApprovalDetail {
    pub(super) label: String,
    pub(super) value: String,
}

pub(crate) type ToolApprovalPanelRenderResult = InlinePanelRenderResult;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ToolApprovalChoice {
    Allow,
    AllowInSession,
    Deny,
    DenyInSession,
}

impl ToolApprovalChoice {
    fn display_label(self) -> &'static str {
        match self {
            Self::Allow => "Yes",
            Self::AllowInSession => "Yes, allow similar requests during this session",
            Self::Deny => "No",
            Self::DenyInSession => "No, reject similar requests during this session",
        }
    }

    fn file_preview_display_label(self) -> &'static str {
        match self {
            Self::Allow => "Yes",
            Self::AllowInSession => "Yes, allow all edits during this session",
            Self::Deny => "No",
            Self::DenyInSession => "No, reject all edits during this session",
        }
    }
}

impl Model {
    pub(crate) fn tool_approval_panel_active(&self) -> bool {
        self.tool_approval_panel.is_open
    }

    pub(super) fn open_tool_approval_panel(
        &mut self,
        source: ToolApprovalSource,
        title: String,
        details: Vec<ToolApprovalDetail>,
    ) {
        self.open_tool_approval_panel_with_preview(source, title, details, None);
    }

    pub(crate) fn open_tool_approval_panel_with_preview(
        &mut self,
        source: ToolApprovalSource,
        title: String,
        details: Vec<ToolApprovalDetail>,
        preview: Option<ToolApprovalPreview>,
    ) {
        self.restore_suspended_acp_tool_call_for_approval_panel();
        self.close_transcript_overlay();
        self.pause_stream_activity();
        self.model_panel.is_open = false;
        self.acp_panel.is_open = false;
        self.tool_approval_panel = ToolApprovalPanelState {
            is_open: true,
            selected: 0,
            source: Some(source),
            title,
            details,
            preview,
            suspended_acp_tool_call_item_index: None,
        };
        self.tool_approval_panel_revision = self.tool_approval_panel_revision.saturating_add(1);
        self.sync_command_panel_navigation();
        self.sync_file_picker_state();
        self.sync_composer_height();
        self.sync_document_viewport_for_composer_cursor();
    }

    pub(crate) fn close_tool_approval_panel(&mut self) {
        if !self.tool_approval_panel.is_open {
            return;
        }

        let suspended_item_index = self
            .tool_approval_panel
            .suspended_acp_tool_call_item_index
            .take();
        let permission_tool_call_item_index = self
            .pending_acp_permission
            .as_ref()
            .and_then(|permission| permission.tool_call_item_index);
        self.tool_approval_panel = ToolApprovalPanelState::default();
        self.pending_acp_permission = None;
        self.clear_acp_tool_call_permission_waiting(permission_tool_call_item_index);
        self.restore_suspended_acp_tool_call_item(suspended_item_index);
        self.resume_stream_activity();
        self.tool_approval_panel_revision = self.tool_approval_panel_revision.saturating_add(1);
        self.sync_composer_height();
        self.sync_document_viewport_for_composer_cursor();
    }

    pub(crate) fn suspend_acp_tool_call_for_approval_panel(&mut self, item_index: usize) {
        if !self.tool_approval_panel_active() {
            return;
        }

        if self.set_acp_tool_call_approval_suspended_from_runtime(item_index, true) {
            self.tool_approval_panel.suspended_acp_tool_call_item_index = Some(item_index);
        }
    }

    pub(crate) fn handle_tool_approval_panel_key(
        &mut self,
        key: KeyEvent,
    ) -> Option<Option<AppEffect>> {
        if !self.tool_approval_panel_active() {
            return None;
        }

        match key.code {
            KeyCode::Up | KeyCode::Down if key.modifiers.is_empty() => {
                move_tool_approval_selection(
                    &mut self.tool_approval_panel,
                    if key.code == KeyCode::Up {
                        ToolApprovalSelectionMove::Up
                    } else {
                        ToolApprovalSelectionMove::Down
                    },
                );
                self.tool_approval_panel_revision =
                    self.tool_approval_panel_revision.saturating_add(1);
                Some(None)
            }
            KeyCode::Left | KeyCode::Right if key.modifiers.is_empty() => {
                move_tool_approval_selection(
                    &mut self.tool_approval_panel,
                    if key.code == KeyCode::Left {
                        ToolApprovalSelectionMove::Left
                    } else {
                        ToolApprovalSelectionMove::Right
                    },
                );
                self.tool_approval_panel_revision =
                    self.tool_approval_panel_revision.saturating_add(1);
                Some(None)
            }
            KeyCode::Enter if key.modifiers.is_empty() => {
                let choices = tool_approval_choices(&self.tool_approval_panel);
                let choice = choices
                    .get(self.tool_approval_panel.selected)
                    .copied()
                    .unwrap_or(ToolApprovalChoice::Deny);
                Some(self.resolve_tool_approval_choice(choice))
            }
            KeyCode::Char('y') | KeyCode::Char('Y') => Some(
                self.resolve_tool_approval_choice(
                    preferred_tool_approval_choice(
                        &self.tool_approval_panel,
                        &[
                            ToolApprovalChoice::Allow,
                            ToolApprovalChoice::AllowInSession,
                        ],
                    )
                    .unwrap_or(ToolApprovalChoice::Allow),
                ),
            ),
            KeyCode::Char('n') | KeyCode::Char('N') => Some(
                self.resolve_tool_approval_choice(
                    preferred_tool_approval_choice(
                        &self.tool_approval_panel,
                        &[ToolApprovalChoice::Deny, ToolApprovalChoice::DenyInSession],
                    )
                    .unwrap_or(ToolApprovalChoice::Deny),
                ),
            ),
            KeyCode::Esc if key.modifiers.is_empty() => Some(self.cancel_tool_approval_panel()),
            _ => Some(None),
        }
    }

    pub(crate) fn current_inline_tool_approval_panel_render_result(
        &self,
    ) -> ToolApprovalPanelRenderResult {
        if !self.tool_approval_panel_active() {
            return ToolApprovalPanelRenderResult::default();
        }

        let width = usize::from(self.width.max(1));
        let lines = build_panel_lines(self, width);
        inline_panel_render_result(lines)
    }

    fn resolve_tool_approval_choice(&mut self, choice: ToolApprovalChoice) -> Option<AppEffect> {
        let suspended_item_index = self
            .tool_approval_panel
            .suspended_acp_tool_call_item_index
            .take();
        let Some(source) = self.tool_approval_panel.source.clone() else {
            let permission_tool_call_item_index = self
                .pending_acp_permission
                .as_ref()
                .and_then(|permission| permission.tool_call_item_index);
            self.clear_acp_tool_call_permission_waiting(permission_tool_call_item_index);
            self.restore_suspended_acp_tool_call_item(suspended_item_index);
            return None;
        };
        let title = self.tool_approval_panel.title.clone();
        let pending_permission = self.pending_acp_permission.clone();
        let permission_tool_call_item_index = pending_permission
            .as_ref()
            .and_then(|permission| permission.tool_call_item_index);
        let permission_tool_call_id =
            pending_permission.and_then(|permission| permission.tool_call_id);
        self.tool_approval_panel = ToolApprovalPanelState::default();
        self.pending_acp_permission = None;
        self.clear_acp_tool_call_permission_waiting(permission_tool_call_item_index);
        self.restore_suspended_acp_tool_call_item(suspended_item_index);
        self.resume_stream_activity();
        self.tool_approval_panel_revision = self.tool_approval_panel_revision.saturating_add(1);
        self.sync_composer_height();
        self.sync_document_viewport_for_composer_cursor();

        match source {
            ToolApprovalSource::AcpPermission {
                request_id,
                allow_option_id,
                allow_always_option_id,
                reject_option_id,
                reject_always_option_id,
            } => {
                let option_id = match choice {
                    ToolApprovalChoice::Allow => allow_option_id,
                    ToolApprovalChoice::AllowInSession => allow_always_option_id,
                    ToolApprovalChoice::Deny => reject_option_id,
                    ToolApprovalChoice::DenyInSession => reject_always_option_id,
                };
                let is_rejection = matches!(
                    choice,
                    ToolApprovalChoice::Deny | ToolApprovalChoice::DenyInSession
                );
                if is_rejection {
                    if let Some(item_index) = permission_tool_call_item_index {
                        self.mark_acp_tool_call_rejected_from_runtime(item_index);
                    } else {
                        self.append_tool_result_from_runtime(
                            approval_result_content(choice, &title),
                            approval_result_kind(choice),
                        );
                    }
                }
                Some(AppEffect::RespondAcpPermission {
                    request_id,
                    option_id,
                    is_rejection,
                    rejected_tool_call_id: is_rejection
                        .then_some(permission_tool_call_id)
                        .flatten(),
                })
            }
            ToolApprovalSource::Preview => {
                self.append_tool_result_from_runtime(
                    approval_result_content(choice, &title),
                    approval_result_kind(choice),
                );
                None
            }
        }
    }

    fn cancel_tool_approval_panel(&mut self) -> Option<AppEffect> {
        let source = self.tool_approval_panel.source.clone();
        self.close_tool_approval_panel();

        match source {
            Some(ToolApprovalSource::AcpPermission { request_id, .. }) => {
                Some(AppEffect::RespondAcpPermission {
                    request_id,
                    option_id: None,
                    is_rejection: false,
                    rejected_tool_call_id: None,
                })
            }
            Some(ToolApprovalSource::Preview) | None => None,
        }
    }

    fn restore_suspended_acp_tool_call_for_approval_panel(&mut self) {
        let suspended_item_index = self
            .tool_approval_panel
            .suspended_acp_tool_call_item_index
            .take();
        self.restore_suspended_acp_tool_call_item(suspended_item_index);
    }

    fn restore_suspended_acp_tool_call_item(&mut self, item_index: Option<usize>) {
        if let Some(item_index) = item_index {
            self.set_acp_tool_call_approval_suspended_from_runtime(item_index, false);
        }
    }

    fn clear_acp_tool_call_permission_waiting(&mut self, item_index: Option<usize>) {
        if let Some(item_index) = item_index {
            self.set_acp_tool_call_permission_waiting_from_runtime(item_index, false);
        }
    }
}

fn approval_result_kind(choice: ToolApprovalChoice) -> ToolResultKind {
    match choice {
        ToolApprovalChoice::Allow | ToolApprovalChoice::AllowInSession => ToolResultKind::Ran,
        ToolApprovalChoice::Deny | ToolApprovalChoice::DenyInSession => ToolResultKind::Rejected,
    }
}

fn approval_result_content(choice: ToolApprovalChoice, title: &str) -> String {
    let verb = match approval_result_kind(choice) {
        ToolResultKind::Ran => "Ran",
        ToolResultKind::Rejected => "Reject",
    };
    let title = title.trim();
    if title.is_empty() {
        verb.to_string()
    } else {
        format!("{verb} {title}")
    }
}

fn build_panel_lines(model: &Model, width: usize) -> Vec<Line<'static>> {
    let width = width.max(1);
    if model.tool_approval_panel.preview.is_some() {
        return build_file_preview_panel_lines(model, width);
    }

    let mut lines = vec![
        inline_panel_rule_line(width, model.palette),
        header_line(model),
        Line::raw(""),
    ];
    if !model.tool_approval_panel.title.trim().is_empty() {
        append_command_lines(model, width, &mut lines);
        lines.push(Line::raw(""));
    }
    if !model.tool_approval_panel.details.is_empty() {
        append_detail_lines(model, width, &mut lines);
        lines.push(Line::raw(""));
    }
    append_choice_lines(model, width, &mut lines);
    lines.push(Line::raw(""));
    lines.push(Line::styled(
        " Esc to cancel · Enter to choose",
        tertiary_text_style(model.palette).add_modifier(Modifier::ITALIC),
    ));

    lines
}

fn header_line(model: &Model) -> Line<'static> {
    Line::from(vec![
        Span::raw("  "),
        Span::styled("Tool Approval:", primary_text_style(model.palette)),
    ])
}

fn append_command_lines(model: &Model, width: usize, lines: &mut Vec<Line<'static>>) {
    let command_width = width.saturating_sub(2).max(1);
    let base_style = primary_text_style(model.palette).add_modifier(Modifier::BOLD);
    if let Some(highlighted) =
        highlight_code_chunks(&model.tool_approval_panel.title, "bash", base_style)
    {
        let command_lines = wrap_highlight_chunks(&highlighted, command_width);
        if command_lines.is_empty() {
            lines.push(Line::styled("  ", base_style));
            return;
        }

        for spans in command_lines {
            let mut line_spans = Vec::with_capacity(spans.len() + 1);
            line_spans.push(Span::raw("  "));
            line_spans.extend(spans);
            lines.push(Line::from(line_spans));
        }
        return;
    }

    let command_lines = wrap_inline_text(&model.tool_approval_panel.title, command_width);
    if command_lines.is_empty() {
        lines.push(Line::styled("  ", base_style));
        return;
    }

    for line in command_lines {
        lines.push(Line::styled(format!("  {line}"), base_style));
    }
}

fn append_detail_lines(model: &Model, width: usize, lines: &mut Vec<Line<'static>>) {
    for detail in &model.tool_approval_panel.details {
        append_wrapped_inline_value(
            lines,
            width,
            &format!("• {}: ", detail.label),
            &detail.value,
            tertiary_text_style(model.palette),
            secondary_text_style(model.palette),
        );
    }
}

fn append_choice_lines(model: &Model, _width: usize, lines: &mut Vec<Line<'static>>) {
    for (index, choice) in tool_approval_choices(&model.tool_approval_panel)
        .into_iter()
        .enumerate()
    {
        let selected = index == model.tool_approval_panel.selected;
        lines.push(approval_choice_line(
            model,
            index,
            selected,
            choice.display_label(),
        ));
    }
}

pub(super) fn approval_choice_line(
    model: &Model,
    index: usize,
    selected: bool,
    label: &str,
) -> Line<'static> {
    let marker = if selected { "➜ " } else { "  " };
    let style = if selected {
        primary_text_style(model.palette).bold()
    } else {
        secondary_text_style(model.palette)
    };
    Line::from(vec![
        Span::raw("  "),
        Span::styled(marker, secondary_text_style(model.palette)),
        Span::styled(format!("{}. {label}", index + 1), style),
    ])
}

fn tool_approval_choices(state: &ToolApprovalPanelState) -> Vec<ToolApprovalChoice> {
    match state.source.as_ref() {
        Some(ToolApprovalSource::AcpPermission {
            allow_option_id,
            allow_always_option_id,
            reject_option_id,
            reject_always_option_id,
            ..
        }) => {
            let mut choices = Vec::new();
            if allow_option_id.is_some() {
                choices.push(ToolApprovalChoice::Allow);
            }
            if allow_always_option_id.is_some() {
                choices.push(ToolApprovalChoice::AllowInSession);
            }
            if reject_option_id.is_some() {
                choices.push(ToolApprovalChoice::Deny);
            }
            if reject_always_option_id.is_some() {
                choices.push(ToolApprovalChoice::DenyInSession);
            }
            choices
        }
        Some(ToolApprovalSource::Preview) => vec![
            ToolApprovalChoice::Allow,
            ToolApprovalChoice::AllowInSession,
            ToolApprovalChoice::Deny,
            ToolApprovalChoice::DenyInSession,
        ],
        None => Vec::new(),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ToolApprovalSelectionMove {
    Up,
    Down,
    Left,
    Right,
}

fn move_tool_approval_selection(
    state: &mut ToolApprovalPanelState,
    direction: ToolApprovalSelectionMove,
) {
    let choices = tool_approval_choices(state);
    let choice_count = choices.len();
    if choice_count == 0 {
        state.selected = 0;
        return;
    }

    state.selected = match direction {
        ToolApprovalSelectionMove::Up | ToolApprovalSelectionMove::Left => state
            .selected
            .checked_sub(1)
            .unwrap_or(choice_count.saturating_sub(1)),
        ToolApprovalSelectionMove::Down | ToolApprovalSelectionMove::Right => {
            (state.selected + 1) % choice_count
        }
    };
}

fn preferred_tool_approval_choice(
    state: &ToolApprovalPanelState,
    preferred: &[ToolApprovalChoice],
) -> Option<ToolApprovalChoice> {
    let choices = tool_approval_choices(state);
    preferred
        .iter()
        .copied()
        .find(|preferred_choice| choices.contains(preferred_choice))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        HeroOptions, Sender,
        theme::{default_palette, primary_text_style, secondary_text_style},
    };

    #[test]
    fn preview_layout_omits_labels_and_uses_vertical_numbered_choices() {
        let mut model = Model::new(HeroOptions::default());
        model.palette = default_palette();
        open_preview_panel(&mut model);

        let lines = build_panel_lines(&model, 72)
            .into_iter()
            .map(|line| {
                line.spans
                    .into_iter()
                    .map(|span| span.content.into_owned())
                    .collect::<String>()
            })
            .collect::<Vec<_>>();

        assert!(
            lines
                .iter()
                .all(|line| !line.contains("Preview") && !line.contains("Preview tool request")),
            "preview marker and synthetic preview title should not be rendered: {lines:?}"
        );
        assert!(
            lines
                .iter()
                .all(|line| !line.contains("Tool   :") && !line.contains("Request:")),
            "tool and request labels should not be rendered: {lines:?}"
        );
        let header = lines
            .iter()
            .position(|line| line.contains("Tool Approval:"))
            .expect("header should render");
        let command = lines
            .iter()
            .position(|line| line.contains("sed -n"))
            .expect("command row should render");
        let first_choice = lines
            .iter()
            .position(|line| line.contains("1. Yes"))
            .expect("first approval choice should render");
        assert!(
            header < command && command < first_choice,
            "command should sit between header and choices: {lines:?}"
        );
        assert_eq!(
            lines.get(header + 1).map(String::as_str),
            Some(""),
            "header should keep a blank row before the command: {lines:?}"
        );
        assert_eq!(
            first_choice.saturating_sub(command + 1),
            1,
            "choices should keep one blank row after the command when details are absent: {lines:?}"
        );
        assert!(
            lines.iter().all(|line| !line.contains("Reason")),
            "preview should not synthesize a reason row: {lines:?}"
        );
        assert!(
            lines.iter().all(|line| !line.contains("Actions:")),
            "shell approval should not use the old actions heading: {lines:?}"
        );
        assert!(
            lines.iter().any(|line| line == "  ➜ 1. Yes"),
            "selected choice should match the file-preview marker and numbering style: {lines:?}"
        );
        assert!(
            lines
                .iter()
                .any(|line| line.contains("2. Yes, allow similar requests during this session")),
            "preview should expose the session allow option for design checks: {lines:?}"
        );
        assert!(
            lines
                .iter()
                .any(|line| line.contains("4. No, reject similar requests during this session")),
            "preview should expose the session reject option for design checks: {lines:?}"
        );
        assert!(
            lines
                .iter()
                .any(|line| line.contains("Esc to cancel · Enter to choose")),
            "footer hint should use the concise approval copy: {lines:?}"
        );
    }

    #[test]
    fn command_line_wraps_without_request_label() {
        let mut model = Model::new(HeroOptions::default());
        model.palette = default_palette();
        model.open_tool_approval_panel(
            ToolApprovalSource::Preview,
            "cargo clippy --workspace --all-targets -- -D warnings".to_string(),
            vec![ToolApprovalDetail {
                label: "Reason".to_string(),
                value: "Inspect wrapping".to_string(),
            }],
        );

        let lines = build_panel_lines(&model, 28)
            .into_iter()
            .map(|line| {
                line.spans
                    .into_iter()
                    .map(|span| span.content.into_owned())
                    .collect::<String>()
            })
            .collect::<Vec<_>>();

        assert!(
            lines.iter().all(|line| !line.contains("Request:")),
            "wrapped command should not use a request label: {lines:?}"
        );
        assert!(
            lines
                .iter()
                .filter(|line| line.starts_with("  ") && !line.contains(':'))
                .count()
                > 1
                && lines.iter().any(|line| line.contains("cargo clippy"))
                && lines.iter().any(|line| line.contains("warning")),
            "long command should wrap across multiple display rows: {lines:?}"
        );
    }

    #[test]
    fn long_command_keeps_full_document_flow_without_truncating_choices() {
        let mut model = Model::new(HeroOptions::default());
        model.palette = default_palette();
        model.set_window(24, 8);
        model.open_tool_approval_panel(
            ToolApprovalSource::Preview,
            "cargo run --bin lumos -- --very-long-debug-command-that-wraps".to_string(),
            Vec::new(),
        );

        let panel = model.current_inline_tool_approval_panel_render_result();
        let text = panel.plain_lines.join("\n");

        assert!(
            panel.plain_lines.len() > usize::from(model.height),
            "long wrapped command should remain in document flow for viewport scrolling"
        );
        assert!(
            text.contains("1. Yes") && text.contains("Esc to cancel · Enter to choose"),
            "choices and footer should not be truncated away: {text:?}"
        );
    }

    #[test]
    fn acp_session_allow_option_only_renders_when_available() {
        let mut model = Model::new(HeroOptions::default());
        model.palette = default_palette();
        model.open_tool_approval_panel(
            ToolApprovalSource::AcpPermission {
                request_id: "permission-1".to_string(),
                allow_option_id: Some("allow-once".to_string()),
                allow_always_option_id: None,
                reject_option_id: Some("reject-once".to_string()),
                reject_always_option_id: None,
            },
            "touch src/main.rs".to_string(),
            vec![ToolApprovalDetail {
                label: "Reason".to_string(),
                value: "Inspect actions".to_string(),
            }],
        );

        let without_session = build_panel_lines(&model, 72)
            .into_iter()
            .map(|line| {
                line.spans
                    .into_iter()
                    .map(|span| span.content.into_owned())
                    .collect::<String>()
            })
            .collect::<Vec<_>>();
        assert!(
            without_session
                .iter()
                .all(|line| !line.contains("allow similar requests")),
            "session allow should not render without an upstream option: {without_session:?}"
        );

        model.open_tool_approval_panel(
            ToolApprovalSource::AcpPermission {
                request_id: "permission-2".to_string(),
                allow_option_id: Some("allow-once".to_string()),
                allow_always_option_id: Some("allow-always".to_string()),
                reject_option_id: Some("reject-once".to_string()),
                reject_always_option_id: Some("reject-always".to_string()),
            },
            "touch src/main.rs".to_string(),
            vec![ToolApprovalDetail {
                label: "Reason".to_string(),
                value: "Inspect actions".to_string(),
            }],
        );

        let with_session = build_panel_lines(&model, 72)
            .into_iter()
            .map(|line| {
                line.spans
                    .into_iter()
                    .map(|span| span.content.into_owned())
                    .collect::<String>()
            })
            .collect::<Vec<_>>();
        assert_ordered_plain_lines(
            &with_session,
            &[
                "1. Yes",
                "2. Yes, allow similar requests during this session",
                "3. No",
                "4. No, reject similar requests during this session",
            ],
        );
    }

    #[test]
    fn choices_render_vertically_like_file_preview_panel() {
        let mut model = Model::new(HeroOptions::default());
        model.palette = default_palette();
        open_preview_panel(&mut model);

        let lines = build_panel_lines(&model, 72)
            .into_iter()
            .map(|line| {
                line.spans
                    .into_iter()
                    .map(|span| span.content.into_owned())
                    .collect::<String>()
            })
            .collect::<Vec<_>>();

        assert_ordered_plain_lines(
            &lines,
            &[
                "  ➜ 1. Yes",
                "    2. Yes, allow similar requests during this session",
                "    3. No",
                "    4. No, reject similar requests during this session",
            ],
        );
        assert!(
            lines.iter().all(|line| {
                let combines_allow_choices = line.contains("1. Yes") && line.contains("2.");
                let combines_deny_choices = line.contains("3. No") && line.contains("4.");
                !(combines_allow_choices || combines_deny_choices)
            }),
            "each approval choice should occupy its own line: {lines:?}"
        );
    }

    #[test]
    fn preview_choice_closes_without_status_notice_and_appends_result() {
        let mut model = Model::new(HeroOptions::default());
        model.palette = default_palette();
        open_preview_panel(&mut model);

        let effect = model
            .handle_tool_approval_panel_key(KeyCode::Enter.into())
            .expect("tool approval panel should handle Enter");

        assert!(effect.is_none());
        assert!(!model.tool_approval_panel_active());
        assert!(
            model.current_status_notice_text().is_empty(),
            "preview approval should close silently instead of showing a status notice"
        );
        assert!(
            model
                .transcript_mut()
                .plain_items()
                .iter()
                .any(|item| item == "● Ran sed -n '1,80p' src/main.rs"),
            "preview approval should append a testable tool result to transcript"
        );
    }

    #[test]
    fn acp_allow_choice_does_not_append_redundant_ran_result() {
        let mut model = Model::new(HeroOptions::default());
        model.palette = default_palette();
        model.open_tool_approval_panel(
            ToolApprovalSource::AcpPermission {
                request_id: "permission-ran".to_string(),
                allow_option_id: Some("allow-once".to_string()),
                allow_always_option_id: None,
                reject_option_id: Some("reject-once".to_string()),
                reject_always_option_id: None,
            },
            "cargo test tool_approval".to_string(),
            Vec::new(),
        );
        let before = model.transcript_mut().plain_items();

        let effect = model
            .handle_tool_approval_panel_key(KeyCode::Enter.into())
            .expect("tool approval panel should handle Enter");

        assert_eq!(
            effect,
            Some(AppEffect::RespondAcpPermission {
                request_id: "permission-ran".to_string(),
                option_id: Some("allow-once".to_string()),
                is_rejection: false,
                rejected_tool_call_id: None,
            })
        );
        assert!(
            model.transcript_mut().plain_items() == before,
            "ACP allow should not append a redundant approval result when the tool call item will already show execution"
        );
        assert_eq!(
            model.transcript_mut().source_messages(),
            Vec::<(Sender, String)>::new(),
            "tool approval results should not be sent back to the model"
        );
    }

    #[test]
    fn esc_cancels_acp_permission_without_rejecting() {
        let mut model = Model::new(HeroOptions::default());
        model.palette = default_palette();
        model.open_tool_approval_panel(
            ToolApprovalSource::AcpPermission {
                request_id: "permission-cancel".to_string(),
                allow_option_id: Some("allow-once".to_string()),
                allow_always_option_id: None,
                reject_option_id: Some("reject-once".to_string()),
                reject_always_option_id: Some("reject-always".to_string()),
            },
            "cargo check".to_string(),
            Vec::new(),
        );
        let before = model.transcript_mut().plain_items();

        let effect = model
            .handle_tool_approval_panel_key(KeyCode::Esc.into())
            .expect("tool approval panel should handle Esc");

        assert_eq!(
            effect,
            Some(AppEffect::RespondAcpPermission {
                request_id: "permission-cancel".to_string(),
                option_id: None,
                is_rejection: false,
                rejected_tool_call_id: None,
            })
        );
        assert!(!model.tool_approval_panel_active());
        assert_eq!(
            model.transcript_mut().plain_items(),
            before,
            "Esc is cancellation, so it must not append a reject result"
        );
    }

    #[test]
    fn file_preview_panel_renders_numbered_content_without_transport_json() {
        let mut model = Model::new(HeroOptions::default());
        model.palette = default_palette();
        model.open_tool_approval_panel_with_preview(
            ToolApprovalSource::AcpPermission {
                request_id: "permission-write".to_string(),
                allow_option_id: Some("allow-once".to_string()),
                allow_always_option_id: Some("allow-always".to_string()),
                reject_option_id: Some("reject-once".to_string()),
                reject_always_option_id: None,
            },
            "WriteFile: TEMP.md".to_string(),
            Vec::new(),
            Some(ToolApprovalPreview::create_file(
                "TEMP.md".to_string(),
                "# 临时文档\n\nbody\n  indented".to_string(),
            )),
        );

        let lines = build_panel_lines(&model, 72)
            .into_iter()
            .map(|line| {
                line.spans
                    .into_iter()
                    .map(|span| span.content.into_owned())
                    .collect::<String>()
            })
            .collect::<Vec<_>>();
        let text = lines.join("\n");

        assert!(
            !text.contains("Create file") && !text.contains("Edit file"),
            "file preview should keep the header to the file path only: {lines:?}"
        );
        assert!(
            text.contains("TEMP.md"),
            "preview path should render: {lines:?}"
        );
        assert!(
            lines.iter().any(|line| line == "      1  # 临时文档")
                && lines.iter().any(|line| line == "      2  ")
                && lines.iter().any(|line| line == "      3  body")
                && lines.iter().any(|line| line == "      4    indented"),
            "file preview should render numbered file content: {lines:?}"
        );
        assert!(
            !text.contains("\"path\"") && !text.contains("\"content\""),
            "file preview should not expose raw transport JSON: {lines:?}"
        );
        assert!(
            text.contains("Yes") && text.contains("Yes, allow all edits during this session"),
            "file preview should use user-facing approval labels: {lines:?}"
        );
    }

    #[test]
    fn file_preview_panel_choices_use_model_panel_selection_style() {
        let mut model = Model::new(HeroOptions::default());
        model.palette = default_palette();
        model.open_tool_approval_panel_with_preview(
            ToolApprovalSource::AcpPermission {
                request_id: "permission-write".to_string(),
                allow_option_id: Some("allow-once".to_string()),
                allow_always_option_id: Some("allow-always".to_string()),
                reject_option_id: Some("reject-once".to_string()),
                reject_always_option_id: None,
            },
            "WriteFile: TEMP.md".to_string(),
            Vec::new(),
            Some(ToolApprovalPreview::create_file(
                "TEMP.md".to_string(),
                "body".to_string(),
            )),
        );

        let selected_line = build_panel_lines(&model, 72)
            .into_iter()
            .find(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
                    .contains("1. Yes")
            })
            .expect("selected file preview choice should render");
        let plain = selected_line
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>();

        assert_eq!(plain, "  ➜ 1. Yes");
        assert_eq!(selected_line.spans[1].content.as_ref(), "➜ ");
        assert_eq!(
            selected_line.spans[1].style,
            secondary_text_style(model.palette)
        );
        assert_eq!(
            selected_line.spans[2].style,
            primary_text_style(model.palette).bold()
        );
    }

    #[test]
    fn file_preview_panel_hides_status_notice() {
        let mut model = Model::new(HeroOptions::default());
        model.palette = default_palette();
        model.open_tool_approval_panel_with_preview(
            ToolApprovalSource::AcpPermission {
                request_id: "permission-write".to_string(),
                allow_option_id: Some("allow-once".to_string()),
                allow_always_option_id: Some("allow-always".to_string()),
                reject_option_id: Some("reject-once".to_string()),
                reject_always_option_id: None,
            },
            "WriteFile: TEMP.md".to_string(),
            Vec::new(),
            Some(ToolApprovalPreview::create_file(
                "TEMP.md".to_string(),
                "body".to_string(),
            )),
        );
        model.show_transient_status_notice("Selection copied");

        assert!(
            !model.current_status_line_render_result().has_content,
            "file preview approval should suppress status notices while waiting for a choice"
        );
    }

    #[test]
    fn file_preview_panel_selection_moves_linearly_for_vertical_choices() {
        let mut model = Model::new(HeroOptions::default());
        model.palette = default_palette();
        model.open_tool_approval_panel_with_preview(
            ToolApprovalSource::AcpPermission {
                request_id: "permission-write".to_string(),
                allow_option_id: Some("allow-once".to_string()),
                allow_always_option_id: Some("allow-always".to_string()),
                reject_option_id: Some("reject-once".to_string()),
                reject_always_option_id: None,
            },
            "WriteFile: TEMP.md".to_string(),
            Vec::new(),
            Some(ToolApprovalPreview::create_file(
                "TEMP.md".to_string(),
                "body".to_string(),
            )),
        );

        assert_eq!(model.tool_approval_panel.selected, 0);
        model.handle_tool_approval_panel_key(KeyEvent::from(KeyCode::Down));
        assert_eq!(
            model.tool_approval_panel.selected, 1,
            "vertical preview choices should move from Yes to session allow with Down"
        );
        model.handle_tool_approval_panel_key(KeyEvent::from(KeyCode::Down));
        assert_eq!(
            model.tool_approval_panel.selected, 2,
            "vertical preview choices should then move to No"
        );
        model.handle_tool_approval_panel_key(KeyEvent::from(KeyCode::Up));
        assert_eq!(model.tool_approval_panel.selected, 1);
    }

    #[test]
    fn arrow_keys_move_linearly_between_vertical_choices() {
        let mut model = Model::new(HeroOptions::default());
        model.palette = default_palette();
        model.open_tool_approval_panel(
            ToolApprovalSource::AcpPermission {
                request_id: "permission-3".to_string(),
                allow_option_id: Some("allow-once".to_string()),
                allow_always_option_id: Some("allow-always".to_string()),
                reject_option_id: Some("reject-once".to_string()),
                reject_always_option_id: Some("reject-always".to_string()),
            },
            "touch src/main.rs".to_string(),
            Vec::new(),
        );

        model.handle_tool_approval_panel_key(KeyCode::Down.into());
        assert_eq!(
            selected_tool_approval_choice(&model),
            Some(ToolApprovalChoice::AllowInSession)
        );

        model.handle_tool_approval_panel_key(KeyCode::Down.into());
        assert_eq!(
            selected_tool_approval_choice(&model),
            Some(ToolApprovalChoice::Deny)
        );

        model.handle_tool_approval_panel_key(KeyCode::Right.into());
        assert_eq!(
            selected_tool_approval_choice(&model),
            Some(ToolApprovalChoice::DenyInSession)
        );

        model.handle_tool_approval_panel_key(KeyCode::Up.into());
        assert_eq!(
            selected_tool_approval_choice(&model),
            Some(ToolApprovalChoice::Deny)
        );
    }

    fn selected_tool_approval_choice(model: &Model) -> Option<ToolApprovalChoice> {
        tool_approval_choices(&model.tool_approval_panel)
            .get(model.tool_approval_panel.selected)
            .copied()
    }

    fn open_preview_panel(model: &mut Model) {
        model.open_tool_approval_panel(
            ToolApprovalSource::Preview,
            "sed -n '1,80p' src/main.rs".to_string(),
            Vec::new(),
        );
    }

    #[test]
    fn shell_command_lines_use_highlighted_styles() {
        let mut model = Model::new(HeroOptions::default());
        model.palette = default_palette();
        open_preview_panel(&mut model);

        let command_line = build_panel_lines(&model, 72)
            .into_iter()
            .find(|line| {
                let text = line
                    .spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>();
                text.contains("sed -n")
            })
            .expect("command line should render");
        let foregrounds = command_line
            .spans
            .iter()
            .filter_map(|span| span.style.fg)
            .fold(Vec::new(), |mut colors, color| {
                if !colors.contains(&color) {
                    colors.push(color);
                }
                colors
            });

        assert!(
            foregrounds.len() > 1,
            "shell command should have syntax-highlighted spans, got: {command_line:?}"
        );
    }

    fn assert_ordered_plain_lines(lines: &[String], needles: &[&str]) {
        let mut last_index = None;
        for needle in needles {
            let index = lines
                .iter()
                .position(|line| line.contains(needle))
                .unwrap_or_else(|| panic!("expected {needle:?} in {lines:?}"));
            if let Some(last_index) = last_index {
                assert!(
                    index >= last_index,
                    "expected {needle:?} after previous item in {lines:?}"
                );
            }
            last_index = Some(index);
        }
    }
}
