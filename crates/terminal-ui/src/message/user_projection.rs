use std::rc::Rc;

#[cfg(test)]
use std::cell::Cell;

use ratatui::text::Line;
use runtime_domain::session::TranscriptSkillBinding;

use crate::{
    StyleMode,
    composer::ComposerSourceMessage,
    selection::SelectableLineRange,
    theme::{SurfaceHalf, TerminalPalette},
    transcript::{ItemLineAnchor, LineAnchorKind, PromptVisualLine, wrap_prompt_visual_lines},
};

use super::{
    UserMessageRenderLayout,
    user::{
        has_visible_user_message_frame, measure_width, projected_compact_user_plain_line,
        projected_compact_user_plain_line_len, projected_framed_user_plain_line,
        projected_framed_user_plain_line_len, projected_legacy_user_plain_line,
        projected_legacy_user_plain_line_len, render_projected_compact_user_line,
        render_projected_framed_user_line, render_projected_legacy_user_line, rendered_line_anchor,
        user_message_compact_content_width, user_message_inset_width, user_message_layout,
        user_message_legacy_content_width, user_message_surface_padding_line,
    },
};

#[cfg(test)]
thread_local! {
    static USER_MESSAGE_PROJECTION_PLAIN_LINE_LEN_CALL_COUNT: Cell<usize> = const { Cell::new(0) };
}

/// `UserMessageRenderProjection` 保存用户消息在固定宽度下的轻量投影视图。
#[derive(Debug, Clone)]
pub(crate) struct UserMessageRenderProjection {
    lines: Rc<Vec<UserMessageProjectedLine>>,
    pub(super) layout: UserMessageRenderLayout,
    pub(super) has_frame: bool,
    pub(super) skill_bindings: Rc<Vec<TranscriptSkillBinding>>,
    palette: TerminalPalette,
    style_mode: StyleMode,
}

#[derive(Debug, Clone)]
pub(super) struct UserMessageProjectedLine {
    // transcript render cache 只会消费渲染文本与 anchor 元数据，不需要列映射。
    pub(super) text: String,
    pub(super) logical_line: usize,
    pub(super) visible_start_char: usize,
    pub(super) end_char: usize,
}

impl From<PromptVisualLine> for UserMessageProjectedLine {
    fn from(line: PromptVisualLine) -> Self {
        Self {
            text: line.text,
            logical_line: line.logical_line,
            visible_start_char: line.visible_start_char,
            end_char: line.end_char,
        }
    }
}

pub(super) struct UserMessageWrapSnapshot {
    pub(super) lines: Vec<crate::transcript::PromptVisualLine>,
    pub(super) layout: UserMessageRenderLayout,
    pub(super) has_frame: bool,
}

impl UserMessageRenderProjection {
    pub(crate) fn line_count(&self) -> usize {
        self.lines.len() + usize::from(self.has_frame) * 2
    }

    pub(crate) fn line_at(&self, index: usize) -> Option<Line<'static>> {
        if self.has_frame && self.is_frame_line(index) {
            let half = if index == 0 {
                SurfaceHalf::Lower
            } else {
                SurfaceHalf::Upper
            };
            return Some(user_message_surface_padding_line(
                self.layout.frame_width,
                self.palette,
                half,
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
                &self.skill_bindings,
            ),
            StyleMode::Cc => render_projected_compact_user_line(
                line,
                is_first,
                self.layout.frame_width.max(1),
                self.palette,
                self.style_mode,
                &self.skill_bindings,
            ),
            StyleMode::Ms => render_projected_legacy_user_line(
                line,
                is_first,
                self.palette,
                self.style_mode,
                &self.skill_bindings,
            ),
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

pub(super) fn user_message_wrap_snapshot(
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

pub(super) fn render_user_message_projection(
    content: &str,
    width: u16,
    palette: TerminalPalette,
    style_mode: StyleMode,
    source_message: Option<&ComposerSourceMessage>,
) -> UserMessageRenderProjection {
    let UserMessageWrapSnapshot {
        lines,
        layout,
        has_frame,
    } = user_message_wrap_snapshot(content, width, palette, style_mode);
    UserMessageRenderProjection {
        lines: Rc::new(
            lines
                .into_iter()
                .map(UserMessageProjectedLine::from)
                .collect(),
        ),
        layout,
        has_frame,
        skill_bindings: Rc::new(
            source_message
                .map(|message| message.skill_bindings().to_vec())
                .unwrap_or_default(),
        ),
        palette,
        style_mode,
    }
}

pub(super) fn render_user_message_selectable_line_ranges(
    content: &str,
    width: u16,
    palette: TerminalPalette,
    style_mode: StyleMode,
) -> Vec<SelectableLineRange> {
    let snapshot = user_message_wrap_snapshot(content, width, palette, style_mode);
    let mut ranges = Vec::with_capacity(snapshot.lines.len() + usize::from(snapshot.has_frame) * 2);

    if snapshot.has_frame {
        ranges.push(SelectableLineRange::default());
    }

    for line in &snapshot.lines {
        let line_width = measure_width(&line.text);
        if line_width == 0 {
            let hit_end = if snapshot.layout.frame_width > 0 {
                snapshot.layout.frame_width
            } else {
                snapshot.layout.line_prefix_width.max(1)
            };
            ranges.push(SelectableLineRange::blank_hit_range(0, hit_end));
            continue;
        }

        ranges.push(SelectableLineRange::with_hit_range(
            snapshot.layout.line_prefix_width,
            snapshot.layout.line_prefix_width + line_width,
            0,
            snapshot.layout.line_prefix_width + line_width,
        ));
    }

    if snapshot.has_frame {
        ranges.push(SelectableLineRange::default());
    }

    ranges
}

#[cfg(test)]
pub(crate) fn reset_user_message_projection_plain_line_len_call_count() {
    USER_MESSAGE_PROJECTION_PLAIN_LINE_LEN_CALL_COUNT.set(0);
}

#[cfg(test)]
pub(crate) fn user_message_projection_plain_line_len_call_count() -> usize {
    USER_MESSAGE_PROJECTION_PLAIN_LINE_LEN_CALL_COUNT.get()
}
