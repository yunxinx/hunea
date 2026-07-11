use std::{cell::RefCell, rc::Rc};

use ratatui::{
    style::Style,
    text::{Line, Span},
};
use runtime_domain::session::{TranscriptCustomPromptBinding, TranscriptSkillBinding};

use super::{
    Composer,
    grapheme::{grapheme_clusters, is_space_cluster, measure_width},
    layout::{ComposerLayoutSnapshot, VisualLine, placeholder_visual_lines_for_text},
};
use crate::{
    selection::SelectableLineRange,
    style_mode::StyleMode,
    theme::{
        TerminalPalette, primary_text_style, secondary_text_style, surface_text_style,
        tertiary_text_style,
    },
};

#[cfg(test)]
use super::viewport::{calculate_cursor_visual_position, visible_viewport_lines};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct LineAnchor {
    pub(crate) logical_line: usize,
    pub(crate) visible_start_char: usize,
    pub(crate) end_char: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum FrameDecorationMode {
    #[default]
    None,
    Surface,
    Rule,
}

#[cfg(test)]
#[derive(Debug, Clone)]
pub(crate) struct RenderResult {
    pub(crate) lines: Vec<Line<'static>>,
    pub(crate) cursor_x: u16,
    pub(crate) cursor_y: u16,
}

#[cfg(test)]
#[derive(Debug, Clone)]
pub(crate) struct DocumentRenderResult {
    pub(crate) lines: Vec<Line<'static>>,
    pub(crate) plain_lines: Vec<String>,
    pub(crate) anchors: Vec<LineAnchor>,
    pub(crate) selectable_ranges: Vec<SelectableLineRange>,
    pub(crate) frame_decoration_top_line: Option<Line<'static>>,
    pub(crate) frame_decoration_bottom_line: Option<Line<'static>>,
    pub(crate) cursor_x: u16,
    pub(crate) cursor_y: usize,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct ComposerDocumentRange {
    pub(crate) lines: Vec<Line<'static>>,
    pub(crate) plain_lines: Vec<String>,
    pub(crate) selectable_ranges: Vec<SelectableLineRange>,
}

#[derive(Debug, Default)]
struct ComposerMaterializedWindow {
    start_line: usize,
    range: ComposerDocumentRange,
}

/// `ComposerDocumentSnapshot` 为一份immutable geometry提供有界的styled row窗口。
#[derive(Debug)]
pub(crate) struct ComposerDocumentSnapshot {
    layout: Rc<ComposerLayoutSnapshot>,
    prompt: String,
    prompt_width: usize,
    content_width: usize,
    frame_fill_width: usize,
    prompt_first_line_only: bool,
    prompt_style: Style,
    text_style: Style,
    placeholder_style: Style,
    highlighted_text_style: Style,
    fill_style: Style,
    skill_bindings: Vec<TranscriptSkillBinding>,
    custom_prompt_bindings: Vec<TranscriptCustomPromptBinding>,
    image_attachment_ranges: Vec<(usize, usize)>,
    placeholder: Option<String>,
    plain_text_len: usize,
    pub(crate) frame_decoration_top_line: Option<Line<'static>>,
    pub(crate) frame_decoration_top_plain_line: Option<String>,
    pub(crate) frame_decoration_bottom_line: Option<Line<'static>>,
    pub(crate) frame_decoration_bottom_plain_line: Option<String>,
    materialized: RefCell<ComposerMaterializedWindow>,
}

impl ComposerDocumentSnapshot {
    pub(crate) fn new(composer: &Composer, palette: TerminalPalette) -> Self {
        let prompt_width = measure_width(composer.prompt());
        let frame_width = usize::from(composer.width.max(1));
        let frame_mode = frame_decoration_mode(composer.style_mode());
        let prompt_first_line_only = matches!(composer.style_mode(), StyleMode::Cx | StyleMode::Cc);
        let mut prompt_style = if matches!(composer.style_mode(), StyleMode::Cx) {
            primary_text_style(palette)
        } else {
            secondary_text_style(palette)
        };
        let mut text_style = primary_text_style(palette);
        let mut placeholder_style = secondary_text_style(palette);
        let mut fill_style = Style::default();
        let mut frame_fill_width = 0;
        let frame_decoration = frame_decoration_lines(frame_mode, palette, frame_width);
        if matches!(frame_mode, FrameDecorationMode::Surface) && palette.surface.is_some() {
            frame_fill_width = frame_width;
            fill_style = surface_text_style(palette);
            prompt_style = apply_optional_background(prompt_style, palette);
            text_style = apply_optional_background(text_style, palette);
            placeholder_style = apply_optional_background(placeholder_style, palette);
        }
        let highlighted_text_style = text_style.fg(palette.command_accent);

        let layout = composer.layout_snapshot();
        let placeholder = composer
            .value()
            .is_empty()
            .then(|| composer.placeholder().to_string());
        let plain_text_len = composer_plain_text_len(
            layout.visual_lines(),
            placeholder.as_deref(),
            composer.prompt(),
            prompt_width,
            composer.content_width(),
            frame_fill_width,
            prompt_first_line_only,
        );

        Self {
            layout,
            prompt: composer.prompt().to_string(),
            prompt_width,
            content_width: composer.content_width(),
            frame_fill_width,
            prompt_first_line_only,
            prompt_style,
            text_style,
            placeholder_style,
            highlighted_text_style,
            fill_style,
            skill_bindings: composer.skill_bindings.clone(),
            custom_prompt_bindings: composer.custom_prompt_bindings.clone(),
            image_attachment_ranges: composer.image_attachment_highlight_ranges(),
            placeholder,
            plain_text_len,
            frame_decoration_top_line: frame_decoration.top_line,
            frame_decoration_top_plain_line: frame_decoration.top_plain_line,
            frame_decoration_bottom_line: frame_decoration.bottom_line,
            frame_decoration_bottom_plain_line: frame_decoration.bottom_plain_line,
            materialized: RefCell::new(ComposerMaterializedWindow::default()),
        }
    }

    pub(crate) fn line_count(&self) -> usize {
        self.layout.line_count()
    }

    /// 返回所有 visual line 的内容字节数，不包含行间换行分隔符。
    pub(crate) fn text_bytes_len(&self) -> usize {
        self.plain_text_len
            .saturating_sub(self.line_count().saturating_sub(1))
    }

    pub(crate) fn anchor_at(&self, index: usize) -> Option<LineAnchor> {
        self.layout
            .visual_lines()
            .get(index)
            .map(|line| LineAnchor {
                logical_line: line.logical_line,
                visible_start_char: line.visible_start_char,
                end_char: line.end_char,
            })
    }

    pub(crate) fn line_index_for_anchor(&self, target: LineAnchor) -> Option<usize> {
        self.layout.visual_lines().iter().position(|line| {
            line.logical_line == target.logical_line
                && line.visible_start_char == target.visible_start_char
                && line.end_char == target.end_char
        })
    }

    pub(crate) fn visual_position_for_char(&self, char_index: usize) -> Option<(u16, usize)> {
        let lines = self.layout.visual_lines();
        let mut fallback = None;
        for (index, line) in lines.iter().enumerate() {
            let start = line.logical_line_start_char + line.visible_start_char;
            let end = line.logical_line_start_char + line.end_char;
            if char_index < start {
                continue;
            }
            fallback = Some((index, line));
            if char_index <= end {
                let relative = char_index.saturating_sub(start);
                let width = measure_width(&line.text.chars().take(relative).collect::<String>());
                return Some((
                    u16::try_from(self.prompt_width + width).unwrap_or(u16::MAX),
                    index,
                ));
            }
        }
        fallback.map(|(index, line)| {
            (
                u16::try_from(self.prompt_width + measure_width(&line.text)).unwrap_or(u16::MAX),
                index,
            )
        })
    }

    pub(crate) fn range(&self, start: usize, count: usize) -> ComposerDocumentRange {
        if count == 0 || start >= self.line_count() {
            return ComposerDocumentRange::default();
        }
        let count = count.min(self.line_count() - start);
        let overscan = crate::transcript::viewport_overscan_line_budget(count);
        let window_start = start.saturating_sub(overscan);
        let window_end = (start + count + overscan).min(self.line_count());

        let needs_refresh = {
            let cached = self.materialized.borrow();
            start < cached.start_line
                || start + count > cached.start_line + cached.range.lines.len()
        };
        if needs_refresh {
            *self.materialized.borrow_mut() = ComposerMaterializedWindow {
                start_line: window_start,
                range: self.materialize(window_start, window_end - window_start),
            };
        }

        let cached = self.materialized.borrow();
        let local_start = start - cached.start_line;
        let local_end = local_start + count;
        ComposerDocumentRange {
            lines: cached.range.lines[local_start..local_end].to_vec(),
            plain_lines: cached.range.plain_lines[local_start..local_end].to_vec(),
            selectable_ranges: cached.range.selectable_ranges[local_start..local_end].to_vec(),
        }
    }

    fn materialize(&self, start: usize, count: usize) -> ComposerDocumentRange {
        let end = (start + count).min(self.line_count());
        let visual_lines = &self.layout.visual_lines()[start..end];
        let placeholder_lines;
        let (visual_lines, text_style, highlighted_text_style, trim_overflow_spaces) =
            if let Some(placeholder) = self.placeholder.as_ref() {
                placeholder_lines = vec![first_placeholder_visual_line(
                    placeholder,
                    self.content_width,
                    self.prompt_width,
                )];
                (
                    placeholder_lines.as_slice(),
                    self.placeholder_style,
                    self.placeholder_style,
                    true,
                )
            } else {
                (
                    visual_lines,
                    self.text_style,
                    self.highlighted_text_style,
                    false,
                )
            };

        ComposerDocumentRange {
            plain_lines: rendered_plain_lines(
                visual_lines,
                PlainVisualRenderOptions {
                    prompt: &self.prompt,
                    prompt_width: self.prompt_width,
                    content_width: self.content_width,
                    frame_fill_width: self.frame_fill_width,
                    prompt_first_line_only: self.prompt_first_line_only,
                    trim_overflow_spaces,
                },
            ),
            selectable_ranges: selectable_ranges_for_visual_lines(
                visual_lines,
                self.prompt_width,
                self.content_width,
                self.frame_fill_width,
                self.prompt_first_line_only,
                trim_overflow_spaces,
            ),
            lines: render_visual_lines(
                visual_lines,
                StyledVisualRenderOptions {
                    prompt: &self.prompt,
                    prompt_width: self.prompt_width,
                    prompt_style: self.prompt_style,
                    text_style,
                    highlighted_text_style,
                    skill_bindings: &self.skill_bindings,
                    custom_prompt_bindings: &self.custom_prompt_bindings,
                    image_attachment_ranges: &self.image_attachment_ranges,
                    content_width: self.content_width,
                    frame_fill_width: self.frame_fill_width,
                    fill_style: self.fill_style,
                    prompt_first_line_only: self.prompt_first_line_only,
                    trim_overflow_spaces,
                },
            ),
        }
    }

    #[cfg(test)]
    pub(crate) fn materialized_window(&self) -> (usize, usize) {
        let window = self.materialized.borrow();
        (window.start_line, window.range.lines.len())
    }
}

fn composer_plain_text_len(
    lines: &[VisualLine],
    placeholder: Option<&str>,
    prompt: &str,
    prompt_width: usize,
    content_width: usize,
    frame_fill_width: usize,
    prompt_first_line_only: bool,
) -> usize {
    if let Some(placeholder) = placeholder {
        let line = first_placeholder_visual_line(placeholder, content_width, prompt_width);
        let text = trim_overflow_boundary_spaces(&line.text, content_width);
        let fill = frame_fill_width.saturating_sub(prompt_width + measure_width(&text));
        return prompt.len() + text.len() + fill;
    }

    let bytes = lines
        .iter()
        .enumerate()
        .map(|(index, line)| {
            let prefix = if line.is_continuation || prompt_first_line_only && index > 0 {
                prompt_width
            } else {
                prompt.len()
            };
            let fill = frame_fill_width.saturating_sub(prompt_width + measure_width(&line.text));
            prefix + line.text.len() + fill
        })
        .sum::<usize>();
    bytes + lines.len().saturating_sub(1)
}

#[derive(Debug, Clone, Copy)]
struct StyledVisualRenderOptions<'a> {
    prompt: &'a str,
    prompt_width: usize,
    prompt_style: ratatui::style::Style,
    text_style: ratatui::style::Style,
    highlighted_text_style: ratatui::style::Style,
    skill_bindings: &'a [TranscriptSkillBinding],
    custom_prompt_bindings: &'a [TranscriptCustomPromptBinding],
    image_attachment_ranges: &'a [(usize, usize)],
    content_width: usize,
    frame_fill_width: usize,
    fill_style: Style,
    prompt_first_line_only: bool,
    trim_overflow_spaces: bool,
}

#[derive(Debug, Clone, Copy)]
struct PlainVisualRenderOptions<'a> {
    prompt: &'a str,
    prompt_width: usize,
    content_width: usize,
    frame_fill_width: usize,
    prompt_first_line_only: bool,
    trim_overflow_spaces: bool,
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

#[cfg(test)]
pub(crate) fn render_document(
    composer: &Composer,
    palette: TerminalPalette,
) -> DocumentRenderResult {
    let prompt_width = measure_width(composer.prompt());
    let frame_width = usize::from(composer.width.max(1));
    let frame_mode = frame_decoration_mode(composer.style_mode());
    let prompt_first_line_only = matches!(composer.style_mode(), StyleMode::Cx | StyleMode::Cc);
    let mut prompt_style = if matches!(composer.style_mode(), StyleMode::Cx) {
        primary_text_style(palette)
    } else {
        secondary_text_style(palette)
    };
    let mut text_style = primary_text_style(palette);
    let mut placeholder_style = secondary_text_style(palette);
    let mut fill_style = Style::default();
    let mut frame_fill_width = 0;
    let frame_decoration = frame_decoration_lines(frame_mode, palette, frame_width);
    if matches!(frame_mode, FrameDecorationMode::Surface) && palette.surface.is_some() {
        frame_fill_width = frame_width;
        fill_style = surface_text_style(palette);
        prompt_style = apply_optional_background(prompt_style, palette);
        text_style = apply_optional_background(text_style, palette);
        placeholder_style = apply_optional_background(placeholder_style, palette);
    }
    let highlighted_text_style = text_style.fg(palette.command_accent);

    if composer.value().is_empty() {
        let placeholder_line = first_placeholder_visual_line(
            composer.placeholder(),
            composer.content_width(),
            prompt_width,
        );
        let placeholder_lines = vec![placeholder_line];
        let plain_lines = rendered_plain_lines(
            &placeholder_lines,
            PlainVisualRenderOptions {
                prompt: composer.prompt(),
                prompt_width,
                content_width: composer.content_width(),
                frame_fill_width,
                prompt_first_line_only,
                trim_overflow_spaces: true,
            },
        );
        let lines = render_visual_lines(
            &placeholder_lines,
            StyledVisualRenderOptions {
                prompt: composer.prompt(),
                prompt_width,
                prompt_style,
                text_style: placeholder_style,
                highlighted_text_style: placeholder_style,
                skill_bindings: &[],
                custom_prompt_bindings: &[],
                image_attachment_ranges: &[],
                content_width: composer.content_width(),
                frame_fill_width,
                fill_style,
                prompt_first_line_only,
                trim_overflow_spaces: true,
            },
        );

        return DocumentRenderResult {
            anchors: line_anchors_for_visual_lines(&placeholder_lines),
            lines,
            plain_lines,
            selectable_ranges: selectable_ranges_for_visual_lines(
                &placeholder_lines,
                prompt_width,
                composer.content_width(),
                frame_fill_width,
                prompt_first_line_only,
                true,
            ),
            frame_decoration_top_line: frame_decoration.top_line,
            frame_decoration_bottom_line: frame_decoration.bottom_line,
            cursor_x: u16::try_from(prompt_width).unwrap_or(u16::MAX),
            cursor_y: 0,
        };
    }

    let snapshot = composer.layout_snapshot();
    let visual_lines = snapshot.visual_lines();
    let image_attachment_ranges = composer.image_attachment_highlight_ranges();
    let (row, column) = composer.cursor_position();
    let (cursor_y, cursor_visual_x) =
        calculate_cursor_visual_position(visual_lines, row, column, prompt_width);
    DocumentRenderResult {
        anchors: line_anchors_for_visual_lines(visual_lines),
        plain_lines: rendered_plain_lines(
            visual_lines,
            PlainVisualRenderOptions {
                prompt: composer.prompt(),
                prompt_width,
                content_width: composer.content_width(),
                frame_fill_width,
                prompt_first_line_only,
                trim_overflow_spaces: false,
            },
        ),
        selectable_ranges: selectable_ranges_for_visual_lines(
            visual_lines,
            prompt_width,
            composer.content_width(),
            frame_fill_width,
            prompt_first_line_only,
            false,
        ),
        lines: render_visual_lines(
            visual_lines,
            StyledVisualRenderOptions {
                prompt: composer.prompt(),
                prompt_width,
                prompt_style,
                text_style,
                highlighted_text_style,
                skill_bindings: &composer.skill_bindings,
                custom_prompt_bindings: &composer.custom_prompt_bindings,
                image_attachment_ranges: &image_attachment_ranges,
                content_width: composer.content_width(),
                frame_fill_width,
                fill_style,
                prompt_first_line_only,
                trim_overflow_spaces: false,
            },
        ),
        frame_decoration_top_line: frame_decoration.top_line,
        frame_decoration_bottom_line: frame_decoration.bottom_line,
        cursor_x: u16::try_from(cursor_visual_x).unwrap_or(u16::MAX),
        cursor_y,
    }
}

fn selectable_ranges_for_visual_lines(
    lines: &[VisualLine],
    prompt_width: usize,
    content_width: usize,
    frame_fill_width: usize,
    prompt_first_line_only: bool,
    trim_overflow_spaces: bool,
) -> Vec<SelectableLineRange> {
    lines
        .iter()
        .enumerate()
        .map(|(index, line)| {
            let line_text = if trim_overflow_spaces {
                trim_overflow_boundary_spaces(&line.text, content_width)
            } else {
                line.text.clone()
            };
            let line_width = measure_width(&line_text);
            if line_width == 0 {
                let hit_end = if frame_fill_width > 0 {
                    frame_fill_width
                } else {
                    prompt_width.max(1)
                };
                return SelectableLineRange::blank_hit_range(0, hit_end);
            }

            let shows_prompt = !(line.is_continuation || prompt_first_line_only && index > 0);
            if shows_prompt {
                SelectableLineRange::with_hit_range(
                    prompt_width,
                    prompt_width + line_width,
                    0,
                    prompt_width + line_width,
                )
            } else {
                SelectableLineRange::new(prompt_width, prompt_width + line_width)
            }
        })
        .collect()
}

fn first_placeholder_visual_line(text: &str, width: usize, line_prefix_width: usize) -> VisualLine {
    placeholder_visual_lines_for_text(text, width, line_prefix_width)
        .into_iter()
        .next()
        .unwrap_or(VisualLine {
            text: String::new(),
            logical_line: 0,
            logical_line_start_char: 0,
            visible_start_char: 0,
            end_char: 0,
            is_continuation: false,
        })
}

fn render_visual_lines(
    lines: &[VisualLine],
    options: StyledVisualRenderOptions<'_>,
) -> Vec<Line<'static>> {
    lines
        .iter()
        .enumerate()
        .map(|(index, line)| {
            let (line_prefix, line_text) = rendered_line_parts(
                line,
                options.prompt,
                options.prompt_width,
                options.content_width,
                options.prompt_first_line_only,
                index,
                options.trim_overflow_spaces,
            );
            let fill_width = options
                .frame_fill_width
                .saturating_sub(options.prompt_width + measure_width(&line_text));
            let mut spans = vec![Span::styled(line_prefix, options.prompt_style)];
            let absolute_visible_start_char =
                line.logical_line_start_char + line.visible_start_char;
            spans.extend(styled_composer_content_spans(
                &line_text,
                absolute_visible_start_char,
                options.text_style,
                options.highlighted_text_style,
                options.skill_bindings,
                options.custom_prompt_bindings,
                options.image_attachment_ranges,
            ));
            spans.push(Span::styled(" ".repeat(fill_width), options.fill_style));
            Line::from(spans)
        })
        .collect()
}

