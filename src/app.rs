use std::io::{self, Write};

use color_eyre::eyre::{Result, WrapErr};

use crate::startup::{self, HeroOptions};

/// Run 负责组装当前应用的最小启动流程。
/// 先保留为“输出启动 hero”，后续接入完整 Ratatui 主循环时可以继续沿用这个入口。
pub fn run() -> Result<()> {
    let stdout = io::stdout();
    let mut handle = stdout.lock();
    run_with_writer(&mut handle)
}

/// RunWithWriter 允许调用方注入输出目标，方便测试入口层的错误传播与上下文包装。
pub fn run_with_writer<W: Write>(writer: &mut W) -> Result<()> {
    startup::write_hero_to(writer, &HeroOptions::default()).wrap_err("failed to write startup hero")
}
