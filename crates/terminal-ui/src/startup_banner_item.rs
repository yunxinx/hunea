use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
};

use ratatui::text::Line;

use super::{
    StartupBannerOptions,
    startup_banner::{
        render_startup_banner_lines_with_palette, render_startup_banner_plain_lines_with_palette,
        resolved_content_width, startup_banner_title_plain_text, startup_banner_total_width,
    },
    theme::TerminalPalette,
    transcript::{
        DEFAULT_RENDER_WIDTH, ItemLineAnchor, LineAnchorKind, TranscriptEstimateKind,
        TranscriptFastEstimate, TranscriptItemMetrics, wrap_prompt_visual_lines,
    },
};
use runtime_domain::envinfo::short_work_dir;

use crate::styled_text::{lines_to_ansi_text, lines_to_plain_text};

const BANNER_LOGICAL_LINE_TOP_BORDER: usize = 0;
const BANNER_LOGICAL_LINE_TITLE: usize = 1;
const BANNER_LOGICAL_LINE_SEPARATOR: usize = 2;
const BANNER_LOGICAL_LINE_WORK_DIR: usize = 3;
const BANNER_LOGICAL_LINE_BOTTOM_BORDER: usize = 4;

/// `StartupBannerItem` 表示 transcript 的开场项。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartupBannerItem {
    options: StartupBannerOptions,
    render_cache_key: u64,
}

impl StartupBannerItem {
    /// `new` 创建一条启动欢迎块项，并捕获启动时的 workdir 快照。
    pub fn new(mut options: StartupBannerOptions) -> Self {
        if options.work_dir.as_deref().unwrap_or("").is_empty() {
            options.work_dir = Some(short_work_dir());
        }

        let render_cache_key = startup_banner_item_render_cache_key(&options);

        Self {
            options,
            render_cache_key,
        }
    }

