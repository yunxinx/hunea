use ratatui::{
    style::Style,
    text::{Line, Span},
};
use unicode_segmentation::UnicodeSegmentation;

use super::{
    Model,
    display_width::{display_width, grapheme_width},
    selection::{SelectableLineRange, selectable_range_for_plain_line},
    styled_text::line_to_plain_text,
    theme::TerminalPalette,
};

#[derive(Debug, Clone, Default)]
pub(crate) struct InlinePanelRenderResult {
    pub(crate) lines: Vec<Line<'static>>,
    pub(crate) plain_lines: Vec<String>,
    pub(crate) selectable: Vec<SelectableLineRange>,
    pub(crate) has_content: bool,
}

pub(crate) fn inline_panel_rule_line(width: usize, palette: TerminalPalette) -> Line<'static> {
    super::theme::accent_rule_line(width, palette)
}

pub(crate) fn inline_panel_visible_rows(model: &Model, fallback_rows: usize) -> usize {
    let viewport_height = model.document_viewport_height();
    if viewport_height == 0 {
        return fallback_rows;
    }

    let mut available_rows =
        viewport_height.saturating_sub(usize::from(model.composer.full_height()));
    if model.composer_uses_rendered_frame_padding() {
        available_rows = available_rows.saturating_sub(1);
    }

    fallback_rows.min(available_rows.max(1))
}

pub(crate) fn inline_panel_render_result(lines: Vec<Line<'static>>) -> InlinePanelRenderResult {
    let plain_lines = lines.iter().map(line_to_plain_text).collect::<Vec<_>>();
    let selectable = plain_lines
        .iter()
        .map(|line| selectable_range_for_plain_line(line))
        .collect::<Vec<_>>();

    InlinePanelRenderResult {
        lines,
        plain_lines,
        selectable,
        has_content: true,
    }
}

pub(crate) fn append_wrapped_inline_value(
    lines: &mut Vec<Line<'static>>,
    width: usize,
    prefix: &str,
    value: &str,
    value_style: Style,
    prefix_style: Style,
) {
    let prefix_width = display_width(prefix);
    let available_width = width.saturating_sub(2 + prefix_width).max(1);
    let wrapped = wrap_inline_text(value, available_width);
    if wrapped.is_empty() {
        lines.push(Line::styled(format!("  {prefix}"), prefix_style));
        return;
    }

    lines.push(Line::from(vec![
        Span::raw("  "),
        Span::styled(prefix.to_string(), prefix_style),
        Span::styled(wrapped[0].clone(), value_style),
    ]));
    let continuation_prefix = " ".repeat(2 + prefix_width);
    for line in wrapped.iter().skip(1) {
        lines.push(Line::from(vec![
            Span::raw(continuation_prefix.clone()),
            Span::styled(line.clone(), value_style),
        ]));
    }
}

pub(crate) fn wrap_inline_text(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    if text.is_empty() {
        return Vec::new();
    }

    let mut lines = Vec::new();
    let mut current = String::new();
    let mut current_width = 0usize;
    for grapheme in UnicodeSegmentation::graphemes(text, true) {
        let cluster_width = grapheme_width(grapheme);
        if current_width > 0 && current_width + cluster_width > width {
            lines.push(current.trim_end().to_string());
            current.clear();
            current_width = 0;
        }
        if current_width == 0 && grapheme.chars().all(char::is_whitespace) {
            continue;
        }
        current.push_str(grapheme);
        current_width += cluster_width;
    }

    if !current.is_empty() {
        lines.push(current.trim_end().to_string());
    }

    lines
}
