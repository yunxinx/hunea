//! Composer mouse interaction helpers owned by the TUI model.

use crate::{
    AppEffect, Model, composer,
    document::{DocumentAnchorRegion, DocumentLayout},
    frame_time::FrameRenderContext,
    selection::{MousePosition, SelectionPoint, selection_auto_scroll_direction_for_mouse_row},
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

/// `ComposerMouseOutcome` 区分 composer 手势是否已经被专门处理。
pub(crate) enum ComposerMouseOutcome {
    Ignored,
    Handled(Option<AppEffect>),
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
        &self,
        column: u16,
        row: u16,
        layout: &DocumentLayout,
        context: FrameRenderContext,
    ) -> Option<PendingComposerCursorClick> {
        if self.composer.value().is_empty() {
            return None;
        }

        let line_indices = self.document_viewport_line_indices(layout);
        let line = *line_indices.get(usize::from(row))?;
        let line_data = layout.line_at(line, context)?;
        if line_data.anchor.region != DocumentAnchorRegion::Composer {
            return None;
        }

        let selectable = line_data.selectable;
        let selection_point = selectable.point_for_drag(line_data.anchor, usize::from(column))?;
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
        &self,
        click: PendingComposerCursorClick,
        column: u16,
        row: u16,
        layout: &DocumentLayout,
        context: FrameRenderContext,
    ) -> bool {
        if !click.hit_content {
            return false;
        }

        let line_indices = self.document_viewport_line_indices(layout);
        let Some(line) = line_indices.get(usize::from(row)).copied() else {
            return false;
        };
        let Some(line_data) = layout.line_at(line, context) else {
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
        &self,
        click: PendingComposerCursorClick,
        row: u16,
        layout: &DocumentLayout,
        context: FrameRenderContext,
    ) -> bool {
        if selection_auto_scroll_direction_for_mouse_row(row, self.document_viewport_height())
            == Default::default()
        {
            return false;
        }

        let line_indices = self.document_viewport_line_indices(layout);
        let Some(line) = line_indices.get(usize::from(row)).copied() else {
            return false;
        };
        layout.line_at(line, context).is_some_and(|line_data| {
            line_data.anchor.region == DocumentAnchorRegion::Composer
                && line_data.anchor == click.selection_point.anchor()
        })
    }

    pub(crate) fn handle_composer_selection_mouse_down(
        &mut self,
        column: u16,
        row: u16,
        layout: &DocumentLayout,
        context: FrameRenderContext,
    ) -> ComposerMouseOutcome {
        let Some(click) = self.composer_cursor_click_for_mouse(column, row, layout, context) else {
            return ComposerMouseOutcome::Ignored;
        };

        if !click.hit_content && click.line_has_content {
            self.reset_selection_click();
            self.clear_selection_range();
            self.pending_composer_cursor_click = click;
            return ComposerMouseOutcome::Handled(None);
        }

        match self.register_selection_click(click.selection_point, context.now()) {
            2 => {
                self.clear_pending_composer_cursor_click();
                if self.select_word_at_point(click.selection_point, layout, context) {
                    return ComposerMouseOutcome::Handled(None);
                }
            }
            3 => {
                self.clear_pending_composer_cursor_click();
                self.select_line_at_point(click.selection_point, layout, context);
                return ComposerMouseOutcome::Handled(None);
            }
            _ => {}
        }

        self.clear_selection_range();
        self.pending_composer_cursor_click = click;
        ComposerMouseOutcome::Handled(None)
    }

    pub(crate) fn handle_pending_composer_mouse_up(
        &mut self,
        column: u16,
        row: u16,
    ) -> ComposerMouseOutcome {
        if !self.pending_composer_cursor_click.active {
            return ComposerMouseOutcome::Ignored;
        }

        let click = self.pending_composer_cursor_click;
        self.clear_pending_composer_cursor_click();
        let context = FrameRenderContext::capture();
        let layout = self.build_document_layout(context);

        if let Some(release_click) =
            self.composer_cursor_click_for_mouse(column, row, &layout, context)
            && self.same_composer_cursor_target(click, release_click)
            && !self.is_composer_end_gutter_drag(click, column, row, &layout, context)
        {
            self.clear_selection_range();
            self.handle_composer_cursor_click(release_click);
            return ComposerMouseOutcome::Handled(None);
        }

        if let Some(point) =
            self.selection_point_for_drag_mouse_with_layout(column, row, &layout, context)
            && point != click.selection_point
        {
            self.start_selection(click.selection_point);
            self.finish_selection(point);
            self.reset_selection_click();
            if self.copy_on_mouse_selection_release
                && self
                    .selection_runtime
                    .selection
                    .ordered_points(&layout, context)
                    .is_some()
            {
                return ComposerMouseOutcome::Handled(self.request_copy_selection());
            }
            return ComposerMouseOutcome::Handled(None);
        }

        if column != click.column || row != click.row {
            self.reset_selection_click();
            self.clear_selection_range();
            return ComposerMouseOutcome::Handled(None);
        }

        self.clear_selection_range();
        self.handle_composer_cursor_click(click);
        ComposerMouseOutcome::Handled(None)
    }

    pub(crate) fn handle_pending_composer_mouse_drag(
        &mut self,
        column: u16,
        row: u16,
    ) -> ComposerMouseOutcome {
        if !self.pending_composer_cursor_click.active {
            return ComposerMouseOutcome::Ignored;
        }

        let click = self.pending_composer_cursor_click;
        let context = FrameRenderContext::capture();
        let layout = self.build_document_layout(context);
        if let Some(motion_click) =
            self.composer_cursor_click_for_mouse(column, row, &layout, context)
            && self.same_composer_cursor_target(click, motion_click)
        {
            if self.is_composer_end_gutter_drag(click, column, row, &layout, context) {
                self.start_selection(click.selection_point);
                self.clear_pending_composer_cursor_click();
                if let Some(point) =
                    self.selection_point_for_drag_mouse_with_layout(column, row, &layout, context)
                {
                    self.update_selection_focus(point);
                }
                self.update_selection_auto_scroll(MousePosition::new(column, row));
                return ComposerMouseOutcome::Handled(None);
            }

            if self.is_composer_edge_clamped_motion(click, row, &layout, context) {
                if click.edge_motions == 0 {
                    self.pending_composer_cursor_click = PendingComposerCursorClick {
                        edge_motions: 1,
                        ..click
                    };
                    return ComposerMouseOutcome::Handled(None);
                }

                self.start_selection(click.selection_point);
                self.clear_pending_composer_cursor_click();
                self.update_selection_auto_scroll(MousePosition::new(column, row));
                return ComposerMouseOutcome::Handled(None);
            }

            return ComposerMouseOutcome::Handled(None);
        }

        let point = self.selection_point_for_drag_mouse_with_layout(column, row, &layout, context);
        let left_viewport = usize::from(row) >= self.document_viewport_height();
        if point.is_none() || point == Some(click.selection_point) {
            if left_viewport || self.is_composer_edge_clamped_motion(click, row, &layout, context) {
                self.start_selection(click.selection_point);
                self.clear_pending_composer_cursor_click();
                self.update_selection_auto_scroll(MousePosition::new(column, row));
                return ComposerMouseOutcome::Handled(None);
            }

            return ComposerMouseOutcome::Handled(None);
        }

        self.start_selection(click.selection_point);
        self.clear_pending_composer_cursor_click();
        self.update_selection_focus(point.expect("point checked to exist"));
        self.update_selection_auto_scroll(MousePosition::new(column, row));
        ComposerMouseOutcome::Handled(None)
    }
}
