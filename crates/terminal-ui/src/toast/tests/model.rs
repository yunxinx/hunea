use super::*;

#[test]
fn model_advance_uses_tick_time_for_toast_enter_completion() {
    let mut model = Model::new(crate::StartupBannerOptions::default());
    model.set_window(32, 8);
    model.set_palette(default_palette(), true);
    model.show_toast(ToastSeverity::Info, "Copied");
    let area = Rect::new(0, 0, 32, 8);
    let started_at = Instant::now();

    let mut first_frame = Buffer::empty(area);
    model.render_to_buffer_at(started_at, area, &mut first_frame);
    assert_eq!(model.next_timeout_deadline(), None);

    model.advance_toast_at(started_at);
    model.advance_toast_at(started_at + TOAST_ENTER_DURATION);

    assert_eq!(
        model.next_timeout_deadline(),
        Some(started_at + TOAST_ENTER_DURATION + Duration::from_secs(2)),
        "toast visible deadline should be based on the frame time used for rendering"
    );
}
