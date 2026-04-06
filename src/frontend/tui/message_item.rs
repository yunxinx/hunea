use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
    rc::Rc,
};

#[cfg(test)]
use std::cell::Cell;

use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use super::{
    Sender, StyleMode,
    selection::SelectableLineRange,
    styled_text::{lines_to_ansi_text, lines_to_plain_text},
    theme::{TerminalPalette, secondary_text_style, surface_emphasis_style, surface_text_style},
    transcript::{
        DEFAULT_RENDER_WIDTH, ItemLineAnchor, LineAnchorKind, render_markdown_lines,
        wrap_assistant_text, wrap_prompt_text, wrap_prompt_visual_lines,
    },
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct UserMessageRenderLayout {
    frame_width: usize,
    content_width: usize,
    line_prefix_width: usize,
    shows_prefix: bool,
    shows_frame: bool,
}

/// `MessageItem` 表示 transcript 中的一条对话消息。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageItem {
    sender: Sender,
    content: String,
    style_mode: StyleMode,
    render_cache_key: u64,
}

/// `UserMessageRenderProjection` 保存用户消息在固定宽度下的轻量投影视图。
#[derive(Debug, Clone)]
pub(crate) struct UserMessageRenderProjection {
    lines: Rc<Vec<UserMessageProjectedLine>>,
    layout: UserMessageRenderLayout,
    has_frame: bool,
    palette: TerminalPalette,
    style_mode: StyleMode,
}

#[derive(Debug, Clone)]
struct UserMessageProjectedLine {
    // transcript render cache 只会消费渲染文本与 anchor 元数据，不需要列映射。
    text: String,
    logical_line: usize,
    visible_start_char: usize,
    end_char: usize,
}

impl From<crate::frontend::tui::transcript::PromptVisualLine> for UserMessageProjectedLine {
    fn from(line: crate::frontend::tui::transcript::PromptVisualLine) -> Self {
        Self {
            text: line.text,
            logical_line: line.logical_line,
            visible_start_char: line.visible_start_char,
            end_char: line.end_char,
        }
    }
}

#[cfg(test)]
thread_local! {
    static MESSAGE_ITEM_RENDER_CACHE_KEY_CALL_COUNT: Cell<usize> = const { Cell::new(0) };
    static USER_MESSAGE_PROJECTION_PLAIN_LINE_LEN_CALL_COUNT: Cell<usize> = const { Cell::new(0) };
}

impl MessageItem {
    /// `new` 创建一条消息项。
    #[cfg(test)]
    pub fn new(sender: Sender, content: impl Into<String>) -> Self {
        Self::new_with_style_mode(sender, content, StyleMode::Cx)
    }

    /// `new_with_style_mode` 创建一条带指定样式模式的消息项。
    pub fn new_with_style_mode(
        sender: Sender,
        content: impl Into<String>,
        style_mode: StyleMode,
    ) -> Self {
        let style_mode = style_mode.normalized();
        let content = content.into();
        let render_cache_key = message_item_render_cache_key(sender, &content, style_mode);
        Self {
            sender,
            content,
            style_mode,
            render_cache_key,
        }
    }

