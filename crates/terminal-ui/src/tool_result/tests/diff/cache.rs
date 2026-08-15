use super::*;

#[test]
fn diff_budget_is_independent_of_the_marker_instant() {
    // 现实触发场景：reduced-motion 下 block_materialize 会把 marker 时间冻结在
    // active_marker_started_at；预算不得受此影响，否则长活动项一开始就拿到耗尽预算。
    let palette = default_palette();
    let item = ToolResultItem::from_runtime_tool_activity(
        RuntimeToolActivity {
            activity_id: "call-1".to_string(),
            title: "Edit src/lib.rs".to_string(),
            kind: RuntimeToolKind::Edit,
            status: RuntimeToolActivityStatus::Completed,
            content: vec![RuntimeToolActivityContent::Diff {
                path: "src/lib.rs".to_string(),
                old_text: Some("let value = compute(alpha, beta);\n".to_string()),
                new_text: "let value = compute(alpha, gamma);\n".to_string(),
                is_truncated: false,
            }],
            locations: Vec::new(),
            raw_input: None,
            raw_output: None,
        },
        ToolActivityRenderMode::Detailed,
    );
    item.prebuild_diff_presentation_with_budget(presentation_budget());
    let has_emphasis = |lines: &[ratatui::text::Line<'static>]| {
        lines
            .iter()
            .flat_map(|line| line.spans.iter())
            .any(|span| span.style.add_modifier.contains(Modifier::BOLD) && span.style.bg.is_some())
    };
    let stale_marker = std::time::Instant::now()
        .checked_sub(std::time::Duration::from_secs(1))
        .expect("process uptime should exceed one second");

    let with_fresh_budget = item.render_lines_at(120, palette, stale_marker);
    assert!(
        has_emphasis(&with_fresh_budget),
        "a stale marker instant must not exhaust the diff budget: {with_fresh_budget:?}"
    );

    let exhausted_item = ToolResultItem::from_runtime_tool_activity(
        diff_call_with_contents(vec![RuntimeToolActivityContent::Diff {
            path: "src/lib.rs".to_string(),
            old_text: Some("let value = compute(alpha, beta);\n".to_string()),
            new_text: "let value = compute(alpha, gamma);\n".to_string(),
            is_truncated: false,
        }]),
        ToolActivityRenderMode::Detailed,
    );
    exhausted_item.prebuild_diff_presentation_with_budget(diff::DiffBudget::exhausted());
    let with_exhausted_budget = exhausted_item.render_lines_at(120, palette, stale_marker);
    assert!(
        !has_emphasis(&with_exhausted_budget),
        "an exhausted budget must fall back to plain styling: {with_exhausted_budget:?}"
    );
}

#[test]
fn tool_result_reuses_the_first_diff_presentation_across_renders() {
    let palette = default_palette();
    let marker_now = std::time::Instant::now();
    let item = ToolResultItem::from_runtime_tool_activity(
        diff_call_with_contents(vec![RuntimeToolActivityContent::Diff {
            path: "src/lib.rs".to_string(),
            old_text: Some("let value = compute(alpha, beta);\n".to_string()),
            new_text: "let value = compute(alpha, gamma);\n".to_string(),
            is_truncated: false,
        }]),
        ToolActivityRenderMode::Detailed,
    );
    let has_emphasis = |lines: &[ratatui::text::Line<'static>]| {
        lines
            .iter()
            .flat_map(|line| &line.spans)
            .any(|span| span.style.add_modifier.contains(Modifier::BOLD) && span.style.bg.is_some())
    };

    item.prebuild_diff_presentation_with_budget(diff::DiffBudget::exhausted());
    let first = item.render_lines_at(120, palette, marker_now);
    assert!(
        !has_emphasis(&first),
        "sanity: an exhausted first build must produce a plain presentation"
    );

    let second = item.render_lines_at(120, palette, marker_now);
    assert_eq!(
        second, first,
        "one content revision must keep the first presentation instead of recomputing it"
    );
}

