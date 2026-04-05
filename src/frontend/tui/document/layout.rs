use std::rc::Rc;

use ratatui::text::Line;

use crate::frontend::tui::{
    Model,
    command_panel::CommandPanelRenderResult,
    composer,
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
    line_access::new_document_transcript_item_index,
    slot_frame::SlotFrame,
    slot_viewport::compose_bottom_follow_document_viewport,
};

#[derive(Debug, Clone)]
pub(crate) struct DocumentLayoutInput {
    pub(crate) transcript: Rc<DocumentTranscriptSnapshot>,
    pub(crate) composer_lines: Vec<Line<'static>>,
    pub(crate) composer_plain_lines: Vec<String>,
    pub(crate) composer_anchors: Vec<DocumentLineAnchor>,
    pub(crate) composer_selectable: Vec<SelectableLineRange>,
    pub(crate) composer_frame_decoration_line: Option<Line<'static>>,
    pub(crate) composer_frame_decoration_plain_line: Option<String>,
    pub(crate) composer_cursor_x: u16,
    pub(crate) composer_cursor_y: usize,
    pub(crate) command_panel: CommandPanelRenderResult,
    pub(crate) status_line: StatusLineRenderResult,
}

impl Model {
    pub(crate) fn invalidate_document_viewport_cache(&mut self) {
        self.document_viewport_cache.valid = false;
    }

    #[cfg(test)]
    pub(crate) fn invalidate_document_caches_for_test(&mut self) {
        self.document_layout_cache.valid = false;
        self.document_viewport_cache.valid = false;
    }

    pub(crate) fn build_document_layout(&mut self) -> Rc<DocumentLayout> {
        let key = self.current_document_layout_key();
        if self.document_layout_cache.valid && self.document_layout_cache.key == key {
            return Rc::clone(&self.document_layout_cache.layout);
        }

        if let Some((layout, transcript_snapshot)) =
            self.build_document_layout_from_transcript_append(&key)
        {
            self.document_transcript_cache = DocumentTranscriptCache {
                key: DocumentTranscriptKey {
                    transcript_render_version: self.transcript_render_version,
                    document_width: self.width,
                },
                snapshot: Rc::clone(&transcript_snapshot),
                valid: true,
            };
            let layout = Rc::new(layout);
            self.document_layout_cache = DocumentLayoutCache {
                key,
                layout: Rc::clone(&layout),
                transcript_line_count: self.transcript_render.line_count,
                valid: true,
            };
            return layout;
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
            transcript_line_count: self.transcript_render.line_count,
            valid: true,
        };
        layout
    }

