use crossterm::event::MouseButton;
use ratatui::{Terminal, backend::TestBackend, buffer::Buffer};
use terminal_ui::{AppEvent, Model, StartupBannerOptions, theme::default_palette};

#[test]
fn manual_scrollback_renders_history_progress_hint() {
    let mut model = ready_model(24, 4);
    for message in ["a", "b", "c", "d", "e", "f", "g", "h"] {
        submit_message(&mut model, message);
    }

    model.update(AppEvent::MouseWheel { delta_lines: -3 });

    let rows = render_trimmed_rows(&mut model, 24, 4);
    assert!(
        rows.iter().any(|row| row.contains('%')),
        "manual scrollback should show a transient progress hint: {rows:?}"
    );
}

#[test]
fn clicking_history_progress_hint_hides_it() {
    let mut model = ready_model(24, 4);
    for message in ["a", "b", "c", "d", "e", "f", "g", "h"] {
        submit_message(&mut model, message);
    }

    model.update(AppEvent::MouseWheel { delta_lines: -3 });
    let before_rows = render_trimmed_rows(&mut model, 24, 4);
    let (row, column) = find_cell_containing(&before_rows, "%");

    model.update(AppEvent::MouseDown {
        button: MouseButton::Left,
        column: u16::try_from(column).unwrap(),
        row: u16::try_from(row).unwrap(),
    });

    let rows = render_trimmed_rows(&mut model, 24, 4);
    assert!(
        rows.iter().all(|current| !current.contains('%')),
        "clicking the visible hint should dismiss it: {rows:?}"
    );
}

fn ready_model(width: u16, height: u16) -> Model {
    let mut model = Model::new(StartupBannerOptions::default());
    model.update(AppEvent::Resized { width, height });
    model.update(AppEvent::DetectedPalette {
        palette: default_palette(),
        has_dark_background: true,
    });
    model
}

fn submit_message(model: &mut Model, text: &str) {
    for character in text.chars() {
        model.update(AppEvent::Key(
            crossterm::event::KeyCode::Char(character).into(),
        ));
    }
    model.update(AppEvent::Key(crossterm::event::KeyCode::Enter.into()));
}

fn render_trimmed_rows(model: &mut Model, width: u16, height: u16) -> Vec<String> {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).expect("test backend should initialize");
    terminal
        .draw(|frame| model.render(frame))
        .expect("model should render on test backend");

    trim_rows(terminal.backend().buffer())
}

fn trim_rows(buffer: &Buffer) -> Vec<String> {
    let mut rows = Vec::with_capacity(buffer.area.height as usize);

    for row in 0..buffer.area.height {
        let mut rendered = String::new();
        for column in 0..buffer.area.width {
            rendered.push_str(buffer[(column, row)].symbol());
        }
        rows.push(rendered.trim_end().to_string());
    }

    while rows.last().is_some_and(String::is_empty) {
        rows.pop();
    }

    rows
}

fn find_cell_containing(rows: &[String], needle: &str) -> (usize, usize) {
    for (row_index, row) in rows.iter().enumerate() {
        if let Some(column) = row.find(needle) {
            return (row_index, column);
        }
    }

    panic!("could not find {needle:?} in rows: {rows:?}");
}
