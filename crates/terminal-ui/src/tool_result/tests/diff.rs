use super::super::diff;
use super::*;
use crate::{
    display_width::display_width,
    theme::{diff_emphasis_tint, diff_row_tint},
    transcript::{TranscriptItem, materialize_transcript_item_render_block},
};

#[path = "diff/algorithm.rs"]
mod algorithm;
#[path = "diff/cache.rs"]
mod cache;
#[path = "diff/presentation.rs"]
mod presentation;
#[path = "diff/rendering.rs"]
mod rendering;

/// 统计 diff 行中 Insert/Delete 的行数，供 header/detail 一致性断言使用。
fn count_insert_delete_lines(lines: &[diff::RuntimeDiffDetailLine]) -> (usize, usize) {
    lines
        .iter()
        .fold((0, 0), |(inserted, deleted), line| match line.kind {
            diff::RuntimeDiffDetailLineKind::Insert => (inserted + 1, deleted),
            diff::RuntimeDiffDetailLineKind::Delete => (inserted, deleted + 1),
            _ => (inserted, deleted),
        })
}

/// presentation 正确性测试使用无 deadline 预算，避免调度延迟改变断言结果。
fn presentation_budget() -> diff::DiffBudget {
    diff::DiffBudget::unlimited_for_test()
}

fn diff_call_with_contents(content: Vec<RuntimeToolActivityContent>) -> RuntimeToolActivity {
    RuntimeToolActivity {
        activity_id: "call-1".to_string(),
        title: "Edit src/lib.rs".to_string(),
        kind: RuntimeToolKind::Edit,
        status: RuntimeToolActivityStatus::Completed,
        content,
        locations: Vec::new(),
        raw_input: None,
        raw_output: None,
    }
}

fn diff_header_text(call: &RuntimeToolActivity, presentations: &diff::DiffPresentations) -> String {
    activity::runtime_tool_activity_diff_header_chunks(call, presentations, default_palette())
        .expect("diff content must produce a diff header")
        .iter()
        .map(|chunk| chunk.text.as_str())
        .collect()
}

fn detail_diff_lines(
    call: &RuntimeToolActivity,
    presentations: &diff::DiffPresentations,
) -> Vec<diff::RuntimeDiffDetailLine> {
    activity::runtime_tool_activity_detail_blocks(
        call,
        presentations,
        ToolActivityRenderMode::Detailed,
        false,
        &std::collections::BTreeMap::new(),
    )
    .into_iter()
    .find_map(|block| match block {
        activity::RuntimeToolActivityDetailBlock::Diff(lines) => Some(lines),
        _ => None,
    })
    .expect("diff content must produce a diff detail block")
    .to_vec()
}
