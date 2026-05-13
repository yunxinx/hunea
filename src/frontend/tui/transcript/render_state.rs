#[cfg(test)]
use std::cmp::Ordering;
use std::rc::Rc;

use crate::frontend::tui::selection::SelectableLineRange;

use super::{TranscriptItemMetricsIndex, cache::CachedRenderBlock};

#[cfg(test)]
use super::cache::CachedLineAnchors;
#[cfg(test)]
use ratatui::text::Line;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum LineAnchorKind {
    #[default]
    RenderedLine,
    LogicalPosition,
    ItemGap,
}

/// `ItemLineAnchor` 描述单个 transcript item 内一条视觉行的语义位置。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct ItemLineAnchor {
    pub(crate) kind: LineAnchorKind,
    pub(crate) logical_line: usize,
    pub(crate) range_start: usize,
    pub(crate) range_end: usize,
    pub(crate) rendered_line: usize,
    pub(crate) gap_offset: usize,
}

/// `LineAnchor` 把 item 内锚点投影到 transcript 的最终行坐标。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct LineAnchor {
    pub(crate) item_index: usize,
    pub(crate) item_anchor: ItemLineAnchor,
}

/// `RenderResult` 表示 transcript 在当前宽度下的稳定渲染结果。
/// steady-state document 主路径通常只保留 index-only 版本；带 item block 的完整结果
/// 只在显式全量 materialization、benchmark 或冷兜底路径里构造。
#[derive(Debug, Clone)]
pub(crate) struct RenderResult {
    pub(crate) index: TranscriptItemMetricsIndex,
    pub(crate) items: Rc<Vec<RenderItemSummary>>,
    #[cfg(test)]
    pub(crate) item_positions: Rc<Vec<usize>>,
    pub(crate) selectable_ranges: Rc<Vec<SelectableLineRange>>,
    pub(crate) line_count: usize,
    pub(crate) content_char_len: usize,
    /// `append_start_line` 标记这次结果是否由尾部追加快路径扩展而来。
    /// `-1` 表示这次是完整重组；非负值表示旧结果的行数。
    pub(crate) append_start_line: isize,
}

/// `RenderItemSummary` 保存单个可见 transcript item 的行级摘要。
#[derive(Debug, Clone)]
pub(crate) struct RenderItemSummary {
    pub(crate) item_index: usize,
    pub(crate) start_line: usize,
    pub(crate) gap_before: usize,
    pub(crate) content_line_count: usize,
    pub(crate) total_line_count: usize,
    pub(crate) gap_owner_item_index: Option<usize>,
    pub(crate) block: Rc<CachedRenderBlock>,
}

/// `RenderItemLines` 表示单个 transcript item 在最终文档中的内容与总占位信息。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct RenderItemLines {
    pub(crate) content_start_line: usize,
    pub(crate) content_line_count: usize,
    pub(crate) total_line_count: usize,
}

/// `RenderedTranscriptLine` 表示按需物化出的一条 transcript 行。
#[cfg(test)]
#[derive(Debug, Clone)]
pub(crate) struct RenderedTranscriptLine {
    pub(crate) plain_line: String,
    pub(crate) anchor: LineAnchor,
}

/// `ViewportRenderResult` 表示 transcript 在给定 viewport 下的可视切片。
#[cfg(test)]
#[derive(Debug, Clone, Default)]
pub(crate) struct ViewportRenderResult {
    pub(crate) plain_lines: Vec<String>,
}

/// `RenderRangeSlice` 表示 transcript 某个连续区间的物化结果。
#[cfg(test)]
#[derive(Debug, Clone, Default)]
pub(crate) struct RenderRangeSlice {
    pub(crate) lines: Vec<Line<'static>>,
    #[cfg(test)]
    pub(crate) plain_lines: Vec<String>,
}

impl RenderResult {
    pub(crate) fn plain_text_len(&self) -> usize {
        if self.line_count == 0 {
            return 0;
        }

        self.content_char_len + self.line_count.saturating_sub(1)
    }

