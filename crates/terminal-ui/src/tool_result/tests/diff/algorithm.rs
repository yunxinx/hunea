use super::*;

#[test]
fn pure_insert_delete_blocks_and_context_lines_have_no_emphasized_segments() {
    let pure_insert = diff::diff_detail_lines(
        Some("alpha\nbravo\ncharlie\n"),
        "alpha\ninserted\nbravo\ncharlie\n",
    );
    let pure_delete = diff::diff_detail_lines(Some("alpha\nbravo\ncharlie\n"), "alpha\ncharlie\n");

    for line in pure_insert.iter().chain(pure_delete.iter()) {
        assert!(
            line.segments.iter().all(|segment| !segment.is_emphasized),
            "pure insert/delete blocks and context lines must not carry emphasis: {line:?}"
        );
    }
}

/// 断言 diff 行的全部段间边界落在 grapheme boundary 上，
/// 保证 CJK / emoji modifier / ZWJ 序列不被行内强调切裂。
/// 末段终点必然等于整行字节长（段拼接即整行），无需额外断言。
fn assert_segments_align_with_grapheme_boundaries(line: &diff::RuntimeDiffDetailLine) {
    use unicode_segmentation::UnicodeSegmentation;

    let joined = line.joined_text();
    let grapheme_boundaries = joined
        .grapheme_indices(true)
        .map(|(offset, _)| offset)
        .collect::<Vec<_>>();

    let mut offset = 0usize;
    for segment in &line.segments {
        assert!(
            offset == 0 || grapheme_boundaries.contains(&offset),
            "segment boundary at byte {offset} must align with a grapheme boundary: {line:?}"
        );
        offset += segment.text.len();
    }
}

#[test]
fn cjk_inline_changes_keep_grapheme_boundaries_intact() {
    let lines = diff::diff_detail_lines(
        Some("进度说明：任务已经开始 👍🏽 请继续\n"),
        "进度说明：任务已经完成 👍🏽 请继续\n",
    );

    let changed_lines = lines
        .iter()
        .filter(|line| line.kind != diff::RuntimeDiffDetailLineKind::Context)
        .collect::<Vec<_>>();
    assert!(
        changed_lines
            .iter()
            .any(|line| line.segments.iter().any(|segment| segment.is_emphasized)),
        "CJK word-level change should produce emphasized segments: {changed_lines:?}"
    );

    for line in &changed_lines {
        assert_segments_align_with_grapheme_boundaries(line);
    }
}

#[test]
fn emoji_modifier_sequence_change_lands_intact_inside_an_emphasized_segment() {
    // 👍🏽 (U+1F44D U+1F3FD) 与 👍🏻 (U+1F44D U+1F3FB) 共享首码点，
    // char 级 diff 最易只强调 skin-tone modifier 而切裂 grapheme。
    let lines = diff::diff_detail_lines(
        Some("状态更新：审阅结果 👍🏽 已记录，感谢确认\n"),
        "状态更新：审阅结果 👍🏻 已记录，感谢确认\n",
    );

    let delete_line = lines
        .iter()
        .find(|line| line.kind == diff::RuntimeDiffDetailLineKind::Delete)
        .expect("modifier-sequence change should produce a delete line");
    let insert_line = lines
        .iter()
        .find(|line| line.kind == diff::RuntimeDiffDetailLineKind::Insert)
        .expect("modifier-sequence change should produce an insert line");

    for line in [delete_line, insert_line] {
        assert!(
            line.segments.iter().any(|segment| segment.is_emphasized),
            "emoji modifier change should produce emphasized segments: {line:?}"
        );
        assert_segments_align_with_grapheme_boundaries(line);
    }
    assert!(
        delete_line
            .segments
            .iter()
            .any(|segment| segment.is_emphasized && segment.text.contains("👍🏽")),
        "the full sequence 👍🏽 must land intact inside an emphasized segment: {delete_line:?}"
    );
    assert!(
        insert_line
            .segments
            .iter()
            .any(|segment| segment.is_emphasized && segment.text.contains("👍🏻")),
        "the full sequence 👍🏻 must land intact inside an emphasized segment: {insert_line:?}"
    );
}

