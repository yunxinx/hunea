use std::rc::Rc;

use ratatui::text::Line;

use crate::{
    frame_time::FrameRenderContext,
    selection::{
        ResolvedSelectionPoint, SelectableLineRange, SelectionPoint,
        normalize_transcript_selectable_range,
    },
    transcript::{
        CachedRenderBlock, ItemLineAnchor, LineAnchor, LineAnchorKind,
        materialize_transcript_item_render_block, viewport_overscan_line_budget,
    },
};

use super::{
    DocumentAnchorRegion, DocumentLayout, DocumentLayoutLine, DocumentLineAnchor,
    DocumentTranscriptItemLines, DocumentTranscriptSnapshot,
};

/// `DocumentSelectionLine` 表示 selection / copy 路径消费的一条语义行。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DocumentSelectionLine {
    pub(crate) text: String,
    pub(crate) anchor: DocumentLineAnchor,
    pub(crate) selectable: SelectableLineRange,
}

/// `DocumentTranscriptViewportSnapshot` 描述 transcript 当前局部窗口真正需要的行级数据。
#[derive(Debug, Clone, Default)]
pub(super) struct DocumentTranscriptViewportSnapshot {
    pub(super) lines: Vec<Line<'static>>,
    pub(super) assistant_lines: Vec<bool>,
    pub(super) plain_text_len: usize,
    pub(super) resolved_offset: usize,
    #[cfg(test)]
    pub(super) plain_lines: Vec<String>,
}

#[derive(Debug, Clone)]
struct DocumentTranscriptViewportLine {
    line: Line<'static>,
    is_assistant: bool,
    plain_line_len: usize,
    #[cfg(test)]
    plain_line: String,
}

impl DocumentTranscriptSnapshot {
    pub(crate) fn line_count(&self) -> usize {
        self.index.line_count
    }

    pub(crate) fn plain_text_len_for_range(
        &self,
        start: usize,
        count: usize,
        context: FrameRenderContext,
    ) -> usize {
        if start == 0 && count >= self.line_count() {
            return if self.line_count() == 0 {
                0
            } else {
                self.index.content_char_len + self.line_count().saturating_sub(1)
            };
        }

        self.range_snapshot(start, count, false, context)
            .plain_text_len
    }

    #[cfg(test)]
    pub(crate) fn plain_lines_for_range(
        &self,
        start: usize,
        count: usize,
        context: FrameRenderContext,
    ) -> Vec<String> {
        self.range_snapshot(start, count, true, context).plain_lines
    }

