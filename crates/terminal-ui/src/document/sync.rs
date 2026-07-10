use std::cmp::Ordering;

use crate::Model;

use super::{
    DocumentLayout, ViewportState, bottom_follow_viewport_line_indices,
    manual_scroll::crossed_manual_document_scroll_restore_target, offset_viewport_line_indices,
};

const DOCUMENT_MOUSE_WHEEL_DELTA: isize = 3;

impl Model {
    pub(crate) fn document_mouse_wheel_delta() -> isize {
        DOCUMENT_MOUSE_WHEEL_DELTA
    }

    pub(crate) fn preserved_viewport_state_for_transcript_refresh(
        &mut self,
    ) -> Option<ViewportState> {
        self.document_runtime
            .manual_scroll
            .then(|| self.current_document_viewport_state())
    }

    pub(crate) fn current_document_viewport_state(&mut self) -> ViewportState {
        let layout = self.build_document_layout(crate::frame_time::FrameRenderContext::capture());
        self.capture_viewport_state_with_layout(
            &layout,
            self.document_runtime.viewport_y,
            self.document_runtime.follow_bottom,
            self.document_runtime.manual_scroll,
        )
    }

    pub(crate) fn scroll_document_by(&mut self, lines: isize) {
        if lines == 0 {
            return;
        }

        let layout = self.build_document_layout(crate::frame_time::FrameRenderContext::capture());
        if layout.line_count() == 0 {
            self.apply_document_viewport_position(&layout, 0, 0, true, false);
            self.clear_manual_document_scroll_restore_target();
            return;
        }

        let current_offset = self
            .clamp_document_viewport_offset(self.document_runtime.viewport_y, layout.line_count());
        let next_offset =
            self.clamp_document_viewport_offset_signed(current_offset, lines, layout.line_count());
        if next_offset == current_offset {
            return;
        }

        self.start_manual_document_scroll_if_needed();
        let (restore_offset, restore_composer_offset, restore_follow_bottom) =
            self.manual_document_scroll_restore_offsets(&layout);

        if crossed_manual_document_scroll_restore_target(
            current_offset,
            next_offset,
            restore_offset,
        ) {
            self.apply_document_viewport_position(
                &layout,
                restore_offset,
                restore_composer_offset,
                restore_follow_bottom,
                false,
            );
            self.clear_manual_document_scroll_restore_target();
            return;
        }

        let composer_offset = self.current_composer_viewport_offset(&layout, next_offset);
        self.apply_document_viewport_position(&layout, next_offset, composer_offset, false, true);
    }

    pub(crate) fn sync_document_viewport_to_bottom(&mut self) {
        let layout = self.build_document_layout(crate::frame_time::FrameRenderContext::capture());
        let (document_offset, composer_offset) = self.bottom_follow_viewport_offsets(&layout);
        self.apply_document_viewport_position(
            &layout,
            document_offset,
            composer_offset,
            true,
            false,
        );
        self.clear_manual_document_scroll_restore_target();
    }

    pub(crate) fn sync_document_viewport_for_composer_cursor(&mut self) {
        let layout = self.build_document_layout(crate::frame_time::FrameRenderContext::capture());
        if self.document_runtime.follow_bottom {
            self.sync_document_viewport_to_bottom();
            return;
        }

        let mut current_offset = self
            .clamp_document_viewport_offset(self.document_runtime.viewport_y, layout.line_count());
        let viewport_height = self.document_viewport_height();
        if viewport_height == 0 {
            self.apply_document_viewport_position(&layout, 0, 0, false, false);
            return;
        }

        match layout.cursor_y.cmp(&current_offset) {
            Ordering::Less => current_offset = layout.cursor_y,
            Ordering::Greater if layout.cursor_y >= current_offset + viewport_height => {
                current_offset = layout.cursor_y - viewport_height + 1;
            }
            _ => {}
        }

        let document_offset =
            self.clamp_document_viewport_offset(current_offset, layout.line_count());
        let composer_offset = self.current_composer_viewport_offset(&layout, document_offset);
        self.apply_document_viewport_position(
            &layout,
            document_offset,
            composer_offset,
            false,
            false,
        );
        self.clear_manual_document_scroll_restore_target();
    }