#[test]
fn emoji_zwj_sequence_change_lands_intact_inside_an_emphasized_segment() {
    // 👨‍👩‍👧 与 👨‍👩‍👦 共享前缀码点（男 ZWJ 女 ZWJ ...），仅末码点不同，
    // 是 ZWJ 序列被 diff 切裂的最典型形态。
    let lines = diff::diff_detail_lines(
        Some("家庭合影：本周活动 👨\u{200d}👩\u{200d}👧 参与踊跃，欢迎报名\n"),
        "家庭合影：本周活动 👨\u{200d}👩\u{200d}👦 参与踊跃，欢迎报名\n",
    );

    let delete_line = lines
        .iter()
        .find(|line| line.kind == diff::RuntimeDiffDetailLineKind::Delete)
        .expect("ZWJ-sequence change should produce a delete line");
    let insert_line = lines
        .iter()
        .find(|line| line.kind == diff::RuntimeDiffDetailLineKind::Insert)
        .expect("ZWJ-sequence change should produce an insert line");

    for line in [delete_line, insert_line] {
        assert!(
            line.segments.iter().any(|segment| segment.is_emphasized),
            "ZWJ sequence change should produce emphasized segments: {line:?}"
        );
        assert_segments_align_with_grapheme_boundaries(line);
    }
    assert!(
        delete_line
            .segments
            .iter()
            .any(|segment| segment.is_emphasized && segment.text.contains("👨\u{200d}👩\u{200d}👧")),
        "the full ZWJ sequence must land intact inside an emphasized segment: {delete_line:?}"
    );
    assert!(
        insert_line
            .segments
            .iter()
            .any(|segment| segment.is_emphasized && segment.text.contains("👨\u{200d}👩\u{200d}👦")),
        "the full ZWJ sequence must land intact inside an emphasized segment: {insert_line:?}"
    );
}

/// 构造一个整块替换的 diff：两侧各 `lines_per_side` 行且逐行不同，
/// 使 similar 产出单个 Replace op，op 行数之和恰为 `lines_per_side * 2`。
fn whole_block_replacement(lines_per_side: usize) -> (String, String) {
    let old_text = (0..lines_per_side)
        .map(|index| format!("shared line {index} with alpha words\n"))
        .collect::<String>();
    let new_text = (0..lines_per_side)
        .map(|index| format!("shared line {index} with gamma words\n"))
        .collect::<String>();
    (old_text, new_text)
}

#[test]
fn replace_block_at_the_line_budget_still_refines_inline() {
    // 两侧各 32 行 = 64 行，恰为 INLINE_MAX_OP_LINES 上限，仍应细化。
    let (old_text, new_text) = whole_block_replacement(32);

    let lines = diff::diff_detail_lines(Some(old_text.as_str()), new_text.as_str());

    assert!(
        lines
            .iter()
            .any(|line| line.segments.iter().any(|segment| segment.is_emphasized)),
        "a replace op exactly at the line-count budget must still get inline emphasis"
    );
}

#[test]
fn oversized_replace_block_falls_back_to_plain_line_styling() {
    // 两侧各 33 行 = 66 行，越过 INLINE_MAX_OP_LINES 上限。
    let (old_text, new_text) = whole_block_replacement(33);

    let lines = diff::diff_detail_lines(Some(old_text.as_str()), new_text.as_str());

    assert!(
        lines
            .iter()
            .all(|line| line.segments.iter().all(|segment| !segment.is_emphasized)),
        "replace ops beyond the line-count budget must fall back to plain styling"
    );
    assert!(
        lines
            .iter()
            .any(|line| line.joined_text() == "shared line 32 with gamma words"),
        "fallback lines must keep the full original text"
    );
}

#[test]
fn overlong_replace_lines_fall_back_to_plain_line_styling() {
    let old_line = format!("{} alpha", "x".repeat(1100));
    let new_line = format!("{} gamma", "x".repeat(1100));
    let old_text = format!("{old_line}\n");
    let new_text = format!("{new_line}\n");

    let lines = diff::diff_detail_lines(Some(old_text.as_str()), new_text.as_str());

    assert!(
        lines
            .iter()
            .all(|line| line.segments.iter().all(|segment| !segment.is_emphasized)),
        "replace ops with overlong lines must fall back to plain styling"
    );
    assert!(
        lines.iter().any(|line| line.joined_text() == old_line)
            && lines.iter().any(|line| line.joined_text() == new_line),
        "fallback lines must keep the full original text"
    );
}