fn styled_composer_content_spans(
    text: &str,
    visible_start_char: usize,
    text_style: Style,
    highlighted_text_style: Style,
    skill_bindings: &[TranscriptSkillBinding],
    custom_prompt_bindings: &[TranscriptCustomPromptBinding],
    image_attachment_ranges: &[(usize, usize)],
) -> Vec<Span<'static>> {
    if text.is_empty() {
        return vec![Span::styled(String::new(), text_style)];
    }
    let binding_ranges = visible_binding_ranges(
        skill_bindings,
        custom_prompt_bindings,
        image_attachment_ranges,
        visible_start_char,
        text,
    );
    if binding_ranges.is_empty() {
        return vec![Span::styled(text.to_string(), text_style)];
    }

    let mut spans = Vec::new();
    let mut cursor = 0usize;

    for (relative_start, relative_end) in binding_ranges {
        if cursor < relative_start {
            spans.push(Span::styled(
                slice_char_range(text, cursor, relative_start),
                text_style,
            ));
        }
        spans.push(Span::styled(
            slice_char_range(text, relative_start, relative_end),
            highlighted_text_style,
        ));
        cursor = relative_end;
    }

    let line_char_len = text.chars().count();
    if cursor < line_char_len {
        spans.push(Span::styled(
            slice_char_range(text, cursor, line_char_len),
            text_style,
        ));
    }

    if spans.is_empty() {
        spans.push(Span::styled(text.to_string(), text_style));
    }
    spans
}

