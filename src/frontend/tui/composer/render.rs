use ratatui::text::{Line, Span};

use super::{
    Composer,
    grapheme::{grapheme_clusters, is_space_cluster, measure_width},
    layout::{VisualLine, placeholder_visual_lines_for_text, visual_lines_for_text},
    viewport::calculate_cursor_visual_position,
};
use crate::frontend::tui::theme::{TerminalPalette, muted_text_style, secondary_text_style};

#[cfg(test)]
use super::viewport::visible_viewport_lines;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct LineAnchor {
    pub(crate) logical_line: usize,
    pub(crate) visible_start_char: usize,
    pub(crate) end_char: usize,
}

#[cfg(test)]
#[derive(Debug, Clone)]
pub(crate) struct RenderResult {
    pub(crate) lines: Vec<Line<'static>>,
    pub(crate) cursor_x: u16,
    pub(crate) cursor_y: u16,
}

#[derive(Debug, Clone)]
pub(crate) struct DocumentRenderResult {
    pub(crate) lines: Vec<Line<'static>>,
    pub(crate) plain_lines: Vec<String>,
    pub(crate) anchors: Vec<LineAnchor>,
    pub(crate) cursor_x: u16,
    pub(crate) cursor_y: usize,
}

#[cfg(test)]
pub(crate) fn render(composer: &Composer, palette: TerminalPalette) -> RenderResult {
    let document = render_document(composer, palette);
    let (visible_lines, resolved_offset) = visible_viewport_lines(
        &document.lines,
        composer.viewport_offset(),
        composer.viewport_height(),
    );
    let max_cursor_y = visible_lines.len().saturating_sub(1);
    let cursor_y = document
        .cursor_y
        .saturating_sub(resolved_offset)
        .min(max_cursor_y);

    RenderResult {
        lines: visible_lines.to_vec(),
        cursor_x: document.cursor_x,
        cursor_y: u16::try_from(cursor_y).unwrap_or(u16::MAX),
    }
}

pub(crate) fn render_document(
    composer: &Composer,
    palette: TerminalPalette,
) -> DocumentRenderResult {
    let prompt_width = measure_width(composer.prompt());
    let prompt_style = secondary_text_style(palette);
    let text_style = muted_text_style(palette);
    let placeholder_style = secondary_text_style(palette);

    if composer.value().is_empty() {
        let placeholder_lines = placeholder_visual_lines_for_text(
            composer.placeholder(),
            composer.content_width(),
            prompt_width,
        );
        let plain_lines = rendered_plain_lines(
            &placeholder_lines,
            composer.prompt(),
            prompt_width,
            composer.content_width(),
            true,
        );
        let lines = render_visual_lines(
            &placeholder_lines,
            composer.prompt(),
            prompt_width,
            prompt_style,
            placeholder_style,
            composer.content_width(),
            true,
        );

        return DocumentRenderResult {
            anchors: line_anchors_for_visual_lines(&placeholder_lines),
            lines,
            plain_lines,
            cursor_x: u16::try_from(prompt_width).unwrap_or(u16::MAX),
            cursor_y: 0,
        };
    }

    let visual_lines =
        visual_lines_for_text(composer.value(), composer.content_width(), prompt_width);
    let (row, column) = composer.cursor_position();
    let (cursor_y, cursor_visual_x) =
        calculate_cursor_visual_position(&visual_lines, row, column, prompt_width);
    DocumentRenderResult {
        anchors: line_anchors_for_visual_lines(&visual_lines),
        plain_lines: rendered_plain_lines(
            &visual_lines,
            composer.prompt(),
            prompt_width,
            composer.content_width(),
            false,
        ),
        lines: render_visual_lines(
            &visual_lines,
            composer.prompt(),
            prompt_width,
            prompt_style,
            text_style,
            composer.content_width(),
            false,
        ),
        cursor_x: u16::try_from(cursor_visual_x).unwrap_or(u16::MAX),
        cursor_y,
    }
}

fn render_visual_lines(
    lines: &[VisualLine],
    prompt: &str,
    prompt_width: usize,
    prompt_style: ratatui::style::Style,
    text_style: ratatui::style::Style,
    content_width: usize,
    trim_overflow_spaces: bool,
) -> Vec<Line<'static>> {
    lines
        .iter()
        .map(|line| {
            let (line_prefix, line_text) = rendered_line_parts(
                line,
                prompt,
                prompt_width,
                content_width,
                trim_overflow_spaces,
            );
            Line::default().spans([
                Span::styled(line_prefix, prompt_style),
                Span::styled(line_text, text_style),
            ])
        })
        .collect()
}

fn rendered_plain_lines(
    lines: &[VisualLine],
    prompt: &str,
    prompt_width: usize,
    content_width: usize,
    trim_overflow_spaces: bool,
) -> Vec<String> {
    lines
        .iter()
        .map(|line| {
            let (line_prefix, line_text) = rendered_line_parts(
                line,
                prompt,
                prompt_width,
                content_width,
                trim_overflow_spaces,
            );
            format!("{line_prefix}{line_text}")
        })
        .collect()
}

fn line_anchors_for_visual_lines(lines: &[VisualLine]) -> Vec<LineAnchor> {
    let anchors = lines
        .iter()
        .map(|line| LineAnchor {
            logical_line: line.logical_line,
            visible_start_char: line.visible_start_char,
            end_char: line.end_char,
        })
        .collect::<Vec<_>>();

    if anchors.is_empty() {
        vec![LineAnchor::default()]
    } else {
        anchors
    }
}

fn rendered_line_parts(
    line: &VisualLine,
    prompt: &str,
    prompt_width: usize,
    content_width: usize,
    trim_overflow_spaces: bool,
) -> (String, String) {
    let line_prefix = if line.is_continuation {
        " ".repeat(prompt_width)
    } else {
        prompt.to_string()
    };
    let line_text = if trim_overflow_spaces {
        trim_overflow_boundary_spaces(&line.text, content_width)
    } else {
        line.text.clone()
    };

    (line_prefix, line_text)
}

fn trim_overflow_boundary_spaces(text: &str, width: usize) -> String {
    if width == 0 || measure_width(text) <= width {
        return text.to_string();
    }

    let mut graphemes = grapheme_clusters(text);
    let mut total_width = graphemes.iter().map(|cluster| cluster.width).sum::<usize>();

    while total_width > width {
        let Some(last) = graphemes.last() else {
            break;
        };
        if !is_space_cluster(last.text) {
            break;
        }

        total_width = total_width.saturating_sub(last.width);
        graphemes.pop();
    }

    graphemes
        .into_iter()
        .map(|cluster| cluster.text)
        .collect::<String>()
}
