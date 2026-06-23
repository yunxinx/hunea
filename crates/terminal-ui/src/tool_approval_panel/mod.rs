//! 工具审批面板状态与渲染装配。

use ratatui::{
    style::Modifier,
    text::{Line, Span},
};

mod file_preview;
mod input;

use super::{
    Model,
    inline_panel::{
        InlinePanelRenderResult, append_wrapped_inline_value, inline_panel_render_result,
        inline_panel_rule_line, wrap_inline_text,
    },
    runtime::tool_activity_preview::ToolApprovalPreview,
    theme::{primary_text_style, secondary_text_style, tertiary_text_style},
    transcript::markdown_highlight::{highlight_code_chunks, wrap_highlight_chunks},
};
use file_preview::{
    FilePreviewRenderCache, build_file_preview_panel_lines, file_preview_fullscreen_max_offset,
};
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
    pub(super) preview_is_fullscreen: bool,
    pub(super) preview_scroll_offset: usize,
    pub(super) preview_render_cache: Option<FilePreviewRenderCache>,
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
}

impl Model {
    pub(crate) fn tool_approval_panel_active(&self) -> bool {
        self.tool_approval_panel.is_open
    }

    pub(crate) fn tool_approval_fullscreen_preview_active(&self) -> bool {
        self.tool_approval_panel.is_open
            && self.tool_approval_panel.preview.is_some()
            && self.tool_approval_panel.preview_is_fullscreen
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
        self.close_fullscreen_modal_layers();
        self.pause_stream_activity();
        self.close_model_panel();
        self.tool_approval_panel = ToolApprovalPanelState {
            is_open: true,
            selected: 0,
            source: Some(source),
            title,
            details,
            preview,
            preview_is_fullscreen: false,
            preview_scroll_offset: 0,
            preview_render_cache: None,
        };
        self.sync_tool_approval_preview_mode();
        self.tool_approval_panel_revision = self.tool_approval_panel_revision.saturating_add(1);
        self.close_composer_attached_ui();
        self.sync_composer_height();
        self.sync_document_viewport_for_composer_cursor();
    }

    pub(crate) fn close_tool_approval_panel(&mut self) {
        if !self.tool_approval_panel.is_open {
            return;
        }

        self.tool_approval_panel = ToolApprovalPanelState::default();
        self.clear_runtime_tool_activity_approval_suspensions_from_runtime();
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

    pub(crate) fn current_inline_tool_approval_panel_render_result(
        &self,
    ) -> ToolApprovalPanelRenderResult {
        if !self.tool_approval_panel_active() {
            return ToolApprovalPanelRenderResult::default();
        }
        if self.tool_approval_fullscreen_preview_active() {
            return ToolApprovalPanelRenderResult::default();
        }

        let width = usize::from(self.width.max(1));
        let lines = build_panel_lines(self, width);
        inline_panel_render_result(lines)
    }

    pub(crate) fn sync_tool_approval_preview_mode(&mut self) {
        if !self.tool_approval_panel.is_open || self.tool_approval_panel.preview.is_none() {
            self.tool_approval_panel.preview_is_fullscreen = false;
            self.tool_approval_panel.preview_scroll_offset = 0;
            self.tool_approval_panel.preview_render_cache = None;
            return;
        }
        if !self.has_window {
            self.tool_approval_panel.preview_is_fullscreen = false;
            self.tool_approval_panel.preview_scroll_offset = 0;
            return;
        }

        let width = usize::from(self.width.max(1));
        let height = usize::from(self.height.max(1));
        let panel_line_count = build_file_preview_panel_lines(self, width).len();
        self.tool_approval_panel.preview_is_fullscreen = panel_line_count > height;
        if self.tool_approval_panel.preview_is_fullscreen {
            self.complete_startup_banner_entrance();
        }
        self.clamp_tool_approval_fullscreen_preview_scroll();
    }

    pub(crate) fn scroll_tool_approval_fullscreen_preview_by(&mut self, delta_lines: isize) {
        if !self.tool_approval_fullscreen_preview_active() {
            return;
        }

        let max_offset = file_preview_fullscreen_max_offset(self);
        let current = self
            .tool_approval_panel
            .preview_scroll_offset
            .min(max_offset);
        let next = if delta_lines.is_negative() {
            current.saturating_sub(delta_lines.unsigned_abs())
        } else {
            current.saturating_add(delta_lines as usize).min(max_offset)
        };
        self.tool_approval_panel.preview_scroll_offset = next;
    }

    fn clamp_tool_approval_fullscreen_preview_scroll(&mut self) {
        if !self.tool_approval_panel.preview_is_fullscreen {
            self.tool_approval_panel.preview_scroll_offset = 0;
            return;
        }
        let max_offset = file_preview_fullscreen_max_offset(self);
        self.tool_approval_panel.preview_scroll_offset = self
            .tool_approval_panel
            .preview_scroll_offset
            .min(max_offset);
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

#[cfg(test)]
mod tests;
