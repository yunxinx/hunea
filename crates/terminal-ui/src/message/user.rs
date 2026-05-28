use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use crate::{
    StyleMode,
    theme::{
        SurfaceHalf, TerminalPalette, secondary_text_style, surface_emphasis_style,
        surface_half_block_line, surface_text_style,
    },
    transcript::{ItemLineAnchor, LineAnchorKind, wrap_prompt_text, wrap_prompt_visual_lines},
};

use super::{UserMessageRenderLayout, user_projection::UserMessageProjectedLine};

pub(super) fn render_user_message_lines(
    content: &str,
    width: u16,
    palette: TerminalPalette,
    style_mode: StyleMode,
) -> Vec<Line<'static>> {
    match style_mode.normalized() {
        StyleMode::Cx => render_framed_user_message(content, width, palette, style_mode),
        StyleMode::Cc => render_compact_user_message(content, width, palette, style_mode),
        StyleMode::Ms => render_legacy_user_message(content, width, palette, style_mode),
    }
}

pub(super) fn render_framed_user_message(
    content: &str,
    width: u16,
    palette: TerminalPalette,
    style_mode: StyleMode,
) -> Vec<Line<'static>> {
    let layout = user_message_layout(width, style_mode);
    let wrapped = wrap_prompt_text(content, layout.content_width, layout.line_prefix_width);
    let mut rendered = format_framed_user_lines(&wrapped, layout, palette, style_mode);
    if !layout.shows_frame || !has_visible_user_message_frame(palette) {
        return rendered;
    }

    let mut lines = Vec::with_capacity(rendered.len() + 2);
    lines.push(user_message_surface_padding_line(
        layout.frame_width,
        palette,
        SurfaceHalf::Lower,
    ));
    lines.append(&mut rendered);
    lines.push(user_message_surface_padding_line(
        layout.frame_width,
        palette,
        SurfaceHalf::Upper,
    ));
    lines
}

pub(super) fn render_compact_user_message(
    content: &str,
    width: u16,
    palette: TerminalPalette,
    style_mode: StyleMode,
) -> Vec<Line<'static>> {
    let wrapped = wrap_prompt_text(
        content,
        user_message_compact_content_width(width, style_mode),
        user_message_inset_width(style_mode),
    );
    format_compact_user_lines(&wrapped, usize::from(width.max(1)), palette, style_mode)
}

pub(super) fn render_legacy_user_message(
    content: &str,
    width: u16,
    palette: TerminalPalette,
    style_mode: StyleMode,
) -> Vec<Line<'static>> {
    let wrapped = wrap_prompt_text(
        content,
        user_message_legacy_content_width(width, style_mode),
        user_message_inset_width(style_mode),
    );
    format_legacy_user_lines(&wrapped, palette, style_mode)
}

pub(super) fn format_framed_user_lines(
    lines: &[String],
    layout: UserMessageRenderLayout,
    palette: TerminalPalette,
    style_mode: StyleMode,
) -> Vec<Line<'static>> {
    let prefix_style = surface_text_style(palette);
    let mut prefix_glyph_style = secondary_text_style(palette);
    if let Some(surface) = palette.surface {
        prefix_glyph_style = prefix_glyph_style.bg(surface);
    }
    let content_style = surface_text_style(palette);
    let continuation_prefix = " ".repeat(layout.line_prefix_width);

    lines
        .iter()
        .enumerate()
        .map(|(index, line)| {
            let trailing_fill_width = layout
                .frame_width
                .saturating_sub(layout.line_prefix_width + measure_width(line));
            let trailing_fill = " ".repeat(trailing_fill_width);

            if index == 0 && layout.shows_prefix {
                Line::default().spans([
                    Span::styled(user_message_prefix_glyph(style_mode), prefix_glyph_style),
                    Span::styled(" ", prefix_style),
                    Span::styled(line.clone(), content_style),
                    Span::styled(trailing_fill, prefix_style),
                ])
            } else {
                Line::default().spans([
                    Span::styled(continuation_prefix.clone(), prefix_style),
                    Span::styled(line.clone(), content_style),
                    Span::styled(trailing_fill, prefix_style),
                ])
            }
        })
        .collect()
}

