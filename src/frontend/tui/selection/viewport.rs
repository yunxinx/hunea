use crate::frontend::tui::{
    Model,
    document::{
        DocumentAnchorRegion, DocumentLayout, DocumentViewport,
        bottom_follow_viewport_line_indices, offset_viewport_line_indices,
    },
};

use super::{
    SelectableLineRange, SelectionPoint, SelectionState, apply_selection_to_line,
    selection_columns_for_line,
};

impl Model {
    pub(crate) fn selection_point_for_mouse_with_layout(
        &self,
        column: u16,
        row: u16,
        layout: &DocumentLayout,
    ) -> Option<SelectionPoint> {
        let line = *self
            .document_viewport_line_indices(layout)
            .get(usize::from(row))?;
        let selectable = layout
            .line_at(line)
            .map(|line_data| line_data.selectable)
            .unwrap_or_default();
        selection_point_for_selectable_line(usize::from(column), line, selectable)
    }

    pub(crate) fn selection_point_for_drag_mouse(
        &mut self,
        column: u16,
        row: u16,
    ) -> Option<SelectionPoint> {
        let layout = self.build_document_layout();
        let line_indices = self.document_viewport_line_indices(&layout);
        if line_indices.is_empty() {
            return None;
        }

        let clamped_row = usize::from(row).min(line_indices.len().saturating_sub(1));
        let line = *line_indices.get(clamped_row)?;
        let selectable = layout
            .line_at(line)
            .map(|line_data| line_data.selectable)
            .unwrap_or_default();
        selection_point_for_drag_on_selectable_line(usize::from(column), line, selectable)
    }

    pub(crate) fn document_viewport_line_indices(&self, layout: &DocumentLayout) -> Vec<usize> {
        if self.follow_bottom && !self.manual_document_scroll {
            return bottom_follow_viewport_line_indices(
                layout,
                self.document_viewport_height(),
                self.bottom_follow_presentation(layout),
            );
        }

        offset_viewport_line_indices(
            layout,
            self.document_viewport_y,
            self.document_viewport_height(),
        )
    }

    pub(crate) fn maybe_clear_selection_for_bottom_status_slot_change(&mut self) {
        if !self.selection.is_active() {
            return;
        }

        let layout = self.build_document_layout();
        if selection_intersects_status_line(&layout, self.selection) {
            self.clear_selection();
        }
    }
}

fn selection_intersects_status_line(layout: &DocumentLayout, selection: SelectionState) -> bool {
    let Some((start, end)) = selection.ordered_points() else {
        return false;
    };

    for line in start.line()..=end.line() {
        if layout
            .line_anchor_at(line)
            .is_some_and(|anchor| anchor.region == DocumentAnchorRegion::StatusLine)
        {
            return true;
        }
    }

    false
}

fn selection_point_for_selectable_line(
    column: usize,
    line: usize,
    selectable: SelectableLineRange,
) -> Option<SelectionPoint> {
    if !selectable.contains(column) {
        return None;
    }

    Some(SelectionPoint::new(
        line,
        if selectable.has_content() { column } else { 0 },
    ))
}

fn selection_point_for_drag_on_selectable_line(
    column: usize,
    line: usize,
    selectable: SelectableLineRange,
) -> Option<SelectionPoint> {
    selectable
        .has_anchor()
        .then_some(SelectionPoint::new(line, selectable.clamp(column)))
}

pub(crate) fn apply_selection_to_viewport(
    viewport: &mut DocumentViewport,
    layout: &DocumentLayout,
    selection: SelectionState,
) {
    let Some((start, end)) = selection.ordered_points() else {
        return;
    };

    for (index, line) in viewport.lines.iter_mut().enumerate() {
        let absolute_line = viewport.resolved_offset + index;
        if absolute_line < start.line() || absolute_line > end.line() {
            continue;
        }

        let Some(selectable) = layout
            .line_at(absolute_line)
            .map(|line_data| line_data.selectable)
        else {
            continue;
        };
        let Some((start_column, end_column)) =
            selection_columns_for_line(selection, absolute_line, selectable)
        else {
            continue;
        };

        *line = apply_selection_to_line(line, start_column, end_column);
    }
}
