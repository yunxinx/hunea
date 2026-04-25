use crate::frontend::tui::transcript::{LineAnchor, LineAnchorKind};

use super::{
    DocumentAnchorRegion, DocumentLayout, DocumentLineAnchor, DocumentViewportAnchor,
    viewport_state::{TranscriptSemanticPosition, transcript_semantic_position},
};

pub(crate) fn canonical_rendered_transcript_anchor_text(text: &str) -> String {
    if text.trim().is_empty() {
        String::new()
    } else {
        text.to_string()
    }
}

pub(crate) fn find_document_offset_for_viewport_anchor(
    layout: &DocumentLayout,
    anchor: &DocumentViewportAnchor,
) -> Option<usize> {
    if matches!(anchor.line_anchor.region, DocumentAnchorRegion::Transcript)
        && matches!(
            anchor.line_anchor.transcript.item_anchor.kind,
            LineAnchorKind::RenderedLine
        )
    {
        return find_document_offset_for_rendered_transcript_anchor(layout, anchor);
    }

    find_document_anchor_offset(layout, anchor.line_anchor)
}

pub(crate) fn transcript_content_line_count_for_item(
    layout: &DocumentLayout,
    item_index: usize,
) -> usize {
    layout
        .transcript_item_lines(item_index)
        .map(|item| item.content_line_count)
        .unwrap_or(0)
}

fn find_document_anchor_offset(
    layout: &DocumentLayout,
    anchor: DocumentLineAnchor,
) -> Option<usize> {
    if matches!(anchor.region, DocumentAnchorRegion::Transcript) {
        return find_transcript_document_anchor_offset(layout, anchor.transcript);
    }

    let mut best = None;
    for index in 0..layout.line_count() {
        let Some(candidate) = layout.line_anchor_at(index) else {
            continue;
        };
        let Some(score) = score_document_anchor_match(candidate, anchor) else {
            continue;
        };
        if best
            .as_ref()
            .map(|(_, best_score)| score < *best_score)
            .unwrap_or(true)
        {
            best = Some((index, score));
        }
    }

    best.map(|(index, _)| index)
}

fn find_transcript_document_anchor_offset(
    layout: &DocumentLayout,
    target: LineAnchor,
) -> Option<usize> {
    let item_lines = layout.transcript_item_lines(target.item_index)?;
    let start = item_lines.content_start_line;
    let end = if matches!(target.item_anchor.kind, LineAnchorKind::ItemGap) {
        start + item_lines.total_line_count
    } else {
        start + item_lines.content_line_count
    };

    let mut best = None;
    let target_anchor = DocumentLineAnchor {
        region: DocumentAnchorRegion::Transcript,
        transcript: target,
        ..DocumentLineAnchor::default()
    };
    for index in start..end {
        let Some(candidate) = layout.line_anchor_at(index) else {
            continue;
        };
        let Some(score) = score_document_anchor_match(candidate, target_anchor) else {
            continue;
        };
        if best
            .as_ref()
            .map(|(_, best_score)| score < *best_score)
            .unwrap_or(true)
        {
            best = Some((index, score));
        }
    }

    best.map(|(index, _)| index)
}

fn score_document_anchor_match(
    candidate: DocumentLineAnchor,
    target: DocumentLineAnchor,
) -> Option<usize> {
    if candidate.region != target.region {
        return None;
    }

    match candidate.region {
        DocumentAnchorRegion::Composer => {
            if candidate.composer.logical_line != target.composer.logical_line {
                return None;
            }
            Some(score_range_anchor_match(
                candidate.composer.visible_start_char,
                candidate.composer.end_char,
                target.composer.visible_start_char,
            ))
        }
        DocumentAnchorRegion::CommandPanel => {
            (candidate.gap_index == target.gap_index).then_some(0)
        }
        DocumentAnchorRegion::Transcript => {
            if candidate.transcript.item_index != target.transcript.item_index {
                return None;
            }
            if candidate.transcript.item_anchor.kind != target.transcript.item_anchor.kind {
                return None;
            }
            match candidate.transcript.item_anchor.kind {
                LineAnchorKind::ItemGap => (candidate.transcript.item_anchor.gap_offset
                    == target.transcript.item_anchor.gap_offset)
                    .then_some(0),
                LineAnchorKind::LogicalPosition => {
                    if candidate.transcript.item_anchor.logical_line
                        != target.transcript.item_anchor.logical_line
                    {
                        return None;
                    }
                    Some(score_range_anchor_match(
                        candidate.transcript.item_anchor.range_start,
                        candidate.transcript.item_anchor.range_end,
                        target.transcript.item_anchor.range_start,
                    ))
                }
                LineAnchorKind::RenderedLine => None,
            }
        }
        DocumentAnchorRegion::TranscriptComposerGap => {
            (candidate.gap_index == target.gap_index).then_some(0)
        }
        DocumentAnchorRegion::ComposerStatusGap => {
            (candidate.gap_index == target.gap_index).then_some(0)
        }
        DocumentAnchorRegion::ComposerPadding => {
            (candidate.gap_index == target.gap_index).then_some(0)
        }
        DocumentAnchorRegion::AcpActivity | DocumentAnchorRegion::StatusLine => Some(0),
        DocumentAnchorRegion::None => Some(0),
    }
}

