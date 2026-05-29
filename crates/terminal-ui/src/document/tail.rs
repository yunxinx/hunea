use std::rc::Rc;

use ratatui::text::Line;

use crate::{
    Model,
    command_panel::CommandPanelRenderResult,
    composer,
    inline_panel::InlinePanelRenderResult,
    model_panel::ModelPanelRenderResult,
    selection::{SelectableLineRange, selectable_range_for_plain_line},
    status_line::{
        StatusLineRenderResult, status_line_gap_before as configured_status_line_gap_before,
        status_line_pair_height,
    },
    style_mode::StyleMode,
    tool_approval_panel::ToolApprovalPanelRenderResult,
};

use super::{DocumentAnchorRegion, DocumentLineAnchor, slot_frame::SlotFrame};

/// `DocumentTailLayoutKey` 描述 tail 语义布局的命中条件。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct DocumentTailLayoutKey {
    pub(crate) transcript_has_content: bool,
    pub(crate) palette_version: usize,
    pub(crate) style_mode: StyleMode,
    pub(crate) document_width: u16,
    pub(crate) document_viewport_height: usize,
    pub(crate) composer_viewport_height: usize,
    pub(crate) composer_content_revision: usize,
    pub(crate) composer_cursor_revision: usize,
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
    pub(crate) selected_model: Option<String>,
    pub(crate) status_line_config: u8,
    pub(crate) status_line_2_config: u8,
    pub(crate) status_line_revision: usize,
    pub(crate) stream_activity_frame: usize,
}

/// `DocumentTailLayout` 保存 composer / command panel / status line 的独立 tail 布局。
#[derive(Debug, Clone, Default)]
pub(crate) struct DocumentTailLayout {
    pub(crate) lines: Vec<Line<'static>>,
    pub(crate) text_lines: Vec<String>,
    pub(crate) anchors: Vec<DocumentLineAnchor>,
    pub(crate) selectable: Vec<SelectableLineRange>,
    pub(crate) composer_slot: SlotFrame,
    pub(crate) cursor_x: u16,
    pub(crate) cursor_y: usize,
}

/// `DocumentTailLayoutCache` 缓存最近一次 tail 语义布局。
#[derive(Debug, Clone, Default)]
pub(crate) struct DocumentTailLayoutCache {
    pub(crate) key: DocumentTailLayoutKey,
    pub(crate) tail: Rc<DocumentTailLayout>,
    pub(crate) valid: bool,
}

/// `DocumentTailLayoutInput` 描述 tail 渲染真正需要的输入。
#[derive(Debug, Clone)]
pub(crate) struct DocumentTailLayoutInput {
    pub(crate) transcript_has_content: bool,
    pub(crate) composer_lines: Vec<Line<'static>>,
    pub(crate) composer_text_lines: Vec<String>,
    pub(crate) composer_anchors: Vec<DocumentLineAnchor>,
    pub(crate) composer_selectable: Vec<SelectableLineRange>,
    pub(crate) composer_frame_decoration_top_line: Option<Line<'static>>,
    pub(crate) composer_frame_decoration_top_text_line: Option<String>,
    pub(crate) composer_frame_decoration_bottom_line: Option<Line<'static>>,
    pub(crate) composer_frame_decoration_bottom_text_line: Option<String>,
    pub(crate) composer_cursor_x: u16,
    pub(crate) composer_cursor_y: usize,
    pub(crate) stream_activity: StatusLineRenderResult,
    pub(crate) command_panel: CommandPanelRenderResult,
    pub(crate) tool_approval_panel: ToolApprovalPanelRenderResult,
    pub(crate) model_panel: ModelPanelRenderResult,
    pub(crate) status_line_gap_before: usize,
    pub(crate) status_line: StatusLineRenderResult,
    pub(crate) status_line_2: StatusLineRenderResult,
}

impl Model {
    pub(crate) fn build_document_tail_layout(&mut self) -> Rc<DocumentTailLayout> {
        let key = self.current_document_tail_layout_key();
        if self.document_runtime.tail_layout_cache.valid
            && self.document_runtime.tail_layout_cache.key == key
        {
            return self.current_document_tail_layout_with_refreshed_cursor();
        }

        let tail = Rc::new(compose_document_tail_layout(
            self.current_document_tail_layout_input(),
        ));
        self.document_runtime.tail_layout_cache = DocumentTailLayoutCache {
            key,
            tail: Rc::clone(&tail),
            valid: true,
        };
        tail
    }

