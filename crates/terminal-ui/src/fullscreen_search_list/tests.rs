use super::FullscreenSearchListState;

#[derive(Debug, Clone, PartialEq, Eq)]
struct Row {
    id: &'static str,
    text: &'static str,
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
    state.replace_rows(
        sample_rows(),
        |row, query| row.text.contains(query),
        |row| row.id,
    );
    state.selected = 1;
    state.sync_selected_id(|row| row.id);

    state.push_search_character('b', |row, query| row.text.contains(query), |row| row.id);

    assert_eq!(state.filtered_indices_for_test(), &[1, 2]);
    assert_eq!(state.selected_visible_position(), Some(0));
    assert_eq!(state.selected_row().map(|row| row.id), Some("two"));
}

#[test]
fn exit_search_preserves_selected_row_and_clears_query() {
    let mut state = FullscreenSearchListState::default();
    state.replace_rows(
        sample_rows(),
        |row, query| row.text.contains(query),
        |row| row.id,
    );
    state.selected = 2;
    state.sync_selected_id(|row| row.id);
    state.start_search();

    state.push_search_character('b', |row, query| row.text.contains(query), |row| row.id);
    assert_eq!(state.selected_row().map(|row| row.id), Some("three"));

    assert!(state.exit_search(|row, query| row.text.contains(query), |row| row.id));
    assert!(!state.is_searching());
    assert!(state.search_query().is_empty());
    assert_eq!(state.selected_row().map(|row| row.id), Some("three"));
}

#[test]
fn clear_search_keeps_search_mode_active() {
    let mut state = FullscreenSearchListState::default();
    state.replace_rows(
        sample_rows(),
        |row, query| row.text.contains(query),
        |row| row.id,
    );
    state.start_search();
    state.push_search_character('b', |row, query| row.text.contains(query), |row| row.id);

    assert!(state.clear_search(|row, query| row.text.contains(query), |row| row.id));
    assert!(state.is_searching());
    assert!(state.search_query().is_empty());
    assert_eq!(state.filtered_indices_for_test(), &[0, 1, 2]);
}
