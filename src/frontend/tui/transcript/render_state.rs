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
#[derive(Debug, Clone, Default)]
pub(crate) struct RenderResult {
    pub(crate) lines: Vec<Line<'static>>,
    pub(crate) plain_lines: Vec<String>,
    pub(crate) line_anchors: Vec<LineAnchor>,
    pub(crate) line_count: usize,
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

impl RenderResult {
    /// `viewport` 返回给定偏移和高度下的可视行切片。
    pub(crate) fn viewport(&self, offset: usize, height: usize) -> ViewportRenderResult {
        let (lines, plain_lines, resolved_offset) =
            visible_rendered_lines(&self.lines, &self.plain_lines, offset, height);

        ViewportRenderResult {
            line_count: lines.len(),
            total_line_count: self.line_count,
            lines,
            plain_lines,
            resolved_offset,
        }
    }
}

pub(crate) fn new_render_result(
    lines: Vec<Line<'static>>,
    plain_lines: Vec<String>,
    line_anchors: Vec<LineAnchor>,
) -> RenderResult {
    if lines.is_empty() {
        return RenderResult::default();
    }

    let line_count = lines.len();
    RenderResult {
        lines,
        plain_lines,
        line_anchors,
        line_count,
    }
}

pub(crate) fn visible_rendered_lines(
    lines: &[Line<'static>],
    plain_lines: &[String],
    offset: usize,
    height: usize,
) -> (Vec<Line<'static>>, Vec<String>, usize) {
    if lines.is_empty() {
        return (Vec::new(), Vec::new(), 0);
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
