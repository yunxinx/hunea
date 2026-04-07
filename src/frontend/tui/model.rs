use std::rc::Rc;
use std::time::Instant;

use ratatui::Frame;

use crate::envinfo;

use super::{
    HeroOptions,
    composer::Composer,
    composer_mouse::PendingComposerCursorClick,
    document::{
        LayoutCache, RestoreState, TailLayoutCache, TranscriptCache, ViewportCache, ViewportState,
        offset_viewport_line_indices,
    },
    external_editor::ExternalEditorLaunch,
    selection::{AutoScrollDirection, MousePosition, SelectionClickState, SelectionState},
    status_line::{StatusLineItem, StatusLineRenderResult},
    style_mode::StyleMode,
    theme::{TerminalPalette, default_palette},
    transcript::{RenderResult, Transcript, index_only_render_result},
    view,
};

/// `Model` 表示交互式 TUI 应用的状态。
#[derive(Debug, Clone)]
pub struct Model {
    pub(super) style_mode: StyleMode,
    pub(super) status_line_items: Vec<StatusLineItem>,
    pub(super) external_editor: Vec<String>,
    pub(super) external_editor_hint: String,
    pub(super) external_editor_helper_enabled: bool,
    pub(super) command_panel_selected: usize,
    pub(super) command_panel_scroll: usize,
    pub(super) copy_on_mouse_selection_release: bool,
    pub(super) swap_enter_and_send: bool,
    pub(super) ctrl_c_clears_input: bool,
    pub(super) selection: SelectionState,
    pub(super) selection_click: SelectionClickState,
    pub(super) pending_composer_cursor_click: PendingComposerCursorClick,
    pub(super) selection_version: usize,
    pub(super) selection_auto_scroll_direction: AutoScrollDirection,
    pub(super) selection_auto_scroll_token: usize,
    pub(super) selection_auto_scroll_mouse: MousePosition,
    pub(super) selection_auto_scroll_deadline: Option<Instant>,
    pub(super) git_branch: String,
    pub(super) current_dir: String,
    pub(super) palette: TerminalPalette,
    pub(super) palette_version: usize,
    pub(super) transcript: Transcript,
    pub(super) transcript_render: Rc<RenderResult>,
    pub(super) transcript_render_version: usize,
    pub(super) composer: Composer,
    pub(super) width: u16,
    pub(super) height: u16,
    pub(super) document_viewport_y: usize,
    pub(super) document_viewport_state: ViewportState,
    pub(super) document_transcript_cache: TranscriptCache,
    pub(super) document_tail_layout_cache: TailLayoutCache,
    pub(super) document_layout_cache: LayoutCache,
    pub(super) document_viewport_cache: ViewportCache,
    pub(super) has_palette: bool,
    pub(super) has_window: bool,
    pub(super) has_dark_background: bool,
    pub(super) follow_bottom: bool,
    pub(super) manual_document_scroll: bool,
    pub(super) manual_scroll_restore: RestoreState,
    pub(super) status_notice_text: String,
    pub(super) status_notice_token: usize,
    pub(super) status_notice_deadline: Option<Instant>,
    pub(super) history_scroll_indicator_token: usize,
    pub(super) history_scroll_indicator_deadline: Option<Instant>,
    pub(super) external_editor_helper_visible: bool,
    pub(super) external_editor_helper_token: usize,
    pub(super) external_editor_helper_deadline: Option<Instant>,
    pub(super) exit_confirmation_deadline: Option<Instant>,
    pub(super) status_line_revision: usize,
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
        let transcript_render = Rc::new(index_only_render_result(transcript.item_metrics_index()));
        let style_mode = options.style_mode.normalized();
        let status_line_items = options.status_line_items;