    #[cfg(test)]
    pub(crate) fn lines_for_range(&self, start: usize, count: usize) -> Vec<Line<'static>> {
        self.range_slice(start, count).lines
    }

    #[cfg(test)]
    pub(crate) fn plain_lines_for_range(&self, start: usize, count: usize) -> Vec<String> {
        self.range_slice(start, count).plain_lines
    }

    #[cfg(test)]
    pub(crate) fn item_lines(&self, item_index: usize) -> Option<RenderItemLines> {
        self.index.item_lines(item_index)
    }

    #[cfg(test)]
    pub(crate) fn line_at(&self, index: usize) -> Option<RenderedTranscriptLine> {
        if index >= self.line_count {
            return None;
        }

        let summary = self.summary_for_line(index)?;
        let relative = index.saturating_sub(summary.start_line);
        if relative < summary.gap_before {
            return Some(RenderedTranscriptLine {
                plain_line: String::new(),
                anchor: LineAnchor {
                    item_index: summary.gap_owner_item_index.unwrap_or(summary.item_index),
                    item_anchor: ItemLineAnchor {
                        kind: LineAnchorKind::ItemGap,
                        gap_offset: relative,
                        ..ItemLineAnchor::default()
                    },
                },
            });
        }

        let block_index = relative - summary.gap_before;
        Some(RenderedTranscriptLine {
            plain_line: summary.block.plain_line_at(block_index)?,
            anchor: LineAnchor {
                item_index: summary.item_index,
                item_anchor: summary.block.anchor_at(block_index)?,
            },
        })
    }

    #[cfg(test)]
    pub(crate) fn line_index_for_anchor(&self, target: LineAnchor) -> Option<usize> {
        if matches!(target.item_anchor.kind, LineAnchorKind::ItemGap) {
            let position = self.summary_position_for_item(target.item_index)?;
            let summary = self.items.get(position)?;
            let gap_line_count = self.trailing_gap_line_count(position);
            if target.item_anchor.gap_offset >= gap_line_count {
                return None;
            }
            return Some(
                summary.start_line
                    + summary.gap_before
                    + summary.content_line_count
                    + target.item_anchor.gap_offset,
            );
        }

        let summary = self.item_summary(target.item_index)?;
        let block_index = self.block_index_for_anchor(summary, target.item_anchor)?;
        Some(summary.start_line + summary.gap_before + block_index)
    }

    pub(crate) fn anchor_count(&self) -> usize {
        self.line_count
    }

    #[cfg(test)]
    pub(crate) fn all_plain_lines(&self) -> Vec<String> {
        self.plain_lines_for_range(0, self.line_count)
    }

    #[cfg(test)]
    pub(crate) fn all_line_anchors(&self) -> Vec<LineAnchor> {
        (0..self.line_count)
            .filter_map(|index| self.line_at(index).map(|line| line.anchor))
            .collect()
    }

    #[cfg(test)]
    pub(crate) fn range_slice(&self, start: usize, count: usize) -> RenderRangeSlice {
        if count == 0 || self.line_count == 0 || start >= self.line_count {
            return RenderRangeSlice::default();
        }

        let end = (start + count).min(self.line_count);
        let mut remaining = end - start;
        let mut slice = RenderRangeSlice {
            lines: Vec::with_capacity(remaining),
            #[cfg(test)]
            plain_lines: Vec::with_capacity(remaining),
        };
        let mut position = match self.summary_position_for_line(start) {
            Some(position) => position,
            None => return RenderRangeSlice::default(),
        };
        let mut line_offset = start.saturating_sub(self.items[position].start_line);

        while remaining > 0 {
            let summary = &self.items[position];
            let taken = remaining.min(summary.total_line_count.saturating_sub(line_offset));
            let gap_start = line_offset.min(summary.gap_before);
            let gap_end = (line_offset + taken).min(summary.gap_before);
            for _ in gap_start..gap_end {
                slice.lines.push(Line::raw(""));
                #[cfg(test)]
                slice.plain_lines.push(String::new());
            }

            let block_start = line_offset.saturating_sub(summary.gap_before);
            let block_end = (line_offset + taken)
                .saturating_sub(summary.gap_before)
                .min(summary.content_line_count);
            if block_start < block_end {
                summary
                    .block
                    .extend_lines(&mut slice.lines, block_start, block_end);
                #[cfg(test)]
                for index in block_start..block_end {
                    if let Some(plain_line) = summary.block.plain_line_at(index) {
                        slice.plain_lines.push(plain_line);
                    }
                }
            }

            remaining -= taken;
            position += 1;
            line_offset = 0;
        }

        slice
    }

    #[cfg(test)]
    fn summary_for_line(&self, index: usize) -> Option<&RenderItemSummary> {
        let position = self.summary_position_for_line(index)?;
        self.items.get(position)
    }

    #[cfg(test)]
    fn item_summary(&self, item_index: usize) -> Option<&RenderItemSummary> {
        let position = self.summary_position_for_item(item_index)?;
        self.items.get(position)
    }

    #[cfg(test)]
    fn summary_position_for_line(&self, index: usize) -> Option<usize> {
        self.items
            .binary_search_by(|summary| {
                if index < summary.start_line {
                    Ordering::Greater
                } else if index >= summary.start_line + summary.total_line_count {
                    Ordering::Less
                } else {
                    Ordering::Equal
                }
            })
            .ok()
    }

    #[cfg(test)]
    fn summary_position_for_item(&self, item_index: usize) -> Option<usize> {
        let position = *self.item_positions.get(item_index)?;
        (position != usize::MAX).then_some(position)
    }

    #[cfg(test)]
    fn trailing_gap_line_count(&self, position: usize) -> usize {
        let Some(summary) = self.items.get(position) else {
            return 0;
        };
        self.items
            .get(position + 1)
            .filter(|next| next.gap_owner_item_index == Some(summary.item_index))
            .map(|next| next.gap_before)
            .unwrap_or(0)
    }

    #[cfg(test)]
    fn block_index_for_anchor(
        &self,
        summary: &RenderItemSummary,
        anchor: ItemLineAnchor,
    ) -> Option<usize> {
        summary.block.anchor_index(anchor)
    }
}

