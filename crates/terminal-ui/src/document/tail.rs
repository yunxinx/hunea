use std::rc::Rc;

use ratatui::text::Line;

use crate::{
    Model,
    command_panel::CommandPanelRenderResult,
    composer,
    context_budget::ContextBudgetRenderResult,
    frame_time::FrameRenderContext,
    inline_panel::InlinePanelRenderResult,
    model_panel::ModelPanelRenderResult,
    selection::SelectableLineRange,
    status_line::{
        StatusLineRenderResult, status_line_gap_before as configured_status_line_gap_before,
        status_line_pair_height,
    },
    stream_activity::StreamActivityFrameKey,
    style_mode::StyleMode,
    tool_approval_panel::ToolApprovalPanelRenderResult,
};

use super::{DocumentAnchorRegion, DocumentLineAnchor, slot_frame::SlotFrame};

/// `DocumentStableTailLayoutKey` 描述稳定 tail rows 的命中条件。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct DocumentStableTailLayoutKey {
    pub(crate) transcript_has_content: bool,
    pub(crate) palette_version: usize,
    pub(crate) style_mode: StyleMode,
    pub(crate) document_width: u16,
    pub(crate) document_viewport_height: usize,
    pub(crate) composer_viewport_height: usize,
    pub(crate) composer_content_revision: usize,
    pub(crate) composer_presentation_revision: usize,
    pub(crate) composer_width: usize,
    pub(crate) command_panel_selected: usize,
    pub(crate) command_panel_scroll: usize,
    pub(crate) tool_approval_panel_active: bool,
    pub(crate) tool_approval_panel_selected: usize,
    pub(crate) tool_approval_panel_revision: usize,
    pub(crate) model_panel_active: bool,
    pub(crate) model_panel_provider_index: usize,
    pub(crate) model_panel_model_index: usize,
    pub(crate) model_panel_scroll: usize,
    pub(crate) model_panel_revision: usize,
    pub(crate) context_budget_active: bool,
    pub(crate) context_budget_revision: usize,
    pub(crate) selected_model: Option<String>,
    pub(crate) status_line_config: u8,
    pub(crate) status_line_2_config: u8,
    pub(crate) status_line_revision: usize,
}

/// `DocumentTailLayoutKey` 描述最终 tail view 的命中条件。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct DocumentTailLayoutKey {
    pub(crate) stable: DocumentStableTailLayoutKey,
    pub(crate) composer_cursor_revision: usize,
    pub(crate) stream_activity_frame: StreamActivityFrameKey,
}

#[derive(Debug, Default)]
struct DocumentTailRows {
    lines: Vec<Line<'static>>,
    text_lines: Vec<String>,
    anchors: Vec<DocumentLineAnchor>,
    selectable: Vec<SelectableLineRange>,
}

impl DocumentTailRows {
    fn len(&self) -> usize {
        self.lines.len()
    }

    fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    fn push_empty(&mut self) {
        self.lines.push(Line::raw(""));
        self.text_lines.push(String::new());
        self.anchors.push(DocumentLineAnchor::default());
        self.selectable.push(SelectableLineRange::default());
    }
}

/// `DocumentStableTailLayout` 保存 activity frame 之间可共享的 tail rows。
#[derive(Debug, Default)]
pub(crate) struct DocumentStableTailLayout {
    rows: DocumentTailRows,
    composer_document: Option<composer::ComposerDocumentSnapshot>,
    composer_insert_at: usize,
    activity_insert_at: usize,
    adds_activity_composer_gap: bool,
    composer_slot: SlotFrame,
    fallback_cursor_x: u16,
    fallback_cursor_y: usize,
}

#[derive(Debug, Default)]
struct DocumentTailActivitySegment {
    rows: DocumentTailRows,
}

