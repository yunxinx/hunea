use std::collections::HashMap;

use crate::frontend::tui::{
    selection::{SelectableLineRange, normalize_transcript_selectable_range},
    transcript::LineAnchorKind,
};

use super::{
    DocumentAnchorRegion, DocumentLayout, DocumentLayoutLine, DocumentLineAnchor,
    DocumentTranscriptItemLines, DocumentTranscriptSegment, DocumentTranscriptSnapshot,
};

impl DocumentLayout {
    /// `line_count` 返回 unified document 的总行数。
    pub(crate) fn line_count(&self) -> usize {
        self.lines.len()
    }

    /// `line_at` 返回指定视觉行的统一只读视图。
    pub(crate) fn line_at(&self, index: usize) -> Option<DocumentLayoutLine> {
        if index >= self.line_count() {
            return None;
        }
        if index < self.transcript_line_count {
            return self.transcript_line_at(index);
        }

        Some(DocumentLayoutLine {
            line: self.lines.get(index).cloned().unwrap_or_default(),
            plain_line: self.plain_lines.get(index).cloned().unwrap_or_default(),
            anchor: self.anchors.get(index).copied().unwrap_or_default(),
            selectable: self.selectable.get(index).copied().unwrap_or_default(),
        })
    }

    /// `line_text_at` 返回指定视觉行的纯文本内容。
    pub(crate) fn line_text_at(&self, index: usize) -> Option<String> {
        self.line_at(index).map(|line| line.plain_line)
    }

    /// `line_anchor_at` 返回指定视觉行的锚点。
    pub(crate) fn line_anchor_at(&self, index: usize) -> Option<DocumentLineAnchor> {
        if index >= self.line_count() {
            return None;
        }
        if index < self.transcript_line_count {
            return self.transcript.anchors.get(index).copied();
        }

        self.anchors.get(index).copied()
    }

    /// `all_plain_lines` 返回 unified document 的完整纯文本行视图。
    pub(crate) fn all_plain_lines(&self) -> Vec<String> {
        self.line_texts_for_range(0, self.line_count())
    }

    #[cfg(test)]
    /// `all_line_anchors` 返回 unified document 的完整锚点视图。
    pub(crate) fn all_line_anchors(&self) -> Vec<DocumentLineAnchor> {
        (0..self.line_count())
            .filter_map(|index| self.line_anchor_at(index))
            .collect()
    }

    /// `line_texts_for_range` 返回给定连续范围内的纯文本行。
    pub(crate) fn line_texts_for_range(&self, mut start: usize, count: usize) -> Vec<String> {
        if count == 0 || self.line_count() == 0 || start >= self.line_count() {
            return Vec::new();
        }

        let end = (start + count).min(self.line_count());
        let mut lines = Vec::with_capacity(end - start);
        if start < self.transcript_line_count {
            let transcript_end = end.min(self.transcript_line_count);
            lines.extend(self.transcript_line_texts_for_range(start, transcript_end));
            start = transcript_end;
        }
        if start < end {
            lines.extend_from_slice(&self.plain_lines[start..end]);
        }

        lines
    }

    /// `lines_for_range` 返回给定连续范围内的带样式行。
    pub(crate) fn lines_for_range(
        &self,
        mut start: usize,
        count: usize,
    ) -> Vec<ratatui::text::Line<'static>> {
        if count == 0 || self.line_count() == 0 || start >= self.line_count() {
            return Vec::new();
        }

        let end = (start + count).min(self.line_count());
        let mut lines = Vec::with_capacity(end - start);
        if start < self.transcript_line_count {
            let transcript_end = end.min(self.transcript_line_count);
            lines.extend(self.transcript_lines_for_range(start, transcript_end));
            start = transcript_end;
        }
        if start < end {
            lines.extend_from_slice(&self.lines[start..end]);
        }

