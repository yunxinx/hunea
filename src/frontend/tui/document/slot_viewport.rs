use ratatui::text::Line;

use super::{DocumentLayout, DocumentViewport};
use crate::frontend::tui::Model;

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
        let anchor_line = layout.cursor_y;
        if !layout.composer_slot.has_padding() {
            if !self.composer.value().is_empty() {
                return layout.lines.len().saturating_sub(1);
            }
            return anchor_line;
        }

        if layout.composer_slot.frame_line_count <= self.document_viewport_height() {
            return layout.composer_slot.frame_bottom_line();
        }

        anchor_line
    }
}

pub(crate) fn bottom_follow_viewport_line_indices(
    layout: &DocumentLayout,
    height: usize,
    presentation: BottomFollowPresentation,
) -> Vec<usize> {
    if layout.lines.is_empty() {
        return Vec::new();
    }

    let anchor_line = presentation.anchor_line.min(layout.lines.len() - 1);
    if height == 0 {
        return (0..=anchor_line).collect();
    }

    let start = anchor_line.saturating_add(1).saturating_sub(height);
    (start..=anchor_line).collect()
}

pub(crate) fn compose_bottom_follow_document_viewport(
    layout: &DocumentLayout,
    height: usize,
    presentation: BottomFollowPresentation,
) -> DocumentViewport {
    compose_document_viewport_from_line_indices(
        layout,
        &bottom_follow_viewport_line_indices(layout, height, presentation),
    )
}

pub(crate) fn compose_document_viewport_from_line_indices(
    layout: &DocumentLayout,
    line_indices: &[usize],
) -> DocumentViewport {
    if line_indices.is_empty() {
        return DocumentViewport {
            lines: vec![Line::raw("")],
            plain_lines: vec![String::new()],
            resolved_offset: 0,
        };
    }

    let mut lines = Vec::with_capacity(line_indices.len());
    let mut plain_lines = Vec::with_capacity(line_indices.len());
    for &index in line_indices {
        if let Some(line) = layout.lines.get(index) {
            lines.push(line.clone());
            plain_lines.push(layout.plain_lines.get(index).cloned().unwrap_or_default());
        }
    }

    DocumentViewport {
        lines,
        plain_lines,
        resolved_offset: line_indices[0],
    }
}
