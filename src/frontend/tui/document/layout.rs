use ratatui::text::Line;

use crate::frontend::tui::{
    Model, composer,
    selection::{
        SelectableLineRange, apply_selection_to_viewport, selectable_range_for_plain_line,
    },
    status_line::StatusLineRenderResult,
    transcript,
};

use super::{
    DocumentAnchorRegion, DocumentLayout, DocumentLayoutCache, DocumentLayoutKey,
    DocumentLineAnchor, DocumentTranscriptCache, DocumentTranscriptKey, DocumentTranscriptSnapshot,
    DocumentViewport, DocumentViewportCache, DocumentViewportKey,
    append::{
        can_extend_cached_document_layout, extend_document_layout_from_transcript_append,
        sliced_transcript_append,
    },
    line_access::new_document_transcript_index,
    slot_frame::SlotFrame,
    slot_viewport::compose_bottom_follow_document_viewport,
};

#[derive(Debug, Clone)]
pub(crate) struct DocumentLayoutInput {
    pub(crate) transcript: DocumentTranscriptSnapshot,
    pub(crate) composer_lines: Vec<Line<'static>>,
    pub(crate) composer_plain_lines: Vec<String>,
    pub(crate) composer_anchors: Vec<DocumentLineAnchor>,
    pub(crate) composer_selectable: Vec<SelectableLineRange>,
    pub(crate) composer_frame_decoration_line: Option<Line<'static>>,
    pub(crate) composer_frame_decoration_plain_line: Option<String>,
    pub(crate) composer_cursor_x: u16,
    pub(crate) composer_cursor_y: usize,
    pub(crate) status_line: StatusLineRenderResult,
}

impl Model {
    pub(crate) fn build_document_layout(&mut self) -> DocumentLayout {
        let key = self.current_document_layout_key();
        if self.document_layout_cache.valid && self.document_layout_cache.key == key {
            return self.document_layout_cache.layout.clone();
        }

        if let Some((layout, transcript_snapshot)) =
            self.build_document_layout_from_transcript_append(&key)
        {
            self.document_transcript_cache = DocumentTranscriptCache {
                key: DocumentTranscriptKey {
                    transcript_render_version: self.transcript_render_version,
                    document_width: self.width,
                },
                snapshot: transcript_snapshot,
                valid: true,
            };
            self.document_layout_cache = DocumentLayoutCache {
                key,
                layout: layout.clone(),
                transcript_line_count: self.transcript_render.line_count,
                valid: true,
            };
            return layout;
        }

        let layout = compose_document_layout(self.current_document_layout_input());
        self.document_transcript_cache = DocumentTranscriptCache {
            key: DocumentTranscriptKey {
                transcript_render_version: self.transcript_render_version,
                document_width: self.width,
            },
            snapshot: layout.transcript.clone(),
            valid: true,
        };

        self.document_layout_cache = DocumentLayoutCache {
            key,
            layout: layout.clone(),
            transcript_line_count: self.transcript_render.line_count,
            valid: true,
        };
        layout
    }

    pub(crate) fn build_document_layout_from_transcript_append(
        &mut self,
        key: &DocumentLayoutKey,
    ) -> Option<(DocumentLayout, DocumentTranscriptSnapshot)> {
        if !self.document_layout_cache.valid {
            return None;
        }
        if !can_extend_cached_document_layout(&self.document_layout_cache.key, key) {
            return None;
        }

        let start_line = usize::try_from(self.transcript_render.append_start_line).ok()?;
        if self.document_layout_cache.transcript_line_count != start_line {
            return None;
        }

        let appended =
            sliced_transcript_append(&self.transcript_render, start_line, &self.transcript)?;
        if appended.lines.is_empty() {
            return None;
        }

        let transcript_snapshot = self.document_transcript_snapshot_after_append(&appended);
        Some((
            extend_document_layout_from_transcript_append(
                &self.document_layout_cache.layout,
                appended,
                transcript_snapshot.clone(),
            ),
            transcript_snapshot,
        ))
    }

