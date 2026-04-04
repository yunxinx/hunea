use std::{cell::RefCell, collections::HashMap, rc::Rc};

use ratatui::text::Line;

use crate::frontend::tui::selection::SelectableLineRange;
use crate::frontend::tui::transcript::{Transcript, TranscriptItem};

use super::{
    DocumentAnchorRegion, DocumentLayout, DocumentLayoutKey, DocumentLineAnchor,
    DocumentTranscriptSnapshot, layout::transcript_composer_gap_line_count,
    line_access::new_document_transcript_item_index,
};

/// `DocumentTranscriptAppend` 表示 transcript 尾部新增到 unified document 的稳定片段。
#[derive(Debug, Clone, Default)]
pub(crate) struct DocumentTranscriptAppend {
    pub(crate) previous_transcript_line_count: usize,
    pub(crate) lines: Vec<Line<'static>>,
    pub(crate) plain_lines: Vec<String>,
    pub(crate) anchors: Vec<DocumentLineAnchor>,
    pub(crate) items: HashMap<usize, TranscriptItem>,
}

/// `sliced_transcript_append` 从完整 transcript 渲染结果中切出这次追加的尾部片段。
pub(crate) fn sliced_transcript_append(
    render: &crate::frontend::tui::transcript::RenderResult,
    start_line: usize,
    transcript: &Transcript,
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

    let mut items = HashMap::new();
    let mut previous_item_index = None;
    for anchor in &render.line_anchors[start_line..] {
        if previous_item_index == Some(anchor.item_index) {
            continue;
        }
        previous_item_index = Some(anchor.item_index);
        if let Some(item) = transcript.item(anchor.item_index).cloned() {
            items.insert(anchor.item_index, item);
        }
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
        items,
    })
}

/// `extend_document_layout_from_transcript_append` 把 transcript 尾部新增行插到 composer 前面。
pub(crate) fn extend_document_layout_from_transcript_append(
    base: &DocumentLayout,
    appended: DocumentTranscriptAppend,
    transcript: DocumentTranscriptSnapshot,
) -> DocumentLayout {
    if appended.lines.is_empty() {
        return base.clone();
    }

    let insert_at = appended
        .previous_transcript_line_count
        .min(base.lines.len());
    let appended_line_count = appended.lines.len();
    let line_delta = appended_line_count
        + usize::from(appended.previous_transcript_line_count == 0)
            * transcript_composer_gap_line_count();
    let mut lines = base.lines.clone();
    lines.splice(insert_at..insert_at, appended.lines);
    let mut plain_lines = base.plain_lines.clone();
    plain_lines.splice(insert_at..insert_at, appended.plain_lines);
    let mut anchors = base.anchors.clone();
    anchors.splice(insert_at..insert_at, appended.anchors);
    let mut selectable = base.selectable.clone();
    selectable.splice(
        insert_at..insert_at,
        std::iter::repeat_n(SelectableLineRange::default(), appended_line_count),
    );

    if appended.previous_transcript_line_count == 0 {
        let gap_insert_at = insert_at + appended_line_count;
        let mut gap_lines = Vec::with_capacity(transcript_composer_gap_line_count());
        let mut gap_plain_lines = Vec::with_capacity(transcript_composer_gap_line_count());
        let mut gap_anchors = Vec::with_capacity(transcript_composer_gap_line_count());
        let mut gap_selectable = Vec::with_capacity(transcript_composer_gap_line_count());
        for gap_index in 0..transcript_composer_gap_line_count() {
            gap_lines.push(Line::raw(""));
            gap_plain_lines.push(String::new());
            gap_anchors.push(DocumentLineAnchor {
                region: DocumentAnchorRegion::TranscriptComposerGap,
                gap_index,
                ..DocumentLineAnchor::default()
            });
            gap_selectable.push(SelectableLineRange::default());
        }
        lines.splice(gap_insert_at..gap_insert_at, gap_lines);
        plain_lines.splice(gap_insert_at..gap_insert_at, gap_plain_lines);
        anchors.splice(gap_insert_at..gap_insert_at, gap_anchors);
        selectable.splice(gap_insert_at..gap_insert_at, gap_selectable);
    }
    let transcript_items = new_document_transcript_item_index(&transcript);

    let mut composer_slot = base.composer_slot;
    composer_slot.frame_start_line += line_delta;
    composer_slot.content_start_line += line_delta;

    DocumentLayout {
        transcript,
        transcript_line_count: base.transcript_line_count + appended_line_count,
        transcript_items,
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

/// `extend_document_transcript_snapshot_from_append` 把新追加的 transcript 尾部并入旧快照。
pub(crate) fn extend_document_transcript_snapshot_from_append(
    base: &DocumentTranscriptSnapshot,
    appended: &DocumentTranscriptAppend,
) -> DocumentTranscriptSnapshot {
    if appended.lines.is_empty() {
        return base.clone();
    }

    let insert_at = appended
        .previous_transcript_line_count
        .min(base.lines.len());
    let mut lines = base.lines.clone();
    lines.splice(insert_at..insert_at, appended.lines.iter().cloned());

    let mut plain_lines = base.plain_lines.clone();
    plain_lines.splice(insert_at..insert_at, appended.plain_lines.iter().cloned());

    let mut anchors = base.anchors.clone();
    anchors.splice(insert_at..insert_at, appended.anchors.iter().copied());

    let mut items = base.items.clone();
    items.extend(appended.items.clone());

    DocumentTranscriptSnapshot {
        lines,
        plain_lines,
        anchors,
        width: base.width,
        palette: base.palette,
        items,
        selectable_cache: Rc::new(RefCell::new(base.selectable_cache.borrow().clone())),
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
