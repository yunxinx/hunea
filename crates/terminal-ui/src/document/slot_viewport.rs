use ratatui::text::Line;

use super::{DocumentLayout, DocumentViewport};
use crate::{Model, frame_time::FrameRenderContext};

/// `BottomFollowPresentation` 描述底部跟随视图的临时展示决策。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct BottomFollowPresentation {
    pub(crate) anchor_line: usize,
}

impl Model {
    pub(crate) fn bottom_follow_presentation(
        &self,
        layout: &DocumentLayout,
    ) -> BottomFollowPresentation {
        BottomFollowPresentation {
            anchor_line: self.bottom_follow_anchor_line(layout),
        }
    }

    pub(crate) fn bottom_follow_anchor_line(&self, layout: &DocumentLayout) -> usize {
        if !layout_has_content_below_composer_frame(layout) {
            if layout.composer_slot.frame_line_count <= self.document_viewport_height()
                && let Some(frame_bottom_line) = layout.composer_slot.frame_bottom_line()
            {
                return frame_bottom_line;
            }
            return layout.cursor_y;
        }

        let lines_below_cursor = layout.line_count().saturating_sub(1 + layout.cursor_y);
        if lines_below_cursor < self.document_viewport_height() {
            return layout.line_count().saturating_sub(1);
        }

        layout.cursor_y
    }
}

pub(crate) fn bottom_follow_viewport_line_indices(
    layout: &DocumentLayout,
    height: usize,
    presentation: BottomFollowPresentation,
) -> Vec<usize> {
    if layout.line_count() == 0 {
        return Vec::new();
    }

    let anchor_line = presentation.anchor_line.min(layout.line_count() - 1);
    if height == 0 {
        return (0..=anchor_line).collect();
    }

    let start = anchor_line.saturating_add(1).saturating_sub(height);
    (start..=anchor_line).collect()
}

pub(crate) fn offset_viewport_line_indices(
    layout: &DocumentLayout,
    offset: usize,
    height: usize,
) -> Vec<usize> {
    if layout.line_count() == 0 {
        return Vec::new();
    }

    if height == 0 {
        return (0..layout.line_count()).collect();
    }

    let max_offset = layout.line_count().saturating_sub(height);
    let resolved_offset = offset.min(max_offset);
    let end = (resolved_offset + height).min(layout.line_count());
    (resolved_offset..end).collect()
}

pub(crate) fn compose_bottom_follow_document_viewport(
    layout: &DocumentLayout,
    height: usize,
    presentation: BottomFollowPresentation,
    context: FrameRenderContext,
) -> DocumentViewport {
    compose_document_viewport_from_line_indices(
        layout,
        &bottom_follow_viewport_line_indices(layout, height, presentation),
        context,
    )
}

pub(crate) fn compose_document_viewport_from_line_indices(
    layout: &DocumentLayout,
    line_indices: &[usize],
    context: FrameRenderContext,
) -> DocumentViewport {
    if line_indices.is_empty() {
        return DocumentViewport {
            lines: vec![Line::raw("")],
            assistant_lines: vec![false],
            plain_text_len: 0,
            #[cfg(test)]
            plain_lines: vec![String::new()],
            resolved_offset: 0,
        };
    }

    if let Some((start, count)) = contiguous_line_range(line_indices) {
        return DocumentViewport {
            lines: layout.lines_for_range(start, count, context),
            assistant_lines: assistant_flags_for_line_indices(layout, line_indices, context),
            plain_text_len: layout.plain_text_len_for_range(start, count, context),
            #[cfg(test)]
            plain_lines: layout.line_texts_for_range(start, count, context),
            resolved_offset: start,
        };
    }

    let mut lines = Vec::with_capacity(line_indices.len());
    let mut assistant_lines = Vec::with_capacity(line_indices.len());
    let mut plain_text_len = 0;
    #[cfg(test)]
    let mut plain_lines = Vec::with_capacity(line_indices.len());
    for &index in line_indices {
        if let Some(line) = layout.line_at(index, context) {
            if !lines.is_empty() {
                plain_text_len += 1;
            }
            plain_text_len += line.plain_line.len();
            lines.push(line.line);
            assistant_lines.push(layout.is_assistant_message_line(index, context));
            #[cfg(test)]
            plain_lines.push(line.plain_line);
        }
    }
    DocumentViewport {
        lines,
        assistant_lines,
        plain_text_len,
        #[cfg(test)]
        plain_lines,
        resolved_offset: line_indices[0],
    }
}

