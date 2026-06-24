use super::*;

use crate::{AppEffect, AppEvent, runtime::RuntimeEventApply};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use runtime_domain::session::{MessageHistoryEntry, RuntimeEvent};

#[test]
fn ctrl_c_clear_does_not_record_whitespace_only_draft() {
    let mut model = Model::new_with_options(
        StartupBannerOptions::default(),
        ModelOptions {
            ctrl_c_clears_input: true,
            ..ModelOptions::default()
        },
    );
    type_text(&mut model, "   ");
    let effect = model.update(AppEvent::Key(KeyEvent::new(
        KeyCode::Char('c'),
        KeyModifiers::CONTROL,
    )));
    assert_eq!(effect, None);
    assert!(model.composer_text().is_empty());
}

#[test]
fn ctrl_c_clear_records_message_history_when_enabled() {
    let mut model = Model::new_with_options(
        StartupBannerOptions::default(),
        ModelOptions {
            ctrl_c_clears_input: true,
            ..ModelOptions::default()
        },
    );
    type_text(&mut model, "draft to save");
    let effect = model.update(AppEvent::Key(KeyEvent::new(
        KeyCode::Char('c'),
        KeyModifiers::CONTROL,
    )));
    assert_eq!(
        effect,
        Some(AppEffect::RecordMessageHistory {
            text: "draft to save".to_string(),
        })
    );
    assert!(model.composer_text().is_empty());
}

#[test]
fn send_emits_conversation_turn_without_separate_record_effect() {
    let mut model = conversation_test_model();
    type_text(&mut model, "hello history");
    let effect = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Enter)));
    assert!(matches!(
        effect,
        Some(AppEffect::SendConversationTurn { .. })
    ));
    assert!(!matches!(
        effect,
        Some(AppEffect::RecordMessageHistory { .. })
    ));
}

#[test]
fn slash_command_enter_does_not_emit_message_history_effects() {
    let mut model = conversation_test_model();
    type_text(&mut model, "/exit");
    let effect = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Enter)));
    assert!(!matches!(
        effect,
        Some(AppEffect::RecordMessageHistory { .. })
    ));
    assert!(!matches!(
        effect,
        Some(AppEffect::SendConversationTurn { .. })
    ));
}

/// `texts` 为 oldest-first，与 `load_message_history_recent` / 启动缓存语义一致。
fn seed_blind_recall_cache(model: &mut Model, texts_oldest_first: &[&str]) {
    let entries: Vec<MessageHistoryEntry> = texts_oldest_first
        .iter()
        .map(|text| MessageHistoryEntry {
            ts: 1,
            text: (*text).to_string(),
        })
        .collect();
    model.apply_runtime_event(RuntimeEvent::MessageHistoryStartupCacheLoaded { entries });
}

#[test]
fn blind_recall_up_from_empty_recalls_newest() {
    let mut model = conversation_test_model();
    seed_blind_recall_cache(&mut model, &["older", "newer"]);
    assert!(model.composer_text().is_empty());
    let _ = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Up)));
    assert_eq!(model.composer_text(), "newer");
    assert_eq!(model.blind_recall.history_cursor(), Some(1));
}

#[test]
fn blind_recall_repeated_up_walks_older_and_noops_at_oldest() {
    let mut model = conversation_test_model();
    seed_blind_recall_cache(&mut model, &["a", "b", "c"]);
    let _ = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Up)));
    assert_eq!(model.composer_text(), "c");
    let _ = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Up)));
    assert_eq!(model.composer_text(), "b");
    let _ = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Up)));
    assert_eq!(model.composer_text(), "a");
    let _ = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Up)));
    assert_eq!(model.composer_text(), "a");
}

#[test]
fn blind_recall_down_past_newest_clears_composer() {
    let mut model = conversation_test_model();
    seed_blind_recall_cache(&mut model, &["only"]);
    let _ = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Up)));
    assert_eq!(model.composer_text(), "only");
    let _ = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Down)));
    assert!(model.composer_text().is_empty());
    assert_eq!(model.blind_recall.history_cursor(), None);
}

#[test]
fn blind_recall_after_edit_does_not_recall_on_up() {
    let mut model = conversation_test_model();
    seed_blind_recall_cache(&mut model, &["old", "keep"]);
    let _ = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Up)));
    assert_eq!(model.composer_text(), "keep");
    type_text(&mut model, "!");
    assert_eq!(model.composer_text(), "keep!");
    let _ = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Up)));
    assert_eq!(model.composer_text(), "keep!");
}

#[test]
fn blind_recall_empty_history_does_not_recall_on_up() {
    let mut model = conversation_test_model();
    let _ = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Up)));
    assert!(model.composer_text().is_empty());
}

#[test]
fn send_pushes_blind_recall_cache_and_adjacent_dedup() {
    let mut model = conversation_test_model();
    type_text(&mut model, "same");
    let _ = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Enter)));
    assert_eq!(model.blind_recall.cache().len(), 1);
    type_text(&mut model, "same");
    let _ = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Enter)));
    assert_eq!(model.blind_recall.cache().len(), 1);
    type_text(&mut model, "other");
    let _ = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Enter)));
    assert_eq!(model.blind_recall.cache().len(), 2);
    assert_eq!(
        model.blind_recall.cache().last().map(|e| e.text.as_str()),
        Some("other")
    );
}

#[test]
fn message_history_record_failed_reverts_blind_recall_tail() {
    let mut model = conversation_test_model();
    type_text(&mut model, "will fail to persist");
    let _ = model.update(AppEvent::Key(KeyEvent::new(
        KeyCode::Char('c'),
        KeyModifiers::CONTROL,
    )));
    assert_eq!(model.blind_recall.cache().len(), 1);
    model.apply_runtime_event(RuntimeEvent::MessageHistoryRecordFailed {
        text: "will fail to persist".to_string(),
        message: "disk full".to_string(),
    });
    assert!(model.blind_recall.cache().is_empty());
}

#[test]
fn late_startup_cache_load_preserves_locally_recorded_blind_recall_entries() {
    let mut model = conversation_test_model();
    type_text(&mut model, "local send while startup load is pending");
    let _ = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Enter)));

    seed_blind_recall_cache(&mut model, &["persisted older", "persisted newer"]);

    let cached_texts = model
        .blind_recall
        .cache()
        .iter()
        .map(|entry| entry.text.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        cached_texts,
        vec![
            "persisted older",
            "persisted newer",
            "local send while startup load is pending"
        ]
    );
}
