use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{buffer::Buffer, layout::Rect};
use terminal_ui::{AppEvent, Model, StartupBannerOptions};

#[test]
fn overflowing_document_bottom_slice_keeps_the_full_draft_visible() {
    let mut model = ready_model(20, 4);

    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char('1'))));
    model.update(AppEvent::Key(KeyEvent::new(
        KeyCode::Char('j'),
        KeyModifiers::CONTROL,
    )));
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char('2'))));
    model.update(AppEvent::Key(KeyEvent::new(
        KeyCode::Char('j'),
        KeyModifiers::CONTROL,
    )));
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char('3'))));

    let rows = render_rows(&mut model, 20, 4);
    assert_eq!(
        rows,
        vec![
            "                    ",
            "› 1                 ",
            "  2                 ",
            "  3                 ",
        ]
    );
}

fn ready_model(width: u16, height: u16) -> Model {
    let mut model = Model::new(StartupBannerOptions::default());
    model.update(AppEvent::Resized { width, height });
    model.update(AppEvent::StartupReadyTimeout);
    model
}

fn render_rows(model: &mut Model, width: u16, height: u16) -> Vec<String> {
    let area = Rect::new(0, 0, width, height);
    let mut buffer = Buffer::empty(area);
    let _ = model.render_to_buffer(area, &mut buffer);

    buffer_rows(&buffer)
}

fn buffer_rows(buffer: &Buffer) -> Vec<String> {
    let mut rows = Vec::with_capacity(buffer.area.height as usize);

    for row in 0..buffer.area.height {
        let mut rendered = String::new();
        for column in 0..buffer.area.width {
            rendered.push_str(buffer[(column, row)].symbol());
        }
        rows.push(rendered);
    }

    rows
}
