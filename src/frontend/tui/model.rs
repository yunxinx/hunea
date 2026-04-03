use ratatui::Frame;

use super::{
    HeroOptions,
    composer::Composer,
    theme::{TerminalPalette, default_palette},
    transcript::Transcript,
    view,
};

const TRANSCRIPT_COMPOSER_GAP: u16 = 1;
const COMPOSER_MIN_HEIGHT: u16 = 1;

/// `Model` 表示交互式 TUI 应用的状态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Model {
    palette: TerminalPalette,
    transcript: Transcript,
    composer: Composer,
    width: u16,
    height: u16,
    transcript_line_count: u16,
    has_palette: bool,
    has_window: bool,
    has_dark_background: bool,
    quitting: bool,
}

impl Model {
    /// `new` 创建并初始化 TUI 模型。
    pub fn new(hero_options: HeroOptions) -> Self {
        let palette = default_palette();
        let mut transcript = Transcript::new(palette);
        transcript.set_gap(1);
        transcript.append_hero(hero_options);

        Self {
            palette,
            transcript,
            composer: Composer::default(),
            width: 0,
            height: 0,
            transcript_line_count: 0,
            has_palette: false,
            has_window: false,
            has_dark_background: true,
            quitting: false,
        }
    }

    /// `render` 将当前模型渲染到一帧。
    pub fn render(&self, frame: &mut Frame<'_>) {
        view::render(self, frame);
    }

    /// `palette` 返回当前使用的配色。
    pub fn palette(&self) -> &TerminalPalette {
        &self.palette
    }

    /// `has_palette` 返回是否已经拿到可用配色。
    pub fn has_palette(&self) -> bool {
        self.has_palette
    }

    /// `is_ready` 判断首帧是否具备稳定布局和主题信息。
    pub fn is_ready(&self) -> bool {
        self.has_palette && self.has_window
    }

    /// `is_quitting` 返回是否正在退出。
    pub fn is_quitting(&self) -> bool {
        self.quitting
    }

    /// `composer_text` 返回输入框当前的内容。
    pub fn composer_text(&self) -> &str {
        self.composer.value()
    }

    /// `transcript_plain_items` 返回适用于退出后打印的 transcript 项列表。
    pub fn transcript_plain_items(&self) -> Vec<String> {
        self.transcript.plain_items()
    }

    pub(crate) fn transcript(&self) -> &Transcript {
        &self.transcript
    }

    pub(crate) fn composer(&self) -> &Composer {
        &self.composer
    }

    pub(crate) fn set_window(&mut self, width: u16, height: u16) {
        self.width = width;
        self.height = height;
        self.has_window = true;
        self.transcript.set_width(width.max(1));
        self.composer.set_width(width.max(1));
        self.sync_transcript_line_count();
        self.sync_composer_layout();
    }

    pub(crate) fn composer_viewport_height(&self) -> u16 {
        let content_height = self.composer.full_height().max(COMPOSER_MIN_HEIGHT);
        if !self.has_window || self.height == 0 {
            return content_height;
        }

        let available_rows = self.height.saturating_sub(self.composer_top_offset());
        if available_rows == 0 {
            return COMPOSER_MIN_HEIGHT;
        }

        content_height.min(available_rows)
    }

    pub(crate) fn composer_gap_height(&self) -> u16 {
        if self.transcript_line_count > 0 {
            TRANSCRIPT_COMPOSER_GAP
        } else {
            0
        }
    }

    pub(crate) fn set_palette(&mut self, palette: TerminalPalette, has_dark_background: bool) {
        self.palette = palette;
        self.has_dark_background = has_dark_background;
        self.has_palette = true;
        self.transcript.set_palette(palette);
        self.sync_transcript_line_count();
        self.sync_composer_layout();
    }

    pub(crate) fn composer_mut(&mut self) -> &mut Composer {
        &mut self.composer
    }

    pub(crate) fn transcript_mut(&mut self) -> &mut Transcript {
        &mut self.transcript
    }

    pub(crate) fn mark_quitting(&mut self) {
        self.quitting = true;
    }

    pub(crate) fn sync_composer_layout(&mut self) {
        let viewport_height = self.composer_viewport_height().max(COMPOSER_MIN_HEIGHT);
        self.composer.set_height(viewport_height);
    }

    pub(crate) fn sync_transcript_line_count(&mut self) {
        let rendered_lines = self.transcript.render_lines().len();
        self.transcript_line_count = u16::try_from(rendered_lines).unwrap_or(u16::MAX);
    }

    fn composer_top_offset(&self) -> u16 {
        if self.transcript_line_count == 0 {
            0
        } else {
            self.transcript_line_count
                .saturating_add(TRANSCRIPT_COMPOSER_GAP)
        }
    }
}
