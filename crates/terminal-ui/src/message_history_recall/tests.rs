use session_store::MessageHistoryEntry;

use super::startup_cache_from_recent;
use super::state::{BlindRecallNavigateResult, BlindRecallState};

fn entry(text: &str) -> MessageHistoryEntry {
    MessageHistoryEntry {
        ts: 1,
        text: text.to_string(),
    }
}

#[test]
fn startup_cache_reverses_recent_to_oldest_first() {
    let recent = vec![entry("newest"), entry("middle"), entry("oldest")];
    let cache = startup_cache_from_recent(recent);
    assert_eq!(
        cache.iter().map(|e| e.text.as_str()).collect::<Vec<_>>(),
        vec!["oldest", "middle", "newest"]
    );
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
    assert_eq!(
        state.navigate_up(),
        BlindRecallNavigateResult::ApplyText("new".to_string())
    );
    assert_eq!(state.history_cursor(), Some(1));
}

#[test]
fn navigate_up_at_oldest_is_noop() {
    let mut state = BlindRecallState::default();
    state.replace_cache(vec![entry("only")]);
    assert_eq!(
        state.navigate_up(),
        BlindRecallNavigateResult::ApplyText("only".to_string())
    );
    assert_eq!(state.navigate_up(), BlindRecallNavigateResult::NoOp);
}

#[test]
fn navigate_down_past_newest_clears() {
    let mut state = BlindRecallState::default();
    state.replace_cache(vec![entry("a"), entry("b")]);
    let _ = state.navigate_up();
    let _ = state.navigate_up();
    assert_eq!(state.history_cursor(), Some(0));
    assert_eq!(
        state.navigate_down(),
        Some(BlindRecallNavigateResult::ApplyText("b".to_string()))
    );
    assert_eq!(
        state.navigate_down(),
        Some(BlindRecallNavigateResult::ApplyText(String::new()))
    );
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
fn push_local_adjacent_dedup_and_trim() {
    let mut state = BlindRecallState::default();
    state.push_local_entry("one".to_string());
    state.push_local_entry("one".to_string());
    assert_eq!(state.cache().len(), 1);

    for i in 0..30 {
        state.push_local_entry(format!("m-{i}"));
    }
    assert_eq!(state.cache().len(), 25);
    assert_eq!(state.cache().last().map(|e| e.text.as_str()), Some("m-29"));
}

#[test]
fn push_local_resets_navigation() {
    let mut state = BlindRecallState::default();
    state.replace_cache(vec![entry("x")]);
    let _ = state.navigate_up();
    state.push_local_entry("fresh".to_string());
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
