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
        let Some(line) = layout.selection_line_for_anchor(point.anchor()) else {
            return false;
        };
        let Some((start_column, end_column)) = word_selection_columns(&line.text, point.column())
        else {
            return false;
        };

        self.selection.select_range(
            SelectionPoint::new(point.anchor(), start_column),
            SelectionPoint::new(point.anchor(), end_column),
        );
        self.mark_selection_changed();
        true
    }

    pub(crate) fn select_line_at_point(&mut self, point: SelectionPoint, layout: &DocumentLayout) {
        let Some(line_index) = layout.line_index_for_anchor(point.anchor()) else {
            return;
        };
        let selectable = layout
            .selection_line_at(line_index)
            .map(|line_data| line_data.selectable)
            .unwrap_or_default();
        let start_column = selectable
            .content_columns()
            .map(|(start_column, _)| start_column)
            .unwrap_or_default();
        let focus = if line_index + 1 < layout.line_count() {
            let next_anchor = layout
                .line_anchor_at(line_index + 1)
                .expect("next line should expose an anchor");
            SelectionPoint::new(next_anchor, 0)
        } else {
            SelectionPoint::new(
                point.anchor(),
                selectable
                    .content_columns()
                    .map(|(_, end_column)| end_column)
                    .unwrap_or_else(|| {
                        layout
                            .selection_line_at(line_index)
                            .map(|line| line.text.width())
                            .unwrap_or_default()
                    }),
            )
        };

        self.selection
            .select_range(SelectionPoint::new(point.anchor(), start_column), focus);
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
        self.ensure_selection_range_exact();
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
        self.transcript_render = std::rc::Rc::new(
            crate::frontend::tui::transcript::index_only_render_result(index),
        );
        self.transcript_render_version += 1;
        self.document_transcript_cache = Default::default();
        self.document_layout_cache = Default::default();
        self.document_viewport_cache = Default::default();
        self.sync_document_viewport_after_transcript_refresh(preserved_viewport_state);
        self.schedule_transcript_refinement();
    }

    fn selected_transcript_item_range(&self) -> Option<(usize, usize)> {
        if !self.selection.is_active() {
            return None;
        }

        let item_count = self.transcript.len();
        if item_count == 0 {
            return None;
        }

        let anchor = self.selection.anchor().anchor();
        let focus = self.selection.focus().anchor();
        let transcript_region = crate::frontend::tui::document::DocumentAnchorRegion::Transcript;
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
        self.selection_auto_scroll_deadline = Some(Instant::now() + SELECTION_AUTO_SCROLL_INTERVAL);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frontend::tui::{
        AppEffect, HeroOptions, Sender,
        document::{DocumentAnchorRegion, DocumentLineAnchor},
        theme::default_palette,
        transcript::{
            LineAnchor, index_only_render_result, materialize_transcript_item_render_block,
        },
    };

    #[test]
    fn transcript_selection_survives_append_and_copies_using_anchor_bound_range() {
        let mut model = Model::new(HeroOptions::default());
        model.set_window(24, 6);
        model.set_palette(default_palette(), true);
        model.transcript_mut().clear();
        model
            .transcript_mut()
            .append_message(Sender::Assistant, "alpha");
        model.sync_transcript_render();

        let layout = model.build_document_layout();
        let anchor = model
            .selection_point_for_mouse_with_layout(1, 0, &layout)
            .expect("selection should start inside the transcript line");
        let focus = model
            .selection_point_for_drag_mouse(5, 0)
            .expect("drag selection should clamp to the line end");
        model.start_selection(anchor);
        model.finish_selection(focus);

        model
            .transcript_mut()
            .append_message(Sender::Assistant, "beta");
        model.sync_transcript_render();

        assert_eq!(
            model.request_copy_selection(),
            Some(AppEffect::CopySelection("lpha".to_string()))
        );
    }

    #[test]
    fn request_copy_selection_exactizes_the_selected_transcript_range_on_demand() {
        let mut model = Model::new(HeroOptions::default());
        model.set_window(18, 6);
        model.set_palette(default_palette(), true);
        model.transcript_mut().clear();
        model.transcript_mut().set_gap(0);
        for index in 0..48 {
            model.transcript_mut().append_message(
                Sender::Assistant,
                format!(
                    "item {index}: alpha beta gamma delta epsilon zeta eta theta iota kappa lambda mu"
                ),
            );
        }
        model.sync_transcript_render();

        let start_block = materialize_transcript_item_render_block(
            model.transcript.item(0).expect("item 0 should exist"),
            18,
            default_palette(),
        );
        let end_block = materialize_transcript_item_render_block(
            model.transcript.item(2).expect("item 2 should exist"),
            18,
            default_palette(),
        );
        let start = SelectionPoint::new(
            DocumentLineAnchor {
                region: DocumentAnchorRegion::Transcript,
                transcript: LineAnchor {
                    item_index: 0,
                    item_anchor: start_block
                        .anchor_at(0)
                        .expect("first item should expose anchors"),
                },
                ..DocumentLineAnchor::default()
            },
            0,
        );
        let end = SelectionPoint::new(
            DocumentLineAnchor {
                region: DocumentAnchorRegion::Transcript,
                transcript: LineAnchor {
                    item_index: 2,
                    item_anchor: end_block
                        .anchor_at(0)
                        .expect("third item should expose anchors"),
                },
                ..DocumentLineAnchor::default()
            },
            4,
        );

        model.start_selection(start);
        model.finish_selection(end);

        let copied = model.request_copy_selection();
        assert!(
            matches!(copied, Some(AppEffect::CopySelection(text)) if !text.is_empty()),
            "copying a transcript selection should still produce text after on-demand exactization"
        );
        assert!(
            model.transcript_render.index.metrics[0..3]
                .iter()
                .all(|metrics| metrics.is_exact()),
            "selection copy should exactize the selected transcript item range before reading it"
        );
        assert!(
            model
                .transcript_render
                .index
                .metrics
                .iter()
                .enumerate()
                .any(|(item_index, metrics)| { item_index > 8 && metrics.is_estimated() }),
            "selection-driven exactization should stay local instead of settling the whole transcript"
        );
    }

    #[test]
    fn request_copy_selection_exactizes_transcript_tail_when_selection_crosses_into_composer() {
        let mut model = Model::new(HeroOptions::default());
        model.set_window(18, 6);
        model.set_palette(default_palette(), true);
        model.transcript_mut().clear();
        model.transcript_mut().set_gap(0);
        for index in 0..48 {
            model.transcript_mut().append_message(
                Sender::Assistant,
                format!(
                    "item {index}: alpha beta gamma delta epsilon zeta eta theta iota kappa lambda mu"
                ),
            );
        }
        model.sync_transcript_render();
        model
            .composer_mut()
            .set_text_for_test("draft line one\ndraft line two");
        model.sync_composer_height();

        let selected_start_item = 30;
        let start_block = materialize_transcript_item_render_block(
            model
                .transcript
                .item(selected_start_item)
                .expect("selected transcript item should exist"),
            18,
            default_palette(),
        );
        let start = SelectionPoint::new(
            DocumentLineAnchor {
                region: DocumentAnchorRegion::Transcript,
                transcript: LineAnchor {
                    item_index: selected_start_item,
                    item_anchor: start_block
                        .anchor_at(0)
                        .expect("selected transcript item should expose anchors"),
                },
                ..DocumentLineAnchor::default()
            },
            0,
        );

        let layout = model.build_document_layout();
        let (composer_anchor, composer_end_column) = (0..layout.line_count())
            .find_map(|line_index| {
                let line = layout.selection_line_at(line_index)?;
                if line.anchor.region != DocumentAnchorRegion::Composer {
                    return None;
                }

                let (_, end_column) = line.selectable.content_columns()?;
                Some((line.anchor, end_column))
            })
            .expect("composer selection line should exist");
        let end = SelectionPoint::new(composer_anchor, composer_end_column);

        model.start_selection(start);
        model.finish_selection(end);

        let mut expected = model.clone();
        expected
            .transcript
            .exactize_item_range(selected_start_item, expected.transcript.len());
        let index = expected.transcript.progressive_item_metrics_index();
        expected.transcript_render = std::rc::Rc::new(index_only_render_result(index));
        expected.transcript_render_version += 1;
        expected.document_transcript_cache = Default::default();
        expected.document_layout_cache = Default::default();
        expected.document_viewport_cache = Default::default();

        let copied = model.request_copy_selection();
        let expected_copied = expected.request_copy_selection();

        assert_eq!(
            copied, expected_copied,
            "copying across transcript and composer should read the transcript tail with exact metrics"
        );
        assert!(
            model.transcript_render.index.metrics[selected_start_item..]
                .iter()
                .all(|metrics| metrics.is_exact()),
            "copying a mixed transcript/tail selection should exactize the covered transcript tail before reading it"
        );
        assert!(
            model.transcript_render.index.metrics[..selected_start_item]
                .iter()
                .any(|metrics| metrics.is_estimated()),
            "mixed-selection exactization should stay local to the covered transcript tail"
        );
    }
}