/// `DocumentTailLayout` 把稳定 tail rows 与当前 activity segment 组合成统一只读 view。
#[derive(Debug, Default)]
pub(crate) struct DocumentTailLayout {
    stable: Rc<DocumentStableTailLayout>,
    activity: DocumentTailActivitySegment,
    pub(crate) composer_slot: SlotFrame,
    pub(crate) cursor_x: u16,
    pub(crate) cursor_y: usize,
    stable_cursor_y: usize,
}

enum DocumentTailLineSource<'a> {
    Rows(&'a DocumentTailRows, usize),
    Composer(&'a composer::ComposerDocumentSnapshot, usize),
}

/// `DocumentStableTailLayoutCache` 缓存最近一次稳定 tail rows。
#[derive(Debug, Clone, Default)]
pub(crate) struct DocumentStableTailLayoutCache {
    pub(crate) key: DocumentStableTailLayoutKey,
    pub(crate) tail: Rc<DocumentStableTailLayout>,
    pub(crate) valid: bool,
}

/// `DocumentTailLayoutCache` 缓存最近一次最终 tail view。
#[derive(Debug, Clone, Default)]
pub(crate) struct DocumentTailLayoutCache {
    pub(crate) key: DocumentTailLayoutKey,
    pub(crate) tail: Rc<DocumentTailLayout>,
    pub(crate) valid: bool,
}

/// `DocumentStableTailLayoutInput` 描述稳定 tail 渲染真正需要的输入。
#[derive(Debug)]
pub(crate) struct DocumentStableTailLayoutInput {
    pub(crate) transcript_has_content: bool,
    pub(crate) composer_document: composer::ComposerDocumentSnapshot,
    pub(crate) command_panel: CommandPanelRenderResult,
    pub(crate) tool_approval_panel: ToolApprovalPanelRenderResult,
    pub(crate) model_panel: ModelPanelRenderResult,
    pub(crate) context_budget: ContextBudgetRenderResult,
    pub(crate) status_line_gap_before: usize,
    pub(crate) status_line: StatusLineRenderResult,
    pub(crate) status_line_2: StatusLineRenderResult,
}

impl Model {
    pub(crate) fn build_document_tail_layout(
        &mut self,
        context: FrameRenderContext,
    ) -> Rc<DocumentTailLayout> {
        let key = self.current_document_tail_layout_key(context);
        if self.document_runtime.tail_layout_cache.valid
            && self.document_runtime.tail_layout_cache.key == key
        {
            return Rc::clone(&self.document_runtime.tail_layout_cache.tail);
        }

        let stable = self.build_document_stable_tail_layout(key.stable.clone());
        let activity = compose_document_tail_activity_segment(
            self.current_stream_activity_render_result_at(context.now()),
            stable.adds_activity_composer_gap,
        );
        let cached_cursor = self
            .document_runtime
            .tail_layout_cache
            .valid
            .then_some(&self.document_runtime.tail_layout_cache)
            .filter(|cache| {
                cache.key.stable == key.stable
                    && cache.key.composer_cursor_revision == key.composer_cursor_revision
            })
            .map(|cache| (cache.tail.cursor_x, cache.tail.stable_cursor_y));
        let (cursor_x, cursor_y) =
            cached_cursor.unwrap_or_else(|| self.current_document_tail_cursor(&stable));
        let tail = Rc::new(compose_document_tail_layout(
            stable, activity, cursor_x, cursor_y,
        ));
        self.document_runtime.tail_layout_cache = DocumentTailLayoutCache {
            key,
            tail: Rc::clone(&tail),
            valid: true,
        };
        tail
    }

    pub(crate) fn current_document_tail_layout_key(
        &self,
        context: FrameRenderContext,
    ) -> DocumentTailLayoutKey {
        DocumentTailLayoutKey {
            stable: self.current_document_stable_tail_layout_key(),
            composer_cursor_revision: self.composer.cursor_revision(),
            stream_activity_frame: self.stream_activity_frame_key(context.now()),
        }
    }

    fn current_document_stable_tail_layout_key(&self) -> DocumentStableTailLayoutKey {
        DocumentStableTailLayoutKey {
            transcript_has_content: self.transcript_render.line_count > 0,
            palette_version: self.palette_version,
            style_mode: self.style_mode,
            document_width: self.width,
            document_viewport_height: self.document_viewport_height(),
            composer_viewport_height: self.composer.viewport_height(),
            composer_content_revision: self.composer.content_revision(),
            composer_presentation_revision: self.composer.presentation_revision(),
            composer_width: self.composer.content_width(),
            command_panel_selected: self.command_panel_selected,
            command_panel_scroll: self.command_panel_scroll,
            tool_approval_panel_active: self.tool_approval_panel_active(),
            tool_approval_panel_selected: self.tool_approval_panel.selected,
            tool_approval_panel_revision: self.tool_approval_panel_revision,
            model_panel_active: self.model_panel_active(),
            model_panel_provider_index: self.model_panel.provider_index,
            model_panel_model_index: self.model_panel.model_index,
            model_panel_scroll: self.model_panel.scroll,
            model_panel_revision: self.model_panel.revision,
            context_budget_active: self.context_budget_active(),
            context_budget_revision: self
                .context_budget
                .as_ref()
                .map(|state| state.revision)
                .unwrap_or_default(),
            selected_model: self
                .selected_model
                .as_ref()
                .map(|model| model.display_name()),
            status_line_config: self.status_line_config_bits(),
            status_line_2_config: self.status_line_2_config_bits(),
            status_line_revision: self.status_line_revision(),
        }
    }

    fn build_document_stable_tail_layout(
        &mut self,
        key: DocumentStableTailLayoutKey,
    ) -> Rc<DocumentStableTailLayout> {
        if self.document_runtime.stable_tail_layout_cache.valid
            && self.document_runtime.stable_tail_layout_cache.key == key
        {
            return Rc::clone(&self.document_runtime.stable_tail_layout_cache.tail);
        }

        let tail = Rc::new(compose_document_stable_tail_layout(
            self.current_document_stable_tail_layout_input(),
        ));
        self.document_runtime.stable_tail_layout_cache = DocumentStableTailLayoutCache {
            key,
            tail: Rc::clone(&tail),
            valid: true,
        };
        tail
    }

    fn current_document_tail_cursor(&self, tail: &DocumentStableTailLayout) -> (u16, usize) {
        if tail.composer_slot.is_empty() {
            return (tail.fallback_cursor_x, tail.fallback_cursor_y);
        }

        let (cursor_x, cursor_y) = self.composer.cursor_visual_position();
        (
            cursor_x,
            tail.composer_slot
                .content_start_line
                .saturating_add(cursor_y),
        )
    }

    fn current_document_stable_tail_layout_input(&mut self) -> DocumentStableTailLayoutInput {
        DocumentStableTailLayoutInput {
            transcript_has_content: self.transcript_render.line_count > 0,
            composer_document: self.composer.document_snapshot(self.palette),
            command_panel: self.current_inline_command_panel_render_result(),
            tool_approval_panel: self.current_inline_tool_approval_panel_render_result(),
            model_panel: self.current_inline_model_panel_render_result(),
            context_budget: self.current_inline_context_budget_render_result(),
            status_line_gap_before: configured_status_line_gap_before(self.style_mode),
            status_line: self.current_status_line_render_result(),
            status_line_2: self.current_status_line_2_render_result(),
        }
    }
}

fn compose_document_stable_tail_layout(
    input: DocumentStableTailLayoutInput,
) -> DocumentStableTailLayout {
    let extra_gap =
        usize::from(input.transcript_has_content) * transcript_composer_gap_line_count();
    let has_composer_padding = input.composer_document.frame_decoration_top_line.is_some()
        && input
            .composer_document
            .frame_decoration_bottom_line
            .is_some();
    let status_line_rows = status_line_pair_height(
        &input.status_line,
        &input.status_line_2,
        input.status_line_gap_before,
    );
    let mut lines = Vec::with_capacity(
        extra_gap
            + usize::from(has_composer_padding) * 2
            + input.command_panel.lines.len()
            + input.tool_approval_panel.lines.len()
            + input.model_panel.lines.len()
            + input.context_budget.lines.len()
            + status_line_rows,
    );
    let mut text_lines = Vec::with_capacity(
        extra_gap
            + usize::from(has_composer_padding) * 2
            + input.command_panel.plain_lines.len()
            + input.tool_approval_panel.plain_lines.len()
            + input.model_panel.plain_lines.len()
            + input.context_budget.plain_lines.len()
            + status_line_rows,
    );
    let mut anchors = Vec::with_capacity(
        extra_gap
            + usize::from(has_composer_padding) * 2
            + input.command_panel.lines.len()
            + input.tool_approval_panel.lines.len()
            + input.model_panel.lines.len()
            + input.context_budget.lines.len()
            + status_line_rows,
    );
    let mut selectable = Vec::with_capacity(
        extra_gap
            + usize::from(has_composer_padding) * 2
            + input.command_panel.lines.len()
            + input.tool_approval_panel.lines.len()
            + input.model_panel.lines.len()
            + input.context_budget.lines.len()
            + status_line_rows,
    );

    if input.transcript_has_content {
        append_transcript_gap(&mut lines, &mut text_lines, &mut anchors, &mut selectable);
    }
    let activity_insert_at = lines.len();

    if input.model_panel.has_content {
        append_model_panel(
            &input.model_panel,
            &mut lines,
            &mut text_lines,
            &mut anchors,
            &mut selectable,
        );

        return compose_document_panel_stable_tail_layout(
            DocumentTailRows {
                lines,
                text_lines,
                anchors,
                selectable,
            },
            activity_insert_at,
        );
    }

    if input.context_budget.has_content {
        append_inline_panel(
            &input.context_budget,
            DocumentAnchorRegion::ContextBudgetPanel,
            &mut lines,
            &mut text_lines,
            &mut anchors,
            &mut selectable,
        );

        return compose_document_panel_stable_tail_layout(
            DocumentTailRows {
                lines,
                text_lines,
                anchors,
                selectable,
            },
            activity_insert_at,
        );
    }

    if input.tool_approval_panel.has_content {
        append_inline_panel(
            &input.tool_approval_panel,
            DocumentAnchorRegion::ToolApprovalPanel,
            &mut lines,
            &mut text_lines,
            &mut anchors,
            &mut selectable,
        );

        return compose_document_panel_stable_tail_layout(
            DocumentTailRows {
                lines,
                text_lines,
                anchors,
                selectable,
            },
            activity_insert_at,
        );
    }

    let composer_line_count = input.composer_document.line_count();
    let composer_slot = SlotFrame::new(lines.len(), has_composer_padding, composer_line_count);
    if let (Some(line), Some(text_line)) = (
        input.composer_document.frame_decoration_top_line.clone(),
        input
            .composer_document
            .frame_decoration_top_plain_line
            .clone(),
    ) {
        lines.push(line);
        text_lines.push(text_line);
        anchors.push(DocumentLineAnchor {
            region: DocumentAnchorRegion::ComposerPadding,
            gap_index: 0,
            ..DocumentLineAnchor::default()
        });
        selectable.push(SelectableLineRange::default());
    }

    let composer_insert_at = lines.len();

    if let (Some(line), Some(text_line)) = (
        input.composer_document.frame_decoration_bottom_line.clone(),
        input
            .composer_document
            .frame_decoration_bottom_plain_line
            .clone(),
    ) {
        lines.push(line);
        text_lines.push(text_line);
        anchors.push(DocumentLineAnchor {
            region: DocumentAnchorRegion::ComposerPadding,
            gap_index: 1,
            ..DocumentLineAnchor::default()
        });
        selectable.push(SelectableLineRange::default());
    }

    if input.command_panel.has_content {
        for index in 0..input.command_panel.lines.len() {
            lines.push(input.command_panel.lines[index].clone());
            text_lines.push(
                input
                    .command_panel
                    .plain_lines
                    .get(index)
                    .cloned()
                    .unwrap_or_default(),
            );
            anchors.push(DocumentLineAnchor {
                region: DocumentAnchorRegion::CommandPanel,
                gap_index: index,
                ..DocumentLineAnchor::default()
            });
            selectable.push(
                input
                    .command_panel
                    .selectable
                    .get(index)
                    .copied()
                    .unwrap_or_default(),
            );
        }
    }

    if input.status_line.has_content || input.status_line_2.has_content {
        for gap_index in 0..input.status_line_gap_before {
            lines.push(Line::raw(""));
            text_lines.push(String::new());
            anchors.push(DocumentLineAnchor {
                region: DocumentAnchorRegion::ComposerStatusGap,
                gap_index,
                ..DocumentLineAnchor::default()
            });
            selectable.push(SelectableLineRange::default());
        }

        append_status_line(
            input.status_line,
            0,
            &mut lines,
            &mut text_lines,
            &mut anchors,
            &mut selectable,
        );
        append_status_line(
            input.status_line_2,
            1,
            &mut lines,
            &mut text_lines,
            &mut anchors,
            &mut selectable,
        );
    }

    DocumentStableTailLayout {
        rows: DocumentTailRows {
            lines,
            text_lines,
            anchors,
            selectable,
        },
        composer_document: Some(input.composer_document),
        composer_insert_at,
        activity_insert_at,
        adds_activity_composer_gap: true,
        composer_slot,
        fallback_cursor_x: 0,
        fallback_cursor_y: composer_slot.content_start_line,
    }
}

fn compose_document_panel_stable_tail_layout(
    mut rows: DocumentTailRows,
    activity_insert_at: usize,
) -> DocumentStableTailLayout {
    if rows.is_empty() {
        rows.push_empty();
    }
    let fallback_cursor_y = rows.len().saturating_add(1);
    let composer_insert_at = rows.len();
    DocumentStableTailLayout {
        rows,
        composer_document: None,
        composer_insert_at,
        activity_insert_at,
        adds_activity_composer_gap: false,
        composer_slot: SlotFrame::empty(),
        fallback_cursor_x: 0,
        fallback_cursor_y,
    }
}

fn compose_document_tail_activity_segment(
    activity: StatusLineRenderResult,
    adds_composer_gap: bool,
) -> DocumentTailActivitySegment {
    let mut rows = DocumentTailRows::default();
    if activity.has_content
        && let Some(line) = activity.line
    {
        rows.lines.push(line);
        rows.text_lines.push(activity.plain_line);
        rows.anchors.push(DocumentLineAnchor {
            region: DocumentAnchorRegion::StreamActivity,
            ..DocumentLineAnchor::default()
        });
        rows.selectable.push(activity.selectable);
    }
    if activity.has_content && adds_composer_gap {
        append_stream_activity_composer_gap(
            &mut rows.lines,
            &mut rows.text_lines,
            &mut rows.anchors,
            &mut rows.selectable,
        );
    }
    DocumentTailActivitySegment { rows }
}

fn compose_document_tail_layout(
    stable: Rc<DocumentStableTailLayout>,
    activity: DocumentTailActivitySegment,
    cursor_x: u16,
    stable_cursor_y: usize,
) -> DocumentTailLayout {
    let activity_line_count = activity.rows.len();
    let composer_slot = if stable.composer_slot.is_empty() {
        stable.composer_slot
    } else {
        offset_slot_frame(stable.composer_slot, activity_line_count)
    };
    let cursor_y = if stable_cursor_y >= stable.activity_insert_at {
        stable_cursor_y.saturating_add(activity_line_count)
    } else {
        stable_cursor_y
    };
    DocumentTailLayout {
        stable,
        activity,
        composer_slot,
        cursor_x,
        cursor_y,
        stable_cursor_y,
    }
}

impl DocumentTailLayout {
    pub(crate) fn line_count(&self) -> usize {
        self.stable_line_count() + self.activity.rows.len()
    }

    pub(crate) fn line_at(&self, index: usize) -> Option<Line<'static>> {
        match self.line_source(index)? {
            DocumentTailLineSource::Rows(rows, index) => rows.lines.get(index).cloned(),
            DocumentTailLineSource::Composer(document, index) => {
                document.range(index, 1).lines.into_iter().next()
            }
        }
    }

    pub(crate) fn text_line_at(&self, index: usize) -> Option<String> {
        match self.line_source(index)? {
            DocumentTailLineSource::Rows(rows, index) => rows.text_lines.get(index).cloned(),
            DocumentTailLineSource::Composer(document, index) => {
                document.range(index, 1).plain_lines.into_iter().next()
            }
        }
    }

    pub(crate) fn anchor_at(&self, index: usize) -> Option<DocumentLineAnchor> {
        match self.line_source(index)? {
            DocumentTailLineSource::Rows(rows, index) => rows.anchors.get(index).copied(),
            DocumentTailLineSource::Composer(document, index) => {
                document
                    .anchor_at(index)
                    .map(|composer| DocumentLineAnchor {
                        region: DocumentAnchorRegion::Composer,
                        composer,
                        ..DocumentLineAnchor::default()
                    })
            }
        }
    }

    pub(crate) fn selectable_at(&self, index: usize) -> Option<SelectableLineRange> {
        match self.line_source(index)? {
            DocumentTailLineSource::Rows(rows, index) => rows.selectable.get(index).copied(),
            DocumentTailLineSource::Composer(document, index) => document
                .range(index, 1)
                .selectable_ranges
                .into_iter()
                .next(),
        }
    }

    pub(crate) fn line_index_for_anchor(&self, target: DocumentLineAnchor) -> Option<usize> {
        if target.region == DocumentAnchorRegion::Composer
            && let Some(document) = self.stable.composer_document.as_ref()
        {
            let composer_index = document.line_index_for_anchor(target.composer)?;
            let stable_index = self.stable.composer_insert_at + composer_index;
            return Some(self.final_index_for_stable_index(stable_index));
        }

        let insert_at = self.stable.activity_insert_at;
        if let Some(index) = self
            .activity
            .rows
            .anchors
            .iter()
            .position(|anchor| *anchor == target)
        {
            return Some(insert_at + index);
        }
        self.stable
            .rows
            .anchors
            .iter()
            .enumerate()
            .find_map(|(row_index, anchor)| {
                (*anchor == target).then(|| {
                    let stable_index = self.stable_index_for_row_index(row_index);
                    self.final_index_for_stable_index(stable_index)
                })
            })
    }

    pub(crate) fn lines_for_range(&self, start: usize, count: usize) -> Vec<Line<'static>> {
        let range = self.range_indices(start, count);
        range.filter_map(|index| self.line_at(index)).collect()
    }

    #[cfg(test)]
    pub(crate) fn text_lines_for_range(&self, start: usize, count: usize) -> Vec<String> {
        let range = self.range_indices(start, count);
        range.filter_map(|index| self.text_line_at(index)).collect()
    }

    pub(crate) fn plain_text_len_for_range(&self, start: usize, count: usize) -> usize {
        let range = self.range_indices(start, count);
        let line_count = range.len();
        if line_count == 0 {
            return 0;
        }
        if range.start == 0 && range.end == self.line_count() {
            let stable_rows = self
                .stable
                .rows
                .text_lines
                .iter()
                .map(String::len)
                .sum::<usize>();
            let activity_rows = self
                .activity
                .rows
                .text_lines
                .iter()
                .map(String::len)
                .sum::<usize>();
            let composer_rows = self
                .stable
                .composer_document
                .as_ref()
                .map_or(0, composer::ComposerDocumentSnapshot::plain_text_len);
            return stable_rows + activity_rows + composer_rows + line_count.saturating_sub(1);
        }
        let plain_text_len = range
            .filter_map(|index| self.text_line_at(index))
            .map(|line| line.len())
            .sum::<usize>();
        plain_text_len + line_count.saturating_sub(1)
    }

    pub(crate) fn composer_visual_position_for_char(
        &self,
        char_index: usize,
    ) -> Option<(u16, usize)> {
        self.stable
            .composer_document
            .as_ref()?
            .visual_position_for_char(char_index)
    }

    fn line_source(&self, index: usize) -> Option<DocumentTailLineSource<'_>> {
        let insert_at = self.stable.activity_insert_at;
        let activity_end = insert_at.saturating_add(self.activity.rows.len());
        if index < insert_at {
            return self.stable_line_source(index);
        }
        if index < activity_end {
            return Some(DocumentTailLineSource::Rows(
                &self.activity.rows,
                index - insert_at,
            ));
        }
        let stable_index = index.saturating_sub(self.activity.rows.len());
        self.stable_line_source(stable_index)
    }

    fn stable_line_source(&self, index: usize) -> Option<DocumentTailLineSource<'_>> {
        let composer_line_count = self
            .stable
            .composer_document
            .as_ref()
            .map_or(0, composer::ComposerDocumentSnapshot::line_count);
        if index < self.stable.composer_insert_at {
            return self
                .stable
                .rows
                .lines
                .get(index)
                .map(|_| DocumentTailLineSource::Rows(&self.stable.rows, index));
        }
        if index < self.stable.composer_insert_at + composer_line_count {
            return self.stable.composer_document.as_ref().map(|document| {
                DocumentTailLineSource::Composer(document, index - self.stable.composer_insert_at)
            });
        }
        let row_index = index.saturating_sub(composer_line_count);
        self.stable
            .rows
            .lines
            .get(row_index)
            .map(|_| DocumentTailLineSource::Rows(&self.stable.rows, row_index))
    }

    fn stable_line_count(&self) -> usize {
        self.stable.rows.len()
            + self
                .stable
                .composer_document
                .as_ref()
                .map_or(0, composer::ComposerDocumentSnapshot::line_count)
    }

    fn stable_index_for_row_index(&self, row_index: usize) -> usize {
        if row_index < self.stable.composer_insert_at {
            row_index
        } else {
            row_index
                + self
                    .stable
                    .composer_document
                    .as_ref()
                    .map_or(0, composer::ComposerDocumentSnapshot::line_count)
        }
    }

    fn final_index_for_stable_index(&self, stable_index: usize) -> usize {
        if stable_index < self.stable.activity_insert_at {
            stable_index
        } else {
            stable_index + self.activity.rows.len()
        }
    }

    fn range_indices(&self, start: usize, count: usize) -> std::ops::Range<usize> {
        let start = start.min(self.line_count());
        let end = start.saturating_add(count).min(self.line_count());
        start..end
    }

    #[cfg(test)]
    pub(crate) fn shares_stable_layout_with(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.stable, &other.stable)
    }

    #[cfg(test)]
    pub(crate) fn all_anchors(&self) -> Vec<DocumentLineAnchor> {
        (0..self.line_count())
            .filter_map(|index| self.anchor_at(index))
            .collect()
    }

    #[cfg(test)]
    pub(crate) fn all_selectable(&self) -> Vec<SelectableLineRange> {
        (0..self.line_count())
            .filter_map(|index| self.selectable_at(index))
            .collect()
    }

    #[cfg(test)]
    pub(crate) fn from_test_parts(
        lines: Vec<Line<'static>>,
        text_lines: Vec<String>,
        anchors: Vec<DocumentLineAnchor>,
        selectable: Vec<SelectableLineRange>,
        composer_slot: SlotFrame,
        cursor_x: u16,
        cursor_y: usize,
    ) -> Self {
        let composer_insert_at = lines.len();
        let stable = Rc::new(DocumentStableTailLayout {
            activity_insert_at: lines.len(),
            rows: DocumentTailRows {
                lines,
                text_lines,
                anchors,
                selectable,
            },
            composer_document: None,
            composer_insert_at,
            composer_slot,
            fallback_cursor_x: cursor_x,
            fallback_cursor_y: cursor_y,
            ..DocumentStableTailLayout::default()
        });
        Self {
            stable,
            activity: DocumentTailActivitySegment::default(),
            composer_slot,
            cursor_x,
            cursor_y,
            stable_cursor_y: cursor_y,
        }
    }
}