    pub(crate) fn sync_document_viewport_preserving_position(&mut self) {
        let layout = self.build_document_layout(crate::frame_time::FrameRenderContext::capture());
        if layout.line_count() == 0 {
            self.apply_document_viewport_position(
                &layout,
                0,
                0,
                false,
                self.document_runtime.manual_scroll,
            );
            return;
        }

        let document_offset = self
            .clamp_document_viewport_offset(self.document_runtime.viewport_y, layout.line_count());
        let composer_offset = self.current_composer_viewport_offset(&layout, document_offset);
        self.apply_document_viewport_position(
            &layout,
            document_offset,
            composer_offset,
            self.document_runtime.follow_bottom,
            self.document_runtime.manual_scroll,
        );
    }

    pub(crate) fn sync_document_viewport_for_viewport_state(&mut self, state: &ViewportState) {
        let layout = self.build_document_layout(crate::frame_time::FrameRenderContext::capture());
        if layout.line_count() == 0 {
            self.apply_document_viewport_position(
                &layout,
                0,
                0,
                state.follow_bottom(),
                state.manual_scroll(),
            );
            return;
        }

        if state.follow_bottom() && !state.manual_scroll() {
            let (document_offset, composer_offset) = self.bottom_follow_viewport_offsets(&layout);
            self.apply_document_viewport_position(
                &layout,
                document_offset,
                composer_offset,
                true,
                false,
            );
            return;
        }

        let document_offset = state.resolve_offset(&layout, self.document_viewport_height());
        let composer_offset = self.current_composer_viewport_offset(&layout, document_offset);
        self.apply_document_viewport_position(
            &layout,
            document_offset,
            composer_offset,
            state.follow_bottom(),
            state.manual_scroll(),
        );
    }

    pub(crate) fn sync_document_viewport_for_composer_page(&mut self) {
        let layout = self.build_document_layout(crate::frame_time::FrameRenderContext::capture());
        let max_offset = layout
            .composer_line_count
            .saturating_sub(self.composer.viewport_height().max(1));
        if self.composer.viewport_offset() > max_offset {
            self.composer.set_viewport_offset(max_offset);
        }

        if layout.composer_line_count <= self.composer.viewport_height().max(1) {
            self.sync_document_viewport_for_composer_cursor();
            return;
        }

        let document_offset = self.clamp_document_viewport_offset(
            layout.composer_start_line + self.composer.viewport_offset(),
            layout.line_count(),
        );
        self.apply_document_viewport_position(
            &layout,
            document_offset,
            self.composer.viewport_offset(),
            false,
            false,
        );
        self.clear_manual_document_scroll_restore_target();
    }

    pub(crate) fn sync_document_viewport_after_composer_interaction(
        &mut self,
        old_value: &str,
        old_line: usize,
        old_column: usize,
    ) {
        if self.composer.value() != old_value {
            if self.selection_runtime.selection.is_active() {
                self.invalidate_selection_for_reflow();
            }
            if self.document_runtime.manual_scroll {
                self.restore_from_manual_document_scroll();
                return;
            }

            if self.document_runtime.follow_bottom {
                self.sync_document_viewport_to_bottom();
                return;
            }

            self.sync_document_viewport_for_composer_cursor();
            return;
        }

        if self.composer.line() != old_line || self.composer.column() != old_column {
            self.document_runtime.follow_bottom = self.composer_at_bottom_follow_anchor();
            if self.document_runtime.follow_bottom {
                self.sync_document_viewport_to_bottom();
                return;
            }

            self.sync_document_viewport_for_composer_cursor();
            return;
        }

        if self.document_runtime.follow_bottom {
            self.sync_document_viewport_to_bottom();
            return;
        }

        if self.document_runtime.manual_scroll {
            self.sync_document_viewport_preserving_position();
            return;
        }

        self.sync_document_viewport_for_composer_cursor();
    }

