use std::cmp::Ordering;

use crate::frontend::tui::{Model, transcript::LineAnchorKind};

use super::{
    DocumentLayout, DocumentViewportAnchor, ManualDocumentScrollRestoreTarget,
    anchor_match::{
        canonical_rendered_transcript_anchor_text, find_document_offset_for_viewport_anchor,
        transcript_content_line_count_for_item,
    },
};

const DOCUMENT_MOUSE_WHEEL_DELTA: isize = 3;

impl Model {
    pub(crate) fn document_mouse_wheel_delta() -> isize {
        DOCUMENT_MOUSE_WHEEL_DELTA
    }

    pub(crate) fn current_document_viewport_anchor(&mut self) -> Option<DocumentViewportAnchor> {
        let layout = self.build_document_layout();
        if layout.line_count() == 0 {
            return None;
        }

        let offset =
            self.clamp_document_viewport_offset(self.document_viewport_y, layout.line_count());
        let line = layout.line_at(offset)?;
        let line_anchor = line.anchor;
        let mut line_text = line.plain_line;
        if matches!(line_anchor.region, super::DocumentAnchorRegion::Transcript)
            && matches!(
                line_anchor.transcript.item_anchor.kind,
                LineAnchorKind::RenderedLine
            )
        {
            line_text = canonical_rendered_transcript_anchor_text(&line_text);
        }

        let transcript_item_line_count =
            if matches!(line_anchor.region, super::DocumentAnchorRegion::Transcript)
                && matches!(
                    line_anchor.transcript.item_anchor.kind,
                    LineAnchorKind::RenderedLine
                )
            {
                transcript_content_line_count_for_item(&layout, line_anchor.transcript.item_index)
            } else {
                0
            };

        Some(DocumentViewportAnchor {
            line_anchor,
            line_text,
            transcript_item_line_count,
        })
    }

    pub(crate) fn scroll_document_by(&mut self, lines: isize) {
        if lines == 0 {
            return;
        }

        let layout = self.build_document_layout();
        if layout.line_count() == 0 {
            self.document_viewport_y = 0;
            self.composer.set_viewport_offset(0);
            self.follow_bottom = true;
            self.manual_document_scroll = false;
            self.clear_manual_document_scroll_restore_target();
            return;
        }

        let current_offset =
            self.clamp_document_viewport_offset(self.document_viewport_y, layout.line_count());
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
            self.document_viewport_y = restore_offset;
            self.composer.set_viewport_offset(restore_composer_offset);
            self.follow_bottom = restore_follow_bottom;
            self.manual_document_scroll = false;
            self.clear_manual_document_scroll_restore_target();
            return;
        }

