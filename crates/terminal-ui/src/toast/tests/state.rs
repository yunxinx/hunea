use super::*;

#[test]
fn render_does_not_advance_entering_transition() {
    let palette = default_palette();
    let notice = ToastNotice::new(ToastSeverity::Info, "Copied");
    let mut state = ToastState::default();
    let mut buffer = Buffer::empty(Rect::new(0, 0, 32, 8));
    let bounds = buffer.area;
    let now = Instant::now();

    show_notice(&mut state, &notice);
    fill_underlay(&mut buffer, "#");
    state.render_at(now, bounds, &mut buffer, palette);
    fill_underlay(&mut buffer, "#");
    state.render_at(now + TOAST_ENTER_DURATION, bounds, &mut buffer, palette);

    assert_eq!(state.next_timeout_deadline(), None);
    assert!(state.is_entering());
}

#[test]
fn advance_completes_entering_transition_with_snappier_duration() {
    let notice = ToastNotice::new(ToastSeverity::Info, "Copied");
    let mut state = ToastState::default();
    let now = Instant::now();

    show_notice(&mut state, &notice);
    state.advance_at(now);
    state.advance_at(now + Duration::from_millis(210));

    assert!(
        state.next_timeout_deadline().is_some(),
        "enter animation should complete at the snappier target duration"
    );
}

#[test]
fn replacement_waits_for_current_exit_before_next_enter() {
    let mut state = ToastState::default();
    let now = Instant::now();
    let bounds = Rect::new(0, 0, 48, 8);
    let palette = default_palette();

    state.show(ToastSeverity::Info, "First notice");
    let exit_started_at = complete_current_enter(&mut state, now, bounds, palette);
    state.show(ToastSeverity::Error, "Second notice");

    assert_eq!(state.active_text(), Some("First notice"));
    assert_eq!(state.pending_text(), Some("Second notice"));
    assert!(state.is_exiting());

    complete_current_exit(&mut state, exit_started_at, bounds, palette);

    assert_eq!(state.active_text(), Some("Second notice"));
    assert_eq!(state.pending_text(), None);
    assert!(state.is_entering());
}

#[test]
fn replacement_keeps_only_latest_pending_notice() {
    let mut state = ToastState::default();
    let now = Instant::now();
    let bounds = Rect::new(0, 0, 48, 8);
    let palette = default_palette();

    state.show(ToastSeverity::Info, "First notice");
    let exit_started_at = complete_current_enter(&mut state, now, bounds, palette);
    state.show(ToastSeverity::Info, "Second notice");
    state.show(ToastSeverity::Error, "Latest notice");

    assert_eq!(state.active_text(), Some("First notice"));
    assert_eq!(state.pending_text(), Some("Latest notice"));

    complete_current_exit(&mut state, exit_started_at, bounds, palette);

    assert_eq!(state.active_text(), Some("Latest notice"));
}

#[test]
fn hold_duration_depends_on_severity_after_enter_completes() {
    let mut state = ToastState::default();
    let now = Instant::now();
    let bounds = Rect::new(0, 0, 32, 8);
    let palette = default_palette();

    state.show(ToastSeverity::Info, "info");
    let info_visible_at = complete_current_enter(&mut state, now, bounds, palette);
    assert_eq!(
        state.next_timeout_deadline(),
        Some(info_visible_at + Duration::from_secs(3))
    );

    let mut state = ToastState::default();
    state.show(ToastSeverity::Error, "error");
    let error_visible_at = complete_current_enter(&mut state, now, bounds, palette);
    assert_eq!(
        state.next_timeout_deadline(),
        Some(error_visible_at + Duration::from_secs(4))
    );
}