    pub(crate) fn sync_document_viewport_after_transcript_refresh(
        &mut self,
        preserved_viewport_state: Option<ViewportState>,
    ) {
        if self.document_runtime.follow_bottom {
            self.sync_document_viewport_to_bottom();
            return;
        }

        if let Some(state) = preserved_viewport_state.as_ref() {
            self.sync_document_viewport_for_viewport_state(state);
            if self.document_runtime.manual_scroll {
                self.complete_manual_document_scroll_if_restored();
            }
            return;
        }

        if self.document_runtime.manual_scroll {
            self.sync_document_viewport_preserving_position();
            self.complete_manual_document_scroll_if_restored();
            return;
        }

        self.sync_document_viewport_for_composer_cursor();
    }

    pub(crate) fn composer_at_bottom_follow_anchor(&self) -> bool {
        if self.composer.value().is_empty() {
            return true;
        }

        let lines = self.composer.value().split('\n').collect::<Vec<_>>();
        let Some(last_line) = lines.last() else {
            return true;
        };

        self.composer.line() == lines.len().saturating_sub(1)
            && self.composer.column() == last_line.chars().count()
    }

    pub(crate) fn bottom_follow_viewport_offsets(&self, layout: &DocumentLayout) -> (usize, usize) {
        if self.composer.value().is_empty() {
            let viewport_height = self.document_viewport_height();
            if viewport_height == 0 {
                return (0, 0);
            }

            let document_offset = self.clamp_document_viewport_offset(
                layout.cursor_y.saturating_sub(viewport_height - 1),
                layout.line_count(),
            );
            return (document_offset, 0);
        }

        (
            self.document_bottom_offset(layout.line_count()),
            self.composer.bottom_viewport_offset(),
        )
    }

    pub(crate) fn capture_viewport_state_with_layout(
        &self,
        layout: &DocumentLayout,
        document_offset: usize,
        follow_bottom: bool,
        manual_scroll: bool,
    ) -> ViewportState {
        let resolved_offset =
            self.clamp_document_viewport_offset(document_offset, layout.line_count());
        let line_indices = self.document_viewport_line_indices_for_mode(
            layout,
            resolved_offset,
            follow_bottom,
            manual_scroll,
        );
        ViewportState::capture(
            layout,
            &line_indices,
            resolved_offset,
            follow_bottom,
            manual_scroll,
            self.document_viewport_height(),
            self.width,
        )
    }

    pub(crate) fn apply_document_viewport_position(
        &mut self,
        layout: &DocumentLayout,
        document_offset: usize,
        composer_offset: usize,
        follow_bottom: bool,
        manual_scroll: bool,
    ) {
        let document_offset =
            self.clamp_document_viewport_offset(document_offset, layout.line_count());
        self.document_runtime.viewport_y = document_offset;
        self.composer.set_viewport_offset(composer_offset);
        self.document_runtime.follow_bottom = follow_bottom;
        self.document_runtime.manual_scroll = manual_scroll;
        self.document_runtime.viewport_state = self.capture_viewport_state_with_layout(
            layout,
            document_offset,
            follow_bottom,
            manual_scroll,
        );
    }

    pub(crate) fn document_viewport_line_indices_for_mode(
        &self,
        layout: &DocumentLayout,
        document_offset: usize,
        follow_bottom: bool,
        manual_scroll: bool,
    ) -> Vec<usize> {
        if follow_bottom && !manual_scroll {
            return bottom_follow_viewport_line_indices(
                layout,
                self.document_viewport_height(),
                self.bottom_follow_presentation(layout),
            );
        }

        offset_viewport_line_indices(layout, document_offset, self.document_viewport_height())
    }
}
