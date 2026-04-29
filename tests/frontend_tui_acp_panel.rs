use crossterm::event::{KeyCode, KeyEvent};
use lumos::frontend::tui::{
    AppEffect, AppEvent, HeroOptions, Model, ModelOptions, theme::default_palette,
};
use lumos::runtime::models::{
    ModelCatalog, ModelEntry, ModelProvider, ModelSelection, ModelSource, ProviderKind,
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
fn acp_panel_selection_makes_models_panel_independent_from_native_catalog() {
    let mut model = ready_model(
        72,
        18,
        ModelOptions {
            acp_agent_servers: vec!["codex-acp".to_string()],
            model_catalog: ModelCatalog::new(vec![ModelProvider::new(
                "local",
                ProviderKind::OpenAiCompatible,
                "Local",
                Some("http://127.0.0.1:1234/v1".to_string()),
                ModelSource::Configured,
                vec![ModelEntry::new("qwen3", None, ModelSource::Configured)],
            )]),
            selected_model: Some(ModelSelection::new("local", "qwen3")),
            ..ModelOptions::default()
        },
    );
    type_text(&mut model, "/acp");
    model.update(AppEvent::Key(KeyCode::Enter.into()));
    model.update(AppEvent::Key(KeyCode::Enter.into()));

    type_text(&mut model, "/models");
    model.update(AppEvent::Key(KeyCode::Enter.into()));

    let rows = render_trimmed_rows(&mut model, 72, 18);
    assert!(
        rows.iter()
            .any(|row| row.contains("Providers:") && row.contains("[ACP: codex-acp]")),
        "expected ACP model provider, got: {rows:?}"
    );
    assert!(
        rows.iter()
            .any(|row| row.contains("No models available for this provider")),
        "expected ACP empty model state, got: {rows:?}"
    );
    assert!(
        rows.iter()
            .all(|row| !row.contains("Local") && !row.contains("qwen3")),
        "native model catalog should be hidden in ACP mode, got: {rows:?}"
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

#[test]
fn acp_permission_request_replaces_composer_with_tool_approval_panel() {
    let mut model = ready_model(72, 18, ModelOptions::default());

    model.update(AppEvent::AcpPermissionRequested {
        request_id: "permission-1".to_string(),
        title: Some("Write file".to_string()),
        allow_option_id: Some("allow-once".to_string()),
        allow_always_option_id: Some("allow-always".to_string()),
        reject_option_id: Some("reject-once".to_string()),
        reject_always_option_id: Some("reject-always".to_string()),
    });

    let buffer = render_buffer(&mut model, 72, 18);
    let line_row = find_row_containing(&buffer, "━")
        .expect("tool approval panel should render a blue separator line");
    assert_blue_bold_row(&buffer, line_row);

    let rows = trim_rows(&buffer);
    assert!(
        rows.iter().any(|row| row.contains("Tool Approval:")),
        "expected tool approval header, got: {rows:?}"
    );
    assert!(
        rows.iter().all(|row| !row.contains("Tool   :")),
        "tool label row should not be rendered: {rows:?}"
    );
    assert!(
        rows.iter().all(|row| !row.contains("ACP agent")),
        "ACP tool name should not be rendered as a separate row: {rows:?}"
    );
    assert!(
        rows.iter().all(|row| !row.contains("Request:")),
        "request label row should not be rendered: {rows:?}"
    );
    assert_ordered_rows(&rows, &["Tool Approval:", "Write file", "Actions:"]);
    assert_ordered_rows(
        &rows,
        &["Allow", "Allow in session", "Reject", "Reject in session"],
    );
    assert!(
        rows.iter().all(|row| !row.contains("Reason")),
        "ACP permission panel should not synthesize a reason row: {rows:?}"
    );
    assert_blank_row_after(&rows, "Tool Approval:");
    assert_gap_between_rows(&rows, "Write file", "Actions:", 1);
    assert!(
        rows.iter().all(|row| !row.contains("ACP permission:")),
        "permission requests should not be rendered as status notices: {rows:?}"
    );
    assert!(
        rows.iter().any(|row| row.contains("Write file")),
        "expected permission title in approval panel, got: {rows:?}"
    );
    assert!(
        rows.iter().all(|row| !row.contains('›')),
        "composer prompt should be hidden while tool approval panel replaces the input area: {rows:?}"
    );
}

fn assert_blank_row_after(rows: &[String], needle: &str) {
    let index = row_index(rows, needle);
    assert_eq!(
        rows.get(index + 1).map(String::as_str),
        Some(""),
        "expected a blank row after {needle:?}, got: {rows:?}"
    );
}

fn assert_gap_between_rows(rows: &[String], upper: &str, lower: &str, gap: usize) {
    let upper_index = row_index(rows, upper);
    let lower_index = row_index(rows, lower);
    assert_eq!(
        lower_index.saturating_sub(upper_index + 1),
        gap,
        "expected {gap} blank rows between {upper:?} and {lower:?}, got: {rows:?}"
    );
}

fn assert_ordered_rows(rows: &[String], needles: &[&str]) {
    let mut last_index = None;
    for needle in needles {
        let index = row_index(rows, needle);
        if let Some(last_index) = last_index {
            assert!(
                index >= last_index,
                "expected {needle:?} to appear after previous item, got: {rows:?}"
            );
        }
        last_index = Some(index);
    }
}

fn row_index(rows: &[String], needle: &str) -> usize {
    rows.iter()
        .position(|row| row.contains(needle))
        .unwrap_or_else(|| panic!("expected row containing {needle:?}, got: {rows:?}"))
}

#[test]
fn acp_permission_enter_responds_with_selected_tool_approval_option() {
    let mut model = ready_model(72, 18, ModelOptions::default());
    model.update(AppEvent::AcpPermissionRequested {
        request_id: "permission-2".to_string(),
        title: Some("Run command".to_string()),
        allow_option_id: Some("allow-once".to_string()),
        allow_always_option_id: Some("allow-always".to_string()),
        reject_option_id: Some("reject-once".to_string()),
        reject_always_option_id: Some("reject-always".to_string()),
    });

    let effect = model.update(AppEvent::Key(KeyCode::Enter.into()));

    assert_eq!(
        effect,
        Some(AppEffect::RespondAcpPermission {
            request_id: "permission-2".to_string(),
            option_id: Some("allow-once".to_string()),
        })
    );
    let rows = render_trimmed_rows(&mut model, 72, 18);
    assert!(
        rows.iter().all(|row| !row.contains("Tool Approval:")),
        "panel should close after responding: {rows:?}"
    );
}

#[test]
fn acp_permission_deny_appends_reject_result_to_transcript() {
    let mut model = ready_model(72, 18, ModelOptions::default());
    model.update(AppEvent::AcpPermissionRequested {
        request_id: "permission-3".to_string(),
        title: Some("Run destructive command".to_string()),
        allow_option_id: Some("allow-once".to_string()),
        allow_always_option_id: Some("allow-always".to_string()),
        reject_option_id: Some("reject-once".to_string()),
        reject_always_option_id: Some("reject-always".to_string()),
    });

    let effect = model.update(AppEvent::Key(KeyCode::Char('n').into()));

    assert_eq!(
        effect,
        Some(AppEffect::RespondAcpPermission {
            request_id: "permission-3".to_string(),
            option_id: Some("reject-once".to_string()),
        })
    );
    let buffer = render_buffer(&mut model, 72, 18);
    let rows = trim_rows(&buffer);
    assert!(
        rows.iter()
            .any(|row| row.contains("● Reject destructive command")),
        "reject result should be appended to transcript, got: {rows:?}"
    );
    assert_text_cells_use_color(&buffer, "● ", default_palette().approval_rejected);
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
