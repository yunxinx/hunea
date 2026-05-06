use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
    time::{Duration, Instant},
};

use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};
use unicode_width::UnicodeWidthStr;

mod acp;

use super::transcript::markdown_highlight::HighlightChunk;
use super::{
    acp_tool_preview::is_acp_write_tool_call,
    styled_text::{line_to_plain_text, lines_to_ansi_text, lines_to_plain_text},
    theme::{TerminalPalette, secondary_text_style},
    transcript::{
        ItemLineAnchor, TranscriptEstimateKind, TranscriptFastEstimate, TranscriptItemMetrics,
        markdown_highlight::{highlight_code_chunks, wrap_highlight_chunks},
        wrap_prompt_visual_lines,
    },
};
use crate::runtime::acp::{AcpToolCall, AcpToolCallContent, AcpToolCallStatus, AcpToolCallUpdate};
#[cfg(test)]
use crate::runtime::acp::{AcpToolCallLocation, AcpToolKind};
use acp::{
    AcpDiffDetailLine, AcpToolCallDetailBlock, acp_diff_line_prefix,
    acp_read_tool_call_title_chunks, acp_tool_call_content_byte_len, acp_tool_call_detail_blocks,
    acp_tool_call_diff_header_chunks, acp_tool_call_diff_line_style, acp_tool_call_diff_row_style,
    acp_tool_call_display_title, acp_tool_call_has_diff_content, acp_tool_call_location_suffix,
    acp_tool_call_status_color, acp_write_tool_call_title_chunks, active_marker_visible_at,
    is_acp_read_tool_call, style_for_color,
};

const TOOL_RESULT_PREFIX: &str = "● ";
const TOOL_RESULT_CONTINUATION_PREFIX: &str = "  ";
const TOOL_ACTIVITY_DETAIL_PREFIX: &str = "  └─ ";
const TOOL_ACTIVITY_DETAIL_CONTINUATION_PREFIX: &str = "    ";
pub(crate) const TOOL_ACTIVITY_LINE_NUMBER_WIDTH: usize = 7;
pub(crate) const TOOL_ACTIVITY_ACTIVE_MARKER_BLINK_INTERVAL: Duration = Duration::from_millis(600);
const TOOL_ACTIVITY_DIFF_LINE_NUMBER_WIDTH: usize = TOOL_ACTIVITY_LINE_NUMBER_WIDTH;
const TOOL_ACTIVITY_COMPACT_EDGE_LINES: usize = 5;
const TOOL_ACTIVITY_TRANSCRIPT_HINT: &str = "ctrl + t to view transcript";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum ToolResultKind {
    Ran,
    Rejected,
}

/// `ToolActivityRenderMode` 控制工具活动在主界面与 transcript overlay 中的详略。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum ToolActivityRenderMode {
    Compact,
    Detailed,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum ToolResultBody {
    Approval {
        content: String,
        kind: ToolResultKind,
    },
    AcpToolCall(AcpToolCall),
}

/// `ToolResultItem` 表示只用于 TUI 展示的工具活动，不参与模型上下文。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ToolResultItem {
    body: ToolResultBody,
    render_mode: ToolActivityRenderMode,
    render_cache_key: u64,
    active_marker_started_at: Option<Instant>,
    approval_suspended: bool,
    permission_waiting: bool,
}

impl ToolResultItem {
    /// `new` 创建一条工具审批结果展示项。
    pub(crate) fn new(content: impl Into<String>, kind: ToolResultKind) -> Self {
        let content = content.into();
        Self::from_body(
            ToolResultBody::Approval { content, kind },
            ToolActivityRenderMode::Compact,
        )
    }

    /// `from_acp_tool_call` 创建一条 ACP tool call 展示项。
    pub(crate) fn from_acp_tool_call(
        call: AcpToolCall,
        render_mode: ToolActivityRenderMode,
    ) -> Self {
        Self::from_body(ToolResultBody::AcpToolCall(call), render_mode)
    }

    fn from_body(body: ToolResultBody, render_mode: ToolActivityRenderMode) -> Self {
        let approval_suspended = false;
        let permission_waiting = false;
        let render_cache_key = tool_result_render_cache_key(
            &body,
            render_mode,
            approval_suspended,
            permission_waiting,
        );
        let active_marker_started_at =
            active_marker_started_at_for_body(&body).then_some(Instant::now());
        Self {
            body,
            render_mode,
            render_cache_key,
            active_marker_started_at,
            approval_suspended,
            permission_waiting,
        }
    }