    pub(crate) fn lines_for_range(
        &self,
        start: usize,
        count: usize,
        context: FrameRenderContext,
    ) -> Vec<Line<'static>> {
        self.range_snapshot(start, count, false, context).lines
    }

    pub(super) fn viewport_snapshot(
        &self,
        offset: usize,
        height: usize,
        context: FrameRenderContext,
    ) -> DocumentTranscriptViewportSnapshot {
        if self.line_count() == 0 {
            return DocumentTranscriptViewportSnapshot::default();
        }

        let resolved_offset = if height == 0 || height >= self.line_count() {
            0
        } else {
            offset.min(self.line_count().saturating_sub(height))
        };
        let visible_line_count = if height == 0 || height >= self.line_count() {
            self.line_count()
        } else {
            height
        };

        let mut snapshot =
            self.range_snapshot(resolved_offset, visible_line_count, cfg!(test), context);
        snapshot.resolved_offset = resolved_offset;
        snapshot
    }

    pub(crate) fn line_at(
        &self,
        index: usize,
        context: FrameRenderContext,
    ) -> Option<DocumentLayoutLine> {
        self.materialize_line(index, true, context)
    }

    pub(crate) fn plain_line_at(
        &self,
        index: usize,
        context: FrameRenderContext,
    ) -> Option<String> {
        let position = self.index.position_for_line(index)?;
        let relative = index.saturating_sub(position.start_line);
        if relative < position.gap_before {
            return Some(String::new());
        }

        let block = self.render_block(position.item_index, context)?;
        block.plain_line_at(relative - position.gap_before)
    }

    pub(crate) fn anchor_at(
        &self,
        index: usize,
        context: FrameRenderContext,
    ) -> Option<DocumentLineAnchor> {
        let position = self.index.position_for_line(index)?;
        let relative = index.saturating_sub(position.start_line);
        if relative < position.gap_before {
            return Some(document_anchor_for_transcript(LineAnchor {
                item_index: position.gap_owner_item_index.unwrap_or(position.item_index),
                item_anchor: ItemLineAnchor {
                    kind: LineAnchorKind::ItemGap,
                    gap_offset: relative,
                    ..ItemLineAnchor::default()
                },
            }));
        }

        let block = self.render_block(position.item_index, context)?;
        Some(document_anchor_for_transcript(LineAnchor {
            item_index: position.item_index,
            item_anchor: block.anchor_at(relative - position.gap_before)?,
        }))
    }

    fn materialize_line(
        &self,
        index: usize,
        include_selectable: bool,
        context: FrameRenderContext,
    ) -> Option<DocumentLayoutLine> {
        let position = self.index.position_for_line(index)?;
        let relative = index.saturating_sub(position.start_line);
        if relative < position.gap_before {
            return Some(DocumentLayoutLine {
                line: Line::raw(""),
                plain_line: String::new(),
                anchor: document_anchor_for_transcript(LineAnchor {
                    item_index: position.gap_owner_item_index.unwrap_or(position.item_index),
                    item_anchor: ItemLineAnchor {
                        kind: LineAnchorKind::ItemGap,
                        gap_offset: relative,
                        ..ItemLineAnchor::default()
                    },
                }),
                selectable: SelectableLineRange::default(),
            });
        }

        let block = self.render_block(position.item_index, context)?;
        let block_index = relative - position.gap_before;
        let anchor = document_anchor_for_transcript(LineAnchor {
            item_index: position.item_index,
            item_anchor: block.anchor_at(block_index)?,
        });
        let plain_line = block.plain_line_at(block_index)?;
        let selectable = if include_selectable {
            self.selectable_at(anchor, &plain_line)
        } else {
            SelectableLineRange::default()
        };

        Some(DocumentLayoutLine {
            line: block.line_at(block_index)?,
            plain_line,
            anchor,
            selectable,
        })
    }

    fn viewport_line(
        &self,
        index: usize,
        include_test_plain_lines: bool,
        context: FrameRenderContext,
    ) -> Option<DocumentTranscriptViewportLine> {
        #[cfg(not(test))]
        let _ = include_test_plain_lines;

        let position = self.index.position_for_line(index)?;
        let relative = index.saturating_sub(position.start_line);
        if relative < position.gap_before {
            return Some(DocumentTranscriptViewportLine {
                line: Line::raw(""),
                is_assistant: false,
                plain_line_len: 0,
                #[cfg(test)]
                plain_line: String::new(),
            });
        }

        let block = self.render_block(position.item_index, context)?;
        let block_index = relative - position.gap_before;
        let is_assistant = self
            .items
            .get(position.item_index)
            .is_some_and(|item| item.as_ref().is_assistant_message());
        Some(DocumentTranscriptViewportLine {
            line: block.line_at(block_index)?,
            is_assistant,
            plain_line_len: block.plain_line_len(block_index)?,
            #[cfg(test)]
            plain_line: if include_test_plain_lines {
                block.plain_line_at(block_index)?
            } else {
                String::new()
            },
        })
    }

    pub(crate) fn line_index_for_anchor(
        &self,
        target: LineAnchor,
        context: FrameRenderContext,
    ) -> Option<usize> {
        let item_lines = self.index.item_lines(target.item_index)?;
        if matches!(target.item_anchor.kind, LineAnchorKind::ItemGap) {
            let gap_line_count = self.index.trailing_gap_line_count(target.item_index);
            if target.item_anchor.gap_offset >= gap_line_count {
                return None;
            }
            return Some(
                item_lines.content_start_line
                    + item_lines.content_line_count
                    + target.item_anchor.gap_offset,
            );
        }

        let block = self.render_block(target.item_index, context)?;
        let block_index = block.anchor_index(target.item_anchor)?;
        Some(item_lines.content_start_line + block_index)
    }

    fn range_snapshot(
        &self,
        start: usize,
        count: usize,
        include_test_plain_lines: bool,
        context: FrameRenderContext,
    ) -> DocumentTranscriptViewportSnapshot {
        if count == 0 || self.line_count() == 0 || start >= self.line_count() {
            return DocumentTranscriptViewportSnapshot::default();
        }

        self.prewarm_item_blocks_for_window(start, count, context);
        let end = (start + count).min(self.line_count());
        let mut lines = Vec::with_capacity(end - start);
        let mut assistant_lines = Vec::with_capacity(end - start);
        let mut plain_text_len = 0;
        #[cfg(test)]
        let mut plain_lines = Vec::with_capacity(end - start);

        for index in start..end {
            let Some(line) = self.viewport_line(index, include_test_plain_lines, context) else {
                continue;
            };
            if !lines.is_empty() {
                plain_text_len += 1;
            }
            plain_text_len += line.plain_line_len;
            assistant_lines.push(line.is_assistant);
            #[cfg(test)]
            if include_test_plain_lines {
                plain_lines.push(line.plain_line.clone());
            }
            lines.push(line.line);
        }

        DocumentTranscriptViewportSnapshot {
            lines,
            assistant_lines,
            plain_text_len,
            resolved_offset: start,
            #[cfg(test)]
            plain_lines,
        }
    }

    fn prewarm_item_blocks_for_window(
        &self,
        start: usize,
        count: usize,
        context: FrameRenderContext,
    ) {
        let overscan_lines = viewport_overscan_line_budget(count);
        let Some((start_position, end_position)) =
            self.index
                .summary_positions_for_line_window(start, count, overscan_lines)
        else {
            return;
        };

        let required_items = self.index.visible_items[start_position..=end_position]
            .iter()
            .map(|position| position.item_index)
            .collect::<Vec<_>>();
        self.item_block_cache
            .borrow_mut()
            .retain(|item_index, _| required_items.contains(item_index));

        for item_index in required_items {
            if self.item_block_cache.borrow().contains_key(&item_index) {
                continue;
            }

            let Some(block) = self.materialize_block(item_index, context) else {
                continue;
            };
            self.item_block_cache.borrow_mut().insert(item_index, block);
        }
    }

    fn render_block(
        &self,
        item_index: usize,
        context: FrameRenderContext,
    ) -> Option<Rc<CachedRenderBlock>> {
        if let Some(block) = self.item_block_cache.borrow().get(&item_index).cloned() {
            return Some(block);
        }

        if let Some(block) = self.warmed_block(item_index) {
            return Some(block);
        }

        let block = Rc::new(materialize_transcript_item_render_block(
            self.items.get(item_index)?.as_ref(),
            self.width.max(1),
            self.palette,
            context,
        ));
        self.item_block_cache
            .borrow_mut()
            .insert(item_index, Rc::clone(&block));
        Some(block)
    }

    fn materialize_block(
        &self,
        item_index: usize,
        context: FrameRenderContext,
    ) -> Option<Rc<CachedRenderBlock>> {
        if let Some(block) = self.warmed_block(item_index) {
            return Some(block);
        }

        Some(Rc::new(materialize_transcript_item_render_block(
            self.items.get(item_index)?.as_ref(),
            self.width.max(1),
            self.palette,
            context,
        )))
    }

    fn warmed_block(&self, item_index: usize) -> Option<Rc<CachedRenderBlock>> {
        let item = self.items.get(item_index)?.as_ref();
        if item.has_active_runtime_tool_activity() {
            return None;
        }
        let warmed_block = self
            .warmed_item_block_cache
            .borrow()
            .get(&item_index)
            .cloned();
        if let Some(block) = warmed_block
            && block.width == self.width.max(1)
            && block.palette == self.palette
            && block.cache_key == item.render_cache_key()
        {
            return Some(block);
        }

        None
    }

    fn selectable_at(&self, anchor: DocumentLineAnchor, plain_line: &str) -> SelectableLineRange {
        if anchor.region != DocumentAnchorRegion::Transcript
            || matches!(anchor.transcript.item_anchor.kind, LineAnchorKind::ItemGap)
        {
            return SelectableLineRange::default();
        }

        if let Some(item_ranges) = self.selectable_ranges_for_item(anchor.transcript.item_index)
            && anchor.transcript.item_anchor.rendered_line < item_ranges.len()
        {
            return item_ranges[anchor.transcript.item_anchor.rendered_line];
        }

        normalize_transcript_selectable_range(plain_line, usize::from(self.width.max(1)), true)
    }

    fn selectable_ranges_for_item(&self, item_index: usize) -> Option<Vec<SelectableLineRange>> {
        if let Some(ranges) = self.selectable_cache.borrow().get(&item_index).cloned() {
            return Some(ranges);
        }

        let plain_lines = self.item_text_lines(item_index)?;
        let item = self.items.get(item_index)?.as_ref();
        let ranges =
            item.render_selectable_line_ranges(self.width.max(1), self.palette, &plain_lines);
        self.selectable_cache
            .borrow_mut()
            .insert(item_index, ranges.clone());
        Some(ranges)
    }

    fn item_text_lines(&self, item_index: usize) -> Option<Vec<String>> {
        if let Some(lines) = self
            .item_text_lines_cache
            .borrow()
            .get(&item_index)
            .cloned()
        {
            return Some(lines);
        }

        let item = self.items.get(item_index)?.as_ref();
        let lines = item.render_plain_lines(self.width.max(1), self.palette);
        self.item_text_lines_cache
            .borrow_mut()
            .insert(item_index, lines.clone());
        Some(lines)
    }
}

