mod copy;
mod mouse;
mod range;
mod render;
mod state;
mod viewport;
use std::time::Instant;

use unicode_width::UnicodeWidthStr;

use crate::frontend::tui::{AppEffect, Model, document::DocumentLayout};

pub(super) use self::copy::selection_text;
pub(super) use self::range::{
    SelectableLineRange, normalize_transcript_selectable_range, selectable_range_for_plain_line,
    selection_columns_for_line, selection_ends_before_line_content, word_selection_columns,
};
pub(super) use self::render::apply_selection_to_line;
pub(super) use self::state::{
    AutoScrollDirection, MousePosition, SELECTION_AUTO_SCROLL_INTERVAL, SelectionClickState,
    SelectionPoint, SelectionState, selection_auto_scroll_direction_for_mouse_row,
};
pub(super) use self::viewport::apply_selection_to_viewport;

const SELECTION_COPIED_NOTICE_TEXT: &str = "Selection copied";
const SELECTION_COPY_FAILED_NOTICE_TEXT: &str = "Copy selection failed";

impl Model {
    pub(crate) fn handle_selection_copy_completed(&mut self, success: bool) {
        if success {
            self.show_transient_status_notice(SELECTION_COPIED_NOTICE_TEXT);
        } else {
            self.show_transient_status_notice(SELECTION_COPY_FAILED_NOTICE_TEXT);
        }
    }

    pub(crate) fn invalidate_selection_for_reflow(&mut self) {
        self.stop_selection_auto_scroll();
        self.clear_selection();
    }

    pub(crate) fn start_selection(&mut self, point: SelectionPoint) {
        let mut next = SelectionState::default();
        next.begin(point);
        if self.selection == next {
            return;
        }

        self.selection = next;
        self.mark_selection_changed();
    }

    pub(crate) fn update_selection_focus(&mut self, point: SelectionPoint) {
        if !self.selection.is_active() || self.selection.focus() == point {
            return;
        }

        self.selection.update_focus(point);
        self.mark_selection_changed();
    }

    pub(crate) fn finish_selection(&mut self, point: SelectionPoint) {
        if !self.selection.is_active() {
            return;
        }
        if self.selection.focus() == point && !self.selection.is_dragging() {
            return;
        }

        self.selection.finish(point);
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

        self.selection.clear();
        self.mark_selection_changed();
    }

    pub(crate) fn clear_selection_range(&mut self) {
        if self.selection == SelectionState::default() {
            return;
        }

        self.selection.clear();
        self.mark_selection_changed();
    }

    pub(crate) fn reset_selection_click(&mut self) {
        self.selection_click.clear();
    }

    pub(crate) fn register_selection_click(&mut self, point: SelectionPoint, at: Instant) -> u8 {
        self.selection_click.register(point, at)
    }

    pub(crate) fn select_word_at_point(
        &mut self,
        point: SelectionPoint,
        layout: &DocumentLayout,
    ) -> bool {
        let Some(line) = layout.line_text_at(point.line()) else {
            return false;
        };
        let Some((start_column, end_column)) = word_selection_columns(&line, point.column()) else {
            return false;
        };

        self.selection.select_range(
            SelectionPoint::new(point.line(), start_column),
            SelectionPoint::new(point.line(), end_column),
        );
        self.mark_selection_changed();
        true
    }

    pub(crate) fn select_line_at_point(&mut self, line: usize, layout: &DocumentLayout) {
        let selectable = layout
            .line_at(line)
            .map(|line_data| line_data.selectable)
            .unwrap_or_default();
        let start_column = selectable
            .content_columns()
            .map(|(start_column, _)| start_column)
            .unwrap_or_default();
        let focus = if line + 1 < layout.line_count() {
            SelectionPoint::new(line + 1, 0)
        } else {
            SelectionPoint::new(
                line,
                selectable
                    .content_columns()
                    .map(|(_, end_column)| end_column)
                    .unwrap_or_else(|| {
                        layout
                            .line_text_at(line)
                            .map(|text| text.width())
                            .unwrap_or_default()
                    }),
            )
        };

        self.selection
            .select_range(SelectionPoint::new(line, start_column), focus);
        self.mark_selection_changed();
    }

    pub(crate) fn update_selection_auto_scroll(&mut self, mouse: MousePosition) {
        self.selection_auto_scroll_mouse = mouse;
        let next_direction = selection_auto_scroll_direction_for_mouse_row(
            mouse.row(),
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
        self.invalidate_document_viewport_cache();
    }

    fn arm_selection_auto_scroll(&mut self) {
        self.selection_auto_scroll_deadline = Some(Instant::now() + SELECTION_AUTO_SCROLL_INTERVAL);
    }
}