    pub(crate) fn build_document_viewport(&mut self, layout: &DocumentLayout) -> DocumentViewport {
        let uses_bottom_follow = self.follow_bottom && !self.manual_document_scroll;
        let key = DocumentViewportKey {
            layout_key: self.current_document_layout_key(),
            offset: self.document_viewport_y,
            height: self.document_viewport_height(),
            bottom_follow: uses_bottom_follow,
            selection_version: self.selection_version,
        };
        if self.document_viewport_cache.valid && self.document_viewport_cache.key == key {
            return self.document_viewport_cache.viewport.clone();
        }

        let mut viewport = compose_document_viewport(
            layout,
            self.document_viewport_y,
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

        self.document_viewport_cache = DocumentViewportCache {
            key,
            viewport: viewport.clone(),
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
            composer_value: self.composer.value().to_string(),
            composer_width: self.composer.content_width(),
            composer_prompt: self.composer.prompt().to_string(),
            composer_placeholder: self.composer.placeholder().to_string(),
            composer_line: self.composer.line(),
            composer_column: self.composer.column(),
            status_line_text: self.current_status_line_cache_key(),
        }
    }

    pub(crate) fn current_document_layout_input(&mut self) -> DocumentLayoutInput {
        let composer_document = self.composer.render_document(self.palette);
        let transcript = self.current_document_transcript_snapshot();

        DocumentLayoutInput {
            transcript,
            composer_lines: composer_document.lines,
            composer_plain_lines: composer_document.plain_lines,
            composer_anchors: document_anchors_for_composer(&composer_document.anchors),
            composer_selectable: composer_document.selectable_ranges,
            composer_frame_decoration_line: composer_document.frame_decoration_line,
            composer_frame_decoration_plain_line: composer_document.frame_decoration_plain_line,
            composer_cursor_x: composer_document.cursor_x,
            composer_cursor_y: composer_document.cursor_y,
            status_line: self.current_status_line_render_result(),
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
    let transcript_line_count = input.transcript.lines.len();
    let (transcript_segments, transcript_items) = new_document_transcript_index(&input.transcript);
    let extra_gap = if transcript_line_count == 0 {
        0
    } else {
        transcript_composer_gap_line_count()
    };
    let has_composer_padding = input.composer_frame_decoration_line.is_some();
    let mut lines = Vec::with_capacity(
        transcript_line_count
            + extra_gap
            + input.composer_lines.len()
            + usize::from(has_composer_padding) * 2
            + input.status_line.gap_before
            + usize::from(input.status_line.has_content),
    );
    let mut plain_lines = Vec::with_capacity(
        transcript_line_count
            + extra_gap
            + input.composer_plain_lines.len()
            + usize::from(has_composer_padding) * 2
            + input.status_line.gap_before
            + usize::from(input.status_line.has_content),
    );
    let mut anchors = Vec::with_capacity(
        transcript_line_count
            + extra_gap
            + input.composer_anchors.len()
            + usize::from(has_composer_padding) * 2
            + input.status_line.gap_before
            + usize::from(input.status_line.has_content),
    );
    let mut selectable = Vec::with_capacity(
        transcript_line_count
            + extra_gap
            + input.composer_selectable.len()
            + usize::from(has_composer_padding) * 2
            + input.status_line.gap_before
            + usize::from(input.status_line.has_content),
    );

    lines.extend(input.transcript.lines.iter().cloned());
    plain_lines.extend(input.transcript.plain_lines.iter().cloned());
    anchors.extend(input.transcript.anchors.iter().copied());
    selectable.extend(std::iter::repeat_n(
        SelectableLineRange::default(),
        transcript_line_count,
    ));
    if transcript_line_count > 0 {
        for gap_index in 0..transcript_composer_gap_line_count() {
            lines.push(Line::raw(""));
            plain_lines.push(String::new());
            anchors.push(DocumentLineAnchor {
                region: DocumentAnchorRegion::TranscriptComposerGap,
                gap_index,
                ..DocumentLineAnchor::default()
            });
            selectable.push(SelectableLineRange::default());
        }
    }

    let composer_slot = SlotFrame::new(
        lines.len(),
        has_composer_padding,
        input.composer_plain_lines.len(),
    );
    if let (Some(line), Some(plain)) = (
        input.composer_frame_decoration_line.clone(),
        input.composer_frame_decoration_plain_line.clone(),
    ) {
        lines.push(line);
        plain_lines.push(plain);
        anchors.push(DocumentLineAnchor {
            region: DocumentAnchorRegion::ComposerPadding,
            gap_index: 0,
            ..DocumentLineAnchor::default()
        });
        selectable.push(SelectableLineRange::default());
    }

    lines.extend(input.composer_lines);
    plain_lines.extend(input.composer_plain_lines);
    anchors.extend(input.composer_anchors);
    selectable.extend(ensure_selectable_ranges(
        &plain_lines[plain_lines.len() - input.composer_selectable.len()..],
        &input.composer_selectable,
    ));
    if let (Some(line), Some(plain)) = (
        input.composer_frame_decoration_line,
        input.composer_frame_decoration_plain_line,
    ) {
        lines.push(line);
        plain_lines.push(plain);
        anchors.push(DocumentLineAnchor {
            region: DocumentAnchorRegion::ComposerPadding,
            gap_index: 1,
            ..DocumentLineAnchor::default()
        });
        selectable.push(SelectableLineRange::default());
    }

    if input.status_line.has_content {
        for gap_index in 0..input.status_line.gap_before {
            lines.push(Line::raw(""));
            plain_lines.push(String::new());
            anchors.push(DocumentLineAnchor {
                region: DocumentAnchorRegion::ComposerStatusGap,
                gap_index,
                ..DocumentLineAnchor::default()
            });
            selectable.push(SelectableLineRange::default());
        }

        if let Some(line) = input.status_line.line {
            lines.push(line);
            plain_lines.push(input.status_line.plain_line);
            anchors.push(DocumentLineAnchor {
                region: DocumentAnchorRegion::StatusLine,
                ..DocumentLineAnchor::default()
            });
            selectable.push(input.status_line.selectable);
        }
    }

    let composer_slot = if lines.is_empty() {
        SlotFrame::new(0, false, 1)
    } else {
        composer_slot
    };
    if lines.is_empty() {
        lines.push(Line::raw(""));
        plain_lines.push(String::new());
        anchors.push(DocumentLineAnchor::default());
        selectable.push(SelectableLineRange::default());
    }

    DocumentLayout {
        transcript: input.transcript,
        transcript_line_count,
        transcript_segments,
        transcript_items,
        composer_slot,
        composer_start_line: composer_slot.content_start_line,
        composer_line_count: composer_slot.content_line_count,
        cursor_x: input.composer_cursor_x,
        cursor_y: composer_slot.content_start_line + input.composer_cursor_y,
        lines,
        plain_lines,
        anchors,
        selectable,
    }
}

pub(crate) fn transcript_composer_gap_line_count() -> usize {
    1
}

fn ensure_selectable_ranges(
    plain_lines: &[String],
    ranges: &[SelectableLineRange],
) -> Vec<SelectableLineRange> {
    plain_lines
        .iter()
        .enumerate()
        .map(|(index, line)| {
            ranges
                .get(index)
                .copied()
                .unwrap_or_else(|| selectable_range_for_plain_line(line))
        })
        .collect()
}

pub(crate) fn compose_document_viewport(
    layout: &DocumentLayout,
    offset: usize,
    height: usize,
) -> DocumentViewport {
    let (lines, plain_lines, resolved_offset) = visible_document_lines(layout, offset, height);

    DocumentViewport {
        lines,
        plain_lines,
        resolved_offset,
    }
}

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
    (
        layout.lines_for_range(resolved_offset, height),
        layout.line_texts_for_range(resolved_offset, height),
        resolved_offset,
    )
}

fn document_anchors_for_transcript(
    line_anchors: &[transcript::LineAnchor],
) -> Vec<DocumentLineAnchor> {
    line_anchors
        .iter()
        .copied()
        .map(|transcript| DocumentLineAnchor {
            region: DocumentAnchorRegion::Transcript,
            transcript,
            ..DocumentLineAnchor::default()
        })
        .collect()
}

impl Model {
    pub(crate) fn current_document_transcript_snapshot(&mut self) -> DocumentTranscriptSnapshot {
        let key = DocumentTranscriptKey {
            transcript_render_version: self.transcript_render_version,
            document_width: self.width,
        };
        if self.document_transcript_cache.valid && self.document_transcript_cache.key == key {
            return self.document_transcript_cache.snapshot.clone();
        }

        let mut items = std::collections::HashMap::new();
        for anchor in &self.transcript_render.line_anchors {
            if items.contains_key(&anchor.item_index) {
                continue;
            }
            if let Some(item) = self.transcript.item(anchor.item_index).cloned() {
                items.insert(anchor.item_index, item);
            }
        }

        let snapshot = DocumentTranscriptSnapshot {
            lines: self.transcript_render.lines.clone(),
            plain_lines: self.transcript_render.plain_lines.clone(),
            anchors: document_anchors_for_transcript(&self.transcript_render.line_anchors),
            width: if self.width == 0 {
                crate::frontend::tui::transcript::DEFAULT_RENDER_WIDTH as u16
            } else {
                self.width
            },
            palette: self.palette,
            items,
            selectable_cache: std::rc::Rc::new(std::cell::RefCell::new(
                std::collections::HashMap::new(),
            )),
        };
        self.document_transcript_cache = DocumentTranscriptCache {
            key,
            snapshot: snapshot.clone(),
            valid: true,
        };
        snapshot
    }

    pub(crate) fn document_transcript_snapshot_after_append(
        &mut self,
        appended: &super::append::DocumentTranscriptAppend,
    ) -> DocumentTranscriptSnapshot {
        let previous_key = DocumentTranscriptKey {
            transcript_render_version: self.document_layout_cache.key.transcript_render_version,
            document_width: self.width,
        };
        if self.document_transcript_cache.valid
            && self.document_transcript_cache.key == previous_key
        {
            return super::append::extend_document_transcript_snapshot_from_append(
                &self.document_transcript_cache.snapshot,
                appended,
            );
        }

        self.current_document_transcript_snapshot()
    }
}

fn document_anchors_for_composer(line_anchors: &[composer::LineAnchor]) -> Vec<DocumentLineAnchor> {
    line_anchors
        .iter()
        .copied()
        .map(|composer| DocumentLineAnchor {
            region: DocumentAnchorRegion::Composer,
            composer,
            ..DocumentLineAnchor::default()
        })
        .collect()
}
