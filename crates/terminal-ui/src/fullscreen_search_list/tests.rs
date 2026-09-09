use std::cell::Cell;

use crate::text_search::CaseInsensitiveQuery;

use super::FullscreenSearchListState;

#[derive(Debug, Clone, PartialEq, Eq)]
struct Row {
    id: &'static str,
    text: &'static str,
}

fn row_text_matches(row: &Row, query: &CaseInsensitiveQuery<'_>) -> bool {
    query.matches(row.text)
}

fn sample_rows() -> Vec<Row> {
    vec![
        Row {
            id: "one",
            text: "alpha",
        },
        Row {
            id: "two",
            text: "beta",
        },
        Row {
            id: "three",
            text: "beta extra",
        },
    ]
}

#[test]
fn filter_restores_selected_row_by_stable_id() {
    let mut state = FullscreenSearchListState::default();
    state.replace_rows(sample_rows(), row_text_matches, |row| row.id);
    state.selected = 1;
    state.sync_selected_id(|row| row.id);

    state.push_search_character('b', row_text_matches, |row| row.id);

    assert_eq!(state.filtered_indices_for_test(), &[1, 2]);
    assert_eq!(state.selected_visible_position(), Some(0));
    assert_eq!(state.selected_row().map(|row| row.id), Some("two"));
}

#[test]
fn exit_search_preserves_selected_row_and_clears_query() {
    let mut state = FullscreenSearchListState::default();
    state.replace_rows(sample_rows(), row_text_matches, |row| row.id);
    state.selected = 2;
    state.sync_selected_id(|row| row.id);
    state.start_search();

    state.push_search_character('b', row_text_matches, |row| row.id);
    assert_eq!(state.selected_row().map(|row| row.id), Some("three"));

    assert!(state.exit_search(row_text_matches, |row| row.id));
    assert!(!state.is_searching());
    assert!(state.search_query().is_empty());
    assert_eq!(state.selected_row().map(|row| row.id), Some("three"));
}

#[test]
fn clear_search_keeps_search_mode_active() {
    let mut state = FullscreenSearchListState::default();
    state.replace_rows(sample_rows(), row_text_matches, |row| row.id);
    state.start_search();
    state.push_search_character('b', row_text_matches, |row| row.id);

    assert!(state.clear_search(row_text_matches, |row| row.id));
    assert!(state.is_searching());
    assert!(state.search_query().is_empty());
    assert_eq!(state.filtered_indices_for_test(), &[0, 1, 2]);
}

#[test]
fn upsert_row_replaces_in_place_and_keeps_selection_identity() {
    let mut state = FullscreenSearchListState::default();
    state.replace_rows(sample_rows(), row_text_matches, |row| row.id);
    state.selected = 2;
    state.sync_selected_id(|row| row.id);
    assert_eq!(state.selected_row().map(|row| row.id), Some("three"));

    // 既有行原位替换：行序与 selection 不变。
    state.upsert_row(
        Row {
            id: "one",
            text: "alpha updated",
        },
        row_text_matches,
        |row| row.id,
    );

    assert_eq!(state.filtered_count(), 3);
    assert_eq!(state.selected_row().map(|row| row.id), Some("three"));
    assert_eq!(state.rows()[0].text, "alpha updated");
}

#[test]
fn upsert_row_appends_new_row_without_resetting_selection() {
    let mut state = FullscreenSearchListState::default();
    state.replace_rows(sample_rows(), row_text_matches, |row| row.id);
    state.selected = 1;
    state.sync_selected_id(|row| row.id);

    state.upsert_row(
        Row {
            id: "four",
            text: "gamma",
        },
        row_text_matches,
        |row| row.id,
    );

    assert_eq!(state.filtered_count(), 4);
    assert_eq!(state.selected_row().map(|row| row.id), Some("two"));
}

#[test]
fn remove_row_clamps_selection_to_remaining_rows() {
    let mut state = FullscreenSearchListState::default();
    state.replace_rows(sample_rows(), row_text_matches, |row| row.id);
    state.selected = 1;
    state.sync_selected_id(|row| row.id);

    state.remove_row("two", row_text_matches, |row| row.id);

    assert_eq!(state.filtered_count(), 2);
    // selected position clamp：被移除行之后回落到剩余列表的同一位置。
    assert_eq!(state.selected_row().map(|row| row.id), Some("three"));
}