fn score_range_anchor_match(start: usize, end: usize, target: usize) -> usize {
    if start == end {
        return start.abs_diff(target);
    }

    if start <= target && target < end {
        return 0;
    }
    if target < start {
        return start - target;
    }

    target - end + 1
}

fn find_document_offset_for_rendered_transcript_anchor(
    layout: &DocumentLayout,
    anchor: &DocumentViewportAnchor,
) -> Option<usize> {
    let item_index = anchor.line_anchor.transcript.item_index;
    let target_rendered_line = anchor.line_anchor.transcript.item_anchor.rendered_line;
    let item_lines = layout.transcript_item_lines(item_index)?;
    if anchor.transcript_item_line_count == item_lines.content_line_count
        && target_rendered_line < item_lines.content_line_count
    {
        return Some(item_lines.content_start_line + target_rendered_line);
    }

    let item_offsets = transcript_content_line_offsets_for_item(layout, item_index);
    if item_offsets.is_empty() {
        return None;
    }

    let exact = find_rendered_transcript_text_match(
        layout,
        &item_offsets,
        &anchor.line_text,
        target_rendered_line,
        anchor.transcript_item_line_count,
        anchor.transcript_semantic_position,
        true,
    );
    let fuzzy = find_rendered_transcript_text_match(
        layout,
        &item_offsets,
        &anchor.line_text,
        target_rendered_line,
        anchor.transcript_item_line_count,
        anchor.transcript_semantic_position,
        false,
    );
    match (exact, fuzzy) {
        (Some((_exact_offset, exact_score)), Some((fuzzy_offset, fuzzy_score)))
            if fuzzy_score <= exact_score =>
        {
            Some(fuzzy_offset)
        }
        (Some((exact_offset, _)), _) => Some(exact_offset),
        (_, Some((fuzzy_offset, _))) => Some(fuzzy_offset),
        _ => {
            let mut best = item_offsets[0];
            let mut best_score = score_rendered_transcript_relative_position(
                layout
                    .line_anchor_at(best)?
                    .transcript
                    .item_anchor
                    .rendered_line,
                item_offsets.len(),
                target_rendered_line,
                anchor.transcript_item_line_count,
                anchor.transcript_semantic_position,
            );
            for offset in item_offsets.into_iter().skip(1) {
                let score = score_rendered_transcript_relative_position(
                    layout
                        .line_anchor_at(offset)?
                        .transcript
                        .item_anchor
                        .rendered_line,
                    transcript_content_line_count_for_item(layout, item_index),
                    target_rendered_line,
                    anchor.transcript_item_line_count,
                    anchor.transcript_semantic_position,
                );
                if score < best_score {
                    best = offset;
                    best_score = score;
                }
            }
            Some(best)
        }
    }
}

fn transcript_content_line_offsets_for_item(
    layout: &DocumentLayout,
    item_index: usize,
) -> Vec<usize> {
    let Some(item_lines) = layout.transcript_item_lines(item_index) else {
        return Vec::new();
    };

    (item_lines.content_start_line..item_lines.content_start_line + item_lines.content_line_count)
        .collect()
}

fn find_rendered_transcript_text_match(
    layout: &DocumentLayout,
    item_offsets: &[usize],
    target_text: &str,
    target_rendered_line: usize,
    target_item_line_count: usize,
    target_semantic_position: TranscriptSemanticPosition,
    exact: bool,
) -> Option<(usize, usize)> {
    if exact {
        let mut best = None;
        for &offset in item_offsets {
            let candidate_text =
                canonical_rendered_transcript_anchor_text(&layout.line_text_at(offset)?);
            if candidate_text != target_text {
                continue;
            }

            let score = score_rendered_transcript_relative_position(
                layout
                    .line_anchor_at(offset)?
                    .transcript
                    .item_anchor
                    .rendered_line,
                item_offsets.len(),
                target_rendered_line,
                target_item_line_count,
                target_semantic_position,
            );
            if best
                .as_ref()
                .map(|(_, best_score)| score < *best_score)
                .unwrap_or(true)
            {
                best = Some((offset, score));
            }
        }
        return best;
    }

    find_rendered_transcript_split_sequence_match(
        layout,
        item_offsets,
        target_text,
        target_rendered_line,
        target_item_line_count,
        target_semantic_position,
    )
    .or_else(|| {
        find_rendered_transcript_merged_line_match(
            layout,
            item_offsets,
            target_text,
            target_rendered_line,
            target_item_line_count,
            target_semantic_position,
        )
    })
    .or_else(|| {
        find_rendered_transcript_boundary_spanning_match(
            layout,
            item_offsets,
            target_text,
            target_rendered_line,
            target_item_line_count,
            target_semantic_position,
        )
    })
}

