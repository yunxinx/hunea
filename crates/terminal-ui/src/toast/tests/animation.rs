use super::*;

#[test]
fn entering_transition_erases_space_without_surface_fill() {
    let mut palette = default_palette();
    palette.surface = Some(Color::Blue);
    let notice = ToastNotice::new(ToastSeverity::Info, "Copied");
    let mut state = ToastState::default();
    let mut buffer = Buffer::empty(Rect::new(0, 0, 32, 8));
    let bounds = buffer.area;
    let toast_area = toast_rect(bounds, &notice.text).expect("toast should fit");
    let now = Instant::now();

    show_notice(&mut state, &notice);
    state.advance_at(now);
    fill_underlay(&mut buffer, "#");
    state.render_at(now, bounds, &mut buffer, palette);

    assert_reset_blank_cell(&buffer, toast_area.right() - 1, toast_area.y);
    assert_ne!(
        buffer[(toast_area.right() - 1, toast_area.y)].bg,
        palette.surface.unwrap(),
        "transition should erase cells instead of painting with the theme surface"
    );
}

#[test]
fn exiting_transition_reveals_underlay_from_right_without_surface_fill() {
    let mut palette = default_palette();
    palette.surface = Some(Color::Blue);
    let notice = ToastNotice::new(ToastSeverity::Info, "Copied");
    let mut state = ToastState::default();
    let mut buffer = Buffer::empty(Rect::new(0, 0, 32, 8));
    let bounds = buffer.area;
    let toast_area = toast_rect(bounds, &notice.text).expect("toast should fit");
    let now = Instant::now();

    let exit_started_at = start_notice_exit(&mut state, &notice, now, bounds, palette);
    fill_underlay(&mut buffer, "#");
    state.render_at(exit_started_at, bounds, &mut buffer, palette);

    assert_eq!(
        buffer[(toast_area.right() - 1, toast_area.y)].symbol(),
        "#",
        "exit should reveal the underlay as each column disappears"
    );
    assert_ne!(
        buffer[(toast_area.right() - 1, toast_area.y)].bg,
        palette.surface.unwrap(),
        "transition should reveal underlay instead of painting with the theme surface"
    );
    assert_eq!(
        buffer[(toast_area.x, toast_area.y)].symbol(),
        "╭",
        "exit should erase from right to left without moving the toast content"
    );
}

#[test]
fn entering_transition_uses_narrow_erase_edge() {
    let palette = default_palette();
    let notice = ToastNotice::new(ToastSeverity::Info, "Copied to clipboard");
    let mut state = ToastState::default();
    let mut buffer = Buffer::empty(Rect::new(0, 0, 48, 8));
    let bounds = buffer.area;
    let toast_area = toast_rect(bounds, &notice.text).expect("toast should fit");
    let now = Instant::now();

    show_notice(&mut state, &notice);
    state.advance_at(now);
    fill_underlay(&mut buffer, "#");
    state.render_at(now, bounds, &mut buffer, palette);

    let erased_columns = count_reset_blank_columns_on_row(&buffer, toast_area, toast_area.y);
    assert!(
        (1..=2).contains(&erased_columns),
        "first enter frame should use a narrow erase edge, got {erased_columns} columns"
    );
}

#[test]
fn entering_transition_moves_content_from_the_right() {
    let palette = default_palette();
    let notice = ToastNotice::new(ToastSeverity::Info, "Copied to clipboard");
    let mut state = ToastState::default();
    let mut buffer = Buffer::empty(Rect::new(0, 0, 48, 8));
    let bounds = buffer.area;
    let toast_area = toast_rect(bounds, &notice.text).expect("toast should fit");
    let now = Instant::now();

    show_notice(&mut state, &notice);
    state.advance_at(now);
    fill_underlay(&mut buffer, "#");
    state.render_at(now, bounds, &mut buffer, palette);

    fill_underlay(&mut buffer, "#");
    state.render_at(now + TOAST_ENTER_DURATION / 2, bounds, &mut buffer, palette);

    assert_ne!(
        buffer[(toast_area.x, toast_area.y)].symbol(),
        "╭",
        "mid-enter frame should not draw the toast at its final left edge"
    );
    let shifted_left_corner = find_symbol_on_row(&buffer, toast_area, toast_area.y, "╭")
        .expect("mid-enter frame should draw the moving toast left edge");
    assert!(
        shifted_left_corner > toast_area.x,
        "mid-enter frame should shift content right from the final rect"
    );

    fill_underlay(&mut buffer, "#");
    state.render_at(now + TOAST_ENTER_DURATION, bounds, &mut buffer, palette);

    assert_eq!(buffer[(toast_area.x, toast_area.y)].symbol(), "╭");
}
