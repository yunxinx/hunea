use ratatui::Frame;

use super::{
    HeroOptions,
    composer::Composer,
    theme::{TerminalPalette, default_palette},
    transcript::{RenderResult, Transcript},
    view,
};

const TRANSCRIPT_COMPOSER_GAP: u16 = 1;
const COMPOSER_MIN_HEIGHT: u16 = 1;

/// `Model` 表示交互式 TUI 应用的状态。
#[derive(Debug, Clone)]
pub struct Model {
    palette: TerminalPalette,
    transcript: Transcript,
    transcript_render: RenderResult,
    composer: Composer,
    width: u16,
    height: u16,
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
        let transcript_render = transcript.render();

        Self {
            palette,
            transcript,
            transcript_render,
            composer: Composer::default(),
            width: 0,
            height: 0,
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

    /// `transcript_exit_items` 返回退出后打印所需的 transcript 项。
    pub fn transcript_exit_items(&self, preserve_ansi: bool) -> Vec<String> {
        self.transcript.exit_items(preserve_ansi)
    }

    pub(crate) fn transcript_render(&self) -> &RenderResult {
        &self.transcript_render
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
        self.sync_transcript_render();
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
        if self.transcript_render.line_count > 0 {
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
        self.sync_transcript_render();
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

    pub(crate) fn sync_transcript_render(&mut self) {
        self.transcript_render = self.transcript.render();
    }

    fn composer_top_offset(&self) -> u16 {
        if self.transcript_render.line_count == 0 {
            0
        } else {
            u16::try_from(self.transcript_render.line_count)
                .unwrap_or(u16::MAX)
                .saturating_add(TRANSCRIPT_COMPOSER_GAP)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frontend::tui::Sender;

    #[test]
    fn wrapped_transcript_rows_reduce_composer_viewport_height() {
        let mut model = Model::new(HeroOptions::default());
        model.transcript_mut().clear();
        model
            .transcript_mut()
            .append_message(Sender::Assistant, "1234567890");

        model.set_window(10, 4);
        model.composer_mut().set_text_for_test("1\n2\n3");
        model.sync_composer_layout();

        assert_eq!(model.transcript_render.line_count, 1);
        assert_eq!(model.composer().visible_height(), 2);
    }

    #[test]
    fn transcript_plain_items_use_assistant_markdown_render_path() {
        let mut model = Model::new(HeroOptions::default());
        model.transcript_mut().clear();
        model
            .transcript_mut()
            .append_message(Sender::Assistant, "# Overview of the API");

        assert_eq!(model.transcript_plain_items(), vec!["Overview of the API"]);
    }
}
