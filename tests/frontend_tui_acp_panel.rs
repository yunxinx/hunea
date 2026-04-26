use crossterm::event::{KeyCode, KeyEvent};
use lumos::frontend::tui::{
    AppEffect, AppEvent, HeroOptions, Model, ModelOptions, theme::default_palette,
};
use ratatui::{
    Terminal,
    backend::TestBackend,
    buffer::Buffer,
    style::{Color, Modifier},
};

#[test]
fn acp_command_replaces_composer_with_acp_panel() {
    let mut model = ready_model(72, 18, ModelOptions::default());
    type_text(&mut model, "/acp");

    model.update(AppEvent::Key(KeyCode::Enter.into()));

    let buffer = render_buffer(&mut model, 72, 18);
    let line_row =
        find_row_containing(&buffer, "━").expect("acp panel should render a blue separator line");
    assert_blue_bold_row(&buffer, line_row);

    let rows = trim_rows(&buffer);
    assert!(
        rows.iter().any(|row| row.contains("ACP Agents:")),
        "expected ACP panel header, got: {rows:?}"
    );
    assert!(
        rows.iter().all(|row| !row.contains("Overview")),
        "ACP panel should stay compact and omit Overview: {rows:?}"
    );
    assert!(
        rows.iter()
            .any(|row| row.contains("No ACP agents configured")),
        "expected empty ACP state, got: {rows:?}"
    );
    assert!(
        rows.iter().all(|row| !row.contains('›')),
        "composer prompt should be hidden while ACP panel replaces the input area: {rows:?}"
    );
}

#[test]
fn acp_panel_lists_configured_agents() {
    let mut model = ready_model(
        72,
        18,
        ModelOptions {
            acp_agent_servers: vec!["kimi".to_string(), "codex-acp".to_string()],
            ..ModelOptions::default()
        },
    );
    type_text(&mut model, "/acp");

    model.update(AppEvent::Key(KeyCode::Enter.into()));

    let rows = render_trimmed_rows(&mut model, 72, 18);
    assert!(
        rows.iter().any(|row| row.contains("ACP Agents:")),
        "expected ACP panel header, got: {rows:?}"
    );
    assert!(
        rows.iter().all(|row| !row.contains("[Session]")),
        "ACP panel should not render a fake tab/provider label: {rows:?}"
    );
    assert!(
        rows.iter().any(|row| row.contains("kimi")),
        "expected first ACP agent, got: {rows:?}"
    );
    assert!(
        rows.iter().any(|row| row.contains("codex-acp")),
        "expected second ACP agent, got: {rows:?}"
    );
    assert!(
        rows.iter().all(|row| !row.contains('›')),
        "composer prompt should stay hidden while ACP panel is open: {rows:?}"
    );
}

#[test]
fn acp_panel_enter_selects_agent_and_restores_composer() {
    let mut model = ready_model(
        72,
        18,
        ModelOptions {
            acp_agent_servers: vec!["kimi".to_string(), "codex-acp".to_string()],
            ..ModelOptions::default()
        },
    );
    type_text(&mut model, "/acp");
    model.update(AppEvent::Key(KeyCode::Enter.into()));
    model.update(AppEvent::Key(KeyCode::Down.into()));

    let effect = model.update(AppEvent::Key(KeyCode::Enter.into()));

    assert_eq!(model.selected_acp_agent(), Some("codex-acp"));
    assert_eq!(
        effect,
        Some(AppEffect::StartAcpSession {
            agent_id: "codex-acp".to_string(),
        })
    );
    let rows = render_trimmed_rows(&mut model, 72, 18);
    assert!(
        rows.iter().all(|row| !row.contains("ACP Agents:")),
        "panel should close after selecting an ACP agent: {rows:?}"
    );
}

