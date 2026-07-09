//! ConfigAccessibility step：告知用户全局配置目录不可访问的具体错误。
//!
//! 此 step 主要起"告知"作用，用户按任意键后 Complete，继续到下一步（通常是 PortableModeConfirm）。

use crossterm::event::KeyEvent;
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::Modifier,
    text::{Line, Span},
    widgets::{Paragraph, Widget, Wrap},
};
use terminal_ui::theme::{
    TerminalPalette, muted_text_style, primary_text_style, system_error_text_style,
    tertiary_text_style,
};

use super::layout::{inset_spans, inset_styled, rule_line, title_line};
use crate::precheck::accessibility::Accessibility;
use crate::precheck::step::{KeyboardHandler, StepRenderer, StepState, StepStateProvider};

#[derive(Debug)]
pub(crate) struct ConfigAccessibilityWidget {
    accessibility: Accessibility,
    palette: TerminalPalette,
    completed: bool,
}

impl ConfigAccessibilityWidget {
    pub(crate) fn new(accessibility: Accessibility, palette: TerminalPalette) -> Self {
        Self {
            accessibility,
            palette,
            completed: false,
        }
    }
}

impl StepStateProvider for ConfigAccessibilityWidget {
    fn step_state(&self) -> StepState {
        if self.completed {
            StepState::Complete
        } else {
            StepState::InProgress
        }
    }
}

impl KeyboardHandler for ConfigAccessibilityWidget {
    fn handle_key_event(&mut self, _key: KeyEvent) {
        // Release 过滤在 PrecheckScreen；此处只处理已放行的 Press/Repeat。
        self.completed = true;
    }
}

impl StepRenderer for ConfigAccessibilityWidget {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        let Accessibility::Unavailable {
            read_error,
            write_error,
        } = &self.accessibility
        else {
            return;
        };

        let error_style = system_error_text_style(self.palette);
        let ok_style = muted_text_style(self.palette);
        let body_style = primary_text_style(self.palette);
        let hint_style = tertiary_text_style(self.palette).add_modifier(Modifier::ITALIC);

        let mut lines: Vec<Line> = vec![
            title_line("Configuration access error", self.palette),
            rule_line(area.width, self.palette),
            Line::raw(""),
            inset_styled(
                "The global config directory (~/.config/hunea/) is not accessible:",
                body_style,
            ),
            Line::raw(""),
        ];
        match read_error {
            Some(err) => lines.push(inset_spans(vec![
                Span::raw("Read:  "),
                Span::styled(err.clone(), error_style),
            ])),
            None => lines.push(inset_spans(vec![
                Span::raw("Read:  "),
                Span::styled("ok", ok_style),
            ])),
        }
        match write_error {
            Some(err) => lines.push(inset_spans(vec![
                Span::raw("Write: "),
                Span::styled(err.clone(), error_style),
            ])),
            None => lines.push(inset_spans(vec![
                Span::raw("Write: "),
                Span::styled("ok", ok_style),
            ])),
        }
        lines.push(Line::raw(""));
        lines.push(inset_styled("Press any key to continue", hint_style));

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

    fn unavailable(read: Option<&str>, write: Option<&str>) -> Accessibility {
        Accessibility::Unavailable {
            read_error: read.map(str::to_string),
            write_error: write.map(str::to_string),
        }
    }

    fn make_widget(accessibility: Accessibility) -> ConfigAccessibilityWidget {
        ConfigAccessibilityWidget::new(accessibility, default_palette())
    }

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn starts_in_progress() {
        let widget = make_widget(unavailable(Some("denied"), Some("denied")));
        assert_eq!(widget.step_state(), StepState::InProgress);
    }

    #[test]
    fn any_press_completes() {
        let mut widget = make_widget(unavailable(Some("denied"), None));
        widget.handle_key_event(press(KeyCode::Enter));
        assert_eq!(widget.step_state(), StepState::Complete);
    }

    #[test]
    fn space_key_also_completes() {
        let mut widget = make_widget(unavailable(Some("denied"), None));
        widget.handle_key_event(press(KeyCode::Char(' ')));
        assert_eq!(widget.step_state(), StepState::Complete);
    }
}
