//! PortableModeRecovery step：便携标记存在且全局已恢复可用时，提醒用户可移除标记。
//!
//! 用户选 Continue：保持便携模式，step Complete，正常启动。
//! 用户选 Quit（或按 q）：`should_exit = true`，用户自行删标记后重启回到全局模式。

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::Modifier,
    text::Line,
    widgets::{Paragraph, Widget, Wrap},
};
use terminal_ui::theme::{
    TerminalPalette, muted_text_style, primary_text_style, secondary_text_style,
    tertiary_text_style,
};

use super::layout::{inset_styled, option_line, rule_line, title_line};
use crate::precheck::step::{KeyboardHandler, StepRenderer, StepState, StepStateProvider};

/// 用户在 Continue/Quit 之间的选择。`None` 表示尚未确认。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RecoverySelection {
    Continue,
    Quit,
}

#[derive(Debug)]
pub(crate) struct PortableModeRecoveryWidget {
    highlighted: RecoverySelection,
    pub(crate) selection: Option<RecoverySelection>,
    palette: TerminalPalette,
}

impl PortableModeRecoveryWidget {
    pub(crate) fn new(palette: TerminalPalette) -> Self {
        Self {
            highlighted: RecoverySelection::Continue,
            selection: None,
            palette,
        }
    }

    fn toggle_highlight(&mut self) {
        self.highlighted = match self.highlighted {
            RecoverySelection::Continue => RecoverySelection::Quit,
            RecoverySelection::Quit => RecoverySelection::Continue,
        };
    }

    fn confirm(&mut self) {
        self.selection = Some(self.highlighted);
    }

    fn quit(&mut self) {
        self.highlighted = RecoverySelection::Quit;
        self.selection = Some(RecoverySelection::Quit);
    }
}

impl StepStateProvider for PortableModeRecoveryWidget {
    fn step_state(&self) -> StepState {
        if self.selection.is_some() {
            StepState::Complete
        } else {
            StepState::InProgress
        }
    }
}

impl KeyboardHandler for PortableModeRecoveryWidget {
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

impl StepRenderer for PortableModeRecoveryWidget {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        let body_style = secondary_text_style(self.palette);
        let hint_style = tertiary_text_style(self.palette).add_modifier(Modifier::ITALIC);
        let selected = primary_text_style(self.palette).add_modifier(Modifier::BOLD);
        let unselected = secondary_text_style(self.palette);

        let lines = vec![
            title_line("Portable mode is active", self.palette),
            rule_line(area.width, self.palette),
            Line::raw(""),
            inset_styled(
                "The global config directory (~/.config/hunea/) is now accessible again.",
                body_style,
            ),
            Line::raw(""),
            inset_styled(
                "You can remove the portable marker to switch back to global config and data:",
                body_style,
            ),
            inset_styled("rm .hunea/portable.marker", muted_text_style(self.palette)),
            Line::raw(""),
            inset_styled(
                "Note: session data in .hunea/ will not be migrated automatically.",
                body_style,
            ),
            inset_styled(
                "A migration command will be available in a future release.",
                body_style,
            ),
            Line::raw(""),
            option_line(
                self.highlighted == RecoverySelection::Continue,
                "Continue in portable mode (recommended if you have data here)",
                selected,
                unselected,
                self.palette,
            ),
            option_line(
                self.highlighted == RecoverySelection::Quit,
                "Quit to remove marker and restart with global config",
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
    use terminal_ui::theme::default_palette;

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn starts_in_progress_with_continue_highlighted() {
        let w = PortableModeRecoveryWidget::new(default_palette());
        assert_eq!(w.step_state(), StepState::InProgress);
        assert_eq!(w.highlighted, RecoverySelection::Continue);
        assert_eq!(w.selection, None);
    }

    #[test]
    fn down_key_toggles_to_quit() {
        let mut w = PortableModeRecoveryWidget::new(default_palette());
        w.handle_key_event(press(KeyCode::Down));
        assert_eq!(w.highlighted, RecoverySelection::Quit);
    }

    #[test]
    fn j_key_toggles_to_quit() {
        let mut w = PortableModeRecoveryWidget::new(default_palette());
        w.handle_key_event(press(KeyCode::Char('j')));
        assert_eq!(w.highlighted, RecoverySelection::Quit);
    }

    #[test]
    fn up_key_toggles_back_to_continue() {
        let mut w = PortableModeRecoveryWidget::new(default_palette());
        w.handle_key_event(press(KeyCode::Down));
        w.handle_key_event(press(KeyCode::Up));
        assert_eq!(w.highlighted, RecoverySelection::Continue);
    }

    #[test]
    fn k_key_toggles_back_to_continue() {
        let mut w = PortableModeRecoveryWidget::new(default_palette());
        w.handle_key_event(press(KeyCode::Char('j')));
        w.handle_key_event(press(KeyCode::Char('k')));
        assert_eq!(w.highlighted, RecoverySelection::Continue);
    }

    #[test]
    fn enter_on_continue_completes() {
        let mut w = PortableModeRecoveryWidget::new(default_palette());
        w.handle_key_event(press(KeyCode::Enter));
        assert_eq!(w.selection, Some(RecoverySelection::Continue));
        assert_eq!(w.step_state(), StepState::Complete);
    }

    #[test]
    fn enter_on_quit_completes() {
        let mut w = PortableModeRecoveryWidget::new(default_palette());
        w.handle_key_event(press(KeyCode::Down));
        w.handle_key_event(press(KeyCode::Enter));
        assert_eq!(w.selection, Some(RecoverySelection::Quit));
        assert_eq!(w.step_state(), StepState::Complete);
    }

    #[test]
    fn q_key_quits_immediately() {
        let mut w = PortableModeRecoveryWidget::new(default_palette());
        w.handle_key_event(press(KeyCode::Char('q')));
        assert_eq!(w.selection, Some(RecoverySelection::Quit));
        assert_eq!(w.highlighted, RecoverySelection::Quit);
        assert_eq!(w.step_state(), StepState::Complete);
    }

    #[test]
    fn esc_key_quits_immediately() {
        let mut w = PortableModeRecoveryWidget::new(default_palette());
        w.handle_key_event(press(KeyCode::Esc));
        assert_eq!(w.selection, Some(RecoverySelection::Quit));
        assert_eq!(w.highlighted, RecoverySelection::Quit);
        assert_eq!(w.step_state(), StepState::Complete);
    }

    #[test]
    fn other_keys_are_ignored() {
        let mut w = PortableModeRecoveryWidget::new(default_palette());
        w.handle_key_event(press(KeyCode::Char('x')));
        w.handle_key_event(press(KeyCode::Tab));
        w.handle_key_event(press(KeyCode::Backspace));
        assert_eq!(w.selection, None);
        assert_eq!(w.highlighted, RecoverySelection::Continue);
    }

    #[test]
    fn uppercase_q_also_quits() {
        let mut w = PortableModeRecoveryWidget::new(default_palette());
        w.handle_key_event(press(KeyCode::Char('Q')));
        assert_eq!(w.selection, Some(RecoverySelection::Quit));
    }
}
