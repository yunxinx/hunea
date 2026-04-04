use std::time::Instant;

use ratatui::Frame;

use crate::envinfo;

use super::{
    HeroOptions,
    composer::Composer,
    document::{
        LayoutCache, RestoreTarget, ViewportAnchor, ViewportCache, offset_viewport_line_indices,
    },
    external_editor::ExternalEditorLaunch,
    selection::{AutoScrollDirection, MousePosition, SelectionClickState, SelectionState},
    status_line::{StatusLineItem, StatusLineRenderResult},
    style_mode::StyleMode,
    theme::{TerminalPalette, default_palette},
    transcript::{RenderResult, Transcript},
    view,
};

/// `Model` 表示交互式 TUI 应用的状态。
#[derive(Debug, Clone)]
pub struct Model {
    pub(crate) style_mode: StyleMode,
    pub(crate) status_line_items: Vec<StatusLineItem>,
    pub(crate) external_editor: Vec<String>,
    pub(crate) external_editor_hint: String,
    pub(crate) external_editor_helper_enabled: bool,
    pub(crate) copy_on_mouse_selection_release: bool,
    pub(crate) swap_enter_and_send: bool,
    pub(crate) ctrl_c_clears_input: bool,
    pub(crate) selection: SelectionState,
    pub(crate) selection_click: SelectionClickState,
    pub(crate) selection_version: usize,
    pub(crate) selection_auto_scroll_direction: AutoScrollDirection,
    pub(crate) selection_auto_scroll_token: usize,
    pub(crate) selection_auto_scroll_mouse: MousePosition,
    pub(crate) selection_auto_scroll_deadline: Option<Instant>,
    pub(crate) git_branch: String,
    pub(crate) current_dir: String,
    pub(crate) palette: TerminalPalette,
    pub(crate) palette_version: usize,
    pub(crate) transcript: Transcript,
    pub(crate) transcript_render: RenderResult,
    pub(crate) transcript_render_version: usize,
    pub(crate) composer: Composer,
    pub(crate) width: u16,
    pub(crate) height: u16,
    pub(crate) document_viewport_y: usize,
    pub(crate) document_layout_cache: LayoutCache,
    pub(crate) document_viewport_cache: ViewportCache,
    pub(crate) has_palette: bool,
    pub(crate) has_window: bool,
    pub(crate) has_dark_background: bool,
    pub(crate) follow_bottom: bool,
    pub(crate) manual_document_scroll: bool,
    pub(crate) scroll_restore_target: RestoreTarget,
    pub(crate) scroll_restore_anchor: ViewportAnchor,
    pub(crate) status_notice_text: String,
    pub(crate) status_notice_token: usize,
    pub(crate) status_notice_deadline: Option<Instant>,
    pub(crate) external_editor_helper_visible: bool,
    pub(crate) external_editor_helper_token: usize,
    pub(crate) external_editor_helper_deadline: Option<Instant>,
    pub(crate) exit_confirmation_deadline: Option<Instant>,
    quitting: bool,
}

/// `ModelOptions` 表示创建 TUI 模型时可配置的样式与状态行选项。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelOptions {
    pub style_mode: StyleMode,
    pub status_line_items: Vec<StatusLineItem>,
    pub external_editor: Vec<String>,
    pub external_editor_hint: String,
    pub show_external_editor_helper: bool,
    pub copy_on_mouse_selection_release: bool,
    pub swap_enter_and_send: bool,
    pub ctrl_c_clears_input: bool,
}

impl Default for ModelOptions {
    fn default() -> Self {
        Self {
            style_mode: StyleMode::default(),
            status_line_items: Vec::new(),
            external_editor: Vec::new(),
            external_editor_hint: String::new(),
            show_external_editor_helper: true,
            copy_on_mouse_selection_release: false,
            swap_enter_and_send: false,
            ctrl_c_clears_input: true,
        }
    }
}

impl Model {
    /// `new` 创建并初始化 TUI 模型。
    pub fn new(hero_options: HeroOptions) -> Self {
        Self::new_with_options(hero_options, ModelOptions::default())
    }

    /// `new_with_style_mode` 创建并初始化带指定样式模式的 TUI 模型。
    pub fn new_with_style_mode(hero_options: HeroOptions, style_mode: StyleMode) -> Self {
        Self::new_with_options(
            hero_options,
            ModelOptions {
                style_mode,
                ..ModelOptions::default()
            },
        )
    }

