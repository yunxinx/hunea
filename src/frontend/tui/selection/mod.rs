mod copy;
mod range;
mod render;
mod state;

use std::time::Instant;

use crossterm::event::MouseButton;
use unicode_width::UnicodeWidthStr;

use crate::frontend::tui::{
    AppEffect, Model,
    document::{
        DocumentAnchorRegion, DocumentLayout, DocumentViewport,
        bottom_follow_viewport_line_indices, offset_viewport_line_indices,
    },
};

pub(crate) use self::copy::selection_text;
pub(crate) use self::range::{
    SelectableLineRange, normalize_transcript_selectable_range, selectable_range_for_plain_line,
    selection_columns_for_line, selection_ends_before_line_content, word_selection_columns,
};
pub(crate) use self::render::apply_selection_to_line;
pub(crate) use self::state::{
    AutoScrollDirection, MousePosition, SELECTION_AUTO_SCROLL_INTERVAL,
    SELECTION_MULTI_CLICK_WINDOW, SelectionClickState, SelectionPoint, SelectionState,
    selection_auto_scroll_direction_for_mouse_row,
};

const SELECTION_COPIED_NOTICE_TEXT: &str = "Selection copied";
const SELECTION_COPY_FAILED_NOTICE_TEXT: &str = "Copy selection failed";

