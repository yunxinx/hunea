use super::*;

#[test]
fn runtime_tool_activity_diff_context_lines_keep_default_style() {
    let palette = default_palette();
    let item = ToolResultItem::from_runtime_tool_activity(
        RuntimeToolActivity {
            activity_id: "call-1".to_string(),
            title: "WriteFile: src/lib.rs".to_string(),
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
        },
        ToolActivityRenderMode::Detailed,
    );
    item.prebuild_diff_presentation_with_budget(presentation_budget());
    let lines = item.render_lines(80, palette);
    let context_line = lines
        .iter()
        .find(|line| line_to_plain_text(line).contains(" one"))
        .expect("context line should be rendered");
    let insert_line = lines
        .iter()
        .find(|line| line_to_plain_text(line).contains("+  new"))
        .expect("insert line should be rendered");
    let delete_line = lines
        .iter()
        .find(|line| line_to_plain_text(line).contains("-  old"))
        .expect("delete line should be rendered");

    assert_eq!(context_line.style.bg, None);
    assert!(
        context_line
            .spans
            .iter()
            .all(|span| span.style.bg.is_none() && span.style.fg.is_none()),
        "context diff spans should keep default styling like codex-rs: {context_line:?}"
    );
    assert!(insert_line.style.bg.is_some());
    assert!(delete_line.style.bg.is_some());
}

#[test]
fn runtime_tool_activity_added_diff_uses_codex_like_header_and_line_numbers() {
    let palette = default_palette();
    let absolute_path = std::env::current_dir()
        .expect("cwd should be available")
        .join("temp.md")
        .display()
        .to_string();
    let new_text = (1..=25)
        .map(|line| format!("line {line}"))
        .collect::<Vec<_>>()
        .join("\n");
    let item = ToolResultItem::from_runtime_tool_activity(
        RuntimeToolActivity {
            activity_id: "call-1".to_string(),
            title: "WriteFile: temp.md".to_string(),
            kind: RuntimeToolKind::Edit,
            status: RuntimeToolActivityStatus::Completed,
            content: vec![RuntimeToolActivityContent::Diff {
                path: absolute_path,
                old_text: None,
                new_text,
                is_truncated: false,
            }],
            locations: Vec::new(),
            raw_input: None,
            raw_output: None,
        },
        ToolActivityRenderMode::Compact,
    );
    item.prebuild_diff_presentation_with_budget(presentation_budget());
    let lines = item.render_lines(120, palette);
    let rendered_plain = lines.iter().map(line_to_plain_text).collect::<Vec<_>>();

    assert_eq!(rendered_plain[0], "● Added temp.md (+25 -0)");
    assert!(
        rendered_plain
            .iter()
            .all(|line| !line.contains("WriteFile") && !line.contains("Diff:")),
        "diff rendering should not expose redundant tool or diff labels: {rendered_plain:?}"
    );
    assert!(
        rendered_plain
            .iter()
            .any(|line| line == "      1 +  line 1"),
        "diff lines should right-align line numbers in a seven-column gutter: {rendered_plain:?}"
    );
    assert!(
        rendered_plain
            .iter()
            .any(|line| line == "     25 +  line 25"),
        "compact diff should keep the tail lines: {rendered_plain:?}"
    );
    assert!(
        rendered_plain
            .iter()
            .any(|line| line == "      ⋮ +15 lines (ctrl + t to view transcript)"),
        "compact diff omitted hint should align with the number gutter edge: {rendered_plain:?}"
    );
    assert!(
        !rendered_plain
            .iter()
            .any(|line| line.contains("13 +line 13")),
        "compact mode should omit middle diff rows: {rendered_plain:?}"
    );
}

