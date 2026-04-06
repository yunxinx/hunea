use std::rc::Rc;

use ratatui::text::Line;

use crate::frontend::tui::{Model, selection::apply_selection_to_viewport};

use super::{
    DocumentLayout, DocumentLayoutCache, DocumentLayoutKey, DocumentTranscriptCache,
    DocumentTranscriptKey, DocumentTranscriptSnapshot, DocumentViewport, DocumentViewportCache,
    DocumentViewportKey, offset_slot_frame, slot_viewport::compose_bottom_follow_document_viewport,
};

/// `DocumentLayoutInput` 表示统一文档布局真正依赖的 transcript 与 tail 快照。
#[derive(Debug, Clone)]
pub(crate) struct DocumentLayoutInput {
    pub(crate) transcript: Rc<DocumentTranscriptSnapshot>,
    pub(crate) tail: Rc<super::DocumentTailLayout>,
}

impl Model {
    pub(crate) fn invalidate_document_viewport_cache(&mut self) {
        self.document_viewport_cache.valid = false;
    }

    #[cfg(test)]
    pub(crate) fn invalidate_document_caches_for_test(&mut self) {
        self.document_tail_layout_cache.valid = false;
        self.document_layout_cache.valid = false;
        self.document_viewport_cache.valid = false;
    }

    pub(crate) fn build_document_layout(&mut self) -> Rc<DocumentLayout> {
        let key = self.current_document_layout_key();
        if self.document_layout_cache.valid && self.document_layout_cache.key == key {
            return Rc::clone(&self.document_layout_cache.layout);
        }

        let layout = Rc::new(compose_document_layout(
            self.current_document_layout_input(),
        ));
        self.document_transcript_cache = DocumentTranscriptCache {
            key: DocumentTranscriptKey {
                transcript_render_version: self.transcript_render_version,
                document_width: self.width,
            },
            snapshot: Rc::clone(&layout.transcript),
            valid: true,
        };
        self.document_layout_cache = DocumentLayoutCache {
            key,
            layout: Rc::clone(&layout),
            valid: true,
        };
        layout
    }

    pub(crate) fn build_document_viewport(
        &mut self,
        layout: &DocumentLayout,
    ) -> Rc<DocumentViewport> {
        let uses_bottom_follow = self.document_viewport_state.follow_bottom()
            && !self.document_viewport_state.manual_scroll();
        let key = DocumentViewportKey {
            layout_key: self.current_document_layout_key(),
            offset: self.document_viewport_state.resolved_offset(),
            height: self.document_viewport_height(),
            bottom_follow: uses_bottom_follow,
            selection_version: self.selection_version,
        };
        if self.document_viewport_cache.valid && self.document_viewport_cache.key == key {
            return Rc::clone(&self.document_viewport_cache.viewport);
        }

        let mut viewport = compose_document_viewport(
            layout,
            self.document_viewport_state.resolved_offset(),
            self.document_viewport_height(),
        );
        if uses_bottom_follow {
            viewport = compose_bottom_follow_document_viewport(
                layout,
                self.document_viewport_height(),
                self.bottom_follow_presentation(layout),
            );
        }
        apply_selection_to_viewport(&mut viewport, layout, self.selection);

        let viewport = Rc::new(viewport);
        self.document_viewport_cache = DocumentViewportCache {
            key,
            viewport: Rc::clone(&viewport),
            valid: true,
        };
        viewport
    }

    pub(crate) fn document_viewport_height(&self) -> usize {
        if !self.has_window || self.height == 0 {
            return 0;
        }

        usize::from(self.height.max(1))
    }

    pub(crate) fn clamp_document_viewport_offset(
        &self,
        offset: usize,
        total_lines: usize,
    ) -> usize {
        let viewport_height = self.document_viewport_height();
        if viewport_height == 0 || total_lines <= viewport_height {
            return 0;
        }

        offset.min(total_lines - viewport_height)
    }