fn find_rendered_transcript_split_sequence_match(
    layout: &DocumentLayout,
    item_offsets: &[usize],
    target_text: &str,
    target_rendered_line: usize,
    target_item_line_count: usize,
    target_semantic_position: TranscriptSemanticPosition,
) -> Option<(usize, usize)> {
    if target_text.is_empty() {
        return None;
    }

    let mut best = None;
    for start in 0..item_offsets.len() {
        let offset = item_offsets[start];
        let mut prefix = String::new();
        for next in start..item_offsets.len() {
            let piece = layout.line_text_at(item_offsets[next])?;
            if piece.is_empty() {
                break;
            }

            prefix.push_str(&piece);
            if !target_text.starts_with(&prefix) {
                break;
            }
            if prefix != target_text || next == start {
                continue;
            }

            let score = score_rendered_transcript_relative_position(
                layout
                    .line_anchor_at(offset)?
                    .transcript
                    .item_anchor
                    .rendered_line,
                item_offsets.len(),
                target_rendered_line,
                target_item_line_count,
                target_semantic_position,
            );
            if best
                .as_ref()
                .map(|(_, best_score)| score < *best_score)
                .unwrap_or(true)
            {
                best = Some((offset, score));
            }
            break;
        }
    }

    best
}

fn find_rendered_transcript_merged_line_match(
    layout: &DocumentLayout,
    item_offsets: &[usize],
    target_text: &str,
    target_rendered_line: usize,
    target_item_line_count: usize,
    target_semantic_position: TranscriptSemanticPosition,
) -> Option<(usize, usize)> {
    if target_text.is_empty() {
        return None;
    }

    let mut best = None;
    for &offset in item_offsets {
        let candidate_text = layout.line_text_at(offset)?;
        if candidate_text.is_empty()
            || !contains_rendered_transcript_merged_text(&candidate_text, target_text)
        {
            continue;
        }

        let score = score_rendered_transcript_relative_position(
            layout
                .line_anchor_at(offset)?
                .transcript
                .item_anchor
                .rendered_line,
            item_offsets.len(),
            target_rendered_line,
            target_item_line_count,
            target_semantic_position,
        );
        if best
            .as_ref()
            .map(|(_, best_score)| score < *best_score)
            .unwrap_or(true)
        {
            best = Some((offset, score));
        }
    }

    best
}

fn contains_rendered_transcript_merged_text(candidate_text: &str, target_text: &str) -> bool {
    if target_text.is_empty() {
        return false;
    }

    let candidate = candidate_text.chars().collect::<Vec<_>>();
    let target = target_text.chars().collect::<Vec<_>>();
    if target.is_empty() || candidate.len() < target.len() {
        return false;
    }

    for start in 0..=candidate.len() - target.len() {
        if candidate[start..start + target.len()] != target[..] {
            continue;
        }
        if rendered_transcript_match_has_boundaries(&candidate, &target, start) {
            return true;
        }
    }

    false
}

fn rendered_transcript_match_has_boundaries(
    candidate: &[char],
    target: &[char],
    start: usize,
) -> bool {
    let end = start + target.len();
    if rendered_transcript_target_needs_left_boundary(target)
        && start > 0
        && rendered_transcript_word_char(candidate[start - 1])
    {
        return false;
    }
    if rendered_transcript_target_needs_right_boundary(target)
        && end < candidate.len()
        && rendered_transcript_word_char(candidate[end])
    {
        return false;
    }

    true
}

fn rendered_transcript_target_needs_left_boundary(target: &[char]) -> bool {
    target
        .first()
        .copied()
        .is_some_and(rendered_transcript_word_char)
}

fn rendered_transcript_target_needs_right_boundary(target: &[char]) -> bool {
    target
        .last()
        .copied()
        .is_some_and(rendered_transcript_word_char)
}

fn rendered_transcript_word_char(character: char) -> bool {
    character.is_alphanumeric() || character == '_'
}

