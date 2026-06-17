use super::*;

#[test]
fn copy_picker_preview_copy_uses_previewed_message_only() {
    let mut model = ready_copy_picker_model();

    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Tab)));
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Up)));
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char(' '))));

    let effect = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char('c'))));

    assert_eq!(
        effect,
        Some(AppEffect::CopySelection(
            "assistant display\n\nTool call `read_file` (call-1)".to_string()
        ))
    );
}

#[test]
fn copy_picker_preview_render_does_not_change_scroll_offset() {
    let mut model = Model::new(StartupBannerOptions::default());
    model.set_window(60, 8);
    model.set_palette(default_palette(), true);
    model.open_copy_picker_loading();
    model.apply_copy_picker_payload(SessionTreePayload {
        rows: vec![tree_row_with_preview_replay_items(
            "assistant-long",
            SessionTreeRowKind::Assistant,
            "assistant raw",
            vec![TranscriptReplayItem::Message {
                role: TranscriptReplayRole::Assistant,
                content: (0..20)
                    .map(|index| format!("preview line {index}"))
                    .collect::<Vec<_>>()
                    .join("\n"),
            }],
        )],
        current_row_id: Some("assistant-long".to_string()),
    });
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char(' '))));
    assert!(model.copy_picker_preview_active());

    let preview = model
        .copy_picker
        .as_mut()
        .and_then(|state| state.preview.as_mut())
        .expect("copy picker preview should be open");
    preview.transcript_preview.is_following_bottom = true;
    preview.transcript_preview.overlay.scroll_offset = 0;

    let _ = render_model_buffer(&mut model, 60, 8);

    let scroll_offset = model
        .copy_picker
        .as_ref()
        .and_then(|state| state.preview.as_ref())
        .map(|preview| preview.transcript_preview.overlay.scroll_offset);
    assert_eq!(
        scroll_offset,
        Some(0),
        "rendering must not repair or advance preview scroll state"
    );
}

#[test]
fn copy_picker_preview_shift_c_copies_previewed_raw_text() {
    let mut model = ready_copy_picker_model();

    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Up)));
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char(' '))));

    let effect = model.update(AppEvent::Key(KeyEvent::new(
        KeyCode::Char('C'),
        KeyModifiers::SHIFT,
    )));

    assert_eq!(
        effect,
        Some(AppEffect::CopySelection("assistant raw".to_string()))
    );
}

#[test]
fn copy_picker_escape_closes_preview_before_overlay() {
    let mut model = ready_copy_picker_model();

    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char(' '))));
    assert!(model.copy_picker_preview_active());

    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Esc)));
    assert!(model.copy_picker_active());
    assert!(!model.copy_picker_preview_active());

    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Esc)));
    assert!(!model.copy_picker_active());
}