#[test]
fn runtime_tool_activity_truncated_diff_shows_partial_preview_notice() {
    let item = ToolResultItem::from_runtime_tool_activity(
        RuntimeToolActivity {
            activity_id: "call-1".to_string(),
            title: "Write temp.md".to_string(),
            kind: RuntimeToolKind::Write,
            status: RuntimeToolActivityStatus::Completed,
            content: vec![RuntimeToolActivityContent::Diff {
                path: "temp.md".to_string(),
                old_text: Some("old\n".to_string()),
                new_text: "new\n".to_string(),
                is_truncated: true,
            }],
            locations: Vec::new(),
            raw_input: None,
            raw_output: None,
        },
        ToolActivityRenderMode::Detailed,
    );
    item.prebuild_diff_presentation_with_budget(presentation_budget());

    let rendered_plain = item
        .render_lines(120, default_palette())
        .iter()
        .map(line_to_plain_text)
        .collect::<Vec<_>>();

    assert!(
        rendered_plain
            .iter()
            .any(|line| line.contains("preview truncated")),
        "truncated diffs should clearly say the preview is partial: {rendered_plain:?}"
    );
}

#[test]
fn runtime_tool_activity_write_kind_uses_diff_rendering() {
    let item = ToolResultItem::from_runtime_tool_activity(
        RuntimeToolActivity {
            activity_id: "call-1".to_string(),
            title: "Write temp.md".to_string(),
            kind: RuntimeToolKind::Write,
            status: RuntimeToolActivityStatus::Completed,
            content: vec![RuntimeToolActivityContent::Diff {
                path: "temp.md".to_string(),
                old_text: Some("old\n".to_string()),
                new_text: "new\n".to_string(),
                is_truncated: false,
            }],
            locations: vec![RuntimeToolActivityLocation {
                path: "temp.md".to_string(),
                line: None,
            }],
            raw_input: Some(
                serde_json::json!({
                    "path": "temp.md",
                    "content": "new\n"
                })
                .into(),
            ),
            raw_output: Some(
                runtime_domain::session::RuntimeToolActivityRawValue::tool_result_with_display_content(
                    "The file temp.md has been updated successfully.",
                    Some("The file temp.md has been updated successfully."),
                    Some(serde_json::json!({
                        "path": "temp.md",
                        "old_text": "old\n",
                        "new_text": "new\n"
                    })),
                ),
            ),
        },
        ToolActivityRenderMode::Compact,
    );
    item.prebuild_diff_presentation_with_budget(presentation_budget());

    let rendered_plain = item
        .render_lines(120, default_palette())
        .iter()
        .map(line_to_plain_text)
        .collect::<Vec<_>>();

    assert_eq!(rendered_plain[0], "● Edited temp.md (+1 -1)");
    assert!(
        rendered_plain
            .iter()
            .all(|line| !line.contains("The file temp.md has been updated successfully")),
        "write diff rendering should prefer the diff view over the raw success payload: {rendered_plain:?}"
    );
}

#[test]
fn runtime_tool_activity_detailed_diff_keeps_all_rows() {
    let item = ToolResultItem::from_runtime_tool_activity(
        RuntimeToolActivity {
            activity_id: "call-1".to_string(),
            title: "WriteFile: temp.md".to_string(),
            kind: RuntimeToolKind::Edit,
            status: RuntimeToolActivityStatus::Completed,
            content: vec![RuntimeToolActivityContent::Diff {
                path: "temp.md".to_string(),
                old_text: None,
                new_text: (1..=25)
                    .map(|line| format!("line {line}"))
                    .collect::<Vec<_>>()
                    .join("\n"),
                is_truncated: false,
            }],
            locations: Vec::new(),
            raw_input: None,
            raw_output: None,
        },
        ToolActivityRenderMode::Detailed,
    );
    item.prebuild_diff_presentation_with_budget(presentation_budget());
    let rendered_plain = item
        .render_lines(120, default_palette())
        .iter()
        .map(line_to_plain_text)
        .collect::<Vec<_>>();

    assert!(
        rendered_plain
            .iter()
            .any(|line| line == "     13 +  line 13"),
        "detailed mode should keep middle diff rows: {rendered_plain:?}"
    );
    assert!(
        !rendered_plain
            .iter()
            .any(|line| line.contains("ctrl + t to view transcript")),
        "detailed mode should not render compact truncation hints: {rendered_plain:?}"
    );
}

