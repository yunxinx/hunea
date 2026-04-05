use std::{cell::RefCell, collections::HashMap, rc::Rc};

use ratatui::text::Line;

use crate::frontend::tui::{
    composer,
    document::slot_frame::SlotFrame,
    selection::SelectableLineRange,
    style_mode::StyleMode,
    theme::TerminalPalette,
    transcript::{self, TranscriptItem},
};

/// `DocumentTranscriptKey` 描述 transcript->document 中间快照的命中条件。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct DocumentTranscriptKey {
    pub(super) transcript_render_version: usize,
    pub(super) document_width: u16,
}

/// `DocumentTranscriptSnapshot` 缓存 unified document 真正需要的 transcript 行级数据。
#[derive(Debug, Clone)]
pub(crate) struct DocumentTranscriptSnapshot {
    pub(super) lines: Vec<Line<'static>>,
    pub(super) plain_lines: Vec<String>,
    pub(super) anchors: Vec<DocumentLineAnchor>,
    pub(super) width: u16,
    pub(super) palette: TerminalPalette,
    pub(super) items: HashMap<usize, TranscriptItem>,
    pub(super) selectable_cache: Rc<RefCell<HashMap<usize, Vec<SelectableLineRange>>>>,
}

impl Default for DocumentTranscriptSnapshot {
    fn default() -> Self {
        Self {
            lines: Vec::new(),
            plain_lines: Vec::new(),
            anchors: Vec::new(),
            width: 0,
            palette: crate::frontend::tui::theme::default_palette(),
            items: HashMap::new(),
            selectable_cache: Rc::new(RefCell::new(HashMap::new())),
        }
    }
}

/// `DocumentTranscriptCache` 避免 composer 编辑时反复重建 transcript 快照。
#[derive(Debug, Clone, Default)]
pub(crate) struct DocumentTranscriptCache {
    pub(super) key: DocumentTranscriptKey,
    pub(super) snapshot: Rc<DocumentTranscriptSnapshot>,
    pub(super) valid: bool,
}

/// `DocumentLayoutKey` 描述影响统一文档布局的最小状态集合。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct DocumentLayoutKey {
    pub(super) transcript_render_version: usize,
    pub(super) palette_version: usize,
    pub(super) style_mode: StyleMode,
    pub(super) document_width: u16,
    pub(super) viewport_height: usize,
    pub(super) composer_content_revision: usize,
    pub(super) composer_cursor_revision: usize,
    pub(super) composer_width: usize,
    pub(super) command_panel_selected: usize,
    pub(super) command_panel_scroll: usize,
    pub(super) status_line_config: u8,
    pub(super) status_line_revision: usize,
}

/// `DocumentLayout` 表示整份统一文档流的稳定布局。
#[derive(Debug, Clone, Default)]
pub(crate) struct DocumentLayout {
    pub(super) transcript: Rc<DocumentTranscriptSnapshot>,
    pub(crate) transcript_line_count: usize,
    pub(super) transcript_items: HashMap<usize, DocumentTranscriptItemLines>,
    pub(crate) tail_lines: Vec<Line<'static>>,
    pub(crate) tail_plain_lines: Vec<String>,
    pub(super) tail_anchors: Vec<DocumentLineAnchor>,
    pub(super) tail_selectable: Vec<SelectableLineRange>,
    pub(crate) composer_slot: SlotFrame,
    pub(super) composer_start_line: usize,
    pub(crate) composer_line_count: usize,
    pub(crate) cursor_x: u16,
    pub(crate) cursor_y: usize,
}

#[cfg(test)]
impl DocumentLayout {
    pub(crate) fn with_test_plain_lines(
        transcript_line_count: usize,
        plain_lines: &[&str],
    ) -> Self {
        let transcript_line_count = transcript_line_count.min(plain_lines.len());
        let transcript_plain_lines = &plain_lines[..transcript_line_count];
        let tail_plain_lines = &plain_lines[transcript_line_count..];
        Self {
            transcript_line_count,
            transcript: Rc::new(DocumentTranscriptSnapshot {
                lines: transcript_plain_lines
                    .iter()
                    .map(|line| Line::raw((*line).to_string()))
                    .collect(),
                plain_lines: transcript_plain_lines
                    .iter()
                    .map(|line| (*line).to_string())
                    .collect(),
                anchors: vec![DocumentLineAnchor::default(); transcript_plain_lines.len()],
                ..DocumentTranscriptSnapshot::default()
            }),
            tail_lines: tail_plain_lines
                .iter()
                .map(|line| Line::raw((*line).to_string()))
                .collect(),
            tail_plain_lines: tail_plain_lines
                .iter()
                .map(|line| (*line).to_string())
                .collect(),
            tail_anchors: vec![DocumentLineAnchor::default(); tail_plain_lines.len()],
            tail_selectable: vec![SelectableLineRange::default(); tail_plain_lines.len()],
            ..Self::default()
        }
    }
}

