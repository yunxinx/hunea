//! PortableModeConfirm step：全局配置不可用时，询问用户是否进入便携模式。
//!
//! 用户选 Yes：由 screen 调用 `write_portable_marker` 创建标记文件，
//! 并将 `data_dir_resolution` 切换为 `Portable`。
//! 用户选 No（或按 q）：`should_exit = true`，退出应用。

use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::Modifier,
    text::{Line, Span},
    widgets::{Paragraph, Widget, Wrap},
};
use terminal_ui::theme::{
    TerminalPalette, muted_text_style, primary_text_style, secondary_text_style,
    tertiary_text_style,
};

use super::layout::{inset_spans, inset_styled, option_line, rule_line, title_line};
use crate::precheck::step::{KeyboardHandler, StepRenderer, StepState, StepStateProvider};

/// 用户在 Yes/No 之间的选择。`None` 表示尚未确认。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ConfirmSelection {
    Yes,
    No,
}

#[derive(Debug)]
pub(crate) struct PortableModeConfirmWidget {
    pub(crate) working_dir: PathBuf,
    highlighted: ConfirmSelection,
    pub(crate) selection: Option<ConfirmSelection>,
    pub(crate) activated: bool,
    palette: TerminalPalette,
}

impl PortableModeConfirmWidget {
    pub(crate) fn new(working_dir: PathBuf, palette: TerminalPalette) -> Self {
        Self {
            working_dir,
            highlighted: ConfirmSelection::Yes,
            selection: None,
            activated: false,
            palette,
        }
    }

    fn toggle_highlight(&mut self) {
        self.highlighted = match self.highlighted {
            ConfirmSelection::Yes => ConfirmSelection::No,
            ConfirmSelection::No => ConfirmSelection::Yes,
        };
    }

    fn confirm(&mut self) {
        self.selection = Some(self.highlighted);
    }

    fn quit(&mut self) {
        self.highlighted = ConfirmSelection::No;
        self.selection = Some(ConfirmSelection::No);
    }
}

impl StepStateProvider for PortableModeConfirmWidget {
    fn step_state(&self) -> StepState {
        // Yes 选中后仍保持 InProgress，直到 screen 写完 marker 并置 `activated`。
        // 这样写盘失败时 step 不会被误判为 Complete，event loop 能传播错误。
        if self.activated || self.selection == Some(ConfirmSelection::No) {
            StepState::Complete
        } else {
            StepState::InProgress
        }
    }
}

impl KeyboardHandler for PortableModeConfirmWidget {
    fn handle_key_event(&mut self, key: KeyEvent) {
        // Release 过滤在 PrecheckScreen。
        match key.code {
            KeyCode::Up | KeyCode::Char('k') | KeyCode::Char('K') => self.toggle_highlight(),
            KeyCode::Down | KeyCode::Char('j') | KeyCode::Char('J') => self.toggle_highlight(),
            KeyCode::Enter => self.confirm(),
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('Q') => self.quit(),
            _ => {}
        }
    }
}

impl StepRenderer for PortableModeConfirmWidget {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        let body_style = secondary_text_style(self.palette);
        let hint_style = tertiary_text_style(self.palette).add_modifier(Modifier::ITALIC);
        let selected = primary_text_style(self.palette).add_modifier(Modifier::BOLD);
        let unselected = secondary_text_style(self.palette);

        let lines = vec![
            title_line("Portable mode", self.palette),
            rule_line(area.width, self.palette),
            Line::raw(""),
            inset_styled(
                "Hunea can run in portable mode, using .hunea/ in the current",
                body_style,
            ),
            inset_styled("working directory for config and session data.", body_style),
            Line::raw(""),
            inset_spans(vec![
                Span::raw("Working directory: "),
                Span::styled(
                    self.working_dir.to_string_lossy().to_string(),
                    muted_text_style(self.palette),
                ),
            ]),
            Line::raw(""),
            option_line(
                self.highlighted == ConfirmSelection::Yes,
                "Yes, continue in portable mode",
                selected,
                unselected,
                self.palette,
            ),
            option_line(
                self.highlighted == ConfirmSelection::No,
                "No, quit",
                selected,
                unselected,
                self.palette,
            ),
            Line::raw(""),
            inset_styled("Esc quit · Enter confirm · ↑/↓/j/k move", hint_style),
        ];

        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .render(area, buf);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use std::path::PathBuf;
    use terminal_ui::theme::default_palette;

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn widget() -> PortableModeConfirmWidget {
        PortableModeConfirmWidget::new(PathBuf::from("/tmp/work"), default_palette())
    }