    pub(crate) fn current_document_layout_key(&self) -> DocumentLayoutKey {
        DocumentLayoutKey {
            transcript_render_version: self.transcript_render_version,
            palette_version: self.palette_version,
            style_mode: self.style_mode,
            document_width: self.width,
            document_viewport_height: self.document_viewport_height(),
            composer_viewport_height: self.composer.viewport_height(),
            composer_content_revision: self.composer.content_revision(),
            composer_cursor_revision: self.composer.cursor_revision(),
            composer_width: self.composer.content_width(),
            command_panel_selected: self.command_panel_selected,
            command_panel_scroll: self.command_panel_scroll,
            status_line_config: self.status_line_config_bits(),
            status_line_revision: self.status_line_revision(),
        }
    }

    pub(crate) fn current_document_layout_input(&mut self) -> DocumentLayoutInput {
        DocumentLayoutInput {
            transcript: self.current_document_transcript_snapshot(),
            tail: self.build_document_tail_layout(),
        }
    }

    pub(crate) fn document_bottom_offset(&self, total_lines: usize) -> usize {
        self.clamp_document_viewport_offset(total_lines, total_lines)
    }

    pub(crate) fn current_composer_viewport_offset(
        &self,
        layout: &DocumentLayout,
        document_viewport_y: usize,
    ) -> usize {
        let viewport_height = self.composer.viewport_height().max(1);
        if layout.composer_slot.content_line_count <= viewport_height {
            return 0;
        }

        let offset = document_viewport_y.saturating_sub(layout.composer_slot.content_start_line);
        offset.min(layout.composer_slot.content_line_count - viewport_height)
    }
}

pub(crate) fn compose_document_layout(input: DocumentLayoutInput) -> DocumentLayout {
    let transcript_line_count = input.transcript.line_count();
    let transcript = Rc::clone(&input.transcript);
    let composer_slot = offset_slot_frame(input.tail.composer_slot, transcript_line_count);

    DocumentLayout {
        transcript,
        transcript_line_count,
        tail: Rc::clone(&input.tail),
        composer_slot,
        composer_start_line: composer_slot.content_start_line,
        composer_line_count: composer_slot.content_line_count,
        cursor_x: input.tail.cursor_x,
        cursor_y: transcript_line_count + input.tail.cursor_y,
    }
}

pub(crate) fn compose_document_viewport(
    layout: &DocumentLayout,
    offset: usize,
    height: usize,
) -> DocumentViewport {
    if layout.line_count() == 0 {
        return DocumentViewport {
            lines: vec![Line::raw("")],
            plain_text_len: 0,
            #[cfg(test)]
            plain_lines: vec![String::new()],
            resolved_offset: 0,
        };
    }

    if height == 0 || height >= layout.line_count() {
        return DocumentViewport {
            lines: layout.lines_for_range(0, layout.line_count()),
            plain_text_len: layout.plain_text_len(),
            #[cfg(test)]
            plain_lines: layout.all_plain_lines(),
            resolved_offset: 0,
        };
    }

    let max_offset = layout.line_count().saturating_sub(height);
    let resolved_offset = offset.min(max_offset);
    let visible_line_count = height.min(layout.line_count() - resolved_offset);
    let range = document_range_snapshot(layout, resolved_offset, visible_line_count);

    DocumentViewport {
        lines: range.lines,
        plain_text_len: range.plain_text_len,
        #[cfg(test)]
        plain_lines: range.plain_lines,
        resolved_offset,
    }
}

#[cfg(test)]
pub(crate) fn visible_document_lines(
    layout: &DocumentLayout,
    offset: usize,
    height: usize,
) -> (Vec<Line<'static>>, Vec<String>, usize) {
    if layout.line_count() == 0 {
        return (vec![Line::raw("")], vec![String::new()], 0);
    }

    if height == 0 || height >= layout.line_count() {
        return (
            layout.lines_for_range(0, layout.line_count()),
            layout.all_plain_lines(),
            0,
        );
    }

    let max_offset = layout.line_count().saturating_sub(height);
    let resolved_offset = offset.min(max_offset);
    let range = document_range_snapshot(layout, resolved_offset, height);
    (range.lines, range.plain_lines, resolved_offset)
}

