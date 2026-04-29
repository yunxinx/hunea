use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    style::Modifier,
    text::{Line, Span},
};
use unicode_width::UnicodeWidthStr;

use super::{
    AppEffect, Model,
    inline_panel::{
        InlinePanelRenderResult, append_wrapped_inline_value, inline_panel_render_result,
        inline_panel_rule_line, wrap_inline_text,
    },
    theme::{
        command_accent_text_style, primary_text_style, secondary_text_style, tertiary_text_style,
    },
    tool_result::ToolResultKind,
    transcript::markdown_highlight::{highlight_code_chunks, wrap_highlight_chunks},
};

const ACTION_COLUMN_GAP: usize = 4;
const ACTION_LEFT_LABEL_WIDTH: usize = 5;

/// `ToolApprovalPanelState` 保存通用工具审批面板的展示与导航状态。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(super) struct ToolApprovalPanelState {
    pub(super) is_open: bool,
    pub(super) selected: usize,
    pub(super) source: Option<ToolApprovalSource>,
    pub(super) title: String,
    pub(super) details: Vec<ToolApprovalDetail>,
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
    fn label(self) -> &'static str {
        match self {
            Self::Allow => "Allow",
            Self::AllowInSession => "Allow in session",
            Self::Deny => "Reject",
            Self::DenyInSession => "Reject in session",
        }
    }

    fn display_label(self) -> &'static str {
        self.label()
    }

    fn position(self) -> ToolApprovalChoicePosition {
        match self {
            Self::Allow => ToolApprovalChoicePosition { row: 0, column: 0 },
            Self::AllowInSession => ToolApprovalChoicePosition { row: 0, column: 1 },
            Self::Deny => ToolApprovalChoicePosition { row: 1, column: 0 },
            Self::DenyInSession => ToolApprovalChoicePosition { row: 1, column: 1 },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ToolApprovalChoicePosition {
    row: usize,
    column: usize,
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
        self.model_panel.is_open = false;
        self.acp_panel.is_open = false;
        self.tool_approval_panel = ToolApprovalPanelState {
            is_open: true,
            selected: 0,
            source: Some(source),
            title,
            details,
        };
        self.tool_approval_panel_revision = self.tool_approval_panel_revision.saturating_add(1);
        self.sync_command_panel_navigation();
        self.sync_composer_height();
        self.sync_document_viewport_for_composer_cursor();
    }

    pub(crate) fn close_tool_approval_panel(&mut self) {
        if !self.tool_approval_panel.is_open {
            return;
        }

        self.tool_approval_panel = ToolApprovalPanelState::default();
        self.pending_acp_permission = None;
        self.tool_approval_panel_revision = self.tool_approval_panel_revision.saturating_add(1);
        self.sync_composer_height();
        self.sync_document_viewport_for_composer_cursor();
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
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => Some(
                self.resolve_tool_approval_choice(
                    preferred_tool_approval_choice(
                        &self.tool_approval_panel,
                        &[ToolApprovalChoice::Deny, ToolApprovalChoice::DenyInSession],
                    )
                    .unwrap_or(ToolApprovalChoice::Deny),
                ),
            ),
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
        self.pending_acp_permission = None;
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
                self.append_tool_result_from_runtime(
                    approval_result_content(choice, &title),
                    approval_result_kind(choice),
                );
                Some(AppEffect::RespondAcpPermission {
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
    lines.push(Line::styled(
        "  Actions:",
        secondary_text_style(model.palette).bold(),
    ));
    append_choice_lines(model, width, &mut lines);
    lines.push(Line::raw(""));
    lines.push(Line::styled(
        "  Press Enter to choose · Y allow · ESC/N reject · ↑↓←→ to navigate",
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
    for row in 0..=1 {
        let left = tool_approval_choice_at_position(&model.tool_approval_panel, row, 0);
        let right = tool_approval_choice_at_position(&model.tool_approval_panel, row, 1);
        if left.is_none() && right.is_none() {
            continue;
        }

        let mut spans = vec![Span::raw("  ")];
        append_choice_cell(model, &mut spans, left, ACTION_LEFT_LABEL_WIDTH);
        spans.push(Span::raw(" ".repeat(ACTION_COLUMN_GAP)));
        append_choice_cell(model, &mut spans, right, 0);
        lines.push(Line::from(spans));
    }
}

fn append_choice_cell(
    model: &Model,
    spans: &mut Vec<Span<'static>>,
    choice: Option<ToolApprovalChoice>,
    label_width: usize,
) {
    let Some(choice) = choice else {
        spans.push(Span::raw(" ".repeat(2 + label_width)));
        return;
    };

    let choices = tool_approval_choices(&model.tool_approval_panel);
    let selected = choices
        .get(model.tool_approval_panel.selected)
        .is_some_and(|selected_choice| *selected_choice == choice);
    let marker = if selected { "➜ " } else { "  " };
    let style = if selected {
        command_accent_text_style(model.palette).bold()
    } else {
        secondary_text_style(model.palette)
    };
    let mut label = choice.display_label().to_string();
    if label_width > label.width() {
        label.push_str(&" ".repeat(label_width - label.width()));
    }

    spans.push(Span::styled(
        marker.to_string(),
        secondary_text_style(model.palette),
    ));
    spans.push(Span::styled(label, style));
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
    let Some(current_choice) = choices.get(state.selected).copied() else {
        state.selected = 0;
        return;
    };

    let next_choice = match direction {
        ToolApprovalSelectionMove::Left => {
            let position = current_choice.position();
            if position.column == 0 {
                None
            } else {
                tool_approval_choice_at_position(state, position.row, 0)
            }
        }
        ToolApprovalSelectionMove::Right => {
            let position = current_choice.position();
            tool_approval_choice_at_position(state, position.row, position.column + 1)
        }
        ToolApprovalSelectionMove::Up | ToolApprovalSelectionMove::Down => {
            move_tool_approval_selection_vertically(state, current_choice, direction)
        }
    };

    if let Some(next_choice) = next_choice
        && let Some(index) = choices.iter().position(|choice| *choice == next_choice)
    {
        state.selected = index;
    }
}

fn move_tool_approval_selection_vertically(
    state: &ToolApprovalPanelState,
    current_choice: ToolApprovalChoice,
    direction: ToolApprovalSelectionMove,
) -> Option<ToolApprovalChoice> {
    let rows = tool_approval_choice_rows(state);
    let position = current_choice.position();
    let row_index = rows.iter().position(|row| *row == position.row)?;
    let next_row_index = match direction {
        ToolApprovalSelectionMove::Up => row_index.checked_sub(1).unwrap_or(rows.len() - 1),
        ToolApprovalSelectionMove::Down => (row_index + 1) % rows.len(),
        ToolApprovalSelectionMove::Left | ToolApprovalSelectionMove::Right => row_index,
    };
    let next_row = rows[next_row_index];
    tool_approval_choice_at_position(state, next_row, position.column)
        .or_else(|| tool_approval_choice_at_position(state, next_row, 0))
        .or_else(|| tool_approval_choice_at_position(state, next_row, 1))
}

fn tool_approval_choice_rows(state: &ToolApprovalPanelState) -> Vec<usize> {
    let mut rows = Vec::new();
    for choice in tool_approval_choices(state) {
        let row = choice.position().row;
        if !rows.contains(&row) {
            rows.push(row);
        }
    }
    rows
}

fn tool_approval_choice_at_position(
    state: &ToolApprovalPanelState,
    row: usize,
    column: usize,
) -> Option<ToolApprovalChoice> {
    tool_approval_choices(state)
        .into_iter()
        .find(|choice| choice.position() == ToolApprovalChoicePosition { row, column })
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
    use crate::frontend::tui::{HeroOptions, Sender, theme::default_palette};

    #[test]
    fn preview_layout_omits_labels_and_places_wrapped_command_before_actions() {
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
        let actions = lines
            .iter()
            .position(|line| line.contains("Actions:"))
            .expect("actions row should render");
        assert!(
            header < command && command < actions,
            "command should sit between header and actions: {lines:?}"
        );
        assert_eq!(
            lines.get(header + 1).map(String::as_str),
            Some(""),
            "header should keep a blank row before the command: {lines:?}"
        );
        assert_eq!(
            actions.saturating_sub(command + 1),
            1,
            "actions should keep one blank row after the command when details are absent: {lines:?}"
        );
        assert!(
            lines.iter().all(|line| !line.contains("Reason")),
            "preview should not synthesize a reason row: {lines:?}"
        );
        assert!(
            lines.iter().any(|line| line.contains("Allow in session")),
            "preview should expose the session allow option for design checks: {lines:?}"
        );
        assert!(
            lines.iter().any(|line| line.contains("Reject in session")),
            "preview should expose the session reject option for design checks: {lines:?}"
        );
        assert!(
            lines.iter().any(|line| {
                line.contains("Press Enter to choose · Y allow · ESC/N reject · ↑↓←→ to navigate")
            }),
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
    fn long_command_keeps_full_document_flow_without_truncating_actions() {
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
            text.contains("Actions:") && text.contains("Press Enter to choose"),
            "actions and footer should not be truncated away: {text:?}"
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
                .all(|line| !line.contains("Allow in session")),
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
            &["Allow", "Allow in session", "Reject", "Reject in session"],
        );
    }

    #[test]
    fn choices_render_session_options_in_right_column() {
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

        let allow_row = lines
            .iter()
            .find(|line| line.contains("Allow") && line.contains("Allow in session"))
            .expect("allow choices should share one display row");
        let deny_row = lines
            .iter()
            .find(|line| line.contains("Reject") && line.contains("Reject in session"))
            .expect("reject choices should share one display row");

        let allow_gap = allow_row
            .find("Allow in session")
            .expect("session allow should render")
            .saturating_sub(allow_row.find("Allow").expect("allow should render") + "Allow".len());
        let deny_gap = deny_row
            .find("Reject in session")
            .expect("session reject should render")
            .saturating_sub(
                deny_row.find("Reject").expect("reject should render") + "Reject".len(),
            );

        assert!(
            allow_gap >= 4 && deny_gap >= 4,
            "session choices should sit to the right with a visible gap: {lines:?}"
        );
        assert!(
            deny_row.contains("Reject in session"),
            "reject session label should render with consistent spacing: {lines:?}"
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
    fn acp_allow_choice_appends_ran_result_without_source_message() {
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

        let effect = model
            .handle_tool_approval_panel_key(KeyCode::Enter.into())
            .expect("tool approval panel should handle Enter");

        assert_eq!(
            effect,
            Some(AppEffect::RespondAcpPermission {
                request_id: "permission-ran".to_string(),
                option_id: Some("allow-once".to_string()),
            })
        );
        assert!(
            model
                .transcript_mut()
                .plain_items()
                .iter()
                .any(|item| item == "● Ran cargo test tool_approval"),
            "approval result should be appended to transcript"
        );
        assert_eq!(
            model.transcript_mut().source_messages(),
            Vec::<(Sender, String)>::new(),
            "tool approval results should not be sent back to the model"
        );
    }

    #[test]
    fn arrow_keys_move_between_choice_rows_and_columns() {
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

        model.handle_tool_approval_panel_key(KeyCode::Right.into());
        assert_eq!(
            selected_tool_approval_choice(&model),
            Some(ToolApprovalChoice::AllowInSession)
        );

        model.handle_tool_approval_panel_key(KeyCode::Down.into());
        assert_eq!(
            selected_tool_approval_choice(&model),
            Some(ToolApprovalChoice::DenyInSession)
        );

        model.handle_tool_approval_panel_key(KeyCode::Left.into());
        assert_eq!(
            selected_tool_approval_choice(&model),
            Some(ToolApprovalChoice::Deny)
        );

        model.handle_tool_approval_panel_key(KeyCode::Up.into());
        assert_eq!(
            selected_tool_approval_choice(&model),
            Some(ToolApprovalChoice::Allow)
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
