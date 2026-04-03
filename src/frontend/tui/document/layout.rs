use ratatui::text::Line;

use crate::frontend::tui::{Model, composer, status_line::StatusLineRenderResult, transcript};

use super::{
    DocumentAnchorRegion, DocumentLayout, DocumentLayoutCache, DocumentLayoutKey,
    DocumentLineAnchor, DocumentViewport, DocumentViewportCache, DocumentViewportKey,
    slot_frame::SlotFrame, slot_viewport::compose_bottom_follow_document_viewport,
};

#[derive(Debug, Clone)]
pub(crate) struct DocumentLayoutInput {
    pub(crate) transcript_lines: Vec<Line<'static>>,
    pub(crate) transcript_plain_lines: Vec<String>,
    pub(crate) transcript_anchors: Vec<DocumentLineAnchor>,
    pub(crate) composer_lines: Vec<Line<'static>>,
    pub(crate) composer_plain_lines: Vec<String>,
    pub(crate) composer_anchors: Vec<DocumentLineAnchor>,
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

        let layout = compose_document_layout(self.current_document_layout_input());

        self.document_layout_cache = DocumentLayoutCache {
            key,
            layout: layout.clone(),
            valid: true,
        };
        layout
    }

    pub(crate) fn build_document_viewport(&mut self, layout: &DocumentLayout) -> DocumentViewport {
        let uses_bottom_follow = self.follow_bottom && !self.manual_document_scroll;
        let key = DocumentViewportKey {
            layout_key: self.current_document_layout_key(),
            offset: self.document_viewport_y,
            height: self.document_viewport_height(),
            bottom_follow: uses_bottom_follow,
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

    pub(crate) fn current_document_layout_input(&self) -> DocumentLayoutInput {
        let composer_document = self.composer.render_document(self.palette);

        DocumentLayoutInput {
            transcript_lines: self.transcript_render.lines.clone(),
            transcript_plain_lines: self.transcript_render.plain_lines.clone(),
            transcript_anchors: document_anchors_for_transcript(
                &self.transcript_render.line_anchors,
            ),
            composer_lines: composer_document.lines,
            composer_plain_lines: composer_document.plain_lines,
            composer_anchors: document_anchors_for_composer(&composer_document.anchors),
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
    let extra_gap = usize::from(!input.transcript_lines.is_empty());
    let has_composer_padding = input.composer_frame_decoration_line.is_some();
    let mut lines = Vec::with_capacity(
        input.transcript_lines.len()
            + extra_gap
            + input.composer_lines.len()
            + usize::from(has_composer_padding) * 2
            + input.status_line.gap_before
            + usize::from(input.status_line.has_content),
    );
    let mut plain_lines = Vec::with_capacity(
        input.transcript_plain_lines.len()
            + extra_gap
            + input.composer_plain_lines.len()
            + usize::from(has_composer_padding) * 2
            + input.status_line.gap_before
            + usize::from(input.status_line.has_content),
    );
    let mut anchors = Vec::with_capacity(
        input.transcript_anchors.len()
            + extra_gap
            + input.composer_anchors.len()
            + usize::from(has_composer_padding) * 2
            + input.status_line.gap_before
            + usize::from(input.status_line.has_content),
    );

    lines.extend(input.transcript_lines);
    plain_lines.extend(input.transcript_plain_lines);
    anchors.extend(input.transcript_anchors);
    if !lines.is_empty() {
        lines.push(Line::raw(""));
        plain_lines.push(String::new());
        anchors.push(DocumentLineAnchor {
            region: DocumentAnchorRegion::TranscriptComposerGap,
            gap_index: 0,
            ..DocumentLineAnchor::default()
        });
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
    }

    lines.extend(input.composer_lines);
    plain_lines.extend(input.composer_plain_lines);
    anchors.extend(input.composer_anchors);
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
        }

        if let Some(line) = input.status_line.line {
            lines.push(line);
            plain_lines.push(input.status_line.plain_line);
            anchors.push(DocumentLineAnchor {
                region: DocumentAnchorRegion::StatusLine,
                ..DocumentLineAnchor::default()
            });
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
    }

    DocumentLayout {
        composer_slot,
        composer_start_line: composer_slot.content_start_line,
        composer_line_count: composer_slot.content_line_count,
        cursor_x: input.composer_cursor_x,
        cursor_y: composer_slot.content_start_line + input.composer_cursor_y,
        lines,
        plain_lines,
        anchors,
    }
}

pub(crate) fn compose_document_viewport(
    layout: &DocumentLayout,
    offset: usize,
    height: usize,
) -> DocumentViewport {
    let (lines, plain_lines, resolved_offset) =
        visible_document_lines(&layout.lines, &layout.plain_lines, offset, height);

    DocumentViewport {
        lines,
        plain_lines,
        resolved_offset,
    }
}

pub(crate) fn visible_document_lines(
    lines: &[Line<'static>],
    plain_lines: &[String],
    offset: usize,
    height: usize,
) -> (Vec<Line<'static>>, Vec<String>, usize) {
    if lines.is_empty() {
        return (vec![Line::raw("")], vec![String::new()], 0);
    }

    if height == 0 || height >= lines.len() {
        return (lines.to_vec(), plain_lines.to_vec(), 0);
    }

    let max_offset = lines.len().saturating_sub(height);
    let resolved_offset = offset.min(max_offset);
    let end = resolved_offset + height;
    (
        lines[resolved_offset..end].to_vec(),
        plain_lines[resolved_offset..end].to_vec(),
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