impl Model {
    pub(crate) fn handle_mouse_down(
        &mut self,
        button: MouseButton,
        column: u16,
        row: u16,
    ) -> Option<AppEffect> {
        self.cancel_exit_confirmation();

        match button {
            MouseButton::Middle => {
                self.reset_selection_click();
                self.request_copy_selection()
            }
            MouseButton::Left => {
                self.stop_selection_auto_scroll();
                let layout = self.build_document_layout();
                if let Some(point) =
                    self.selection_point_for_mouse_with_layout(column, row, &layout)
                {
                    match self.register_selection_click(point, Instant::now()) {
                        2 if self.select_word_at_point(point, &layout) => return None,
                        3 => {
                            self.select_line_at_point(point.line, &layout);
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
        self.cancel_exit_confirmation();

        if button != MouseButton::Left || !self.selection.active {
            return None;
        }

        let was_dragging = self.selection.dragging;
        self.stop_selection_auto_scroll();
        if was_dragging {
            if let Some(point) = self.selection_point_for_drag_mouse(column, row) {
                self.finish_selection(point);
            } else {
                self.selection.dragging = false;
            }
        }

        let completed_drag_selection = was_dragging && self.selection.ordered_points().is_some();
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
        if button != MouseButton::Left || !self.selection.dragging {
            return None;
        }

        if let Some(point) = self.selection_point_for_drag_mouse(column, row) {
            self.update_selection_focus(point);
        }
        self.update_selection_auto_scroll(MousePosition { column, row });
        None
    }

    pub(crate) fn handle_selection_auto_scroll_tick(&mut self, token: usize) {
        if !self.selection.dragging
            || self.selection_auto_scroll_direction == AutoScrollDirection::None
            || token != self.selection_auto_scroll_token
        {
            return;
        }

        let previous_viewport_y = self.document_viewport_y;
        match self.selection_auto_scroll_direction {
            AutoScrollDirection::Down => self.scroll_document_by(1),
            AutoScrollDirection::Up => self.scroll_document_by(-1),
            AutoScrollDirection::None => {}
        }

        if self.document_viewport_y == previous_viewport_y {
            self.stop_selection_auto_scroll();
            return;
        }

        if let Some(point) = self.selection_point_for_drag_mouse(
            self.selection_auto_scroll_mouse.column,
            self.selection_auto_scroll_mouse.row,
        ) {
            self.update_selection_focus(point);
        }
        self.arm_selection_auto_scroll();
    }

    pub(crate) fn handle_selection_copy_completed(&mut self, success: bool) {
        if success {
            self.show_transient_status_notice(SELECTION_COPIED_NOTICE_TEXT);
        } else {
            self.show_transient_status_notice(SELECTION_COPY_FAILED_NOTICE_TEXT);
        }
    }

    pub(crate) fn maybe_clear_selection_for_bottom_status_slot_change(&mut self) {
        if !self.selection.active {
            return;
        }

        let layout = self.build_document_layout();
        if selection_intersects_status_line(&layout, self.selection) {
            self.clear_selection();
        }
    }

    pub(crate) fn invalidate_selection_for_reflow(&mut self) {
        self.stop_selection_auto_scroll();
        self.clear_selection();
    }

    pub(crate) fn start_selection(&mut self, point: SelectionPoint) {
        let next = SelectionState {
            active: true,
            dragging: true,
            anchor: point,
            focus: point,
        };
        if self.selection == next {
            return;
        }

        self.selection = next;
        self.mark_selection_changed();
    }

    pub(crate) fn update_selection_focus(&mut self, point: SelectionPoint) {
        if !self.selection.active || self.selection.focus == point {
            return;
        }

        self.selection.focus = point;
        self.mark_selection_changed();
    }

    pub(crate) fn finish_selection(&mut self, point: SelectionPoint) {
        if !self.selection.active {
            return;
        }
        if self.selection.focus == point && !self.selection.dragging {
            return;
        }

        self.selection.focus = point;
        self.selection.dragging = false;
        self.mark_selection_changed();
    }

    pub(crate) fn clear_selection(&mut self) {
        let selection_changed = self.selection != SelectionState::default();
        let click_changed = self.selection_click != SelectionClickState::default();
        if !selection_changed && !click_changed {
            return;
        }

        self.reset_selection_click();
        if !selection_changed {
            return;
        }

        self.selection = SelectionState::default();
        self.mark_selection_changed();
    }

    pub(crate) fn reset_selection_click(&mut self) {
        self.selection_click = SelectionClickState::default();
    }

    pub(crate) fn register_selection_click(&mut self, point: SelectionPoint, at: Instant) -> u8 {
        let mut next_count = 1;
        if let Some(previous_at) = self.selection_click.at
            && at.duration_since(previous_at) <= SELECTION_MULTI_CLICK_WINDOW
            && self.selection_click.point.line == point.line
            && self.selection_click.point.column.abs_diff(point.column) <= 1
        {
            next_count = self.selection_click.count.saturating_add(1);
            if next_count > 3 {
                next_count = 1;
            }
        }

        self.selection_click = SelectionClickState {
            point,
            count: next_count,
            at: Some(at),
        };
        next_count
    }

    pub(crate) fn select_word_at_point(
        &mut self,
        point: SelectionPoint,
        layout: &DocumentLayout,
    ) -> bool {
        let Some(line) = layout.plain_lines.get(point.line) else {
            return false;
        };
        let Some((start_column, end_column)) = word_selection_columns(line, point.column) else {
            return false;
        };

        self.selection = SelectionState {
            active: true,
            dragging: false,
            anchor: SelectionPoint {
                line: point.line,
                column: start_column,
            },
            focus: SelectionPoint {
                line: point.line,
                column: end_column,
            },
        };
        self.mark_selection_changed();
        true
    }

    pub(crate) fn select_line_at_point(&mut self, line: usize, layout: &DocumentLayout) {
        let selectable = layout.selectable.get(line).copied().unwrap_or_default();
        let start_column = if selectable.has_content() {
            selectable.start_column
        } else {
            0
        };
        let focus = if line + 1 < layout.plain_lines.len() {
            SelectionPoint {
                line: line + 1,
                column: 0,
            }
        } else {
            SelectionPoint {
                line,
                column: if selectable.has_content() {
                    selectable.end_column
                } else {
                    layout
                        .plain_lines
                        .get(line)
                        .map(|text| text.width())
                        .unwrap_or_default()
                },
            }
        };

        self.selection = SelectionState {
            active: true,
            dragging: false,
            anchor: SelectionPoint {
                line,
                column: start_column,
            },
            focus,
        };
        self.mark_selection_changed();
    }

    pub(crate) fn selection_point_for_mouse_with_layout(
        &self,
        column: u16,
        row: u16,
        layout: &DocumentLayout,
    ) -> Option<SelectionPoint> {
        let line = *self
            .document_viewport_line_indices(layout)
            .get(usize::from(row))?;
        let selectable = layout.selectable.get(line).copied().unwrap_or_default();
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
        let selectable = layout.selectable.get(line).copied().unwrap_or_default();
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

    pub(crate) fn update_selection_auto_scroll(&mut self, mouse: MousePosition) {
        self.selection_auto_scroll_mouse = mouse;
        let next_direction = selection_auto_scroll_direction_for_mouse_row(
            mouse.row,
            self.document_viewport_height(),
        );
        if next_direction == AutoScrollDirection::None {
            self.stop_selection_auto_scroll();
            return;
        }
        if self.selection_auto_scroll_direction == next_direction
            && self.selection_auto_scroll_deadline.is_some()
        {
            return;
        }

        self.selection_auto_scroll_direction = next_direction;
        self.selection_auto_scroll_token += 1;
        self.arm_selection_auto_scroll();
    }

    pub(crate) fn stop_selection_auto_scroll(&mut self) {
        self.selection_auto_scroll_direction = AutoScrollDirection::None;
        self.selection_auto_scroll_deadline = None;
        self.selection_auto_scroll_mouse = MousePosition::default();
    }

    pub(crate) fn request_copy_selection(&mut self) -> Option<AppEffect> {
        let layout = self.build_document_layout();
        let text = selection_text(&layout, self.selection)?;
        if text.is_empty() {
            return None;
        }

        Some(AppEffect::CopySelection(text))
    }

    fn mark_selection_changed(&mut self) {
        self.selection_version += 1;
        self.document_viewport_cache.valid = false;
    }

    fn arm_selection_auto_scroll(&mut self) {
        self.selection_auto_scroll_deadline = Some(Instant::now() + SELECTION_AUTO_SCROLL_INTERVAL);
    }
}

fn selection_intersects_status_line(layout: &DocumentLayout, selection: SelectionState) -> bool {
    let Some((start, end)) = selection.ordered_points() else {
        return false;
    };

    for line in start.line..=end.line {
        if layout
            .anchors
            .get(line)
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

    Some(SelectionPoint {
        line,
        column: if selectable.has_content() { column } else { 0 },
    })
}

fn selection_point_for_drag_on_selectable_line(
    column: usize,
    line: usize,
    selectable: SelectableLineRange,
) -> Option<SelectionPoint> {
    selectable.has_anchor().then_some(SelectionPoint {
        line,
        column: selectable.clamp(column),
    })
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
        if absolute_line < start.line || absolute_line > end.line {
            continue;
        }

        let Some(selectable) = layout.selectable.get(absolute_line).copied() else {
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