#[test]
fn acp_panel_esc_closes_without_changing_selection() {
    let mut model = ready_model(
        72,
        18,
        ModelOptions {
            acp_agent_servers: vec!["kimi".to_string(), "codex-acp".to_string()],
            ..ModelOptions::default()
        },
    );
    type_text(&mut model, "/acp");
    model.update(AppEvent::Key(KeyCode::Enter.into()));
    model.update(AppEvent::Key(KeyCode::Down.into()));

    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Esc)));

    assert_eq!(model.selected_acp_agent(), None);
    let rows = render_trimmed_rows(&mut model, 72, 18);
    assert!(
        rows.iter().all(|row| !row.contains("ACP Agents:")),
        "panel should close after Esc: {rows:?}"
    );
}

fn ready_model(width: u16, height: u16, options: ModelOptions) -> Model {
    let mut model = Model::new_with_options(HeroOptions::default(), options);
    model.update(AppEvent::Resized { width, height });
    model.update(AppEvent::DetectedPalette {
        palette: default_palette(),
        has_dark_background: true,
    });
    model
}

fn type_text(model: &mut Model, text: &str) {
    for character in text.chars() {
        model.update(AppEvent::Key(KeyCode::Char(character).into()));
    }
}

fn render_trimmed_rows(model: &mut Model, width: u16, height: u16) -> Vec<String> {
    trim_rows(&render_buffer(model, width, height))
}

fn render_buffer(model: &mut Model, width: u16, height: u16) -> Buffer {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).expect("test backend should initialize");
    terminal
        .draw(|frame| model.render(frame))
        .expect("model should render on test backend");

    terminal.backend().buffer().clone()
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

fn find_row_containing(buffer: &Buffer, needle: &str) -> Option<u16> {
    for row in 0..buffer.area.height {
        let mut rendered = String::new();
        for column in 0..buffer.area.width {
            rendered.push_str(buffer[(column, row)].symbol());
        }
        if rendered.contains(needle) {
            return Some(row);
        }
    }

    None
}

fn assert_blue_bold_row(buffer: &Buffer, row: u16) {
    let palette = default_palette();
    let styled_cells = (0..buffer.area.width)
        .filter(|column| buffer[(*column, row)].symbol() == "━")
        .collect::<Vec<_>>();
    assert!(
        !styled_cells.is_empty(),
        "separator row should contain horizontal rule glyphs"
    );
    for column in styled_cells {
        let cell = &buffer[(column, row)];
        assert_eq!(cell.fg, palette.accent);
        assert!(
            cell.modifier.contains(Modifier::BOLD),
            "separator line should be bold at column {column}"
        );
    }
}

#[test]
fn acp_panel_header_uses_primary_text_color() {
    let mut model = ready_model(72, 18, ModelOptions::default());
    type_text(&mut model, "/acp");
    model.update(AppEvent::Key(KeyCode::Enter.into()));

    let buffer = render_buffer(&mut model, 72, 18);
    assert_text_cells_use_color(&buffer, "ACP Agents:", default_palette().main);
}

#[test]
fn acp_panel_footer_hint_is_italic() {
    let mut model = ready_model(72, 18, ModelOptions::default());
    type_text(&mut model, "/acp");
    model.update(AppEvent::Key(KeyCode::Enter.into()));

    let buffer = render_buffer(&mut model, 72, 18);

    assert_text_cells_are_italic(
        &buffer,
        "Press Enter to select · Esc to exit · ↑↓ to navigate",
    );
}

fn assert_text_cells_use_color(buffer: &Buffer, text: &str, expected: Color) {
    let (row, column) = find_cell_containing(buffer, text);
    for offset in 0..text.chars().count() {
        assert_eq!(
            buffer[(column + offset as u16, row)].fg,
            expected,
            "expected {text:?} to use {expected:?} at offset {offset}"
        );
    }
}

fn assert_text_cells_are_italic(buffer: &Buffer, text: &str) {
    let (row, column) = find_cell_containing(buffer, text);
    for offset in 0..text.chars().count() {
        assert!(
            buffer[(column + offset as u16, row)]
                .modifier
                .contains(Modifier::ITALIC),
            "expected {text:?} to be italic at offset {offset}"
        );
    }
}

fn find_cell_containing(buffer: &Buffer, needle: &str) -> (u16, u16) {
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
                return (row, column as u16);
            }
        }
    }

    panic!(
        "could not find {needle:?} in rendered rows: {:?}",
        trim_rows(buffer)
    )
}
