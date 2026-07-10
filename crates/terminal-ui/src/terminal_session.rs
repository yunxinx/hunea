//! 预检等轻量 TUI 复用的最小终端会话。
//!
//! 与 `runner::terminal::TerminalSession` 的区别：
//! - 不启用 mouse capture / bracketed paste（预检不需要）
//! - 使用 ratatui 原生 `Terminal`，不依赖 `TerminalSurface` 的 diff 渲染
//! - lifecycle 只登记 raw mode / alternate screen / cursor，不登记 mouse/paste
//!
//! 单独抽这一层而不是复用主 TUI session：预检生命周期短、能力面小，
//! 绑主 session 会把 mouse/paste 等状态机带进启动路径，得不偿失。

use std::io;

use ratatui::{Terminal, backend::CrosstermBackend};

use crate::terminal_lifecycle::TerminalLifecycleGuard;

/// `MinimalTerminalSession` 为预检等轻量 TUI 提供终端会话管理。
///
/// 进入时启用 raw mode + alternate screen；Drop 时自动恢复。
/// 不启用 mouse capture / bracketed paste，适合不需要这些特性的短生命周期 TUI。
pub struct MinimalTerminalSession {
    terminal: Terminal<CrosstermBackend<io::Stdout>>,
    lifecycle: TerminalLifecycleGuard,
}

impl MinimalTerminalSession {
    /// 进入 alternate screen 并返回 terminal handle。
    pub fn enter() -> io::Result<Self> {
        let mut lifecycle = TerminalLifecycleGuard::default();
        let mut stdout = io::stdout();
        lifecycle.activate_minimal(&mut stdout)?;
        let backend = CrosstermBackend::new(stdout);
        let mut terminal = Terminal::new(backend)?;
        lifecycle.hide_cursor_with(|| terminal.hide_cursor())?;
        Ok(Self {
            terminal,
            lifecycle,
        })
    }

    /// 返回可变 terminal 引用，供调用方 draw。
    pub fn terminal_mut(&mut self) -> &mut Terminal<CrosstermBackend<io::Stdout>> {
        &mut self.terminal
    }
}

impl Drop for MinimalTerminalSession {
    fn drop(&mut self) {
        let _ = self
            .lifecycle
            .show_cursor_with(|| self.terminal.show_cursor());
        let _ = self.lifecycle.restore_modes(self.terminal.backend_mut());
    }
}