    #[test]
    fn starts_in_progress_with_yes_highlighted() {
        let w = widget();
        assert_eq!(w.step_state(), StepState::InProgress);
        assert_eq!(w.highlighted, ConfirmSelection::Yes);
        assert_eq!(w.selection, None);
    }

    #[test]
    fn down_key_toggles_to_no() {
        let mut w = widget();
        w.handle_key_event(press(KeyCode::Down));
        assert_eq!(w.highlighted, ConfirmSelection::No);
    }

    #[test]
    fn j_key_toggles_to_no() {
        let mut w = widget();
        w.handle_key_event(press(KeyCode::Char('j')));
        assert_eq!(w.highlighted, ConfirmSelection::No);
    }

    #[test]
    fn up_key_toggles_back_to_yes() {
        let mut w = widget();
        w.handle_key_event(press(KeyCode::Down));
        w.handle_key_event(press(KeyCode::Up));
        assert_eq!(w.highlighted, ConfirmSelection::Yes);
    }

    #[test]
    fn k_key_toggles_back_to_yes() {
        let mut w = widget();
        w.handle_key_event(press(KeyCode::Char('j')));
        w.handle_key_event(press(KeyCode::Char('k')));
        assert_eq!(w.highlighted, ConfirmSelection::Yes);
    }

    #[test]
    fn enter_on_yes_sets_selection_without_completing() {
        let mut w = widget();
        w.handle_key_event(press(KeyCode::Enter));
        assert_eq!(w.selection, Some(ConfirmSelection::Yes));
        assert_eq!(w.step_state(), StepState::InProgress);
    }

    #[test]
    fn enter_on_no_sets_selection_and_completes() {
        let mut w = widget();
        w.handle_key_event(press(KeyCode::Down));
        w.handle_key_event(press(KeyCode::Enter));
        assert_eq!(w.selection, Some(ConfirmSelection::No));
        assert_eq!(w.step_state(), StepState::Complete);
    }

    #[test]
    fn q_key_quits_immediately() {
        let mut w = widget();
        w.handle_key_event(press(KeyCode::Char('q')));
        assert_eq!(w.selection, Some(ConfirmSelection::No));
        assert_eq!(w.highlighted, ConfirmSelection::No);
        assert_eq!(w.step_state(), StepState::Complete);
    }

    #[test]
    fn esc_key_quits_immediately() {
        let mut w = widget();
        w.handle_key_event(press(KeyCode::Esc));
        assert_eq!(w.selection, Some(ConfirmSelection::No));
        assert_eq!(w.highlighted, ConfirmSelection::No);
        assert_eq!(w.step_state(), StepState::Complete);
    }

    #[test]
    fn other_keys_are_ignored() {
        let mut w = widget();
        w.handle_key_event(press(KeyCode::Char('x')));
        w.handle_key_event(press(KeyCode::Tab));
        w.handle_key_event(press(KeyCode::Backspace));
        assert_eq!(w.selection, None);
        assert_eq!(w.highlighted, ConfirmSelection::Yes);
    }

    #[test]
    fn activated_flag_completes_yes_path() {
        let mut w = widget();
        w.handle_key_event(press(KeyCode::Enter));
        w.activated = true;
        assert_eq!(w.step_state(), StepState::Complete);
    }

    #[test]
    fn uppercase_q_also_quits() {
        let mut w = widget();
        w.handle_key_event(press(KeyCode::Char('Q')));
        assert_eq!(w.selection, Some(ConfirmSelection::No));
    }
}
