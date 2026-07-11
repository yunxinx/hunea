use std::{fmt, io, io::Write, panic};

use crossterm::{
    Command,
    cursor::Show,
    event::{DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TerminalLifecycleOperation {
    EnableRawMode,
    DisableRawMode,
    EnterAlternateScreen,
    LeaveAlternateScreen,
    EnableAlternateScroll,
    DisableAlternateScroll,
    EnableMouseCapture,
    DisableMouseCapture,
    EnableBracketedPaste,
    DisableBracketedPaste,
    ShowCursor,
}

trait TerminalLifecycleOperations {
    fn perform(&mut self, operation: TerminalLifecycleOperation) -> io::Result<()>;
}

struct CrosstermTerminalLifecycleOperations<'a, W> {
    writer: &'a mut W,
}

impl<'a, W> CrosstermTerminalLifecycleOperations<'a, W> {
    fn new(writer: &'a mut W) -> Self {
        Self { writer }
    }
}

impl<W> TerminalLifecycleOperations for CrosstermTerminalLifecycleOperations<'_, W>
where
    W: Write,
{
    fn perform(&mut self, operation: TerminalLifecycleOperation) -> io::Result<()> {
        match operation {
            TerminalLifecycleOperation::EnableRawMode => enable_raw_mode(),
            TerminalLifecycleOperation::DisableRawMode => disable_raw_mode(),
            TerminalLifecycleOperation::EnterAlternateScreen => {
                execute!(self.writer, EnterAlternateScreen)
            }
            TerminalLifecycleOperation::LeaveAlternateScreen => {
                execute!(self.writer, LeaveAlternateScreen)
            }
            TerminalLifecycleOperation::EnableAlternateScroll => {
                execute!(self.writer, EnableAlternateScroll)
            }
            TerminalLifecycleOperation::DisableAlternateScroll => {
                execute!(self.writer, DisableAlternateScroll)
            }
            TerminalLifecycleOperation::EnableMouseCapture => {
                execute!(self.writer, EnableMouseCapture)
            }
            TerminalLifecycleOperation::DisableMouseCapture => {
                execute!(self.writer, DisableMouseCapture)
            }
            TerminalLifecycleOperation::EnableBracketedPaste => {
                execute!(self.writer, EnableBracketedPaste)
            }
            TerminalLifecycleOperation::DisableBracketedPaste => {
                execute!(self.writer, DisableBracketedPaste)
            }
            TerminalLifecycleOperation::ShowCursor => execute!(self.writer, Show),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct EnableAlternateScroll;

impl Command for EnableAlternateScroll {
    fn write_ansi(&self, f: &mut impl fmt::Write) -> fmt::Result {
        write!(f, "\x1b[?1007h")
    }

    #[cfg(windows)]
    fn execute_winapi(&self) -> io::Result<()> {
        Err(io::Error::other(
            "tried to execute EnableAlternateScroll using WinAPI; use ANSI instead",
        ))
    }

    #[cfg(windows)]
    fn is_ansi_code_supported(&self) -> bool {
        true
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DisableAlternateScroll;

impl Command for DisableAlternateScroll {
    fn write_ansi(&self, f: &mut impl fmt::Write) -> fmt::Result {
        write!(f, "\x1b[?1007l")
    }

    #[cfg(windows)]
    fn execute_winapi(&self) -> io::Result<()> {
        Err(io::Error::other(
            "tried to execute DisableAlternateScroll using WinAPI; use ANSI instead",
        ))
    }

    #[cfg(windows)]
    fn is_ansi_code_supported(&self) -> bool {
        true
    }
}

/// `TerminalLifecycleState` 记录退出前仍必须执行的 terminal inverse operation。
#[derive(Debug, Default)]
struct TerminalLifecycleState {
    must_disable_raw_mode: bool,
    must_leave_alternate_screen: bool,
    must_disable_alternate_scroll: bool,
    must_disable_mouse_capture: bool,
    must_disable_bracketed_paste: bool,
    must_show_cursor: bool,
}

impl TerminalLifecycleState {
    fn emergency_restore_state() -> Self {
        Self {
            must_disable_raw_mode: true,
            must_leave_alternate_screen: true,
            must_disable_alternate_scroll: true,
            must_disable_mouse_capture: true,
            must_disable_bracketed_paste: true,
            must_show_cursor: true,
        }
    }
}

impl TerminalLifecycleState {
    fn activate_main_with(
        &mut self,
        operations: &mut impl TerminalLifecycleOperations,
    ) -> io::Result<()> {
        let activation = (|| {
            self.enable_raw_mode_with(operations)?;
            self.enter_alternate_screen_with(operations)?;
            self.enforce_alternate_scroll_disabled_with(operations)?;
            self.enable_mouse_capture_with(operations)?;
            self.enable_bracketed_paste_with(operations)
        })();
        if let Err(error) = activation {
            let _ = self.restore_all_with(operations);
            return Err(error);
        }
        Ok(())
    }

    fn activate_minimal_with(
        &mut self,
        operations: &mut impl TerminalLifecycleOperations,
    ) -> io::Result<()> {
        let activation = (|| {
            self.enable_raw_mode_with(operations)?;
            self.enter_alternate_screen_with(operations)
        })();
        if let Err(error) = activation {
            let _ = self.restore_all_with(operations);
            return Err(error);
        }
        Ok(())
    }

    fn apply_mouse_mode_with(
        &mut self,
        operations: &mut impl TerminalLifecycleOperations,
        has_mouse_capture: bool,
        has_alternate_scroll: bool,
    ) -> io::Result<()> {
        if !has_mouse_capture {
            self.disable_mouse_capture_with(operations)?;
        }
        if !has_alternate_scroll {
            self.disable_alternate_scroll_with(operations)?;
        }
        if has_alternate_scroll {
            self.enable_alternate_scroll_with(operations)?;
        }
        if has_mouse_capture {
            self.enable_mouse_capture_with(operations)?;
        }
        Ok(())
    }

    fn hide_cursor_with(&mut self, hide_cursor: impl FnOnce() -> io::Result<()>) -> io::Result<()> {
        self.must_show_cursor = true;
        hide_cursor()
    }

    fn show_cursor_with(&mut self, show_cursor: impl FnOnce() -> io::Result<()>) -> io::Result<()> {
        if !self.must_show_cursor {
            return Ok(());
        }
        show_cursor()?;
        self.must_show_cursor = false;
        Ok(())
    }

    fn restore_all_with(
        &mut self,
        operations: &mut impl TerminalLifecycleOperations,
    ) -> io::Result<()> {
        let mut first_error = None;
        if self.must_show_cursor {
            match operations.perform(TerminalLifecycleOperation::ShowCursor) {
                Ok(()) => self.must_show_cursor = false,
                Err(error) => record_first_error(&mut first_error, error),
            }
        }
        self.restore_modes_collecting_error(operations, &mut first_error);
        finish_cleanup(first_error)
    }

    fn restore_modes_with(
        &mut self,
        operations: &mut impl TerminalLifecycleOperations,
    ) -> io::Result<()> {
        let mut first_error = None;
        self.restore_modes_collecting_error(operations, &mut first_error);
        finish_cleanup(first_error)
    }

    fn restore_modes_collecting_error(
        &mut self,
        operations: &mut impl TerminalLifecycleOperations,
        first_error: &mut Option<io::Error>,
    ) {
        if self.must_disable_bracketed_paste {
            match operations.perform(TerminalLifecycleOperation::DisableBracketedPaste) {
                Ok(()) => self.must_disable_bracketed_paste = false,
                Err(error) => record_first_error(first_error, error),
            }
        }
        if self.must_disable_mouse_capture {
            match operations.perform(TerminalLifecycleOperation::DisableMouseCapture) {
                Ok(()) => self.must_disable_mouse_capture = false,
                Err(error) => record_first_error(first_error, error),
            }
        }
        if self.must_disable_alternate_scroll {
            match operations.perform(TerminalLifecycleOperation::DisableAlternateScroll) {
                Ok(()) => self.must_disable_alternate_scroll = false,
                Err(error) => record_first_error(first_error, error),
            }
        }
        if self.must_leave_alternate_screen {
            match operations.perform(TerminalLifecycleOperation::LeaveAlternateScreen) {
                Ok(()) => self.must_leave_alternate_screen = false,
                Err(error) => record_first_error(first_error, error),
            }
        }
        if self.must_disable_raw_mode {
            match operations.perform(TerminalLifecycleOperation::DisableRawMode) {
                Ok(()) => self.must_disable_raw_mode = false,
                Err(error) => record_first_error(first_error, error),
            }
        }
    }

    fn enable_raw_mode_with(
        &mut self,
        operations: &mut impl TerminalLifecycleOperations,
    ) -> io::Result<()> {
        if self.must_disable_raw_mode {
            return Ok(());
        }
        self.must_disable_raw_mode = true;
        operations.perform(TerminalLifecycleOperation::EnableRawMode)
    }

    fn enter_alternate_screen_with(
        &mut self,
        operations: &mut impl TerminalLifecycleOperations,
    ) -> io::Result<()> {
        if self.must_leave_alternate_screen {
            return Ok(());
        }
        self.must_leave_alternate_screen = true;
        operations.perform(TerminalLifecycleOperation::EnterAlternateScreen)
    }

    fn enforce_alternate_scroll_disabled_with(
        &mut self,
        operations: &mut impl TerminalLifecycleOperations,
    ) -> io::Result<()> {
        operations.perform(TerminalLifecycleOperation::DisableAlternateScroll)?;
        self.must_disable_alternate_scroll = false;
        Ok(())
    }

    fn enable_alternate_scroll_with(
        &mut self,
        operations: &mut impl TerminalLifecycleOperations,
    ) -> io::Result<()> {
        if self.must_disable_alternate_scroll {
            return Ok(());
        }
        self.must_disable_alternate_scroll = true;
        operations.perform(TerminalLifecycleOperation::EnableAlternateScroll)
    }

    fn disable_alternate_scroll_with(
        &mut self,
        operations: &mut impl TerminalLifecycleOperations,
    ) -> io::Result<()> {
        if !self.must_disable_alternate_scroll {
            return Ok(());
        }
        operations.perform(TerminalLifecycleOperation::DisableAlternateScroll)?;
        self.must_disable_alternate_scroll = false;
        Ok(())
    }

    fn enable_mouse_capture_with(
        &mut self,
        operations: &mut impl TerminalLifecycleOperations,
    ) -> io::Result<()> {
        if self.must_disable_mouse_capture {
            return Ok(());
        }
        self.must_disable_mouse_capture = true;
        operations.perform(TerminalLifecycleOperation::EnableMouseCapture)
    }

    fn disable_mouse_capture_with(
        &mut self,
        operations: &mut impl TerminalLifecycleOperations,
    ) -> io::Result<()> {
        if !self.must_disable_mouse_capture {
            return Ok(());
        }
        operations.perform(TerminalLifecycleOperation::DisableMouseCapture)?;
        self.must_disable_mouse_capture = false;
        Ok(())
    }

    fn enable_bracketed_paste_with(
        &mut self,
        operations: &mut impl TerminalLifecycleOperations,
    ) -> io::Result<()> {
        if self.must_disable_bracketed_paste {
            return Ok(());
        }
        self.must_disable_bracketed_paste = true;
        operations.perform(TerminalLifecycleOperation::EnableBracketedPaste)
    }
}

fn restore_terminal_after_panic() -> io::Result<()> {
    let mut state = TerminalLifecycleState::emergency_restore_state();
    state.restore_all_with(&mut CrosstermTerminalLifecycleOperations::new(
        &mut io::stdout(),
    ))
}

fn run_terminal_panic_hook(restore: impl FnOnce() -> io::Result<()>, previous_hook: impl FnOnce()) {
    let _ = restore();
    previous_hook();
}

/// 安装 panic hook，在既有报告 hook 输出前先 best-effort 恢复终端模式。
pub fn install_terminal_panic_hook() {
    let previous_hook = panic::take_hook();
    panic::set_hook(Box::new(move |panic_info| {
        run_terminal_panic_hook(restore_terminal_after_panic, || previous_hook(panic_info));
    }));
}

fn record_first_error(first_error: &mut Option<io::Error>, error: io::Error) {
    if first_error.is_none() {
        *first_error = Some(error);
    }
}

fn finish_cleanup(first_error: Option<io::Error>) -> io::Result<()> {
    match first_error {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

/// `TerminalLifecycleGuard` 在正常路径与 Drop 之间共享未完成的恢复 obligation。
#[derive(Debug, Default)]
pub(crate) struct TerminalLifecycleGuard {
    state: TerminalLifecycleState,
}

impl TerminalLifecycleGuard {
    pub(crate) fn activate_main<W>(&mut self, writer: &mut W) -> io::Result<()>
    where
        W: Write,
    {
        self.state
            .activate_main_with(&mut CrosstermTerminalLifecycleOperations::new(writer))
    }

    pub(crate) fn activate_minimal<W>(&mut self, writer: &mut W) -> io::Result<()>
    where
        W: Write,
    {
        self.state
            .activate_minimal_with(&mut CrosstermTerminalLifecycleOperations::new(writer))
    }

    pub(crate) fn apply_mouse_mode<W>(
        &mut self,
        writer: &mut W,
        has_mouse_capture: bool,
        has_alternate_scroll: bool,
    ) -> io::Result<()>
    where
        W: Write,
    {
        self.state.apply_mouse_mode_with(
            &mut CrosstermTerminalLifecycleOperations::new(writer),
            has_mouse_capture,
            has_alternate_scroll,
        )
    }

    pub(crate) fn hide_cursor_with(
        &mut self,
        hide_cursor: impl FnOnce() -> io::Result<()>,
    ) -> io::Result<()> {
        self.state.hide_cursor_with(hide_cursor)
    }

    pub(crate) fn show_cursor_with(
        &mut self,
        show_cursor: impl FnOnce() -> io::Result<()>,
    ) -> io::Result<()> {
        self.state.show_cursor_with(show_cursor)
    }

    pub(crate) fn restore_modes<W>(&mut self, writer: &mut W) -> io::Result<()>
    where
        W: Write,
    {
        self.state
            .restore_modes_with(&mut CrosstermTerminalLifecycleOperations::new(writer))
    }

    pub(crate) fn restore_all<W>(&mut self, writer: &mut W) -> io::Result<()>
    where
        W: Write,
    {
        self.state
            .restore_all_with(&mut CrosstermTerminalLifecycleOperations::new(writer))
    }
}

impl Drop for TerminalLifecycleGuard {
    fn drop(&mut self) {
        let _ = self.restore_all(&mut io::stdout());
    }
}

#[cfg(test)]
mod tests {
    use std::{cell::RefCell, collections::VecDeque, io};

    use crossterm::Command;

    use super::{
        DisableAlternateScroll, EnableAlternateScroll, TerminalLifecycleOperation,
        TerminalLifecycleOperations, TerminalLifecycleState, run_terminal_panic_hook,
    };

    #[test]
    fn terminal_panic_hook_restores_before_calling_previous_hook() {
        let calls = RefCell::new(Vec::new());

        run_terminal_panic_hook(
            || {
                calls.borrow_mut().push("restore");
                Ok(())
            },
            || calls.borrow_mut().push("previous_hook"),
        );

        assert_eq!(*calls.borrow(), vec!["restore", "previous_hook"]);
    }

    #[test]
    fn terminal_panic_hook_calls_previous_hook_when_restore_fails() {
        let calls = RefCell::new(Vec::new());

        run_terminal_panic_hook(
            || {
                calls.borrow_mut().push("restore");
                Err(io::Error::other("restore failed"))
            },
            || calls.borrow_mut().push("previous_hook"),
        );

        assert_eq!(*calls.borrow(), vec!["restore", "previous_hook"]);
    }

    #[derive(Debug, Default)]
    struct FakeTerminalLifecycleOperations {
        calls: Vec<TerminalLifecycleOperation>,
        failures: VecDeque<TerminalLifecycleOperation>,
    }

    impl FakeTerminalLifecycleOperations {
        fn failing_once(operation: TerminalLifecycleOperation) -> Self {
            Self {
                failures: VecDeque::from([operation]),
                ..Self::default()
            }
        }

        fn fail_next(&mut self, operation: TerminalLifecycleOperation) {
            self.failures.push_back(operation);
        }

        fn take_calls(&mut self) -> Vec<TerminalLifecycleOperation> {
            std::mem::take(&mut self.calls)
        }
    }

    impl TerminalLifecycleOperations for FakeTerminalLifecycleOperations {
        fn perform(&mut self, operation: TerminalLifecycleOperation) -> io::Result<()> {
            self.calls.push(operation);
            if self.failures.front() == Some(&operation) {
                self.failures.pop_front();
                return Err(io::Error::other(format!("{operation:?} failed")));
            }
            Ok(())
        }
    }

    #[test]
    fn main_activation_failure_restores_attempted_operations_in_reverse_order() {
        let mut state = TerminalLifecycleState::default();
        let mut operations = FakeTerminalLifecycleOperations::failing_once(
            TerminalLifecycleOperation::EnableMouseCapture,
        );

        let error = state
            .activate_main_with(&mut operations)
            .expect_err("mouse activation should fail");

        assert_eq!(error.to_string(), "EnableMouseCapture failed");
        assert_eq!(
            operations.calls,
            vec![
                TerminalLifecycleOperation::EnableRawMode,
                TerminalLifecycleOperation::EnterAlternateScreen,
                TerminalLifecycleOperation::DisableAlternateScroll,
                TerminalLifecycleOperation::EnableMouseCapture,
                TerminalLifecycleOperation::DisableMouseCapture,
                TerminalLifecycleOperation::LeaveAlternateScreen,
                TerminalLifecycleOperation::DisableRawMode,
            ],
        );
    }

    #[test]
    fn main_activation_rolls_back_every_failed_stage() {
        let cases = [
            (
                TerminalLifecycleOperation::EnableRawMode,
                vec![
                    TerminalLifecycleOperation::EnableRawMode,
                    TerminalLifecycleOperation::DisableRawMode,
                ],
            ),
            (
                TerminalLifecycleOperation::EnterAlternateScreen,
                vec![
                    TerminalLifecycleOperation::EnableRawMode,
                    TerminalLifecycleOperation::EnterAlternateScreen,
                    TerminalLifecycleOperation::LeaveAlternateScreen,
                    TerminalLifecycleOperation::DisableRawMode,
                ],
            ),
            (
                TerminalLifecycleOperation::DisableAlternateScroll,
                vec![
                    TerminalLifecycleOperation::EnableRawMode,
                    TerminalLifecycleOperation::EnterAlternateScreen,
                    TerminalLifecycleOperation::DisableAlternateScroll,
                    TerminalLifecycleOperation::LeaveAlternateScreen,
                    TerminalLifecycleOperation::DisableRawMode,
                ],
            ),
            (
                TerminalLifecycleOperation::EnableBracketedPaste,
                vec![
                    TerminalLifecycleOperation::EnableRawMode,
                    TerminalLifecycleOperation::EnterAlternateScreen,
                    TerminalLifecycleOperation::DisableAlternateScroll,
                    TerminalLifecycleOperation::EnableMouseCapture,
                    TerminalLifecycleOperation::EnableBracketedPaste,
                    TerminalLifecycleOperation::DisableBracketedPaste,
                    TerminalLifecycleOperation::DisableMouseCapture,
                    TerminalLifecycleOperation::LeaveAlternateScreen,
                    TerminalLifecycleOperation::DisableRawMode,
                ],
            ),
        ];

        for (failed_operation, expected_calls) in cases {
            let mut state = TerminalLifecycleState::default();
            let mut operations = FakeTerminalLifecycleOperations::failing_once(failed_operation);

            state
                .activate_main_with(&mut operations)
                .expect_err("configured activation stage should fail");

            assert_eq!(
                operations.calls, expected_calls,
                "failed at {failed_operation:?}"
            );
        }
    }

    #[test]
    fn cleanup_continues_after_error_and_retries_only_failed_obligation() {
        let mut state = TerminalLifecycleState::default();
        let mut operations = FakeTerminalLifecycleOperations::default();
        state.activate_main_with(&mut operations).unwrap();
        state.hide_cursor_with(|| Ok(())).unwrap();
        operations.take_calls();
        operations.fail_next(TerminalLifecycleOperation::DisableBracketedPaste);

        let error = state
            .restore_all_with(&mut operations)
            .expect_err("paste cleanup should fail once");

        assert_eq!(error.to_string(), "DisableBracketedPaste failed");
        assert_eq!(
            operations.take_calls(),
            vec![
                TerminalLifecycleOperation::ShowCursor,
                TerminalLifecycleOperation::DisableBracketedPaste,
                TerminalLifecycleOperation::DisableMouseCapture,
                TerminalLifecycleOperation::LeaveAlternateScreen,
                TerminalLifecycleOperation::DisableRawMode,
            ],
        );

        state.restore_all_with(&mut operations).unwrap();
        assert_eq!(
            operations.take_calls(),
            vec![TerminalLifecycleOperation::DisableBracketedPaste],
        );
    }

    #[test]
    fn cursor_hide_failure_registers_show_obligation() {
        let mut state = TerminalLifecycleState::default();
        let mut operations = FakeTerminalLifecycleOperations::default();

        state
            .hide_cursor_with(|| Err(io::Error::other("hide cursor failed")))
            .expect_err("cursor hide should fail");
        state.restore_all_with(&mut operations).unwrap();

        assert_eq!(
            operations.calls,
            vec![TerminalLifecycleOperation::ShowCursor],
        );
    }

    #[test]
    fn second_successful_cleanup_is_empty() {
        let mut state = TerminalLifecycleState::default();
        let mut operations = FakeTerminalLifecycleOperations::default();
        state.activate_main_with(&mut operations).unwrap();
        operations.take_calls();

        state.restore_all_with(&mut operations).unwrap();
        assert!(!operations.take_calls().is_empty());
        state.restore_all_with(&mut operations).unwrap();
        assert!(operations.take_calls().is_empty());
    }

    #[test]
    fn minimal_activation_never_touches_mouse_or_paste() {
        let mut state = TerminalLifecycleState::default();
        let mut operations = FakeTerminalLifecycleOperations::default();

        state.activate_minimal_with(&mut operations).unwrap();

        assert_eq!(
            operations.calls,
            vec![
                TerminalLifecycleOperation::EnableRawMode,
                TerminalLifecycleOperation::EnterAlternateScreen,
            ],
        );
    }

    #[test]
    fn minimal_alternate_screen_failure_restores_attempted_state() {
        let mut state = TerminalLifecycleState::default();
        let mut operations = FakeTerminalLifecycleOperations::failing_once(
            TerminalLifecycleOperation::EnterAlternateScreen,
        );

        state
            .activate_minimal_with(&mut operations)
            .expect_err("alternate screen activation should fail");

        assert_eq!(
            operations.calls,
            vec![
                TerminalLifecycleOperation::EnableRawMode,
                TerminalLifecycleOperation::EnterAlternateScreen,
                TerminalLifecycleOperation::LeaveAlternateScreen,
                TerminalLifecycleOperation::DisableRawMode,
            ],
        );
    }

    #[test]
    fn mouse_transition_disables_old_mode_before_enabling_new_mode() {
        let mut state = TerminalLifecycleState::default();
        let mut operations = FakeTerminalLifecycleOperations::default();
        state.activate_main_with(&mut operations).unwrap();
        operations.take_calls();

        state
            .apply_mouse_mode_with(&mut operations, false, true)
            .unwrap();

        assert_eq!(
            operations.calls,
            vec![
                TerminalLifecycleOperation::DisableMouseCapture,
                TerminalLifecycleOperation::EnableAlternateScroll,
            ],
        );
    }

    #[test]
    fn mouse_transition_failure_keeps_inverse_obligation_for_cleanup() {
        let mut state = TerminalLifecycleState::default();
        let mut operations = FakeTerminalLifecycleOperations::default();
        state.activate_main_with(&mut operations).unwrap();
        operations.take_calls();
        operations.fail_next(TerminalLifecycleOperation::EnableAlternateScroll);

        state
            .apply_mouse_mode_with(&mut operations, false, true)
            .expect_err("alternate scroll activation should fail");
        operations.take_calls();
        state.restore_all_with(&mut operations).unwrap();

        assert_eq!(
            operations.calls,
            vec![
                TerminalLifecycleOperation::DisableBracketedPaste,
                TerminalLifecycleOperation::DisableAlternateScroll,
                TerminalLifecycleOperation::LeaveAlternateScreen,
                TerminalLifecycleOperation::DisableRawMode,
            ],
        );
    }

    #[test]
    fn alternate_scroll_commands_emit_xterm_mode_sequences() {
        let mut enable = String::new();
        EnableAlternateScroll.write_ansi(&mut enable).unwrap();
        assert_eq!(enable, "\x1b[?1007h");

        let mut disable = String::new();
        DisableAlternateScroll.write_ansi(&mut disable).unwrap();
        assert_eq!(disable, "\x1b[?1007l");
    }
}
