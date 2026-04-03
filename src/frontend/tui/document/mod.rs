mod cache;

use std::cmp::Ordering;

use ratatui::text::Line;

use self::cache::{
    DocumentAnchorRegion, DocumentLayout, DocumentLayoutCache, DocumentLayoutKey,
    DocumentLineAnchor, DocumentViewport, DocumentViewportAnchor, DocumentViewportCache,
    DocumentViewportKey, ManualDocumentScrollRestoreTarget,
};
use super::{
    Model,
    transcript::{self, LineAnchorKind},
};

pub(crate) use self::cache::{
    DocumentLayoutCache as LayoutCache, DocumentViewportAnchor as ViewportAnchor,
    DocumentViewportCache as ViewportCache, ManualDocumentScrollRestoreTarget as RestoreTarget,
};

const DOCUMENT_MOUSE_WHEEL_DELTA: isize = 3;

impl Model {
    pub(crate) fn document_mouse_wheel_delta() -> isize {
        DOCUMENT_MOUSE_WHEEL_DELTA
    }

    pub(crate) fn build_document_layout(&mut self) -> DocumentLayout {
        let key = self.current_document_layout_key();
        if self.document_layout_cache.valid && self.document_layout_cache.key == key {
            return self.document_layout_cache.layout.clone();
        }

        let composer_document = self.composer.render_document(self.palette);
        let transcript_lines = &self.transcript_render.lines;
        let transcript_plain_lines = &self.transcript_render.plain_lines;
        let transcript_anchors =
            document_anchors_for_transcript(&self.transcript_render.line_anchors);

        let extra_gap = usize::from(!transcript_lines.is_empty());
        let mut lines =
            Vec::with_capacity(transcript_lines.len() + extra_gap + composer_document.lines.len());
        let mut plain_lines = Vec::with_capacity(
            transcript_plain_lines.len() + extra_gap + composer_document.plain_lines.len(),
        );
        let mut anchors = Vec::with_capacity(
            transcript_anchors.len() + extra_gap + composer_document.anchors.len(),
        );

        lines.extend(transcript_lines.iter().cloned());
        plain_lines.extend(transcript_plain_lines.iter().cloned());
        anchors.extend(transcript_anchors);
        if !transcript_lines.is_empty() {
            lines.push(Line::raw(""));
            plain_lines.push(String::new());
            anchors.push(DocumentLineAnchor {
                region: DocumentAnchorRegion::TranscriptComposerGap,
                gap_index: 0,
                ..DocumentLineAnchor::default()
            });
        }

        let composer_start_line = lines.len();
        lines.extend(composer_document.lines.iter().cloned());
        plain_lines.extend(composer_document.plain_lines.iter().cloned());
        anchors.extend(document_anchors_for_composer(&composer_document.anchors));

        if lines.is_empty() {
            lines.push(Line::raw(""));
            plain_lines.push(String::new());
            anchors.push(DocumentLineAnchor::default());
        }

        let layout = DocumentLayout {
            composer_start_line,
            composer_line_count: composer_document.lines.len().max(1),
            cursor_x: composer_document.cursor_x,
            cursor_y: composer_start_line + composer_document.cursor_y,
            lines,
            plain_lines,
            anchors,
        };

        self.document_layout_cache = DocumentLayoutCache {
            key,
            layout: layout.clone(),
            valid: true,
        };
        layout
    }

    pub(crate) fn build_document_viewport(&mut self, layout: &DocumentLayout) -> DocumentViewport {
        let key = DocumentViewportKey {
            layout_key: self.current_document_layout_key(),
            offset: self.document_viewport_y,
            height: self.document_viewport_height(),
        };
        if self.document_viewport_cache.valid && self.document_viewport_cache.key == key {
            return self.document_viewport_cache.viewport.clone();
        }

        let (lines, plain_lines, resolved_offset) = visible_document_lines(
            &layout.lines,
            &layout.plain_lines,
            self.document_viewport_y,
            self.document_viewport_height(),
        );
        let viewport = DocumentViewport {
            lines,
            plain_lines,
            resolved_offset,
        };

        self.document_viewport_cache = DocumentViewportCache {
            key,
            viewport: viewport.clone(),
            valid: true,
        };
        viewport
    }

