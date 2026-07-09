//! SearchToolPrecheck：fd/rg 缺失时选 Download / Fallback / Quit。
//!
//! 下载在独立线程 + current_thread runtime 中跑，进度经 channel 回传。
//! Downloading 中 Esc/Ctrl-C 取消回 Choosing；Choosing/Failed 中 Esc/Ctrl-C/q 退出应用。

use std::path::PathBuf;
use std::time::Instant;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::Modifier,
    widgets::{Paragraph, Widget, Wrap},
};
use tokio::sync::mpsc::UnboundedReceiver;
use tokio_util::sync::CancellationToken;
use tool_runtime::builtin::{ManagedToolKind, ManagedToolProgress};

use super::layout::{inset_styled, option_line, rule_line, title_line};
use super::search_tool_progress::{
    format_empty_progress_bar, format_full_progress_bar, format_progress_bar,
    format_transfer_stats, stage_label,
};
use crate::precheck::managed_search::{ManagedSearchOutcome, spawn_managed_install};
use crate::precheck::step::{KeyboardHandler, StepRenderer, StepState, StepStateProvider};
use terminal_ui::theme::{
    TerminalPalette, muted_text_style, primary_text_style, secondary_text_style,
    tertiary_text_style,
};

const FULL_CHOICES: &[SearchToolChoice] = &[
    SearchToolChoice::Download,
    SearchToolChoice::Fallback,
    SearchToolChoice::Quit,
];
const ANDROID_CHOICES: &[SearchToolChoice] = &[SearchToolChoice::Fallback, SearchToolChoice::Quit];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WidgetStatus {
    Choosing,
    Downloading,
    Failed,
    Complete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SearchToolChoice {
    Download,
    Fallback,
    Quit,
}

pub(crate) struct SearchToolPrecheckWidget {
    tool: ManagedToolKind,
    status: WidgetStatus,
    highlighted: SearchToolChoice,
    progress: Option<ManagedToolProgress>,
    download_error: Option<String>,
    outcome: Option<ManagedSearchOutcome>,
    is_android: bool,
    should_exit: bool,
    palette: TerminalPalette,
    cancellation: Option<CancellationToken>,
    download_handle: Option<std::thread::JoinHandle<()>>,
    progress_rx: Option<UnboundedReceiver<ManagedToolProgress>>,
    managed_root: PathBuf,
    /// managed 存在但 spawn 失败时为 true，文案用 reinstall。
    is_rebuild: bool,
    download_started_at: Option<Instant>,
}

impl SearchToolPrecheckWidget {
    pub(crate) fn new(
        tool: ManagedToolKind,
        is_android: bool,
        palette: TerminalPalette,
        managed_root: PathBuf,
        is_rebuild: bool,
    ) -> Self {
        let highlighted = if is_android {
            SearchToolChoice::Fallback
        } else {
            SearchToolChoice::Download
        };
        Self {
            tool,
            status: WidgetStatus::Choosing,
            highlighted,
            progress: None,
            download_error: None,
            outcome: None,
            is_android,
            should_exit: false,
            palette,
            cancellation: None,
            download_handle: None,
            progress_rx: None,
            managed_root,
            is_rebuild,
            download_started_at: None,
        }
    }

    /// resolution 切换后同步安装根；进行中的下载仍用旧 root，取消后重试用新 root。
    pub(crate) fn set_managed_root(&mut self, root: PathBuf) {
        self.managed_root = root;
    }

    pub(crate) fn is_downloading(&self) -> bool {
        self.status == WidgetStatus::Downloading
    }

    fn available_choices(&self) -> &'static [SearchToolChoice] {
        if self.is_android {
            ANDROID_CHOICES
        } else {
            FULL_CHOICES
        }
    }

    fn move_highlight(&mut self, forward: bool) {
        let choices = self.available_choices();
        let idx = choices
            .iter()
            .position(|c| *c == self.highlighted)
            .unwrap_or(0);
        let len = choices.len();
        let new_idx = if forward {
            (idx + 1) % len
        } else {
            (idx + len - 1) % len
        };
        self.highlighted = choices[new_idx];
    }

    fn confirm(&mut self) {
        match self.highlighted {
            SearchToolChoice::Download => self.start_download(),
            SearchToolChoice::Fallback => {
                self.outcome = Some(ManagedSearchOutcome::Rejected(self.tool));
                self.status = WidgetStatus::Complete;
            }
            SearchToolChoice::Quit => self.quit(),
        }
    }

    fn quit(&mut self) {
        self.should_exit = true;
        self.status = WidgetStatus::Complete;
    }

    fn start_download(&mut self) {
        let cancellation = CancellationToken::new();
        let (join, rx) =
            spawn_managed_install(self.tool, self.managed_root.clone(), cancellation.clone());
        self.cancellation = Some(cancellation);
        self.download_handle = Some(join);
        self.progress_rx = Some(rx);
        self.progress = None;
        self.download_error = None;
        self.download_started_at = Some(Instant::now());
        self.status = WidgetStatus::Downloading;
    }

    fn cancel_download(&mut self) {
        if let Some(cancellation) = self.cancellation.take() {
            cancellation.cancel();
        }
        // JoinHandle drop 只 detach；靠 cancel 让任务在 cancellation point 清理临时目录。
        self.download_handle = None;
        self.progress_rx = None;
        self.progress = None;
        self.download_error = None;
        self.download_started_at = None;
        self.status = WidgetStatus::Choosing;
        self.highlighted = if self.is_android {
            SearchToolChoice::Fallback
        } else {
            SearchToolChoice::Download
        };
    }

    pub(crate) fn poll_progress(&mut self) {
        let Some(rx) = self.progress_rx.as_mut() else {
            return;
        };
        while let Ok(progress) = rx.try_recv() {
            match &progress {
                ManagedToolProgress::Ready { .. } => {
                    self.outcome = Some(ManagedSearchOutcome::Authorized(self.tool));
                    self.status = WidgetStatus::Complete;
                    self.progress = Some(progress);
                    self.cancellation = None;
                    self.download_handle = None;
                    self.progress_rx = None;
                    self.download_started_at = None;
                    return;
                }
                ManagedToolProgress::Failed { error } => {
                    self.download_error = Some(error.clone());
                    self.status = WidgetStatus::Failed;
                    self.highlighted = if self.is_android {
                        SearchToolChoice::Fallback
                    } else {
                        SearchToolChoice::Download
                    };
                    self.progress = None;
                    self.cancellation = None;
                    self.download_handle = None;
                    self.progress_rx = None;
                    self.download_started_at = None;
                    return;
                }
                _ => {
                    self.progress = Some(progress);
                }
            }
        }
    }

    pub(crate) fn take_outcome(&mut self) -> Option<ManagedSearchOutcome> {
        self.outcome.take()
    }

    pub(crate) fn wants_exit(&self) -> bool {
        self.should_exit
    }

    pub(crate) fn abort_download(&mut self) {
        if let Some(cancellation) = self.cancellation.take() {
            cancellation.cancel();
        }
        self.download_handle = None;
        self.progress_rx = None;
        self.download_started_at = None;
    }

    fn tool_label(&self) -> String {
        format!("{} {}", self.tool.display_name(), self.tool.version())
    }

    fn is_ctrl_c(key: &KeyEvent) -> bool {
        key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL)
    }
}