    /// `new_with_options` 创建并初始化带显式选项的 TUI 模型。
    pub fn new_with_options(hero_options: HeroOptions, options: ModelOptions) -> Self {
        let palette = default_palette();
        let mut transcript = Transcript::new(palette);
        transcript.set_gap(1);
        transcript.append_hero(hero_options);
        let transcript_render = transcript.render();
        let style_mode = options.style_mode.normalized();
        let status_line_items = options.status_line_items;

        Self {
            style_mode,
            status_line_items: status_line_items.clone(),
            external_editor: options.external_editor,
            external_editor_hint: options.external_editor_hint,
            external_editor_helper_enabled: options.show_external_editor_helper,
            copy_on_mouse_selection_release: options.copy_on_mouse_selection_release,
            swap_enter_and_send: options.swap_enter_and_send,
            ctrl_c_clears_input: options.ctrl_c_clears_input,
            selection: SelectionState::default(),
            selection_click: SelectionClickState::default(),
            selection_version: 0,
            selection_auto_scroll_direction: AutoScrollDirection::None,
            selection_auto_scroll_token: 0,
            selection_auto_scroll_mouse: MousePosition::default(),
            selection_auto_scroll_deadline: None,
            git_branch: resolve_initial_git_branch(&status_line_items),
            current_dir: resolve_initial_current_dir(&status_line_items),
            palette,
            palette_version: 1,
            transcript_render_version: 1,
            transcript,
            transcript_render,
            composer: Composer::new(style_mode),
            width: 0,
            height: 0,
            document_viewport_y: 0,
            document_layout_cache: LayoutCache::default(),
            document_viewport_cache: ViewportCache::default(),
            has_palette: false,
            has_window: false,
            has_dark_background: true,
            follow_bottom: true,
            manual_document_scroll: false,
            scroll_restore_target: RestoreTarget::None,
            scroll_restore_anchor: ViewportAnchor::default(),
            status_notice_text: String::new(),
            status_notice_token: 0,
            status_notice_deadline: None,
            external_editor_helper_visible: false,
            external_editor_helper_token: 0,
            external_editor_helper_deadline: None,
            exit_confirmation_deadline: None,
            quitting: false,
        }
    }

