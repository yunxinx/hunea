use super::*;

#[test]
fn empty_new_file_has_no_synthetic_insert_line() {
    let call = diff_call_with_contents(vec![RuntimeToolActivityContent::Diff {
        path: "empty.rs".to_string(),
        old_text: None,
        new_text: String::new(),
        is_truncated: false,
    }]);
    let presentations = diff::DiffPresentations::build(&call, presentation_budget());
    let presentation = presentations
        .for_content(0)
        .expect("diff content must have a positional presentation");

    assert_eq!((presentation.added, presentation.removed), (0, 0));
    assert!(
        detail_diff_lines(&call, &presentations).is_empty(),
        "empty text has no line tokens and must not create a synthetic insert row"
    );
    assert_eq!(
        diff_header_text(&call, &presentations),
        "Added empty.rs (+0 -0)"
    );

    let rendered_plain =
        ToolResultItem::from_runtime_tool_activity(call, ToolActivityRenderMode::Detailed)
            .render_lines(120, default_palette())
            .iter()
            .map(line_to_plain_text)
            .collect::<Vec<_>>();
    assert_eq!(rendered_plain, ["● Added empty.rs (+0 -0)"]);
}

#[test]
fn existing_empty_file_is_edited_when_content_is_inserted() {
    let call = diff_call_with_contents(vec![RuntimeToolActivityContent::Diff {
        path: "existing.rs".to_string(),
        old_text: Some(String::new()),
        new_text: "line\n".to_string(),
        is_truncated: false,
    }]);
    let presentations = diff::DiffPresentations::build(&call, presentation_budget());
    let presentation = presentations
        .for_content(0)
        .expect("diff content must have a positional presentation");

    assert_eq!((presentation.added, presentation.removed), (1, 0));
    assert_eq!(
        count_insert_delete_lines(&detail_diff_lines(&call, &presentations)),
        (1, 0)
    );
    assert_eq!(
        diff_header_text(&call, &presentations),
        "Edited existing.rs (+1 -0)"
    );
}

#[test]
fn bare_carriage_returns_have_identical_new_file_and_empty_file_diff_semantics() {
    let new_text = "alpha\rbeta\r";
    let project_lines = |lines: Vec<diff::RuntimeDiffDetailLine>| {
        lines
            .into_iter()
            .map(|line| (line.line_number, line.kind, line.joined_text()))
            .collect::<Vec<_>>()
    };

    let added_lines = project_lines(diff::diff_detail_lines(None, new_text));
    let edited_lines = project_lines(diff::diff_detail_lines(Some(""), new_text));
    let expected_lines = vec![
        (
            Some(1),
            diff::RuntimeDiffDetailLineKind::Insert,
            "alpha".to_string(),
        ),
        (
            Some(2),
            diff::RuntimeDiffDetailLineKind::Insert,
            "beta".to_string(),
        ),
    ];
    assert_eq!(added_lines, expected_lines);
    assert_eq!(edited_lines, expected_lines);

    for old_text in [None, Some(String::new())] {
        let call = diff_call_with_contents(vec![RuntimeToolActivityContent::Diff {
            path: "src/lib.rs".to_string(),
            old_text,
            new_text: new_text.to_string(),
            is_truncated: false,
        }]);
        let presentations = diff::DiffPresentations::build(&call, presentation_budget());
        let presentation = presentations
            .for_content(0)
            .expect("diff content must have a presentation");
        assert_eq!((presentation.added, presentation.removed), (2, 0));
    }
}

#[test]
fn header_counts_and_detail_lines_stay_consistent_from_one_shared_presentation() {
    let call = diff_call_with_contents(vec![RuntimeToolActivityContent::Diff {
        path: "src/lib.rs".to_string(),
        old_text: Some("alpha\nbravo\ncharlie\n".to_string()),
        new_text: "alpha\nbravo changed\ncharlie\ndelta\n".to_string(),
        is_truncated: false,
    }]);
    let presentations = diff::DiffPresentations::build(&call, presentation_budget());

    let presentation = presentations
        .for_content(0)
        .expect("diff content must have a positional presentation");
    assert!(
        presentation.added > 0 && presentation.removed > 0,
        "sanity: this input must count both inserts and deletes: {presentation:?}"
    );

    let header_text = diff_header_text(&call, &presentations);
    assert!(
        header_text.contains(&format!("+{}", presentation.added))
            && header_text.contains(&format!("-{}", presentation.removed)),
        "header counts must come from the shared presentation: {header_text:?}"
    );

    let (inserted, deleted) = count_insert_delete_lines(&detail_diff_lines(&call, &presentations));
    assert_eq!(
        (inserted, deleted),
        (presentation.added, presentation.removed),
        "detail insert/delete lines must match the header counts by construction"
    );
}

