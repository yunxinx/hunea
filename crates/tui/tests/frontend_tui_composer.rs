use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use mo_tui::{AppEvent, HeroOptions, Model};
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

    let rows = render_rows(&mut model, 20, 12);
    let first_line = rows
        .iter()
        .position(|row| row == "› 1                 ")
        .expect("document should contain the first draft line");
    assert_eq!(rows[first_line + 1], "                    ");
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
    assert_eq!(items[1], "›   hi  ");
}

#[test]
fn swap_enter_and_send_makes_enter_insert_newline() {
    let mut model = ready_model_with_swap_enter_and_send(20, 12, true);

    for character in "hello".chars() {
        model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char(character))));
    }
    let before_items = model.transcript_plain_items();
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Enter)));

    assert_eq!(model.composer_text(), "hello\n");
    assert_eq!(model.transcript_plain_items(), before_items);
}

#[test]
fn swap_enter_and_send_makes_ctrl_j_send_message() {
    let mut model = ready_model_with_swap_enter_and_send(20, 12, true);

    for character in "hello".chars() {
        model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char(character))));
    }
    let before_len = model.transcript_plain_items().len();
    model.update(AppEvent::Key(KeyEvent::new(
        KeyCode::Char('j'),
        KeyModifiers::CONTROL,
    )));

    let items = model.transcript_plain_items();
    assert_eq!(items.len(), before_len + 1);
    assert_eq!(items.last().map(String::as_str), Some("› hello"));
    assert_eq!(model.composer_text(), "");
}

#[test]
fn swap_enter_and_send_makes_shift_enter_send_message() {
    let mut model = ready_model_with_swap_enter_and_send(20, 12, true);

    for character in "hello".chars() {
        model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char(character))));
    }
    let before_len = model.transcript_plain_items().len();
    model.update(AppEvent::Key(KeyEvent::new(
        KeyCode::Enter,
        KeyModifiers::SHIFT,
    )));

    let items = model.transcript_plain_items();
    assert_eq!(items.len(), before_len + 1);
    assert_eq!(items.last().map(String::as_str), Some("› hello"));
    assert_eq!(model.composer_text(), "");
}

#[test]
fn long_english_input_wraps_by_word_boundary() {
    let mut model = ready_model(9, 20);

    for character in "hello world".chars() {
        model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char(character))));
    }

    let rows = render_rows(&mut model, 9, 20);
    let first_line = rows
        .iter()
        .position(|row| row == "› hello  ")
        .expect("document should contain the wrapped first line");
    assert_eq!(rows[first_line + 1], "  world  ");
}

#[test]
fn paste_event_inserts_multiline_text_without_sending_message() {
    let mut model = ready_model(20, 12);
    let before_items = model.transcript_plain_items();

    model.update(AppEvent::Paste("alpha\nbeta\ngamma".to_string()));

    assert_eq!(model.composer_text(), "alpha\nbeta\ngamma");
    assert_eq!(model.transcript_plain_items(), before_items);
}

#[test]
fn paste_event_normalizes_crlf_into_composer_newlines() {
    let mut model = ready_model(20, 12);

    model.update(AppEvent::Paste("alpha\r\nbeta\r\ngamma".to_string()));

    assert_eq!(model.composer_text(), "alpha\nbeta\ngamma");
}

fn ready_model(width: u16, height: u16) -> Model {
    ready_model_with_swap_enter_and_send(width, height, false)
}

fn ready_model_with_swap_enter_and_send(
    width: u16,
    height: u16,
    swap_enter_and_send: bool,
) -> Model {
    let mut model = Model::new_with_options(
        HeroOptions::default(),
        mo_tui::ModelOptions {
            swap_enter_and_send,
            ..mo_tui::ModelOptions::default()
        },
    );
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
