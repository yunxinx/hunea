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
    pub(crate) transcript_render_version: usize,
    pub(crate) document_width: u16,
}

/// `DocumentTranscriptSnapshot` 缓存 unified document 真正需要的 transcript 行级数据。
#[derive(Debug, Clone)]
pub(crate) struct DocumentTranscriptSnapshot {
    pub(crate) lines: Vec<Line<'static>>,
    pub(crate) plain_lines: Vec<String>,
    pub(crate) anchors: Vec<DocumentLineAnchor>,
    pub(crate) width: u16,
    pub(crate) palette: TerminalPalette,
    pub(crate) items: HashMap<usize, TranscriptItem>,
    pub(crate) selectable_cache: Rc<RefCell<HashMap<usize, Vec<SelectableLineRange>>>>,
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
    pub(crate) key: DocumentTranscriptKey,
    pub(crate) snapshot: DocumentTranscriptSnapshot,
    pub(crate) valid: bool,
}

/// `DocumentLayoutKey` 描述影响统一文档布局的最小状态集合。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct DocumentLayoutKey {
    pub(crate) transcript_render_version: usize,
    pub(crate) palette_version: usize,
    pub(crate) style_mode: StyleMode,
    pub(crate) document_width: u16,
    pub(crate) composer_value: String,
    pub(crate) composer_width: usize,
    pub(crate) composer_prompt: String,
    pub(crate) composer_placeholder: String,
    pub(crate) composer_line: usize,
    pub(crate) composer_column: usize,
    pub(crate) status_line_text: String,
}

/// `DocumentLayout` 表示整份统一文档流的稳定布局。
#[derive(Debug, Clone, Default)]
pub(crate) struct DocumentLayout {
    pub(crate) transcript: DocumentTranscriptSnapshot,
    pub(crate) transcript_line_count: usize,
    pub(crate) transcript_segments: Vec<DocumentTranscriptSegment>,
    pub(crate) transcript_items: HashMap<usize, DocumentTranscriptItemLines>,
    pub(crate) lines: Vec<Line<'static>>,
    pub(crate) plain_lines: Vec<String>,
    pub(crate) anchors: Vec<DocumentLineAnchor>,
    pub(crate) selectable: Vec<SelectableLineRange>,
    pub(crate) composer_slot: SlotFrame,
    pub(crate) composer_start_line: usize,
    pub(crate) composer_line_count: usize,
    pub(crate) cursor_x: u16,
    pub(crate) cursor_y: usize,
}

/// `DocumentLayoutLine` 收敛 unified document 某一视觉行的稳定只读视图。
#[derive(Debug, Clone, Default)]
pub(crate) struct DocumentLayoutLine {
    pub(crate) line: Line<'static>,
    pub(crate) plain_line: String,
    pub(crate) anchor: DocumentLineAnchor,
    pub(crate) selectable: SelectableLineRange,
}

/// `DocumentTranscriptSegment` 描述 transcript 在 unified document 里的一个可索引连续片段。
#[derive(Debug, Clone, Default)]
pub(crate) struct DocumentTranscriptSegment {
    pub(crate) start_line: usize,
    pub(crate) line_count: usize,
    pub(crate) lines: Vec<Line<'static>>,
    pub(crate) plain_lines: Vec<String>,
}

/// `DocumentTranscriptItemLines` 描述单个 transcript item 在 unified document 顶部前缀里的内容行范围。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct DocumentTranscriptItemLines {
    pub(crate) content_start_line: usize,
    pub(crate) content_line_count: usize,
    pub(crate) total_line_count: usize,
}

/// `DocumentLayoutCache` 缓存最近一次合成出的统一文档布局。
#[derive(Debug, Clone, Default)]
pub(crate) struct DocumentLayoutCache {
    pub(crate) key: DocumentLayoutKey,
    pub(crate) layout: DocumentLayout,
    pub(crate) transcript_line_count: usize,
    pub(crate) valid: bool,
}

/// `DocumentViewportKey` 描述可视窗口缓存的命中条件。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct DocumentViewportKey {
    pub(crate) layout_key: DocumentLayoutKey,
    pub(crate) offset: usize,
    pub(crate) height: usize,
    pub(crate) bottom_follow: bool,
    pub(crate) selection_version: usize,
}

/// `DocumentViewport` 表示统一文档在当前 offset 下的可视切片。
#[derive(Debug, Clone, Default)]
pub(crate) struct DocumentViewport {
    pub(crate) lines: Vec<Line<'static>>,
    #[allow(dead_code)]
    pub(crate) plain_lines: Vec<String>,
    pub(crate) resolved_offset: usize,
}

/// `DocumentViewportCache` 缓存当前 viewport 的可视结果。
#[derive(Debug, Clone, Default)]
pub(crate) struct DocumentViewportCache {
    pub(crate) key: DocumentViewportKey,
    pub(crate) viewport: DocumentViewport,
    pub(crate) valid: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum DocumentAnchorRegion {
    #[default]
    None,
    Transcript,
    Composer,
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
    pub(crate) line_anchor: DocumentLineAnchor,
    pub(crate) line_text: String,
    pub(crate) transcript_item_line_count: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum ManualDocumentScrollRestoreTarget {
    #[default]
    None,
    BottomFollow,
    ComposerCursor,
}
