use crossterm::event::{KeyCode, KeyEvent};
use runtime_domain::session::{SessionTreeEntry, SessionTreeEntryKind, SessionTreePayload};

use crate::{AppEffect, AppEvent, Model, StartupBannerOptions, theme::default_palette};

#[test]
fn entry_tree_filters_and_selects_entry_with_prefill() {
    let mut model = ready_model();
    model.open_entry_tree_loading();
    model.apply_entry_tree_payload(SessionTreePayload {
        entries: vec![
            tree_entry(
                "assistant-a",
                SessionTreeEntryKind::Assistant,
                "alpha answer",
                None,
            ),
            tree_entry(
                "user-b",
                SessionTreeEntryKind::User,
                "beta question",
                Some("beta question".to_string()),
            ),
        ],
    });

    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char('b'))));
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char('e'))));

    let effect = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Enter)));

    assert_eq!(
        effect,
        Some(AppEffect::SelectEntryRewind {
            entry_id: "user-b".to_string(),
            prefill: Some("beta question".to_string()),
        })
    );
    assert!(!model.entry_tree_active());
}

#[test]
fn entry_tree_esc_closes_without_effect() {
    let mut model = ready_model();
    model.open_entry_tree_loading();
    model.apply_entry_tree_payload(SessionTreePayload {
        entries: vec![tree_entry(
            "assistant-a",
            SessionTreeEntryKind::Assistant,
            "alpha answer",
            None,
        )],
    });

    let effect = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Esc)));

    assert_eq!(effect, None);
    assert!(!model.entry_tree_active());
}

fn ready_model() -> Model {
    let mut model = Model::new(StartupBannerOptions::default());
    model.set_window(80, 20);
    model.set_palette(default_palette(), true);
    model
}

fn tree_entry(
    entry_id: &str,
    kind: SessionTreeEntryKind,
    content: &str,
    rewind_prefill: Option<String>,
) -> SessionTreeEntry {
    SessionTreeEntry {
        entry_id: entry_id.to_string(),
        parent_id: Some("header".to_string()),
        depth: 1,
        kind,
        label: content.to_string(),
        content: content.to_string(),
        rewind_target_id: Some(entry_id.to_string()),
        rewind_prefill,
        is_active_path: false,
        is_current_leaf: false,
    }
}