impl DocumentLayout {
    /// `line_count` 返回 unified document 的总行数。
    pub(crate) fn line_count(&self) -> usize {
        self.transcript_line_count + self.tail.line_count()
    }

    /// `plain_text_len` 返回 unified document 纯文本的总字符数（含换行分隔）。
    pub(crate) fn plain_text_len(&self, context: FrameRenderContext) -> usize {
        self.plain_text_len_for_range(0, self.line_count(), context)
    }

    /// `plain_text_len_for_range` 返回指定连续范围内纯文本的总字符数（含换行分隔）。
    pub(crate) fn plain_text_len_for_range(
        &self,
        mut start: usize,
        count: usize,
        context: FrameRenderContext,
    ) -> usize {
        if count == 0 || self.line_count() == 0 || start >= self.line_count() {
            return 0;
        }

        let end = (start + count).min(self.line_count());
        let mut total = 0;
        let mut used_transcript = false;
        if start < self.transcript_line_count {
            let transcript_end = end.min(self.transcript_line_count);
            total +=
                self.transcript
                    .plain_text_len_for_range(start, transcript_end - start, context);
            used_transcript = transcript_end > start;
            start = transcript_end;
        }
        if start < end {
            let tail_start = start - self.transcript_line_count;
            let tail_end = end - self.transcript_line_count;
            if used_transcript {
                total += 1;
            }
            total += self
                .tail
                .plain_text_len_for_range(tail_start, tail_end - tail_start);
        }

        total
    }

