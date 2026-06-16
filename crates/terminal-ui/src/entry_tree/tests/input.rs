use super::*;

#[test]
fn entry_tree_maps_mouse_wheel_to_selection_if_delivered() {
    let mut model = ready_model();
    model.open_entry_tree_loading();
    model.apply_entry_tree_payload(SessionTreePayload {
        rows: (0..4).map(numbered_tree_row).collect(),
        current_row_id: None,
    });

    model.update(AppEvent::MouseWheel { delta_lines: -3 });
    let up_rows = rendered_rows(&render_model_buffer(&mut model, 60, 8));
    assert!(
        up_rows[0].starts_with("  Session Tree (3 of 4)"),
        "mouse wheel up should move selection up by one row: {up_rows:?}"
    );

    model.update(AppEvent::MouseWheel { delta_lines: 3 });
    let down_rows = rendered_rows(&render_model_buffer(&mut model, 60, 8));
    assert!(
        down_rows[0].starts_with("  Session Tree (4 of 4)"),
        "mouse wheel down should move selection down by one row: {down_rows:?}"
    );
}

#[test]
fn entry_tree_captures_mouse_for_click_selection_and_keeps_coalescing() {
    let mut model = ready_model();
    model.open_entry_tree_loading();
    model.apply_entry_tree_payload(SessionTreePayload {
        rows: (0..3).map(numbered_tree_row).collect(),
        current_row_id: None,
    });

    assert_eq!(
        model.mouse_mode_preference(),
        TerminalMouseModePreference::CaptureWithAlternateScroll,
        "tree should capture mouse clicks while keeping alternate-scroll behavior for wheel navigation"
    );
    assert!(
        model
            .terminal_input_coalescing()
            .has_page_scroll_burst_coalescing,
        "tree should coalesce high-frequency wheel bursts like resume picker"
    );
}

#[test]
fn entry_tree_left_click_selects_visible_body_row() {
    let mut model = ready_model();
    model.open_entry_tree_loading();
    model.apply_entry_tree_payload(SessionTreePayload {
        rows: (0..4).map(numbered_tree_row).collect(),
        current_row_id: None,
    });

    let effect = model.update(AppEvent::MouseDown {
        button: MouseButton::Left,
        column: 12,
        row: ENTRY_TREE_HEADER_HEIGHT + ENTRY_TREE_HEADER_RULE_HEIGHT + 1,
    });
    let rows = rendered_rows(&render_model_buffer(&mut model, 60, 8));

    assert_eq!(effect, None);
    assert!(
        rows[0].starts_with("  Session Tree (2 of 4)"),
        "clicking the second visible body row should select the second logical row: {rows:?}"
    );

    model.update(AppEvent::MouseDown {
        button: MouseButton::Left,
        column: 12,
        row: ENTRY_TREE_HEADER_HEIGHT,
    });
    let after_header_click_rows = rendered_rows(&render_model_buffer(&mut model, 60, 8));
    assert!(
        after_header_click_rows[0].starts_with("  Session Tree (2 of 4)"),
        "clicking tree chrome should not move selection: {after_header_click_rows:?}"
    );
}