        self.document_viewport_y = next_offset;
        self.composer
            .set_viewport_offset(self.current_composer_viewport_offset(&layout, next_offset));
        self.follow_bottom = false;
        self.manual_document_scroll = true;
    }

    pub(crate) fn sync_document_viewport_to_bottom(&mut self) {
        let layout = self.build_document_layout();
        let (document_offset, composer_offset) = self.bottom_follow_viewport_offsets(&layout);
        self.document_viewport_y = document_offset;
        self.composer.set_viewport_offset(composer_offset);
        self.manual_document_scroll = false;
        self.clear_manual_document_scroll_restore_target();
    }

    pub(crate) fn sync_document_viewport_for_composer_cursor(&mut self) {
        let layout = self.build_document_layout();
        if self.follow_bottom {
            self.sync_document_viewport_to_bottom();
            return;
        }

        let mut current_offset =
            self.clamp_document_viewport_offset(self.document_viewport_y, layout.line_count());
        let viewport_height = self.document_viewport_height();
        if viewport_height == 0 {
            self.document_viewport_y = 0;
            self.composer.set_viewport_offset(0);
            return;
        }

        match layout.cursor_y.cmp(&current_offset) {
            Ordering::Less => current_offset = layout.cursor_y,
            Ordering::Greater if layout.cursor_y >= current_offset + viewport_height => {
                current_offset = layout.cursor_y - viewport_height + 1;
            }
            _ => {}
        }

        self.document_viewport_y =
            self.clamp_document_viewport_offset(current_offset, layout.line_count());
        self.composer.set_viewport_offset(
            self.current_composer_viewport_offset(&layout, self.document_viewport_y),
        );
        self.manual_document_scroll = false;
        self.clear_manual_document_scroll_restore_target();
    }

    pub(crate) fn sync_document_viewport_preserving_position(&mut self) {
        let layout = self.build_document_layout();
        if layout.line_count() == 0 {
            self.document_viewport_y = 0;
            self.composer.set_viewport_offset(0);
            return;
        }

        self.document_viewport_y =
            self.clamp_document_viewport_offset(self.document_viewport_y, layout.line_count());
        self.composer.set_viewport_offset(
            self.current_composer_viewport_offset(&layout, self.document_viewport_y),
        );
    }

    pub(crate) fn sync_document_viewport_for_viewport_anchor(
        &mut self,
        anchor: &DocumentViewportAnchor,
    ) {
        let layout = self.build_document_layout();
        if layout.line_count() == 0 {
            self.document_viewport_y = 0;
            self.composer.set_viewport_offset(0);
            return;
        }

        let Some(offset) = find_document_offset_for_viewport_anchor(&layout, anchor) else {
            self.sync_document_viewport_preserving_position();
            return;
        };

        self.document_viewport_y = self.clamp_document_viewport_offset(offset, layout.line_count());
        self.composer.set_viewport_offset(
            self.current_composer_viewport_offset(&layout, self.document_viewport_y),
        );
    }

    pub(crate) fn sync_document_viewport_for_composer_page(&mut self) {
        let layout = self.build_document_layout();
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

        self.document_viewport_y = self.clamp_document_viewport_offset(
            layout.composer_start_line + self.composer.viewport_offset(),
            layout.line_count(),
        );
        self.manual_document_scroll = false;
        self.clear_manual_document_scroll_restore_target();
    }

    pub(crate) fn sync_document_viewport_after_composer_interaction(
        &mut self,
        old_value: &str,
        old_line: usize,
        old_column: usize,
    ) {
        if self.composer.value() != old_value {
            if self.selection.active {
                self.invalidate_selection_for_reflow();
            }
            if self.manual_document_scroll {
                self.restore_from_manual_document_scroll();
                return;
            }

            if self.follow_bottom {
                self.sync_document_viewport_to_bottom();
                return;
            }

            self.sync_document_viewport_for_composer_cursor();
            return;
        }

        if self.composer.line() != old_line || self.composer.column() != old_column {
            self.follow_bottom = self.composer_at_bottom_follow_anchor();
            if self.follow_bottom {
                self.sync_document_viewport_to_bottom();
                return;
            }

            self.sync_document_viewport_for_composer_cursor();
            return;
        }

        if self.follow_bottom {
            self.sync_document_viewport_to_bottom();
            return;
        }

        if self.manual_document_scroll {
            self.sync_document_viewport_preserving_position();
            return;
        }

        self.sync_document_viewport_for_composer_cursor();
    }

    pub(crate) fn sync_document_viewport_after_transcript_refresh(
        &mut self,
        preserved_anchor: Option<DocumentViewportAnchor>,
    ) {
        if self.follow_bottom {
            self.sync_document_viewport_to_bottom();
            return;
        }

        if self.manual_document_scroll {
            if let Some(anchor) = preserved_anchor.as_ref() {
                self.sync_document_viewport_for_viewport_anchor(anchor);
            } else {
                self.sync_document_viewport_preserving_position();
            }
            self.complete_manual_document_scroll_if_restored();
            return;
        }

        self.sync_document_viewport_for_composer_cursor();
    }

    pub(crate) fn clear_manual_document_scroll_restore_target(&mut self) {
        self.scroll_restore_target = ManualDocumentScrollRestoreTarget::None;
        self.scroll_restore_anchor = DocumentViewportAnchor::default();
    }

    pub(crate) fn start_manual_document_scroll_if_needed(&mut self) {
        if self.manual_document_scroll {
            return;
        }

        if self.follow_bottom {
            self.scroll_restore_target = ManualDocumentScrollRestoreTarget::BottomFollow;
            return;
        }

        if let Some(anchor) = self.current_document_viewport_anchor() {
            self.scroll_restore_target = ManualDocumentScrollRestoreTarget::ComposerCursor;
            self.scroll_restore_anchor = anchor;
            return;
        }

        self.scroll_restore_target = ManualDocumentScrollRestoreTarget::ComposerCursor;
    }

    pub(crate) fn manual_document_scroll_restore_offsets(
        &self,
        layout: &DocumentLayout,
    ) -> (usize, usize, bool) {
        match self.scroll_restore_target {
            ManualDocumentScrollRestoreTarget::BottomFollow => {
                let (document_offset, composer_offset) =
                    self.bottom_follow_viewport_offsets(layout);
                (document_offset, composer_offset, true)
            }
            _ => {
                if let Some(offset) =
                    find_document_offset_for_viewport_anchor(layout, &self.scroll_restore_anchor)
                {
                    let document_offset =
                        self.clamp_document_viewport_offset(offset, layout.line_count());
                    if self.document_offset_keeps_cursor_visible(layout, document_offset) {
                        let composer_offset =
                            self.current_composer_viewport_offset(layout, document_offset);
                        return (document_offset, composer_offset, false);
                    }
                }

                let (document_offset, composer_offset) =
                    self.composer_cursor_restore_viewport_offsets(layout);
                (document_offset, composer_offset, false)
            }
        }
    }

    pub(crate) fn complete_manual_document_scroll_if_restored(&mut self) {
        if !self.manual_document_scroll
            || self.scroll_restore_target == ManualDocumentScrollRestoreTarget::None
        {
            return;
        }

        let layout = self.build_document_layout();
        let (restore_offset, restore_composer_offset, restore_follow_bottom) =
            self.manual_document_scroll_restore_offsets(&layout);
        if self.document_viewport_y != restore_offset
            || self.composer.viewport_offset() != restore_composer_offset
        {
            return;
        }

        self.follow_bottom = restore_follow_bottom;
        self.manual_document_scroll = false;
        self.clear_manual_document_scroll_restore_target();
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

    fn restore_from_manual_document_scroll(&mut self) {
        let layout = self.build_document_layout();
        let (document_offset, composer_offset, follow_bottom) =
            self.manual_document_scroll_edit_restore_offsets(&layout);
        self.document_viewport_y = document_offset;
        self.composer.set_viewport_offset(composer_offset);
        self.follow_bottom = follow_bottom;
        self.manual_document_scroll = false;
        self.clear_manual_document_scroll_restore_target();
    }

    fn clamp_document_viewport_offset_signed(
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

    fn manual_document_scroll_edit_restore_offsets(
        &self,
        layout: &DocumentLayout,
    ) -> (usize, usize, bool) {
        match self.scroll_restore_target {
            ManualDocumentScrollRestoreTarget::BottomFollow => {
                let (document_offset, composer_offset) =
                    self.bottom_follow_viewport_offsets(layout);
                (document_offset, composer_offset, true)
            }
            _ => {
                let (document_offset, composer_offset) =
                    self.composer_cursor_restore_viewport_offsets(layout);
                (document_offset, composer_offset, false)
            }
        }
    }
}

fn crossed_manual_document_scroll_restore_target(
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
