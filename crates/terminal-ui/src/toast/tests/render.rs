use super::*;

#[test]
fn static_render_uses_rounded_border_without_padding_or_outer_margin() {
    let palette = default_palette();
    let notice = ToastNotice::new(ToastSeverity::Error, "Copied 中");
    let mut buffer = Buffer::empty(Rect::new(0, 0, 32, 8));
    fill_underlay(&mut buffer, "#");
    let area = toast_rect(buffer.area, &notice.text).expect("toast should fit");

    render_toast_notice(&notice, area, &mut buffer, palette);

    assert_eq!(area.y, 0);
    assert_eq!(area.right(), buffer.area.right());
    assert_eq!(area.height, 3);
    assert_eq!(buffer[(area.x, area.y)].symbol(), "╭");
    assert_eq!(buffer[(area.right() - 1, area.y)].symbol(), "╮");
    assert_eq!(buffer[(area.x, area.bottom() - 1)].symbol(), "╰");
    assert_eq!(buffer[(area.right() - 1, area.bottom() - 1)].symbol(), "╯");
    assert_eq!(buffer[(area.x + 1, area.y + 1)].symbol(), "C");
    assert_eq!(buffer[(area.x, area.y)].fg, palette.system_error);
    assert_eq!(buffer[(area.x - 1, area.y)].symbol(), "#");
}

#[test]
fn severity_controls_distinct_border_colors() {
    let palette = default_palette();

    let border_color = |severity| {
        let notice = ToastNotice::new(severity, "notice");
        let mut buffer = Buffer::empty(Rect::new(0, 0, 24, 8));
        let area = toast_rect(buffer.area, &notice.text).expect("toast should fit");
        render_toast_notice(&notice, area, &mut buffer, palette);
        buffer[(area.x, area.y)].fg
    };

    assert_eq!(border_color(ToastSeverity::Info), palette.accent);
    assert_eq!(border_color(ToastSeverity::Error), palette.system_error);
}

#[test]
fn final_exit_frame_restores_underlying_cells() {
    let palette = default_palette();
    let notice = ToastNotice::new(ToastSeverity::Info, "Copied");
    let mut state = ToastState::default();
    let mut buffer = Buffer::empty(Rect::new(0, 0, 32, 8));
    let bounds = buffer.area;
    let toast_area = toast_rect(bounds, &notice.text).expect("toast should fit");
    let restored_area = toast_area;
    let now = Instant::now();

    let exit_started_at = start_notice_exit(&mut state, &notice, now, bounds, palette);
    fill_underlay(&mut buffer, "#");
    state.render_at(exit_started_at, bounds, &mut buffer, palette);
    fill_underlay(&mut buffer, "#");
    let completed_at = exit_started_at + TOAST_EXIT_DURATION;
    state.advance_at(completed_at);
    state.render_at(completed_at, bounds, &mut buffer, palette);

    assert_eq!(state.active_text(), None);
    assert_rect_symbols(&buffer, restored_area, "#");
}