pub(super) fn render_projected_framed_user_line(
    line: &UserMessageProjectedLine,
    is_first: bool,
    layout: UserMessageRenderLayout,
    palette: TerminalPalette,
    style_mode: StyleMode,
) -> Line<'static> {
    let prefix_style = surface_text_style(palette);
    let mut prefix_glyph_style = secondary_text_style(palette);
    if let Some(surface) = palette.surface {
        prefix_glyph_style = prefix_glyph_style.bg(surface);
    }
    let content_style = surface_text_style(palette);
    let continuation_prefix = " ".repeat(layout.line_prefix_width);
    let trailing_fill_width = layout
        .frame_width
        .saturating_sub(layout.line_prefix_width + measure_width(&line.text));
    let trailing_fill = " ".repeat(trailing_fill_width);

    if is_first && layout.shows_prefix {
        Line::default().spans([
            Span::styled(user_message_prefix_glyph(style_mode), prefix_glyph_style),
            Span::styled(" ", prefix_style),
            Span::styled(line.text.clone(), content_style),
            Span::styled(trailing_fill, prefix_style),
        ])
    } else {
        Line::default().spans([
            Span::styled(continuation_prefix, prefix_style),
            Span::styled(line.text.clone(), content_style),
            Span::styled(trailing_fill, prefix_style),
        ])
    }
}

pub(super) fn format_compact_user_lines(
    lines: &[String],
    width: usize,
    palette: TerminalPalette,
    style_mode: StyleMode,
) -> Vec<Line<'static>> {
    let prefix_style = surface_text_style(palette);
    let mut prefix_glyph_style = secondary_text_style(palette);
    if let Some(surface) = palette.surface {
        prefix_glyph_style = prefix_glyph_style.bg(surface);
    }
    let content_style = surface_text_style(palette);
    let continuation_prefix = " ".repeat(user_message_inset_width(style_mode));

    lines
        .iter()
        .enumerate()
        .map(|(index, line)| {
            let trailing_fill_width =
                width.saturating_sub(user_message_inset_width(style_mode) + measure_width(line));
            let trailing_fill = " ".repeat(trailing_fill_width);

            if index == 0 {
                Line::default().spans([
                    Span::styled(user_message_prefix_glyph(style_mode), prefix_glyph_style),
                    Span::styled(" ", prefix_style),
                    Span::styled(line.clone(), content_style),
                    Span::styled(trailing_fill, prefix_style),
                ])
            } else {
                Line::default().spans([
                    Span::styled(continuation_prefix.clone(), prefix_style),
                    Span::styled(line.clone(), content_style),
                    Span::styled(trailing_fill, prefix_style),
                ])
            }
        })
        .collect()
}

pub(super) fn render_projected_compact_user_line(
    line: &UserMessageProjectedLine,
    is_first: bool,
    width: usize,
    palette: TerminalPalette,
    style_mode: StyleMode,
) -> Line<'static> {
    let prefix_style = surface_text_style(palette);
    let mut prefix_glyph_style = secondary_text_style(palette);
    if let Some(surface) = palette.surface {
        prefix_glyph_style = prefix_glyph_style.bg(surface);
    }
    let content_style = surface_text_style(palette);
    let continuation_prefix = " ".repeat(user_message_inset_width(style_mode));
    let trailing_fill_width =
        width.saturating_sub(user_message_inset_width(style_mode) + measure_width(&line.text));
    let trailing_fill = " ".repeat(trailing_fill_width);

    if is_first {
        Line::default().spans([
            Span::styled(user_message_prefix_glyph(style_mode), prefix_glyph_style),
            Span::styled(" ", prefix_style),
            Span::styled(line.text.clone(), content_style),
            Span::styled(trailing_fill, prefix_style),
        ])
    } else {
        Line::default().spans([
            Span::styled(continuation_prefix, prefix_style),
            Span::styled(line.text.clone(), content_style),
            Span::styled(trailing_fill, prefix_style),
        ])
    }
}

pub(super) fn format_legacy_user_lines(
    lines: &[String],
    palette: TerminalPalette,
    style_mode: StyleMode,
) -> Vec<Line<'static>> {
    let prefix_style = surface_text_style(palette);
    let content_style = surface_emphasis_style(palette);
    let continuation_prefix = " ".repeat(user_message_inset_width(style_mode));

    lines
        .iter()
        .enumerate()
        .map(|(index, line)| {
            if index == 0 {
                Line::default().spans([
                    Span::styled(user_message_prefix(style_mode), prefix_style),
                    Span::styled(line.clone(), content_style),
                ])
            } else {
                Line::default().spans([
                    Span::styled(continuation_prefix.clone(), prefix_style),
                    Span::styled(line.clone(), content_style),
                ])
            }
        })
        .collect()
}

pub(super) fn render_projected_legacy_user_line(
    line: &UserMessageProjectedLine,
    is_first: bool,
    palette: TerminalPalette,
    style_mode: StyleMode,
) -> Line<'static> {
    let prefix_style = surface_text_style(palette);
    let content_style = surface_emphasis_style(palette);
    let continuation_prefix = " ".repeat(user_message_inset_width(style_mode));

    if is_first {
        Line::default().spans([
            Span::styled(user_message_prefix(style_mode), prefix_style),
            Span::styled(line.text.clone(), content_style),
        ])
    } else {
        Line::default().spans([
            Span::styled(continuation_prefix, prefix_style),
            Span::styled(line.text.clone(), content_style),
        ])
    }
}

