use ratatui::{buffer::Buffer, layout::Rect, style::Style};

use crate::{
    runner::terminal_surface::draw_terminal_commands,
    terminal_grid::{TerminalDrawCommand, diff_terminal_buffers},
};

/// `TerminalGridScenario` 标识 terminal buffer diff 的稳定输入形态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalGridScenario {
    SingleCell,
    FullScreen,
    ScrollOneLine,
}

/// `TerminalCommandSummary` 收敛一次 terminal buffer diff 的命令特征。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerminalCommandSummary {
    pub command_count: usize,
    pub put_count: usize,
    pub clear_to_end_count: usize,
    pub wide_prefill_cells: usize,
}

/// `TerminalGridBench` 保存一对可重复使用的 terminal buffer fixture。
#[derive(Debug)]
pub struct TerminalGridBench {
    previous: Buffer,
    current: Buffer,
}

/// `TerminalFlushSummary` 收敛一次 buffer diff 与 ANSI emission 的输出特征。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerminalFlushSummary {
    pub commands: TerminalCommandSummary,
    pub output_bytes: usize,
}

/// `TerminalFlushBench` 复用输出缓冲并测量生产 ANSI command emitter。
#[derive(Debug)]
pub struct TerminalFlushBench {
    grid: TerminalGridBench,
    output: Vec<u8>,
}

impl TerminalGridBench {
    /// 构造指定 diff 场景。
    pub fn new(scenario: TerminalGridScenario, width: u16, height: u16) -> Self {
        assert!(width > 0, "terminal grid benchmark width must be non-zero");
        assert!(
            height > 0,
            "terminal grid benchmark height must be non-zero"
        );
        let area = Rect::new(0, 0, width, height);
        let mut previous = Buffer::empty(area);
        let mut current = Buffer::empty(area);

        match scenario {
            TerminalGridScenario::SingleCell => {
                current.set_string(area.width / 2, area.height / 2, "x", Style::default());
            }
            TerminalGridScenario::FullScreen => {
                for y in 0..area.height {
                    for x in 0..area.width {
                        current[(x, y)].set_symbol("x");
                    }
                }
            }
            TerminalGridScenario::ScrollOneLine => {
                for y in 0..area.height {
                    fill_row(&mut previous, y, row_symbol(usize::from(y)));
                    fill_row(&mut current, y, row_symbol(usize::from(y) + 1));
                }
            }
        }

        Self { previous, current }
    }

    /// 执行生产 buffer diff 并返回稳定命令摘要。
    pub fn diff(&self) -> TerminalCommandSummary {
        let commands = self.commands();
        summarize_commands(&commands)
    }

    fn commands(&self) -> Vec<TerminalDrawCommand<'_>> {
        diff_terminal_buffers(&self.previous, &self.current)
    }
}

impl TerminalFlushBench {
    /// 构造指定 diff 场景的 ANSI flush benchmark。
    pub fn new(scenario: TerminalGridScenario, width: u16, height: u16) -> Self {
        Self {
            grid: TerminalGridBench::new(scenario, width, height),
            output: Vec::new(),
        }
    }

    /// 运行生产 diff 与 ANSI emission，并返回命令与输出字节摘要。
    pub fn diff_and_flush(&mut self) -> TerminalFlushSummary {
        self.output.clear();
        let commands = self.grid.commands();
        let command_summary = summarize_commands(&commands);
        draw_terminal_commands(&mut self.output, commands.into_iter())
            .expect("benchmark ANSI emission should succeed");

        TerminalFlushSummary {
            commands: command_summary,
            output_bytes: self.output.len(),
        }
    }
}

fn fill_row(buffer: &mut Buffer, y: u16, symbol: String) {
    for x in 0..buffer.area.width {
        buffer[(x, y)].set_symbol(&symbol);
    }
}

fn row_symbol(index: usize) -> String {
    char::from(b'A' + u8::try_from(index % 26).expect("row symbol should fit in ASCII")).to_string()
}

fn summarize_commands(commands: &[TerminalDrawCommand<'_>]) -> TerminalCommandSummary {
    let mut put_count = 0;
    let mut clear_to_end_count = 0;
    let mut wide_prefill_cells = 0;

    for command in commands {
        match command {
            TerminalDrawCommand::Put { prefill_width, .. } => {
                put_count += 1;
                wide_prefill_cells += prefill_width;
            }
            TerminalDrawCommand::ClearToEnd { .. } => clear_to_end_count += 1,
        }
    }

    TerminalCommandSummary {
        command_count: commands.len(),
        put_count,
        clear_to_end_count,
        wide_prefill_cells,
    }
}