#[test]
fn select_id_targets_row_in_filtered_view() {
    let mut state = FullscreenSearchListState::default();
    state.replace_rows(sample_rows(), row_text_matches, |row| row.id);
    state.selected = 0;
    state.sync_selected_id(|row| row.id);

    assert!(state.select_id("two", |row| row.id));
    assert_eq!(state.selected_row().map(|row| row.id), Some("two"));
}

#[test]
fn select_id_respects_search_filter() {
    let mut state = FullscreenSearchListState::default();
    state.replace_rows(sample_rows(), row_text_matches, |row| row.id);
    state.start_search();
    // 过滤后只剩 beta 两行："one" 不在 filtered 视图中。
    state.push_search_character('b', row_text_matches, |row| row.id);
    state.selected = 0;
    state.sync_selected_id(|row| row.id);

    assert!(!state.select_id("one", |row| row.id));
    // fail closed：选中目标不存在时保持原 selection。
    assert_eq!(state.selected_row().map(|row| row.id), Some("two"));

    assert!(state.select_id("three", |row| row.id));
    assert_eq!(state.selected_row().map(|row| row.id), Some("three"));
}

#[test]
fn select_id_missing_keeps_selection() {
    let mut state = FullscreenSearchListState::default();
    state.replace_rows(sample_rows(), row_text_matches, |row| row.id);
    state.selected = 1;
    state.sync_selected_id(|row| row.id);

    assert!(!state.select_id("missing", |row| row.id));
    assert_eq!(state.selected_row().map(|row| row.id), Some("two"));
}

// ---- reorder_rows 幂等早退 ----

#[test]
fn reorder_rows_skips_the_filtered_rebuild_on_ordered_input() {
    // 分组归一在每帧渲染前调用：行序已满足目标顺序时必须早退——不进入
    // sort，也不重建过滤视图（matches_query 零调用），行序/过滤/selection
    // 原样保持。
    let mut state = FullscreenSearchListState::default();
    // sample_rows 按 text 字母序（alpha < beta < beta extra）有序。
    state.replace_rows(sample_rows(), row_text_matches, |row| row.id);
    state.selected = 1;
    state.sync_selected_id(|row| row.id);

    let match_calls = Cell::new(0usize);
    state.reorder_rows(
        |a, b| a.text.cmp(b.text),
        |row, query| {
            match_calls.set(match_calls.get() + 1);
            row_text_matches(row, query)
        },
        |row| row.id,
    );

    assert_eq!(
        match_calls.get(),
        0,
        "ordered input must skip the filtered-view rebuild"
    );
    assert_eq!(state.filtered_indices_for_test(), &[0, 1, 2]);
    assert_eq!(state.selected_row().map(|row| row.id), Some("two"));
}

#[test]
fn reorder_rows_sorts_unordered_input_and_rebuilds_the_filter() {
    let rows = vec![
        Row {
            id: "one",
            text: "gamma",
        },
        Row {
            id: "two",
            text: "alpha",
        },
        Row {
            id: "three",
            text: "beta extra",
        },
    ];
    let mut state = FullscreenSearchListState::default();
    state.replace_rows(rows, row_text_matches, |row| row.id);
    state.selected = 0;
    state.sync_selected_id(|row| row.id);
    // 搜索过滤后只剩 "beta extra" 一行。
    state.start_search();
    state.push_search_character('b', row_text_matches, |row| row.id);
    assert_eq!(state.filtered_indices_for_test(), &[2]);

    state.reorder_rows(|a, b| a.text.cmp(b.text), row_text_matches, |row| row.id);

    // 无序输入走完整路径：重排行存储、重建过滤视图（每行匹配一次）、
    // selection 以 stable id 重锚到迁移后的行。
    assert_eq!(
        state.rows().iter().map(|row| row.id).collect::<Vec<_>>(),
        vec!["two", "three", "one"],
    );
    assert_eq!(state.filtered_indices_for_test(), &[1]);
    assert_eq!(state.selected_row().map(|row| row.id), Some("three"));
}
