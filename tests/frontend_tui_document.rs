use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use lumos::frontend::tui::{AppEvent, HeroOptions, Model};
use ratatui::{Terminal, backend::TestBackend, buffer::Buffer};

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
            "┃ 1                 ",
            "┃ 2                 ",
            "┃ 3                 ",
        ]
    );
}

fn ready_model(width: u16, height: u16) -> Model {
    let mut model = Model::new(HeroOptions::default());
    model.update(AppEvent::Resized { width, height });
    model.update(AppEvent::StartupReadyTimeout);
    model
}

fn render_rows(model: &mut Model, width: u16, height: u16) -> Vec<String> {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).expect("test backend should initialize");
    terminal
        .draw(|frame| model.render(frame))
        .expect("model should render on test backend");

    buffer_rows(terminal.backend().buffer())
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
