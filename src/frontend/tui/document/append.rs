use ratatui::text::Line;

use crate::frontend::tui::selection::SelectableLineRange;

use super::{
    DocumentAnchorRegion, DocumentLayout, DocumentLayoutKey, DocumentLineAnchor,
    layout::transcript_composer_gap_line_count,
};

/// `DocumentTranscriptAppend` 表示 transcript 尾部新增到 unified document 的稳定片段。
#[derive(Debug, Clone, Default)]
pub(crate) struct DocumentTranscriptAppend {
    pub(crate) previous_transcript_line_count: usize,
    pub(crate) lines: Vec<Line<'static>>,
    pub(crate) plain_lines: Vec<String>,
    pub(crate) anchors: Vec<DocumentLineAnchor>,
    pub(crate) selectable: Vec<SelectableLineRange>,
}

/// `sliced_transcript_append` 从完整 transcript 渲染结果中切出这次追加的尾部片段。
pub(crate) fn sliced_transcript_append(
    render: &crate::frontend::tui::transcript::RenderResult,
    start_line: usize,
) -> Option<DocumentTranscriptAppend> {
    if start_line > render.lines.len() {
        return None;
    }

    let appended_len = render.lines.len().saturating_sub(start_line);
    if appended_len == 0 {
        return Some(DocumentTranscriptAppend {
            previous_transcript_line_count: start_line,
            ..DocumentTranscriptAppend::default()
        });
    }

    Some(DocumentTranscriptAppend {
        previous_transcript_line_count: start_line,
        lines: render.lines[start_line..].to_vec(),
        plain_lines: render.plain_lines[start_line..].to_vec(),
        anchors: render.line_anchors[start_line..]
            .iter()
            .copied()
            .map(|transcript| DocumentLineAnchor {
                region: DocumentAnchorRegion::Transcript,
                transcript,
                ..DocumentLineAnchor::default()
            })
            .collect(),
        selectable: render.selectable_ranges[start_line..].to_vec(),
    })
}

/// `extend_document_layout_from_transcript_append` 把 transcript 尾部新增行插到 composer 前面。
pub(crate) fn extend_document_layout_from_transcript_append(
    base: &DocumentLayout,
    appended: DocumentTranscriptAppend,
) -> DocumentLayout {
    if appended.lines.is_empty() {
        return base.clone();
    }

    let insert_at = appended
        .previous_transcript_line_count
        .min(base.lines.len());
    let mut insert_lines = appended.lines;
    let mut insert_plain_lines = appended.plain_lines;
    let mut insert_anchors = appended.anchors;
    let mut insert_selectable = appended.selectable;

    if appended.previous_transcript_line_count == 0 {
        for gap_index in 0..transcript_composer_gap_line_count() {
            insert_lines.push(Line::raw(""));
            insert_plain_lines.push(String::new());
            insert_anchors.push(DocumentLineAnchor {
                region: DocumentAnchorRegion::TranscriptComposerGap,
                gap_index,
                ..DocumentLineAnchor::default()
            });
            insert_selectable.push(SelectableLineRange::default());
        }
    }

    let line_delta = insert_lines.len();

    let mut lines = Vec::with_capacity(base.lines.len() + line_delta);
    lines.extend_from_slice(&base.lines[..insert_at]);
    lines.extend(insert_lines);
    lines.extend_from_slice(&base.lines[insert_at..]);

    let mut plain_lines = Vec::with_capacity(base.plain_lines.len() + line_delta);
    plain_lines.extend_from_slice(&base.plain_lines[..insert_at]);
    plain_lines.extend(insert_plain_lines);
    plain_lines.extend_from_slice(&base.plain_lines[insert_at..]);

    let mut anchors = Vec::with_capacity(base.anchors.len() + line_delta);
    anchors.extend_from_slice(&base.anchors[..insert_at]);
    anchors.extend(insert_anchors);
    anchors.extend_from_slice(&base.anchors[insert_at..]);

    let mut selectable = Vec::with_capacity(base.selectable.len() + line_delta);
    selectable.extend_from_slice(&base.selectable[..insert_at]);
    selectable.extend(insert_selectable);
    selectable.extend_from_slice(&base.selectable[insert_at..]);

    let mut composer_slot = base.composer_slot;
    composer_slot.frame_start_line += line_delta;
    composer_slot.content_start_line += line_delta;

    DocumentLayout {
        lines,
        plain_lines,
        anchors,
        selectable,
        composer_slot,
        composer_start_line: base.composer_start_line + line_delta,
        composer_line_count: base.composer_line_count,
        cursor_x: base.cursor_x,
        cursor_y: base.cursor_y + line_delta,
    }
}

/// `can_extend_cached_document_layout` 判断除了 transcript render 版本外，其余布局键是否一致。
pub(crate) fn can_extend_cached_document_layout(
    previous: &DocumentLayoutKey,
    current: &DocumentLayoutKey,
) -> bool {
    if current.transcript_render_version <= previous.transcript_render_version {
        return false;
    }

    let mut previous = previous.clone();
    let mut current = current.clone();
    previous.transcript_render_version = 0;
    current.transcript_render_version = 0;
    previous == current
}