#[test]
fn active_diff_metrics_and_materialized_block_share_one_presentation() {
    let palette = default_palette();
    let frame_now = std::time::Instant::now();
    let mut call = diff_call_with_contents(vec![RuntimeToolActivityContent::Diff {
        path: "src/lib.rs".to_string(),
        old_text: Some("alpha\nbravo\ncharlie\n".to_string()),
        new_text: "alpha\nbravo changed\ncharlie\ndelta\n".to_string(),
        is_truncated: false,
    }]);
    call.status = RuntimeToolActivityStatus::InProgress;
    let item = ToolResultItem::from_runtime_tool_activity(call, ToolActivityRenderMode::Detailed);
    item.prebuild_diff_presentation_with_budget(diff::DiffBudget::exhausted());

    let (metric_line_count, _) = item.measure_render_metrics(120, palette);
    let transcript_item = TranscriptItem::ToolResult(item.clone());
    let first_block = materialize_transcript_item_render_block(
        &transcript_item,
        120,
        palette,
        crate::frame_time::FrameRenderContext::new(frame_now),
        crate::MotionMode::Full,
    );
    let second_block = materialize_transcript_item_render_block(
        &transcript_item,
        120,
        palette,
        crate::frame_time::FrameRenderContext::new(
            frame_now + TOOL_ACTIVITY_ACTIVE_MARKER_BLINK_INTERVAL,
        ),
        crate::MotionMode::Full,
    );

    assert_eq!(first_block.line_count, metric_line_count);
    assert_eq!(second_block.line_count, metric_line_count);
    assert_eq!(
        &first_block.lines[1..],
        &second_block.lines[1..],
        "only the active marker may change between frames; diff detail must stay stable"
    );
}