impl Default for RenderResult {
    fn default() -> Self {
        Self {
            index: TranscriptItemMetricsIndex::default(),
            items: Rc::new(Vec::new()),
            #[cfg(test)]
            item_positions: Rc::new(Vec::new()),
            selectable_ranges: Rc::new(Vec::new()),
            line_count: 0,
            content_char_len: 0,
            append_start_line: -1,
        }
    }
}

#[cfg(test)]
pub(crate) fn new_render_result(items: Vec<RenderItemSummary>) -> RenderResult {
    new_render_result_with_append_start(items, TranscriptItemMetricsIndex::default(), -1)
}

pub(crate) fn index_only_render_result(index: TranscriptItemMetricsIndex) -> RenderResult {
    RenderResult {
        line_count: index.line_count,
        content_char_len: index.content_char_len,
        #[cfg(test)]
        item_positions: Rc::clone(&index.visible_positions),
        index,
        items: Rc::new(Vec::new()),
        selectable_ranges: Rc::new(Vec::new()),
        append_start_line: -1,
    }
}

pub(crate) fn new_render_result_with_append_start(
    items: Vec<RenderItemSummary>,
    mut index: TranscriptItemMetricsIndex,
    append_start_line: isize,
) -> RenderResult {
    if items.is_empty() {
        return RenderResult {
            index,
            ..RenderResult::default()
        };
    }

    if index.visible_positions.is_empty() {
        index = synthesize_metrics_index_from_render_items(&items);
    }

    #[cfg(test)]
    let item_positions = if index.visible_positions.is_empty() {
        let item_position_len = items
            .last()
            .map(|item| item.item_index.saturating_add(1))
            .unwrap_or(0);
        let mut item_positions = vec![usize::MAX; item_position_len];
        for (summary_position, item) in items.iter().enumerate() {
            item_positions[item.item_index] = summary_position;
        }
        Rc::new(item_positions)
    } else {
        Rc::clone(&index.visible_positions)
    };
    if index.line_count == 0 {
        return RenderResult {
            index,
            ..RenderResult::default()
        };
    }

    RenderResult {
        line_count: index.line_count,
        content_char_len: index.content_char_len,
        index,
        items: Rc::new(items),
        #[cfg(test)]
        item_positions,
        selectable_ranges: Rc::new(Vec::new()),
        append_start_line,
    }
}

fn synthesize_metrics_index_from_render_items(
    items: &[RenderItemSummary],
) -> TranscriptItemMetricsIndex {
    if items.is_empty() {
        return TranscriptItemMetricsIndex::default();
    }

    let item_count = items
        .last()
        .map(|item| item.item_index.saturating_add(1))
        .unwrap_or(0);
    let mut metrics = vec![super::TranscriptItemMetrics::default(); item_count];
    let mut visible_positions = vec![usize::MAX; item_count];
    let mut visible_items = Vec::with_capacity(items.len());
    let mut content_prefix_sums = vec![0_usize; item_count.saturating_add(1)];
    let mut summaries = items.iter().peekable();

    for item_index in 0..item_count {
        if summaries
            .peek()
            .is_some_and(|summary| summary.item_index == item_index)
        {
            let summary = summaries.next().expect("peeked summary should exist");
            metrics[item_index] = super::TranscriptItemMetrics {
                item_index,
                content_line_count: summary.content_line_count,
                content_char_len: summary.block.plain_text_char_len,
                quality: super::TranscriptItemMetricsQuality::Exact,
                is_valid: true,
                ..super::TranscriptItemMetrics::default()
            };
            visible_positions[item_index] = visible_items.len();
            visible_items.push(super::TranscriptItemPosition {
                item_index,
                start_line: summary.start_line,
                gap_before: summary.gap_before,
                content_line_count: summary.content_line_count,
                total_line_count: summary.total_line_count,
                content_char_len: summary.block.plain_text_char_len,
                gap_owner_item_index: summary.gap_owner_item_index,
            });
        }

        content_prefix_sums[item_index + 1] =
            content_prefix_sums[item_index].saturating_add(metrics[item_index].content_char_len);
    }

    TranscriptItemMetricsIndex {
        metrics: Rc::new(metrics),
        visible_items: Rc::new(visible_items),
        visible_positions: Rc::new(visible_positions),
        content_prefix_sums: Rc::new(content_prefix_sums),
        line_count: items
            .last()
            .map(|item| item.start_line + item.total_line_count)
            .unwrap_or(0),
        content_char_len: items
            .iter()
            .map(|item| item.block.plain_text_char_len)
            .sum(),
        ..TranscriptItemMetricsIndex::default()
    }
}

