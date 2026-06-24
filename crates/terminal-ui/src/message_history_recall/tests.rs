use runtime_domain::session::MessageHistoryEntry;

use super::state::BlindRecallState;

fn entry(text: &str) -> MessageHistoryEntry {
    MessageHistoryEntry {
        ts: 1,
        text: text.to_string(),
    }
}

fn cached_texts(state: &BlindRecallState) -> Vec<String> {
    state.cache().into_iter().map(|entry| entry.text).collect()
}

#[test]
fn gate_empty_history_is_false() {
    let state = BlindRecallState::default();
    assert!(!state.should_handle_navigation("", 0));
}

#[test]
fn gate_empty_text_is_true_when_cache_nonempty() {
    let mut state = BlindRecallState::default();
    state.replace_cache(vec![entry("a")]);
    assert!(state.should_handle_navigation("", 0));
}

#[test]
fn gate_requires_last_history_text_and_boundary_cursor() {
    let mut state = BlindRecallState::default();
    state.replace_cache(vec![entry("hello")]);
    let _ = state.navigate_up();

    assert!(state.should_handle_navigation("hello", 0));
    assert!(state.should_handle_navigation("hello", 5));
    assert!(!state.should_handle_navigation("hello", 2));
    assert!(!state.should_handle_navigation("hell", 4));
}

#[test]
fn navigate_up_from_empty_starts_at_newest() {
    let mut state = BlindRecallState::default();
    state.replace_cache(vec![entry("old"), entry("new")]);
    assert!(state.navigate_up());
    assert_eq!(state.active_history_text(), Some("new"));
    assert_eq!(state.history_cursor(), Some(1));
}

#[test]
fn navigate_up_at_oldest_is_noop() {
    let mut state = BlindRecallState::default();
    state.replace_cache(vec![entry("only")]);
    assert!(state.navigate_up());
    assert!(!state.navigate_up());
}

#[test]
fn navigate_down_past_newest_clears() {
    let mut state = BlindRecallState::default();
    state.replace_cache(vec![entry("a"), entry("b")]);
    let _ = state.navigate_up();
    let _ = state.navigate_up();
    assert_eq!(state.history_cursor(), Some(0));
    assert_eq!(state.navigate_down(), Some(true));
    assert_eq!(state.active_history_text(), Some("b"));
    assert_eq!(state.navigate_down(), Some(false));
    assert_eq!(state.history_cursor(), None);
    assert_eq!(state.last_history_text(), None);
}

#[test]
fn navigate_down_when_not_browsing_is_none() {
    let mut state = BlindRecallState::default();
    state.replace_cache(vec![entry("a")]);
    assert_eq!(state.navigate_down(), None);
}

#[test]
fn push_local_skips_whitespace_only() {
    let mut state = BlindRecallState::default();
    state.push_local_entry("   ");
    assert!(state.cache().is_empty());
    state
        .push_local_entry_with_timestamp_for_test("ok", Some(1))
        .expect("ok should stage a pending persist");
    assert_eq!(state.cache().len(), 1);
}

#[test]
fn push_local_entry_returns_none_on_adjacent_duplicate() {
    let mut state = BlindRecallState::default();
    assert!(
        state
            .push_local_entry_with_timestamp_for_test("one", Some(1))
            .is_some()
    );
    assert_eq!(
        state.push_local_entry_with_timestamp_for_test("one", Some(2)),
        None
    );
}

#[test]
fn push_local_adjacent_dedup_and_trim() {
    let mut state = BlindRecallState::default();
    state.push_local_entry_with_timestamp_for_test("one", Some(1));
    state.push_local_entry_with_timestamp_for_test("one", Some(2));
    assert_eq!(state.cache().len(), 1);

    for i in 0..30 {
        state.push_local_entry_with_timestamp_for_test(&format!("m-{i}"), Some(i64::from(i + 3)));
    }
    assert_eq!(state.cache().len(), 25);
    assert_eq!(state.cache().last().map(|e| e.text.as_str()), Some("m-29"));
}

#[test]
fn push_local_resets_navigation() {
    let mut state = BlindRecallState::default();
    state.replace_cache(vec![entry("x")]);
    let _ = state.navigate_up();
    state
        .push_local_entry_with_timestamp_for_test("fresh", Some(2))
        .expect("fresh should stage a pending persist");
    assert_eq!(state.history_cursor(), None);
    assert_eq!(state.last_history_text(), None);
}

#[test]
fn apply_recalled_text_sets_last_history_and_cursor_when_in_cache() {
    let mut state = BlindRecallState::default();
    state.replace_cache(vec![entry("a"), entry("b")]);
    state.apply_recalled_text("b");
    assert_eq!(state.last_history_text(), Some("b"));
    assert_eq!(state.history_cursor(), Some(1));
    assert!(state.should_handle_navigation("b", 1));
}

#[test]
fn revert_failed_persist_removes_tail_and_resets_navigation() {
    let mut state = BlindRecallState::default();
    let persisted = state
        .push_local_entry_with_timestamp_for_test("saved", Some(1))
        .expect("saved entry should stage a pending persist");
    let _ = state.navigate_up();
    assert!(state.revert_failed_persist(persisted.id));
    assert!(state.cache().is_empty());
    assert_eq!(state.history_cursor(), None);
    assert!(!state.revert_failed_persist(persisted.id));
}

#[test]
fn revert_failed_persist_only_reverts_matching_entry_id() {
    let mut state = BlindRecallState::default();
    let older = state
        .push_local_entry_with_timestamp_for_test("older", Some(10))
        .expect("older entry should stage a pending persist");
    let newer = state
        .push_local_entry_with_timestamp_for_test("newer", Some(11))
        .expect("newer entry should stage a pending persist");

    assert!(state.revert_failed_persist(older.id));
    assert_eq!(cached_texts(&state), vec!["newer".to_string()]);
    assert_eq!(state.pending_entry_id_for_test(), Some(newer.id));
}

#[test]
fn revert_failed_persist_restores_trimmed_entries() {
    let mut state = BlindRecallState::default();
    let seed_entries = (0..25)
        .map(|index| MessageHistoryEntry {
            ts: i64::from(index),
            text: format!("seed-{index}"),
        })
        .collect();
    state.replace_cache(seed_entries);

    let persisted = state
        .push_local_entry_with_timestamp_for_test("fresh", Some(100))
        .expect("fresh entry should stage a pending persist");

    assert_eq!(cached_texts(&state)[0], "seed-1");
    assert_eq!(
        cached_texts(&state).last().map(String::as_str),
        Some("fresh")
    );

    assert!(state.revert_failed_persist(persisted.id));
    assert_eq!(cached_texts(&state)[0], "seed-0");
    assert_eq!(
        cached_texts(&state).last().map(String::as_str),
        Some("seed-24")
    );
    assert_eq!(state.cache().len(), 25);
}

#[test]
fn push_local_entry_without_timestamp_does_not_stage_pending_persist() {
    let mut state = BlindRecallState::default();

    assert_eq!(
        state.push_local_entry_with_timestamp_for_test("draft", None),
        None
    );
    assert_eq!(cached_texts(&state), Vec::<String>::new());
    assert_eq!(state.pending_entry_id_for_test(), None);
}
