use std::{cmp::Ordering, rc::Rc};

use ratatui::text::Line;

use crate::frontend::tui::selection::SelectableLineRange;

use super::cache::CachedRenderBlock;

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
#[derive(Debug, Clone)]
pub(crate) struct RenderResult {
    pub(crate) items: Rc<Vec<RenderItemSummary>>,
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
    pub(crate) content_char_len: usize,
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
#[derive(Debug, Clone)]
pub(crate) struct RenderedTranscriptLine {
    pub(crate) line: Line<'static>,
    pub(crate) plain_line: String,
    pub(crate) anchor: LineAnchor,
}

/// `ViewportRenderResult` 表示 transcript 在给定 viewport 下的可视切片。
#[allow(dead_code)]
#[derive(Debug, Clone, Default)]
pub(crate) struct ViewportRenderResult {
    pub(crate) lines: Vec<Line<'static>>,
    pub(crate) plain_lines: Vec<String>,
    pub(crate) line_count: usize,
    pub(crate) total_line_count: usize,
    pub(crate) resolved_offset: usize,
}

/// `RenderRangeSlice` 表示 transcript 某个连续区间的物化结果。
#[derive(Debug, Clone, Default)]
pub(crate) struct RenderRangeSlice {
    pub(crate) lines: Vec<Line<'static>>,
    pub(crate) line_count: usize,
    pub(crate) plain_char_len: usize,
    #[cfg(test)]
    pub(crate) plain_lines: Vec<String>,
}

impl RenderResult {
    /// `viewport` 返回给定偏移和高度下的可视行切片。
    pub(crate) fn viewport(&self, offset: usize, height: usize) -> ViewportRenderResult {
        let (slice, resolved_offset) = visible_rendered_lines(self, offset, height);

        ViewportRenderResult {
            line_count: slice.line_count,
            total_line_count: self.line_count,
            lines: slice.lines,
            plain_lines: {
                #[cfg(test)]
                {
                    slice.plain_lines
                }
                #[cfg(not(test))]
                {
                    Vec::new()
                }
            },
            resolved_offset,
        }
    }

    pub(crate) fn plain_text_len(&self) -> usize {
        if self.line_count == 0 {
            return 0;
        }

        self.content_char_len + self.line_count.saturating_sub(1)
    }

    pub(crate) fn plain_text_len_for_range(&self, start: usize, count: usize) -> usize {
        if count == 0 || self.line_count == 0 || start >= self.line_count {
            return 0;
        }

        if start == 0 && count >= self.line_count {
            return self.plain_text_len();
        }

        let slice = self.range_slice(start, count);
        if slice.line_count == 0 {
            return 0;
        }

        slice.plain_char_len + slice.line_count.saturating_sub(1)
    }

    pub(crate) fn lines_for_range(&self, start: usize, count: usize) -> Vec<Line<'static>> {
        self.range_slice(start, count).lines
    }

    #[cfg(test)]
    pub(crate) fn plain_lines_for_range(&self, start: usize, count: usize) -> Vec<String> {
        self.range_slice(start, count).plain_lines
    }