impl StepStateProvider for SearchToolPrecheckWidget {
    fn step_state(&self) -> StepState {
        match self.status {
            WidgetStatus::Complete => StepState::Complete,
            _ => StepState::InProgress,
        }
    }
}

impl KeyboardHandler for SearchToolPrecheckWidget {
    fn handle_key_event(&mut self, key: KeyEvent) {
        match self.status {
            WidgetStatus::Choosing => match key.code {
                KeyCode::Up | KeyCode::Char('k') | KeyCode::Char('K') => self.move_highlight(false),
                KeyCode::Down | KeyCode::Char('j') | KeyCode::Char('J') => {
                    self.move_highlight(true)
                }
                KeyCode::Enter => self.confirm(),
                KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('Q') => self.quit(),
                _ if Self::is_ctrl_c(&key) => self.quit(),
                _ => {}
            },
            WidgetStatus::Downloading => {
                if key.code == KeyCode::Esc || Self::is_ctrl_c(&key) {
                    self.cancel_download();
                }
            }
            WidgetStatus::Failed => match key.code {
                KeyCode::Up | KeyCode::Char('k') | KeyCode::Char('K') => self.move_highlight(false),
                KeyCode::Down | KeyCode::Char('j') | KeyCode::Char('J') => {
                    self.move_highlight(true)
                }
                KeyCode::Enter => self.confirm(),
                KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('Q') => self.quit(),
                _ if Self::is_ctrl_c(&key) => self.quit(),
                _ => {}
            },
            WidgetStatus::Complete => {}
        }
    }
}