    /// `render` 将当前模型渲染到一帧。
    pub fn render(&mut self, frame: &mut Frame<'_>) {
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

    /// `next_timeout_deadline` 返回当前最早需要处理的内部超时。
    pub fn next_timeout_deadline(&self) -> Option<Instant> {
        match (
            self.status_notice_deadline,
            self.external_editor_helper_deadline,
            self.selection_auto_scroll_deadline,
        ) {
            (Some(left), Some(right), Some(third)) => Some(left.min(right).min(third)),
            (Some(left), Some(right), None) => Some(left.min(right)),
            (Some(left), None, Some(right)) | (None, Some(left), Some(right)) => {
                Some(left.min(right))
            }
            (Some(deadline), None, None)
            | (None, Some(deadline), None)
            | (None, None, Some(deadline)) => Some(deadline),
            (None, None, None) => None,
        }
    }

    /// `composer_text` 返回输入框当前的内容。
    pub fn composer_text(&self) -> &str {
        self.composer.value()
    }

    /// `transcript_plain_items` 返回适用于纯文本消费的 transcript 项列表。
    pub fn transcript_plain_items(&self) -> Vec<String> {
        self.transcript.plain_items()
    }

    /// `terminal_replay_items` 返回退出 AltScreen 后回放到终端的 transcript 项。
    pub fn terminal_replay_items(&self, preserve_ansi: bool) -> Vec<String> {
        self.transcript.terminal_replay_items(preserve_ansi)
    }

    pub(crate) fn set_window(&mut self, width: u16, height: u16) {
        self.width = width;
        self.height = height;
        self.has_window = true;
        self.transcript.set_width(width.max(1));
        self.composer.set_width(width.max(1));
        self.sync_transcript_render();
        self.sync_composer_height();
    }

    pub(crate) fn set_palette(&mut self, palette: TerminalPalette, has_dark_background: bool) {
        let preserved_anchor = if self.manual_document_scroll {
            self.current_document_viewport_anchor()
        } else {
            None
        };
        if self.selection.active {
            self.invalidate_selection_for_reflow();
        }
        if self.palette != palette {
            self.palette_version += 1;
        }
        self.palette = palette;
        self.has_dark_background = has_dark_background;
        self.has_palette = true;
        self.transcript.set_palette(palette);
        self.sync_transcript_render();
        self.sync_composer_height();
        self.sync_document_viewport_after_transcript_refresh(preserved_anchor);
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

    pub(crate) fn timeout_event(&self, now: Instant) -> Option<super::AppEvent> {
        if let Some(deadline) = self.status_notice_deadline
            && now >= deadline
        {
            return Some(super::AppEvent::StatusNoticeTimeout {
                token: self.status_notice_token,
            });
        }

        if let Some(deadline) = self.external_editor_helper_deadline
            && now >= deadline
        {
            return Some(super::AppEvent::ExternalEditorHelperTimeout {
                token: self.external_editor_helper_token,
            });
        }

        if let Some(deadline) = self.selection_auto_scroll_deadline
            && now >= deadline
        {
            return Some(super::AppEvent::SelectionAutoScrollTick {
                token: self.selection_auto_scroll_token,
            });
        }

        None
    }

    pub(crate) fn maybe_prepare_external_editor_launch(&mut self) -> Option<ExternalEditorLaunch> {
        self.prepare_external_editor_launch()
    }

    pub(crate) fn sync_composer_height(&mut self) {
        let full_height = self.composer.full_height().max(1);
        let mut viewport_height = if !self.has_window || self.height == 0 {
            full_height
        } else {
            full_height.min(self.height.max(1))
        };

        let status_line = self.current_status_line_render_result();
        if status_line.has_content {
            if self.follow_bottom && !self.manual_document_scroll {
                let visible_height = self.bottom_follow_composer_content_line_count(&status_line);
                viewport_height =
                    viewport_height.min(u16::try_from(visible_height).unwrap_or(u16::MAX));
            } else {
                let visible_height = self.visible_composer_content_line_count_in_viewport();
                if visible_height > 0 {
                    viewport_height =
                        viewport_height.min(u16::try_from(visible_height).unwrap_or(u16::MAX));
                }
            }
        }

        self.composer.set_height(viewport_height);
    }

    pub(crate) fn sync_transcript_render(&mut self) {
        if self.selection.active {
            self.invalidate_selection_for_reflow();
        }
        self.transcript_render = self.transcript.render();
        self.transcript_render_version += 1;
        self.document_layout_cache.valid = false;
        self.document_viewport_cache.valid = false;
    }

    fn bottom_follow_composer_content_line_count(
        &self,
        status_line: &StatusLineRenderResult,
    ) -> usize {
        let viewport_height = usize::from(self.height.max(1));
        let mut tail_rows = status_line.gap_before + 1;
        if self.composer_uses_rendered_frame_padding() {
            tail_rows += 1;
        }

        if tail_rows < viewport_height {
            viewport_height - tail_rows
        } else {
            viewport_height
        }
    }

    fn composer_uses_rendered_frame_padding(&self) -> bool {
        match self.style_mode {
            StyleMode::Cx => self.palette.surface.is_some(),
            StyleMode::Cc => true,
            StyleMode::Ms => false,
        }
    }

    fn visible_composer_content_line_count_in_viewport(&mut self) -> usize {
        let layout = self.build_document_layout();
        let line_indices = offset_viewport_line_indices(
            &layout,
            self.document_viewport_y,
            self.document_viewport_height(),
        );

        line_indices
            .into_iter()
            .filter(|line_index| {
                *line_index >= layout.composer_slot.content_start_line
                    && *line_index <= layout.composer_slot.content_bottom_line()
            })
            .count()
    }
}

fn resolve_initial_git_branch(items: &[StatusLineItem]) -> String {
    if items
        .iter()
        .any(|item| matches!(item, StatusLineItem::GitBranch))
    {
        envinfo::git_branch()
    } else {
        String::new()
    }
}

fn resolve_initial_current_dir(items: &[StatusLineItem]) -> String {
    if items
        .iter()
        .any(|item| matches!(item, StatusLineItem::CurrentDir))
    {
        envinfo::short_work_dir()
    } else {
        String::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frontend::tui::{Sender, StyleMode};

    #[test]
    fn overflowing_document_bottom_slice_keeps_full_draft_height() {
        let mut model = Model::new_with_style_mode(HeroOptions::default(), StyleMode::Ms);
        model.set_window(20, 4);
        model.set_palette(default_palette(), true);
        model.composer_mut().set_text_for_test("1\n2\n3");
        model.sync_composer_height();
        model.sync_document_viewport_to_bottom();

        let layout = model.build_document_layout();
        assert_eq!(layout.composer_line_count, 3);

        let viewport = model.build_document_viewport(&layout);
        let rendered = viewport.plain_lines;
        assert_eq!(
            rendered,
            vec![
                String::new(),
                "┃ 1".to_string(),
                "┃ 2".to_string(),
                "┃ 3".to_string(),
            ]
        );
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
