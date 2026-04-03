use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use lumos::frontend::tui::{AppEvent, HeroOptions, Model};
use ratatui::{Terminal, backend::TestBackend, buffer::Buffer};

#[test]
fn ctrl_j_inserts_newline_and_renders_expanded_composer() {
    let mut model = ready_model(20, 12);

    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char('1'))));
    model.update(AppEvent::Key(KeyEvent::new(
        KeyCode::Char('j'),
        KeyModifiers::CONTROL,
    )));

    assert_eq!(model.composer_text(), "1\n");

    let rows = render_rows(&model, 20, 12);
    assert_eq!(rows[rows.len() - 2], "┃ 1                 ");
    assert_eq!(rows[rows.len() - 1], "┃                   ");
}

#[test]
fn backspace_deletes_the_full_combining_grapheme_cluster() {
    let mut model = ready_model(20, 8);

    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char('e'))));
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char('\u{301}'))));
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Backspace)));

    assert_eq!(model.composer_text(), "");
}

#[test]
fn enter_preserves_leading_and_trailing_whitespace() {
    let mut model = ready_model(80, 24);

    for character in [' ', ' ', 'h', 'i', ' ', ' '] {
        model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char(character))));
    }
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Enter)));

    let items = model.transcript_plain_items();
    assert_eq!(items.len(), 2);
    assert_eq!(items[1], ">   hi  ");
}

#[test]
fn long_english_input_wraps_by_word_boundary() {
    let mut model = ready_model(9, 20);

    for character in "hello world".chars() {
        model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char(character))));
    }

    let rows = render_rows(&model, 9, 20);
    assert_eq!(rows[rows.len() - 2], "┃ hello  ");
    assert_eq!(rows[rows.len() - 1], "  world  ");
}

fn ready_model(width: u16, height: u16) -> Model {
    let mut model = Model::new(HeroOptions::default());
    model.update(AppEvent::Resized { width, height });
    model.update(AppEvent::StartupReadyTimeout);
    model
}

fn render_rows(model: &Model, width: u16, height: u16) -> Vec<String> {
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