impl StepRenderer for SearchToolPrecheckWidget {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        let body_style = secondary_text_style(self.palette);
        let hint_style = tertiary_text_style(self.palette).add_modifier(Modifier::ITALIC);
        let selected = primary_text_style(self.palette).add_modifier(Modifier::BOLD);
        let unselected = secondary_text_style(self.palette);

        let lines = match self.status {
            WidgetStatus::Choosing => {
                self.render_choosing(area.width, body_style, selected, unselected, hint_style)
            }
            WidgetStatus::Downloading => {
                self.render_downloading(area.width, body_style, hint_style)
            }
            WidgetStatus::Failed => {
                self.render_failed(area.width, body_style, selected, unselected, hint_style)
            }
            WidgetStatus::Complete => return,
        };

        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .render(area, buf);
    }
}

impl SearchToolPrecheckWidget {
    fn render_choosing(
        &self,
        area_width: u16,
        body_style: ratatui::style::Style,
        selected: ratatui::style::Style,
        unselected: ratatui::style::Style,
        hint_style: ratatui::style::Style,
    ) -> Vec<ratatui::text::Line<'static>> {
        let title = format!("Search tool: {}", self.tool_label());
        let mut lines = vec![
            title_line(&title, self.palette),
            rule_line(area_width, self.palette),
            ratatui::text::Line::raw(""),
        ];

        if self.is_android {
            lines.push(inset_styled(
                "Detected Termux/Android. Managed download is unavailable",
                body_style,
            ));
            lines.push(inset_styled(
                "(Linux binaries are incompatible with Termux).",
                body_style,
            ));
            lines.push(inset_styled(
                "Install via the system package manager:",
                body_style,
            ));
            lines.push(inset_styled(
                "  pkg install ripgrep fd",
                muted_text_style(self.palette),
            ));
        } else if self.is_rebuild {
            lines.push(inset_styled(
                format!(
                    "{} was found but appears corrupted or incompatible.",
                    self.tool.display_name()
                ),
                body_style,
            ));
            lines.push(inset_styled(
                "Reinstall from GitHub Releases (pinned, SHA256-verified), or use",
                body_style,
            ));
            lines.push(inset_styled("the built-in Rust fallback.", body_style));
        } else {
            lines.push(inset_styled(
                format!(
                    "{} was not found on PATH, bundled, or managed cache.",
                    self.tool.display_name()
                ),
                body_style,
            ));
            lines.push(inset_styled(
                "Download from GitHub Releases (pinned, SHA256-verified), or use",
                body_style,
            ));
            lines.push(inset_styled("the built-in Rust fallback.", body_style));
        }

        lines.push(ratatui::text::Line::raw(""));