#[test]
fn runtime_tool_activity_updated_diff_renders_delete_and_insert_line_numbers() {
    let item = ToolResultItem::from_runtime_tool_activity(
        RuntimeToolActivity {
            activity_id: "call-1".to_string(),
            title: "WriteFile: src/lib.rs".to_string(),
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
        },
        ToolActivityRenderMode::Detailed,
    );
    item.prebuild_diff_presentation_with_budget(presentation_budget());
    let rendered_plain = item
        .render_lines(120, default_palette())
        .iter()
        .map(line_to_plain_text)
        .collect::<Vec<_>>();

    assert_eq!(rendered_plain[0], "● Edited src/lib.rs (+1 -1)");
    assert!(
        rendered_plain.iter().any(|line| line == "      2 -  old"),
        "updated diff should render old line numbers for deletions: {rendered_plain:?}"
    );
    assert!(
        rendered_plain.iter().any(|line| line == "      2 +  new"),
        "updated diff should render new line numbers for insertions: {rendered_plain:?}"
    );
    assert!(
        rendered_plain.iter().any(|line| line == "      1    one"),
        "context diff rows should right-align the line number and align content after the sign column: {rendered_plain:?}"
    );
    assert!(
        rendered_plain
            .iter()
            .all(|line| !line.contains("---") && !line.contains("+++")),
        "updated diff should not expose raw unified diff file headers: {rendered_plain:?}"
    );
}

#[test]
fn runtime_tool_activity_diff_suppresses_raw_input_and_output_details() {
    let item = ToolResultItem::from_runtime_tool_activity(
        RuntimeToolActivity {
            activity_id: "call-1".to_string(),
            title: "Edit test/temp.md".to_string(),
            kind: RuntimeToolKind::Edit,
            status: RuntimeToolActivityStatus::Completed,
            content: vec![RuntimeToolActivityContent::Diff {
                path: "test/temp.md".to_string(),
                old_text: Some("old\n".to_string()),
                new_text: "new\n".to_string(),
                is_truncated: false,
            }],
            locations: vec![RuntimeToolActivityLocation {
                path: "test/temp.md".to_string(),
                line: None,
            }],
            raw_input: Some(serde_json::json!({
                "path": "test/temp.md",
                "old_string": "old\n",
                "new_string": "new\n"
            })
            .into()),
            raw_output: Some(
                runtime_domain::session::RuntimeToolActivityRawValue::tool_result_with_display_content(
                    "Successfully replaced 1 block(s) in test/temp.md.",
                    Some("Successfully replaced 1 block(s) in test/temp.md."),
                    Some(serde_json::json!({
                        "path": "test/temp.md",
                        "old_text": "old\n",
                        "new_text": "new\n",
                        "replacements": 1
                    })),
                ),
            ),
        },
        ToolActivityRenderMode::Compact,
    );
    item.prebuild_diff_presentation_with_budget(presentation_budget());
    let rendered_plain = item
        .render_lines(120, default_palette())
        .iter()
        .map(line_to_plain_text)
        .collect::<Vec<_>>();

    assert_eq!(rendered_plain[0], "● Edited test/temp.md (+1 -1)");
    assert!(
        rendered_plain.iter().all(|line| !line.contains("Input")),
        "diff rendering should not append raw input details: {rendered_plain:?}"
    );
    assert!(
        rendered_plain
            .iter()
            .all(|line| !line.contains("Successfully replaced 1 block(s)")),
        "diff rendering should not repeat the tool success payload next to the patch view: {rendered_plain:?}"
    );
    assert_eq!(
        rendered_plain
            .iter()
            .filter(|line| line.contains("test/temp.md"))
            .count(),
        1,
        "filename should appear once in the diff header: {rendered_plain:?}"
    );
}

#[test]
fn runtime_tool_activity_diff_right_aligns_three_digit_line_numbers_in_fixed_gutter() {
    let item = ToolResultItem::from_runtime_tool_activity(
        RuntimeToolActivity {
            activity_id: "call-1".to_string(),
            title: "WriteFile: temp.md".to_string(),
            kind: RuntimeToolKind::Edit,
            status: RuntimeToolActivityStatus::Completed,
            content: vec![RuntimeToolActivityContent::Diff {
                path: "temp.md".to_string(),
                old_text: None,
                new_text: (1..=267)
                    .map(|line| format!("line {line}"))
                    .collect::<Vec<_>>()
                    .join("\n"),
                is_truncated: false,
            }],
            locations: Vec::new(),
            raw_input: None,
            raw_output: None,
        },
        ToolActivityRenderMode::Detailed,
    );
    item.prebuild_diff_presentation_with_budget(presentation_budget());
    let rendered_plain = item
        .render_lines(120, default_palette())
        .iter()
        .map(line_to_plain_text)
        .collect::<Vec<_>>();

    assert!(
        rendered_plain
            .iter()
            .any(|line| line == "    267 +  line 267"),
        "three-digit line numbers should grow left within the fixed seven-column gutter: {rendered_plain:?}"
    );
}