    /// `render_lines` 将启动欢迎块渲染为带样式的文本行。
    pub fn render_lines(&self, width: u16, palette: TerminalPalette) -> Vec<Line<'static>> {
        self.adjusted_options(width, palette)
            .map(|options| render_startup_banner_lines_with_palette(&options, palette))
            .unwrap_or_else(|| render_startup_banner_lines_with_palette(&self.options, palette))
    }

    /// `render_for_terminal_replay` 返回适合退出 AltScreen 后回放到终端的启动欢迎块文本。
    pub fn render_for_terminal_replay(
        &self,
        width: u16,
        palette: TerminalPalette,
        preserve_ansi: bool,
    ) -> String {
        let width = if width == 0 {
            u16::try_from(DEFAULT_RENDER_WIDTH).unwrap_or(u16::MAX)
        } else {
            width
        };

        let lines = self
            .adjusted_options(width, palette)
            .map(|options| render_startup_banner_lines_with_palette(&options, palette))
            .unwrap_or_else(|| render_startup_banner_lines_with_palette(&self.options, palette));

        if preserve_ansi {
            lines_to_ansi_text(&lines)
        } else {
            lines_to_plain_text(&lines)
        }
    }

    /// `render_plain_text` 返回不带 ANSI 的纯文本启动欢迎块内容。
    pub fn render_plain_text(&self, width: u16, palette: TerminalPalette) -> String {
        let lines = self.render_lines(width, palette);
        lines_to_plain_text(&lines)
    }

    pub(crate) fn render_cache_key(&self) -> u64 {
        self.render_cache_key
    }

    pub(crate) fn source_text_byte_len(&self) -> usize {
        self.options.app_name.as_deref().unwrap_or("").len()
            + self.options.version.as_deref().unwrap_or("").len()
            + self.options.work_dir.as_deref().unwrap_or("").len()
    }

    pub(crate) fn measure_render_metrics(
        &self,
        width: u16,
        palette: TerminalPalette,
    ) -> (usize, usize) {
        let options = self
            .adjusted_options(width, palette)
            .unwrap_or_else(|| self.options.clone());
        let plain_lines = render_startup_banner_plain_lines_with_palette(&options, palette);

        (
            plain_lines.len(),
            plain_lines.iter().map(String::len).sum::<usize>(),
        )
    }

    pub(crate) fn estimate_render_metrics_fast(
        &self,
        width: u16,
        palette: TerminalPalette,
        _previous_metrics: Option<TranscriptItemMetrics>,
    ) -> TranscriptFastEstimate {
        let (content_line_count, content_char_len) = self.measure_render_metrics(width, palette);
        TranscriptFastEstimate {
            content_line_count,
            content_char_len,
            kind: TranscriptEstimateKind::NonAssistant,
            ..TranscriptFastEstimate::default()
        }
    }

    pub(crate) fn render_line_anchors(
        &self,
        width: u16,
        palette: TerminalPalette,
    ) -> Vec<ItemLineAnchor> {
        let options = self
            .adjusted_options(width, palette)
            .unwrap_or_else(|| self.options.clone());
        let rendered_lines = render_startup_banner_lines_with_palette(&options, palette);
        if rendered_lines.is_empty() {
            return Vec::new();
        }

        let content_anchors = self.content_line_anchors(&options);
        if content_anchors.len() + 2 != rendered_lines.len() {
            return Vec::new();
        }

        let mut anchors = Vec::with_capacity(rendered_lines.len());
        anchors.push(startup_banner_zero_width_anchor(
            BANNER_LOGICAL_LINE_TOP_BORDER,
            0,
        ));
        for (rendered_line, anchor) in content_anchors.into_iter().enumerate() {
            anchors.push(ItemLineAnchor {
                rendered_line: rendered_line + 1,
                ..anchor
            });
        }
        anchors.push(startup_banner_zero_width_anchor(
            BANNER_LOGICAL_LINE_BOTTOM_BORDER,
            rendered_lines.len() - 1,
        ));
        anchors
    }

    fn adjusted_options(
        &self,
        width: u16,
        _palette: TerminalPalette,
    ) -> Option<StartupBannerOptions> {
        if width == 0 {
            return None;
        }

        let natural_width = startup_banner_total_width(&self.options);
        if natural_width <= width {
            return None;
        }

        Some(StartupBannerOptions {
            width: width.saturating_sub(6).max(1),
            ..self.options.clone()
        })
    }

    fn content_line_anchors(&self, options: &StartupBannerOptions) -> Vec<ItemLineAnchor> {
        let app_name = options.app_name.as_deref().unwrap_or("Lumos");
        let version = options.version.as_deref().unwrap_or("v0.1.0");
        let work_dir = options.work_dir.as_deref().unwrap_or("");
        let content_width = resolved_content_width(
            options.width,
            &startup_banner_title_plain_text(app_name, version),
            work_dir,
        );
        let mut anchors = startup_banner_wrapped_text_anchors(
            &startup_banner_title_plain_text(app_name, version),
            usize::from(content_width),
            BANNER_LOGICAL_LINE_TITLE,
        );
        if work_dir.is_empty() {
            return anchors;
        }

        anchors.push(startup_banner_zero_width_anchor(
            BANNER_LOGICAL_LINE_SEPARATOR,
            0,
        ));
        anchors.extend(startup_banner_wrapped_text_anchors(
            work_dir,
            usize::from(content_width),
            BANNER_LOGICAL_LINE_WORK_DIR,
        ));
        anchors
    }
}

fn startup_banner_item_render_cache_key(options: &StartupBannerOptions) -> u64 {
    let mut hasher = DefaultHasher::new();
    options.app_name.as_deref().unwrap_or("").hash(&mut hasher);
    options.version.as_deref().unwrap_or("").hash(&mut hasher);
    options.work_dir.as_deref().unwrap_or("").hash(&mut hasher);
    options.width.hash(&mut hasher);
    hasher.finish()
}

fn startup_banner_wrapped_text_anchors(
    text: &str,
    width: usize,
    logical_line: usize,
) -> Vec<ItemLineAnchor> {
    wrap_prompt_visual_lines(text, width.max(1), 0)
        .into_iter()
        .map(|line| ItemLineAnchor {
            kind: LineAnchorKind::LogicalPosition,
            logical_line,
            range_start: line.visible_start_char,
            range_end: line.end_char,
            rendered_line: 0,
            gap_offset: 0,
        })
        .collect()
}

fn startup_banner_zero_width_anchor(logical_line: usize, rendered_line: usize) -> ItemLineAnchor {
    ItemLineAnchor {
        kind: LineAnchorKind::LogicalPosition,
        logical_line,
        range_start: 0,
        range_end: 0,
        rendered_line,
        gap_offset: 0,
    }
}
