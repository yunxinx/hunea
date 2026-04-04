use std::collections::HashMap;

use crate::frontend::tui::{
    selection::{SelectableLineRange, normalize_transcript_selectable_range},
    transcript::LineAnchorKind,
};

use super::{
    DocumentAnchorRegion, DocumentLayout, DocumentLayoutLine, DocumentLineAnchor,
    DocumentTranscriptItemLines, DocumentTranscriptSnapshot,
};

impl DocumentLayout {
    /// `line_count` 返回 unified document 的总行数。
    pub(crate) fn line_count(&self) -> usize {
        self.transcript_line_count + self.tail_lines.len()
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
            total += plain_lines_len(&self.tail_plain_lines[tail_start..tail_end]);
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
                .tail_lines
                .get(index - self.transcript_line_count)
                .cloned()
                .unwrap_or_default(),
            plain_line: self
                .tail_plain_lines
                .get(index - self.transcript_line_count)
                .cloned()
                .unwrap_or_default(),
            anchor: self
                .tail_anchors
                .get(index - self.transcript_line_count)
                .copied()
                .unwrap_or_default(),
            selectable: self
                .tail_selectable
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
        if index >= self.line_count() {
            return None;
        }
        if index < self.transcript_line_count {
            return self.transcript.anchors.get(index).copied();
        }

        self.tail_anchors
            .get(index - self.transcript_line_count)
            .copied()
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
            lines.extend_from_slice(&self.tail_plain_lines[tail_start..tail_end]);
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
            let tail_start = start - self.transcript_line_count;
            let tail_end = end - self.transcript_line_count;
            lines.extend_from_slice(&self.tail_lines[tail_start..tail_end]);
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

        self.transcript.plain_lines[start..end.min(self.transcript_line_count)].to_vec()
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