pub(crate) fn offset_slot_frame(slot: SlotFrame, offset: usize) -> SlotFrame {
    if slot.is_empty() {
        return slot;
    }

    SlotFrame {
        frame_start_line: slot.frame_start_line + offset,
        content_start_line: slot.content_start_line + offset,
        ..slot
    }
}

pub(crate) fn transcript_composer_gap_line_count() -> usize {
    1
}

fn stream_activity_composer_gap_line_count() -> usize {
    1
}

fn append_transcript_gap(
    lines: &mut Vec<Line<'static>>,
    text_lines: &mut Vec<String>,
    anchors: &mut Vec<DocumentLineAnchor>,
    selectable: &mut Vec<SelectableLineRange>,
) {
    for gap_index in 0..transcript_composer_gap_line_count() {
        lines.push(Line::raw(""));
        text_lines.push(String::new());
        anchors.push(DocumentLineAnchor {
            region: DocumentAnchorRegion::TranscriptComposerGap,
            gap_index,
            ..DocumentLineAnchor::default()
        });
        selectable.push(SelectableLineRange::default());
    }
}

fn append_stream_activity_composer_gap(
    lines: &mut Vec<Line<'static>>,
    text_lines: &mut Vec<String>,
    anchors: &mut Vec<DocumentLineAnchor>,
    selectable: &mut Vec<SelectableLineRange>,
) {
    for gap_index in 0..stream_activity_composer_gap_line_count() {
        lines.push(Line::raw(""));
        text_lines.push(String::new());
        anchors.push(DocumentLineAnchor {
            region: DocumentAnchorRegion::StreamActivityComposerGap,
            gap_index,
            ..DocumentLineAnchor::default()
        });
        selectable.push(SelectableLineRange::default());
    }
}

