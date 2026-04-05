use std::collections::HashMap;

use crate::frontend::tui::{
    selection::{
        ResolvedSelectionPoint, SelectableLineRange, SelectionPoint,
        normalize_transcript_selectable_range,
    },
    transcript::LineAnchorKind,
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

impl DocumentLayout {
    /// `line_count` 返回 unified document 的总行数。
    pub(crate) fn line_count(&self) -> usize {
        self.transcript_line_count + self.tail.lines.len()
    }

    /// `plain_text_len` 返回 unified document 纯文本的总字符数（含换行分隔）。
    pub(crate) fn plain_text_len(&self) -> usize {
        self.plain_text_len_for_range(0, self.line_count())
    }

    /// `plain_text_len_for_range` 返回指定连续范围内纯文本的总字符数（含换行分隔）。
    pub(crate) fn plain_text_len_for_range(&self, mut start: usize, count: usize) -> usize {
        if count == 0 || self.line_count() == 0 || start >= self.line_count() {
            return 0;
        }

        let end = (start + count).min(self.line_count());
        let mut total = 0;
        let mut used_transcript = false;
        if start < self.transcript_line_count {
            let transcript_end = end.min(self.transcript_line_count);
            total += plain_lines_len(&self.transcript.plain_lines[start..transcript_end]);
            used_transcript = transcript_end > start;
            start = transcript_end;
        }
        if start < end {
            let tail_start = start - self.transcript_line_count;
            let tail_end = end - self.transcript_line_count;
            if used_transcript {
                total += 1;
            }
            total += plain_lines_len(&self.tail.text_lines[tail_start..tail_end]);
        }

        total
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
            line: self
                .tail
                .lines
                .get(index - self.transcript_line_count)
                .cloned()
                .unwrap_or_default(),
            plain_line: self
                .tail
                .text_lines
                .get(index - self.transcript_line_count)
                .cloned()
                .unwrap_or_default(),
            anchor: self
                .tail
                .anchors
                .get(index - self.transcript_line_count)
                .copied()
                .unwrap_or_default(),
            selectable: self
                .tail
                .selectable
                .get(index - self.transcript_line_count)
                .copied()
                .unwrap_or_default(),
        })
    }

    /// `line_text_at` 返回指定视觉行的纯文本内容。
    pub(crate) fn line_text_at(&self, index: usize) -> Option<String> {
        self.line_at(index).map(|line| line.plain_line)
    }

    /// `line_anchor_at` 返回指定视觉行的锚点。
    pub(crate) fn line_anchor_at(&self, index: usize) -> Option<DocumentLineAnchor> {
        self.selection_line_at(index).map(|line| line.anchor)
    }

    /// `line_index_for_anchor` 把语义锚点解析回当前布局中的视觉行。
    pub(crate) fn line_index_for_anchor(&self, target: DocumentLineAnchor) -> Option<usize> {
        if target.region == DocumentAnchorRegion::Transcript {
            return self
                .transcript
                .anchors
                .iter()
                .position(|anchor| *anchor == target);
        }

        self.tail
            .anchors
            .iter()
            .position(|anchor| *anchor == target)
            .map(|index| self.transcript_line_count + index)
    }

    /// `resolve_selection_point` 把语义选区端点投影回当前布局。
    pub(crate) fn resolve_selection_point(
        &self,
        point: SelectionPoint,
    ) -> Option<ResolvedSelectionPoint> {
        Some(ResolvedSelectionPoint::new(
            self.line_index_for_anchor(point.anchor())?,
            point.column(),
        ))
    }

    /// `selection_line_at` 返回 selection / copy 路径需要的文本与锚点信息。
    pub(crate) fn selection_line_at(&self, index: usize) -> Option<DocumentSelectionLine> {
        if index >= self.line_count() {
            return None;
        }
        if index < self.transcript_line_count {
            return self.transcript_selection_line_at(index);
        }

        let tail_index = index - self.transcript_line_count;
        Some(DocumentSelectionLine {
            text: self
                .tail
                .text_lines
                .get(tail_index)
                .cloned()
                .unwrap_or_default(),
            anchor: self
                .tail
                .anchors
                .get(tail_index)
                .copied()
                .unwrap_or_default(),
            selectable: self
                .tail
                .selectable
                .get(tail_index)
                .copied()
                .unwrap_or_default(),
        })
    }

    /// `selection_line_for_anchor` 按语义锚点返回 selection 行。
    pub(crate) fn selection_line_for_anchor(
        &self,
        anchor: DocumentLineAnchor,
    ) -> Option<DocumentSelectionLine> {
        let index = self.line_index_for_anchor(anchor)?;
        self.selection_line_at(index)
    }

    #[cfg(test)]
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

    #[cfg(test)]
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
            let tail_start = start - self.transcript_line_count;
            let tail_end = end - self.transcript_line_count;
            lines.extend_from_slice(&self.tail.text_lines[tail_start..tail_end]);
        }

        lines
    }

    #[cfg(test)]
    fn transcript_line_texts_for_range(&self, start: usize, end: usize) -> Vec<String> {
        if start >= end || start >= self.transcript_line_count {
            return Vec::new();
        }

        (start..end.min(self.transcript_line_count))
            .filter_map(|index| {
                let anchor = self.transcript.anchors.get(index).copied()?;
                self.transcript_line_text_at(index, anchor)
            })
            .collect()
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
            let tail_start = start - self.transcript_line_count;
            let tail_end = end - self.transcript_line_count;
            lines.extend_from_slice(&self.tail.lines[tail_start..tail_end]);
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
        let anchor = self.transcript.anchors.get(index).copied()?;
        let plain_line = self.transcript_line_text_at(index, anchor)?;
        let selectable = self.transcript_selectable_at(anchor, &plain_line);

        Some(DocumentLayoutLine {
            line,
            plain_line,
            anchor,
            selectable,
        })
    }

    fn transcript_selection_line_at(&self, index: usize) -> Option<DocumentSelectionLine> {
        let anchor = self.transcript.anchors.get(index).copied()?;
        let text = self.transcript_line_text_at(index, anchor)?;
        let selectable = self.transcript_selectable_at(anchor, &text);

        Some(DocumentSelectionLine {
            text,
            anchor,
            selectable,
        })
    }

    fn transcript_selectable_at(
        &self,
        anchor: DocumentLineAnchor,
        plain_line: &str,
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

    fn transcript_line_text_at(&self, index: usize, anchor: DocumentLineAnchor) -> Option<String> {
        if matches!(anchor.transcript.item_anchor.kind, LineAnchorKind::ItemGap) {
            return Some(String::new());
        }

        if let Some(lines) = self.transcript_item_text_lines(anchor.transcript.item_index)
            && anchor.transcript.item_anchor.rendered_line < lines.len()
        {
            return Some(lines[anchor.transcript.item_anchor.rendered_line].clone());
        }

        self.transcript.plain_lines.get(index).cloned()
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

        let plain_lines = self.transcript_item_text_lines(item_index)?;
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

    fn transcript_item_text_lines(&self, item_index: usize) -> Option<Vec<String>> {
        if let Some(lines) = self
            .transcript
            .item_text_lines_cache
            .borrow()
            .get(&item_index)
            .cloned()
        {
            return Some(lines);
        }

        let item = self.transcript.items.get(&item_index)?;
        let lines = item.render_plain_lines(self.transcript.width.max(1), self.transcript.palette);
        self.transcript
            .item_text_lines_cache
            .borrow_mut()
            .insert(item_index, lines.clone());
        Some(lines)
    }

    fn transcript_lines_for_range(
        &self,
        start: usize,
        end: usize,
    ) -> Vec<ratatui::text::Line<'static>> {
        if start >= end || start >= self.transcript_line_count {
            return Vec::new();
        }

        self.transcript.lines[start..end.min(self.transcript_line_count)].to_vec()
    }
}

fn plain_lines_len(lines: &[String]) -> usize {
    if lines.is_empty() {
        return 0;
    }

    lines.iter().map(String::len).sum::<usize>() + lines.len().saturating_sub(1)
}

/// `new_document_transcript_item_index` 为 transcript snapshot 构建 item 行索引。
pub(crate) fn new_document_transcript_item_index(
    snapshot: &DocumentTranscriptSnapshot,
) -> HashMap<usize, DocumentTranscriptItemLines> {
    if snapshot.lines.is_empty() || snapshot.anchors.is_empty() {
        return HashMap::new();
    }

    let mut items = HashMap::with_capacity(snapshot.items.len().max(1));
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
    items.insert(
        current_item_index,
        DocumentTranscriptItemLines {
            content_start_line: start,
            content_line_count,
            total_line_count: line_count,
        },
    );

    items
}