    /// `line_at` 返回指定视觉行的统一只读视图。
    pub(crate) fn line_at(
        &self,
        index: usize,
        context: FrameRenderContext,
    ) -> Option<DocumentLayoutLine> {
        if index >= self.line_count() {
            return None;
        }
        if index < self.transcript_line_count {
            return self.transcript.line_at(index, context);
        }

        Some(DocumentLayoutLine {
            line: self
                .tail
                .line_at(index - self.transcript_line_count)
                .unwrap_or_default(),
            plain_line: self
                .tail
                .text_line_at(index - self.transcript_line_count)
                .unwrap_or_default(),
            anchor: self
                .tail
                .anchor_at(index - self.transcript_line_count)
                .unwrap_or_default(),
            selectable: self
                .tail
                .selectable_at(index - self.transcript_line_count)
                .unwrap_or_default(),
        })
    }

    /// `line_text_at` 返回指定视觉行的纯文本内容。
    pub(crate) fn line_text_at(&self, index: usize, context: FrameRenderContext) -> Option<String> {
        if index >= self.line_count() {
            return None;
        }
        if index < self.transcript_line_count {
            return self.transcript.plain_line_at(index, context);
        }

        self.tail.text_line_at(index - self.transcript_line_count)
    }

    /// `line_anchor_at` 返回指定视觉行的锚点。
    pub(crate) fn line_anchor_at(
        &self,
        index: usize,
        context: FrameRenderContext,
    ) -> Option<DocumentLineAnchor> {
        if index >= self.line_count() {
            return None;
        }
        if index < self.transcript_line_count {
            return self.transcript.anchor_at(index, context);
        }

        self.tail.anchor_at(index - self.transcript_line_count)
    }

    /// `is_assistant_message_line` 判断指定文档行是否属于 assistant 消息正文。
    pub(crate) fn is_assistant_message_line(
        &self,
        index: usize,
        context: FrameRenderContext,
    ) -> bool {
        let Some(anchor) = self.line_anchor_at(index, context) else {
            return false;
        };
        if anchor.region != DocumentAnchorRegion::Transcript
            || matches!(anchor.transcript.item_anchor.kind, LineAnchorKind::ItemGap)
        {
            return false;
        }

        self.transcript
            .items
            .get(anchor.transcript.item_index)
            .is_some_and(|item| item.as_ref().is_assistant_message())
    }

    /// `line_index_for_anchor` 把语义锚点解析回当前布局中的视觉行。
    pub(crate) fn line_index_for_anchor(
        &self,
        target: DocumentLineAnchor,
        context: FrameRenderContext,
    ) -> Option<usize> {
        if target.region == DocumentAnchorRegion::Transcript {
            return self
                .transcript
                .line_index_for_anchor(target.transcript, context);
        }

        self.tail
            .line_index_for_anchor(target)
            .map(|index| self.transcript_line_count + index)
    }