fn append_model_panel(
    model_panel: &ModelPanelRenderResult,
    lines: &mut Vec<Line<'static>>,
    text_lines: &mut Vec<String>,
    anchors: &mut Vec<DocumentLineAnchor>,
    selectable: &mut Vec<SelectableLineRange>,
) {
    append_inline_panel(
        model_panel,
        DocumentAnchorRegion::ModelPanel,
        lines,
        text_lines,
        anchors,
        selectable,
    );
}

fn append_inline_panel(
    panel: &InlinePanelRenderResult,
    region: DocumentAnchorRegion,
    lines: &mut Vec<Line<'static>>,
    text_lines: &mut Vec<String>,
    anchors: &mut Vec<DocumentLineAnchor>,
    selectable: &mut Vec<SelectableLineRange>,
) {
    for index in 0..panel.lines.len() {
        lines.push(panel.lines[index].clone());
        text_lines.push(panel.plain_lines.get(index).cloned().unwrap_or_default());
        anchors.push(DocumentLineAnchor {
            region,
            gap_index: index,
            ..DocumentLineAnchor::default()
        });
        selectable.push(panel.selectable.get(index).copied().unwrap_or_default());
    }
}

fn append_status_line(
    status_line: StatusLineRenderResult,
    status_line_index: usize,
    lines: &mut Vec<Line<'static>>,
    text_lines: &mut Vec<String>,
    anchors: &mut Vec<DocumentLineAnchor>,
    selectable: &mut Vec<SelectableLineRange>,
) {
    if !status_line.has_content {
        return;
    }

    if let Some(line) = status_line.line {
        lines.push(line);
        text_lines.push(status_line.plain_line);
        anchors.push(DocumentLineAnchor {
            region: DocumentAnchorRegion::StatusLine,
            gap_index: status_line_index,
            ..DocumentLineAnchor::default()
        });
        selectable.push(status_line.selectable);
    }
}