        lines
    }

    /// `transcript_item_lines` 返回单个 transcript item 的内容行索引信息。
    pub(crate) fn transcript_item_lines(
        &self,
        item_index: usize,
    ) -> Option<DocumentTranscriptItemLines> {
        self.transcript_items
            .get(&item_index)
            .copied()
            .filter(|item| item.content_line_count > 0)
    }

    fn transcript_line_at(&self, index: usize) -> Option<DocumentLayoutLine> {
        let line = self.transcript.lines.get(index).cloned()?;
        let plain_line = self.transcript.plain_lines.get(index).cloned()?;
        let anchor = self.transcript.anchors.get(index).copied()?;
        let selectable = self.transcript_selectable_at(index, &plain_line, anchor);

        Some(DocumentLayoutLine {
            line,
            plain_line,
            anchor,
            selectable,
        })
    }

    fn transcript_selectable_at(
        &self,
        _index: usize,
        plain_line: &str,
        anchor: DocumentLineAnchor,
    ) -> SelectableLineRange {
        if anchor.region != DocumentAnchorRegion::Transcript
            || matches!(anchor.transcript.item_anchor.kind, LineAnchorKind::ItemGap)
        {
            return SelectableLineRange::default();
        }

        if let Some(item_ranges) =
            self.transcript_selectable_ranges_for_item(anchor.transcript.item_index)
            && anchor.transcript.item_anchor.rendered_line < item_ranges.len()
        {
            return item_ranges[anchor.transcript.item_anchor.rendered_line];
        }

        normalize_transcript_selectable_range(
            plain_line,
            usize::from(self.transcript.width.max(1)),
            true,
        )
    }

    fn transcript_selectable_ranges_for_item(
        &self,
        item_index: usize,
    ) -> Option<Vec<SelectableLineRange>> {
        if let Some(ranges) = self
            .transcript
            .selectable_cache
            .borrow()
            .get(&item_index)
            .cloned()
        {
            return Some(ranges);
        }

        let item_lines = self.transcript_item_lines(item_index)?;
        let end = item_lines.content_start_line + item_lines.content_line_count;
        let plain_lines = self.transcript_line_texts_for_range(item_lines.content_start_line, end);
        let item = self.transcript.items.get(&item_index)?;
        let ranges = item.render_selectable_line_ranges(
            self.transcript.width.max(1),
            self.transcript.palette,
            &plain_lines,
        );
        self.transcript
            .selectable_cache
            .borrow_mut()
            .insert(item_index, ranges.clone());
        Some(ranges)
    }

    fn transcript_line_texts_for_range(&self, start: usize, end: usize) -> Vec<String> {
        if start >= end || start >= self.transcript_line_count {
            return Vec::new();
        }

        if self.transcript_segments.is_empty() {
            return self.transcript.plain_lines[start..end.min(self.transcript_line_count)]
                .to_vec();
        }

        let end = end.min(self.transcript_line_count);
        let mut lines = Vec::with_capacity(end - start);
        let mut current = start;

        let mut segment_index = self
            .transcript_segments
            .partition_point(|segment| segment.start_line + segment.line_count <= start);
        while segment_index < self.transcript_segments.len() && current < end {
            let segment = &self.transcript_segments[segment_index];
            if current < segment.start_line {
                let fallback_end = end.min(segment.start_line);
                lines.extend_from_slice(&self.transcript.plain_lines[current..fallback_end]);
                current = fallback_end;
                if current >= end {
                    break;
                }
            }

            let local_start = current.saturating_sub(segment.start_line);
            let local_end = segment
                .line_count
                .min(end.saturating_sub(segment.start_line));
            if local_start < local_end && local_end <= segment.plain_lines.len() {
                lines.extend_from_slice(&segment.plain_lines[local_start..local_end]);
                current = segment.start_line + local_end;
            } else {
                let fallback_end = end.min(segment.start_line + segment.line_count);
                lines.extend_from_slice(&self.transcript.plain_lines[current..fallback_end]);
                current = fallback_end;
            }
            segment_index += 1;
        }

        if current < end {
            lines.extend_from_slice(&self.transcript.plain_lines[current..end]);
        }

        lines
    }

    fn transcript_lines_for_range(
        &self,
        start: usize,
        end: usize,
    ) -> Vec<ratatui::text::Line<'static>> {
        if start >= end || start >= self.transcript_line_count {
            return Vec::new();
        }

        if self.transcript_segments.is_empty() {
            return self.transcript.lines[start..end.min(self.transcript_line_count)].to_vec();
        }

        let end = end.min(self.transcript_line_count);
        let mut lines = Vec::with_capacity(end - start);
        let mut current = start;

        let mut segment_index = self
            .transcript_segments
            .partition_point(|segment| segment.start_line + segment.line_count <= start);
        while segment_index < self.transcript_segments.len() && current < end {
            let segment = &self.transcript_segments[segment_index];
            if current < segment.start_line {
                let fallback_end = end.min(segment.start_line);
                lines.extend_from_slice(&self.transcript.lines[current..fallback_end]);
                current = fallback_end;
                if current >= end {
                    break;
                }
            }

            let local_start = current.saturating_sub(segment.start_line);
            let local_end = segment
                .line_count
                .min(end.saturating_sub(segment.start_line));
            if local_start < local_end && local_end <= segment.lines.len() {
                lines.extend_from_slice(&segment.lines[local_start..local_end]);
                current = segment.start_line + local_end;
            } else {
                let fallback_end = end.min(segment.start_line + segment.line_count);
                lines.extend_from_slice(&self.transcript.lines[current..fallback_end]);
                current = fallback_end;
            }
            segment_index += 1;
        }

        if current < end {
            lines.extend_from_slice(&self.transcript.lines[current..end]);
        }

        lines
    }
}

