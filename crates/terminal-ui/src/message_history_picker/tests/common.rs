use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use runtime_domain::session::MessageHistoryRow;

use crate::{AppEvent, Model, StartupBannerOptions};

pub(super) fn ctrl_r() -> KeyEvent {
    KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL)
}

pub(super) fn type_text(model: &mut Model, text: &str) {
    for ch in text.chars() {
        model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char(ch))));
    }
}

pub(super) fn sample_rows() -> Vec<MessageHistoryRow> {
    vec![
        MessageHistoryRow {
            id: 1,
            ts: 1_000,
            text: "older prompt".to_string(),
        },
        MessageHistoryRow {
            id: 2,
            ts: 2_000,
            text: "newest prompt".to_string(),
        },
    ]
}

pub(super) fn diverse_rows() -> Vec<MessageHistoryRow> {
    vec![
        MessageHistoryRow {
            id: 1,
            ts: 1_000,
            text: "git status".to_string(),
        },
        MessageHistoryRow {
            id: 2,
            ts: 2_000,
            text: "cargo test".to_string(),
        },
        MessageHistoryRow {
            id: 3,
            ts: 3_000,
            text: "GIT diff".to_string(),
        },
    ]
}

pub(super) fn selection_stability_rows() -> Vec<MessageHistoryRow> {
    vec![
        MessageHistoryRow {
            id: 10,
            ts: 1_000,
            text: "unmatched first".to_string(),
        },
        MessageHistoryRow {
            id: 20,
            ts: 2_000,
            text: "target one".to_string(),
        },
        MessageHistoryRow {
            id: 30,
            ts: 3_000,
            text: "target two".to_string(),
        },
    ]
}

pub(super) fn ready_picker_model() -> Model {
    let mut model = Model::new(StartupBannerOptions::default());
    model.set_window(80, 24);
    let request_id = model.open_message_history_picker_loading_at(10_000);
    model.apply_message_history_picker_rows(request_id, sample_rows());
    model
}

pub(super) fn long_message_for_copy() -> Vec<MessageHistoryRow> {
    vec![MessageHistoryRow {
        id: 1,
        ts: 1_000,
        text: "short in list but this is the full message body for clipboard".to_string(),
    }]
}
