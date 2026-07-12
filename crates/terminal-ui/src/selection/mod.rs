mod copy;
mod mouse;
mod policy;
mod range;
mod render;
mod state;
mod viewport;
use std::time::Instant;

use crate::{
    AppEffect, Model, display_width::display_width, document::DocumentLayout,
    frame_time::FrameRenderContext, toast::ToastSeverity,
};

pub(super) use self::copy::selection_text;
pub(super) use self::range::{
    SelectableLineRange, normalize_transcript_selectable_range, selectable_range_for_plain_line,
    selection_columns_for_line, selection_ends_before_line_content, word_selection_columns,
};
pub(super) use self::render::apply_selection_to_line;
pub(super) use self::state::{
    AutoScrollDirection, MousePosition, ResolvedSelectionPoint, SELECTION_AUTO_SCROLL_INTERVAL,
    SelectionClickState, SelectionPoint, SelectionState,
    selection_auto_scroll_direction_for_mouse_row,
};
pub(super) use self::viewport::apply_selection_to_viewport;

const SELECTION_COPIED_NOTICE_TEXT: &str = "Selection copied";
const SELECTION_COPY_FAILED_NOTICE_TEXT: &str = "Copy selection failed";

impl Model {
    pub(crate) fn handle_selection_copy_completed(&mut self, success: bool) {
        if success {
            self.show_toast(ToastSeverity::Info, SELECTION_COPIED_NOTICE_TEXT);
        } else {
            self.show_toast(ToastSeverity::Error, SELECTION_COPY_FAILED_NOTICE_TEXT);
        }
    }

    pub(crate) fn invalidate_selection_for_reflow(&mut self) {
        self.stop_selection_auto_scroll();
        self.clear_selection();
    }

    pub(crate) fn start_selection(&mut self, point: SelectionPoint) {
        let mut next = SelectionState::default();
        next.begin(point);
        if self.selection_runtime.selection == next {
            return;
        }

        self.selection_runtime.selection = next;
        self.mark_selection_changed();
    }

    pub(crate) fn update_selection_focus(&mut self, point: SelectionPoint) {
        if !self.selection_runtime.selection.is_active()
            || self.selection_runtime.selection.focus() == point
        {
            return;
        }

        self.selection_runtime.selection.update_focus(point);
        self.mark_selection_changed();
    }

    pub(crate) fn finish_selection(&mut self, point: SelectionPoint) {
        if !self.selection_runtime.selection.is_active() {
            return;
        }
        if self.selection_runtime.selection.focus() == point
            && !self.selection_runtime.selection.is_dragging()
        {
            return;
        }

        self.selection_runtime.selection.finish(point);
        self.mark_selection_changed();
    }

    pub(crate) fn clear_selection(&mut self) {
        let selection_changed = self.selection_runtime.selection != SelectionState::default();
        let click_changed = self.selection_runtime.click != SelectionClickState::default();
        if !selection_changed && !click_changed {
            return;
        }

        self.reset_selection_click();
        if !selection_changed {
            return;
        }

        self.selection_runtime.selection.clear();
        self.mark_selection_changed();
    }

    pub(crate) fn clear_selection_range(&mut self) {
        if self.selection_runtime.selection == SelectionState::default() {
            return;
        }

        self.selection_runtime.selection.clear();
        self.mark_selection_changed();
    }

    pub(crate) fn reset_selection_click(&mut self) {
        self.selection_runtime.click.clear();
    }

    pub(crate) fn register_selection_click(&mut self, point: SelectionPoint, at: Instant) -> u8 {
        self.selection_runtime.click.register(point, at)
    }

    pub(crate) fn select_word_at_point(
        &mut self,
        point: SelectionPoint,
        layout: &DocumentLayout,
        context: FrameRenderContext,
    ) -> bool {
        let Some(line) = layout.selection_line_for_anchor(point.anchor(), context) else {
            return false;
        };
        let Some((start_column, end_column)) = word_selection_columns(&line.text, point.column())
        else {
            return false;
        };

        self.selection_runtime.selection.select_range(
            SelectionPoint::new(point.anchor(), start_column),
            SelectionPoint::new(point.anchor(), end_column),
        );
        self.mark_selection_changed();
        true
    }

    pub(crate) fn select_line_at_point(
        &mut self,
        point: SelectionPoint,
        layout: &DocumentLayout,
        context: FrameRenderContext,
    ) {
        let Some(line_index) = layout.line_index_for_anchor(point.anchor(), context) else {
            return;
        };
        let selectable = layout
            .selection_line_at(line_index, context)
            .map(|line_data| line_data.selectable)
            .unwrap_or_default();
        let start_column = selectable
            .content_columns()
            .map(|(start_column, _)| start_column)
            .unwrap_or_default();
        let focus = if line_index + 1 < layout.line_count() {
            match layout.line_anchor_at(line_index + 1, context) {
                Some(next_anchor) => SelectionPoint::new(next_anchor, 0),
                None => SelectionPoint::new(
                    point.anchor(),
                    line_selection_end_column(line_index, layout, context),
                ),
            }
        } else {
            SelectionPoint::new(
                point.anchor(),
                line_selection_end_column(line_index, layout, context),
            )
        };

        self.selection_runtime
            .selection
            .select_range(SelectionPoint::new(point.anchor(), start_column), focus);
        self.mark_selection_changed();
    }