fn visible_binding_ranges(
    skill_bindings: &[TranscriptSkillBinding],
    custom_prompt_bindings: &[TranscriptCustomPromptBinding],
    image_attachment_ranges: &[(usize, usize)],
    visible_start_char: usize,
    text: &str,
) -> Vec<(usize, usize)> {
    let visible_end_char = visible_start_char + text.chars().count();
    let mut ranges = skill_bindings
        .iter()
        .map(|binding| (binding.start_char, binding.end_char))
        .chain(
            custom_prompt_bindings
                .iter()
                .map(|binding| (binding.start_char, binding.end_char)),
        )
        .chain(image_attachment_ranges.iter().copied())
        .filter_map(|(start_char, end_char)| {
            let overlap_start = start_char.max(visible_start_char);
            let overlap_end = end_char.min(visible_end_char);
            (overlap_start < overlap_end).then_some((
                overlap_start - visible_start_char,
                overlap_end - visible_start_char,
            ))
        })
        .collect::<Vec<_>>();
    ranges.sort_unstable_by_key(|(start, _)| *start);
    ranges
}

fn slice_char_range(text: &str, start: usize, end: usize) -> String {
    text.chars()
        .skip(start)
        .take(end.saturating_sub(start))
        .collect()
}

fn rendered_plain_lines(
    lines: &[VisualLine],
    options: PlainVisualRenderOptions<'_>,
) -> Vec<String> {
    lines
        .iter()
        .enumerate()
        .map(|(index, line)| {
            let (line_prefix, line_text) = rendered_line_parts(
                line,
                options.prompt,
                options.prompt_width,
                options.content_width,
                options.prompt_first_line_only,
                index,
                options.trim_overflow_spaces,
            );
            let fill_width = options
                .frame_fill_width
                .saturating_sub(options.prompt_width + measure_width(&line_text));
            format!("{line_prefix}{line_text}{}", " ".repeat(fill_width))
        })
        .collect()
}

