use crate::{
    AppEffect, AppEvent, Model, StartupBannerOptions,
    overlay_input_result::OverlayInputResult,
    runtime::RuntimeEventApply,
    test_helpers::{
        render_model_buffer, rendered_rows, tree_row, tree_row_with_preview_replay_items,
    },
    theme::default_palette,
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton};
use runtime_domain::session::{
    RuntimeEvent, SessionTreePayload, SessionTreeRowKind, TranscriptReplayItem,
    TranscriptReplayRole,
};

mod input;
mod preview;
mod render;
mod runtime;
mod selection;

fn ready_copy_picker_model() -> Model {
    let mut model = Model::new(StartupBannerOptions::default());
    model.set_window(80, 12);
    model.set_palette(default_palette(), true);
    model.open_copy_picker_loading();
    model.apply_copy_picker_payload(SessionTreePayload {
        rows: vec![
            tree_row(
                "user-1",
                SessionTreeRowKind::User,
                "first user",
                Some("first user".to_string()),
                Some("user-1"),
            ),
            tree_row(
                "reasoning-1",
                SessionTreeRowKind::Reasoning,
                "hidden chain",
                None,
                Some("reasoning-1"),
            ),
            tree_row_with_preview_replay_items(
                "assistant-1",
                SessionTreeRowKind::Assistant,
                "assistant raw",
                vec![TranscriptReplayItem::Message {
                    role: TranscriptReplayRole::Assistant,
                    content: "assistant display\n\nTool call `read_file` (call-1)".to_string(),
                }],
            ),
            tree_row(
                "user-2",
                SessionTreeRowKind::User,
                "second user",
                Some("second user".to_string()),
                Some("user-2"),
            ),
        ],
        current_row_id: Some("user-2".to_string()),
    });
    model
}

fn type_text(model: &mut Model, text: &str) {
    for ch in text.chars() {
        model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char(ch))));
    }
}