/// `new_document_transcript_index` 为 transcript snapshot 构建 segment 和 item 行索引。
pub(crate) fn new_document_transcript_index(
    snapshot: &DocumentTranscriptSnapshot,
) -> (
    Vec<DocumentTranscriptSegment>,
    HashMap<usize, DocumentTranscriptItemLines>,
) {
    if snapshot.lines.is_empty() || snapshot.anchors.is_empty() {
        return (Vec::new(), HashMap::new());
    }

    let mut segments = Vec::new();
    let mut items = HashMap::new();
    let mut start = 0;
    let mut current_item_index = snapshot.anchors[0].transcript.item_index;
    let mut content_line_count = usize::from(!matches!(
        snapshot.anchors[0].transcript.item_anchor.kind,
        LineAnchorKind::ItemGap
    ));

    for index in 1..snapshot.anchors.len() {
        let anchor = snapshot.anchors[index];
        if anchor.transcript.item_index != current_item_index {
            let line_count = index - start;
            segments.push(DocumentTranscriptSegment {
                start_line: start,
                line_count,
                lines: snapshot.lines[start..index].to_vec(),
                plain_lines: snapshot.plain_lines[start..index].to_vec(),
            });
            items.insert(
                current_item_index,
                DocumentTranscriptItemLines {
                    content_start_line: start,
                    content_line_count,
                    total_line_count: line_count,
                },
            );
            start = index;
            current_item_index = anchor.transcript.item_index;
            content_line_count = 0;
        }
        if !matches!(anchor.transcript.item_anchor.kind, LineAnchorKind::ItemGap) {
            content_line_count += 1;
        }
    }

    let line_count = snapshot.lines.len() - start;
    segments.push(DocumentTranscriptSegment {
        start_line: start,
        line_count,
        lines: snapshot.lines[start..].to_vec(),
        plain_lines: snapshot.plain_lines[start..].to_vec(),
    });
    items.insert(
        current_item_index,
        DocumentTranscriptItemLines {
            content_start_line: start,
            content_line_count,
            total_line_count: line_count,
        },
    );

    (segments, items)
}
