use ratatui::style::Color;

use crate::{
    Model, StartupBannerOptions,
    test_helpers::{render_model_buffer, rendered_rows},
    theme::default_palette,
};

use super::{ready_picker_model, sample_rows};

#[test]
fn message_history_picker_render_shows_chrome_hint_and_cursor_marker() {
    let mut model = ready_picker_model();
    model.set_palette(default_palette(), true);
    model.set_window(80, 12);

    let buffer = render_model_buffer(&mut model, 80, 12);
    let rows = rendered_rows(&buffer);

    assert!(rows[0].starts_with("  Message history (2 of 2)"));
    assert!(
        rows.last().unwrap().starts_with("  Esc close"),
        "footer should use copy-style leading padding: {:?}",
        rows.last()
    );
    assert!(
        !rows.iter().any(|row| row.contains('█')),
        "selected row should not use block marker: {rows:?}"
    );
    let body_y = rows
        .iter()
        .position(|row| row.contains("newest prompt"))
        .expect("newest row in buffer");
    let reversed_on_row = (0..buffer.area.width).any(|x| {
        buffer[(x, body_y as u16)]
            .modifier
            .contains(ratatui::style::Modifier::REVERSED)
    });
    assert!(
        reversed_on_row,
        "cursor row should use reversed text highlight"
    );
}

#[test]
fn message_history_picker_render_zebra_stripes_and_timestamp_before_text() {
    let mut model = Model::new(StartupBannerOptions::default());
    model.set_palette(default_palette(), true);
    model.set_window(80, 12);
    model.open_message_history_picker_loading_at(10_000);
    model.apply_message_history_picker_rows(sample_rows());

    let buffer = render_model_buffer(&mut model, 80, 12);
    let rows = rendered_rows(&buffer);
    let newest = rows
        .iter()
        .find(|row| row.contains("newest prompt"))
        .expect("newest row");
    let older = rows
        .iter()
        .find(|row| row.contains("older prompt"))
        .expect("older row");
    assert!(
        newest.find("newest prompt").unwrap() > 2,
        "timestamp column should precede message text: {newest}"
    );
    assert!(
        !newest.contains('前'),
        "relative age should not use Chinese suffix: {newest}"
    );
    if let (Some(a), Some(b)) = (newest.find('·'), older.find('·')) {
        assert_eq!(a, b, "middot should align: {newest} vs {older}");
    }
    assert_eq!(buffer[(0, 2)].bg, default_palette().surface.unwrap());
    assert_eq!(buffer[(0, 3)].bg, Color::Reset);
}

#[test]
fn message_history_picker_loading_body_uses_leading_padding() {
    let mut model = Model::new(StartupBannerOptions::default());
    model.set_palette(default_palette(), true);
    model.set_window(60, 8);
    model.open_message_history_picker_loading_at(1);

    let rows = rendered_rows(&render_model_buffer(&mut model, 60, 8));
    assert!(
        rows.iter().any(|row| row.contains("  Loading message")),
        "loading state should match copy picker body padding: {rows:?}"
    );
}
