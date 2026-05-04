use std::rc::Rc;

use super::*;
use crate::frontend::tui::styled_text::{line_plain_text_len, line_to_plain_text};

impl Transcript {
    pub(super) fn build_render_result(
        &mut self,
        width: u16,
        dirty_from: usize,
        append_start_line: isize,
        index: TranscriptItemMetricsIndex,
    ) -> RenderResult {
        let previous = Rc::clone(&self.screen_cache.result);
        let mut items = Vec::with_capacity(self.items.len());

        if dirty_from > 0 {
            for summary in previous.items.iter() {
                if summary.item_index >= dirty_from {
                    break;
                }
                items.push(summary.clone());
            }
        }

        let start_position = index
            .visible_items
            .partition_point(|item| item.item_index < dirty_from);
        for position in index.visible_items.iter().skip(start_position) {
            let block = self.render_screen_block(position.item_index, width);
            let summary = RenderItemSummary {
                item_index: position.item_index,
                start_line: position.start_line,
                gap_before: position.gap_before,
                content_line_count: position.content_line_count,
                total_line_count: position.total_line_count,
                gap_owner_item_index: position.gap_owner_item_index,
                block,
            };
            items.push(summary);
        }

        new_render_result_with_append_start(items, index, append_start_line)
    }

    pub(super) fn render_screen_block(
        &mut self,
        index: usize,
        width: u16,
    ) -> Rc<CachedRenderBlock> {
        let has_dynamic_render = self.items[index].has_active_acp_tool_call();
        let cache_key = self.items[index].render_cache_key();
        if !has_dynamic_render
            && let Some(cached) = self
                .screen_cache
                .reusable_item_block(index, width, cache_key)
        {
            return cached;
        }

        let block = Rc::new(materialize_transcript_item_render_block(
            self.items[index].as_ref(),
            width,
            self.palette,
        ));
        if !has_dynamic_render {
            self.screen_cache.store_item_block(index, Rc::clone(&block));
        }
        block
    }
}

/// `materialize_transcript_item_render_block` 为单个 transcript item 构造稳定的屏幕块。
pub(crate) fn materialize_transcript_item_render_block(
    item: &TranscriptItem,
    width: u16,
    palette: TerminalPalette,
) -> CachedRenderBlock {
    let cache_key = item.render_cache_key();

    if let TranscriptItem::Message(message) = item
        && let Some(projection) = message.render_projection(width, palette)
    {
        let plain_line_byte_lens = projection.plain_line_lens();
        let plain_text_char_len = plain_line_byte_lens.iter().sum();
        let anchors = projection.line_anchors();
        let line_count = projection.line_count();
        return CachedRenderBlock {
            cache_key,
            width,
            palette,
            lines: Rc::new(Vec::new()),
            projected_user: Some(Rc::new(projection)),
            line_count,
            plain_text_char_len,
            plain_line_byte_lens: Rc::new(plain_line_byte_lens),
            anchors: CachedLineAnchors::Explicit(Rc::new(anchors)),
        };
    }

    let lines = match item {
        TranscriptItem::ToolResult(tool_result) => {
            tool_result.render_lines_at(width, palette, std::time::Instant::now())
        }
        _ => item.render_lines(width, palette),
    };
    let anchors = item.render_line_anchors(width, palette);
    let plain_line_byte_lens = lines.iter().map(line_plain_text_len).collect::<Vec<_>>();
    let plain_text_char_len = plain_line_byte_lens.iter().sum();
    let uses_explicit_anchors = anchors.len() == lines.len();
    CachedRenderBlock {
        cache_key,
        width,
        palette,
        plain_text_char_len,
        line_count: lines.len(),
        lines: Rc::new(lines),
        projected_user: None,
        plain_line_byte_lens: Rc::new(plain_line_byte_lens),
        anchors: if uses_explicit_anchors {
            CachedLineAnchors::Explicit(Rc::new(anchors))
        } else {
            CachedLineAnchors::GeneratedRenderedLines
        },
    }
}

impl TranscriptItem {
    pub(super) fn estimate_render_metrics_fast(
        &self,
        width: u16,
        palette: TerminalPalette,
        previous_metrics: Option<TranscriptItemMetrics>,
    ) -> TranscriptFastEstimate {
        match self {
            Self::Hero(item) => item.estimate_render_metrics_fast(width, palette, previous_metrics),
            Self::Message(item) => {
                item.estimate_render_metrics_fast(width, palette, previous_metrics)
            }
            Self::Reasoning(item) => {
                item.estimate_render_metrics_fast(width, palette, previous_metrics)
            }
            Self::System(item) => {
                item.estimate_render_metrics_fast(width, palette, previous_metrics)
            }
            Self::ToolResult(item) => {
                item.estimate_render_metrics_fast(width, palette, previous_metrics)
            }
        }
    }