#[derive(Debug, Clone, Default)]
struct DocumentRangeSnapshot {
    lines: Vec<Line<'static>>,
    plain_text_len: usize,
    #[cfg(test)]
    plain_lines: Vec<String>,
}

fn document_range_snapshot(
    layout: &DocumentLayout,
    mut start: usize,
    count: usize,
) -> DocumentRangeSnapshot {
    if count == 0 || layout.line_count() == 0 || start >= layout.line_count() {
        return DocumentRangeSnapshot::default();
    }

    let end = (start + count).min(layout.line_count());
    let mut lines = Vec::with_capacity(end - start);
    let mut plain_text_len = 0;
    let mut line_count = 0;
    #[cfg(test)]
    let mut plain_lines = Vec::with_capacity(end - start);

    if start < layout.transcript_line_count {
        let transcript_end = end.min(layout.transcript_line_count);
        let transcript_slice = layout
            .transcript
            .viewport_snapshot(start, transcript_end - start);
        plain_text_len += transcript_slice.plain_text_len;
        line_count += transcript_slice.lines.len();
        lines.extend(transcript_slice.lines);
        #[cfg(test)]
        plain_lines.extend(transcript_slice.plain_lines);
        start = transcript_end;
    }

    if start < end {
        let tail_start = start - layout.transcript_line_count;
        let tail_end = end - layout.transcript_line_count;
        let tail_line_count = tail_end - tail_start;
        lines.extend_from_slice(&layout.tail.lines[tail_start..tail_end]);
        if line_count > 0 && tail_line_count > 0 {
            plain_text_len += 1;
        }
        plain_text_len += if tail_line_count == 0 {
            0
        } else {
            layout.tail.text_lines[tail_start..tail_end]
                .iter()
                .map(String::len)
                .sum::<usize>()
                + tail_line_count.saturating_sub(1)
        };
        #[cfg(test)]
        plain_lines.extend_from_slice(&layout.tail.text_lines[tail_start..tail_end]);
    }

    DocumentRangeSnapshot {
        lines,
        plain_text_len,
        #[cfg(test)]
        plain_lines,
    }
}

impl Model {
    pub(crate) fn current_document_transcript_snapshot(
        &mut self,
    ) -> Rc<DocumentTranscriptSnapshot> {
        let key = DocumentTranscriptKey {
            transcript_render_version: self.transcript_render_version,
            document_width: self.width,
        };
        if self.document_transcript_cache.valid && self.document_transcript_cache.key == key {
            return Rc::clone(&self.document_transcript_cache.snapshot);
        }

        let index = self.transcript.item_metrics_index();
        let warmed_item_block_cache = self.transcript.cached_screen_blocks_snapshot();
        let snapshot = Rc::new(DocumentTranscriptSnapshot {
            index,
            width: if self.width == 0 {
                crate::frontend::tui::transcript::DEFAULT_RENDER_WIDTH as u16
            } else {
                self.width
            },
            palette: self.palette,
            items: self.transcript.items_snapshot(),
            warmed_item_block_cache,
            item_block_cache: Rc::new(std::cell::RefCell::new(std::collections::HashMap::new())),
            item_text_lines_cache: Rc::new(std::cell::RefCell::new(
                std::collections::HashMap::new(),
            )),
            selectable_cache: Rc::new(std::cell::RefCell::new(std::collections::HashMap::new())),
        });
        self.document_transcript_cache = DocumentTranscriptCache {
            key,
            snapshot: Rc::clone(&snapshot),
            valid: true,
        };
        snapshot
    }
}
