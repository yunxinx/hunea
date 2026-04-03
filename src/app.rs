use std::io::{self, IsTerminal, Write};

use color_eyre::eyre::{Result, WrapErr};

use crate::{
    appconfig::{self, UserInputStyle},
    frontend::tui::{self, HeroOptions, Model, StyleMode},
};

/// `run` 负责组装并启动交互式 TUI 应用。
pub fn run() -> Result<()> {
    let stdout = io::stdout();
    let preserve_ansi = stdout.is_terminal();
    let mut handle = stdout.lock();
    let config = appconfig::load().wrap_err("failed to load app config")?;
    run_with_writer(&mut handle, preserve_ansi, config.tui.user_input_style)
}

/// `run_with_writer` 允许调用方注入退出 AltScreen 后的 terminal replay 输出目标。
pub fn run_with_writer<W: Write>(
    writer: &mut W,
    preserve_ansi: bool,
    user_input_style: UserInputStyle,
) -> Result<()> {
    let model = tui::run_with_style_mode(
        HeroOptions::default(),
        style_mode_from_config(user_input_style),
    )
    .wrap_err("failed to run tui application")?;
    write_terminal_replay_with_context(writer, &model, preserve_ansi)
}

/// `write_terminal_replay` 将 terminal replay 内容输出到目标 writer。
pub fn write_terminal_replay<W: Write>(writer: &mut W, model: &Model) -> io::Result<()> {
    write_terminal_replay_with_mode(writer, model, false)
}

/// `write_terminal_replay_preserving_ansi` 在目标仍支持终端样式时保留 ANSI。
pub fn write_terminal_replay_preserving_ansi<W: Write>(
    writer: &mut W,
    model: &Model,
) -> io::Result<()> {
    write_terminal_replay_with_mode(writer, model, true)
}

fn write_terminal_replay_with_mode<W: Write>(
    writer: &mut W,
    model: &Model,
    preserve_ansi: bool,
) -> io::Result<()> {
    let items = model.terminal_replay_items(preserve_ansi);

    for (index, item) in items.iter().enumerate() {
        writeln!(writer, "{item}")?;
        if index + 1 < items.len() {
            writeln!(writer)?;
        }
    }

    Ok(())
}

/// `write_terminal_replay_with_context` 为 terminal replay 输出补充入口层错误上下文。
pub fn write_terminal_replay_with_context<W: Write>(
    writer: &mut W,
    model: &Model,
    preserve_ansi: bool,
) -> Result<()> {
    write_terminal_replay_with_mode(writer, model, preserve_ansi)
        .wrap_err("failed to write terminal replay")
}

fn style_mode_from_config(style: UserInputStyle) -> StyleMode {
    match style {
        UserInputStyle::Cx => StyleMode::Cx,
        UserInputStyle::Cc => StyleMode::Cc,
        UserInputStyle::Ms => StyleMode::Ms,
    }
}
