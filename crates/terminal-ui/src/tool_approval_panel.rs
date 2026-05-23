use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    style::Modifier,
    text::{Line, Span},
};

mod file_preview;

use super::{
    AppEffect, Model,
    inline_panel::{
        InlinePanelRenderResult, append_wrapped_inline_value, inline_panel_render_result,
        inline_panel_rule_line, wrap_inline_text,
    },
    runtime_tool_preview::ToolApprovalPreview,
    theme::{primary_text_style, secondary_text_style, tertiary_text_style},
    tool_result::ToolResultKind,
    transcript::markdown_highlight::{highlight_code_chunks, wrap_highlight_chunks},
};
use file_preview::build_file_preview_panel_lines;
use runtime_domain::session::RuntimeTarget;

/// `ToolApprovalPanelState` 保存通用工具审批面板的展示与导航状态。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(super) struct ToolApprovalPanelState {
    pub(super) is_open: bool,
    pub(super) selected: usize,
    pub(super) source: Option<ToolApprovalSource>,
    pub(super) title: String,
    pub(super) details: Vec<ToolApprovalDetail>,
    pub(super) preview: Option<ToolApprovalPreview>,
}

/// `ToolApprovalSource` 描述工具审批确认后需要回到哪个运行时来源。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ToolApprovalSource {
    RuntimePermission {
        target: RuntimeTarget,
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
        self.close_transcript_overlay();
        self.pause_stream_activity();
        self.model_panel.is_open = false;
        self.tool_approval_panel = ToolApprovalPanelState {
            is_open: true,
            selected: 0,
            source: Some(source),
            title,
            details,
            preview,
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

        self.tool_approval_panel = ToolApprovalPanelState::default();
        self.resume_stream_activity();
        self.tool_approval_panel_revision = self.tool_approval_panel_revision.saturating_add(1);
        self.sync_composer_height();
        self.sync_document_viewport_for_composer_cursor();
    }

    pub(crate) fn close_runtime_permission_approval_panel(&mut self) {
        if matches!(
            self.tool_approval_panel.source,
            Some(ToolApprovalSource::RuntimePermission { .. })
        ) {
            self.close_tool_approval_panel();
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
        let source = self.tool_approval_panel.source.clone()?;
        let title = self.tool_approval_panel.title.clone();
        self.tool_approval_panel = ToolApprovalPanelState::default();
        self.resume_stream_activity();
        self.tool_approval_panel_revision = self.tool_approval_panel_revision.saturating_add(1);
        self.sync_composer_height();
        self.sync_document_viewport_for_composer_cursor();

        match source {
            ToolApprovalSource::RuntimePermission {
                target,
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
                Some(AppEffect::RespondRuntimePermission {
                    target,
                    request_id,
                    option_id,
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
            Some(ToolApprovalSource::RuntimePermission {
                target, request_id, ..
            }) => Some(AppEffect::RespondRuntimePermission {
                target,
                request_id,
                option_id: None,
            }),
            Some(ToolApprovalSource::Preview) | None => None,
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
        Some(ToolApprovalSource::RuntimePermission {
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
mod tests;
