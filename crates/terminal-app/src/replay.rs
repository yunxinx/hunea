use std::io::{self, Write};

use color_eyre::eyre::{Result, WrapErr};
use terminal_ui::Model;

use app_config::appconfig::TuiConfig;

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

pub(crate) fn write_terminal_replay_on_exit<W: Write>(
    writer: &mut W,
    model: &Model,
    preserve_ansi: bool,
    tui_config: &TuiConfig,
) -> Result<()> {
    if !tui_config.print_transcript_on_exit {
        return Ok(());
    }

    write_terminal_replay_with_context(writer, model, preserve_ansi)
}