        Self {
            style_mode,
            status_line_items: status_line_items.clone(),
            external_editor: options.external_editor,
            external_editor_hint: options.external_editor_hint,
            external_editor_helper_enabled: options.show_external_editor_helper,
            command_panel_selected: 0,
            command_panel_scroll: 0,
            copy_on_mouse_selection_release: options.copy_on_mouse_selection_release,
            swap_enter_and_send: options.swap_enter_and_send,
            ctrl_c_clears_input: options.ctrl_c_clears_input,
            selection: SelectionState::default(),
            selection_click: SelectionClickState::default(),
            pending_composer_cursor_click: PendingComposerCursorClick::default(),
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
            document_viewport_state: ViewportState::default(),
            document_transcript_cache: TranscriptCache::default(),
            document_tail_layout_cache: TailLayoutCache::default(),
            document_layout_cache: LayoutCache::default(),
            document_viewport_cache: ViewportCache::default(),
            has_palette: false,
            has_window: false,
            has_dark_background: true,
            follow_bottom: true,
            manual_document_scroll: false,
            manual_scroll_restore: RestoreState::default(),
            status_notice_text: String::new(),
            status_notice_token: 0,
            status_notice_deadline: None,
            history_scroll_indicator_token: 0,
            history_scroll_indicator_deadline: None,
            external_editor_helper_visible: false,
            external_editor_helper_token: 0,
            external_editor_helper_deadline: None,
            exit_confirmation_deadline: None,
            status_line_revision: 1,
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
        [
            self.status_notice_deadline,
            self.external_editor_helper_deadline,
            self.history_scroll_indicator_deadline,
            self.selection_auto_scroll_deadline,
        ]
        .into_iter()
        .flatten()
        .min()
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
        let width = width.max(1);
        let width_changed = !self.has_window || self.width != width;

        self.width = width;
        self.height = height;
        self.has_window = true;
        self.transcript.set_width(width);
        self.composer.set_width(width);
        if width_changed {
            self.sync_transcript_render();
        }
        self.sync_command_panel_navigation();
        self.sync_composer_height();
    }