    pub(crate) fn build_document_layout_from_transcript_append(
        &mut self,
        key: &DocumentLayoutKey,
    ) -> Option<(DocumentLayout, Rc<DocumentTranscriptSnapshot>)> {
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
                Rc::clone(&transcript_snapshot),
            ),
            transcript_snapshot,
        ))
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
            viewport_height: self.document_viewport_height(),
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
            command_panel: self.current_inline_command_panel_render_result(),
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
    let transcript_items = new_document_transcript_item_index(&input.transcript);
    let extra_gap = if transcript_line_count == 0 {
        0
    } else {
        transcript_composer_gap_line_count()
    };
    let has_composer_padding = input.composer_frame_decoration_line.is_some();
    let mut tail_lines = Vec::with_capacity(
        extra_gap
            + input.composer_lines.len()
            + usize::from(has_composer_padding) * 2
            + input.command_panel.lines.len()
            + input.status_line.gap_before
            + usize::from(input.status_line.has_content),
    );
    let mut tail_plain_lines = Vec::with_capacity(
        extra_gap
            + input.composer_plain_lines.len()
            + usize::from(has_composer_padding) * 2
            + input.command_panel.plain_lines.len()
            + input.status_line.gap_before
            + usize::from(input.status_line.has_content),
    );
    let mut tail_anchors = Vec::with_capacity(
        extra_gap
            + input.composer_anchors.len()
            + usize::from(has_composer_padding) * 2
            + input.command_panel.lines.len()
            + input.status_line.gap_before
            + usize::from(input.status_line.has_content),
    );
    let mut tail_selectable = Vec::with_capacity(
        extra_gap
            + input.composer_selectable.len()
            + usize::from(has_composer_padding) * 2
            + input.command_panel.selectable.len()
            + input.status_line.gap_before
            + usize::from(input.status_line.has_content),
    );

    if transcript_line_count > 0 {
        for gap_index in 0..transcript_composer_gap_line_count() {
            tail_lines.push(Line::raw(""));
            tail_plain_lines.push(String::new());
            tail_anchors.push(DocumentLineAnchor {
                region: DocumentAnchorRegion::TranscriptComposerGap,
                gap_index,
                ..DocumentLineAnchor::default()
            });
            tail_selectable.push(SelectableLineRange::default());
        }
    }

    let composer_slot = SlotFrame::new(
        transcript_line_count + tail_lines.len(),
        has_composer_padding,
        input.composer_plain_lines.len(),
    );
    if let (Some(line), Some(plain)) = (
        input.composer_frame_decoration_line.clone(),
        input.composer_frame_decoration_plain_line.clone(),
    ) {
        tail_lines.push(line);
        tail_plain_lines.push(plain);
        tail_anchors.push(DocumentLineAnchor {
            region: DocumentAnchorRegion::ComposerPadding,
            gap_index: 0,
            ..DocumentLineAnchor::default()
        });
        tail_selectable.push(SelectableLineRange::default());
    }

    tail_lines.extend(input.composer_lines);
    tail_plain_lines.extend(input.composer_plain_lines);
    tail_anchors.extend(input.composer_anchors);
    tail_selectable.extend(ensure_selectable_ranges(
        &tail_plain_lines[tail_plain_lines.len() - input.composer_selectable.len()..],
        &input.composer_selectable,
    ));
    if let (Some(line), Some(plain)) = (
        input.composer_frame_decoration_line,
        input.composer_frame_decoration_plain_line,
    ) {
        tail_lines.push(line);
        tail_plain_lines.push(plain);
        tail_anchors.push(DocumentLineAnchor {
            region: DocumentAnchorRegion::ComposerPadding,
            gap_index: 1,
            ..DocumentLineAnchor::default()
        });
        tail_selectable.push(SelectableLineRange::default());
    }

    if input.command_panel.has_content {
        for index in 0..input.command_panel.lines.len() {
            tail_lines.push(input.command_panel.lines[index].clone());
            tail_plain_lines.push(
                input
                    .command_panel
                    .plain_lines
                    .get(index)
                    .cloned()
                    .unwrap_or_default(),
            );
            tail_anchors.push(DocumentLineAnchor {
                region: DocumentAnchorRegion::CommandPanel,
                gap_index: index,
                ..DocumentLineAnchor::default()
            });
            tail_selectable.push(
                input
                    .command_panel
                    .selectable
                    .get(index)
                    .copied()
                    .unwrap_or_default(),
            );
        }
    }

    if input.status_line.has_content {
        for gap_index in 0..input.status_line.gap_before {
            tail_lines.push(Line::raw(""));
            tail_plain_lines.push(String::new());
            tail_anchors.push(DocumentLineAnchor {
                region: DocumentAnchorRegion::ComposerStatusGap,
                gap_index,
                ..DocumentLineAnchor::default()
            });
            tail_selectable.push(SelectableLineRange::default());
        }

        if let Some(line) = input.status_line.line {
            tail_lines.push(line);
            tail_plain_lines.push(input.status_line.plain_line);
            tail_anchors.push(DocumentLineAnchor {
                region: DocumentAnchorRegion::StatusLine,
                ..DocumentLineAnchor::default()
            });
            tail_selectable.push(input.status_line.selectable);
        }
    }

    let composer_slot = if transcript_line_count == 0 && tail_lines.is_empty() {
        SlotFrame::new(0, false, 1)
    } else {
        composer_slot
    };
    if transcript_line_count == 0 && tail_lines.is_empty() {
        tail_lines.push(Line::raw(""));
        tail_plain_lines.push(String::new());
        tail_anchors.push(DocumentLineAnchor::default());
        tail_selectable.push(SelectableLineRange::default());
    }

    DocumentLayout {
        transcript: input.transcript,
        transcript_line_count,
        transcript_items,
        tail_lines,
        tail_plain_lines,
        tail_anchors,
        tail_selectable,
        composer_slot,
        composer_start_line: composer_slot.content_start_line,
        composer_line_count: composer_slot.content_line_count,
        cursor_x: input.composer_cursor_x,
        cursor_y: composer_slot.content_start_line + input.composer_cursor_y,
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

    DocumentViewport {
        lines: layout.lines_for_range(resolved_offset, visible_line_count),
        plain_text_len: layout.plain_text_len_for_range(resolved_offset, visible_line_count),
        #[cfg(test)]
        plain_lines: layout.line_texts_for_range(resolved_offset, visible_line_count),
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

        let mut items = std::collections::HashMap::new();
        let mut previous_item_index = None;
        for anchor in &self.transcript_render.line_anchors {
            if previous_item_index == Some(anchor.item_index) {
                continue;
            }
            previous_item_index = Some(anchor.item_index);
            if let Some(item) = self.transcript.item(anchor.item_index).cloned() {
                items.insert(anchor.item_index, item);
            }
        }

        let snapshot = Rc::new(DocumentTranscriptSnapshot {
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
        });
        self.document_transcript_cache = DocumentTranscriptCache {
            key,
            snapshot: Rc::clone(&snapshot),
            valid: true,
        };
        snapshot
    }

    pub(crate) fn document_transcript_snapshot_after_append(
        &mut self,
        appended: &super::append::DocumentTranscriptAppend,
    ) -> Rc<DocumentTranscriptSnapshot> {
        let previous_key = DocumentTranscriptKey {
            transcript_render_version: self.document_layout_cache.key.transcript_render_version,
            document_width: self.width,
        };
        if self.document_transcript_cache.valid
            && self.document_transcript_cache.key == previous_key
        {
            return Rc::new(
                super::append::extend_document_transcript_snapshot_from_append(
                    &self.document_transcript_cache.snapshot,
                    appended,
                ),
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