    pub(crate) fn update_selection_auto_scroll(&mut self, mouse: MousePosition) {
        self.selection_runtime.auto_scroll_mouse = mouse;
        let next_direction = selection_auto_scroll_direction_for_mouse_row(
            mouse.row(),
            self.document_viewport_height(),
        );
        if next_direction == AutoScrollDirection::None {
            self.stop_selection_auto_scroll();
            return;
        }
        if self.selection_runtime.auto_scroll_direction == next_direction
            && self.selection_runtime.auto_scroll_deadline.is_some()
        {
            return;
        }

        self.selection_runtime.auto_scroll_direction = next_direction;
        self.selection_runtime.auto_scroll_token += 1;
        self.arm_selection_auto_scroll();
    }

    pub(crate) fn stop_selection_auto_scroll(&mut self) {
        self.selection_runtime.auto_scroll_direction = AutoScrollDirection::None;
        self.selection_runtime.auto_scroll_deadline = None;
        self.selection_runtime.auto_scroll_mouse = MousePosition::default();
    }

    pub(crate) fn request_copy_selection(&mut self) -> Option<AppEffect> {
        self.ensure_selection_range_exact();
        let context = crate::frame_time::FrameRenderContext::capture();
        let layout = self.build_document_layout(context);
        let text = selection_text(&layout, self.selection_runtime.selection, context)?;
        if text.is_empty() {
            return None;
        }

        Some(AppEffect::CopySelection(text))
    }

    fn mark_selection_changed(&mut self) {
        self.selection_runtime.version += 1;
        self.invalidate_document_viewport_cache();
    }

    fn ensure_selection_range_exact(&mut self) {
        let Some((start_item, end_item)) = self.selected_transcript_item_range() else {
            return;
        };
        if self.transcript_render.index.metrics[start_item..end_item]
            .iter()
            .all(|metrics| metrics.is_exact())
        {
            return;
        }

        let preserved_viewport_state = self.preserved_viewport_state_for_transcript_refresh();
        self.transcript.exactize_item_range(start_item, end_item);
        let index = self.transcript.progressive_item_metrics_index();
        self.transcript_render =
            std::rc::Rc::new(crate::transcript::index_only_render_result(index));
        self.transcript_render_version += 1;
        self.document_runtime.transcript_cache = Default::default();
        self.document_runtime.layout_cache = Default::default();
        self.document_runtime.viewport_cache = Default::default();
        self.sync_document_viewport_after_transcript_refresh(preserved_viewport_state);
    }

    fn selected_transcript_item_range(&self) -> Option<(usize, usize)> {
        if !self.selection_runtime.selection.is_active() {
            return None;
        }

        let item_count = self.transcript.len();
        if item_count == 0 {
            return None;
        }

        let anchor = self.selection_runtime.selection.anchor().anchor();
        let focus = self.selection_runtime.selection.focus().anchor();
        let transcript_region = crate::document::DocumentAnchorRegion::Transcript;
        let (start_item, end_item) = match (anchor.region, focus.region) {
            (region, other_region)
                if region == transcript_region && other_region == transcript_region =>
            {
                (
                    anchor
                        .transcript
                        .item_index
                        .min(focus.transcript.item_index),
                    anchor
                        .transcript
                        .item_index
                        .max(focus.transcript.item_index)
                        .saturating_add(1),
                )
            }
            (region, _) if region == transcript_region => {
                // transcript 之后只剩 tail，跨区选中时 transcript 端会一直覆盖到末尾。
                (anchor.transcript.item_index, item_count)
            }
            (_, region) if region == transcript_region => {
                // 反向拖拽时归一化后的覆盖范围相同，仍然是 transcript 端到末尾。
                (focus.transcript.item_index, item_count)
            }
            _ => return None,
        };
        let start_item = start_item.min(item_count);
        let end_item = end_item.min(item_count);
        (start_item < end_item).then_some((start_item, end_item))
    }

    fn arm_selection_auto_scroll(&mut self) {
        self.selection_runtime.auto_scroll_deadline =
            Some(Instant::now() + SELECTION_AUTO_SCROLL_INTERVAL);
    }
}

fn line_selection_end_column(
    line_index: usize,
    layout: &DocumentLayout,
    context: crate::frame_time::FrameRenderContext,
) -> usize {
    layout
        .selection_line_at(line_index, context)
        .map(|line| {
            line.selectable
                .content_columns()
                .map_or_else(|| display_width(&line.text), |(_, end_column)| end_column)
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests;