        for choice in self.available_choices() {
            let label = match choice {
                SearchToolChoice::Download => {
                    if self.is_rebuild {
                        format!("Reinstall ({})", self.tool_label())
                    } else {
                        format!("Download & install ({})", self.tool_label())
                    }
                }
                SearchToolChoice::Fallback => "Use built-in Rust fallback".to_string(),
                SearchToolChoice::Quit => "Quit".to_string(),
            };
            lines.push(option_line(
                self.highlighted == *choice,
                &label,
                selected,
                unselected,
                self.palette,
            ));
        }

        lines.push(ratatui::text::Line::raw(""));
        lines.push(inset_styled(
            "Esc/Ctrl-C quit · Enter confirm · ↑/↓/j/k move",
            hint_style,
        ));
        lines
    }

    fn render_downloading(
        &self,
        area_width: u16,
        body_style: ratatui::style::Style,
        hint_style: ratatui::style::Style,
    ) -> Vec<ratatui::text::Line<'static>> {
        // 固定 3 行内容槽（阶段/条/统计），避免阶段切换时 hint 抖动。
        let bar_width = (area_width as usize)
            .saturating_sub(super::layout::LEFT_INSET.chars().count())
            .clamp(20, 40);

        let mut lines = vec![
            title_line(&format!("Downloading {}", self.tool_label()), self.palette),
            rule_line(area_width, self.palette),
            ratatui::text::Line::raw(""),
        ];

        match self.progress.as_ref() {
            Some(ManagedToolProgress::Downloading {
                bytes_received,
                bytes_total,
            }) => {
                lines.push(inset_styled("Downloading", body_style));
                lines.push(inset_styled(
                    format_progress_bar(*bytes_received, *bytes_total, bar_width),
                    body_style,
                ));
                lines.push(inset_styled(
                    format_transfer_stats(*bytes_received, *bytes_total, self.download_started_at),
                    body_style,
                ));
            }
            Some(progress) => {
                lines.push(inset_styled(stage_label(progress), body_style));
                lines.push(inset_styled(
                    format_full_progress_bar(bar_width),
                    body_style,
                ));
                lines.push(ratatui::text::Line::raw(""));
            }
            None => {
                lines.push(inset_styled("Starting...", body_style));
                lines.push(inset_styled(
                    format_empty_progress_bar(bar_width),
                    body_style,
                ));
                lines.push(ratatui::text::Line::raw(""));
            }
        }

        lines.push(ratatui::text::Line::raw(""));
        lines.push(inset_styled("Esc/Ctrl-C cancel", hint_style));
        lines
    }

    fn render_failed(
        &self,
        area_width: u16,
        body_style: ratatui::style::Style,
        selected: ratatui::style::Style,
        unselected: ratatui::style::Style,
        hint_style: ratatui::style::Style,
    ) -> Vec<ratatui::text::Line<'static>> {
        let error = self.download_error.clone().unwrap_or_default();
        let mut lines = vec![
            title_line(
                &format!("Download failed: {}", self.tool_label()),
                self.palette,
            ),
            rule_line(area_width, self.palette),
            ratatui::text::Line::raw(""),
        ];
        for line in error.lines() {
            lines.push(inset_styled(line, body_style));
        }
        lines.push(ratatui::text::Line::raw(""));

        for choice in self.available_choices() {
            let label = match choice {
                SearchToolChoice::Download => format!("Retry download ({})", self.tool_label()),
                SearchToolChoice::Fallback => "Use built-in Rust fallback".to_string(),
                SearchToolChoice::Quit => "Quit".to_string(),
            };
            lines.push(option_line(
                self.highlighted == *choice,
                &label,
                selected,
                unselected,
                self.palette,
            ));
        }

        lines.push(ratatui::text::Line::raw(""));
        lines.push(inset_styled(
            "Esc/Ctrl-C quit · Enter confirm · ↑/↓/j/k move",
            hint_style,
        ));
        lines
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use terminal_ui::theme::default_palette;

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl_c() -> KeyEvent {
        KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)
    }

    fn widget() -> SearchToolPrecheckWidget {
        SearchToolPrecheckWidget::new(
            ManagedToolKind::Ripgrep,
            false,
            default_palette(),
            PathBuf::from("/tmp/fake-managed-root"),
            false,
        )
    }

    fn android_widget() -> SearchToolPrecheckWidget {
        SearchToolPrecheckWidget::new(
            ManagedToolKind::Ripgrep,
            true,
            default_palette(),
            PathBuf::from("/tmp/fake-managed-root"),
            false,
        )
    }

    #[test]
    fn starts_in_choosing_with_download_highlighted() {
        let w = widget();
        assert_eq!(w.status, WidgetStatus::Choosing);
        assert_eq!(w.highlighted, SearchToolChoice::Download);
        assert_eq!(w.step_state(), StepState::InProgress);
    }

    #[test]
    fn android_starts_with_fallback_highlighted() {
        let w = android_widget();
        assert_eq!(w.highlighted, SearchToolChoice::Fallback);
    }

    #[test]
    fn down_moves_to_fallback() {
        let mut w = widget();
        w.handle_key_event(press(KeyCode::Down));
        assert_eq!(w.highlighted, SearchToolChoice::Fallback);
    }

    #[test]
    fn up_from_download_wraps_to_quit() {
        let mut w = widget();
        w.handle_key_event(press(KeyCode::Up));
        assert_eq!(w.highlighted, SearchToolChoice::Quit);
    }

    #[test]
    fn enter_fallback_completes_rejected() {
        let mut w = widget();
        w.handle_key_event(press(KeyCode::Down));
        w.handle_key_event(press(KeyCode::Enter));
        assert_eq!(w.status, WidgetStatus::Complete);
        assert_eq!(
            w.take_outcome(),
            Some(ManagedSearchOutcome::Rejected(ManagedToolKind::Ripgrep))
        );
    }

    #[test]
    fn enter_quit_completes_with_exit() {
        let mut w = widget();
        w.handle_key_event(press(KeyCode::Up));
        w.handle_key_event(press(KeyCode::Enter));
        assert_eq!(w.status, WidgetStatus::Complete);
        assert!(w.wants_exit());
    }

    #[test]
    fn esc_in_choosing_exits() {
        let mut w = widget();
        w.handle_key_event(press(KeyCode::Esc));
        assert_eq!(w.status, WidgetStatus::Complete);
        assert!(w.wants_exit());
    }

    #[test]
    fn q_in_choosing_exits() {
        let mut w = widget();
        w.handle_key_event(press(KeyCode::Char('q')));
        assert!(w.wants_exit());
    }

    #[test]
    fn ctrl_c_in_choosing_exits() {
        let mut w = widget();
        w.handle_key_event(ctrl_c());
        assert_eq!(w.status, WidgetStatus::Complete);
        assert!(w.wants_exit());
    }

    #[test]
    fn android_has_no_download_choice() {
        let w = android_widget();
        assert!(!w.available_choices().contains(&SearchToolChoice::Download));
    }

    #[test]
    fn android_enter_fallback_completes_rejected() {
        let mut w = android_widget();
        w.handle_key_event(press(KeyCode::Enter));
        assert_eq!(w.status, WidgetStatus::Complete);
        assert_eq!(
            w.take_outcome(),
            Some(ManagedSearchOutcome::Rejected(ManagedToolKind::Ripgrep))
        );
    }

    #[test]
    fn poll_progress_without_rx_is_noop() {
        let mut w = widget();
        w.poll_progress();
        assert_eq!(w.status, WidgetStatus::Choosing);
    }

    #[test]
    fn cancel_download_without_active_is_noop() {
        let mut w = widget();
        w.cancel_download();
        assert_eq!(w.status, WidgetStatus::Choosing);
    }

    #[test]
    fn abort_download_without_active_is_noop() {
        let mut w = widget();
        w.abort_download();
        assert_eq!(w.status, WidgetStatus::Choosing);
    }
}