    pub(crate) fn line_at(&self, index: usize) -> Option<RenderedTranscriptLine> {
        if index >= self.line_count {
            return None;
        }

        let summary = self.summary_for_line(index)?;
        let relative = index.saturating_sub(summary.start_line);
        if relative < summary.gap_before {
            return Some(RenderedTranscriptLine {
                line: Line::raw(""),
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
            line: summary.block.lines.get(block_index)?.clone(),
            plain_line: summary.block.plain_lines.get(block_index)?.clone(),
            anchor: LineAnchor {
                item_index: summary.item_index,
                item_anchor: *summary.block.anchors.get(block_index)?,
            },
        })
    }

    pub(crate) fn item_lines(&self, item_index: usize) -> Option<RenderItemLines> {
        let position = self.summary_position_for_item(item_index)?;
        let summary = self.items.get(position)?;
        Some(RenderItemLines {
            content_start_line: summary.start_line + summary.gap_before,
            content_line_count: summary.content_line_count,
            total_line_count: summary.content_line_count + self.trailing_gap_line_count(position),
        })
    }

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

    pub(crate) fn range_slice(&self, start: usize, count: usize) -> RenderRangeSlice {
        if count == 0 || self.line_count == 0 || start >= self.line_count {
            return RenderRangeSlice::default();
        }

        let end = (start + count).min(self.line_count);
        let mut remaining = end - start;
        let mut slice = RenderRangeSlice {
            lines: Vec::with_capacity(remaining),
            line_count: remaining,
            plain_char_len: 0,
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
                slice
                    .lines
                    .extend(summary.block.lines[block_start..block_end].iter().cloned());
                slice.plain_char_len += summary.block.plain_lines[block_start..block_end]
                    .iter()
                    .map(String::len)
                    .sum::<usize>();
                #[cfg(test)]
                slice.plain_lines.extend(
                    summary.block.plain_lines[block_start..block_end]
                        .iter()
                        .cloned(),
                );
            }

            remaining -= taken;
            position += 1;
            line_offset = 0;
        }

        slice
    }

    fn summary_for_line(&self, index: usize) -> Option<&RenderItemSummary> {
        let position = self.summary_position_for_line(index)?;
        self.items.get(position)
    }

    fn item_summary(&self, item_index: usize) -> Option<&RenderItemSummary> {
        let position = self.summary_position_for_item(item_index)?;
        self.items.get(position)
    }

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

    fn summary_position_for_item(&self, item_index: usize) -> Option<usize> {
        let position = *self.item_positions.get(item_index)?;
        (position != usize::MAX).then_some(position)
    }

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

    fn block_index_for_anchor(
        &self,
        summary: &RenderItemSummary,
        anchor: ItemLineAnchor,
    ) -> Option<usize> {
        let block_index = anchor.rendered_line;
        if summary.block.anchors.get(block_index).copied() == Some(anchor) {
            return Some(block_index);
        }

        summary
            .block
            .anchors
            .iter()
            .position(|candidate| *candidate == anchor)
    }
}

impl Default for RenderResult {
    fn default() -> Self {
        Self {
            items: Rc::new(Vec::new()),
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
    new_render_result_with_append_start(items, -1)
}

pub(crate) fn new_render_result_with_append_start(
    items: Vec<RenderItemSummary>,
    append_start_line: isize,
) -> RenderResult {
    if items.is_empty() {
        return RenderResult::default();
    }

    let item_position_len = items
        .last()
        .map(|item| item.item_index.saturating_add(1))
        .unwrap_or(0);
    let mut item_positions = vec![usize::MAX; item_position_len];
    let mut line_count = 0;
    let mut content_char_len = 0;
    for (index, item) in items.iter().enumerate() {
        item_positions[item.item_index] = index;
        line_count += item.total_line_count;
        content_char_len += item.content_char_len;
    }
    if line_count == 0 {
        return RenderResult::default();
    }

    RenderResult {
        items: Rc::new(items),
        item_positions: Rc::new(item_positions),
        selectable_ranges: Rc::new(Vec::new()),
        line_count,
        content_char_len,
        append_start_line,
    }
}

pub(crate) fn visible_rendered_lines(
    render: &RenderResult,
    offset: usize,
    height: usize,
) -> (RenderRangeSlice, usize) {
    if render.line_count == 0 {
        return (RenderRangeSlice::default(), 0);
    }

    if height == 0 || height >= render.line_count {
        return (render.range_slice(0, render.line_count), 0);
    }

    let max_offset = render.line_count.saturating_sub(height);
    let resolved_offset = offset.min(max_offset);

    (render.range_slice(resolved_offset, height), resolved_offset)
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
            lines: Rc::new(vec![Line::raw(text.to_string()); anchors.len()]),
            plain_lines: Rc::new(vec![text.to_string(); anchors.len()]),
            anchors: Rc::new(anchors),
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
                content_char_len: 5,
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
                content_char_len: 6,
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
                content_char_len: 5,
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
                content_char_len: 6,
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
                content_char_len: 5,
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