#[cfg(test)]
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
    prompt_first_line_only: bool,
    rendered_index: usize,
    trim_overflow_spaces: bool,
) -> (String, String) {
    let line_prefix = if line.is_continuation || (prompt_first_line_only && rendered_index > 0) {
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

fn frame_decoration_mode(style_mode: StyleMode) -> FrameDecorationMode {
    match style_mode {
        StyleMode::Cx => FrameDecorationMode::Surface,
        StyleMode::Cc => FrameDecorationMode::Rule,
        StyleMode::Ms => FrameDecorationMode::None,
    }
}

#[derive(Debug, Clone, Default)]
struct FrameDecorationLines {
    top_line: Option<Line<'static>>,
    top_plain_line: Option<String>,
    bottom_line: Option<Line<'static>>,
    bottom_plain_line: Option<String>,
}

fn frame_decoration_lines(
    mode: FrameDecorationMode,
    palette: TerminalPalette,
    width: usize,
) -> FrameDecorationLines {
    if width == 0 {
        return FrameDecorationLines::default();
    }

    match mode {
        FrameDecorationMode::None => FrameDecorationLines::default(),
        FrameDecorationMode::Surface => {
            let Some(top_line) = crate::theme::surface_half_block_line(
                width,
                palette,
                crate::theme::SurfaceHalf::Lower,
            ) else {
                return FrameDecorationLines::default();
            };
            let Some(bottom_line) = crate::theme::surface_half_block_line(
                width,
                palette,
                crate::theme::SurfaceHalf::Upper,
            ) else {
                return FrameDecorationLines::default();
            };
            let plain = crate::theme::surface_half_block_plain_line(width);
            FrameDecorationLines {
                top_line: Some(top_line),
                top_plain_line: Some(plain.clone()),
                bottom_line: Some(bottom_line),
                bottom_plain_line: Some(plain),
            }
        }
        FrameDecorationMode::Rule => {
            let plain = "─".repeat(width);
            let line = Line::styled(plain.clone(), tertiary_text_style(palette));
            FrameDecorationLines {
                top_line: Some(line.clone()),
                top_plain_line: Some(plain.clone()),
                bottom_line: Some(line),
                bottom_plain_line: Some(plain),
            }
        }
    }
}

fn apply_optional_background(style: Style, palette: TerminalPalette) -> Style {
    match palette.surface {
        Some(surface) => style.bg(surface),
        None => style,
    }
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

#[cfg(test)]
mod tests {
    use super::{render_document, trim_overflow_boundary_spaces};
    use crate::composer::Composer;
    use crate::composer::grapheme::is_space_cluster;
    use crate::theme::{default_palette, surface_text_style, tertiary_text_style};

    #[test]
    fn trim_overflow_boundary_spaces_drops_trailing_spaces_only_when_needed() {
        assert_eq!(trim_overflow_boundary_spaces("ab  ", 2), "ab");
        assert_eq!(trim_overflow_boundary_spaces("ab ", 3), "ab ");
        assert_eq!(trim_overflow_boundary_spaces("ab", 2), "ab");
    }

    #[test]
    fn is_space_cluster_recognizes_whitespace_only_clusters() {
        assert!(is_space_cluster(" "));
        assert!(is_space_cluster("\t"));
        assert!(!is_space_cluster(" x"));
        assert!(!is_space_cluster(""));
    }

    #[test]
    fn render_document_preserves_invariants_across_seed_cases() {
        let cases = [
            ("hello world", 12, 0, 5),
            ("", 8, 0, 0),
            ("中文和 emoji 👨‍👩‍👧", 6, 0, 4),
        ];

        for (value, width, cursor_line, cursor_column) in cases {
            assert_render_document_invariants(value, width, cursor_line, cursor_column);
        }
    }

    #[test]
    fn render_document_preserves_invariants_across_generated_cases() {
        for (value, width, cursor_line, cursor_column) in generated_composer_cases() {
            assert_render_document_invariants(&value, width, cursor_line, cursor_column);
        }
    }

    #[test]
    #[ignore = "performance smoke test"]
    fn render_document_perf_smoke() {
        use std::hint::black_box;

        let draft = [
            "draft heading for transcript and composer benchmark",
            "",
            "soft wrap should stay stable under repeated rendering soft wrap should stay stable under repeated rendering",
            "    indented literal line with spaces",
            "\tindented literal line with tabs",
            "中文内容需要继续参与真实宽度计算。",
            "emoji cluster 👨‍👩‍👧 should keep cursor mapping correct",
            "line eight keeps the input tall enough to exercise viewport math",
            "line nine keeps the document renderer allocating multiple visual rows",
            "line ten keeps the cursor near the bottom of the draft",
            "benchmark final line with emoji 👨‍👩‍👧 and trailing text",
        ]
        .join("\n");

        let composer = composer_with_cursor(&draft, 64, 10, 48);
        for _ in 0..256 {
            black_box(render_document(&composer, default_palette()));
        }
    }

    #[test]
    fn rule_frame_decoration_uses_tertiary_palette_slot() {
        let mut composer = Composer::new(crate::StyleMode::Cc);
        composer.set_width(12);
        composer.set_height(4);

        let result = render_document(&composer, default_palette());
        let expected =
            ratatui::text::Line::styled("─".repeat(12), tertiary_text_style(default_palette()));

        assert_eq!(result.frame_decoration_top_line, Some(expected.clone()));
        assert_eq!(result.frame_decoration_bottom_line, Some(expected));
    }

    #[test]
    fn cx_prompt_uses_primary_text_style() {
        let palette = default_palette();
        let composer = composer_with_cursor("alpha", 12, 0, 0);
        let result = render_document(&composer, palette);

        assert_eq!(
            result.lines[0].spans[0].style.fg,
            Some(palette.main),
            "live cx composer prompt should use the normal text color"
        );
    }

    #[test]
    fn cx_composer_input_text_uses_surface_text_style() {
        let palette = default_palette();
        let composer = composer_with_cursor("alpha", 12, 0, 0);
        let result = render_document(&composer, palette);

        assert_eq!(result.lines[0].spans[1].content.as_ref(), "alpha");
        assert_eq!(
            result.lines[0].spans[1].style,
            surface_text_style(palette),
            "live cx composer text should match submitted user message text"
        );
    }

    #[test]
    fn selectable_range_uses_prompt_as_hit_range_not_content() {
        let composer = composer_with_cursor("alpha", 12, 0, 0);
        let result = render_document(&composer, default_palette());

        assert_eq!(result.selectable_ranges[0].content_columns(), Some((2, 7)));
        assert_eq!(result.selectable_ranges[0].hit_columns(), Some((0, 7)));
    }

    fn assert_render_document_invariants(
        value: &str,
        content_width: usize,
        cursor_line: usize,
        cursor_column: usize,
    ) {
        let composer = composer_with_cursor(value, content_width, cursor_line, cursor_column);
        let result = render_document(&composer, default_palette());

        assert!(
            !result.lines.is_empty(),
            "rendered lines should not be empty"
        );
        assert_eq!(
            result.lines.len(),
            result.anchors.len(),
            "line count {} did not match anchors {}",
            result.lines.len(),
            result.anchors.len()
        );
        assert_eq!(result.lines.len(), result.plain_lines.len());
        assert!(
            result.cursor_y < result.lines.len(),
            "cursor_y {} was out of range for {} lines",
            result.cursor_y,
            result.lines.len()
        );
    }

    fn composer_with_cursor(
        value: &str,
        content_width: usize,
        cursor_line: usize,
        cursor_column: usize,
    ) -> Composer {
        let mut composer = Composer::default();
        composer.set_width((content_width + 2) as u16);
        composer.set_height(8);
        composer.set_text_for_test(value);

        let lines = super::super::logical_lines(composer.value());
        composer.set_cursor(super::super::absolute_cursor_for_position(
            &lines,
            cursor_line,
            cursor_column,
        ));
        composer.sync_viewport_to_cursor();
        composer
    }

    fn generated_composer_cases() -> Vec<(String, usize, usize, usize)> {
        let segments = ["a", "b", " ", "  ", "\t", "\n", "中", "文", "👨‍👩‍👧", "emoji"];
        let mut seed = 0xC0DE_u64;
        let mut cases = Vec::new();

        for _ in 0..48 {
            let len = next_u32(&mut seed) as usize % 24;
            let mut value = String::new();
            for _ in 0..len {
                let index = next_u32(&mut seed) as usize % segments.len();
                value.push_str(segments[index]);
            }

            let width = (next_u32(&mut seed) as usize % 32) + 1;
            let lines = value.split('\n').collect::<Vec<_>>();
            let cursor_line = if lines.is_empty() {
                0
            } else {
                next_u32(&mut seed) as usize % lines.len()
            };
            let cursor_column = lines
                .get(cursor_line)
                .map(|line| next_u32(&mut seed) as usize % (line.chars().count() + 1))
                .unwrap_or(0);

            cases.push((value, width, cursor_line, cursor_column));
        }

        cases
    }

    fn next_u32(seed: &mut u64) -> u32 {
        *seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        (*seed >> 32) as u32
    }
}
