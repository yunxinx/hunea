use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton};
use ratatui::{buffer::Buffer, layout::Rect};
use terminal_ui::{AppEffect, AppEvent, Model, ModelOptions, StartupBannerOptions};
use unicode_segmentation::UnicodeSegmentation;

#[test]
fn mouse_click_moves_cursor_after_clicked_grapheme() {
    let mut model = ready_model_with_options(
        20,
        8,
        ModelOptions {
            ctrl_c_clears_input: false,
            ..ModelOptions::default()
        },
    );
    type_text(&mut model, "hello");

    let (row, column) = find_cell_containing(&mut model, 20, 8, "e");
    click_left(&mut model, column, row);
    type_text(&mut model, "X");

    assert_eq!(model.composer_text(), "heXllo");
}

#[test]
fn mouse_click_snaps_to_emoji_grapheme_boundary() {
    let mut model = ready_model(20, 8);
    type_text(&mut model, "a👨‍👩‍👧b");

    let (row, column) = find_cell_containing(&mut model, 20, 8, "👨‍👩‍👧");
    click_left(&mut model, column + 1, row);
    type_text(&mut model, "X");

    assert_eq!(model.composer_text(), "a👨‍👩‍👧Xb");
}

#[test]
fn mouse_click_clamps_to_line_start_and_end() {
    let mut model = ready_model(20, 8);
    type_text(&mut model, "hello");

    let (row, _) = find_cell_containing(&mut model, 20, 8, "hello");
    click_left(&mut model, 0, row);
    type_text(&mut model, "X");
    assert_eq!(model.composer_text(), "Xhello");

    let mut model = ready_model(20, 8);
    type_text(&mut model, "hello");

    let (row, _) = find_cell_containing(&mut model, 20, 8, "hello");
    click_left(&mut model, 19, row);
    type_text(&mut model, "X");

    assert_eq!(model.composer_text(), "helloX");
}

#[test]
fn mouse_click_moves_cursor_on_wrapped_continuation_line() {
    let mut model = ready_model(6, 8);
    type_text(&mut model, "abcdef");

    let (row, column) = find_cell_containing(&mut model, 6, 8, "e");
    click_left(&mut model, column, row);
    type_text(&mut model, "X");

    assert_eq!(model.composer_text(), "abcdeXf");
}

#[test]
fn mouse_release_across_gutter_cells_still_moves_cursor_to_line_end() {
    let mut model = ready_model(24, 8);
    type_text(&mut model, "hello world");

    let (row, _) = find_cell_containing(&mut model, 24, 8, "hello world");
    mouse_down_left(&mut model, 23, row);
    mouse_up_left(&mut model, 22, row);
    type_text(&mut model, "X");

    assert_eq!(model.composer_text(), "hello worldX");
}

#[test]
fn mouse_release_across_wide_grapheme_cells_still_moves_cursor() {
    let mut model = ready_model(20, 8);
    type_text(&mut model, "a你b");

    let (row, column) = find_cell_containing(&mut model, 20, 8, "你");
    mouse_down_left(&mut model, column, row);
    mouse_up_left(&mut model, column + 1, row);
    type_text(&mut model, "X");

    assert_eq!(model.composer_text(), "a你Xb");
}

#[test]
fn mouse_release_from_last_grapheme_into_gutter_selects_text() {
    let mut model = ready_model(24, 8);
    type_text(&mut model, "hello");

    let (row, column) = find_cell_containing(&mut model, 24, 8, "o");
    mouse_down_left(&mut model, column, row);
    mouse_up_left(&mut model, column + 2, row);

    let effect = middle_click(&mut model, 0, row);
    assert_eq!(effect, Some(AppEffect::CopySelection("o".to_string())));
}

#[test]
fn mouse_drag_keeps_selection_instead_of_moving_cursor() {
    let mut model = ready_model(24, 8);
    type_text(&mut model, "hello world");

    let (row, start_column) = find_cell_containing(&mut model, 24, 8, "e");
    let (_, end_column) = find_cell_containing(&mut model, 24, 8, "w");
    mouse_down_left(&mut model, start_column, row);
    mouse_drag_left(&mut model, end_column, row);
    mouse_up_left(&mut model, end_column, row);
    type_text(&mut model, "X");

    assert_eq!(model.composer_text(), "hello worldX");
}

