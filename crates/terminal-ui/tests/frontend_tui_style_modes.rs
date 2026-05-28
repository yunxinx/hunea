use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{Terminal, backend::TestBackend, buffer::Buffer};
use terminal_ui::{AppEvent, Model, StartupBannerOptions, StyleMode, theme::default_palette};

#[test]
fn cx_style_mode_frames_user_messages_in_terminal_replay() {
    let model = submitted_model(StyleMode::Cx, "hello");
    let items = model.terminal_replay_items(false);

    assert!(
        items.len() >= 2,
        "expected startup banner and submitted user message, got {:?}",
        items
    );
    assert_eq!(trim_right_per_line(&items[1]), "\n› hello\n");
}

#[test]
fn cc_style_mode_uses_rule_lines_for_the_empty_composer() {
    let mut model = ready_model(StyleMode::Cc, 40, 8);

    assert_contains_sequence(
        &trimmed_rows(&mut model, 40, 8),
        &[
            "────────────────────────────────────────",
            "❯ Enter to send Prompt",
            "────────────────────────────────────────",
        ],
    );
}

#[test]
fn cx_style_mode_uses_half_height_surface_rows_for_the_composer() {
    let mut model = ready_model(StyleMode::Cx, 40, 8);
    let buffer = render_buffer(&mut model, 40, 8);
    let rows = buffer_rows(&buffer);
    let top = "▄".repeat(40);
    let bottom = "▀".repeat(40);
    let top_index = rows
        .iter()
        .position(|row| row == &top)
        .expect("cx composer should render a lower-half surface row above input");

    assert_eq!(rows[top_index + 1].trim_end(), "› Enter to send Prompt");
    assert_eq!(rows[top_index + 2], bottom);

    let surface = default_palette()
        .surface
        .expect("default palette should have a surface color");
    for column in 0..40 {
        assert_eq!(buffer[(column, top_index as u16)].fg, surface);
        assert_eq!(
            buffer[(column, top_index as u16)].bg,
            ratatui::style::Color::Reset
        );
        assert_eq!(buffer[(column, (top_index + 2) as u16)].fg, surface);
        assert_eq!(
            buffer[(column, (top_index + 2) as u16)].bg,
            ratatui::style::Color::Reset
        );
    }
}

#[test]
fn cx_style_mode_uses_half_height_surface_rows_for_sent_user_messages() {
    let mut model = submitted_model(StyleMode::Cx, "hello");
    let rows = render_rows(&mut model, 40, 10);
    let top = "▄".repeat(40);
    let bottom = "▀".repeat(40);
    let message_index = rows
        .iter()
        .position(|row| row == "› hello                                 ")
        .expect("submitted user message should be visible");

    assert_eq!(rows[message_index - 1], top);
    assert_eq!(rows[message_index + 1], bottom);
}

#[test]
fn ms_style_mode_keeps_the_legacy_prompt_without_frames() {
    let mut model = ready_model(StyleMode::Ms, 40, 8);

    assert_contains_sequence(
        &trimmed_rows(&mut model, 40, 8),
        &["┃ Enter to send Prompt"],
    );
}

fn ready_model(style_mode: StyleMode, width: u16, height: u16) -> Model {
    let mut model = Model::new_with_style_mode(StartupBannerOptions::default(), style_mode);
    model.update(AppEvent::Resized { width, height });
    model.update(AppEvent::DetectedPalette {
        palette: default_palette(),
        has_dark_background: true,
    });
    model
}

fn submitted_model(style_mode: StyleMode, message: &str) -> Model {
    let mut model = ready_model(style_mode, 40, 8);
    for character in message.chars() {
        model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char(character))));
    }
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Enter)));
    model
}

fn trimmed_rows(model: &mut Model, width: u16, height: u16) -> Vec<String> {
    render_rows(model, width, height)
        .into_iter()
        .map(|row| row.trim_end().to_string())
        .collect()
}

fn render_buffer(model: &mut Model, width: u16, height: u16) -> Buffer {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).expect("test backend should initialize");
    terminal
        .draw(|frame| model.render(frame))
        .expect("model should render on test backend");

    terminal.backend().buffer().clone()
}

fn render_rows(model: &mut Model, width: u16, height: u16) -> Vec<String> {
    buffer_rows(&render_buffer(model, width, height))
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

fn trim_right_per_line(text: &str) -> String {
    text.split('\n')
        .map(|line| line.trim_end())
        .collect::<Vec<_>>()
        .join("\n")
}

fn assert_contains_sequence(rows: &[String], expected: &[&str]) {
    let haystack = rows.join("\n");
    let needle = expected.join("\n");
    assert!(
        haystack.contains(&needle),
        "expected sequence not found.\nneedle:\n{needle}\n\nhaystack:\n{haystack}"
    );
}
