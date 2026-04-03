use ratatui::text::Line;

use crate::frontend::tui::{
    composer,
    document::slot_frame::SlotFrame,
    style_mode::StyleMode,
    transcript::{self},
};

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
    pub(crate) lines: Vec<Line<'static>>,
    pub(crate) plain_lines: Vec<String>,
    pub(crate) anchors: Vec<DocumentLineAnchor>,
    pub(crate) composer_slot: SlotFrame,
    pub(crate) composer_start_line: usize,
    pub(crate) composer_line_count: usize,
    pub(crate) cursor_x: u16,
    pub(crate) cursor_y: usize,
}

/// `DocumentLayoutCache` 缓存最近一次合成出的统一文档布局。
#[derive(Debug, Clone, Default)]
pub(crate) struct DocumentLayoutCache {
    pub(crate) key: DocumentLayoutKey,
    pub(crate) layout: DocumentLayout,
    pub(crate) valid: bool,
}

/// `DocumentViewportKey` 描述可视窗口缓存的命中条件。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct DocumentViewportKey {
    pub(crate) layout_key: DocumentLayoutKey,
    pub(crate) offset: usize,
    pub(crate) height: usize,
    pub(crate) bottom_follow: bool,
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
