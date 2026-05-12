use std::{cell::RefCell, collections::HashMap, rc::Rc};

use ratatui::text::Line;

use crate::frontend::tui::{
    Model, selection::apply_selection_to_viewport, transcript::TranscriptItemMetricsIndex,
};

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
        self.document_runtime.viewport_cache.valid = false;
    }

    #[cfg(test)]
    pub(crate) fn invalidate_document_caches_for_test(&mut self) {
        self.document_runtime.tail_layout_cache.valid = false;
        self.document_runtime.layout_cache.valid = false;
        self.document_runtime.viewport_cache.valid = false;
    }

    pub(crate) fn build_document_layout(&mut self) -> Rc<DocumentLayout> {
        self.ensure_current_transcript_window_exact();
        let key = self.current_document_layout_key();
        if self.document_runtime.layout_cache.valid && self.document_runtime.layout_cache.key == key
        {
            return Rc::clone(&self.document_runtime.layout_cache.layout);
        }

        let layout = Rc::new(compose_document_layout(
            self.current_document_layout_input(),
        ));
        self.document_runtime.transcript_cache = DocumentTranscriptCache {
            key: DocumentTranscriptKey {
                transcript_render_version: self.transcript_render_version,
                document_width: self.width,
                tool_activity_frame: self.tool_activity_frame_key(std::time::Instant::now()),
            },
            snapshot: Rc::clone(&layout.transcript),
            valid: true,
        };
        self.document_runtime.layout_cache = DocumentLayoutCache {
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
        let uses_bottom_follow = self.document_runtime.viewport_state.follow_bottom()
            && !self.document_runtime.viewport_state.manual_scroll();
        let viewport_height = self.document_viewport_height();
        let key = DocumentViewportKey {
            layout_key: self.current_document_layout_key(),
            offset: self.document_runtime.viewport_state.resolved_offset(),
            height: viewport_height,
            bottom_follow: uses_bottom_follow,
            selection_version: self.selection_runtime.version,
        };
        if self.document_runtime.viewport_cache.valid
            && self.document_runtime.viewport_cache.key == key
        {
            return Rc::clone(&self.document_runtime.viewport_cache.viewport);
        }

        let mut viewport = compose_document_viewport(
            layout,
            self.document_runtime.viewport_state.resolved_offset(),
            viewport_height,
        );
        if uses_bottom_follow {
            viewport = compose_bottom_follow_document_viewport(
                layout,
                viewport_height,
                self.bottom_follow_presentation(layout),
            );
        }
        apply_selection_to_viewport(&mut viewport, layout, self.selection_runtime.selection);

        let viewport = Rc::new(viewport);
        self.document_runtime.viewport_cache = DocumentViewportCache {
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
            acp_panel_active: self.acp_panel_active(),
            acp_panel_selected: self.acp_panel.selected,
            acp_panel_scroll: self.acp_panel.scroll,
            acp_debug_panel_selected: self.acp_debug_panel.selected,
            acp_debug_panel_scroll: self.acp_debug_panel.scroll,
            tool_approval_panel_active: self.tool_approval_panel_active(),
            tool_approval_panel_selected: self.tool_approval_panel.selected,
            tool_approval_panel_revision: self.tool_approval_panel_revision,
            selected_acp_agent: self.selected_acp_agent.clone(),
            model_panel_active: self.model_panel_active(),
            model_panel_provider_index: self.model_panel.provider_index,
            model_panel_model_index: self.model_panel.model_index,
            model_panel_scroll: self.model_panel.scroll,
            selected_model: self
                .selected_model
                .as_ref()
                .map(|model| model.display_name()),
            status_line_config: self.status_line_config_bits(),
            status_line_2_config: self.status_line_2_config_bits(),
            status_line_revision: self.status_line_revision(),
            stream_activity_frame: self.stream_activity_frame_key(std::time::Instant::now()),
            tool_activity_frame: self.tool_activity_frame_key(std::time::Instant::now()),
        }
    }

    pub(crate) fn current_document_layout_input(&mut self) -> DocumentLayoutInput {
        DocumentLayoutInput {
            transcript: self.current_document_transcript_snapshot(),
            tail: self.build_document_tail_layout(),
        }
    }

    /// `transient_document_transcript_snapshot` 用当前 transcript index 构造可解析锚点的临时快照，
    /// 避免在预热窗口计算里递归走完整的 snapshot 预热路径。
    pub(crate) fn transient_document_transcript_snapshot(
        &self,
        index: TranscriptItemMetricsIndex,
    ) -> DocumentTranscriptSnapshot {
        DocumentTranscriptSnapshot {
            index,
            width: if self.width == 0 {
                crate::frontend::tui::transcript::DEFAULT_RENDER_WIDTH as u16
            } else {
                self.width
            },
            palette: self.palette,
            items: self.transcript.items_snapshot(),
            warmed_item_block_cache: self.transcript.cached_screen_blocks_snapshot(),
            item_block_cache: Rc::new(RefCell::new(HashMap::new())),
            item_text_lines_cache: Rc::new(RefCell::new(HashMap::new())),
            selectable_cache: Rc::new(RefCell::new(HashMap::new())),
        }
    }

    /// `document_layout_for_transcript_index` 组合当前 tail 与 transcript index，
    /// 生成一份可用于 viewport 恢复解析的临时文档布局。
    pub(crate) fn document_layout_for_transcript_index(
        &mut self,
        index: TranscriptItemMetricsIndex,
    ) -> DocumentLayout {
        compose_document_layout(DocumentLayoutInput {
            transcript: Rc::new(self.transient_document_transcript_snapshot(index)),
            tail: self.build_document_tail_layout(),
        })
    }

    pub(crate) fn transcript_window_layout(
        &mut self,
        transcript_line_count: usize,
    ) -> DocumentLayout {
        let tail = self.build_document_tail_layout();
        let composer_slot = offset_slot_frame(tail.composer_slot, transcript_line_count);

        DocumentLayout {
            transcript: Rc::new(DocumentTranscriptSnapshot {
                index: TranscriptItemMetricsIndex {
                    line_count: transcript_line_count,
                    ..TranscriptItemMetricsIndex::default()
                },
                ..DocumentTranscriptSnapshot::default()
            }),
            transcript_line_count,
            tail: Rc::clone(&tail),
            composer_slot,
            composer_start_line: composer_slot.content_start_line,
            composer_line_count: composer_slot.content_line_count,
            cursor_x: tail.cursor_x,
            cursor_y: transcript_line_count + tail.cursor_y,
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
            assistant_lines: vec![false],
            plain_text_len: 0,
            #[cfg(test)]
            plain_lines: vec![String::new()],
            resolved_offset: 0,
        };
    }

    if height == 0 || height >= layout.line_count() {
        let range = document_range_snapshot(layout, 0, layout.line_count());
        return DocumentViewport {
            lines: range.lines,
            assistant_lines: range.assistant_lines,
            plain_text_len: range.plain_text_len,
            #[cfg(test)]
            plain_lines: range.plain_lines,
            resolved_offset: 0,
        };
    }

    let max_offset = layout.line_count().saturating_sub(height);
    let resolved_offset = offset.min(max_offset);
    let visible_line_count = height.min(layout.line_count() - resolved_offset);
    let range = document_range_snapshot(layout, resolved_offset, visible_line_count);

    DocumentViewport {
        lines: range.lines,
        assistant_lines: range.assistant_lines,
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
    assistant_lines: Vec<bool>,
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
    let mut assistant_lines = Vec::with_capacity(end - start);
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
        assistant_lines.extend(transcript_slice.assistant_lines);
        #[cfg(test)]
        plain_lines.extend(transcript_slice.plain_lines);
        start = transcript_end;
    }

    if start < end {
        let tail_start = start - layout.transcript_line_count;
        let tail_end = end - layout.transcript_line_count;
        let tail_line_count = tail_end - tail_start;
        lines.extend_from_slice(&layout.tail.lines[tail_start..tail_end]);
        assistant_lines.extend(std::iter::repeat_n(false, tail_line_count));
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
        assistant_lines,
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
            tool_activity_frame: self.tool_activity_frame_key(std::time::Instant::now()),
        };
        if self.document_runtime.transcript_cache.valid
            && self.document_runtime.transcript_cache.key == key
        {
            return Rc::clone(&self.document_runtime.transcript_cache.snapshot);
        }

        self.transcript.begin_recent_render_block_batch();
        let index = if self.transcript_render.index.metrics.len() == self.transcript.len() {
            self.transcript_render.index.clone()
        } else {
            self.transcript.progressive_item_metrics_index()
        };
        let warmed_item_count = if let Some((start, count)) =
            self.current_visible_transcript_window(index.line_count)
        {
            self.transcript
                .prewarm_viewport_window(&index, start, count)
        } else {
            0
        };
        let warmed_item_block_cache = self.transcript.cached_screen_blocks_snapshot();
        self.transcript
            .finish_recent_render_block_batch(warmed_item_count);
        let snapshot = Rc::new(DocumentTranscriptSnapshot {
            warmed_item_block_cache,
            ..self.transient_document_transcript_snapshot(index)
        });
        self.document_runtime.transcript_cache = DocumentTranscriptCache {
            key,
            snapshot: Rc::clone(&snapshot),
            valid: true,
        };
        snapshot
    }
}