#[test]
fn diff_presentation_cache_invalidates_only_when_its_inputs_change() {
    let palette = default_palette();
    let marker_now = std::time::Instant::now();
    let mut item = ToolResultItem::from_runtime_tool_activity(
        diff_call_with_contents(vec![RuntimeToolActivityContent::Diff {
            path: "src/old.rs".to_string(),
            old_text: Some("let value = compute(alpha, beta);\n".to_string()),
            new_text: "let value = compute(alpha, gamma);\n".to_string(),
            is_truncated: false,
        }]),
        ToolActivityRenderMode::Detailed,
    );
    item.prebuild_diff_presentation_with_budget(diff::DiffBudget::exhausted());
    let has_emphasis = |lines: &[ratatui::text::Line<'static>]| {
        lines
            .iter()
            .flat_map(|line| &line.spans)
            .any(|span| span.style.add_modifier.contains(Modifier::BOLD) && span.style.bg.is_some())
    };

    assert!(update_single_runtime_tool_activity(
        &mut item,
        RuntimeToolActivityUpdate {
            activity_id: "call-1".to_string(),
            status: Some(RuntimeToolActivityStatus::InProgress),
            ..RuntimeToolActivityUpdate::default()
        }
    ));
    let status_update = item.render_lines_at(120, palette, marker_now);
    assert!(
        !has_emphasis(&status_update),
        "status changes must preserve the cached presentation"
    );

    assert!(update_single_runtime_tool_activity(
        &mut item,
        RuntimeToolActivityUpdate {
            activity_id: "call-1".to_string(),
            content: Some(vec![RuntimeToolActivityContent::Diff {
                path: "src/renamed.rs".to_string(),
                old_text: Some("let value = compute(alpha, beta);\n".to_string()),
                new_text: "let value = compute(alpha, gamma);\n".to_string(),
                is_truncated: false,
            }]),
            ..RuntimeToolActivityUpdate::default()
        }
    ));
    let path_update = item.render_lines_at(120, palette, marker_now);
    assert!(
        !has_emphasis(&path_update),
        "path-only changes do not alter the cached diff computation"
    );
    assert!(
        line_to_plain_text(&path_update[0]).contains("src/renamed.rs"),
        "the current path must still be rendered with the cached counts"
    );

    assert!(update_single_runtime_tool_activity(
        &mut item,
        RuntimeToolActivityUpdate {
            activity_id: "call-1".to_string(),
            content: Some(vec![RuntimeToolActivityContent::Diff {
                path: "src/renamed.rs".to_string(),
                old_text: Some("let value = compute(alpha, beta);\n".to_string()),
                new_text: "let value = compute(alpha, delta);\n".to_string(),
                is_truncated: false,
            }]),
            ..RuntimeToolActivityUpdate::default()
        }
    ));
    item.prebuild_diff_presentation_with_budget(presentation_budget());
    let content_update = item.render_lines_at(120, palette, marker_now);
    let content_text = content_update
        .iter()
        .map(line_to_plain_text)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        content_text.contains("delta") && !content_text.contains("gamma"),
        "changed diff input must rebuild from the current content: {content_text:?}"
    );
    assert!(
        has_emphasis(&content_update),
        "the rebuilt presentation should use a fresh computation budget"
    );

    item.set_render_mode(ToolActivityRenderMode::Compact);
    assert!(update_single_runtime_tool_activity(
        &mut item,
        RuntimeToolActivityUpdate {
            activity_id: "call-1".to_string(),
            content: Some(vec![RuntimeToolActivityContent::Diff {
                path: "src/renamed.rs".to_string(),
                old_text: Some("let value = compute(alpha, beta);\n".to_string()),
                new_text: "let value = compute(alpha, delta);\n".to_string(),
                is_truncated: true,
            }]),
            ..RuntimeToolActivityUpdate::default()
        }
    ));
    let truncated_update = item.render_lines_at(120, palette, marker_now);
    assert!(
        truncated_update
            .iter()
            .map(line_to_plain_text)
            .any(|line| line.contains("preview truncated; showing partial diff")),
        "is_truncated changes must rebuild the cached detailed/compact views"
    );
}

#[test]
fn diff_presentation_cache_invalidates_when_diff_slot_structure_changes() {
    let marker_now = std::time::Instant::now();
    let diff_content = || RuntimeToolActivityContent::Diff {
        path: "src/lib.rs".to_string(),
        old_text: Some("let value = compute(alpha, beta);\n".to_string()),
        new_text: "let value = compute(alpha, gamma);\n".to_string(),
        is_truncated: false,
    };
    let cases = [
        (
            "inserting a non-Diff slot before Diff",
            vec![diff_content()],
            vec![
                RuntimeToolActivityContent::Text("prefix".to_string()),
                diff_content(),
            ],
        ),
        (
            "removing a non-Diff slot before Diff",
            vec![
                RuntimeToolActivityContent::Text("prefix".to_string()),
                diff_content(),
            ],
            vec![diff_content()],
        ),
        (
            "moving Diff to another slot",
            vec![
                diff_content(),
                RuntimeToolActivityContent::Text("suffix".to_string()),
            ],
            vec![
                RuntimeToolActivityContent::Text("suffix".to_string()),
                diff_content(),
            ],
        ),
    ];

    for (case, initial_content, updated_content) in cases {
        let mut item = ToolResultItem::from_runtime_tool_activity(
            diff_call_with_contents(initial_content),
            ToolActivityRenderMode::Detailed,
        );
        item.prebuild_diff_presentation_with_budget(diff::DiffBudget::exhausted());

        assert!(update_single_runtime_tool_activity(
            &mut item,
            RuntimeToolActivityUpdate {
                activity_id: "call-1".to_string(),
                content: Some(updated_content),
                ..RuntimeToolActivityUpdate::default()
            }
        ));
        item.prebuild_diff_presentation_with_budget(presentation_budget());
        let lines = item.render_lines_at(120, default_palette(), marker_now);
        assert!(
            lines.iter().flat_map(|line| &line.spans).any(|span| {
                span.style.add_modifier.contains(Modifier::BOLD) && span.style.bg.is_some()
            }),
            "{case} must invalidate the exhausted presentation and rebuild with a fresh budget"
        );
    }
}