#[cfg(test)]
mod tests {
    use std::rc::Rc;

    use ratatui::text::Line;

    use super::*;

    fn render_block(
        text: &str,
        anchors: Vec<ItemLineAnchor>,
        plain_text_char_len: usize,
    ) -> Rc<CachedRenderBlock> {
        Rc::new(CachedRenderBlock {
            cache_key: 0,
            width: 80,
            palette: crate::frontend::tui::theme::default_palette(),
            lines: Rc::new(vec![Line::raw(text.to_string()); anchors.len()]),
            projected_user: None,
            projected_assistant: None,
            line_count: anchors.len(),
            plain_line_byte_lens: Rc::new(vec![text.len(); anchors.len()]),
            anchors: CachedLineAnchors::Explicit(Rc::new(anchors)),
            plain_text_char_len,
        })
    }

    #[test]
    fn item_lines_include_trailing_gap_on_gap_owner_item() {
        let render = new_render_result(vec![
            RenderItemSummary {
                item_index: 0,
                start_line: 0,
                gap_before: 0,
                content_line_count: 1,
                total_line_count: 1,
                gap_owner_item_index: None,
                block: render_block(
                    "first",
                    vec![ItemLineAnchor {
                        kind: LineAnchorKind::RenderedLine,
                        rendered_line: 0,
                        ..ItemLineAnchor::default()
                    }],
                    5,
                ),
            },
            RenderItemSummary {
                item_index: 1,
                start_line: 1,
                gap_before: 2,
                content_line_count: 1,
                total_line_count: 3,
                gap_owner_item_index: Some(0),
                block: render_block(
                    "second",
                    vec![ItemLineAnchor {
                        kind: LineAnchorKind::RenderedLine,
                        rendered_line: 0,
                        ..ItemLineAnchor::default()
                    }],
                    6,
                ),
            },
        ]);

        assert_eq!(
            render.item_lines(0),
            Some(RenderItemLines {
                content_start_line: 0,
                content_line_count: 1,
                total_line_count: 3,
            })
        );
        assert_eq!(
            render.item_lines(1),
            Some(RenderItemLines {
                content_start_line: 3,
                content_line_count: 1,
                total_line_count: 1,
            })
        );
    }

    #[test]
    fn line_index_for_anchor_resolves_later_gap_anchor_to_separator_line() {
        let render = new_render_result(vec![
            RenderItemSummary {
                item_index: 0,
                start_line: 0,
                gap_before: 0,
                content_line_count: 1,
                total_line_count: 1,
                gap_owner_item_index: None,
                block: render_block(
                    "first",
                    vec![ItemLineAnchor {
                        kind: LineAnchorKind::RenderedLine,
                        rendered_line: 0,
                        ..ItemLineAnchor::default()
                    }],
                    5,
                ),
            },
            RenderItemSummary {
                item_index: 1,
                start_line: 1,
                gap_before: 1,
                content_line_count: 1,
                total_line_count: 2,
                gap_owner_item_index: Some(0),
                block: render_block(
                    "second",
                    vec![ItemLineAnchor {
                        kind: LineAnchorKind::RenderedLine,
                        rendered_line: 0,
                        ..ItemLineAnchor::default()
                    }],
                    6,
                ),
            },
            RenderItemSummary {
                item_index: 2,
                start_line: 3,
                gap_before: 1,
                content_line_count: 1,
                total_line_count: 2,
                gap_owner_item_index: Some(1),
                block: render_block(
                    "third",
                    vec![ItemLineAnchor {
                        kind: LineAnchorKind::RenderedLine,
                        rendered_line: 0,
                        ..ItemLineAnchor::default()
                    }],
                    5,
                ),
            },
        ]);

        let gap_anchor = render.line_at(3).expect("line 3 should exist").anchor;

        assert_eq!(gap_anchor.item_index, 1);
        assert_eq!(gap_anchor.item_anchor.kind, LineAnchorKind::ItemGap);
        assert_eq!(render.line_index_for_anchor(gap_anchor), Some(3));
    }
}