    pub(crate) fn document_viewport_height(&self) -> usize {
        if !self.has_window || self.height == 0 {
            return 0;
        }

        usize::from(self.height.max(1))
    }

    pub(crate) fn current_document_viewport_anchor(&mut self) -> Option<DocumentViewportAnchor> {
        let layout = self.build_document_layout();
        if layout.anchors.is_empty() {
            return None;
        }

        let offset =
            self.clamp_document_viewport_offset(self.document_viewport_y, layout.lines.len());
        let line_anchor = layout.anchors.get(offset).copied()?;
        let mut line_text = layout.plain_lines.get(offset).cloned().unwrap_or_default();
        if matches!(line_anchor.region, DocumentAnchorRegion::Transcript)
            && matches!(
                line_anchor.transcript.item_anchor.kind,
                LineAnchorKind::RenderedLine
            )
        {
            line_text = canonical_rendered_transcript_anchor_text(&line_text);
        }

        let transcript_item_line_count =
            if matches!(line_anchor.region, DocumentAnchorRegion::Transcript)
                && matches!(
                    line_anchor.transcript.item_anchor.kind,
                    LineAnchorKind::RenderedLine
                )
            {
                transcript_content_line_count_for_item(&layout, line_anchor.transcript.item_index)
            } else {
                0
            };

        Some(DocumentViewportAnchor {
            line_anchor,
            line_text,
            transcript_item_line_count,
        })
    }

    pub(crate) fn scroll_document_by(&mut self, lines: isize) {
        if lines == 0 {
            return;
        }

        let layout = self.build_document_layout();
        if layout.lines.is_empty() {
            self.document_viewport_y = 0;
            self.composer.set_viewport_offset(0);
            self.follow_bottom = true;
            self.manual_document_scroll = false;
            self.clear_manual_document_scroll_restore_target();
            return;
        }

        let current_offset =
            self.clamp_document_viewport_offset(self.document_viewport_y, layout.lines.len());
        let next_offset =
            self.clamp_document_viewport_offset_signed(current_offset, lines, layout.lines.len());
        if next_offset == current_offset {
            return;
        }

        self.start_manual_document_scroll_if_needed();
        let (restore_offset, restore_composer_offset, restore_follow_bottom) =
            self.manual_document_scroll_restore_offsets(&layout);

        if crossed_manual_document_scroll_restore_target(
            current_offset,
            next_offset,
            restore_offset,
        ) {
            self.document_viewport_y = restore_offset;
            self.composer.set_viewport_offset(restore_composer_offset);
            self.follow_bottom = restore_follow_bottom;
            self.manual_document_scroll = false;
            self.clear_manual_document_scroll_restore_target();
            return;
        }

        self.document_viewport_y = next_offset;
        self.composer
            .set_viewport_offset(self.current_composer_viewport_offset(&layout, next_offset));
        self.follow_bottom = false;
        self.manual_document_scroll = true;
    }

    pub(crate) fn sync_document_viewport_to_bottom(&mut self) {
        let layout = self.build_document_layout();
        let (document_offset, composer_offset) = self.bottom_follow_viewport_offsets(&layout);
        self.document_viewport_y = document_offset;
        self.composer.set_viewport_offset(composer_offset);
        self.manual_document_scroll = false;
        self.clear_manual_document_scroll_restore_target();
    }

    pub(crate) fn sync_document_viewport_for_composer_cursor(&mut self) {
        let layout = self.build_document_layout();
        if self.follow_bottom {
            self.sync_document_viewport_to_bottom();
            return;
        }

        let mut current_offset =
            self.clamp_document_viewport_offset(self.document_viewport_y, layout.lines.len());
        let viewport_height = self.document_viewport_height();
        if viewport_height == 0 {
            self.document_viewport_y = 0;
            self.composer.set_viewport_offset(0);
            return;
        }

        match layout.cursor_y.cmp(&current_offset) {
            Ordering::Less => current_offset = layout.cursor_y,
            Ordering::Greater if layout.cursor_y >= current_offset + viewport_height => {
                current_offset = layout.cursor_y - viewport_height + 1;
            }
            _ => {}
        }

        self.document_viewport_y =
            self.clamp_document_viewport_offset(current_offset, layout.lines.len());
        self.composer.set_viewport_offset(
            self.current_composer_viewport_offset(&layout, self.document_viewport_y),
        );
        self.manual_document_scroll = false;
        self.clear_manual_document_scroll_restore_target();
    }

