use crate::frontend::tui::{
    Model, composer,
    document::DocumentAnchorRegion,
    selection::{
        SelectableLineRange, SelectionPoint, selection_auto_scroll_direction_for_mouse_row,
    },
};

/// `PendingComposerCursorClick` 暂存一次 composer 单击待定位的鼠标落点。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct PendingComposerCursorClick {
    pub(crate) active: bool,
    pub(crate) hit_content: bool,
    pub(crate) line_has_content: bool,
    pub(crate) edge_motions: u8,
    pub(crate) column: u16,
    pub(crate) row: u16,
    pub(crate) selection_point: SelectionPoint,
    pub(crate) logical_line: usize,
    pub(crate) logical_column: usize,
}

impl Model {
    pub(crate) fn clear_pending_composer_cursor_click(&mut self) {
        self.pending_composer_cursor_click = PendingComposerCursorClick::default();
    }

    pub(crate) fn maybe_clear_pending_composer_cursor_click_for_bottom_status_slot_change(
        &mut self,
    ) {
        if !self.pending_composer_cursor_click.active {
            return;
        }

        self.clear_pending_composer_cursor_click();
        self.reset_selection_click();
    }

    pub(crate) fn composer_cursor_click_for_mouse(
        &mut self,
        column: u16,
        row: u16,
    ) -> Option<PendingComposerCursorClick> {
        if self.composer.value().is_empty() {
            return None;
        }

        let layout = self.build_document_layout();
        let line_indices = self.document_viewport_line_indices(&layout);
        let line = *line_indices.get(usize::from(row))?;
        let line_data = layout.line_at(line)?;
        if line_data.anchor.region != DocumentAnchorRegion::Composer {
            return None;
        }

        let selectable = line_data.selectable;
        let selection_point = selection_point_for_drag_selectable_line(
            usize::from(column),
            line_data.anchor,
            selectable,
        )?;
        let (logical_line, logical_column) = composer::cursor_position_for_line_anchor_click(
            &self.composer,
            line_data.anchor.composer,
            usize::from(column),
        )?;

        Some(PendingComposerCursorClick {
            active: true,
            hit_content: selectable.contains_content(usize::from(column)),
            line_has_content: selectable.has_content(),
            edge_motions: 0,
            column,
            row,
            selection_point,
            logical_line,
            logical_column,
        })
    }

    pub(crate) fn same_composer_cursor_target(
        &self,
        left: PendingComposerCursorClick,
        right: PendingComposerCursorClick,
    ) -> bool {
        left.logical_line == right.logical_line && left.logical_column == right.logical_column
    }

    pub(crate) fn handle_composer_cursor_click(&mut self, click: PendingComposerCursorClick) {
        let old_value = self.composer_text().to_string();
        let old_line = self.composer.line();
        let old_column = self.composer.column();

        composer::move_cursor_to_logical_position(
            &mut self.composer,
            click.logical_line,
            click.logical_column,
        );
        self.sync_document_viewport_after_composer_interaction(&old_value, old_line, old_column);
    }

    pub(crate) fn is_composer_end_gutter_drag(
        &mut self,
        click: PendingComposerCursorClick,
        column: u16,
        row: u16,
    ) -> bool {
        if !click.hit_content {
            return false;
        }

        let layout = self.build_document_layout();
        let line_indices = self.document_viewport_line_indices(&layout);
        let Some(line) = line_indices.get(usize::from(row)).copied() else {
            return false;
        };
        let Some(line_data) = layout.line_at(line) else {
            return false;
        };
        if line_data.anchor.region != DocumentAnchorRegion::Composer
            || line_data.anchor != click.selection_point.anchor()
            || click.logical_column != line_data.anchor.composer.end_char
        {
            return false;
        }

        line_data.selectable.has_content()
            && line_data
                .selectable
                .content_columns()
                .is_some_and(|(_, end_column)| usize::from(column) >= end_column)
    }

    pub(crate) fn is_composer_edge_clamped_motion(
        &mut self,
        click: PendingComposerCursorClick,
        row: u16,
    ) -> bool {
        if selection_auto_scroll_direction_for_mouse_row(row, self.document_viewport_height())
            == Default::default()
        {
            return false;
        }

        let layout = self.build_document_layout();
        let line_indices = self.document_viewport_line_indices(&layout);
        let Some(line) = line_indices.get(usize::from(row)).copied() else {
            return false;
        };
        layout.line_at(line).is_some_and(|line_data| {
            line_data.anchor.region == DocumentAnchorRegion::Composer
                && line_data.anchor == click.selection_point.anchor()
        })
    }
}

fn selection_point_for_drag_selectable_line(
    column: usize,
    anchor: crate::frontend::tui::document::DocumentLineAnchor,
    selectable: SelectableLineRange,
) -> Option<SelectionPoint> {
    selectable
        .has_anchor()
        .then_some(SelectionPoint::new(anchor, selectable.clamp(column)))
}
