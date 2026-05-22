use crate::transcript::LineAnchorKind;

use super::{
    DocumentAnchorRegion, DocumentLayout, DocumentLineAnchor, DocumentViewportAnchor,
    anchor_match::{
        canonical_rendered_transcript_anchor_text, find_document_offset_for_viewport_anchor,
        transcript_content_line_count_for_item,
    },
};

/// `TranscriptSemanticPosition` 用粗粒度语义描述锚点更接近 item 的哪一段。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum TranscriptSemanticPosition {
    #[default]
    Unknown,
    WholeItem,
    Start,
    Middle,
    End,
}

/// `ViewAnchor` 表示 viewport 当前围绕的语义锚点。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) enum ViewAnchor {
    Top,
    #[default]
    Bottom,
    Line(DocumentViewportAnchor),
}

/// `ViewportState` 收口主路径使用的视口状态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ViewportState {
    anchor: ViewAnchor,
    anchor_viewport_offset: usize,
    resolved_offset: usize,
    follow_bottom: bool,
    manual_scroll: bool,
    viewport_height: usize,
    document_width: u16,
}

impl Default for ViewportState {
    fn default() -> Self {
        Self {
            anchor: ViewAnchor::Bottom,
            anchor_viewport_offset: 0,
            resolved_offset: 0,
            follow_bottom: true,
            manual_scroll: false,
            viewport_height: 0,
            document_width: 0,
        }
    }
}

impl ViewportState {
    pub(crate) fn capture(
        layout: &DocumentLayout,
        line_indices: &[usize],
        resolved_offset: usize,
        follow_bottom: bool,
        manual_scroll: bool,
        viewport_height: usize,
        document_width: u16,
    ) -> Self {
        let resolved_offset =
            clamp_document_offset(resolved_offset, viewport_height, layout.line_count());
        if layout.line_count() == 0 {
            return Self {
                anchor: ViewAnchor::Top,
                anchor_viewport_offset: 0,
                resolved_offset,
                follow_bottom,
                manual_scroll,
                viewport_height,
                document_width,
            };
        }

        if follow_bottom && !manual_scroll {
            return Self::bottom_follow(resolved_offset, viewport_height, document_width);
        }

        let (anchor, anchor_viewport_offset) = preferred_view_anchor(layout, line_indices)
            .map(|(line_index, viewport_offset)| {
                let anchor = document_viewport_anchor_at_line(layout, line_index)
                    .map(ViewAnchor::Line)
                    .unwrap_or(ViewAnchor::Top);
                (anchor, viewport_offset)
            })
            .unwrap_or((ViewAnchor::Top, 0));

        Self {
            anchor,
            anchor_viewport_offset,
            resolved_offset,
            follow_bottom,
            manual_scroll,
            viewport_height,
            document_width,
        }
    }

    pub(crate) fn bottom_follow(
        resolved_offset: usize,
        viewport_height: usize,
        document_width: u16,
    ) -> Self {
        Self {
            anchor: ViewAnchor::Bottom,
            anchor_viewport_offset: 0,
            resolved_offset,
            follow_bottom: true,
            manual_scroll: false,
            viewport_height,
            document_width,
        }
    }

    pub(crate) fn resolve_offset(&self, layout: &DocumentLayout, viewport_height: usize) -> usize {
        if layout.line_count() == 0 {
            return 0;
        }

        let fallback =
            clamp_document_offset(self.resolved_offset, viewport_height, layout.line_count());
        let anchor_offset = match &self.anchor {
            ViewAnchor::Top => 0,
            ViewAnchor::Bottom => layout.line_count().saturating_sub(1),
            ViewAnchor::Line(anchor) => {
                if let Some(offset) = find_document_offset_for_viewport_anchor(layout, anchor) {
                    offset
                } else {
                    return fallback;
                }
            }
        };

        clamp_document_offset(
            anchor_offset.saturating_sub(self.anchor_viewport_offset),
            viewport_height,
            layout.line_count(),
        )
    }

    /// `resolve_offset_for_current_geometry` 在终端几何未变化时复用已解析 offset。
    ///
    /// 手动滚动热路径只需要知道当前窗口覆盖哪些行；跨 resize / reflow 的语义
    /// anchor 匹配仍由 `resolve_offset` 负责。
    pub(crate) fn resolve_offset_for_current_geometry(
        &self,
        layout: &DocumentLayout,
        viewport_height: usize,
        document_width: u16,
    ) -> usize {
        if self.viewport_height == viewport_height && self.document_width == document_width {
            return clamp_document_offset(
                self.resolved_offset,
                viewport_height,
                layout.line_count(),
            );
        }

        self.resolve_offset(layout, viewport_height)
    }