    pub(crate) fn sync_document_viewport_preserving_position(&mut self) {
        let layout = self.build_document_layout();
        if layout.lines.is_empty() {
            self.document_viewport_y = 0;
            self.composer.set_viewport_offset(0);
            return;
        }

        self.document_viewport_y =
            self.clamp_document_viewport_offset(self.document_viewport_y, layout.lines.len());
        self.composer.set_viewport_offset(
            self.current_composer_viewport_offset(&layout, self.document_viewport_y),
        );
    }

    pub(crate) fn sync_document_viewport_for_viewport_anchor(
        &mut self,
        anchor: &DocumentViewportAnchor,
    ) {
        let layout = self.build_document_layout();
        if layout.lines.is_empty() {
            self.document_viewport_y = 0;
            self.composer.set_viewport_offset(0);
            return;
        }

        let Some(offset) = find_document_offset_for_viewport_anchor(&layout, anchor) else {
            self.sync_document_viewport_preserving_position();
            return;
        };

        self.document_viewport_y = self.clamp_document_viewport_offset(offset, layout.lines.len());
        self.composer.set_viewport_offset(
            self.current_composer_viewport_offset(&layout, self.document_viewport_y),
        );
    }

    pub(crate) fn sync_document_viewport_for_composer_page(&mut self) {
        let layout = self.build_document_layout();
        let max_offset = layout
            .composer_line_count
            .saturating_sub(self.composer.viewport_height().max(1));
        if self.composer.viewport_offset() > max_offset {
            self.composer.set_viewport_offset(max_offset);
        }

        if layout.composer_line_count <= self.composer.viewport_height().max(1) {
            self.sync_document_viewport_for_composer_cursor();
            return;
        }

        self.document_viewport_y = self.clamp_document_viewport_offset(
            layout.composer_start_line + self.composer.viewport_offset(),
            layout.lines.len(),
        );
        self.manual_document_scroll = false;
        self.clear_manual_document_scroll_restore_target();
    }

    pub(crate) fn sync_document_viewport_after_composer_interaction(
        &mut self,
        old_value: &str,
        old_line: usize,
        old_column: usize,
    ) {
        if self.composer.value() != old_value {
            if self.manual_document_scroll {
                self.restore_from_manual_document_scroll();
                return;
            }

            if self.follow_bottom {
                self.sync_document_viewport_to_bottom();
                return;
            }

            self.sync_document_viewport_for_composer_cursor();
            return;
        }

        if self.composer.line() != old_line || self.composer.column() != old_column {
            self.follow_bottom = self.composer_at_bottom_follow_anchor();
            if self.follow_bottom {
                self.sync_document_viewport_to_bottom();
                return;
            }

            self.sync_document_viewport_for_composer_cursor();
            return;
        }

        if self.follow_bottom {
            self.sync_document_viewport_to_bottom();
            return;
        }

        if self.manual_document_scroll {
            self.sync_document_viewport_preserving_position();
            return;
        }

        self.sync_document_viewport_for_composer_cursor();
    }

    pub(crate) fn sync_document_viewport_after_transcript_refresh(
        &mut self,
        preserved_anchor: Option<DocumentViewportAnchor>,
    ) {
        if self.follow_bottom {
            self.sync_document_viewport_to_bottom();
            return;
        }

        if self.manual_document_scroll {
            if let Some(anchor) = preserved_anchor.as_ref() {
                self.sync_document_viewport_for_viewport_anchor(anchor);
            } else {
                self.sync_document_viewport_preserving_position();
            }
            self.complete_manual_document_scroll_if_restored();
            return;
        }

        self.sync_document_viewport_for_composer_cursor();
    }

    pub(crate) fn clamp_document_viewport_offset(
        &self,
        offset: usize,
        total_lines: usize,
    ) -> usize {
        let viewport_height = self.document_viewport_height();
        if viewport_height == 0 || total_lines <= viewport_height {
            return 0;
        }

        offset.min(total_lines - viewport_height)
    }