/// `DocumentLayoutLine` 收敛 unified document 某一视觉行的稳定只读视图。
#[derive(Debug, Clone, Default)]
pub(crate) struct DocumentLayoutLine {
    pub(crate) line: Line<'static>,
    pub(crate) plain_line: String,
    pub(crate) anchor: DocumentLineAnchor,
    pub(crate) selectable: SelectableLineRange,
}

/// `DocumentTranscriptItemLines` 描述单个 transcript item 在 unified document 顶部前缀里的内容行范围。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct DocumentTranscriptItemLines {
    pub(super) content_start_line: usize,
    pub(super) content_line_count: usize,
    pub(super) total_line_count: usize,
}

/// `DocumentLayoutCache` 缓存最近一次合成出的统一文档布局。
#[derive(Debug, Clone, Default)]
pub(crate) struct DocumentLayoutCache {
    pub(super) key: DocumentLayoutKey,
    pub(super) layout: Rc<DocumentLayout>,
    pub(super) transcript_line_count: usize,
    pub(super) valid: bool,
}

/// `DocumentViewportKey` 描述可视窗口缓存的命中条件。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct DocumentViewportKey {
    pub(super) layout_key: DocumentLayoutKey,
    pub(super) offset: usize,
    pub(super) height: usize,
    pub(super) bottom_follow: bool,
    pub(super) selection_version: usize,
}

/// `DocumentViewport` 表示统一文档在当前 offset 下的可视切片。
#[derive(Debug, Clone, Default)]
pub(crate) struct DocumentViewport {
    pub(crate) lines: Vec<Line<'static>>,
    pub(crate) plain_text_len: usize,
    #[cfg(test)]
    pub(crate) plain_lines: Vec<String>,
    pub(crate) resolved_offset: usize,
}

#[cfg(test)]
impl DocumentViewport {
    pub(crate) fn with_test_plain_lines(plain_lines: &[&str], resolved_offset: usize) -> Self {
        Self {
            lines: plain_lines
                .iter()
                .map(|line| Line::raw((*line).to_string()))
                .collect(),
            plain_text_len: plain_lines.iter().map(|line| line.len()).sum::<usize>()
                + plain_lines.len().saturating_sub(1),
            plain_lines: plain_lines.iter().map(|line| (*line).to_string()).collect(),
            resolved_offset,
        }
    }
}

/// `DocumentViewportCache` 缓存当前 viewport 的可视结果。
#[derive(Debug, Clone, Default)]
pub(crate) struct DocumentViewportCache {
    pub(super) key: DocumentViewportKey,
    pub(super) viewport: Rc<DocumentViewport>,
    pub(super) valid: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum DocumentAnchorRegion {
    #[default]
    None,
    Transcript,
    Composer,
    CommandPanel,
    ComposerPadding,
    TranscriptComposerGap,
    ComposerStatusGap,
    StatusLine,
}

/// `DocumentLineAnchor` 把 transcript 与 composer 的行级锚点统一到同一坐标系。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct DocumentLineAnchor {
    pub(crate) region: DocumentAnchorRegion,
    pub(crate) transcript: transcript::LineAnchor,
    pub(crate) composer: composer::LineAnchor,
    pub(crate) gap_index: usize,
}

/// `DocumentViewportAnchor` 保存 viewport 顶部的语义位置。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct DocumentViewportAnchor {
    pub(super) line_anchor: DocumentLineAnchor,
    pub(super) line_text: String,
    pub(super) transcript_item_line_count: usize,
    pub(super) transcript_semantic_position: super::viewport_state::TranscriptSemanticPosition,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum ManualDocumentScrollRestoreTarget {
    #[default]
    None,
    BottomFollow,
    ComposerCursor,
}
