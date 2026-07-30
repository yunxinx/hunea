//! 工具活动 diff 渲染的性能基准支撑。
//!
//! 覆盖 content revision 的首次构建与 active frame cache hit：diff 的开销由「变更分布」决定，
//! 而不是纯行数——同样 2000 行的文件，整体逆序比整体重写贵一个数量级。
//! 因此按分布取样，而不是只按体量取样。

use std::time::Instant;

use ratatui::style::Modifier;

use crate::{
    theme::{TerminalPalette, default_palette},
    tool_result::{ToolActivityRenderMode, ToolResultItem},
};
use runtime_domain::session::{
    RuntimeToolActivity, RuntimeToolActivityContent, RuntimeToolActivityStatus, RuntimeToolKind,
};

/// 基准 fixture 的文件行数，取值贴近真实编辑预览而非上游 6000 行硬上限，
/// 让最坏分布的单次迭代仍停留在几十毫秒量级。
const BENCH_DIFF_LINE_COUNT: usize = 2000;
/// `BlockReplacement` 场景替换的连续行数：两侧合计 64 行，恰好落在行内细化上限。
const BENCH_DIFF_BLOCK_LINES: usize = 32;

/// `ToolResultDiffSummary` 收敛一次 diff 渲染的稳定输出特征。
/// `emphasized_span_count` 为 0 说明该次渲染整体回退到了整行样式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ToolResultDiffSummary {
    pub line_count: usize,
    pub span_count: usize,
    pub emphasized_span_count: usize,
}

/// `ToolResultDiffScenario` 描述一种变更分布。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolResultDiffScenario {
    /// 全文重写：单个巨型 Replace op，越过行内细化行数上限，只剩行级 diff 的代价。
    WholeFileRewrite,
    /// 变更均匀散布：产出大量小 Replace op，行级 diff 与行内细化都要算。
    ScatteredEdits,
    /// 块状替换：单块两侧各 32 行，恰好落在行内细化预算内。
    BlockReplacement,
    /// 整体逆序：行级 Myers 的最坏形态。
    ReorderedFile,
}

/// `ToolResultDiffBench` 驱动单条 Diff content 的完整渲染：
/// diff 计算、行内细化与折行都在测量范围内，与 transcript 物化路径一致。
#[derive(Debug)]
pub struct ToolResultDiffBench {
    item: ToolResultItem,
    palette: TerminalPalette,
    width: u16,
    marker_now: Instant,
}

impl ToolResultDiffBench {
    /// 构造指定变更分布的 active diff 渲染基准。
    pub fn new(scenario: ToolResultDiffScenario, width: u16) -> Self {
        assert!(width > 0, "diff benchmark width must be non-zero");
        let (old_text, new_text) = scenario_texts(scenario);
        let item = ToolResultItem::from_runtime_tool_activity(
            RuntimeToolActivity {
                activity_id: "benchmark-diff".to_string(),
                title: "Edit src/lib.rs".to_string(),
                kind: RuntimeToolKind::Edit,
                status: RuntimeToolActivityStatus::InProgress,
                content: vec![RuntimeToolActivityContent::Diff {
                    path: "src/lib.rs".to_string(),
                    old_text: Some(old_text),
                    new_text,
                    is_truncated: false,
                }],
                locations: Vec::new(),
                raw_input: None,
                raw_output: None,
            },
            ToolActivityRenderMode::Detailed,
        );

        Self {
            item,
            palette: default_palette(),
            width,
            marker_now: Instant::now(),
        }
    }

    /// 执行一次 active diff 渲染；是否命中 presentation cache 由 fixture 生命周期决定。
    pub fn render(&self) -> ToolResultDiffSummary {
        let lines = self
            .item
            .render_lines_at(self.width, self.palette, self.marker_now);

        let mut span_count = 0;
        let mut emphasized_span_count = 0;
        for span in lines.iter().flat_map(|line| line.spans.iter()) {
            span_count += 1;
            // 行内强调段的判据与渲染实现一致：更饱和背景 + BOLD。
            if span.style.add_modifier.contains(Modifier::BOLD) && span.style.bg.is_some() {
                emphasized_span_count += 1;
            }
        }

        ToolResultDiffSummary {
            line_count: lines.len(),
            span_count,
            emphasized_span_count,
        }
    }
}