    pub(crate) fn current_document_layout_key(&self) -> DocumentLayoutKey {
        DocumentLayoutKey {
            transcript_render_version: self.transcript_render_version,
            palette_version: self.palette_version,
            composer_value: self.composer.value().to_string(),
            composer_width: self.composer.content_width(),
            composer_prompt: self.composer.prompt().to_string(),
            composer_placeholder: self.composer.placeholder().to_string(),
            composer_line: self.composer.line(),
            composer_column: self.composer.column(),
        }
    }

    pub(crate) fn clear_manual_document_scroll_restore_target(&mut self) {
        self.scroll_restore_target = ManualDocumentScrollRestoreTarget::None;
        self.scroll_restore_anchor = DocumentViewportAnchor::default();
    }

    pub(crate) fn start_manual_document_scroll_if_needed(&mut self) {
        if self.manual_document_scroll {
            return;
        }

        if self.follow_bottom {
            self.scroll_restore_target = ManualDocumentScrollRestoreTarget::BottomFollow;
            return;
        }

        if let Some(anchor) = self.current_document_viewport_anchor() {
            self.scroll_restore_target = ManualDocumentScrollRestoreTarget::ComposerCursor;
            self.scroll_restore_anchor = anchor;
            return;
        }

        self.scroll_restore_target = ManualDocumentScrollRestoreTarget::ComposerCursor;
    }

    pub(crate) fn manual_document_scroll_restore_offsets(
        &self,
        layout: &DocumentLayout,
    ) -> (usize, usize, bool) {
        match self.scroll_restore_target {
            ManualDocumentScrollRestoreTarget::BottomFollow => {
                let (document_offset, composer_offset) =
                    self.bottom_follow_viewport_offsets(layout);
                (document_offset, composer_offset, true)
            }
            _ => {
                if let Some(offset) =
                    find_document_offset_for_viewport_anchor(layout, &self.scroll_restore_anchor)
                {
                    let document_offset =
                        self.clamp_document_viewport_offset(offset, layout.lines.len());
                    if self.document_offset_keeps_cursor_visible(layout, document_offset) {
                        let composer_offset =
                            self.current_composer_viewport_offset(layout, document_offset);
                        return (document_offset, composer_offset, false);
                    }
                }

                let (document_offset, composer_offset) =
                    self.composer_cursor_restore_viewport_offsets(layout);
                (document_offset, composer_offset, false)
            }
        }
    }

    pub(crate) fn restore_from_manual_document_scroll(&mut self) {
        let layout = self.build_document_layout();
        let (document_offset, composer_offset, follow_bottom) =
            self.manual_document_scroll_edit_restore_offsets(&layout);
        self.document_viewport_y = document_offset;
        self.composer.set_viewport_offset(composer_offset);
        self.follow_bottom = follow_bottom;
        self.manual_document_scroll = false;
        self.clear_manual_document_scroll_restore_target();
    }

    pub(crate) fn complete_manual_document_scroll_if_restored(&mut self) {
        if !self.manual_document_scroll
            || self.scroll_restore_target == ManualDocumentScrollRestoreTarget::None
        {
            return;
        }

        let layout = self.build_document_layout();
        let (restore_offset, restore_composer_offset, restore_follow_bottom) =
            self.manual_document_scroll_restore_offsets(&layout);
        if self.document_viewport_y != restore_offset
            || self.composer.viewport_offset() != restore_composer_offset
        {
            return;
        }

        self.follow_bottom = restore_follow_bottom;
        self.manual_document_scroll = false;
        self.clear_manual_document_scroll_restore_target();
    }

    fn clamp_document_viewport_offset_signed(
        &self,
        offset: usize,
        delta: isize,
        total_lines: usize,
    ) -> usize {
        let next = if delta.is_negative() {
            offset.saturating_sub(delta.unsigned_abs())
        } else {
            offset.saturating_add(delta as usize)
        };

        self.clamp_document_viewport_offset(next, total_lines)
    }

    fn document_bottom_offset(&self, total_lines: usize) -> usize {
        self.clamp_document_viewport_offset(total_lines, total_lines)
    }

    fn current_composer_viewport_offset(
        &self,
        layout: &DocumentLayout,
        document_viewport_y: usize,
    ) -> usize {
        let viewport_height = self.composer.viewport_height().max(1);
        if layout.composer_line_count <= viewport_height {
            return 0;
        }

        let offset = document_viewport_y.saturating_sub(layout.composer_start_line);
        offset.min(layout.composer_line_count - viewport_height)
    }