#[test]
fn exhausted_budget_falls_back_to_plain_single_segment_lines_without_panic() {
    // 多个彼此分离的 replace hunk，正常预算下必有强调段。
    let build_text = |tokens: [&str; 3]| {
        (1..=24)
            .map(|index| match index {
                2 => format!("second line with {} token\n", tokens[0]),
                12 => format!("twelfth line with {} token\n", tokens[1]),
                22 => format!("final line with {} token\n", tokens[2]),
                other => format!("shared context line {other}\n"),
            })
            .collect::<String>()
    };
    let old_text = build_text(["alpha", "bravo", "charlie"]);
    let new_text = build_text(["delta", "echo", "foxtrot"]);

    let fresh_budget_lines = diff::diff_detail_lines(Some(old_text.as_str()), new_text.as_str());
    assert!(
        fresh_budget_lines
            .iter()
            .any(|line| line.segments.iter().any(|segment| segment.is_emphasized)),
        "sanity: this input must produce emphasis under a fresh budget"
    );

    // 注入已耗尽的预算：行级 diff 变粗但合法，全部行内细化被跳过。
    let expired_lines = diff::diff_detail_lines_with_budget(
        Some(old_text.as_str()),
        new_text.as_str(),
        diff::DiffBudget::exhausted(),
    );

    assert!(!expired_lines.is_empty());
    for line in &expired_lines {
        assert_eq!(
            line.segments.len(),
            1,
            "an exhausted budget must yield plain single-segment lines: {line:?}"
        );
        assert!(
            !line.segments[0].is_emphasized,
            "an exhausted budget must not emphasize any segment: {line:?}"
        );
    }
}

#[test]
fn inline_budget_keeps_refinement_at_exactly_the_character_limit() {
    // 正文恰 1000 字符：重复短词 + 5 字符变化 token，避免单巨型 token 触发 min_ratio 回退。
    let old_body = format!("{}alpha", "word ".repeat(199));
    let new_body = format!("{}gamma", "word ".repeat(199));
    assert_eq!(old_body.chars().count(), 1000);
    assert_eq!(new_body.chars().count(), 1000);

    let has_emphasis = |lines: &[diff::RuntimeDiffDetailLine]| {
        lines
            .iter()
            .any(|line| line.segments.iter().any(|segment| segment.is_emphasized))
    };

    let lf_old = format!("{old_body}\n");
    let lf_new = format!("{new_body}\n");
    let lf_lines = diff::diff_detail_lines(Some(lf_old.as_str()), lf_new.as_str());
    assert!(
        has_emphasis(&lf_lines),
        "a 1000-char body with LF terminator sits within the inline budget: {lf_lines:?}"
    );

    // CRLF 终止符不占正文预算；修复前 \r\n 被错误计入导致回退。
    let crlf_old = format!("{old_body}\r\n");
    let crlf_new = format!("{new_body}\r\n");
    let crlf_lines = diff::diff_detail_lines(Some(crlf_old.as_str()), crlf_new.as_str());
    assert!(
        has_emphasis(&crlf_lines),
        "a 1000-char body with CRLF terminator sits within the inline budget: {crlf_lines:?}"
    );
    assert!(
        crlf_lines.iter().any(|line| {
            line.kind == diff::RuntimeDiffDetailLineKind::Delete && line.joined_text() == old_body
        }) && crlf_lines.iter().any(|line| {
            line.kind == diff::RuntimeDiffDetailLineKind::Insert && line.joined_text() == new_body
        }),
        "CRLF refinement must reconstruct both line bodies without retaining terminators"
    );
}

#[test]
fn inline_budget_falls_back_one_character_beyond_the_limit() {
    // 正文 1001 字符：越界一个字符即整个 op 回退 plain。
    let old_body = format!("{}alphas", "word ".repeat(199));
    let new_body = format!("{}gammas", "word ".repeat(199));
    assert_eq!(old_body.chars().count(), 1001);
    assert_eq!(new_body.chars().count(), 1001);

    let old_text = format!("{old_body}\n");
    let new_text = format!("{new_body}\n");
    let lines = diff::diff_detail_lines(Some(old_text.as_str()), new_text.as_str());

    assert!(
        lines
            .iter()
            .all(|line| line.segments.iter().all(|segment| !segment.is_emphasized)),
        "a 1001-char body must fall back to plain styling: {lines:?}"
    );
    assert!(
        lines.iter().any(|line| line.joined_text() == old_body)
            && lines.iter().any(|line| line.joined_text() == new_body),
        "fallback lines must keep the full original text"
    );
}

