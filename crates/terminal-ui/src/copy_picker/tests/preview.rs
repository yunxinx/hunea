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
fn copy_picker_preview_transcript_tracks_resize_after_opening() {
    let mut model = Model::new(StartupBannerOptions::default());
    model.set_window(80, 12);
    model.set_palette(default_palette(), true);
    model.open_copy_picker_loading();
    model.apply_copy_picker_payload(SessionTreePayload {
        rows: vec![tree_row_with_preview_replay_items(
            "assistant-long",
            SessionTreeRowKind::Assistant,
            "assistant raw",
            vec![TranscriptReplayItem::Message {
                role: TranscriptReplayRole::Assistant,
                content: "alpha beta gamma delta epsilon zeta eta theta iota kappa lambda mu"
                    .to_string(),
            }],
        )],
        current_row_id: Some("assistant-long".to_string()),
    });
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char(' '))));
    assert!(model.copy_picker_preview_active());

    let wide_line_count = model
        .copy_picker
        .as_mut()
        .and_then(|state| state.preview.as_mut())
        .map(|preview| {
            preview
                .transcript_preview
                .transcript
                .progressive_item_metrics_index()
                .line_count
        })
        .expect("copy picker preview should be open");

    model.update(AppEvent::Resized {
        width: 18,
        height: 12,
    });

    let narrow_line_count = model
        .copy_picker
        .as_mut()
        .and_then(|state| state.preview.as_mut())
        .map(|preview| {
            preview
                .transcript_preview
                .transcript
                .progressive_item_metrics_index()
                .line_count
        })
        .expect("copy picker preview should stay open after resize");
    assert!(
        narrow_line_count > wide_line_count,
        "open copy picker preview should rewrap after resize: wide={wide_line_count}, narrow={narrow_line_count}"
    );
}

#[test]
fn copy_picker_preview_transcript_tracks_palette_after_opening() {
    let mut model = Model::new(StartupBannerOptions::default());
    model.set_window(80, 12);
    model.set_palette(default_palette(), true);
    model.open_copy_picker_loading();
    model.apply_copy_picker_payload(SessionTreePayload {
        rows: vec![tree_row_with_preview_replay_items(
            "user-message",
            SessionTreeRowKind::User,
            "user raw",
            vec![TranscriptReplayItem::Message {
                role: TranscriptReplayRole::User,
                content: "surface-backed user message".to_string(),
            }],
        )],
        current_row_id: Some("user-message".to_string()),
    });
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char(' '))));
    assert!(model.copy_picker_preview_active());

    let surface_line_count = model
        .copy_picker
        .as_mut()
        .and_then(|state| state.preview.as_mut())
        .map(|preview| {
            preview
                .transcript_preview
                .transcript
                .progressive_item_metrics_index()
                .line_count
        })
        .expect("copy picker preview should be open");

    model.set_palette(terminal_default_palette(), false);

    let terminal_default_line_count = model
        .copy_picker
        .as_mut()
        .and_then(|state| state.preview.as_mut())
        .map(|preview| {
            preview
                .transcript_preview
                .transcript
                .progressive_item_metrics_index()
                .line_count
        })
        .expect("copy picker preview should stay open after palette change");
    assert!(
        terminal_default_line_count < surface_line_count,
        "open copy picker preview should refresh user-message surface metrics after palette change: surface={surface_line_count}, terminal_default={terminal_default_line_count}"
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