pub(super) fn format_user_plain_lines(lines: &[String], style_mode: StyleMode) -> String {
    let continuation_prefix = " ".repeat(user_message_inset_width(style_mode));

    lines
        .iter()
        .enumerate()
        .map(|(index, line)| {
            if index == 0 {
                format!("{}{}", user_message_prefix(style_mode), line)
            } else {
                format!("{continuation_prefix}{line}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub(super) fn projected_framed_user_plain_line(
    line: &UserMessageProjectedLine,
    is_first: bool,
    layout: UserMessageRenderLayout,
    style_mode: StyleMode,
) -> String {
    let trailing_fill_width = layout
        .frame_width
        .saturating_sub(layout.line_prefix_width + measure_width(&line.text));
    let trailing_fill = " ".repeat(trailing_fill_width);

    if is_first && layout.shows_prefix {
        format!(
            "{} {}{}",
            user_message_prefix_glyph(style_mode),
            line.text,
            trailing_fill
        )
    } else {
        format!(
            "{}{}{}",
            " ".repeat(layout.line_prefix_width),
            line.text,
            trailing_fill
        )
    }
}

pub(super) fn projected_compact_user_plain_line(
    line: &UserMessageProjectedLine,
    is_first: bool,
    width: usize,
    style_mode: StyleMode,
) -> String {
    let trailing_fill_width =
        width.saturating_sub(user_message_inset_width(style_mode) + measure_width(&line.text));
    let trailing_fill = " ".repeat(trailing_fill_width);

    if is_first {
        format!(
            "{} {}{}",
            user_message_prefix_glyph(style_mode),
            line.text,
            trailing_fill
        )
    } else {
        format!(
            "{}{}{}",
            " ".repeat(user_message_inset_width(style_mode)),
            line.text,
            trailing_fill
        )
    }
}

pub(super) fn projected_legacy_user_plain_line(
    line: &UserMessageProjectedLine,
    is_first: bool,
    style_mode: StyleMode,
) -> String {
    if is_first {
        format!("{}{}", user_message_prefix(style_mode), line.text)
    } else {
        format!(
            "{}{}",
            " ".repeat(user_message_inset_width(style_mode)),
            line.text
        )
    }
}

pub(super) fn projected_framed_user_plain_line_len(
    line: &UserMessageProjectedLine,
    is_first: bool,
    layout: UserMessageRenderLayout,
    style_mode: StyleMode,
) -> usize {
    framed_user_plain_line_len(&line.text, is_first, layout, style_mode)
}

pub(super) fn projected_compact_user_plain_line_len(
    line: &UserMessageProjectedLine,
    is_first: bool,
    width: usize,
    style_mode: StyleMode,
) -> usize {
    compact_user_plain_line_len(&line.text, is_first, width, style_mode)
}

pub(super) fn projected_legacy_user_plain_line_len(
    line: &UserMessageProjectedLine,
    is_first: bool,
    style_mode: StyleMode,
) -> usize {
    legacy_user_plain_line_len(&line.text, is_first, style_mode)
}

pub(super) fn framed_user_plain_line_len(
    text: &str,
    is_first: bool,
    layout: UserMessageRenderLayout,
    style_mode: StyleMode,
) -> usize {
    let trailing_fill_width = layout
        .frame_width
        .saturating_sub(layout.line_prefix_width + measure_width(text));

    if is_first && layout.shows_prefix {
        user_message_prefix_glyph(style_mode).len() + 1 + text.len() + trailing_fill_width
    } else {
        layout.line_prefix_width + text.len() + trailing_fill_width
    }
}

pub(super) fn compact_user_plain_line_len(
    text: &str,
    is_first: bool,
    width: usize,
    style_mode: StyleMode,
) -> usize {
    let trailing_fill_width =
        width.saturating_sub(user_message_inset_width(style_mode) + measure_width(text));

    if is_first {
        user_message_prefix_glyph(style_mode).len() + 1 + text.len() + trailing_fill_width
    } else {
        user_message_inset_width(style_mode) + text.len() + trailing_fill_width
    }
}

pub(super) fn legacy_user_plain_line_len(
    text: &str,
    is_first: bool,
    style_mode: StyleMode,
) -> usize {
    if is_first {
        user_message_prefix(style_mode).len() + text.len()
    } else {
        user_message_inset_width(style_mode) + text.len()
    }
}

pub(super) fn measure_width(text: &str) -> usize {
    UnicodeWidthStr::width(text)
}

pub(super) fn rendered_line_anchor(rendered_line: usize) -> ItemLineAnchor {
    ItemLineAnchor {
        kind: LineAnchorKind::RenderedLine,
        logical_line: 0,
        range_start: 0,
        range_end: 0,
        rendered_line,
        gap_offset: 0,
    }
}

pub(super) fn user_message_logical_line_anchors(
    content: &str,
    content_width: usize,
    line_prefix_width: usize,
) -> Vec<ItemLineAnchor> {
    wrap_prompt_visual_lines(content, content_width, line_prefix_width)
        .into_iter()
        .enumerate()
        .map(|(rendered_line, line)| ItemLineAnchor {
            kind: LineAnchorKind::LogicalPosition,
            logical_line: line.logical_line,
            range_start: line.visible_start_char,
            range_end: line.end_char,
            rendered_line,
            gap_offset: 0,
        })
        .collect()
}

pub(super) fn has_visible_user_message_frame(palette: TerminalPalette) -> bool {
    palette.surface.is_some()
}

pub(super) fn user_message_surface_padding_line(
    width: usize,
    palette: TerminalPalette,
    half: SurfaceHalf,
) -> Line<'static> {
    surface_half_block_line(width, palette, half)
        .unwrap_or_else(|| Line::raw(" ".repeat(width.max(1))))
}

pub(super) fn user_message_prefix(style_mode: StyleMode) -> &'static str {
    match style_mode.normalized() {
        StyleMode::Cx => "› ",
        StyleMode::Cc => "❯ ",
        StyleMode::Ms => "> ",
    }
}

pub(super) fn user_message_prefix_glyph(style_mode: StyleMode) -> &'static str {
    match style_mode.normalized() {
        StyleMode::Cx => "›",
        StyleMode::Cc => "❯",
        StyleMode::Ms => ">",
    }
}

pub(super) fn user_message_inset_width(style_mode: StyleMode) -> usize {
    measure_width(user_message_prefix(style_mode))
}

pub(super) fn user_message_compact_content_width(width: u16, style_mode: StyleMode) -> usize {
    usize::from(width.max(1))
        .saturating_sub(user_message_inset_width(style_mode) * 2)
        .max(1)
}

pub(super) fn user_message_legacy_content_width(width: u16, style_mode: StyleMode) -> usize {
    usize::from(width.max(1))
        .saturating_sub(user_message_inset_width(style_mode))
        .max(1)
}

pub(super) fn user_message_layout(width: u16, style_mode: StyleMode) -> UserMessageRenderLayout {
    let content_width = user_message_compact_content_width(width, style_mode);
    UserMessageRenderLayout {
        frame_width: usize::from(width.max(1))
            .max(user_message_inset_width(style_mode) + content_width),
        content_width,
        line_prefix_width: user_message_inset_width(style_mode),
        shows_prefix: true,
        shows_frame: true,
    }
}

pub(super) fn render_user_message_line_anchors(
    content: &str,
    width: u16,
    palette: TerminalPalette,
    style_mode: StyleMode,
) -> Vec<ItemLineAnchor> {
    match style_mode.normalized() {
        StyleMode::Ms => user_message_logical_line_anchors(
            content,
            user_message_legacy_content_width(width, style_mode),
            user_message_inset_width(style_mode),
        ),
        StyleMode::Cc => user_message_logical_line_anchors(
            content,
            user_message_compact_content_width(width, style_mode),
            user_message_inset_width(style_mode),
        ),
        StyleMode::Cx => {
            let layout = user_message_layout(width, style_mode);
            let wrapped =
                wrap_prompt_visual_lines(content, layout.content_width, layout.line_prefix_width);
            let has_frame = layout.shows_frame && has_visible_user_message_frame(palette);
            let mut anchors = Vec::with_capacity(wrapped.len() + usize::from(has_frame) * 2);

            if has_frame {
                anchors.push(rendered_line_anchor(0));
            }

            let rendered_offset = usize::from(has_frame);
            for (index, line) in wrapped.into_iter().enumerate() {
                anchors.push(ItemLineAnchor {
                    kind: LineAnchorKind::LogicalPosition,
                    logical_line: line.logical_line,
                    range_start: line.visible_start_char,
                    range_end: line.end_char,
                    rendered_line: index + rendered_offset,
                    gap_offset: 0,
                });
            }

            if has_frame {
                anchors.push(rendered_line_anchor(anchors.len()));
            }

            anchors
        }
    }
}

pub(super) fn render_user_plain_text(content: &str, width: u16, style_mode: StyleMode) -> String {
    let wrapped = match style_mode.normalized() {
        StyleMode::Cx => {
            let layout = user_message_layout(width, style_mode);
            wrap_prompt_text(content, layout.content_width, layout.line_prefix_width)
        }
        StyleMode::Cc => wrap_prompt_text(
            content,
            user_message_compact_content_width(width, style_mode),
            user_message_inset_width(style_mode),
        ),
        StyleMode::Ms => wrap_prompt_text(
            content,
            user_message_legacy_content_width(width, style_mode),
            user_message_inset_width(style_mode),
        ),
    };

    format_user_plain_lines(&wrapped, style_mode)
}