#[test]
fn updated_diff_emphasizes_inline_changed_fragments_and_reconstructs_line_text() {
    let lines = diff::diff_detail_lines(
        Some("fn main() {\n    let value = compute(alpha, beta);\n}\n"),
        "fn main() {\n    let value = compute(alpha, gamma);\n}\n",
    );

    let delete_line = lines
        .iter()
        .find(|line| line.kind == diff::RuntimeDiffDetailLineKind::Delete)
        .expect("replace op should produce a delete line");
    let insert_line = lines
        .iter()
        .find(|line| line.kind == diff::RuntimeDiffDetailLineKind::Insert)
        .expect("replace op should produce an insert line");

    assert_eq!(
        delete_line.joined_text(),
        "    let value = compute(alpha, beta);"
    );
    assert_eq!(
        insert_line.joined_text(),
        "    let value = compute(alpha, gamma);"
    );
    assert_eq!(delete_line.line_number, Some(2));
    assert_eq!(insert_line.line_number, Some(2));

    let emphasized_delete = delete_line
        .segments
        .iter()
        .filter(|segment| segment.is_emphasized)
        .map(|segment| segment.text.as_str())
        .collect::<String>();
    let emphasized_insert = insert_line
        .segments
        .iter()
        .filter(|segment| segment.is_emphasized)
        .map(|segment| segment.text.as_str())
        .collect::<String>();
    assert!(
        emphasized_delete.contains("beta") && !emphasized_delete.contains("alpha"),
        "delete emphasis should cover only the changed fragment: {emphasized_delete:?}"
    );
    assert!(
        emphasized_insert.contains("gamma") && !emphasized_insert.contains("alpha"),
        "insert emphasis should cover only the changed fragment: {emphasized_insert:?}"
    );
}

#[test]
fn rendered_inline_emphasis_carries_the_emphasis_tint_through_item_rendering() {
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
    let lines = item.render_lines(120, palette);

    let insert_tint = diff_emphasis_tint(&palette, true)
        .expect("explicit palette should provide an insert emphasis tint");
    let delete_tint = diff_emphasis_tint(&palette, false)
        .expect("explicit palette should provide a delete emphasis tint");
    let emphasized_spans = lines
        .iter()
        .flat_map(|line| line.spans.iter())
        .filter(|span| span.style.add_modifier.contains(Modifier::BOLD) && span.style.bg.is_some())
        .collect::<Vec<_>>();
    assert!(
        emphasized_spans
            .iter()
            .any(|span| span.content.contains("gamma") && span.style.bg == Some(insert_tint)),
        "insert emphasis span should use the saturated insert tint: {lines:?}"
    );
    assert!(
        emphasized_spans
            .iter()
            .any(|span| span.content.contains("beta") && span.style.bg == Some(delete_tint)),
        "delete emphasis span should use the saturated delete tint: {lines:?}"
    );
}