#[test]
fn exhausted_budget_keeps_header_and_detail_counts_consistent() {
    let old_text = (1..=24)
        .map(|index| format!("shared line {index} alpha\n"))
        .collect::<String>();
    let new_text = (1..=24)
        .map(|index| format!("shared line {index} gamma\n"))
        .collect::<String>();
    let call = diff_call_with_contents(vec![RuntimeToolActivityContent::Diff {
        path: "src/lib.rs".to_string(),
        old_text: Some(old_text),
        new_text,
        is_truncated: false,
    }]);

    // 注入已耗尽预算：行级结果变粗、行内细化全部跳过，
    // 但 header 与 detail 仍取自同一 presentation，按构造一致。
    let presentations = diff::DiffPresentations::build(&call, diff::DiffBudget::exhausted());
    let presentation = presentations
        .for_content(0)
        .expect("diff content must have a positional presentation");
    assert!(
        presentation
            .detailed_lines
            .iter()
            .all(|line| line.segments.iter().all(|segment| !segment.is_emphasized)),
        "an exhausted budget must not emphasize any segment: {presentation:?}"
    );

    let header_text = diff_header_text(&call, &presentations);
    assert!(
        header_text.contains(&format!("+{}", presentation.added))
            && header_text.contains(&format!("-{}", presentation.removed)),
        "header counts must come from the shared presentation: {header_text:?}"
    );

    let (inserted, deleted) = count_insert_delete_lines(&detail_diff_lines(&call, &presentations));
    assert_eq!(
        (inserted, deleted),
        (presentation.added, presentation.removed),
        "detail lines built under an exhausted budget must still match the header counts"
    );
}

#[test]
fn multi_diff_contents_align_positionally_and_sum_into_the_edited_files_header() {
    let call = diff_call_with_contents(vec![
        RuntimeToolActivityContent::Diff {
            path: "a.rs".to_string(),
            old_text: Some("alpha\nbravo\n".to_string()),
            new_text: "alpha\nbravo changed\n".to_string(),
            is_truncated: false,
        },
        RuntimeToolActivityContent::Text("note".to_string()),
        RuntimeToolActivityContent::Diff {
            path: "b.rs".to_string(),
            old_text: Some("one\ntwo\nthree\n".to_string()),
            new_text: "one\nthree\n".to_string(),
            is_truncated: false,
        },
    ]);
    let presentations = diff::DiffPresentations::build(&call, presentation_budget());

    let first = presentations
        .for_content(0)
        .expect("first diff content must have a presentation");
    assert!(
        presentations.for_content(1).is_none(),
        "non-diff content must not carry a presentation"
    );
    let second = presentations
        .for_content(2)
        .expect("second diff content must have a presentation");

    let header_text = diff_header_text(&call, &presentations);
    assert!(
        header_text.contains("Edited 2 files"),
        "multi-file header must summarize the diff content count: {header_text:?}"
    );
    assert!(
        header_text.contains(&format!("+{}", first.added + second.added))
            && header_text.contains(&format!("-{}", first.removed + second.removed)),
        "multi-file header counts must equal the sum of per-content presentations: {header_text:?}"
    );
}

#[test]
fn tool_activity_without_diff_content_skips_presentation_allocation() {
    let call = diff_call_with_contents(vec![
        RuntimeToolActivityContent::Text("plain output".to_string()),
        RuntimeToolActivityContent::Text("more output".to_string()),
    ]);
    let presentations = diff::DiffPresentations::build(&call, presentation_budget());

    // 空表快速路径：按下标读取恒为 None，与全 None 语义等价。
    assert!(
        presentations.is_empty(),
        "calls without diff content should take the empty fast path"
    );
    assert!(presentations.for_content(0).is_none());
}

#[test]
fn rendering_tool_activity_without_diff_does_not_allocate_presentations() {
    let call = diff_call_with_contents(vec![RuntimeToolActivityContent::Text(
        "plain output".to_string(),
    )]);
    let item = ToolResultItem::from_runtime_tool_activity(call, ToolActivityRenderMode::Detailed);

    assert!(!item.has_cached_no_diff_state());
    let lines = item.render_lines(80, default_palette());
    assert!(!lines.is_empty());
    assert!(
        item.has_cached_no_diff_state(),
        "rendering a non-Diff activity must cache NoDiff without allocating a presentation"
    );
}

#[test]
fn inline_refinement_uses_deadline_controlled_default_options() {
    assert!(
        !diff::inline_semantic_cleanup_enabled(),
        "post-deadline semantic cleanup must remain disabled on the render path"
    );
}