    /// `render_lines` 将消息渲染为带样式的文本行。
    pub fn render_lines(&self, width: u16, palette: TerminalPalette) -> Vec<Line<'static>> {
        match self.sender {
            Sender::User => {
                render_user_message_lines(&self.content, width, palette, self.style_mode)
            }
            Sender::Assistant => render_assistant_message(&self.content, width, palette),
        }
    }

    /// `render_for_terminal_replay` 返回适合退出 AltScreen 后回放到终端的消息文本。
    pub fn render_for_terminal_replay(
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

    /// `render_plain_text` 返回不带 ANSI 的纯文本消息内容。
    pub fn render_plain_text(&self, width: u16, palette: TerminalPalette) -> String {
        match self.sender {
            Sender::User => render_user_plain_text(&self.content, width, self.style_mode),
            Sender::Assistant => {
                lines_to_plain_text(&render_assistant_message(&self.content, width, palette))
            }
        }
    }

    pub(crate) fn render_cache_key(&self) -> u64 {
        self.render_cache_key
    }

    pub(crate) fn source_text_byte_len(&self) -> usize {
        self.content.len()
    }

    pub(crate) fn render_line_anchors(
        &self,
        width: u16,
        palette: TerminalPalette,
    ) -> Vec<ItemLineAnchor> {
        if self.sender != Sender::User {
            return Vec::new();
        }

        match self.style_mode.normalized() {
            StyleMode::Ms => user_message_logical_line_anchors(
                &self.content,
                user_message_legacy_content_width(width, self.style_mode),
                user_message_inset_width(self.style_mode),
            ),
            StyleMode::Cc => user_message_logical_line_anchors(
                &self.content,
                user_message_compact_content_width(width, self.style_mode),
                user_message_inset_width(self.style_mode),
            ),
            StyleMode::Cx => {
                let layout = user_message_layout(width, self.style_mode);
                let wrapped = wrap_prompt_visual_lines(
                    &self.content,
                    layout.content_width,
                    layout.line_prefix_width,
                );
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

    pub(crate) fn render_selectable_line_ranges(
        &self,
        width: u16,
        palette: TerminalPalette,
    ) -> Vec<SelectableLineRange> {
        if self.sender != Sender::User {
            return Vec::new();
        }

        let snapshot = user_message_wrap_snapshot(&self.content, width, palette, self.style_mode);
        let mut ranges =
            Vec::with_capacity(snapshot.lines.len() + usize::from(snapshot.has_frame) * 2);

        if snapshot.has_frame {
            ranges.push(SelectableLineRange::default());
        }

        for (index, line) in snapshot.lines.iter().enumerate() {
            let line_width = measure_width(&line.text);
            if line_width == 0 {
                let anchor_end = if snapshot.layout.frame_width > 0 {
                    snapshot.layout.frame_width
                } else {
                    snapshot.layout.line_prefix_width.max(1)
                };
                ranges.push(SelectableLineRange::blank_anchor(0, anchor_end));
                continue;
            }

            if index == 0 {
                ranges.push(SelectableLineRange::new(
                    0,
                    snapshot.layout.line_prefix_width + line_width,
                ));
            } else {
                ranges.push(SelectableLineRange::new(
                    snapshot.layout.line_prefix_width,
                    snapshot.layout.line_prefix_width + line_width,
                ));
            }
        }

        if snapshot.has_frame {
            ranges.push(SelectableLineRange::default());
        }

        ranges
    }

    pub(crate) fn render_projection(
        &self,
        width: u16,
        palette: TerminalPalette,
    ) -> Option<UserMessageRenderProjection> {
        (self.sender == Sender::User).then(|| {
            let UserMessageWrapSnapshot {
                lines,
                layout,
                has_frame,
            } = user_message_wrap_snapshot(&self.content, width, palette, self.style_mode);
            UserMessageRenderProjection {
                lines: Rc::new(
                    lines
                        .into_iter()
                        .map(UserMessageProjectedLine::from)
                        .collect(),
                ),
                layout,
                has_frame,
                palette,
                style_mode: self.style_mode,
            }
        })
    }

    #[cfg(test)]
    fn render_plain_for_test(&self, width: u16) -> String {
        self.render_plain_text(width, crate::frontend::tui::theme::default_palette())
    }
}

#[cfg(test)]
pub(crate) fn reset_message_item_render_cache_key_call_count() {
    MESSAGE_ITEM_RENDER_CACHE_KEY_CALL_COUNT.set(0);
}

#[cfg(test)]
pub(crate) fn message_item_render_cache_key_call_count() -> usize {
    MESSAGE_ITEM_RENDER_CACHE_KEY_CALL_COUNT.get()
}

#[cfg(test)]
pub(crate) fn reset_user_message_projection_plain_line_len_call_count() {
    USER_MESSAGE_PROJECTION_PLAIN_LINE_LEN_CALL_COUNT.set(0);
}

#[cfg(test)]
pub(crate) fn user_message_projection_plain_line_len_call_count() -> usize {
    USER_MESSAGE_PROJECTION_PLAIN_LINE_LEN_CALL_COUNT.get()
}

fn message_item_render_cache_key(sender: Sender, content: &str, style_mode: StyleMode) -> u64 {
    #[cfg(test)]
    MESSAGE_ITEM_RENDER_CACHE_KEY_CALL_COUNT.with(|count| count.set(count.get() + 1));

    let mut hasher = DefaultHasher::new();
    (sender as u8).hash(&mut hasher);
    if sender == Sender::User {
        style_mode.hash(&mut hasher);
    }
    content.hash(&mut hasher);
    hasher.finish()
}

fn render_user_message_lines(
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

fn render_framed_user_message(
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

    let padding_line = user_message_surface_padding_line(layout.frame_width, palette);
    let mut lines = Vec::with_capacity(rendered.len() + 2);
    lines.push(padding_line.clone());
    lines.append(&mut rendered);
    lines.push(padding_line);
    lines
}

fn render_compact_user_message(
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

fn render_legacy_user_message(
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

fn render_assistant_message(
    content: &str,
    width: u16,
    palette: TerminalPalette,
) -> Vec<Line<'static>> {
    let width = if width == 0 {
        DEFAULT_RENDER_WIDTH
    } else {
        usize::from(width)
    };
    let rendered = render_markdown_lines(content, width, palette);
    if rendered.is_empty() {
        return wrap_assistant_text(content, width, 0)
            .into_iter()
            .map(Line::raw)
            .collect();
    }

    rendered
}

fn render_user_plain_text(content: &str, width: u16, style_mode: StyleMode) -> String {
    match style_mode.normalized() {
        StyleMode::Cx | StyleMode::Cc => {
            let wrapped = wrap_prompt_text(
                content,
                user_message_compact_content_width(width, style_mode),
                user_message_inset_width(style_mode),
            );
            format_user_plain_lines(&wrapped, style_mode)
        }
        StyleMode::Ms => {
            let wrapped = wrap_prompt_text(
                content,
                user_message_legacy_content_width(width, style_mode),
                user_message_inset_width(style_mode),
            );
            format_user_plain_lines(&wrapped, style_mode)
        }
    }
}

#[derive(Debug, Clone)]
struct UserMessageWrapSnapshot {
    lines: Vec<crate::frontend::tui::transcript::PromptVisualLine>,
    layout: UserMessageRenderLayout,
    has_frame: bool,
}

impl UserMessageRenderProjection {
    pub(crate) fn line_count(&self) -> usize {
        self.lines.len() + usize::from(self.has_frame) * 2
    }

    pub(crate) fn line_at(&self, index: usize) -> Option<Line<'static>> {
        if self.has_frame && self.is_frame_line(index) {
            return Some(user_message_surface_padding_line(
                self.layout.frame_width,
                self.palette,
            ));
        }

        let content_index = self.content_line_index(index)?;
        let line = self.lines.get(content_index)?;
        let is_first = content_index == 0;

        Some(match self.style_mode.normalized() {
            StyleMode::Cx => render_projected_framed_user_line(
                line,
                is_first,
                self.layout,
                self.palette,
                self.style_mode,
            ),
            StyleMode::Cc => render_projected_compact_user_line(
                line,
                is_first,
                self.layout.frame_width.max(1),
                self.palette,
                self.style_mode,
            ),
            StyleMode::Ms => {
                render_projected_legacy_user_line(line, is_first, self.palette, self.style_mode)
            }
        })
    }

    pub(crate) fn plain_line_at(&self, index: usize) -> Option<String> {
        if self.has_frame && self.is_frame_line(index) {
            return Some(" ".repeat(self.layout.frame_width.max(1)));
        }

        let content_index = self.content_line_index(index)?;
        let line = self.lines.get(content_index)?;
        let is_first = content_index == 0;

        Some(match self.style_mode.normalized() {
            StyleMode::Cx => {
                projected_framed_user_plain_line(line, is_first, self.layout, self.style_mode)
            }
            StyleMode::Cc => projected_compact_user_plain_line(
                line,
                is_first,
                self.layout.frame_width,
                self.style_mode,
            ),
            StyleMode::Ms => projected_legacy_user_plain_line(line, is_first, self.style_mode),
        })
    }

    pub(crate) fn plain_line_lens(&self) -> Vec<usize> {
        (0..self.line_count())
            .filter_map(|index| self.plain_line_len(index))
            .collect()
    }

    pub(crate) fn plain_line_len(&self, index: usize) -> Option<usize> {
        #[cfg(test)]
        USER_MESSAGE_PROJECTION_PLAIN_LINE_LEN_CALL_COUNT.with(|count| count.set(count.get() + 1));

        if self.has_frame && self.is_frame_line(index) {
            return Some(self.layout.frame_width.max(1));
        }

        let content_index = self.content_line_index(index)?;
        let line = self.lines.get(content_index)?;
        let is_first = content_index == 0;

        Some(match self.style_mode.normalized() {
            StyleMode::Cx => {
                projected_framed_user_plain_line_len(line, is_first, self.layout, self.style_mode)
            }
            StyleMode::Cc => projected_compact_user_plain_line_len(
                line,
                is_first,
                self.layout.frame_width,
                self.style_mode,
            ),
            StyleMode::Ms => projected_legacy_user_plain_line_len(line, is_first, self.style_mode),
        })
    }

    pub(crate) fn line_anchors(&self) -> Vec<ItemLineAnchor> {
        match self.style_mode.normalized() {
            StyleMode::Cx => {
                let mut anchors =
                    Vec::with_capacity(self.lines.len() + usize::from(self.has_frame) * 2);
                if self.has_frame {
                    anchors.push(rendered_line_anchor(0));
                }

                let rendered_offset = usize::from(self.has_frame);
                for (index, line) in self.lines.iter().enumerate() {
                    anchors.push(ItemLineAnchor {
                        kind: LineAnchorKind::LogicalPosition,
                        logical_line: line.logical_line,
                        range_start: line.visible_start_char,
                        range_end: line.end_char,
                        rendered_line: index + rendered_offset,
                        gap_offset: 0,
                    });
                }

                if self.has_frame {
                    anchors.push(rendered_line_anchor(anchors.len()));
                }

                anchors
            }
            StyleMode::Cc | StyleMode::Ms => self
                .lines
                .iter()
                .enumerate()
                .map(|(rendered_line, line)| ItemLineAnchor {
                    kind: LineAnchorKind::LogicalPosition,
                    logical_line: line.logical_line,
                    range_start: line.visible_start_char,
                    range_end: line.end_char,
                    rendered_line,
                    gap_offset: 0,
                })
                .collect(),
        }
    }

    pub(crate) fn estimated_render_ui_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            + std::mem::size_of_val(self.lines.as_slice())
            + self.lines.iter().map(|line| line.text.len()).sum::<usize>()
    }

    fn is_frame_line(&self, index: usize) -> bool {
        index == 0 || index + 1 == self.line_count()
    }

    fn content_line_index(&self, index: usize) -> Option<usize> {
        if self.has_frame {
            index
                .checked_sub(1)
                .filter(|index| *index < self.lines.len())
        } else {
            (index < self.lines.len()).then_some(index)
        }
    }
}

fn user_message_wrap_snapshot(
    content: &str,
    width: u16,
    palette: TerminalPalette,
    style_mode: StyleMode,
) -> UserMessageWrapSnapshot {
    match style_mode.normalized() {
        StyleMode::Ms => {
            let layout = UserMessageRenderLayout {
                frame_width: usize::from(width.max(1)),
                content_width: user_message_legacy_content_width(width, style_mode),
                line_prefix_width: user_message_inset_width(style_mode),
                shows_prefix: true,
                shows_frame: false,
            };
            UserMessageWrapSnapshot {
                lines: wrap_prompt_visual_lines(
                    content,
                    layout.content_width,
                    layout.line_prefix_width,
                ),
                layout,
                has_frame: false,
            }
        }
        StyleMode::Cc => {
            let layout = UserMessageRenderLayout {
                frame_width: usize::from(width.max(1)),
                content_width: user_message_compact_content_width(width, style_mode),
                line_prefix_width: user_message_inset_width(style_mode),
                shows_prefix: true,
                shows_frame: false,
            };
            UserMessageWrapSnapshot {
                lines: wrap_prompt_visual_lines(
                    content,
                    layout.content_width,
                    layout.line_prefix_width,
                ),
                layout,
                has_frame: false,
            }
        }
        StyleMode::Cx => {
            let layout = user_message_layout(width, style_mode);
            let has_frame = layout.shows_frame && has_visible_user_message_frame(palette);
            UserMessageWrapSnapshot {
                lines: wrap_prompt_visual_lines(
                    content,
                    layout.content_width,
                    layout.line_prefix_width,
                ),
                layout,
                has_frame,
            }
        }
    }
}

fn format_framed_user_lines(
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

fn render_projected_framed_user_line(
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

fn format_compact_user_lines(
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

fn render_projected_compact_user_line(
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

fn format_legacy_user_lines(
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

fn render_projected_legacy_user_line(
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

fn format_user_plain_lines(lines: &[String], style_mode: StyleMode) -> String {
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

fn projected_framed_user_plain_line(
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

fn projected_compact_user_plain_line(
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

fn projected_legacy_user_plain_line(
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

fn projected_framed_user_plain_line_len(
    line: &UserMessageProjectedLine,
    is_first: bool,
    layout: UserMessageRenderLayout,
    style_mode: StyleMode,
) -> usize {
    projected_framed_user_plain_line(line, is_first, layout, style_mode).len()
}

fn projected_compact_user_plain_line_len(
    line: &UserMessageProjectedLine,
    is_first: bool,
    width: usize,
    style_mode: StyleMode,
) -> usize {
    projected_compact_user_plain_line(line, is_first, width, style_mode).len()
}

fn projected_legacy_user_plain_line_len(
    line: &UserMessageProjectedLine,
    is_first: bool,
    style_mode: StyleMode,
) -> usize {
    projected_legacy_user_plain_line(line, is_first, style_mode).len()
}

fn measure_width(text: &str) -> usize {
    UnicodeWidthStr::width(text)
}

fn rendered_line_anchor(rendered_line: usize) -> ItemLineAnchor {
    ItemLineAnchor {
        kind: LineAnchorKind::RenderedLine,
        logical_line: 0,
        range_start: 0,
        range_end: 0,
        rendered_line,
        gap_offset: 0,
    }
}

fn user_message_logical_line_anchors(
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

fn has_visible_user_message_frame(palette: TerminalPalette) -> bool {
    palette.surface.is_some()
}

fn user_message_surface_padding_line(width: usize, palette: TerminalPalette) -> Line<'static> {
    Line::default().spans([Span::styled(
        " ".repeat(width.max(1)),
        surface_text_style(palette),
    )])
}

fn user_message_prefix(style_mode: StyleMode) -> &'static str {
    match style_mode.normalized() {
        StyleMode::Cx => "› ",
        StyleMode::Cc => "❯ ",
        StyleMode::Ms => "> ",
    }
}

fn user_message_prefix_glyph(style_mode: StyleMode) -> &'static str {
    match style_mode.normalized() {
        StyleMode::Cx => "›",
        StyleMode::Cc => "❯",
        StyleMode::Ms => ">",
    }
}

fn user_message_inset_width(style_mode: StyleMode) -> usize {
    measure_width(user_message_prefix(style_mode))
}

fn user_message_compact_content_width(width: u16, style_mode: StyleMode) -> usize {
    usize::from(width.max(1))
        .saturating_sub(user_message_inset_width(style_mode) * 2)
        .max(1)
}

fn user_message_legacy_content_width(width: u16, style_mode: StyleMode) -> usize {
    usize::from(width.max(1))
        .saturating_sub(user_message_inset_width(style_mode))
        .max(1)
}

fn user_message_layout(width: u16, style_mode: StyleMode) -> UserMessageRenderLayout {
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

#[cfg(test)]
#[path = "message_item_test.rs"]
mod tests;