#[test]
fn stripping_line_terminators_reuses_the_existing_string_allocation() {
    for (input, expected) in [
        ("content\n", "content"),
        ("content\r\n", "content"),
        ("content\r", "content"),
        ("content", "content"),
    ] {
        let mut line = input.to_string();
        let allocation = line.as_ptr();

        diff::strip_line_terminator_in_place(&mut line);

        assert_eq!(line, expected);
        assert_eq!(
            line.as_ptr(),
            allocation,
            "stripping {input:?} should not replace the existing String allocation"
        );
    }
}

#[test]
fn diff_emphasis_style_degrades_to_reversed_without_surface() {
    let terminal_default = terminal_default_palette();
    for kind in [
        diff::RuntimeDiffDetailLineKind::Insert,
        diff::RuntimeDiffDetailLineKind::Delete,
    ] {
        let style = diff::runtime_tool_activity_diff_emphasis_style(kind, terminal_default);
        assert!(
            style.add_modifier.contains(Modifier::REVERSED),
            "terminal-default palette must keep emphasis visible through REVERSED: {style:?}"
        );
        assert_eq!(style.bg, None);
    }

    let explicit = default_palette();
    let insert_style = diff::runtime_tool_activity_diff_emphasis_style(
        diff::RuntimeDiffDetailLineKind::Insert,
        explicit,
    );
    let delete_style = diff::runtime_tool_activity_diff_emphasis_style(
        diff::RuntimeDiffDetailLineKind::Delete,
        explicit,
    );
    assert_eq!(insert_style.bg, diff_emphasis_tint(&explicit, true));
    assert_eq!(delete_style.bg, diff_emphasis_tint(&explicit, false));
    assert!(insert_style.bg.is_some());
    assert!(delete_style.bg.is_some());
    assert!(insert_style.add_modifier.contains(Modifier::BOLD));
    assert!(!insert_style.add_modifier.contains(Modifier::REVERSED));
}

#[test]
fn multi_hunk_diff_keeps_the_separator_between_hunks() {
    let old_text = (1..=20)
        .map(|index| format!("line {index}\n"))
        .collect::<String>();
    let new_text = (1..=20)
        .map(|index| match index {
            2 => "line two changed\n".to_string(),
            18 => "line eighteen changed\n".to_string(),
            other => format!("line {other}\n"),
        })
        .collect::<String>();

    let lines = diff::diff_detail_lines(Some(old_text.as_str()), new_text.as_str());

    let separators = lines
        .iter()
        .filter(|line| line.kind == diff::RuntimeDiffDetailLineKind::Separator)
        .collect::<Vec<_>>();
    assert_eq!(
        separators.len(),
        1,
        "two distant hunks should be joined by exactly one separator: {lines:?}"
    );
    assert_eq!(separators[0].joined_text(), "⋮");
    assert_eq!(separators[0].line_number, None);
}

#[test]
fn diff_hunks_split_when_context_windows_only_touch() {
    for (equal_gap_lines, expected_separators) in [(5, 0), (6, 1), (7, 1)] {
        let old_text = (0..equal_gap_lines + 2)
            .map(|index| format!("line {index}\n"))
            .collect::<String>();
        let new_text = (0..equal_gap_lines + 2)
            .map(|index| match index {
                0 => "first line changed\n".to_string(),
                index if index == equal_gap_lines + 1 => "last line changed\n".to_string(),
                other => format!("line {other}\n"),
            })
            .collect::<String>();

        let lines = diff::diff_detail_lines(Some(old_text.as_str()), new_text.as_str());
        let separator_count = lines
            .iter()
            .filter(|line| line.kind == diff::RuntimeDiffDetailLineKind::Separator)
            .count();
        let context_count = lines
            .iter()
            .filter(|line| line.kind == diff::RuntimeDiffDetailLineKind::Context)
            .count();

        assert_eq!(
            separator_count, expected_separators,
            "hunks separated by {equal_gap_lines} equal lines have an unexpected grouping: {lines:?}"
        );
        assert_eq!(
            context_count,
            equal_gap_lines.min(6),
            "each hunk must keep at most three context lines: {lines:?}"
        );
    }
}
