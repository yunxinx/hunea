use crate::{
    Model,
    document::{DocumentAnchorRegion, DocumentLayout, DocumentViewport},
    frame_time::FrameRenderContext,
    message::assistant_message_visual_inset,
};

use super::{SelectionPoint, SelectionState, apply_selection_to_line, selection_columns_for_line};

/// `VisibleSelectableRange` 表示当前 viewport 中一段可见的选区高亮投影。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct VisibleSelectableRange {
    pub(crate) viewport_row: usize,
    pub(crate) start_column: usize,
    pub(crate) end_column: usize,
}

impl Model {
    pub(crate) fn selection_point_for_mouse_with_layout(
        &self,
        column: u16,
        row: u16,
        layout: &DocumentLayout,
        context: FrameRenderContext,
    ) -> Option<SelectionPoint> {
        let line = *self
            .document_viewport_line_indices(layout)
            .get(usize::from(row))?;
        let selection_line = layout.selection_line_at(line, context)?;
        let column =
            self.selection_column_for_display_column(usize::from(column), line, layout, context)?;
        selection_line
            .selectable
            .point_for_mouse_down(selection_line.anchor, column)
    }

    pub(crate) fn selection_point_for_drag_mouse(
        &mut self,
        column: u16,
        row: u16,
    ) -> Option<SelectionPoint> {
        let context = FrameRenderContext::capture();
        let layout = self.build_document_layout(context);
        self.selection_point_for_drag_mouse_with_layout(column, row, &layout, context)
    }

    pub(crate) fn selection_point_for_drag_mouse_with_layout(
        &self,
        column: u16,
        row: u16,
        layout: &DocumentLayout,
        context: FrameRenderContext,
    ) -> Option<SelectionPoint> {
        let line_indices = self.document_viewport_line_indices(layout);
        if line_indices.is_empty() {
            return None;
        }

        let clamped_row = usize::from(row).min(line_indices.len().saturating_sub(1));
        let line = *line_indices.get(clamped_row)?;
        let selection_line = layout.selection_line_at(line, context)?;
        let mut column =
            self.selection_column_for_display_column(usize::from(column), line, layout, context)?;
        if column + 1 == usize::from(self.width) {
            column = column.saturating_add(1);
        }
        selection_line
            .selectable
            .point_for_drag(selection_line.anchor, column)
    }

    pub(crate) fn document_viewport_line_indices(&self, layout: &DocumentLayout) -> Vec<usize> {
        self.document_viewport_line_indices_for_mode(
            layout,
            self.document_runtime.viewport_state.resolved_offset(),
            self.document_runtime.viewport_state.follow_bottom(),
            self.document_runtime.viewport_state.manual_scroll(),
        )
    }

    pub(crate) fn maybe_clear_selection_for_bottom_status_slot_change(&mut self) {
        if !self.selection_runtime.selection.is_active() {
            return;
        }

        let context = FrameRenderContext::capture();
        let layout = self.build_document_layout(context);
        if selection_intersects_status_line(&layout, self.selection_runtime.selection, context) {
            self.clear_selection();
        }
    }

    fn selection_column_for_display_column(
        &self,
        column: usize,
        line: usize,
        layout: &DocumentLayout,
        context: FrameRenderContext,
    ) -> Option<usize> {
        if !layout.is_assistant_message_line(line, context) {
            return Some(column);
        }

        let inset = usize::from(assistant_message_visual_inset(self.width));
        if inset == 0 {
            return Some(column);
        }
        if column < inset {
            return Some(0);
        }

        Some(column - inset)
    }
}

fn selection_intersects_status_line(
    layout: &DocumentLayout,
    selection: SelectionState,
    context: FrameRenderContext,
) -> bool {
    let Some((start, end)) = selection.ordered_points(layout, context) else {
        return false;
    };

    for line in start.line()..=end.line() {
        if layout
            .line_anchor_at(line, context)
            .is_some_and(|anchor| anchor.region == DocumentAnchorRegion::StatusLine)
        {
            return true;
        }
    }

    false
}

pub(crate) fn visible_selection_ranges(
    viewport: &DocumentViewport,
    layout: &DocumentLayout,
    selection: SelectionState,
    context: FrameRenderContext,
) -> Vec<VisibleSelectableRange> {
    let Some((start, end)) = selection.ordered_points(layout, context) else {
        return Vec::new();
    };

    let mut visible = Vec::new();
    for viewport_row in 0..viewport.lines.len() {
        let absolute_line = viewport.resolved_offset + viewport_row;
        if absolute_line < start.line() || absolute_line > end.line() {
            continue;
        }

        let Some(selection_line) = layout.selection_line_at(absolute_line, context) else {
            continue;
        };
        let Some((start_column, end_column)) = selection_columns_for_line(
            selection,
            layout,
            absolute_line,
            selection_line.selectable,
            context,
        ) else {
            continue;
        };
        visible.push(VisibleSelectableRange {
            viewport_row,
            start_column,
            end_column,
        });
    }

    visible
}

pub(crate) fn apply_selection_to_viewport(
    viewport: &mut DocumentViewport,
    layout: &DocumentLayout,
    selection: SelectionState,
    context: FrameRenderContext,
) {
    for visible in visible_selection_ranges(viewport, layout, selection, context) {
        if let Some(line) = viewport.lines.get_mut(visible.viewport_row) {
            *line = apply_selection_to_line(line, visible.start_column, visible.end_column);
        }
    }
}
