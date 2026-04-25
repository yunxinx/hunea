use std::cmp::Ordering;

use crate::frontend::tui::Model;

use super::{DocumentLayout, ManualDocumentScrollRestoreTarget, ViewportState};

/// `ManualScrollRestoreState` 收口手动滚动返回目标及其锚点。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct ManualScrollRestoreState {
    target: ManualDocumentScrollRestoreTarget,
    viewport_state: ViewportState,
}

impl ManualScrollRestoreState {
    pub(crate) fn clear(&mut self) {
        self.target = ManualDocumentScrollRestoreTarget::None;
        self.viewport_state = ViewportState::default();
    }

    pub(crate) fn is_pending(&self) -> bool {
        self.target != ManualDocumentScrollRestoreTarget::None
    }

    pub(crate) fn track_bottom_follow(&mut self) {
        self.target = ManualDocumentScrollRestoreTarget::BottomFollow;
        self.viewport_state = ViewportState::default();
    }

    pub(crate) fn track_composer_cursor(&mut self, viewport_state: Option<ViewportState>) {
        self.target = ManualDocumentScrollRestoreTarget::ComposerCursor;
        self.viewport_state = viewport_state.unwrap_or_default();
    }

    pub(crate) const fn target(&self) -> ManualDocumentScrollRestoreTarget {
        self.target
    }

    pub(crate) fn viewport_state(&self) -> &ViewportState {
        &self.viewport_state
    }
}

impl Model {
    pub(crate) fn clear_manual_document_scroll_restore_target(&mut self) {
        self.document_runtime.restore.clear();
    }

    pub(crate) fn start_manual_document_scroll_if_needed(&mut self) {
        if self.document_runtime.manual_scroll {
            return;
        }

        if self.document_runtime.follow_bottom {
            self.document_runtime.restore.track_bottom_follow();
            return;
        }

        let viewport_state = self.current_document_viewport_state();
        self.document_runtime
            .restore
            .track_composer_cursor(Some(viewport_state));
    }

    pub(crate) fn manual_document_scroll_restore_offsets(
        &self,
        layout: &DocumentLayout,
    ) -> (usize, usize, bool) {
        let offsets = self.restore_offsets(layout);
        (
            offsets.document_offset,
            offsets.composer_offset,
            offsets.follow_bottom,
        )
    }

    pub(crate) fn complete_manual_document_scroll_if_restored(&mut self) {
        if !self.document_runtime.manual_scroll || !self.document_runtime.restore.is_pending() {
            return;
        }

        let layout = self.build_document_layout();
        let offsets = self.restore_offsets(&layout);
        if self.document_runtime.viewport_y != offsets.document_offset
            || self.composer.viewport_offset() != offsets.composer_offset
        {
            return;
        }

        self.apply_document_viewport_position(
            &layout,
            offsets.document_offset,
            offsets.composer_offset,
            offsets.follow_bottom,
            false,
        );
        self.clear_manual_document_scroll_restore_target();
    }

    pub(super) fn restore_from_manual_document_scroll(&mut self) {
        let layout = self.build_document_layout();
        let offsets = self.edit_restore_offsets(&layout);
        self.apply_document_viewport_position(
            &layout,
            offsets.document_offset,
            offsets.composer_offset,
            offsets.follow_bottom,
            false,
        );
        self.clear_manual_document_scroll_restore_target();
    }

    pub(super) fn clamp_document_viewport_offset_signed(
        &self,
        offset: usize,
        delta: isize,
        total_lines: usize,
    ) -> usize {
        let next = if delta.is_negative() {
            offset.saturating_sub(delta.unsigned_abs())
        } else {
            offset.saturating_add(delta as usize)
        };

        self.clamp_document_viewport_offset(next, total_lines)
    }

    fn composer_cursor_restore_viewport_offsets(&self, layout: &DocumentLayout) -> (usize, usize) {
        let viewport_height = self.document_viewport_height();
        if viewport_height == 0 {
            return (0, 0);
        }

        let document_offset = self.clamp_document_viewport_offset(
            layout.cursor_y.saturating_sub(viewport_height - 1),
            layout.line_count(),
        );
        let composer_offset = self.current_composer_viewport_offset(layout, document_offset);
        (document_offset, composer_offset)
    }

    fn document_offset_keeps_cursor_visible(
        &self,
        layout: &DocumentLayout,
        document_offset: usize,
    ) -> bool {
        let viewport_height = self.document_viewport_height();
        if viewport_height == 0 {
            return true;
        }

        let document_offset =
            self.clamp_document_viewport_offset(document_offset, layout.line_count());
        layout.cursor_y >= document_offset && layout.cursor_y < document_offset + viewport_height
    }

    fn restore_offsets(&self, layout: &DocumentLayout) -> ManualScrollRestoreOffsets {
        match self.document_runtime.restore.target() {
            ManualDocumentScrollRestoreTarget::BottomFollow => {
                let (document_offset, composer_offset) =
                    self.bottom_follow_viewport_offsets(layout);
                ManualScrollRestoreOffsets::new(document_offset, composer_offset, true)
            }
            _ => {
                let document_offset = self
                    .document_runtime
                    .restore
                    .viewport_state()
                    .resolve_offset(layout, self.document_viewport_height());
                if self.document_offset_keeps_cursor_visible(layout, document_offset) {
                    let composer_offset =
                        self.current_composer_viewport_offset(layout, document_offset);
                    return ManualScrollRestoreOffsets::new(
                        document_offset,
                        composer_offset,
                        false,
                    );
                }

                let (document_offset, composer_offset) =
                    self.composer_cursor_restore_viewport_offsets(layout);
                ManualScrollRestoreOffsets::new(document_offset, composer_offset, false)
            }
        }
    }

    fn edit_restore_offsets(&self, layout: &DocumentLayout) -> ManualScrollRestoreOffsets {
        match self.document_runtime.restore.target() {
            ManualDocumentScrollRestoreTarget::BottomFollow => {
                let (document_offset, composer_offset) =
                    self.bottom_follow_viewport_offsets(layout);
                ManualScrollRestoreOffsets::new(document_offset, composer_offset, true)
            }
            _ => {
                let (document_offset, composer_offset) =
                    self.composer_cursor_restore_viewport_offsets(layout);
                ManualScrollRestoreOffsets::new(document_offset, composer_offset, false)
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ManualScrollRestoreOffsets {
    document_offset: usize,
    composer_offset: usize,
    follow_bottom: bool,
}

impl ManualScrollRestoreOffsets {
    const fn new(document_offset: usize, composer_offset: usize, follow_bottom: bool) -> Self {
        Self {
            document_offset,
            composer_offset,
            follow_bottom,
        }
    }
}

pub(super) fn crossed_manual_document_scroll_restore_target(
    current_offset: usize,
    next_offset: usize,
    restore_offset: usize,
) -> bool {
    match current_offset.cmp(&restore_offset) {
        Ordering::Less => next_offset >= restore_offset,
        Ordering::Greater => next_offset <= restore_offset,
        Ordering::Equal => false,
    }
}