fn layout_has_content_below_composer_frame(layout: &DocumentLayout) -> bool {
    layout
        .composer_slot
        .frame_bottom_line()
        .map_or(layout.line_count() > 0, |frame_bottom_line| {
            frame_bottom_line < layout.line_count().saturating_sub(1)
        })
}

fn contiguous_line_range(line_indices: &[usize]) -> Option<(usize, usize)> {
    let start = *line_indices.first()?;
    for (offset, index) in line_indices.iter().copied().enumerate().skip(1) {
        if index != start + offset {
            return None;
        }
    }

    Some((start, line_indices.len()))
}

fn assistant_flags_for_line_indices(
    layout: &DocumentLayout,
    line_indices: &[usize],
    context: FrameRenderContext,
) -> Vec<bool> {
    line_indices
        .iter()
        .map(|index| layout.is_assistant_message_line(*index, context))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::{DocumentTailLayout, slot_frame::SlotFrame};
    use crate::{Model, StartupBannerOptions};

    #[test]
    fn bottom_follow_keeps_legacy_composer_anchor_without_status_line() {
        let mut model = Model::new(StartupBannerOptions::default());
        model.height = 2;
        model.has_window = true;

        let layout = DocumentLayout {
            tail: std::rc::Rc::new(DocumentTailLayout::from_test_parts(
                vec![
                    Line::raw("frame-top"),
                    Line::raw("input-line"),
                    Line::raw("frame-bottom"),
                ],
                vec![
                    "frame-top".to_string(),
                    "input-line".to_string(),
                    "frame-bottom".to_string(),
                ],
                Vec::new(),
                Vec::new(),
                SlotFrame::new(0, true, 1),
                2,
                1,
            )),
            composer_slot: SlotFrame::new(0, true, 1),
            cursor_x: 2,
            cursor_y: 1,
            ..DocumentLayout::default()
        };

        let viewport = compose_bottom_follow_document_viewport(
            &layout,
            model.document_viewport_height(),
            model.bottom_follow_presentation(&layout),
            FrameRenderContext::capture(),
        );

        assert_eq!(
            viewport.plain_lines,
            vec!["frame-top".to_string(), "input-line".to_string()]
        );
        assert_eq!(viewport.resolved_offset, 0);
    }

    #[test]
    fn bottom_follow_can_include_status_line_below_composer() {
        let mut model = Model::new(StartupBannerOptions::default());
        model.height = 3;
        model.has_window = true;

        let layout = DocumentLayout {
            tail: std::rc::Rc::new(DocumentTailLayout::from_test_parts(
                vec![
                    Line::raw("frame-top"),
                    Line::raw("input-line"),
                    Line::raw("frame-bottom"),
                    Line::raw("status-line"),
                ],
                vec![
                    "frame-top".to_string(),
                    "input-line".to_string(),
                    "frame-bottom".to_string(),
                    "status-line".to_string(),
                ],
                Vec::new(),
                Vec::new(),
                SlotFrame::new(0, true, 1),
                2,
                1,
            )),
            composer_slot: SlotFrame::new(0, true, 1),
            cursor_x: 2,
            cursor_y: 1,
            ..DocumentLayout::default()
        };

        let viewport = compose_bottom_follow_document_viewport(
            &layout,
            model.document_viewport_height(),
            model.bottom_follow_presentation(&layout),
            FrameRenderContext::capture(),
        );

        assert_eq!(
            viewport.plain_lines,
            vec![
                "input-line".to_string(),
                "frame-bottom".to_string(),
                "status-line".to_string()
            ]
        );
        assert_eq!(viewport.resolved_offset, 1);
    }
}
