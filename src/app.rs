use std::io::{self, IsTerminal, Write};

use color_eyre::eyre::{Result, WrapErr};

use crate::frontend::tui::{self, HeroOptions, Model};

/// `run` 负责组装并启动交互式 TUI 应用。
pub fn run() -> Result<()> {
    let stdout = io::stdout();
    let preserve_ansi = stdout.is_terminal();
    let mut handle = stdout.lock();
    run_with_writer(&mut handle, preserve_ansi)
}

/// `run_with_writer` 允许调用方注入退出后 transcript 的输出目标。
pub fn run_with_writer<W: Write>(writer: &mut W, preserve_ansi: bool) -> Result<()> {
    let model = tui::run(HeroOptions::default()).wrap_err("failed to run tui application")?;
    write_exit_transcript_with_context(writer, &model, preserve_ansi)
}

/// `write_exit_transcript` 将 transcript 内容打印到终端。
pub fn write_exit_transcript<W: Write>(writer: &mut W, model: &Model) -> io::Result<()> {
    write_exit_transcript_with_mode(writer, model, false)
}

/// `write_exit_transcript_preserving_ansi` 在退出 writer 支持终端样式时保留 ANSI。
pub fn write_exit_transcript_preserving_ansi<W: Write>(
    writer: &mut W,
    model: &Model,
) -> io::Result<()> {
    write_exit_transcript_with_mode(writer, model, true)
}

fn write_exit_transcript_with_mode<W: Write>(
    writer: &mut W,
    model: &Model,
    preserve_ansi: bool,
) -> io::Result<()> {
    let items = model.transcript_exit_items(preserve_ansi);

    for (index, item) in items.iter().enumerate() {
        writeln!(writer, "{item}")?;
        if index + 1 < items.len() {
            writeln!(writer)?;
        }
    }

    Ok(())
}

/// `write_exit_transcript_with_context` 为退出打印补充入口层错误上下文。
pub fn write_exit_transcript_with_context<W: Write>(
    writer: &mut W,
    model: &Model,
    preserve_ansi: bool,
) -> Result<()> {
    write_exit_transcript_with_mode(writer, model, preserve_ansi)
        .wrap_err("failed to write exit transcript")
}