#[test]
fn compact_diff_truncation_keeps_notice_inside_the_head_edge() {
    // 锁定「通知参与截断计算」的历史组合语义：通知占据首行位、
    // 头部内容行因此少一行，省略计数把通知计入总行数。
    let new_text = (1..=20)
        .map(|index| format!("row {index}\n"))
        .collect::<String>();
    let call = diff_call_with_contents(vec![RuntimeToolActivityContent::Diff {
        path: "big.rs".to_string(),
        old_text: None,
        new_text,
        is_truncated: true,
    }]);
    let presentations = diff::DiffPresentations::build(&call, presentation_budget());
    let blocks = activity::runtime_tool_activity_detail_blocks(
        &call,
        &presentations,
        ToolActivityRenderMode::Compact,
        false,
        &std::collections::BTreeMap::new(),
    );

    let diff_lines = blocks
        .iter()
        .find_map(|block| match block {
            activity::RuntimeToolActivityDetailBlock::Diff(lines) => Some(lines),
            _ => None,
        })
        .expect("diff content should produce a diff detail block");

    let edge = TOOL_ACTIVITY_COMPACT_EDGE_LINES;
    assert_eq!(
        diff_lines.len(),
        edge * 2 + 1,
        "compact truncation keeps head edge, omitted marker, and tail edge: {diff_lines:?}"
    );
    assert!(
        diff_lines[0]
            .joined_text()
            .contains("preview truncated; showing partial diff"),
        "the preview truncation notice must occupy the first head slot: {diff_lines:?}"
    );
    // 通知占一行：头部只保留 edge-1 个内容行，省略计数为 (20+1) - 2*edge。
    let expected_omitted = 20 + 1 - edge * 2;
    assert!(
        diff_lines[edge]
            .joined_text()
            .contains(&format!("+{expected_omitted} lines")),
        "omitted count must treat the notice as part of the total: {diff_lines:?}"
    );
    assert_eq!(diff_lines[1].joined_text(), "row 1");
    assert_eq!(
        diff_lines[edge - 1].joined_text(),
        format!("row {}", edge - 1)
    );
    assert_eq!(
        diff_lines[diff_lines.len() - 1].joined_text(),
        "row 20",
        "tail edge must keep the final content lines: {diff_lines:?}"
    );
}

#[test]
fn tool_result_render_cache_key_includes_diff_display() {
    let activity = RuntimeToolActivity {
        activity_id: "call-1".to_string(),
        title: "Edit src/lib.rs".to_string(),
        kind: RuntimeToolKind::Edit,
        status: RuntimeToolActivityStatus::Completed,
        content: vec![RuntimeToolActivityContent::Diff {
            path: "src/lib.rs".to_string(),
            old_text: Some("one\nold\ntail\n".to_string()),
            new_text: "one\nnew\ntail\n".to_string(),
            is_truncated: false,
        }],
        locations: Vec::new(),
        raw_input: None,
        raw_output: None,
    };
    let full_line = ToolResultItem::from_runtime_tool_activity(
        activity.clone(),
        ToolActivityRenderMode::Compact,
    )
    .with_diff_display(crate::DiffDisplay::FullLine);
    let summary =
        ToolResultItem::from_runtime_tool_activity(activity, ToolActivityRenderMode::Compact)
            .with_diff_display(crate::DiffDisplay::Summary);

    assert_ne!(
        full_line.render_cache_key(),
        summary.render_cache_key(),
        "summary changes compact line count, so DiffDisplay must participate in the cache key"
    );
}