    pub(super) fn measure_render_metrics(
        &self,
        width: u16,
        palette: TerminalPalette,
    ) -> (usize, usize) {
        match self {
            Self::Hero(item) => item.measure_render_metrics(width, palette),
            Self::Message(item) => item.measure_render_metrics(width, palette),
            Self::Reasoning(item) => item.measure_render_metrics(width, palette),
            Self::System(item) => item.measure_render_metrics(width, palette),
            Self::ToolResult(item) => item.measure_render_metrics(width, palette),
        }
    }

    pub(super) fn render_lines(&self, width: u16, palette: TerminalPalette) -> Vec<Line<'static>> {
        match self {
            Self::Hero(item) => item.render_lines(width, palette),
            Self::Message(item) => item.render_lines(width, palette),
            Self::Reasoning(item) => item.render_lines(width, palette),
            Self::System(item) => item.render_lines(width, palette),
            Self::ToolResult(item) => item.render_lines(width, palette),
        }
    }

    pub(super) fn render_for_terminal_replay(
        &self,
        width: u16,
        palette: TerminalPalette,
        preserve_ansi: bool,
    ) -> String {
        match self {
            Self::Hero(item) => item.render_for_terminal_replay(width, palette, preserve_ansi),
            Self::Message(item) => item.render_for_terminal_replay(width, palette, preserve_ansi),
            Self::Reasoning(item) => item.render_for_terminal_replay(width, palette, preserve_ansi),
            Self::System(item) => item.render_for_terminal_replay(width, palette, preserve_ansi),
            Self::ToolResult(item) => {
                item.render_for_terminal_replay(width, palette, preserve_ansi)
            }
        }
    }

    pub(super) fn render_plain_text(&self, width: u16, palette: TerminalPalette) -> String {
        match self {
            Self::Hero(item) => item.render_plain_text(width, palette),
            Self::Message(item) => item.render_plain_text(width, palette),
            Self::Reasoning(item) => item.render_plain_text(width, palette),
            Self::System(item) => item.render_plain_text(width, palette),
            Self::ToolResult(item) => item.render_plain_text(width, palette),
        }
    }

    pub(crate) fn render_plain_lines(&self, width: u16, palette: TerminalPalette) -> Vec<String> {
        self.render_lines(width, palette)
            .iter()
            .map(line_to_plain_text)
            .collect()
    }

    pub(super) fn render_line_anchors(
        &self,
        width: u16,
        palette: TerminalPalette,
    ) -> Vec<ItemLineAnchor> {
        match self {
            Self::Hero(item) => item.render_line_anchors(width, palette),
            Self::Message(item) => item.render_line_anchors(width, palette),
            Self::Reasoning(item) => item.render_line_anchors(width, palette),
            Self::System(item) => item.render_line_anchors(width, palette),
            Self::ToolResult(item) => item.render_line_anchors(width, palette),
        }
    }

    pub(crate) fn render_selectable_line_ranges(
        &self,
        width: u16,
        palette: TerminalPalette,
        plain_lines: &[String],
    ) -> Vec<SelectableLineRange> {
        let ranges = match self {
            Self::Hero(_) => Vec::new(),
            Self::Message(item) => item.render_selectable_line_ranges(width, palette),
            Self::Reasoning(_) => Vec::new(),
            Self::System(_) => Vec::new(),
            Self::ToolResult(_) => Vec::new(),
        };
        if ranges.len() == plain_lines.len() {
            return ranges;
        }

        plain_lines
            .iter()
            .map(|line| {
                normalize_transcript_selectable_range(line, usize::from(width.max(1)), true)
            })
            .collect()
    }

    pub(crate) fn is_assistant_message(&self) -> bool {
        matches!(self, Self::Message(item) if item.is_assistant())
            || matches!(self, Self::Reasoning(item) if item.uses_assistant_visual_inset())
    }

    pub(crate) fn render_cache_key(&self) -> u64 {
        match self {
            Self::Hero(item) => item.render_cache_key(),
            Self::Message(item) => item.render_cache_key(),
            Self::Reasoning(item) => item.render_cache_key(),
            Self::System(item) => item.render_cache_key(),
            Self::ToolResult(item) => item.render_cache_key(),
        }
    }

    pub(crate) fn source_text_byte_len(&self) -> usize {
        match self {
            Self::Hero(item) => item.source_text_byte_len(),
            Self::Message(item) => item.source_text_byte_len(),
            Self::Reasoning(item) => item.source_text_byte_len(),
            Self::System(item) => item.source_text_byte_len(),
            Self::ToolResult(item) => item.source_text_byte_len(),
        }
    }

    pub(crate) fn has_active_acp_tool_call(&self) -> bool {
        matches!(self, Self::ToolResult(item) if item.has_active_acp_tool_call())
    }

    pub(crate) fn active_marker_started_at(&self) -> Option<std::time::Instant> {
        match self {
            Self::ToolResult(item) => item.active_marker_started_at(),
            _ => None,
        }
    }
}
