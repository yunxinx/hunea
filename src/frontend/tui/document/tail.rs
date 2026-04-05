use std::rc::Rc;

use ratatui::text::Line;

use crate::frontend::tui::{
    Model,
    command_panel::CommandPanelRenderResult,
    composer,
    selection::{SelectableLineRange, selectable_range_for_plain_line},
    status_line::StatusLineRenderResult,
    style_mode::StyleMode,
};

use super::{DocumentAnchorRegion, DocumentLineAnchor, slot_frame::SlotFrame};

/// `DocumentTailLayoutKey` 描述 tail 语义布局的命中条件。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct DocumentTailLayoutKey {
    pub(crate) transcript_has_content: bool,
    pub(crate) palette_version: usize,
    pub(crate) style_mode: StyleMode,
    pub(crate) document_width: u16,
    pub(crate) composer_content_revision: usize,
    pub(crate) composer_cursor_revision: usize,
    pub(crate) composer_width: usize,
    pub(crate) command_panel_selected: usize,
    pub(crate) command_panel_scroll: usize,
    pub(crate) status_line_config: u8,
    pub(crate) status_line_revision: usize,
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
    pub(crate) composer_frame_decoration_line: Option<Line<'static>>,
    pub(crate) composer_frame_decoration_text_line: Option<String>,
    pub(crate) composer_cursor_x: u16,
    pub(crate) composer_cursor_y: usize,
    pub(crate) command_panel: CommandPanelRenderResult,
    pub(crate) status_line: StatusLineRenderResult,
}

impl Model {
    pub(crate) fn build_document_tail_layout(&mut self) -> Rc<DocumentTailLayout> {
        let key = self.current_document_tail_layout_key();
        if self.document_tail_layout_cache.valid && self.document_tail_layout_cache.key == key {
            return Rc::clone(&self.document_tail_layout_cache.tail);
        }

        let tail = Rc::new(compose_document_tail_layout(
            self.current_document_tail_layout_input(),
        ));
        self.document_tail_layout_cache = DocumentTailLayoutCache {
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
            composer_content_revision: self.composer.content_revision(),
            composer_cursor_revision: self.composer.cursor_revision(),
            composer_width: self.composer.content_width(),
            command_panel_selected: self.command_panel_selected,
            command_panel_scroll: self.command_panel_scroll,
            status_line_config: self.status_line_config_bits(),
            status_line_revision: self.status_line_revision(),
        }
    }

    pub(crate) fn current_document_tail_layout_input(&mut self) -> DocumentTailLayoutInput {
        let composer_document = self.composer.render_document(self.palette);

        DocumentTailLayoutInput {
            transcript_has_content: self.transcript_render.line_count > 0,
            composer_lines: composer_document.lines,
            composer_text_lines: composer_document.plain_lines,
            composer_anchors: document_anchors_for_composer(&composer_document.anchors),
            composer_selectable: composer_document.selectable_ranges,
            composer_frame_decoration_line: composer_document.frame_decoration_line,
            composer_frame_decoration_text_line: composer_document.frame_decoration_plain_line,
            composer_cursor_x: composer_document.cursor_x,
            composer_cursor_y: composer_document.cursor_y,
            command_panel: self.current_inline_command_panel_render_result(),
            status_line: self.current_status_line_render_result(),
        }
    }
}

pub(crate) fn compose_document_tail_layout(input: DocumentTailLayoutInput) -> DocumentTailLayout {
    let extra_gap =
        usize::from(input.transcript_has_content) * transcript_composer_gap_line_count();
    let has_composer_padding = input.composer_frame_decoration_line.is_some();
    let mut lines = Vec::with_capacity(
        extra_gap
            + input.composer_lines.len()
            + usize::from(has_composer_padding) * 2
            + input.command_panel.lines.len()
            + input.status_line.gap_before
            + usize::from(input.status_line.has_content),
    );
    let mut text_lines = Vec::with_capacity(
        extra_gap
            + input.composer_text_lines.len()
            + usize::from(has_composer_padding) * 2
            + input.command_panel.plain_lines.len()
            + input.status_line.gap_before
            + usize::from(input.status_line.has_content),
    );
    let mut anchors = Vec::with_capacity(
        extra_gap
            + input.composer_anchors.len()
            + usize::from(has_composer_padding) * 2
            + input.command_panel.lines.len()
            + input.status_line.gap_before
            + usize::from(input.status_line.has_content),
    );
    let mut selectable = Vec::with_capacity(
        extra_gap
            + input.composer_selectable.len()
            + usize::from(has_composer_padding) * 2
            + input.command_panel.lines.len()
            + input.status_line.gap_before
            + usize::from(input.status_line.has_content),
    );

    if input.transcript_has_content {
        append_transcript_gap(&mut lines, &mut text_lines, &mut anchors, &mut selectable);
    }

    let composer_slot = SlotFrame::new(
        lines.len(),
        has_composer_padding,
        input.composer_text_lines.len(),
    );
    if let (Some(line), Some(text_line)) = (
        input.composer_frame_decoration_line.clone(),
        input.composer_frame_decoration_text_line.clone(),
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
        input.composer_frame_decoration_line,
        input.composer_frame_decoration_text_line,
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

    if input.status_line.has_content {
        for gap_index in 0..input.status_line.gap_before {
            lines.push(Line::raw(""));
            text_lines.push(String::new());
            anchors.push(DocumentLineAnchor {
                region: DocumentAnchorRegion::ComposerStatusGap,
                gap_index,
                ..DocumentLineAnchor::default()
            });
            selectable.push(SelectableLineRange::default());
        }

        if let Some(line) = input.status_line.line {
            lines.push(line);
            text_lines.push(input.status_line.plain_line);
            anchors.push(DocumentLineAnchor {
                region: DocumentAnchorRegion::StatusLine,
                ..DocumentLineAnchor::default()
            });
            selectable.push(input.status_line.selectable);
        }
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
