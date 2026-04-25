use std::time::Instant;

use crossterm::event::MouseButton;

use crate::frontend::tui::{AppEffect, Model, composer_mouse::PendingComposerCursorClick};

use super::{AutoScrollDirection, MousePosition};

impl Model {
    pub(crate) fn handle_mouse_down(
        &mut self,
        button: MouseButton,
        column: u16,
        row: u16,
    ) -> Option<AppEffect> {
        if let Some(effect) = self.handle_history_scroll_indicator_mouse_down(button, column, row) {
            return effect;
        }

        self.clear_history_scroll_indicator();
        self.cancel_exit_confirmation();

        match button {
            MouseButton::Middle => {
                self.clear_pending_composer_cursor_click();
                self.reset_selection_click();
                self.request_copy_selection()
            }
            MouseButton::Left => {
                self.stop_selection_auto_scroll();
                let layout = self.build_document_layout();
                if let Some(click) = self.composer_cursor_click_for_mouse(column, row) {
                    if !click.hit_content && click.line_has_content {
                        self.reset_selection_click();
                        self.clear_selection_range();
                        self.pending_composer_cursor_click = click;
                        return None;
                    }

                    match self.register_selection_click(click.selection_point, Instant::now()) {
                        2 => {
                            self.clear_pending_composer_cursor_click();
                            if self.select_word_at_point(click.selection_point, &layout) {
                                return None;
                            }
                        }
                        3 => {
                            self.clear_pending_composer_cursor_click();
                            self.select_line_at_point(click.selection_point, &layout);
                            return None;
                        }
                        _ => {}
                    }

                    self.clear_selection_range();
                    self.pending_composer_cursor_click = click;
                    return None;
                }

                self.clear_pending_composer_cursor_click();
                if let Some(point) =
                    self.selection_point_for_mouse_with_layout(column, row, &layout)
                {
                    match self.register_selection_click(point, Instant::now()) {
                        2 if self.select_word_at_point(point, &layout) => return None,
                        3 => {
                            self.select_line_at_point(point, &layout);
                            return None;
                        }
                        _ => {
                            self.start_selection(point);
                            return None;
                        }
                    }
                }

                self.clear_selection();
                None
            }
            _ => None,
        }
    }

    pub(crate) fn handle_mouse_up(
        &mut self,
        button: MouseButton,
        column: u16,
        row: u16,
    ) -> Option<AppEffect> {
        self.clear_history_scroll_indicator();
        self.cancel_exit_confirmation();

        if button == MouseButton::Left && self.pending_composer_cursor_click.active {
            let click = self.pending_composer_cursor_click;
            self.clear_pending_composer_cursor_click();

            if let Some(release_click) = self.composer_cursor_click_for_mouse(column, row)
                && self.same_composer_cursor_target(click, release_click)
                && !self.is_composer_end_gutter_drag(click, column, row)
            {
                self.clear_selection_range();
                self.handle_composer_cursor_click(release_click);
                return None;
            }

            if let Some(point) = self.selection_point_for_drag_mouse(column, row)
                && point != click.selection_point
            {
                self.start_selection(click.selection_point);
                self.finish_selection(point);
                self.reset_selection_click();
                let layout = self.build_document_layout();
                if self.copy_on_mouse_selection_release
                    && self
                        .selection_runtime
                        .selection
                        .ordered_points(&layout)
                        .is_some()
                {
                    return self.request_copy_selection();
                }
                return None;
            }

            if column != click.column || row != click.row {
                self.reset_selection_click();
                self.clear_selection_range();
                return None;
            }

            self.clear_selection_range();
            self.handle_composer_cursor_click(click);
            return None;
        }

        if button != MouseButton::Left || !self.selection_runtime.selection.is_active() {
            return None;
        }

        let was_dragging = self.selection_runtime.selection.is_dragging();
        self.stop_selection_auto_scroll();
        if was_dragging {
            if let Some(point) = self.selection_point_for_drag_mouse(column, row) {
                self.finish_selection(point);
            } else {
                self.selection_runtime.selection.stop_drag();
            }
        }

        let layout = self.build_document_layout();
        let completed_drag_selection = was_dragging
            && self
                .selection_runtime
                .selection
                .ordered_points(&layout)
                .is_some();
        if completed_drag_selection {
            self.reset_selection_click();
        }
        if self.copy_on_mouse_selection_release && completed_drag_selection {
            return self.request_copy_selection();
        }

        None
    }

