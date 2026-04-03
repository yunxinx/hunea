use std::io::{self, Write};

use color_eyre::eyre::{Result, WrapErr};

use crate::frontend::tui::{self, HeroOptions, Model};

/// `run` 负责组装并启动交互式 TUI 应用。
pub fn run() -> Result<()> {
    let stdout = io::stdout();
    let mut handle = stdout.lock();
    run_with_writer(&mut handle)
}

/// `run_with_writer` 允许调用方注入退出后 transcript 的输出目标。
pub fn run_with_writer<W: Write>(writer: &mut W) -> Result<()> {
    let model = tui::run(HeroOptions::default()).wrap_err("failed to run tui application")?;
    write_exit_transcript_with_context(writer, &model)
}

/// `write_exit_transcript` 将 transcript 内容打印到终端。
pub fn write_exit_transcript<W: Write>(writer: &mut W, model: &Model) -> io::Result<()> {
    let items = model.transcript_plain_items();

    for (index, item) in items.iter().enumerate() {
        writeln!(writer, "{item}")?;
        if index + 1 < items.len() {
            writeln!(writer)?;
        }
    }

    Ok(())
}

/// `write_exit_transcript_with_context` 为退出打印补充入口层错误上下文。
pub fn write_exit_transcript_with_context<W: Write>(writer: &mut W, model: &Model) -> Result<()> {
    write_exit_transcript(writer, model).wrap_err("failed to write exit transcript")
}