#[test]
fn mouse_double_click_selects_word_on_text_hit() {
    let mut model = ready_model(24, 8);
    type_text(&mut model, "hello world");

    let (row, column) = find_cell_containing(&mut model, 24, 8, "world");
    mouse_down_left(&mut model, column, row);
    mouse_up_left(&mut model, column, row);
    mouse_down_left(&mut model, column, row);

    let effect = middle_click(&mut model, 0, row);
    assert_eq!(effect, Some(AppEffect::CopySelection("world".to_string())));
}

#[test]
fn status_notice_clears_pending_composer_cursor_click() {
    let mut model = ready_model_with_options(
        20,
        8,
        ModelOptions {
            ctrl_c_clears_input: false,
            ..ModelOptions::default()
        },
    );
    type_text(&mut model, "hello");

    let (row, column) = find_cell_containing(&mut model, 20, 8, "e");
    mouse_down_left(&mut model, column, row);
    press_key(
        &mut model,
        KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
    );
    mouse_up_left(&mut model, column, row);
    type_text(&mut model, "X");

    assert_eq!(model.composer_text(), "helloX");
}

#[test]
fn window_resize_clears_pending_composer_cursor_click() {
    let mut model = ready_model(20, 8);
    type_text(&mut model, "hello");

    let (row, column) = find_cell_containing(&mut model, 20, 8, "e");
    mouse_down_left(&mut model, column, row);
    model.update(AppEvent::Resized {
        width: 20,
        height: 9,
    });
    mouse_up_left(&mut model, column, row);
    type_text(&mut model, "X");

    assert_eq!(model.composer_text(), "helloX");
}

fn ready_model(width: u16, height: u16) -> Model {
    ready_model_with_options(width, height, ModelOptions::default())
}

fn ready_model_with_options(width: u16, height: u16, options: ModelOptions) -> Model {
    let mut model = Model::new_with_options(StartupBannerOptions::default(), options);
    model.update(AppEvent::Resized { width, height });
    model.update(AppEvent::StartupReadyTimeout);
    model
}

fn type_text(model: &mut Model, text: &str) {
    for character in text.chars() {
        model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char(character))));
    }
}

fn click_left(model: &mut Model, column: usize, row: usize) {
    mouse_down_left(model, column, row);
    mouse_up_left(model, column, row);
}

fn mouse_down_left(model: &mut Model, column: usize, row: usize) {
    let column = u16::try_from(column).expect("test column should fit in u16");
    let row = u16::try_from(row).expect("test row should fit in u16");

    model.update(AppEvent::MouseDown {
        button: MouseButton::Left,
        column,
        row,
    });
}

fn mouse_up_left(model: &mut Model, column: usize, row: usize) {
    let column = u16::try_from(column).expect("test column should fit in u16");
    let row = u16::try_from(row).expect("test row should fit in u16");

    model.update(AppEvent::MouseUp {
        button: MouseButton::Left,
        column,
        row,
    });
}

fn mouse_drag_left(model: &mut Model, column: usize, row: usize) {
    let column = u16::try_from(column).expect("test column should fit in u16");
    let row = u16::try_from(row).expect("test row should fit in u16");

    model.update(AppEvent::MouseDrag {
        button: MouseButton::Left,
        column,
        row,
    });
}

fn middle_click(model: &mut Model, column: usize, row: usize) -> Option<AppEffect> {
    let column = u16::try_from(column).expect("test column should fit in u16");
    let row = u16::try_from(row).expect("test row should fit in u16");

    model.update(AppEvent::MouseDown {
        button: MouseButton::Middle,
        column,
        row,
    })
}

fn press_key(model: &mut Model, key: KeyEvent) {
    model.update(AppEvent::Key(key));
}

fn find_cell_containing(
    model: &mut Model,
    width: u16,
    height: u16,
    needle: &str,
) -> (usize, usize) {
    let buffer = render_buffer(model, width, height);
    let needle_symbols = needle
        .graphemes(true)
        .map(str::to_string)
        .collect::<Vec<_>>();

    for row in (0..buffer.area.height).rev() {
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
    );
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
    let area = Rect::new(0, 0, width, height);
    let mut buffer = Buffer::empty(area);
    let _ = model.render_to_buffer(area, &mut buffer);
    buffer
}