fn find_rendered_transcript_boundary_spanning_match(
    layout: &DocumentLayout,
    item_offsets: &[usize],
    target_text: &str,
    target_rendered_line: usize,
    target_item_line_count: usize,
    target_semantic_position: TranscriptSemanticPosition,
) -> Option<(usize, usize)> {
    if target_text.is_empty() {
        return None;
    }

    let mut best = None;
    let target = target_text.chars().collect::<Vec<_>>();
    for start in 0..item_offsets.len() {
        let candidate_offset = item_offsets[start];
        let mut pieces = Vec::new();
        for next in start..item_offsets.len() {
            let piece = layout.line_text_at(item_offsets[next])?;
            if piece.is_empty() {
                break;
            }

            pieces.push(piece.chars().collect::<Vec<_>>());
            if !rendered_transcript_boundary_spanning_sequence_matches(&target, &pieces) {
                continue;
            }

            let score = score_rendered_transcript_relative_position(
                layout
                    .line_anchor_at(candidate_offset)?
                    .transcript
                    .item_anchor
                    .rendered_line,
                item_offsets.len(),
                target_rendered_line,
                target_item_line_count,
                target_semantic_position,
            );
            if best
                .as_ref()
                .map(|(_, best_score)| score < *best_score)
                .unwrap_or(true)
            {
                best = Some((candidate_offset, score));
            }
        }
    }

    best
}

fn rendered_transcript_boundary_spanning_sequence_matches(
    target: &[char],
    pieces: &[Vec<char>],
) -> bool {
    if target.is_empty() || pieces.len() < 2 || pieces[0].is_empty() {
        return false;
    }

    (0..pieces[0].len()).any(|start| {
        rendered_transcript_boundary_spanning_target_match(target, pieces, 0, start, 0, false)
    })
}

fn rendered_transcript_boundary_spanning_target_match(
    target: &[char],
    pieces: &[Vec<char>],
    piece_index: usize,
    piece_offset: usize,
    target_offset: usize,
    crossed_boundary: bool,
) -> bool {
    let piece = &pieces[piece_index];
    let mut piece_cursor = piece_offset;
    let mut target_cursor = target_offset;
    while piece_cursor < piece.len()
        && target_cursor < target.len()
        && piece[piece_cursor] == target[target_cursor]
    {
        piece_cursor += 1;
        target_cursor += 1;
    }

    if target_cursor == target.len() {
        return crossed_boundary || piece_index > 0;
    }
    if piece_cursor < piece.len() || piece_index + 1 >= pieces.len() {
        return false;
    }
    if rendered_transcript_boundary_spanning_target_match(
        target,
        pieces,
        piece_index + 1,
        0,
        target_cursor,
        true,
    ) {
        return true;
    }

    let mut space_cursor = target_cursor;
    while space_cursor < target.len() && target[space_cursor].is_whitespace() {
        space_cursor += 1;
        if rendered_transcript_boundary_spanning_target_match(
            target,
            pieces,
            piece_index + 1,
            0,
            space_cursor,
            true,
        ) {
            return true;
        }
    }

    false
}

fn score_rendered_transcript_relative_position(
    candidate_rendered_line: usize,
    candidate_item_line_count: usize,
    target_rendered_line: usize,
    target_item_line_count: usize,
    target_semantic_position: TranscriptSemanticPosition,
) -> usize {
    let semantic_distance = score_transcript_semantic_position(
        transcript_semantic_position(
            LineAnchorKind::RenderedLine,
            candidate_rendered_line,
            candidate_item_line_count,
        ),
        target_semantic_position,
    );
    if candidate_item_line_count <= 1 || target_item_line_count <= 1 {
        return semantic_distance
            .saturating_mul(4)
            .saturating_add(candidate_rendered_line.abs_diff(target_rendered_line));
    }

    let left = candidate_rendered_line * (target_item_line_count - 1);
    let right = target_rendered_line * (candidate_item_line_count - 1);
    let relative_distance = left.abs_diff(right);
    let max_line_count = candidate_item_line_count.max(target_item_line_count).max(1);
    semantic_distance
        .saturating_mul(max_line_count.saturating_mul(max_line_count))
        .saturating_add(relative_distance)
}

fn score_transcript_semantic_position(
    candidate: TranscriptSemanticPosition,
    target: TranscriptSemanticPosition,
) -> usize {
    match (candidate, target) {
        (_, TranscriptSemanticPosition::Unknown) => 0,
        (left, right) if left == right => 0,
        (TranscriptSemanticPosition::WholeItem, TranscriptSemanticPosition::Start)
        | (TranscriptSemanticPosition::WholeItem, TranscriptSemanticPosition::End)
        | (TranscriptSemanticPosition::Start, TranscriptSemanticPosition::WholeItem)
        | (TranscriptSemanticPosition::End, TranscriptSemanticPosition::WholeItem) => 1,
        (TranscriptSemanticPosition::Middle, TranscriptSemanticPosition::Start)
        | (TranscriptSemanticPosition::Middle, TranscriptSemanticPosition::End)
        | (TranscriptSemanticPosition::Start, TranscriptSemanticPosition::Middle)
        | (TranscriptSemanticPosition::End, TranscriptSemanticPosition::Middle) => 1,
        _ => 2,
    }
}
