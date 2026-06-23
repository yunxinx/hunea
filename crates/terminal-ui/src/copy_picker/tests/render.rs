use super::*;

#[test]
fn copy_picker_empty_payload_shows_empty_body_without_toast() {
    let mut model = Model::new(StartupBannerOptions::default());
    model.set_window(60, 8);
    model.set_palette(default_palette(), true);
    model.open_copy_picker_loading();

    model.apply_copy_picker_payload(SessionTreePayload {
        rows: vec![tree_row(
            "reasoning-only",
            SessionTreeRowKind::Reasoning,
            "hidden chain",
            None,
            Some("reasoning-only"),
        )],
        current_row_id: Some("reasoning-only".to_string()),
    });

    let rows = rendered_rows(&render_model_buffer(&mut model, 60, 8));
    assert!(
        rows.iter().any(|row| row.contains("No user or assi")),
        "empty copy picker should render a muted empty body: {rows:?}"
    );
    assert_eq!(model.active_toast_text_for_test(), None);
}

#[test]
fn copy_picker_error_renders_in_overlay_without_duplicate_toast() {
    let mut model = Model::new(StartupBannerOptions::default());
    model.set_window(60, 8);
    model.set_palette(default_palette(), true);
    model.open_copy_picker_loading();

    model.show_copy_picker_error("Session tree could not be loaded");

    let rows = rendered_rows(&render_model_buffer(&mut model, 60, 8));
    assert!(
        rows.iter()
            .any(|row| row.contains("Session tree could not be")),
        "copy picker should keep actionable load errors in the overlay body: {rows:?}"
    );
    assert_eq!(model.active_toast_text_for_test(), None);
}

#[test]
fn copy_picker_render_shows_tree_chrome_hint_and_selected_marker() {
    let mut model = ready_copy_picker_model();
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Tab)));

    let buffer = render_model_buffer(&mut model, 80, 12);
    let rows = rendered_rows(&buffer);

    assert!(rows[0].starts_with("  Copy Messages (3 of 3)"));
    assert!(rows[10].contains(" Page 1/1 "));
    assert!(rows[11].contains("A: invert selection"));
    assert!(
        rows.iter().any(|row| row.contains("█ user")),
        "selected row marker should be visible in copy list: {rows:?}"
    );
    assert_eq!(buffer[(0, 2)].bg, default_palette().surface.unwrap());
    assert_eq!(buffer[(0, 3)].bg, ratatui::style::Color::Reset);
    assert_eq!(buffer[(14, 4)].symbol(), "s");
    assert_eq!(buffer[(0, 4)].bg, default_palette().surface.unwrap());
    assert_eq!(buffer[(14, 4)].bg, ratatui::style::Color::Reset);
    assert!(
        buffer[(14, 4)]
            .modifier
            .contains(ratatui::style::Modifier::REVERSED),
        "cursor message text should keep the content-area reversed highlight"
    );
    let marker_cell = buffer
        .content()
        .iter()
        .find(|cell| cell.symbol() == "█")
        .expect("selected marker should render");
    assert_eq!(marker_cell.fg, default_palette().approval_rejected);
}