    fn bottom_follow_viewport_offsets(&self, layout: &DocumentLayout) -> (usize, usize) {
        if self.composer.value().is_empty() {
            let viewport_height = self.document_viewport_height();
            if viewport_height == 0 {
                return (0, 0);
            }

            let document_offset = self.clamp_document_viewport_offset(
                layout.cursor_y.saturating_sub(viewport_height - 1),
                layout.lines.len(),
            );
            return (document_offset, 0);
        }

        (
            self.document_bottom_offset(layout.lines.len()),
            self.composer.bottom_viewport_offset(),
        )
    }

    fn composer_cursor_restore_viewport_offsets(&self, layout: &DocumentLayout) -> (usize, usize) {
        let viewport_height = self.document_viewport_height();
        if viewport_height == 0 {
            return (0, 0);
        }

        let document_offset = self.clamp_document_viewport_offset(
            layout.cursor_y.saturating_sub(viewport_height - 1),
            layout.lines.len(),
        );
        let composer_offset = self.current_composer_viewport_offset(layout, document_offset);
        (document_offset, composer_offset)
    }

    fn document_offset_keeps_cursor_visible(
        &self,
        layout: &DocumentLayout,
        document_offset: usize,
    ) -> bool {
        let viewport_height = self.document_viewport_height();
        if viewport_height == 0 {
            return true;
        }

        let document_offset =
            self.clamp_document_viewport_offset(document_offset, layout.lines.len());
        layout.cursor_y >= document_offset && layout.cursor_y < document_offset + viewport_height
    }

    pub(crate) fn composer_at_bottom_follow_anchor(&self) -> bool {
        if self.composer.value().is_empty() {
            return true;
        }

        let lines = self.composer.value().split('\n').collect::<Vec<_>>();
        let Some(last_line) = lines.last() else {
            return true;
        };

        self.composer.line() == lines.len().saturating_sub(1)
            && self.composer.column() == last_line.chars().count()
    }

    fn manual_document_scroll_edit_restore_offsets(
        &self,
        layout: &DocumentLayout,
    ) -> (usize, usize, bool) {
        match self.scroll_restore_target {
            ManualDocumentScrollRestoreTarget::BottomFollow => {
                let (document_offset, composer_offset) =
                    self.bottom_follow_viewport_offsets(layout);
                (document_offset, composer_offset, true)
            }
            _ => {
                let (document_offset, composer_offset) =
                    self.composer_cursor_restore_viewport_offsets(layout);
                (document_offset, composer_offset, false)
            }
        }
    }
}

fn document_anchors_for_transcript(
    line_anchors: &[transcript::LineAnchor],
) -> Vec<DocumentLineAnchor> {
    line_anchors
        .iter()
        .copied()
        .map(|transcript| DocumentLineAnchor {
            region: DocumentAnchorRegion::Transcript,
            transcript,
            ..DocumentLineAnchor::default()
        })
        .collect()
}

fn document_anchors_for_composer(
    line_anchors: &[super::composer::LineAnchor],
) -> Vec<DocumentLineAnchor> {
    line_anchors
        .iter()
        .copied()
        .map(|composer| DocumentLineAnchor {
            region: DocumentAnchorRegion::Composer,
            composer,
            ..DocumentLineAnchor::default()
        })
        .collect()
}