    pub(crate) fn handle_mouse_drag(
        &mut self,
        button: MouseButton,
        column: u16,
        row: u16,
    ) -> Option<AppEffect> {
        self.cancel_exit_confirmation();
        if button != MouseButton::Left {
            return None;
        }

        if self.pending_composer_cursor_click.active {
            let click = self.pending_composer_cursor_click;
            if let Some(motion_click) = self.composer_cursor_click_for_mouse(column, row)
                && self.same_composer_cursor_target(click, motion_click)
            {
                if self.is_composer_end_gutter_drag(click, column, row) {
                    self.start_selection(click.selection_point);
                    self.clear_pending_composer_cursor_click();
                    if let Some(point) = self.selection_point_for_drag_mouse(column, row) {
                        self.update_selection_focus(point);
                    }
                    self.update_selection_auto_scroll(MousePosition::new(column, row));
                    return None;
                }

                if self.is_composer_edge_clamped_motion(click, row) {
                    if click.edge_motions == 0 {
                        self.pending_composer_cursor_click = PendingComposerCursorClick {
                            edge_motions: 1,
                            ..click
                        };
                        return None;
                    }

                    self.start_selection(click.selection_point);
                    self.clear_pending_composer_cursor_click();
                    self.update_selection_auto_scroll(MousePosition::new(column, row));
                    return None;
                }

                return None;
            }

            let point = self.selection_point_for_drag_mouse(column, row);
            let left_viewport = usize::from(row) >= self.document_viewport_height();
            if point.is_none() || point == Some(click.selection_point) {
                if left_viewport || self.is_composer_edge_clamped_motion(click, row) {
                    self.start_selection(click.selection_point);
                    self.clear_pending_composer_cursor_click();
                    self.update_selection_auto_scroll(MousePosition::new(column, row));
                    return None;
                }

                return None;
            }

            self.start_selection(click.selection_point);
            self.clear_pending_composer_cursor_click();
            self.update_selection_focus(point.expect("point checked to exist"));
            self.update_selection_auto_scroll(MousePosition::new(column, row));
            return None;
        }

        if !self.selection_runtime.selection.is_dragging() {
            return None;
        }

        if let Some(point) = self.selection_point_for_drag_mouse(column, row) {
            self.update_selection_focus(point);
        }
        self.update_selection_auto_scroll(MousePosition::new(column, row));
        None
    }

    pub(crate) fn handle_selection_auto_scroll_tick(&mut self, token: usize) {
        if !self.selection_runtime.selection.is_dragging()
            || self.selection_runtime.auto_scroll_direction == AutoScrollDirection::None
            || token != self.selection_runtime.auto_scroll_token
        {
            return;
        }

        let previous_viewport_y = self.document_runtime.viewport_y;
        match self.selection_runtime.auto_scroll_direction {
            AutoScrollDirection::Down => self.scroll_document_by(1),
            AutoScrollDirection::Up => self.scroll_document_by(-1),
            AutoScrollDirection::None => {}
        }

        if self.document_runtime.viewport_y == previous_viewport_y {
            self.stop_selection_auto_scroll();
            return;
        }

        if let Some(point) = self.selection_point_for_drag_mouse(
            self.selection_runtime.auto_scroll_mouse.column(),
            self.selection_runtime.auto_scroll_mouse.row(),
        ) {
            self.update_selection_focus(point);
        }
        self.arm_selection_auto_scroll();
    }
}