    pub(crate) fn current_document_tail_layout_key(&self) -> DocumentTailLayoutKey {
        DocumentTailLayoutKey {
            transcript_has_content: self.transcript_render.line_count > 0,
            palette_version: self.palette_version,
            style_mode: self.style_mode,
            document_width: self.width,
            document_viewport_height: self.document_viewport_height(),
            composer_viewport_height: self.composer.viewport_height(),
            composer_content_revision: self.composer.content_revision(),
            composer_cursor_revision: 0,
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
            selected_model: self
                .selected_model
                .as_ref()
                .map(|model| model.display_name()),
            status_line_config: self.status_line_config_bits(),
            status_line_2_config: self.status_line_2_config_bits(),
            status_line_revision: self.status_line_revision(),
            stream_activity_frame: self.stream_activity_frame_key(std::time::Instant::now()),
        }
    }

    fn current_document_tail_layout_with_refreshed_cursor(&mut self) -> Rc<DocumentTailLayout> {
        let cached = Rc::clone(&self.document_runtime.tail_layout_cache.tail);
        let Some((cursor_x, composer_cursor_y)) = self.composer_cursor_from_tail(&cached) else {
            return cached;
        };
        let cursor_y = cached
            .composer_slot
            .content_start_line
            .saturating_add(composer_cursor_y);
        if cached.cursor_x == cursor_x && cached.cursor_y == cursor_y {
            return cached;
        }

        let tail = Rc::new(DocumentTailLayout {
            cursor_x,
            cursor_y,
            ..(*cached).clone()
        });
        self.document_runtime.tail_layout_cache.tail = Rc::clone(&tail);
        tail
    }

    fn composer_cursor_from_tail(&self, tail: &DocumentTailLayout) -> Option<(u16, usize)> {
        let start = tail.composer_slot.content_start_line;
        let end = start.saturating_add(tail.composer_slot.content_line_count);
        let anchors = tail.anchors.get(start..end)?;
        let composer_anchors = anchors
            .iter()
            .filter_map(|anchor| {
                (anchor.region == DocumentAnchorRegion::Composer).then_some(anchor.composer)
            })
            .collect::<Vec<_>>();
        self.composer
            .cursor_visual_position_for_anchors(&composer_anchors)
    }

    pub(crate) fn current_document_tail_layout_input(&mut self) -> DocumentTailLayoutInput {
        let composer_document = self.composer.render_document(self.palette);

        DocumentTailLayoutInput {
            transcript_has_content: self.transcript_render.line_count > 0,
            composer_lines: composer_document.lines,
            composer_text_lines: composer_document.plain_lines,
            composer_anchors: document_anchors_for_composer(&composer_document.anchors),
            composer_selectable: composer_document.selectable_ranges,
            composer_frame_decoration_top_line: composer_document.frame_decoration_top_line,
            composer_frame_decoration_top_text_line: composer_document
                .frame_decoration_top_plain_line,
            composer_frame_decoration_bottom_line: composer_document.frame_decoration_bottom_line,
            composer_frame_decoration_bottom_text_line: composer_document
                .frame_decoration_bottom_plain_line,
            composer_cursor_x: composer_document.cursor_x,
            composer_cursor_y: composer_document.cursor_y,
            stream_activity: self.current_stream_activity_render_result(),
            command_panel: self.current_inline_command_panel_render_result(),
            tool_approval_panel: self.current_inline_tool_approval_panel_render_result(),
            model_panel: self.current_inline_model_panel_render_result(),
            status_line_gap_before: configured_status_line_gap_before(self.style_mode),
            status_line: self.current_status_line_render_result(),
            status_line_2: self.current_status_line_2_render_result(),
        }
    }
}