    #[cfg(test)]
    pub(crate) fn anchor(&self) -> &ViewAnchor {
        &self.anchor
    }

    #[cfg(test)]
    pub(crate) const fn anchor_viewport_offset(&self) -> usize {
        self.anchor_viewport_offset
    }

    pub(crate) const fn resolved_offset(&self) -> usize {
        self.resolved_offset
    }

    pub(crate) const fn follow_bottom(&self) -> bool {
        self.follow_bottom
    }

    pub(crate) const fn manual_scroll(&self) -> bool {
        self.manual_scroll
    }
}

/// `document_viewport_anchor_at_line` 为指定 unified document 行提取恢复所需的语义锚点。
pub(crate) fn document_viewport_anchor_at_line(
    layout: &DocumentLayout,
    line_index: usize,
) -> Option<DocumentViewportAnchor> {
    let line_anchor = layout.line_anchor_at(line_index)?;
    let mut line_text = layout.line_text_at(line_index)?;
    let mut transcript_item_line_count = 0;
    let mut semantic_position = TranscriptSemanticPosition::Unknown;

    if matches!(line_anchor.region, DocumentAnchorRegion::Transcript) {
        transcript_item_line_count =
            transcript_content_line_count_for_item(layout, line_anchor.transcript.item_index);
        semantic_position = transcript_semantic_position(
            line_anchor.transcript.item_anchor.kind,
            line_anchor.transcript.item_anchor.rendered_line,
            transcript_item_line_count,
        );

        if matches!(
            line_anchor.transcript.item_anchor.kind,
            LineAnchorKind::RenderedLine
        ) {
            line_text = canonical_rendered_transcript_anchor_text(&line_text);
        }
    }

    Some(DocumentViewportAnchor {
        line_anchor,
        line_text,
        transcript_item_line_count,
        transcript_semantic_position: semantic_position,
    })
}

pub(crate) fn transcript_semantic_position(
    kind: LineAnchorKind,
    rendered_line: usize,
    item_line_count: usize,
) -> TranscriptSemanticPosition {
    if matches!(kind, LineAnchorKind::ItemGap) || item_line_count == 0 {
        return TranscriptSemanticPosition::Unknown;
    }

    if item_line_count == 1 {
        return TranscriptSemanticPosition::WholeItem;
    }

    if rendered_line == 0 {
        return TranscriptSemanticPosition::Start;
    }

    if rendered_line + 1 >= item_line_count {
        return TranscriptSemanticPosition::End;
    }

    TranscriptSemanticPosition::Middle
}

fn preferred_view_anchor(
    layout: &DocumentLayout,
    line_indices: &[usize],
) -> Option<(usize, usize)> {
    let mut best = None;
    for (viewport_offset, &line_index) in line_indices.iter().enumerate() {
        let priority = layout
            .line_anchor_at(line_index)
            .map(view_anchor_priority)
            .unwrap_or(usize::MAX);
        if best
            .as_ref()
            .map(|(_, _, best_priority)| priority < *best_priority)
            .unwrap_or(true)
        {
            best = Some((line_index, viewport_offset, priority));
        }
    }

    best.map(|(line_index, viewport_offset, _)| (line_index, viewport_offset))
}

fn view_anchor_priority(anchor: DocumentLineAnchor) -> usize {
    match anchor.region {
        DocumentAnchorRegion::Transcript
            if !matches!(anchor.transcript.item_anchor.kind, LineAnchorKind::ItemGap) =>
        {
            0
        }
        DocumentAnchorRegion::Composer => 1,
        DocumentAnchorRegion::StreamActivity
        | DocumentAnchorRegion::AcpPanel
        | DocumentAnchorRegion::CommandPanel
        | DocumentAnchorRegion::ToolApprovalPanel
        | DocumentAnchorRegion::ModelPanel
        | DocumentAnchorRegion::StatusLine => 2,
        DocumentAnchorRegion::TranscriptComposerGap
        | DocumentAnchorRegion::StreamActivityComposerGap
        | DocumentAnchorRegion::ComposerStatusGap
        | DocumentAnchorRegion::ComposerPadding => 3,
        DocumentAnchorRegion::Transcript => 4,
        DocumentAnchorRegion::None => 5,
    }
}

fn clamp_document_offset(offset: usize, viewport_height: usize, line_count: usize) -> usize {
    if viewport_height == 0 || line_count <= viewport_height {
        return 0;
    }

    offset.min(line_count - viewport_height)
}
