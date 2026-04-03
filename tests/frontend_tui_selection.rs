use crossterm::event::{KeyCode, KeyEvent, MouseButton};
use lumos::frontend::tui::{AppEffect, AppEvent, HeroOptions, Model, ModelOptions};
use ratatui::{Terminal, backend::TestBackend, buffer::Buffer, style::Modifier};

#[test]
fn drag_selection_highlights_text_and_copies_on_release_when_enabled() {
    let mut model = ready_selection_model(true);
    submit_message(&mut model, "alpha");
    submit_message(&mut model, "beta");

    let (alpha_row, alpha_column) = find_cell_containing(&mut model, 24, 6, "alpha");
    let (beta_row, beta_column) = find_cell_containing(&mut model, 24, 6, "beta");

    assert!(
        model
            .update(AppEvent::MouseDown {
                button: MouseButton::Left,
                column: u16::try_from(alpha_column + 1).unwrap(),
                row: u16::try_from(alpha_row).unwrap(),
            })
            .is_none()
    );
    assert!(
        model
            .update(AppEvent::MouseDrag {
                button: MouseButton::Left,
                column: u16::try_from(beta_column + 2).unwrap(),
                row: u16::try_from(beta_row).unwrap(),
            })
            .is_none()
    );

    let buffer = render_buffer(&mut model, 24, 6);
    assert!(
        buffer[(
            u16::try_from(alpha_column + 1).unwrap(),
            u16::try_from(alpha_row).unwrap()
        )]
            .modifier
            .contains(Modifier::REVERSED)
    );
    assert!(
        buffer[(
            u16::try_from(beta_column + 1).unwrap(),
            u16::try_from(beta_row).unwrap()
        )]
            .modifier
            .contains(Modifier::REVERSED)
    );

    let effect = model.update(AppEvent::MouseUp {
        button: MouseButton::Left,
        column: u16::try_from(beta_column + 2).unwrap(),
        row: u16::try_from(beta_row).unwrap(),
    });

    assert_eq!(
        effect,
        Some(AppEffect::CopySelection("lpha\n› be".to_string()))
    );
}

#[test]
fn double_click_selects_word_and_middle_click_copies_it() {
    let mut model = ready_selection_model(false);
    submit_message(&mut model, "hello world");

    let (row, column) = find_cell_containing(&mut model, 24, 6, "world");
    let column = column + 1;
    let row = u16::try_from(row).unwrap();
    let column = u16::try_from(column).unwrap();

    assert!(
        model
            .update(AppEvent::MouseDown {
                button: MouseButton::Left,
                column,
                row,
            })
            .is_none()
    );
    assert!(
        model
            .update(AppEvent::MouseUp {
                button: MouseButton::Left,
                column,
                row,
            })
            .is_none()
    );
    assert!(
        model
            .update(AppEvent::MouseDown {
                button: MouseButton::Left,
                column,
                row,
            })
            .is_none()
    );

    let effect = model.update(AppEvent::MouseDown {
        button: MouseButton::Middle,
        column: 0,
        row,
    });

    assert_eq!(effect, Some(AppEffect::CopySelection("world".to_string())));
}

fn ready_selection_model(copy_on_release: bool) -> Model {
    let mut model = Model::new_with_options(
        HeroOptions::default(),
        ModelOptions {
            copy_on_mouse_selection_release: copy_on_release,
            ..ModelOptions::default()
        },
    );
    model.update(AppEvent::Resized {
        width: 24,
        height: 6,
    });
    model.update(AppEvent::StartupReadyTimeout);
    model
}

fn submit_message(model: &mut Model, text: &str) {
    for character in text.chars() {
        model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char(character))));
    }
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Enter)));
}

fn find_cell_containing(
    model: &mut Model,
    width: u16,
    height: u16,
    needle: &str,
) -> (usize, usize) {
    let buffer = render_buffer(model, width, height);
    let needle_symbols = needle
        .chars()
        .map(|character| character.to_string())
        .collect::<Vec<_>>();

    for row in 0..buffer.area.height {
        let symbols = (0..buffer.area.width)
            .map(|column| buffer[(column, row)].symbol().to_string())
            .collect::<Vec<_>>();
        for column in 0..=symbols.len().saturating_sub(needle_symbols.len()) {
            if symbols[column..column + needle_symbols.len()] == needle_symbols {
                return (usize::from(row), column);
            }
        }
    }

    panic!(
        "could not find {needle:?} in rendered rows: {:?}",
        render_rows(model, width, height)
    )
}

fn render_rows(model: &mut Model, width: u16, height: u16) -> Vec<String> {
    let buffer = render_buffer(model, width, height);
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

fn render_buffer(model: &mut Model, width: u16, height: u16) -> Buffer {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).expect("test backend should initialize");
    terminal
        .draw(|frame| model.render(frame))
        .expect("model should render on test backend");
    terminal.backend().buffer().clone()
}