fn scenario_texts(scenario: ToolResultDiffScenario) -> (String, String) {
    let old_text = (0..BENCH_DIFF_LINE_COUNT)
        .map(|index| diff_fixture_line(index, "beta"))
        .collect::<String>();

    let new_text = match scenario {
        ToolResultDiffScenario::WholeFileRewrite => (0..BENCH_DIFF_LINE_COUNT)
            .map(|index| diff_fixture_line(index, "gamma"))
            .collect(),
        ToolResultDiffScenario::ScatteredEdits => (0..BENCH_DIFF_LINE_COUNT)
            .map(|index| diff_fixture_line(index, if index % 4 == 0 { "gamma" } else { "beta" }))
            .collect(),
        ToolResultDiffScenario::BlockReplacement => {
            let block_start = BENCH_DIFF_LINE_COUNT.saturating_sub(BENCH_DIFF_BLOCK_LINES) / 2;
            let block_end = block_start + BENCH_DIFF_BLOCK_LINES;
            (0..BENCH_DIFF_LINE_COUNT)
                .map(|index| {
                    let token = if (block_start..block_end).contains(&index) {
                        "gamma"
                    } else {
                        "beta"
                    };
                    diff_fixture_line(index, token)
                })
                .collect()
        }
        ToolResultDiffScenario::ReorderedFile => (0..BENCH_DIFF_LINE_COUNT)
            .rev()
            .map(|index| diff_fixture_line(index, "beta"))
            .collect(),
    };

    (old_text, new_text)
}

fn diff_fixture_line(index: usize, token: &str) -> String {
    format!("    let value_{index} = compute(alpha, {token}_{index});\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn benchmark_scenarios_keep_the_configured_file_size() {
        // 普通单测只锁定 fixture；受 wall-clock deadline 影响的渲染耗时由 Criterion 观测。
        for scenario in [
            ToolResultDiffScenario::WholeFileRewrite,
            ToolResultDiffScenario::ScatteredEdits,
            ToolResultDiffScenario::BlockReplacement,
            ToolResultDiffScenario::ReorderedFile,
        ] {
            let (old_text, new_text) = scenario_texts(scenario);

            assert_eq!(old_text.lines().count(), BENCH_DIFF_LINE_COUNT);
            assert_eq!(new_text.lines().count(), BENCH_DIFF_LINE_COUNT);
        }
    }

    #[test]
    fn benchmark_scenario_change_distributions_are_stable() {
        let changed_line_count = |scenario| {
            let (old_text, new_text) = scenario_texts(scenario);
            old_text
                .lines()
                .zip(new_text.lines())
                .filter(|(old_line, new_line)| old_line != new_line)
                .count()
        };

        assert_eq!(
            changed_line_count(ToolResultDiffScenario::WholeFileRewrite),
            BENCH_DIFF_LINE_COUNT
        );
        assert_eq!(
            changed_line_count(ToolResultDiffScenario::ScatteredEdits),
            BENCH_DIFF_LINE_COUNT.div_ceil(4)
        );
        assert_eq!(
            changed_line_count(ToolResultDiffScenario::BlockReplacement),
            BENCH_DIFF_BLOCK_LINES
        );
        assert_eq!(
            changed_line_count(ToolResultDiffScenario::ReorderedFile),
            BENCH_DIFF_LINE_COUNT
        );
    }

    #[test]
    fn fresh_and_warmed_renders_have_the_same_stable_summary() {
        let bench = ToolResultDiffBench::new(ToolResultDiffScenario::BlockReplacement, 120);

        let cold = bench.render();
        let warmed = bench.render();

        assert_eq!(cold, warmed);
    }
}