pub(crate) fn compose_document_tail_layout(input: DocumentTailLayoutInput) -> DocumentTailLayout {
    let extra_gap =
        usize::from(input.transcript_has_content) * transcript_composer_gap_line_count();
    let stream_activity_gap =
        usize::from(input.stream_activity.has_content) * stream_activity_composer_gap_line_count();
    let has_composer_padding = input.composer_frame_decoration_top_line.is_some()
        && input.composer_frame_decoration_bottom_line.is_some();
    let status_line_rows = status_line_pair_height(
        &input.status_line,
        &input.status_line_2,
        input.status_line_gap_before,
    );
    let mut lines = Vec::with_capacity(
        extra_gap
            + usize::from(input.stream_activity.has_content)
            + stream_activity_gap
            + input.composer_lines.len()
            + usize::from(has_composer_padding) * 2
            + input.command_panel.lines.len()
            + input.tool_approval_panel.lines.len()
            + input.model_panel.lines.len()
            + status_line_rows,
    );
    let mut text_lines = Vec::with_capacity(
        extra_gap
            + usize::from(input.stream_activity.has_content)
            + stream_activity_gap
            + input.composer_text_lines.len()
            + usize::from(has_composer_padding) * 2
            + input.command_panel.plain_lines.len()
            + input.tool_approval_panel.plain_lines.len()
            + input.model_panel.plain_lines.len()
            + status_line_rows,
    );
    let mut anchors = Vec::with_capacity(
        extra_gap
            + usize::from(input.stream_activity.has_content)
            + stream_activity_gap
            + input.composer_anchors.len()
            + usize::from(has_composer_padding) * 2
            + input.command_panel.lines.len()
            + input.tool_approval_panel.lines.len()
            + input.model_panel.lines.len()
            + status_line_rows,
    );
    let mut selectable = Vec::with_capacity(
        extra_gap
            + usize::from(input.stream_activity.has_content)
            + stream_activity_gap
            + input.composer_selectable.len()
            + usize::from(has_composer_padding) * 2
            + input.command_panel.lines.len()
            + input.tool_approval_panel.lines.len()
            + input.model_panel.lines.len()
            + status_line_rows,
    );

    if input.transcript_has_content {
        append_transcript_gap(&mut lines, &mut text_lines, &mut anchors, &mut selectable);
    }

    if input.stream_activity.has_content
        && let Some(line) = input.stream_activity.line
    {
        lines.push(line);
        text_lines.push(input.stream_activity.plain_line);
        anchors.push(DocumentLineAnchor {
            region: DocumentAnchorRegion::StreamActivity,
            ..DocumentLineAnchor::default()
        });
        selectable.push(input.stream_activity.selectable);
    }

    if input.model_panel.has_content {
        append_model_panel(
            &input.model_panel,
            &mut lines,
            &mut text_lines,
            &mut anchors,
            &mut selectable,
        );

        if lines.is_empty() {
            lines.push(Line::raw(""));
            text_lines.push(String::new());
            anchors.push(DocumentLineAnchor::default());
            selectable.push(SelectableLineRange::default());
        }

        let cursor_y = lines.len().saturating_add(1);
        return DocumentTailLayout {
            lines,
            text_lines,
            anchors,
            selectable,
            composer_slot: SlotFrame::new(0, false, 0),
            cursor_x: 0,
            cursor_y,
        };
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

        if lines.is_empty() {
            lines.push(Line::raw(""));
            text_lines.push(String::new());
            anchors.push(DocumentLineAnchor::default());
            selectable.push(SelectableLineRange::default());
        }

        let cursor_y = lines.len().saturating_add(1);
        return DocumentTailLayout {
            lines,
            text_lines,
            anchors,
            selectable,
            composer_slot: SlotFrame::new(0, false, 0),
            cursor_x: 0,
            cursor_y,
        };
    }

    if input.stream_activity.has_content {
        append_stream_activity_composer_gap(
            &mut lines,
            &mut text_lines,
            &mut anchors,
            &mut selectable,
        );
    }

    let composer_slot = SlotFrame::new(
        lines.len(),
        has_composer_padding,
        input.composer_text_lines.len(),
    );
    if let (Some(line), Some(text_line)) = (
        input.composer_frame_decoration_top_line,
        input.composer_frame_decoration_top_text_line,
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

    lines.extend(input.composer_lines);
    text_lines.extend(input.composer_text_lines);
    anchors.extend(input.composer_anchors);
    selectable.extend(ensure_selectable_ranges(
        &text_lines[text_lines.len() - input.composer_selectable.len()..],
        &input.composer_selectable,
    ));

    if let (Some(line), Some(text_line)) = (
        input.composer_frame_decoration_bottom_line,
        input.composer_frame_decoration_bottom_text_line,
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

    let composer_slot = if lines.is_empty() {
        SlotFrame::new(0, false, 1)
    } else {
        composer_slot
    };
    if lines.is_empty() {
        lines.push(Line::raw(""));
        text_lines.push(String::new());
        anchors.push(DocumentLineAnchor::default());
        selectable.push(SelectableLineRange::default());
    }

    DocumentTailLayout {
        lines,
        text_lines,
        anchors,
        selectable,
        composer_slot,
        cursor_x: input.composer_cursor_x,
        cursor_y: composer_slot.content_start_line + input.composer_cursor_y,
    }
}

pub(crate) fn ensure_selectable_ranges(
    text_lines: &[String],
    ranges: &[SelectableLineRange],
) -> Vec<SelectableLineRange> {
    text_lines
        .iter()
        .enumerate()
        .map(|(index, line)| {
            ranges
                .get(index)
                .copied()
                .unwrap_or_else(|| selectable_range_for_plain_line(line))
        })
        .collect()
}

pub(crate) fn offset_slot_frame(slot: SlotFrame, offset: usize) -> SlotFrame {
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

fn document_anchors_for_composer(line_anchors: &[composer::LineAnchor]) -> Vec<DocumentLineAnchor> {
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