    pub(crate) fn set_palette(&mut self, palette: TerminalPalette, has_dark_background: bool) {
        let preserved_viewport_state = if self.manual_document_scroll {
            Some(self.current_document_viewport_state())
        } else {
            None
        };
        let palette_changed = self.palette != palette;
        if palette_changed && self.selection.is_active() {
            self.invalidate_selection_for_reflow();
        }
        if palette_changed {
            self.palette_version += 1;
        }
        self.palette = palette;
        self.has_dark_background = has_dark_background;
        self.has_palette = true;
        self.transcript.set_palette(palette);
        if palette_changed {
            self.sync_transcript_render();
        }
        self.sync_composer_height();
        if palette_changed {
            self.sync_document_viewport_after_transcript_refresh(preserved_viewport_state);
        }
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

        if let Some(deadline) = self.history_scroll_indicator_deadline
            && now >= deadline
        {
            return Some(super::AppEvent::HistoryScrollIndicatorTimeout {
                token: self.history_scroll_indicator_token,
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
        let command_panel = self.current_inline_command_panel_render_result();
        if status_line.has_content || command_panel.has_content {
            if self.follow_bottom && !self.manual_document_scroll {
                let visible_height =
                    self.bottom_follow_composer_content_line_count(&status_line, &command_panel);
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
        self.transcript.begin_recent_render_block_batch();
        let index = self.transcript.item_metrics_index();
        let warmed_item_count = if let Some((start, count)) =
            self.current_visible_transcript_window(index.line_count)
        {
            self.transcript
                .prewarm_viewport_window(&index, start, count)
        } else {
            0
        };
        self.transcript
            .finish_recent_render_block_batch(warmed_item_count);
        self.transcript_render = Rc::new(index_only_render_result(index));
        self.transcript_render_version += 1;
        self.invalidate_document_viewport_cache();
    }

    pub(crate) fn status_line_revision(&self) -> usize {
        self.status_line_revision
    }

    pub(crate) fn bump_status_line_revision(&mut self) {
        self.status_line_revision = self.status_line_revision.saturating_add(1);
    }

    fn bottom_follow_composer_content_line_count(
        &self,
        status_line: &StatusLineRenderResult,
        command_panel: &super::command_panel::CommandPanelRenderResult,
    ) -> usize {
        let viewport_height = usize::from(self.height.max(1));
        let mut tail_rows = command_panel.lines.len();
        if status_line.has_content {
            tail_rows += status_line.gap_before + 1;
        }
        if self.composer_uses_rendered_frame_padding() {
            tail_rows += 1;
        }

        if tail_rows < viewport_height {
            viewport_height - tail_rows
        } else {
            viewport_height
        }
    }

    pub(crate) fn composer_uses_rendered_frame_padding(&self) -> bool {
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

    /// `current_visible_transcript_window` 返回当前 document viewport 与 transcript 的交集窗口。
    pub(crate) fn current_visible_transcript_window(
        &mut self,
        transcript_line_count: usize,
    ) -> Option<(usize, usize)> {
        if transcript_line_count == 0 || self.document_viewport_height() == 0 {
            return None;
        }

        let manual_scroll = self.document_viewport_state.manual_scroll();
        let layout = if manual_scroll {
            let index = self.transcript.item_metrics_index();
            self.document_layout_for_transcript_index(index)
        } else {
            self.transcript_window_layout(transcript_line_count)
        };
        let document_offset = if manual_scroll {
            self.document_viewport_state
                .resolve_offset(&layout, self.document_viewport_height())
        } else {
            self.document_viewport_state.resolved_offset()
        };
        let line_indices = self.document_viewport_line_indices_for_mode(
            &layout,
            document_offset,
            self.document_viewport_state.follow_bottom(),
            manual_scroll,
        );

        let mut start = None;
        let mut count = 0usize;
        for line_index in line_indices {
            if line_index >= transcript_line_count {
                if start.is_some() {
                    break;
                }
                continue;
            }

            start.get_or_insert(line_index);
            count += 1;
        }

        start.map(|start| (start, count))
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
    use std::rc::Rc;

    use super::*;
    use crate::frontend::tui::{Sender, StyleMode, document::DocumentAnchorRegion};

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
        let rendered = viewport.plain_lines.clone();
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

    #[test]
    fn height_only_resize_keeps_transcript_render_stable() {
        let mut model = Model::new_with_style_mode(HeroOptions::default(), StyleMode::Ms);
        model.transcript_mut().clear();
        model
            .transcript_mut()
            .append_message(Sender::Assistant, "alpha\nbeta\ngamma\ndelta");
        model.set_window(20, 4);
        model.set_palette(default_palette(), true);
        model.composer_mut().set_text_for_test("1\n2\n3\n4\n5\n6");
        model.sync_composer_height();

        let before_render_version = model.transcript_render_version;
        let before_render = Rc::clone(&model.transcript_render);
        let before_composer_height = model.composer.visible_height();

        model.set_window(20, 8);

        assert_eq!(
            model.transcript_render_version, before_render_version,
            "height-only resize should not trigger a transcript rerender"
        );
        assert!(
            Rc::ptr_eq(&before_render, &model.transcript_render),
            "height-only resize should keep reusing the current transcript render result"
        );
        assert!(
            model.composer.visible_height() > before_composer_height,
            "height-only resize should still update the tail/composer layout"
        );
    }

    #[test]
    fn setting_the_same_palette_keeps_transcript_render_stable() {
        let mut model = Model::new_with_style_mode(HeroOptions::default(), StyleMode::Ms);
        model.transcript_mut().clear();
        model
            .transcript_mut()
            .append_message(Sender::Assistant, "alpha\nbeta");
        model.set_window(20, 4);
        model.set_palette(default_palette(), true);

        let before_render_version = model.transcript_render_version;
        let before_render = Rc::clone(&model.transcript_render);

        model.set_palette(default_palette(), true);

        assert_eq!(
            model.transcript_render_version, before_render_version,
            "setting the same palette should not trigger a transcript rerender"
        );
        assert!(
            Rc::ptr_eq(&before_render, &model.transcript_render),
            "setting the same palette should keep the existing transcript render result"
        );
    }

    #[test]
    fn current_visible_transcript_window_matches_actual_viewport_line_indices() {
        #[derive(Clone, Copy)]
        enum TailState {
            Plain,
            StatusLine,
            CommandPanel,
        }

        for (name, style_mode, height, composer_text, tail_state) in [
            ("plain draft", StyleMode::Ms, 6, "draft", TailState::Plain),
            (
                "status line with tall draft",
                StyleMode::Ms,
                6,
                "1\n2\n3\n4\n5\n6\n7\n8",
                TailState::StatusLine,
            ),
            (
                "command panel",
                StyleMode::Ms,
                6,
                "/",
                TailState::CommandPanel,
            ),
            ("framed draft", StyleMode::Cc, 3, "draft", TailState::Plain),
            (
                "framed tall draft",
                StyleMode::Cc,
                4,
                "1\n2\n3\n4\n5\n6",
                TailState::Plain,
            ),
        ] {
            let mut model = Model::new_with_style_mode(HeroOptions::default(), style_mode);
            model.transcript_mut().clear();
            model.transcript_mut().set_gap(0);
            for index in 0..48 {
                model
                    .transcript_mut()
                    .append_message(Sender::Assistant, format!("item {index}"));
            }
            model.set_window(24, height);
            model.set_palette(default_palette(), true);
            match tail_state {
                TailState::Plain => {}
                TailState::StatusLine => {
                    model.status_line_items = vec![StatusLineItem::GitBranch];
                    model.git_branch = "main".to_string();
                }
                TailState::CommandPanel => {}
            }
            model.composer_mut().set_text_for_test(composer_text);
            model.sync_command_panel_navigation();
            model.sync_composer_height();
            model.sync_transcript_render();
            model.sync_document_viewport_to_bottom();

            let layout = model.build_document_layout();
            let visible_transcript_indices = model
                .document_viewport_line_indices(&layout)
                .into_iter()
                .filter(|line_index| *line_index < layout.transcript_line_count)
                .collect::<Vec<_>>();
            let expected_window = visible_transcript_indices
                .first()
                .copied()
                .map(|start| (start, visible_transcript_indices.len()));

            assert_eq!(
                model.current_visible_transcript_window(layout.transcript_line_count),
                expected_window,
                "{name} should derive the warmed transcript window from the actual viewport line indices"
            );
        }
    }

    #[test]
    fn current_visible_transcript_window_reresolves_manual_scroll_viewport_after_resize() {
        let mut model = Model::new_with_style_mode(HeroOptions::default(), StyleMode::Ms);
        model.transcript_mut().clear();
        model.transcript_mut().set_gap(0);
        model.transcript_mut().append_message(
            Sender::Assistant,
            "alpha beta gamma delta epsilon zeta eta theta iota kappa lambda mu nu xi omicron pi rho sigma tau upsilon phi chi psi omega",
        );
        model
            .transcript_mut()
            .append_message(Sender::Assistant, "target item");
        model
            .transcript_mut()
            .append_message(Sender::Assistant, "tail item");
        model.set_window(24, 4);
        model.set_palette(default_palette(), true);
        model.sync_transcript_render();

        let layout = model.build_document_layout();
        let target_document_line = (0..layout.line_count())
            .find(|&line_index| {
                layout.line_anchor_at(line_index).is_some_and(|anchor| {
                    anchor.region == DocumentAnchorRegion::Transcript
                        && anchor.transcript.item_index == 1
                })
            })
            .expect("target item should exist in the initial transcript layout");
        let document_offset = target_document_line;
        model.apply_document_viewport_position(&layout, document_offset, 0, false, true);
        let preserved_viewport_state = model.current_document_viewport_state();

        model.set_window(12, 4);

        let transcript_line_count = model.transcript.item_metrics_index().line_count;
        let resized_layout = model.build_document_layout();
        let resized_target_document_line = (0..resized_layout.line_count())
            .find(|&line_index| {
                resized_layout
                    .line_anchor_at(line_index)
                    .is_some_and(|anchor| {
                        anchor.region == DocumentAnchorRegion::Transcript
                            && anchor.transcript.item_index == 1
                    })
            })
            .expect("target item should still exist after resize");
        let expected_offset = preserved_viewport_state
            .resolve_offset(&resized_layout, model.document_viewport_height());
        let stale_offset = preserved_viewport_state.resolved_offset();
        let expected_window = model
            .document_viewport_line_indices_for_mode(
                &resized_layout,
                expected_offset,
                preserved_viewport_state.follow_bottom(),
                preserved_viewport_state.manual_scroll(),
            )
            .into_iter()
            .filter(|line_index| *line_index < transcript_line_count)
            .collect::<Vec<_>>();
        let stale_window = model
            .document_viewport_line_indices_for_mode(
                &resized_layout,
                stale_offset,
                preserved_viewport_state.follow_bottom(),
                preserved_viewport_state.manual_scroll(),
            )
            .into_iter()
            .filter(|line_index| *line_index < transcript_line_count)
            .collect::<Vec<_>>();
        let expected_window = expected_window
            .first()
            .copied()
            .map(|start| (start, expected_window.len()));

        assert_ne!(
            expected_offset, stale_offset,
            "test fixture should force manual-scroll restore to resolve a different offset after reflow (before={target_document_line}, after={resized_target_document_line})"
        );
        assert_ne!(
            stale_window
                .first()
                .copied()
                .map(|start| (start, stale_window.len())),
            expected_window,
            "test fixture should expose a mismatch between stale and re-resolved viewport windows"
        );
        assert_eq!(
            model.current_visible_transcript_window(transcript_line_count),
            expected_window,
            "manual-scroll prewarm should follow the re-resolved viewport that will be restored after resize"
        );
    }

    #[test]
    fn transcript_prewarm_skips_when_document_viewport_is_unavailable() {
        #[derive(Clone, Copy)]
        enum ViewportState {
            MissingWindow,
            ZeroHeight,
        }

        for viewport_state in [ViewportState::MissingWindow, ViewportState::ZeroHeight] {
            let mut model = Model::new_with_style_mode(HeroOptions::default(), StyleMode::Ms);
            model.set_window(24, 6);
            model.set_palette(default_palette(), true);
            model.transcript_mut().clear();
            model.transcript_mut().set_gap(0);
            for index in 0..96 {
                model
                    .transcript_mut()
                    .append_message(Sender::Assistant, format!("item {index}"));
            }

            model.sync_transcript_render();
            assert!(
                !model
                    .transcript
                    .cached_screen_blocks_snapshot()
                    .borrow()
                    .is_empty(),
                "test fixture should prewarm transcript blocks before making the viewport unavailable"
            );

            match viewport_state {
                ViewportState::MissingWindow => {
                    model.has_window = false;
                }
                ViewportState::ZeroHeight => {
                    model.height = 0;
                }
            }

            assert_eq!(model.document_viewport_height(), 0);
            let transcript_line_count = model.transcript.item_metrics_index().line_count;
            assert_eq!(
                model.current_visible_transcript_window(transcript_line_count),
                None,
                "unavailable viewport should not report any transcript line as visible"
            );

            model.sync_transcript_render();
            assert!(
                model
                    .transcript
                    .cached_screen_blocks_snapshot()
                    .borrow()
                    .is_empty(),
                "sync_transcript_render should not retain transcript blocks when no viewport is available"
            );

            model.document_transcript_cache = Default::default();
            let _snapshot = model.current_document_transcript_snapshot();
            assert!(
                model
                    .transcript
                    .cached_screen_blocks_snapshot()
                    .borrow()
                    .is_empty(),
                "document transcript snapshots should not retain transcript blocks when no viewport is available"
            );
        }
    }
}