fn visible_document_lines(
    lines: &[Line<'static>],
    plain_lines: &[String],
    offset: usize,
    height: usize,
) -> (Vec<Line<'static>>, Vec<String>, usize) {
    if lines.is_empty() {
        return (vec![Line::raw("")], vec![String::new()], 0);
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

fn canonical_rendered_transcript_anchor_text(text: &str) -> String {
    if text.trim().is_empty() {
        String::new()
    } else {
        text.to_string()
    }
}

fn find_document_offset_for_viewport_anchor(
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

fn find_document_anchor_offset(
    layout: &DocumentLayout,
    anchor: DocumentLineAnchor,
) -> Option<usize> {
    let mut best = None;
    for (index, candidate) in layout.anchors.iter().copied().enumerate() {
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

fn transcript_content_line_count_for_item(layout: &DocumentLayout, item_index: usize) -> usize {
    layout
        .anchors
        .iter()
        .filter(|anchor| {
            matches!(anchor.region, DocumentAnchorRegion::Transcript)
                && anchor.transcript.item_index == item_index
                && !matches!(anchor.transcript.item_anchor.kind, LineAnchorKind::ItemGap)
        })
        .count()
}

fn find_document_offset_for_rendered_transcript_anchor(
    layout: &DocumentLayout,
    anchor: &DocumentViewportAnchor,
) -> Option<usize> {
    let item_index = anchor.line_anchor.transcript.item_index;
    let item_offsets = transcript_content_line_offsets_for_item(layout, item_index);
    if item_offsets.is_empty() {
        return None;
    }

    let target_rendered_line = anchor.line_anchor.transcript.item_anchor.rendered_line;
    let exact = find_rendered_transcript_text_match(
        layout,
        &item_offsets,
        &anchor.line_text,
        target_rendered_line,
        anchor.transcript_item_line_count,
        true,
    );
    let fuzzy = find_rendered_transcript_text_match(
        layout,
        &item_offsets,
        &anchor.line_text,
        target_rendered_line,
        anchor.transcript_item_line_count,
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
                layout.anchors[best].transcript.item_anchor.rendered_line,
                item_offsets.len(),
                target_rendered_line,
                anchor.transcript_item_line_count,
            );
            for offset in item_offsets.into_iter().skip(1) {
                let score = score_rendered_transcript_relative_position(
                    layout.anchors[offset].transcript.item_anchor.rendered_line,
                    transcript_content_line_count_for_item(layout, item_index),
                    target_rendered_line,
                    anchor.transcript_item_line_count,
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
    layout
        .anchors
        .iter()
        .enumerate()
        .filter_map(|(index, anchor)| {
            (matches!(anchor.region, DocumentAnchorRegion::Transcript)
                && anchor.transcript.item_index == item_index
                && !matches!(anchor.transcript.item_anchor.kind, LineAnchorKind::ItemGap))
            .then_some(index)
        })
        .collect()
}

fn find_rendered_transcript_text_match(
    layout: &DocumentLayout,
    item_offsets: &[usize],
    target_text: &str,
    target_rendered_line: usize,
    target_item_line_count: usize,
    exact: bool,
) -> Option<(usize, usize)> {
    if exact {
        let mut best = None;
        for &offset in item_offsets {
            let candidate_text =
                canonical_rendered_transcript_anchor_text(&layout.plain_lines[offset]);
            if candidate_text != target_text {
                continue;
            }

            let score = score_rendered_transcript_relative_position(
                layout.anchors[offset].transcript.item_anchor.rendered_line,
                item_offsets.len(),
                target_rendered_line,
                target_item_line_count,
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
    )
    .or_else(|| {
        find_rendered_transcript_merged_line_match(
            layout,
            item_offsets,
            target_text,
            target_rendered_line,
            target_item_line_count,
        )
    })
    .or_else(|| {
        find_rendered_transcript_boundary_spanning_match(
            layout,
            item_offsets,
            target_text,
            target_rendered_line,
            target_item_line_count,
        )
    })
}

fn find_rendered_transcript_split_sequence_match(
    layout: &DocumentLayout,
    item_offsets: &[usize],
    target_text: &str,
    target_rendered_line: usize,
    target_item_line_count: usize,
) -> Option<(usize, usize)> {
    if target_text.is_empty() {
        return None;
    }

    let mut best = None;
    for start in 0..item_offsets.len() {
        let offset = item_offsets[start];
        let mut prefix = String::new();
        for next in start..item_offsets.len() {
            let piece = &layout.plain_lines[item_offsets[next]];
            if piece.is_empty() {
                break;
            }

            prefix.push_str(piece);
            if !target_text.starts_with(&prefix) {
                break;
            }
            if prefix != target_text || next == start {
                continue;
            }

            let score = score_rendered_transcript_relative_position(
                layout.anchors[offset].transcript.item_anchor.rendered_line,
                item_offsets.len(),
                target_rendered_line,
                target_item_line_count,
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
) -> Option<(usize, usize)> {
    if target_text.is_empty() {
        return None;
    }

    let mut best = None;
    for &offset in item_offsets {
        let candidate_text = &layout.plain_lines[offset];
        if candidate_text.is_empty()
            || !contains_rendered_transcript_merged_text(candidate_text, target_text)
        {
            continue;
        }

        let score = score_rendered_transcript_relative_position(
            layout.anchors[offset].transcript.item_anchor.rendered_line,
            item_offsets.len(),
            target_rendered_line,
            target_item_line_count,
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
            let piece = &layout.plain_lines[item_offsets[next]];
            if piece.is_empty() {
                break;
            }

            pieces.push(piece.chars().collect::<Vec<_>>());
            if !rendered_transcript_boundary_spanning_sequence_matches(&target, &pieces) {
                continue;
            }

            let score = score_rendered_transcript_relative_position(
                layout.anchors[candidate_offset]
                    .transcript
                    .item_anchor
                    .rendered_line,
                item_offsets.len(),
                target_rendered_line,
                target_item_line_count,
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
) -> usize {
    if candidate_item_line_count <= 1 || target_item_line_count <= 1 {
        return candidate_rendered_line.abs_diff(target_rendered_line);
    }

    let left = candidate_rendered_line * (target_item_line_count - 1);
    let right = target_rendered_line * (candidate_item_line_count - 1);
    left.abs_diff(right)
}

fn crossed_manual_document_scroll_restore_target(
    current_offset: usize,
    next_offset: usize,
    restore_offset: usize,
) -> bool {
    match current_offset.cmp(&restore_offset) {
        Ordering::Less => next_offset >= restore_offset,
        Ordering::Greater => next_offset <= restore_offset,
        Ordering::Equal => false,
    }
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyEvent};
    use ratatui::text::Line;
    use std::hint::black_box;

    use super::*;
    use crate::frontend::tui::{HeroOptions, Model, Sender, theme::default_palette};

    #[test]
    fn build_document_layout_combines_transcript_and_composer_snapshots() {
        let mut model = ready_document_model(20, 4);
        model
            .transcript_mut()
            .append_message(Sender::Assistant, "history");
        model.sync_transcript_render();
        model.composer_mut().set_text_for_test("x");
        model.sync_composer_height();

        let layout = model.build_document_layout();

        assert_eq!(
            layout.plain_lines,
            vec!["history".to_string(), String::new(), "┃ x".to_string(),]
        );
        assert_eq!(layout.composer_start_line, 2);
        assert_eq!(layout.composer_line_count, 1);
        assert_eq!(layout.cursor_x, 3);
        assert_eq!(layout.cursor_y, 2);
    }

    #[test]
    fn visible_document_lines_tracks_cursor_visibility() {
        let layout = DocumentLayout {
            lines: vec![Line::raw("a"), Line::raw("b"), Line::raw("c")],
            plain_lines: vec!["a".to_string(), "b".to_string(), "c".to_string()],
            cursor_x: 4,
            cursor_y: 1,
            ..DocumentLayout::default()
        };

        let (visible_lines, _, visible_offset) =
            visible_document_lines(&layout.lines, &layout.plain_lines, 0, 2);
        assert_eq!(visible_lines.len(), 2);
        assert_eq!(visible_offset, 0);
        assert!(cursor_visible_in_document_viewport(
            &layout,
            visible_offset,
            visible_lines.len()
        ));

        let (hidden_lines, _, hidden_offset) =
            visible_document_lines(&layout.lines, &layout.plain_lines, 2, 1);
        assert_eq!(hidden_lines.len(), 1);
        assert_eq!(hidden_offset, 2);
        assert!(!cursor_visible_in_document_viewport(
            &layout,
            hidden_offset,
            hidden_lines.len()
        ));
    }

    #[test]
    fn scroll_document_by_restores_composer_viewport_when_crossing_restore_target() {
        let mut model = ready_document_model(20, 4);
        model.composer_mut().set_text_for_test("1\n2\n3\n4\n5\n6");
        model.sync_composer_height();
        model.document_viewport_y = 0;
        model.composer.set_viewport_offset(0);
        model.follow_bottom = false;
        model.manual_document_scroll = true;
        model.scroll_restore_target = RestoreTarget::ComposerCursor;
        model.scroll_restore_anchor = ViewportAnchor::default();

        model.scroll_document_by(Model::document_mouse_wheel_delta());

        assert!(!model.follow_bottom);
        assert!(!model.manual_document_scroll);
        assert_eq!(model.document_viewport_y, 2);
        assert_eq!(model.composer.viewport_offset(), 2);
        assert_eq!(model.scroll_restore_target, RestoreTarget::None);
    }

    #[test]
    fn moving_cursor_back_to_draft_end_restores_bottom_follow() {
        let mut model = ready_document_model(20, 4);
        model.composer_mut().set_text_for_test("1\n2\n3\n4\n5\n6");
        model.sync_composer_height();
        model.composer_mut().handle_key(KeyEvent::from(KeyCode::Up));
        model.composer_mut().handle_key(KeyEvent::from(KeyCode::Up));
        model.follow_bottom = false;
        model.manual_document_scroll = false;
        model.sync_document_viewport_for_composer_cursor();

        let old_value = model.composer_text().to_string();
        let old_line = model.composer.line();
        let old_column = model.composer.column();

        model.composer_mut().move_to_end();
        model.sync_composer_height();
        let layout = model.build_document_layout();
        let (expected_document_offset, expected_composer_offset) =
            model.bottom_follow_viewport_offsets(&layout);

        model.sync_document_viewport_after_composer_interaction(&old_value, old_line, old_column);

        assert!(model.follow_bottom);
        assert!(!model.manual_document_scroll);
        assert_eq!(model.document_viewport_y, expected_document_offset);
        assert_eq!(model.composer.viewport_offset(), expected_composer_offset);
    }

    #[test]
    fn transcript_refresh_keeps_manual_scrollback_before_restore_target() {
        let mut model = ready_document_model(20, 4);
        for index in 0..8 {
            model
                .transcript_mut()
                .append_message(Sender::Assistant, format!("history {index}"));
        }
        model.sync_transcript_render();

        model.follow_bottom = false;
        model.manual_document_scroll = true;
        model.document_viewport_y = 0;
        model.composer.set_viewport_offset(0);
        model.scroll_restore_target = RestoreTarget::BottomFollow;

        let anchor = model
            .current_document_viewport_anchor()
            .expect("manual scrollback should have a viewport anchor");
        let original_document_offset = model.document_viewport_y;
        let original_composer_offset = model.composer.viewport_offset();

        model
            .transcript_mut()
            .append_message(Sender::Assistant, "new history line");
        model.sync_transcript_render();
        model.sync_document_viewport_after_transcript_refresh(Some(anchor));

        assert!(!model.follow_bottom);
        assert!(model.manual_document_scroll);
        assert_eq!(model.document_viewport_y, original_document_offset);
        assert_eq!(model.composer.viewport_offset(), original_composer_offset);
        assert_eq!(model.scroll_restore_target, RestoreTarget::BottomFollow);
    }

    #[test]
    #[ignore = "performance smoke test"]
    fn document_layout_and_viewport_perf_smoke() {
        let mut model = ready_document_model(80, 12);
        for index in 0..24 {
            let sender = if index % 3 == 0 {
                Sender::User
            } else {
                Sender::Assistant
            };
            model.transcript_mut().append_message(
                sender,
                format!(
                    "message {index:02}: keep scrollback anchored while the composer draft keeps growing"
                ),
            );
        }
        model.sync_transcript_render();
        model.composer_mut().set_text_for_test(
            "draft heading\nsoft wrap should stay stable under repeated rendering\n中文输入继续参与宽度计算\ncursor placement should stay near the bottom",
        );
        model.sync_composer_height();

        for _ in 0..128 {
            let layout = black_box(model.build_document_layout());
            black_box(model.build_document_viewport(&layout));
            model.document_layout_cache.valid = false;
            model.document_viewport_cache.valid = false;
        }
    }

    fn ready_document_model(width: u16, height: u16) -> Model {
        let mut model = Model::new(HeroOptions::default());
        model.transcript_mut().clear();
        model.sync_transcript_render();
        model.set_window(width, height);
        model.set_palette(default_palette(), true);
        model
    }

    fn cursor_visible_in_document_viewport(
        layout: &DocumentLayout,
        resolved_offset: usize,
        visible_line_count: usize,
    ) -> bool {
        layout.cursor_y >= resolved_offset
            && layout.cursor_y < resolved_offset.saturating_add(visible_line_count)
    }
}