    /// `resolve_selection_point` 把语义选区端点投影回当前布局。
    pub(crate) fn resolve_selection_point(
        &self,
        point: SelectionPoint,
        context: FrameRenderContext,
    ) -> Option<ResolvedSelectionPoint> {
        Some(ResolvedSelectionPoint::new(
            self.line_index_for_anchor(point.anchor(), context)?,
            point.column(),
        ))
    }

    /// `selection_line_at` 返回 selection / copy 路径需要的文本与锚点信息。
    pub(crate) fn selection_line_at(
        &self,
        index: usize,
        context: FrameRenderContext,
    ) -> Option<DocumentSelectionLine> {
        let line = self.line_at(index, context)?;
        Some(DocumentSelectionLine {
            text: line.plain_line,
            anchor: line.anchor,
            selectable: line.selectable,
        })
    }

    /// `selection_line_for_anchor` 按语义锚点返回 selection 行。
    pub(crate) fn selection_line_for_anchor(
        &self,
        anchor: DocumentLineAnchor,
        context: FrameRenderContext,
    ) -> Option<DocumentSelectionLine> {
        let index = self.line_index_for_anchor(anchor, context)?;
        self.selection_line_at(index, context)
    }

    #[cfg(test)]
    /// `all_plain_lines` 返回 unified document 的完整纯文本行视图。
    pub(crate) fn all_plain_lines(&self, context: FrameRenderContext) -> Vec<String> {
        self.line_texts_for_range(0, self.line_count(), context)
    }

    #[cfg(test)]
    /// `all_line_anchors` 返回 unified document 的完整锚点视图。
    pub(crate) fn all_line_anchors(&self, context: FrameRenderContext) -> Vec<DocumentLineAnchor> {
        (0..self.line_count())
            .filter_map(|index| self.line_anchor_at(index, context))
            .collect()
    }

    #[cfg(test)]
    /// `line_texts_for_range` 返回给定连续范围内的纯文本行。
    pub(crate) fn line_texts_for_range(
        &self,
        mut start: usize,
        count: usize,
        context: FrameRenderContext,
    ) -> Vec<String> {
        if count == 0 || self.line_count() == 0 || start >= self.line_count() {
            return Vec::new();
        }

        let end = (start + count).min(self.line_count());
        let mut lines = Vec::with_capacity(end - start);
        if start < self.transcript_line_count {
            let transcript_end = end.min(self.transcript_line_count);
            lines.extend(self.transcript.plain_lines_for_range(
                start,
                transcript_end - start,
                context,
            ));
            start = transcript_end;
        }
        if start < end {
            let tail_start = start - self.transcript_line_count;
            let tail_end = end - self.transcript_line_count;
            lines.extend(
                self.tail
                    .text_lines_for_range(tail_start, tail_end - tail_start),
            );
        }

        lines
    }

    /// `lines_for_range` 返回给定连续范围内的带样式行。
    pub(crate) fn lines_for_range(
        &self,
        mut start: usize,
        count: usize,
        context: FrameRenderContext,
    ) -> Vec<Line<'static>> {
        if count == 0 || self.line_count() == 0 || start >= self.line_count() {
            return Vec::new();
        }

        let end = (start + count).min(self.line_count());
        let mut lines = Vec::with_capacity(end - start);
        if start < self.transcript_line_count {
            let transcript_end = end.min(self.transcript_line_count);
            lines.extend(
                self.transcript
                    .lines_for_range(start, transcript_end - start, context),
            );
            start = transcript_end;
        }
        if start < end {
            let tail_start = start - self.transcript_line_count;
            let tail_end = end - self.transcript_line_count;
            lines.extend(self.tail.lines_for_range(tail_start, tail_end - tail_start));
        }

        lines
    }

    /// `transcript_item_lines` 返回单个 transcript item 的内容行索引信息。
    pub(crate) fn transcript_item_lines(
        &self,
        item_index: usize,
    ) -> Option<DocumentTranscriptItemLines> {
        self.transcript
            .index
            .item_lines(item_index)
            .map(|item| DocumentTranscriptItemLines {
                content_start_line: item.content_start_line,
                content_line_count: item.content_line_count,
                total_line_count: item.total_line_count,
            })
            .filter(|item| item.content_line_count > 0)
    }
}

fn document_anchor_for_transcript(transcript: LineAnchor) -> DocumentLineAnchor {
    DocumentLineAnchor {
        region: DocumentAnchorRegion::Transcript,
        transcript,
        ..DocumentLineAnchor::default()
    }
}