#[test]
fn wrapped_diff_line_keeps_inline_emphasis_across_the_wrap_boundary() {
    // gutter 宽 11 列（行号 7 + 空格 + 符号 + 两空格），width 40 下正文只有 29 列，
    // 因此 45 列的强调段必然跨越折行边界，用来锁定折行后强调样式不丢。
    let palette = default_palette();
    let changed_old = format!("beta_{}", "o".repeat(40));
    let changed_new = format!("gamma_{}", "o".repeat(40));
    let old_text = format!("keep one two three {changed_old} keep four five six\n");
    let new_text = format!("keep one two three {changed_new} keep four five six\n");
    let item = ToolResultItem::from_runtime_tool_activity(
        RuntimeToolActivity {
            activity_id: "call-1".to_string(),
            title: "Edit src/lib.rs".to_string(),
            kind: RuntimeToolKind::Edit,
            status: RuntimeToolActivityStatus::Completed,
            content: vec![RuntimeToolActivityContent::Diff {
                path: "src/lib.rs".to_string(),
                old_text: Some(old_text),
                new_text: new_text.clone(),
                is_truncated: false,
            }],
            locations: Vec::new(),
            raw_input: None,
            raw_output: None,
        },
        ToolActivityRenderMode::Detailed,
    );
    item.prebuild_diff_presentation_with_budget(presentation_budget());

    let lines = item.render_lines(40, palette);

    let insert_tint = diff_emphasis_tint(&palette, true)
        .expect("explicit palette should provide an insert emphasis tint");
    let rows_with_insert_emphasis = lines
        .iter()
        .filter(|line| {
            line.spans
                .iter()
                .any(|span| span.style.bg == Some(insert_tint))
        })
        .count();
    assert!(
        rows_with_insert_emphasis >= 2,
        "an emphasized run wider than the content column must keep its style on every wrapped row: {lines:?}"
    );

    for line in &lines {
        assert!(
            display_width(line_to_plain_text(line).as_str()) <= 40,
            "wrapped diff rows must stay within the render width: {line:?}"
        );
    }

    // 折行行的 gutter 是等宽空格：按序剥掉 gutter 再拼接即可还原整行正文。
    let rendered = lines
        .iter()
        .map(|line| line_to_plain_text(line))
        .collect::<Vec<_>>();
    let insert_gutter = format!(
        "{:>width$} +  ",
        1,
        width = diff::TOOL_ACTIVITY_DIFF_LINE_NUMBER_WIDTH
    );
    let gutter_width = insert_gutter.len();
    let continuation_gutter = " ".repeat(gutter_width);
    let insert_start = rendered
        .iter()
        .position(|text| text.starts_with(&insert_gutter))
        .expect("the inserted line must be rendered");
    let insert_rows = rendered[insert_start..]
        .iter()
        .take_while(|text| {
            text.starts_with(&insert_gutter) || text.starts_with(&continuation_gutter)
        })
        .collect::<Vec<_>>();
    assert!(
        insert_rows.len() > 1,
        "the inserted line must actually wrap for this test to be meaningful: {rendered:#?}"
    );
    let insert_body = insert_rows
        .iter()
        .map(|text| text[gutter_width..].to_string())
        .collect::<String>();
    assert_eq!(
        insert_body,
        new_text.trim_end_matches('\n'),
        "stripping the gutter from the wrapped rows must reconstruct the inserted line"
    );
}

#[test]
fn blank_diff_line_renders_as_a_gutter_only_row() {
    // 空正文的 diff 行不能被折行环节吞掉：应当渲染成只有 gutter 的一行。
    let palette = default_palette();
    let item = ToolResultItem::from_runtime_tool_activity(
        RuntimeToolActivity {
            activity_id: "call-1".to_string(),
            title: "Edit src/lib.rs".to_string(),
            kind: RuntimeToolKind::Edit,
            status: RuntimeToolActivityStatus::Completed,
            content: vec![RuntimeToolActivityContent::Diff {
                path: "src/lib.rs".to_string(),
                old_text: Some("alpha\nbravo\n".to_string()),
                new_text: "alpha\n\nbravo\n".to_string(),
                is_truncated: false,
            }],
            locations: Vec::new(),
            raw_input: None,
            raw_output: None,
        },
        ToolActivityRenderMode::Detailed,
    );
    item.prebuild_diff_presentation_with_budget(presentation_budget());

    let lines = item.render_lines(60, palette);

    let blank_insert_gutter = format!(
        "{:>width$} +  ",
        2,
        width = diff::TOOL_ACTIVITY_DIFF_LINE_NUMBER_WIDTH
    );
    assert!(
        lines
            .iter()
            .any(|line| line_to_plain_text(line) == blank_insert_gutter),
        "an inserted blank line must still occupy a gutter-only row: {lines:?}"
    );
}