    /// `render_lines` 将工具审批结果渲染为带颜色的文本行。
    pub(crate) fn render_lines(&self, width: u16, palette: TerminalPalette) -> Vec<Line<'static>> {
        self.wrapped_styled_lines_with_active_marker_visible(width, palette)
    }

    pub(crate) fn render_lines_at(
        &self,
        width: u16,
        palette: TerminalPalette,
        now: Instant,
    ) -> Vec<Line<'static>> {
        self.wrapped_styled_lines_at(width, palette, now)
    }

    pub(crate) fn has_active_acp_tool_call(&self) -> bool {
        self.active_marker_started_at.is_some() && !self.is_compact_approval_suspended()
    }

    pub(crate) fn acp_tool_call_id(&self) -> Option<&str> {
        match &self.body {
            ToolResultBody::AcpToolCall(call) => Some(call.tool_call_id.as_str()),
            ToolResultBody::Approval { .. } => None,
        }
    }

    pub(crate) fn active_marker_started_at(&self) -> Option<Instant> {
        if self.is_compact_approval_suspended() {
            return None;
        }

        self.active_marker_started_at
    }

    fn wrapped_styled_lines_with_active_marker_visible(
        &self,
        width: u16,
        palette: TerminalPalette,
    ) -> Vec<Line<'static>> {
        let now = self.active_marker_started_at.unwrap_or_else(Instant::now);
        self.wrapped_styled_lines_at(width, palette, now)
    }

    fn wrapped_styled_lines_at(
        &self,
        width: u16,
        palette: TerminalPalette,
        now: Instant,
    ) -> Vec<Line<'static>> {
        if self.is_compact_approval_suspended() {
            return Vec::new();
        }

        match &self.body {
            ToolResultBody::Approval { content, .. } => {
                self.approval_wrapped_styled_lines(content, width, palette)
            }
            ToolResultBody::AcpToolCall(call) => {
                self.acp_tool_call_styled_lines_at(call, width, palette, now)
            }
        }
    }

    /// `render_for_terminal_replay` 返回适合退出 AltScreen 后回放到终端的文本。
    pub(crate) fn render_for_terminal_replay(
        &self,
        width: u16,
        palette: TerminalPalette,
        preserve_ansi: bool,
    ) -> String {
        let lines = self.render_lines(width, palette);
        if preserve_ansi {
            lines_to_ansi_text(&lines)
        } else {
            lines_to_plain_text(&lines)
        }
    }

    /// `render_plain_text` 返回不带 ANSI 的纯文本内容。
    pub(crate) fn render_plain_text(&self, width: u16, palette: TerminalPalette) -> String {
        lines_to_plain_text(&self.render_lines(width, palette))
    }

    pub(crate) fn render_cache_key(&self) -> u64 {
        self.render_cache_key
    }

    pub(crate) fn source_text_byte_len(&self) -> usize {
        match &self.body {
            ToolResultBody::Approval { content, .. } => content.len(),
            ToolResultBody::AcpToolCall(call) => {
                call.title.len()
                    + call
                        .raw_input
                        .as_ref()
                        .map(|raw_input| raw_input.display_byte_len())
                        .unwrap_or(0)
                    + call
                        .raw_output
                        .as_ref()
                        .map(|raw_output| raw_output.display_byte_len())
                        .unwrap_or(0)
                    + call
                        .content
                        .iter()
                        .map(acp_tool_call_content_byte_len)
                        .sum::<usize>()
            }
        }
    }

    pub(crate) fn measure_render_metrics(
        &self,
        width: u16,
        palette: TerminalPalette,
    ) -> (usize, usize) {
        let lines = self.wrapped_styled_lines_with_active_marker_visible(width, palette);
        let content_char_len = lines
            .iter()
            .map(|line| line_to_plain_text(line).len())
            .sum::<usize>();

        (lines.len(), content_char_len)
    }

    pub(crate) fn estimate_render_metrics_fast(
        &self,
        width: u16,
        palette: TerminalPalette,
        previous_metrics: Option<TranscriptItemMetrics>,
    ) -> TranscriptFastEstimate {
        let previous_metrics =
            previous_metrics.filter(|metrics| metrics.cache_key == self.render_cache_key);
        if let Some(metrics) = previous_metrics
            && metrics.is_valid
            && metrics.width == width
        {
            return TranscriptFastEstimate {
                content_line_count: metrics.content_line_count,
                content_char_len: metrics.content_char_len,
                kind: TranscriptEstimateKind::NonAssistant,
                ..TranscriptFastEstimate::default()
            };
        }

        let (content_line_count, content_char_len) = self.measure_render_metrics(width, palette);
        TranscriptFastEstimate {
            content_line_count,
            content_char_len,
            kind: TranscriptEstimateKind::NonAssistant,
            ..TranscriptFastEstimate::default()
        }
    }

    pub(crate) fn render_line_anchors(
        &self,
        _width: u16,
        _palette: TerminalPalette,
    ) -> Vec<ItemLineAnchor> {
        Vec::new()
    }

    pub(crate) fn set_render_mode(&mut self, render_mode: ToolActivityRenderMode) -> bool {
        if self.render_mode == render_mode {
            return false;
        }

        self.render_mode = render_mode;
        self.refresh_render_cache_key();
        true
    }

    pub(crate) fn set_approval_suspended(&mut self, suspended: bool) -> bool {
        if !matches!(self.body, ToolResultBody::AcpToolCall(_)) {
            return false;
        }
        if self.approval_suspended == suspended {
            return false;
        }

        self.approval_suspended = suspended;
        self.refresh_render_cache_key();
        true
    }

    pub(crate) fn set_permission_waiting(&mut self, waiting: bool) -> bool {
        if !matches!(self.body, ToolResultBody::AcpToolCall(_)) {
            return false;
        }
        if self.permission_waiting == waiting {
            return false;
        }

        self.permission_waiting = waiting;
        self.refresh_render_cache_key();
        true
    }

    pub(crate) fn update_acp_tool_call(&mut self, update: AcpToolCallUpdate) -> bool {
        let ToolResultBody::AcpToolCall(call) = &mut self.body else {
            return false;
        };
        if call.tool_call_id != update.tool_call_id {
            return false;
        }

        if let Some(title) = update.title {
            call.title = title;
        }
        if let Some(kind) = update.kind {
            call.kind = kind;
        }
        if let Some(status) = update.status {
            call.status = status;
            if status != AcpToolCallStatus::Pending {
                self.permission_waiting = false;
            }
        }
        if let Some(content) = update.content {
            call.content = content;
        }
        if let Some(locations) = update.locations {
            call.locations = locations;
        }
        if let Some(raw_input) = update.raw_input {
            call.raw_input = Some(raw_input);
        }
        if let Some(raw_output) = update.raw_output {
            call.raw_output = Some(raw_output);
        }
        self.active_marker_started_at = active_marker_started_at_for_body(&self.body)
            .then(|| self.active_marker_started_at.unwrap_or_else(Instant::now));
        self.refresh_render_cache_key();
        true
    }

    pub(crate) fn mark_acp_tool_call_failed(&mut self, message: impl Into<String>) -> bool {
        let ToolResultBody::AcpToolCall(call) = &mut self.body else {
            return false;
        };
        if matches!(
            call.status,
            AcpToolCallStatus::Completed | AcpToolCallStatus::Failed
        ) {
            return false;
        }

        call.status = AcpToolCallStatus::Failed;
        call.content = vec![AcpToolCallContent::Text(message.into())];
        self.permission_waiting = false;
        self.active_marker_started_at = None;
        self.refresh_render_cache_key();
        true
    }

    pub(crate) fn mark_acp_tool_call_rejected(&mut self) -> bool {
        let ToolResultBody::AcpToolCall(call) = &mut self.body else {
            return false;
        };
        if call.status == AcpToolCallStatus::Failed
            && call.content.is_empty()
            && !self.permission_waiting
        {
            return false;
        }

        call.status = AcpToolCallStatus::Failed;
        call.content.clear();
        call.raw_input = None;
        call.raw_output = None;
        self.permission_waiting = false;
        self.active_marker_started_at = None;
        self.refresh_render_cache_key();
        true
    }

    fn approval_wrapped_styled_lines(
        &self,
        content: &str,
        width: u16,
        palette: TerminalPalette,
    ) -> Vec<Line<'static>> {
        let width = usize::from(width.max(1));
        content
            .split('\n')
            .enumerate()
            .flat_map(|(logical_line, content_line)| {
                self.wrap_logical_line(content_line, logical_line, width, palette)
            })
            .collect()
    }

    fn wrap_logical_line(
        &self,
        content_line: &str,
        logical_line: usize,
        width: usize,
        palette: TerminalPalette,
    ) -> Vec<Line<'static>> {
        let initial_prefix = if logical_line == 0 {
            TOOL_RESULT_PREFIX
        } else {
            TOOL_RESULT_CONTINUATION_PREFIX
        };
        let prefix_width = UnicodeWidthStr::width(initial_prefix);
        let content_width = width.saturating_sub(prefix_width).max(1);
        let logical_lines = self.wrap_content_line(content_line, content_width, palette);

        if logical_lines.is_empty() {
            return vec![Line::from(vec![self.prefix_span(initial_prefix, palette)])];
        }

        logical_lines
            .into_iter()
            .enumerate()
            .map(|(wrapped_index, content_spans)| {
                let prefix = if wrapped_index == 0 {
                    initial_prefix
                } else {
                    TOOL_RESULT_CONTINUATION_PREFIX
                };
                let mut spans = Vec::with_capacity(content_spans.len() + 1);
                spans.push(self.prefix_span(prefix, palette));
                spans.extend(content_spans);
                Line::from(spans)
            })
            .collect()
    }

    fn wrap_content_line(
        &self,
        content_line: &str,
        width: usize,
        _palette: TerminalPalette,
    ) -> Vec<Vec<Span<'static>>> {
        let Some(parsed) = ParsedToolResultLine::parse(content_line) else {
            return self.wrap_plain_content(content_line, width);
        };

        if !parsed.should_highlight_as_shell {
            return self.wrap_plain_result_content(&parsed.non_shell_display_text(), width);
        }

        self.wrap_shell_result_content(parsed, width)
    }

    fn wrap_plain_content(&self, content_line: &str, width: usize) -> Vec<Vec<Span<'static>>> {
        wrap_prompt_visual_lines(content_line, width, 0)
            .into_iter()
            .map(|line| vec![Span::raw(line.text)])
            .collect()
    }

    fn wrap_plain_result_content(
        &self,
        content_line: &str,
        width: usize,
    ) -> Vec<Vec<Span<'static>>> {
        wrap_prompt_visual_lines(content_line, width, 0)
            .into_iter()
            .map(|line| style_core_result_line(line.text))
            .collect()
    }

    fn wrap_shell_result_content(
        &self,
        parsed: ParsedToolResultLine<'_>,
        width: usize,
    ) -> Vec<Vec<Span<'static>>> {
        let mut chunks = vec![HighlightChunk {
            text: parsed.verb.to_string(),
            style: Style::new().add_modifier(Modifier::BOLD),
        }];

        if !parsed.body.is_empty() {
            chunks.push(HighlightChunk {
                text: " ".to_string(),
                style: Style::new(),
            });
            chunks.extend(self.shell_command_chunks(parsed.body));
        }

        wrap_highlight_chunks(&[chunks], width)
    }

    fn shell_command_chunks(&self, command: &str) -> Vec<HighlightChunk> {
        self.shell_command_chunks_with_style(command, Style::new())
    }

    fn shell_command_chunks_with_style(
        &self,
        command: &str,
        base_style: Style,
    ) -> Vec<HighlightChunk> {
        highlight_code_chunks(command, "bash", base_style)
            .map(|highlighted| highlighted.into_iter().flatten().collect::<Vec<_>>())
            .filter(|chunks| !chunks.is_empty())
            .unwrap_or_else(|| {
                vec![HighlightChunk {
                    text: command.to_string(),
                    style: base_style,
                }]
            })
    }

    fn prefix_span(&self, prefix: &'static str, palette: TerminalPalette) -> Span<'static> {
        Span::styled(prefix, self.result_style(palette))
    }

    fn acp_tool_call_styled_lines_at(
        &self,
        call: &AcpToolCall,
        width: u16,
        palette: TerminalPalette,
        now: Instant,
    ) -> Vec<Line<'static>> {
        let width = usize::from(width.max(1));
        let mut lines = self.acp_tool_call_header_lines_at(call, width, palette, now);
        for block in acp_tool_call_detail_blocks(call, self.render_mode, self.permission_waiting) {
            lines.extend(self.wrap_acp_detail_block(&block, width, palette));
        }
        lines
    }

    fn acp_tool_call_header_lines_at(
        &self,
        call: &AcpToolCall,
        width: usize,
        palette: TerminalPalette,
        now: Instant,
    ) -> Vec<Line<'static>> {
        let active_started_at = self.active_marker_started_at.filter(|_| {
            matches!(
                call.status,
                AcpToolCallStatus::Pending | AcpToolCallStatus::InProgress
            )
        });
        let marker_visible = active_started_at
            .map(|started_at| active_marker_visible_at(started_at, now))
            .unwrap_or(true);
        let marker_text = if marker_visible {
            TOOL_RESULT_PREFIX
        } else {
            TOOL_RESULT_CONTINUATION_PREFIX
        };
        let marker_color = if active_started_at.is_some() {
            palette.main
        } else {
            acp_tool_call_status_color(call.status, palette)
        };
        let status_style = style_for_color(marker_color).add_modifier(Modifier::BOLD);
        let location_style = style_for_color(palette.tertiary);
        let mut chunks = vec![HighlightChunk {
            text: marker_text.to_string(),
            style: status_style,
        }];
        chunks.extend(self.acp_tool_call_title_chunks(call, palette));

        if !is_acp_read_tool_call(call)
            && !acp_tool_call_has_diff_content(call)
            && let Some(locations) = acp_tool_call_location_suffix(&call.locations)
        {
            chunks.push(HighlightChunk {
                text: format!(" {locations}"),
                style: location_style,
            });
        }

        wrap_highlight_chunks(&[chunks], width)
            .into_iter()
            .map(Line::from)
            .collect()
    }

    fn acp_tool_call_title_chunks(
        &self,
        call: &AcpToolCall,
        palette: TerminalPalette,
    ) -> Vec<HighlightChunk> {
        if let Some(chunks) = acp_tool_call_diff_header_chunks(call, palette) {
            return chunks;
        }

        if is_acp_read_tool_call(call) {
            return acp_read_tool_call_title_chunks(call);
        }

        if is_acp_write_tool_call(call) {
            return acp_write_tool_call_title_chunks(call);
        }

        let title = acp_tool_call_display_title(call);
        let title_style = Style::new().add_modifier(Modifier::BOLD);
        if looks_like_shell_command(&title) {
            return self.shell_command_chunks_with_style(&title, title_style);
        }

        vec![HighlightChunk {
            text: title,
            style: title_style,
        }]
    }

    fn wrap_acp_detail_block(
        &self,
        block: &AcpToolCallDetailBlock,
        width: usize,
        palette: TerminalPalette,
    ) -> Vec<Line<'static>> {
        match block {
            AcpToolCallDetailBlock::Text(logical_lines) => {
                self.wrap_acp_text_detail_block(logical_lines, width)
            }
            AcpToolCallDetailBlock::SecondaryText(logical_lines) => {
                self.wrap_acp_secondary_text_detail_block(logical_lines, width, palette)
            }
            AcpToolCallDetailBlock::Diff(logical_lines) => {
                self.wrap_acp_diff_detail_block(logical_lines, width, palette)
            }
        }
    }

    fn wrap_acp_text_detail_block(
        &self,
        logical_lines: &[String],
        width: usize,
    ) -> Vec<Line<'static>> {
        logical_lines
            .iter()
            .enumerate()
            .flat_map(|(logical_index, content)| {
                let initial_prefix = if logical_index == 0 {
                    TOOL_ACTIVITY_DETAIL_PREFIX
                } else {
                    TOOL_ACTIVITY_DETAIL_CONTINUATION_PREFIX
                };
                let prefix_width = UnicodeWidthStr::width(initial_prefix);
                let content_width = width.saturating_sub(prefix_width).max(1);
                let wrapped = wrap_prompt_visual_lines(content, content_width, 0);

                if wrapped.is_empty() {
                    return vec![Line::from(vec![Span::raw(initial_prefix)])];
                }

                wrapped
                    .into_iter()
                    .enumerate()
                    .map(|(wrapped_index, line)| {
                        let prefix = if wrapped_index == 0 {
                            initial_prefix
                        } else {
                            TOOL_ACTIVITY_DETAIL_CONTINUATION_PREFIX
                        };
                        Line::from(vec![Span::raw(prefix), Span::raw(line.text)])
                    })
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    fn wrap_acp_secondary_text_detail_block(
        &self,
        logical_lines: &[String],
        width: usize,
        palette: TerminalPalette,
    ) -> Vec<Line<'static>> {
        self.wrap_acp_styled_text_detail_block(logical_lines, width, secondary_text_style(palette))
    }

    fn wrap_acp_styled_text_detail_block(
        &self,
        logical_lines: &[String],
        width: usize,
        content_style: Style,
    ) -> Vec<Line<'static>> {
        logical_lines
            .iter()
            .enumerate()
            .flat_map(|(logical_index, content)| {
                let initial_prefix = if logical_index == 0 {
                    TOOL_ACTIVITY_DETAIL_PREFIX
                } else {
                    TOOL_ACTIVITY_DETAIL_CONTINUATION_PREFIX
                };
                let prefix_width = UnicodeWidthStr::width(initial_prefix);
                let content_width = width.saturating_sub(prefix_width).max(1);
                let wrapped = wrap_highlight_chunks(
                    &[vec![HighlightChunk {
                        text: content.clone(),
                        style: content_style,
                    }]],
                    content_width,
                );

                if wrapped.is_empty() {
                    return vec![Line::from(vec![Span::raw(initial_prefix)])];
                }

                wrapped
                    .into_iter()
                    .enumerate()
                    .map(|(wrapped_index, content_spans)| {
                        let prefix = if wrapped_index == 0 {
                            initial_prefix
                        } else {
                            TOOL_ACTIVITY_DETAIL_CONTINUATION_PREFIX
                        };
                        let mut spans = Vec::with_capacity(content_spans.len() + 1);
                        spans.push(Span::raw(prefix));
                        spans.extend(content_spans);
                        Line::from(spans)
                    })
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    fn wrap_acp_diff_detail_block(
        &self,
        logical_lines: &[AcpDiffDetailLine],
        width: usize,
        palette: TerminalPalette,
    ) -> Vec<Line<'static>> {
        logical_lines
            .iter()
            .flat_map(|content| {
                let prefix = acp_diff_line_prefix(content.line_number, content.kind);
                let continuation_prefix = " ".repeat(UnicodeWidthStr::width(prefix.as_str()));
                let prefix_width = UnicodeWidthStr::width(prefix.as_str());
                let content_width = width.saturating_sub(prefix_width).max(1);
                let line_style = acp_tool_call_diff_line_style(content.kind, palette);
                let wrapped = wrap_highlight_chunks(
                    &[vec![HighlightChunk {
                        text: content.text.clone(),
                        style: line_style,
                    }]],
                    content_width,
                );

                if wrapped.is_empty() {
                    let mut line = Line::from(vec![Span::styled(prefix, line_style)]);
                    line.style = line
                        .style
                        .patch(acp_tool_call_diff_row_style(content.kind, palette));
                    return vec![line];
                }

                wrapped
                    .into_iter()
                    .enumerate()
                    .map(|(wrapped_index, content_spans)| {
                        let line_prefix = if wrapped_index == 0 {
                            prefix.clone()
                        } else {
                            continuation_prefix.clone()
                        };
                        let mut spans = Vec::with_capacity(content_spans.len() + 1);
                        spans.push(Span::styled(line_prefix, line_style));
                        spans.extend(content_spans);
                        let mut rendered = Line::from(spans);
                        rendered.style = rendered
                            .style
                            .patch(acp_tool_call_diff_row_style(content.kind, palette));
                        rendered
                    })
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    fn result_style(&self, palette: TerminalPalette) -> Style {
        let color = match &self.body {
            ToolResultBody::Approval {
                kind: ToolResultKind::Ran,
                ..
            } => palette.quote,
            ToolResultBody::Approval {
                kind: ToolResultKind::Rejected,
                ..
            } => palette.approval_rejected,
            ToolResultBody::AcpToolCall(call) => acp_tool_call_status_color(call.status, palette),
        };

        if color == Color::Reset {
            Style::new()
        } else {
            Style::new().fg(color)
        }
    }

    fn refresh_render_cache_key(&mut self) {
        self.render_cache_key = tool_result_render_cache_key(
            &self.body,
            self.render_mode,
            self.approval_suspended,
            self.permission_waiting,
        );
    }

    fn is_compact_approval_suspended(&self) -> bool {
        self.approval_suspended && self.render_mode == ToolActivityRenderMode::Compact
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ParsedToolResultLine<'a> {
    verb: &'a str,
    body: &'a str,
    should_highlight_as_shell: bool,
}

impl<'a> ParsedToolResultLine<'a> {
    fn parse(content_line: &'a str) -> Option<Self> {
        let (verb, body) = split_verb(content_line)?;
        let body = body.trim_start();
        let (body, has_shell_prefix) = body
            .strip_prefix("Shell:")
            .map(|command| (command.trim_start(), true))
            .unwrap_or((body, false));
        let should_highlight_as_shell = has_shell_prefix || looks_like_shell_command(body);

        Some(Self {
            verb,
            body,
            should_highlight_as_shell,
        })
    }

    fn non_shell_display_text(self) -> String {
        match self.verb {
            "Ran" => self.body.to_string(),
            "Reject" => {
                let rejected_body = strip_redundant_reject_title_verb(self.body);
                if rejected_body.is_empty() {
                    self.verb.to_string()
                } else {
                    format!("{} {}", self.verb, rejected_body)
                }
            }
            _ => {
                if self.body.is_empty() {
                    self.verb.to_string()
                } else {
                    format!("{} {}", self.verb, self.body)
                }
            }
        }
    }
}

fn split_verb(content_line: &str) -> Option<(&str, &str)> {
    for verb in ["Ran", "Reject"] {
        if content_line == verb {
            return Some((verb, ""));
        }
        if let Some(body) = content_line.strip_prefix(verb)
            && body.starts_with(char::is_whitespace)
        {
            return Some((verb, body));
        }
    }

    None
}

fn looks_like_shell_command(body: &str) -> bool {
    let Some(first) = body.trim_start().chars().next() else {
        return false;
    };

    first.is_ascii_lowercase()
        || first.is_ascii_digit()
        || matches!(first, '.' | '/' | '~' | '$' | '\'' | '"' | '`')
}

fn strip_redundant_reject_title_verb(text: &str) -> &str {
    let text = text.trim_start();
    text.strip_prefix("Run ")
        .map(str::trim_start)
        .unwrap_or(text)
}

fn style_core_result_line(line: String) -> Vec<Span<'static>> {
    let Some((core, rest)) = split_first_word(&line) else {
        return vec![Span::raw(line)];
    };

    if rest.is_empty() {
        return vec![Span::styled(
            core.to_string(),
            Style::new().add_modifier(Modifier::BOLD),
        )];
    }

    vec![
        Span::styled(core.to_string(), Style::new().add_modifier(Modifier::BOLD)),
        Span::raw(rest.to_string()),
    ]
}

fn split_first_word(line: &str) -> Option<(&str, &str)> {
    if line.is_empty() {
        return None;
    }

    let Some((index, _)) = line.char_indices().find(|(_, ch)| ch.is_whitespace()) else {
        return Some((line, ""));
    };

    Some((&line[..index], &line[index..]))
}

fn active_marker_started_at_for_body(body: &ToolResultBody) -> bool {
    matches!(
        body,
        ToolResultBody::AcpToolCall(AcpToolCall {
            status: AcpToolCallStatus::Pending | AcpToolCallStatus::InProgress,
            ..
        })
    )
}

fn tool_result_render_cache_key(
    body: &ToolResultBody,
    render_mode: ToolActivityRenderMode,
    approval_suspended: bool,
    permission_waiting: bool,
) -> u64 {
    let mut hasher = DefaultHasher::new();
    "tool_result".hash(&mut hasher);
    render_mode.hash(&mut hasher);
    approval_suspended.hash(&mut hasher);
    permission_waiting.hash(&mut hasher);
    body.hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use ratatui::style::Modifier;

    use super::*;
    use crate::frontend::tui::{
        styled_text::line_to_plain_text,
        theme::{default_palette, terminal_default_palette},
    };

    #[test]
    fn ran_result_uses_quote_color_without_italic() {
        let palette = default_palette();
        let item = ToolResultItem::new("Ran Write file", ToolResultKind::Ran);
        let lines = item.render_lines(80, palette);

        assert_eq!(line_to_plain_text(&lines[0]), "● Write file");
        assert_eq!(lines[0].spans[0].style.fg, Some(palette.quote));
        assert_eq!(lines[0].spans[1].content.as_ref(), "Write");
        assert!(lines[0].spans[1].style.fg.is_none());
        assert!(
            lines[0].spans[1]
                .style
                .add_modifier
                .contains(Modifier::BOLD)
        );
        assert!(lines[0].spans[2].style.fg.is_none());
        assert!(
            !lines[0].spans[0]
                .style
                .add_modifier
                .contains(Modifier::ITALIC)
        );
        assert!(
            !lines[0].spans[1]
                .style
                .add_modifier
                .contains(Modifier::ITALIC)
        );
    }

    #[test]
    fn rejected_result_uses_approval_rejected_color() {
        let palette = default_palette();
        let item = ToolResultItem::new("Reject Run destructive command", ToolResultKind::Rejected);
        let lines = item.render_lines(80, palette);

        assert_eq!(
            line_to_plain_text(&lines[0]),
            "● Reject destructive command"
        );
        assert_eq!(lines[0].spans[0].style.fg, Some(palette.approval_rejected));
        assert_eq!(lines[0].spans[1].content.as_ref(), "Reject");
        assert!(lines[0].spans[1].style.fg.is_none());
        assert!(
            lines[0].spans[1]
                .style
                .add_modifier
                .contains(Modifier::BOLD)
        );
        assert!(lines[0].spans[2].style.fg.is_none());
    }

    #[test]
    fn rejected_non_shell_result_preserves_non_run_title_action() {
        let item = ToolResultItem::new("Reject Write file", ToolResultKind::Rejected);
        let lines = item.render_lines(80, default_palette());

        assert_eq!(line_to_plain_text(&lines[0]), "● Reject Write file");
    }

    #[test]
    fn shell_result_removes_shell_prefix_and_highlights_command() {
        let palette = default_palette();
        let item = ToolResultItem::new("Ran Shell: cat Cargo.toml", ToolResultKind::Ran);
        let lines = item.render_lines(80, palette);

        assert_eq!(line_to_plain_text(&lines[0]), "● Ran cat Cargo.toml");
        assert_eq!(lines[0].spans[0].style.fg, Some(palette.quote));
        assert_eq!(lines[0].spans[1].content.as_ref(), "Ran");
        assert!(lines[0].spans[1].style.fg.is_none());
        assert!(
            lines[0].spans[1]
                .style
                .add_modifier
                .contains(Modifier::BOLD)
        );
        assert!(
            lines[0]
                .spans
                .iter()
                .skip(2)
                .any(|span| span.style.fg.is_some()),
            "shell command spans should carry syntax highlight foreground colors: {:?}",
            lines[0].spans
        );
    }

    #[test]
    fn acp_tool_call_header_uses_title_only_and_strips_shell_prefix() {
        let palette = default_palette();
        let item = ToolResultItem::from_acp_tool_call(
            AcpToolCall {
                tool_call_id: "call-1".to_string(),
                title: "Shell: cargo check".to_string(),
                kind: AcpToolKind::Other,
                status: AcpToolCallStatus::Completed,
                content: Vec::new(),
                locations: Vec::new(),
                raw_input: None,
                raw_output: None,
            },
            ToolActivityRenderMode::Compact,
        );
        let lines = item.render_lines(80, palette);

        assert_eq!(line_to_plain_text(&lines[0]), "● cargo check");
        assert_eq!(lines[0].spans[0].style.fg, Some(palette.quote));
        assert!(
            lines[0]
                .spans
                .iter()
                .all(|span| !span.content.as_ref().contains("Completed")),
            "status text should not be part of the ACP header: {:?}",
            lines[0].spans
        );
        assert!(
            lines[0]
                .spans
                .iter()
                .all(|span| !span.content.as_ref().contains("[Other]")),
            "kind label should not be part of the ACP header: {:?}",
            lines[0].spans
        );
        assert!(
            lines[0]
                .spans
                .iter()
                .all(|span| !span.content.as_ref().contains("Shell:")),
            "tool prefix should be stripped from the ACP header: {:?}",
            lines[0].spans
        );
    }

    #[test]
    fn acp_tool_call_header_highlights_shell_titles() {
        let palette = default_palette();
        let item = ToolResultItem::from_acp_tool_call(
            AcpToolCall {
                tool_call_id: "call-1".to_string(),
                title: "Shell: cargo check".to_string(),
                kind: AcpToolKind::Other,
                status: AcpToolCallStatus::Completed,
                content: Vec::new(),
                locations: Vec::new(),
                raw_input: None,
                raw_output: None,
            },
            ToolActivityRenderMode::Compact,
        );
        let lines = item.render_lines(80, palette);

        assert_eq!(line_to_plain_text(&lines[0]), "● cargo check");
        assert!(
            lines[0]
                .spans
                .iter()
                .skip(1)
                .any(|span| span.style.fg.is_some()),
            "shell-like ACP titles should carry syntax highlight foreground colors: {:?}",
            lines[0].spans
        );
    }

    #[test]
    fn pending_execute_tool_call_renders_waiting_detail() {
        let item = ToolResultItem::from_acp_tool_call(
            AcpToolCall {
                tool_call_id: "call-approval".to_string(),
                title: "Shell: cargo check".to_string(),
                kind: AcpToolKind::Execute,
                status: AcpToolCallStatus::Pending,
                content: Vec::new(),
                locations: Vec::new(),
                raw_input: None,
                raw_output: None,
            },
            ToolActivityRenderMode::Compact,
        );
        let rendered_plain = item
            .render_lines(80, default_palette())
            .iter()
            .map(line_to_plain_text)
            .collect::<Vec<_>>();

        assert_eq!(
            rendered_plain,
            vec!["● cargo check".to_string(), "  └─ Waiting...".to_string()]
        );
        assert!(
            rendered_plain
                .iter()
                .all(|line| !line.contains("Requesting approval")),
            "tool call row should not duplicate the approval panel request text: {rendered_plain:?}"
        );
    }

    #[test]
    fn active_execute_tool_call_defers_streamed_content_until_finished() {
        let item = ToolResultItem::from_acp_tool_call(
            AcpToolCall {
                tool_call_id: "call-exec".to_string(),
                title: "Shell: cargo check".to_string(),
                kind: AcpToolKind::Execute,
                status: AcpToolCallStatus::InProgress,
                content: vec![AcpToolCallContent::Text(
                    "Requesting approval to perform: Run command `cargo check`".to_string(),
                )],
                locations: Vec::new(),
                raw_input: Some(r#"{"command":"cargo check"}"#.into()),
                raw_output: Some("Checking lumos v0.1.0".into()),
            },
            ToolActivityRenderMode::Compact,
        );
        let rendered_plain = item
            .render_lines(80, default_palette())
            .iter()
            .map(line_to_plain_text)
            .collect::<Vec<_>>();

        assert_eq!(
            rendered_plain,
            vec!["● cargo check".to_string(), "  └─ Waiting...".to_string()]
        );
        assert!(
            rendered_plain.iter().all(|line| {
                !line.contains("Requesting approval")
                    && !line.contains("Checking lumos")
                    && !line.contains(r#"{"command":"cargo check"}"#)
            }),
            "active command tool calls should not stream command details in the main transcript: {rendered_plain:?}"
        );
    }

    #[test]
    fn completed_execute_tool_call_renders_deferred_content() {
        let item = ToolResultItem::from_acp_tool_call(
            AcpToolCall {
                tool_call_id: "call-exec".to_string(),
                title: "Shell: cargo check".to_string(),
                kind: AcpToolKind::Execute,
                status: AcpToolCallStatus::Completed,
                content: Vec::new(),
                locations: Vec::new(),
                raw_input: None,
                raw_output: Some("Checking lumos v0.1.0".into()),
            },
            ToolActivityRenderMode::Compact,
        );
        let rendered_plain = item
            .render_lines(80, default_palette())
            .iter()
            .map(line_to_plain_text)
            .collect::<Vec<_>>();

        assert_eq!(
            rendered_plain,
            vec![
                "● cargo check".to_string(),
                "  └─ Checking lumos v0.1.0".to_string(),
            ]
        );
    }

    #[test]
    fn completed_execute_tool_call_prefers_raw_output_and_hides_permission_copy_content() {
        let item = ToolResultItem::from_acp_tool_call(
            AcpToolCall {
                tool_call_id: "call-exec".to_string(),
                title: "Shell: cargo check".to_string(),
                kind: AcpToolKind::Execute,
                status: AcpToolCallStatus::Completed,
                content: vec![AcpToolCallContent::Text(
                    "Requesting approval to perform: Run command `cargo check`".to_string(),
                )],
                locations: Vec::new(),
                raw_input: Some(r#"{"command":"cargo check"}"#.into()),
                raw_output: Some("Finished dev profile".into()),
            },
            ToolActivityRenderMode::Compact,
        );
        let rendered_plain = item
            .render_lines(80, default_palette())
            .iter()
            .map(line_to_plain_text)
            .collect::<Vec<_>>();

        assert_eq!(
            rendered_plain,
            vec![
                "● cargo check".to_string(),
                "  └─ Finished dev profile".to_string(),
            ]
        );
        assert!(
            rendered_plain.iter().all(|line| {
                !line.contains("Requesting approval")
                    && !line.contains("Input:")
                    && !line.contains(r#"{"command":"cargo check"}"#)
            }),
            "completed command rows should show final output without approval copy or raw input: {rendered_plain:?}"
        );
    }

    #[test]
    fn failed_execute_tool_call_renders_final_output_without_raw_input() {
        let item = ToolResultItem::from_acp_tool_call(
            AcpToolCall {
                tool_call_id: "call-exec".to_string(),
                title: "Shell: cargo check".to_string(),
                kind: AcpToolKind::Execute,
                status: AcpToolCallStatus::Failed,
                content: Vec::new(),
                locations: Vec::new(),
                raw_input: Some(r#"{"command":"cargo check"}"#.into()),
                raw_output: Some("error: could not compile `lumos`".into()),
            },
            ToolActivityRenderMode::Compact,
        );
        let rendered_plain = item
            .render_lines(80, default_palette())
            .iter()
            .map(line_to_plain_text)
            .collect::<Vec<_>>();

        assert_eq!(
            rendered_plain,
            vec![
                "● cargo check".to_string(),
                "  └─ error: could not compile `lumos`".to_string(),
            ]
        );
        assert!(
            rendered_plain.iter().all(|line| {
                !line.contains("Input:") && !line.contains(r#"{"command":"cargo check"}"#)
            }),
            "failed command rows should show final output without raw transport input: {rendered_plain:?}"
        );
    }

    #[test]
    fn completed_non_execute_tool_call_still_renders_text_content() {
        let item = ToolResultItem::from_acp_tool_call(
            AcpToolCall {
                tool_call_id: "call-fetch".to_string(),
                title: "Fetch package metadata".to_string(),
                kind: AcpToolKind::Fetch,
                status: AcpToolCallStatus::Completed,
                content: vec![AcpToolCallContent::Text("Found 3 releases".to_string())],
                locations: Vec::new(),
                raw_input: None,
                raw_output: None,
            },
            ToolActivityRenderMode::Compact,
        );
        let rendered_plain = item
            .render_lines(80, default_palette())
            .iter()
            .map(line_to_plain_text)
            .collect::<Vec<_>>();

        assert_eq!(
            rendered_plain,
            vec![
                "● Fetch package metadata".to_string(),
                "  └─ Found 3 releases".to_string(),
            ]
        );
    }

    #[test]
    fn acp_tool_call_raw_output_trailing_newline_does_not_render_blank_line() {
        let palette = default_palette();
        let item = ToolResultItem::from_acp_tool_call(
            AcpToolCall {
                tool_call_id: "call-1".to_string(),
                title: "Shell: cargo check".to_string(),
                kind: AcpToolKind::Other,
                status: AcpToolCallStatus::Completed,
                content: Vec::new(),
                locations: Vec::new(),
                raw_input: None,
                raw_output: Some("Checking lumos\n".into()),
            },
            ToolActivityRenderMode::Compact,
        );
        let rendered = item.render_lines(80, palette);
        let rendered_plain = rendered.iter().map(line_to_plain_text).collect::<Vec<_>>();

        assert_eq!(
            rendered_plain,
            vec![
                "● cargo check".to_string(),
                "  └─ Checking lumos".to_string(),
            ]
        );
        assert!(
            rendered
                .last()
                .is_some_and(|line| !line_to_plain_text(line).trim().is_empty()),
            "rendered ACP output should not end with a blank line: {rendered_plain:?}"
        );
    }

    #[test]
    fn acp_pending_text_content_is_not_approval_waiting_without_permission_state() {
        let item = ToolResultItem::from_acp_tool_call(
            AcpToolCall {
                tool_call_id: "call-1".to_string(),
                title: "Check policy".to_string(),
                kind: AcpToolKind::Other,
                status: AcpToolCallStatus::Pending,
                content: vec![AcpToolCallContent::Text(
                    "This result requires approval from the project owner.".to_string(),
                )],
                locations: Vec::new(),
                raw_input: None,
                raw_output: None,
            },
            ToolActivityRenderMode::Compact,
        );
        let rendered_plain = item
            .render_lines(80, default_palette())
            .iter()
            .map(line_to_plain_text)
            .collect::<Vec<_>>();

        assert!(
            rendered_plain
                .iter()
                .any(|line| line.contains("requires approval from the project owner")),
            "plain tool text should remain visible unless the runtime marks the row as waiting for permission: {rendered_plain:?}"
        );
        assert!(
            rendered_plain
                .iter()
                .all(|line| !line.contains("Waiting...")),
            "plain tool text must not be inferred as approval waiting from content wording: {rendered_plain:?}"
        );
    }

    #[test]
    fn acp_tool_call_multi_line_raw_output_uses_four_space_continuation_prefix() {
        let palette = default_palette();
        let item = ToolResultItem::from_acp_tool_call(
            AcpToolCall {
                tool_call_id: "call-1".to_string(),
                title: "Shell: git log --oneline -5".to_string(),
                kind: AcpToolKind::Other,
                status: AcpToolCallStatus::Completed,
                content: Vec::new(),
                locations: Vec::new(),
                raw_input: None,
                raw_output: Some("first line\nsecond line".into()),
            },
            ToolActivityRenderMode::Compact,
        );
        let rendered_plain = item
            .render_lines(80, palette)
            .iter()
            .map(line_to_plain_text)
            .collect::<Vec<_>>();

        assert_eq!(
            rendered_plain,
            vec![
                "● git log --oneline -5".to_string(),
                "  └─ first line".to_string(),
                "    second line".to_string(),
            ]
        );
    }

    #[test]
    fn acp_tool_call_terminal_content_renders_unsupported_notice() {
        let item = ToolResultItem::from_acp_tool_call(
            AcpToolCall {
                tool_call_id: "call-terminal".to_string(),
                title: "Run tests".to_string(),
                kind: AcpToolKind::Execute,
                status: AcpToolCallStatus::InProgress,
                content: vec![AcpToolCallContent::Terminal {
                    terminal_id: "term-1".to_string(),
                }],
                locations: Vec::new(),
                raw_input: None,
                raw_output: None,
            },
            ToolActivityRenderMode::Compact,
        );

        let plain = item
            .render_lines(80, default_palette())
            .iter()
            .map(line_to_plain_text)
            .collect::<Vec<_>>()
            .join("\n");

        assert!(plain.contains("ACP terminal unavailable"));
        assert!(plain.contains("term-1"));
        assert!(plain.contains("terminal/create unsupported"));
    }

    #[test]
    fn acp_tool_call_raw_output_uses_secondary_color_and_codex_like_alignment() {
        let palette = default_palette();
        let item = ToolResultItem::from_acp_tool_call(
            AcpToolCall {
                tool_call_id: "call-1".to_string(),
                title: "Shell: cargo check".to_string(),
                kind: AcpToolKind::Other,
                status: AcpToolCallStatus::Completed,
                content: Vec::new(),
                locations: Vec::new(),
                raw_input: None,
                raw_output: Some(
                    "Checking lumos v0.1.0 (/home/archie/GoCodes/lumos_rust)\nFinished `dev` profile [unoptimized + debuginfo] target(s) in 1.01s"
                        .into(),
                ),
            },
            ToolActivityRenderMode::Compact,
        );
        let lines = item.render_lines(120, palette);
        let rendered_plain = lines.iter().map(line_to_plain_text).collect::<Vec<_>>();

        assert_eq!(
            rendered_plain,
            vec![
                "● cargo check".to_string(),
                "  └─ Checking lumos v0.1.0 (/home/archie/GoCodes/lumos_rust)".to_string(),
                "    Finished `dev` profile [unoptimized + debuginfo] target(s) in 1.01s"
                    .to_string(),
            ]
        );
        assert!(
            lines[1]
                .spans
                .iter()
                .skip(1)
                .all(|span| span.style.fg == Some(palette.secondary)),
            "raw output content should use the secondary semantic color: {:?}",
            lines[1].spans
        );
    }

    #[test]
    fn acp_read_tool_call_renders_compact_summary_without_content_details() {
        let item = ToolResultItem::from_acp_tool_call(
            AcpToolCall {
                tool_call_id: "call-1".to_string(),
                title: "ReadFile: Temp.md".to_string(),
                kind: AcpToolKind::Read,
                status: AcpToolCallStatus::Completed,
                content: vec![AcpToolCallContent::Text(
                    "     1  # 临时文件\n     2\n     3  body".to_string(),
                )],
                locations: vec![AcpToolCallLocation {
                    path: "Temp.md".to_string(),
                    line: Some(1),
                }],
                raw_input: Some(r#"{"path":"Temp.md"}"#.into()),
                raw_output: Some("# 临时文件\nbody".into()),
            },
            ToolActivityRenderMode::Compact,
        );
        let rendered_plain = item
            .render_lines(80, default_palette())
            .iter()
            .map(line_to_plain_text)
            .collect::<Vec<_>>();

        assert_eq!(rendered_plain, vec!["● Read Temp.md".to_string()]);
    }

    #[test]
    fn acp_readfile_title_fallback_renders_compact_summary_even_without_read_kind() {
        let item = ToolResultItem::from_acp_tool_call(
            AcpToolCall {
                tool_call_id: "call-1".to_string(),
                title: "ReadFile: Temp.md".to_string(),
                kind: AcpToolKind::Other,
                status: AcpToolCallStatus::Completed,
                content: vec![AcpToolCallContent::Text(
                    "     1  # 临时文件\n     2\n     3  body".to_string(),
                )],
                locations: Vec::new(),
                raw_input: Some(r#"{"path":"Temp.md"}"#.into()),
                raw_output: Some("# 临时文件\nbody".into()),
            },
            ToolActivityRenderMode::Compact,
        );
        let rendered_plain = item
            .render_lines(80, default_palette())
            .iter()
            .map(line_to_plain_text)
            .collect::<Vec<_>>();

        assert_eq!(rendered_plain, vec!["● Read Temp.md".to_string()]);
    }

    #[test]
    fn acp_writefile_in_progress_suppresses_raw_input_and_uses_compact_title() {
        let item = ToolResultItem::from_acp_tool_call(
            AcpToolCall {
                tool_call_id: "call-1".to_string(),
                title: "WriteFile: TEMP.md".to_string(),
                kind: AcpToolKind::Other,
                status: AcpToolCallStatus::InProgress,
                content: Vec::new(),
                locations: Vec::new(),
                raw_input: Some(
                    r##"{"path":"TEMP.md","content":"# TEMP\n\nraw transport content"}"##.into(),
                ),
                raw_output: None,
            },
            ToolActivityRenderMode::Compact,
        );
        let lines = item.render_lines(80, default_palette());
        let rendered_plain = lines.iter().map(line_to_plain_text).collect::<Vec<_>>();

        assert_eq!(rendered_plain, vec!["● Write TEMP.md".to_string()]);
        assert!(
            lines[0].spans[0].style.fg == Some(default_palette().main),
            "active write calls should render the marker with the main text color: {:?}",
            lines[0].spans[0]
        );
        assert!(
            rendered_plain
                .iter()
                .all(|line| !line.contains("\"path\"") && !line.contains("\"content\"")),
            "write calls should not expose raw transport JSON in the main transcript: {rendered_plain:?}"
        );
    }

    #[test]
    fn active_acp_write_marker_blinks_by_disappearing_with_main_text_color() {
        let item = ToolResultItem::from_acp_tool_call(
            AcpToolCall {
                tool_call_id: "call-1".to_string(),
                title: "WriteFile: TEMP.md".to_string(),
                kind: AcpToolKind::Other,
                status: AcpToolCallStatus::InProgress,
                content: Vec::new(),
                locations: Vec::new(),
                raw_input: Some(r##"{"path":"TEMP.md","content":"body"}"##.into()),
                raw_output: None,
            },
            ToolActivityRenderMode::Compact,
        );
        let palette = default_palette();
        let started_at = item
            .active_marker_started_at()
            .expect("active tool call should record a blink start");
        let visible = item.render_lines_at(80, palette, started_at);
        let hidden = item.render_lines_at(
            80,
            palette,
            started_at + TOOL_ACTIVITY_ACTIVE_MARKER_BLINK_INTERVAL,
        );

        assert_eq!(line_to_plain_text(&visible[0]), "● Write TEMP.md");
        assert_eq!(line_to_plain_text(&hidden[0]), "  Write TEMP.md");
        assert_eq!(visible[0].spans[0].style.fg, Some(palette.main));
        assert!(
            !visible[0].spans[0]
                .style
                .add_modifier
                .contains(Modifier::RAPID_BLINK),
            "active marker should blink through app rendering, not terminal blink modifier"
        );
    }

    #[test]
    fn acp_tool_call_diff_context_lines_keep_default_style() {
        let palette = default_palette();
        let item = ToolResultItem::from_acp_tool_call(
            AcpToolCall {
                tool_call_id: "call-1".to_string(),
                title: "WriteFile: src/lib.rs".to_string(),
                kind: AcpToolKind::Edit,
                status: AcpToolCallStatus::Completed,
                content: vec![AcpToolCallContent::Diff {
                    path: "src/lib.rs".to_string(),
                    old_text: Some("one\nold\ntail\n".to_string()),
                    new_text: "one\nnew\ntail\n".to_string(),
                }],
                locations: Vec::new(),
                raw_input: None,
                raw_output: None,
            },
            ToolActivityRenderMode::Detailed,
        );
        let lines = item.render_lines(80, palette);
        let context_line = lines
            .iter()
            .find(|line| line_to_plain_text(line).contains(" one"))
            .expect("context line should be rendered");
        let insert_line = lines
            .iter()
            .find(|line| line_to_plain_text(line).contains("+  new"))
            .expect("insert line should be rendered");
        let delete_line = lines
            .iter()
            .find(|line| line_to_plain_text(line).contains("-  old"))
            .expect("delete line should be rendered");

        assert_eq!(context_line.style.bg, None);
        assert!(
            context_line
                .spans
                .iter()
                .all(|span| span.style.bg.is_none() && span.style.fg.is_none()),
            "context diff spans should keep default styling like codex-rs: {context_line:?}"
        );
        assert!(insert_line.style.bg.is_some());
        assert!(delete_line.style.bg.is_some());
    }

    #[test]
    fn acp_tool_call_added_diff_uses_codex_like_header_and_line_numbers() {
        let palette = default_palette();
        let absolute_path = std::env::current_dir()
            .expect("cwd should be available")
            .join("temp.md")
            .display()
            .to_string();
        let new_text = (1..=25)
            .map(|line| format!("line {line}"))
            .collect::<Vec<_>>()
            .join("\n");
        let item = ToolResultItem::from_acp_tool_call(
            AcpToolCall {
                tool_call_id: "call-1".to_string(),
                title: "WriteFile: temp.md".to_string(),
                kind: AcpToolKind::Edit,
                status: AcpToolCallStatus::Completed,
                content: vec![AcpToolCallContent::Diff {
                    path: absolute_path,
                    old_text: None,
                    new_text,
                }],
                locations: Vec::new(),
                raw_input: None,
                raw_output: None,
            },
            ToolActivityRenderMode::Compact,
        );
        let lines = item.render_lines(120, palette);
        let rendered_plain = lines.iter().map(line_to_plain_text).collect::<Vec<_>>();

        assert_eq!(rendered_plain[0], "● Added temp.md (+25 -0)");
        assert!(
            rendered_plain
                .iter()
                .all(|line| !line.contains("WriteFile") && !line.contains("Diff:")),
            "diff rendering should not expose redundant tool or diff labels: {rendered_plain:?}"
        );
        assert!(
            rendered_plain
                .iter()
                .any(|line| line == "      1 +  line 1"),
            "diff lines should right-align line numbers in a seven-column gutter: {rendered_plain:?}"
        );
        assert!(
            rendered_plain
                .iter()
                .any(|line| line == "     25 +  line 25"),
            "compact diff should keep the tail lines: {rendered_plain:?}"
        );
        assert!(
            rendered_plain
                .iter()
                .any(|line| line == "      ⋮ +15 lines (ctrl + t to view transcript)"),
            "compact diff omitted hint should align with the number gutter edge: {rendered_plain:?}"
        );
        assert!(
            !rendered_plain
                .iter()
                .any(|line| line.contains("13 +line 13")),
            "compact mode should omit middle diff rows: {rendered_plain:?}"
        );
    }

    #[test]
    fn acp_tool_call_detailed_diff_keeps_all_rows() {
        let item = ToolResultItem::from_acp_tool_call(
            AcpToolCall {
                tool_call_id: "call-1".to_string(),
                title: "WriteFile: temp.md".to_string(),
                kind: AcpToolKind::Edit,
                status: AcpToolCallStatus::Completed,
                content: vec![AcpToolCallContent::Diff {
                    path: "temp.md".to_string(),
                    old_text: None,
                    new_text: (1..=25)
                        .map(|line| format!("line {line}"))
                        .collect::<Vec<_>>()
                        .join("\n"),
                }],
                locations: Vec::new(),
                raw_input: None,
                raw_output: None,
            },
            ToolActivityRenderMode::Detailed,
        );
        let rendered_plain = item
            .render_lines(120, default_palette())
            .iter()
            .map(line_to_plain_text)
            .collect::<Vec<_>>();

        assert!(
            rendered_plain
                .iter()
                .any(|line| line == "     13 +  line 13"),
            "detailed mode should keep middle diff rows: {rendered_plain:?}"
        );
        assert!(
            !rendered_plain
                .iter()
                .any(|line| line.contains("ctrl + t to view transcript")),
            "detailed mode should not render compact truncation hints: {rendered_plain:?}"
        );
    }

    #[test]
    fn acp_tool_call_updated_diff_renders_delete_and_insert_line_numbers() {
        let item = ToolResultItem::from_acp_tool_call(
            AcpToolCall {
                tool_call_id: "call-1".to_string(),
                title: "WriteFile: src/lib.rs".to_string(),
                kind: AcpToolKind::Edit,
                status: AcpToolCallStatus::Completed,
                content: vec![AcpToolCallContent::Diff {
                    path: "src/lib.rs".to_string(),
                    old_text: Some("one\nold\ntail\n".to_string()),
                    new_text: "one\nnew\ntail\n".to_string(),
                }],
                locations: Vec::new(),
                raw_input: None,
                raw_output: None,
            },
            ToolActivityRenderMode::Detailed,
        );
        let rendered_plain = item
            .render_lines(120, default_palette())
            .iter()
            .map(line_to_plain_text)
            .collect::<Vec<_>>();

        assert_eq!(rendered_plain[0], "● Edited src/lib.rs (+1 -1)");
        assert!(
            rendered_plain.iter().any(|line| line == "      2 -  old"),
            "updated diff should render old line numbers for deletions: {rendered_plain:?}"
        );
        assert!(
            rendered_plain.iter().any(|line| line == "      2 +  new"),
            "updated diff should render new line numbers for insertions: {rendered_plain:?}"
        );
        assert!(
            rendered_plain.iter().any(|line| line == "      1    one"),
            "context diff rows should right-align the line number and align content after the sign column: {rendered_plain:?}"
        );
        assert!(
            rendered_plain
                .iter()
                .all(|line| !line.contains("---") && !line.contains("+++")),
            "updated diff should not expose raw unified diff file headers: {rendered_plain:?}"
        );
    }

    #[test]
    fn acp_tool_call_diff_right_aligns_three_digit_line_numbers_in_fixed_gutter() {
        let item = ToolResultItem::from_acp_tool_call(
            AcpToolCall {
                tool_call_id: "call-1".to_string(),
                title: "WriteFile: temp.md".to_string(),
                kind: AcpToolKind::Edit,
                status: AcpToolCallStatus::Completed,
                content: vec![AcpToolCallContent::Diff {
                    path: "temp.md".to_string(),
                    old_text: None,
                    new_text: (1..=267)
                        .map(|line| format!("line {line}"))
                        .collect::<Vec<_>>()
                        .join("\n"),
                }],
                locations: Vec::new(),
                raw_input: None,
                raw_output: None,
            },
            ToolActivityRenderMode::Detailed,
        );
        let rendered_plain = item
            .render_lines(120, default_palette())
            .iter()
            .map(line_to_plain_text)
            .collect::<Vec<_>>();

        assert!(
            rendered_plain
                .iter()
                .any(|line| line == "    267 +  line 267"),
            "three-digit line numbers should grow left within the fixed seven-column gutter: {rendered_plain:?}"
        );
    }

    #[test]
    fn naked_shell_result_highlights_command() {
        let palette = default_palette();
        let item = ToolResultItem::new("Ran sed -n '1,80p' src/main.rs", ToolResultKind::Ran);
        let lines = item.render_lines(80, palette);

        assert_eq!(
            line_to_plain_text(&lines[0]),
            "● Ran sed -n '1,80p' src/main.rs"
        );
        assert_eq!(lines[0].spans[0].style.fg, Some(palette.quote));
        assert_eq!(lines[0].spans[1].content.as_ref(), "Ran");
        assert!(lines[0].spans[1].style.fg.is_none());
        assert!(
            lines[0]
                .spans
                .iter()
                .skip(2)
                .any(|span| span.style.fg.is_some()),
            "naked shell command spans should carry syntax highlight foreground colors: {:?}",
            lines[0].spans
        );
    }

    #[test]
    fn wrapped_shell_result_uses_continuation_prefix_and_keeps_highlight() {
        let item = ToolResultItem::new(
            "Ran sed -n '1,80p' src/frontend/tui/tool_result.rs",
            ToolResultKind::Ran,
        );
        let lines = item.render_lines(18, default_palette());
        let plain_lines = lines.iter().map(line_to_plain_text).collect::<Vec<_>>();

        assert!(
            plain_lines.len() > 1,
            "shell command should wrap in a narrow viewport: {plain_lines:?}"
        );
        assert!(
            plain_lines[0].starts_with("● Ran "),
            "first shell line should keep the status prefix and verb: {plain_lines:?}"
        );
        assert!(
            plain_lines[1..].iter().all(|line| line.starts_with("  ")),
            "wrapped shell continuation lines should use two leading spaces: {plain_lines:?}"
        );
        assert!(
            lines
                .iter()
                .flat_map(|line| line.spans.iter().skip(1))
                .any(|span| span.style.fg.is_some()),
            "wrapped shell command spans should keep syntax highlight foreground colors: {lines:?}"
        );
    }

    #[test]
    fn wrapped_result_uses_two_space_continuation_prefix() {
        let item = ToolResultItem::new("Ran Very-long-command", ToolResultKind::Ran);
        let lines = item
            .render_lines(10, default_palette())
            .into_iter()
            .map(|line| line_to_plain_text(&line))
            .collect::<Vec<_>>();

        assert_eq!(
            lines,
            vec![
                "● Very-lon".to_string(),
                "  g-comman".to_string(),
                "  d".to_string(),
            ]
        );
    }

    #[test]
    fn terminal_default_palette_keeps_reset_style_plain() {
        let item = ToolResultItem::new("Ran echo ok", ToolResultKind::Ran);
        let line = item.render_lines(80, terminal_default_palette()).remove(0);

        assert_eq!(
            line.spans[0].style.fg,
            Some(ratatui::style::Color::LightGreen)
        );
    }
}
