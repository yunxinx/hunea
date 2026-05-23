use std::{
    env, fs,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
    time::{SystemTime, UNIX_EPOCH},
};

use crossterm::event::KeyCode;
use mo_tui::{
    AppEffect, AppEvent, HeroOptions, Model, ModelOptions, StatusLineItem, StyleMode,
    theme::default_palette,
};
use ratatui::{
    Terminal,
    backend::TestBackend,
    buffer::Buffer,
    style::{Color, Modifier},
};

#[test]
fn inline_command_panel_renders_below_composer_and_hides_regular_status_line() {
    let _guard = lock_test_environment();
    let original_dir = env::current_dir().expect("current directory should be available");

    let repo_dir = temp_test_dir("command-panel-status-line");
    write_git_head(&repo_dir, "ref: refs/heads/main\n");
    env::set_current_dir(&repo_dir).expect("should switch into repo directory");

    let mut model = ready_model(
        48,
        12,
        ModelOptions {
            style_mode: StyleMode::Cx,
            status_line_items: vec![StatusLineItem::GitBranch],
            ..ModelOptions::default()
        },
    );

    assert!(
        render_trimmed_rows(&mut model, 48, 12)
            .iter()
            .any(|row| row.contains("main")),
        "regular status line should be visible before the command panel activates"
    );

    type_text(&mut model, "/");

    let rows = render_trimmed_rows(&mut model, 48, 12);
    assert!(
        rows.iter()
            .any(|row| row.contains("/exit") && row.contains("Exit the application")),
        "expected inline command panel rows, got: {rows:?}"
    );
    assert!(
        rows.iter().all(|row| !row.contains("main")),
        "regular status line should hide while the inline command panel is active: {rows:?}"
    );

    env::set_current_dir(original_dir).expect("should restore original directory");
}

#[test]
fn command_panel_tab_completes_alias_to_exit() {
    let mut model = ready_model(48, 12, ModelOptions::default());
    type_text(&mut model, "/qu");

    model.update(AppEvent::Key(KeyCode::Tab.into()));

    assert_eq!(model.composer_text(), "/exit");
}

#[test]
fn command_panel_tab_completes_new_alias_to_clear() {
    let mut model = ready_model(48, 12, ModelOptions::default());
    type_text(&mut model, "/ne");

    model.update(AppEvent::Key(KeyCode::Tab.into()));

    assert_eq!(model.composer_text(), "/clear");
}

#[test]
fn command_panel_enter_executes_exit_command() {
    let mut model = ready_model(48, 12, ModelOptions::default());
    type_text(&mut model, "/quit");

    model.update(AppEvent::Key(KeyCode::Enter.into()));

    assert!(model.is_quitting());
}

#[test]
fn command_panel_enter_executes_new_alias_as_clear() {
    let mut model = ready_model(64, 12, ModelOptions::default());
    type_text(&mut model, "hello");
    model.update(AppEvent::Key(KeyCode::Enter.into()));
    assert!(
        model
            .transcript_plain_items()
            .iter()
            .any(|item| item.contains("hello")),
        "sanity check: message should be in transcript before /clear"
    );

    type_text(&mut model, "/new");
    let effect = model.update(AppEvent::Key(KeyCode::Enter.into()));

    assert_eq!(effect, Some(AppEffect::ResetRuntimeSession));
    assert_eq!(model.composer_text(), "");
    assert!(!model.is_quitting());
    assert!(
        render_trimmed_rows(&mut model, 64, 12)
            .iter()
            .any(|row| row.contains("Lumos")),
        "clear should restore the startup hero"
    );
    assert!(
        model
            .transcript_plain_items()
            .iter()
            .all(|item| !item.contains("hello")),
        "clear should remove previous conversation context"
    );
}

#[test]
fn command_panel_shows_no_commands_for_single_unmatched_character() {
    let mut model = ready_model(48, 12, ModelOptions::default());
    type_text(&mut model, "/h");

    let rows = render_trimmed_rows(&mut model, 48, 12);
    assert!(
        rows.iter().any(|row| row.contains("No commands")),
        "single unmatched query should keep the command panel active: {rows:?}"
    );
}

#[test]
fn command_panel_stops_matching_after_second_unmatched_character() {
    let mut model = ready_model(48, 12, ModelOptions::default());
    type_text(&mut model, "/he");

    let rows = render_trimmed_rows(&mut model, 48, 12);
    assert!(
        rows.iter().all(|row| !row.contains("No commands")),
        "second unmatched character should deactivate the panel: {rows:?}"
    );
}

#[test]
fn command_panel_enter_falls_back_to_send_for_single_unmatched_character() {
    let mut model = ready_model(48, 12, ModelOptions::default());
    type_text(&mut model, "/h");

    model.update(AppEvent::Key(KeyCode::Enter.into()));

    assert!(!model.is_quitting());
    assert_eq!(model.composer_text(), "");
    assert!(
        model
            .transcript_plain_items()
            .iter()
            .any(|item| item.contains("/h")),
        "single unmatched slash query should be sent as a normal message"
    );
}

#[test]
fn command_panel_descriptions_align_for_all_root_commands() {
    let mut model = ready_model(64, 12, ModelOptions::default());
    type_text(&mut model, "/");

    let rows = render_trimmed_rows(&mut model, 64, 12);
    let exit_row = rows
        .iter()
        .find(|row| row.contains("/exit"))
        .expect("/exit command should render");
    let models_row = rows
        .iter()
        .find(|row| row.contains("/models"))
        .expect("/models command should render");

    assert_eq!(
        exit_row.find("Exit the application"),
        models_row.find("Select model for this session"),
        "command descriptions should start in the same column: {rows:?}"
    );
}

#[test]
fn command_panel_selected_item_uses_accent_without_coloring_description_blue() {
    let palette = default_palette();
    assert_eq!(
        palette.command_accent,
        Color::Cyan,
        "selected slash commands should use the same bright cyan foreground as codex-rs"
    );

    let mut model = ready_model(64, 12, ModelOptions::default());
    type_text(&mut model, "/");

    let buffer = render_buffer(&mut model, 64, 12);

    assert_text_cells_use_color(&buffer, "/exit", palette.command_accent);
    assert_text_cells_do_not_use_color(&buffer, "/exit", palette.accent);
    assert_text_cells_are_bold(&buffer, "/exit");
    assert_text_cells_use_color(&buffer, "Exit the application", palette.main);
    assert_text_cells_do_not_use_color(&buffer, "Exit the application", palette.command_accent);
    assert_text_cells_use_color(&buffer, "/models", palette.secondary);
}

#[test]
fn debug_tool_command_is_hidden_by_default() {
    let mut model = ready_model(80, 16, ModelOptions::default());
    type_text(&mut model, "/");

    let rows = render_trimmed_rows(&mut model, 80, 16);

    assert!(
        rows.iter().all(|row| !row.contains("/tool-debug")),
        "debug command should be hidden unless debug mode is enabled: {rows:?}"
    );
}

#[test]
fn debug_tool_command_opens_tool_approval_preview_panel() {
    let mut model = ready_model(
        80,
        16,
        ModelOptions {
            debug_commands_enabled: true,
            ..ModelOptions::default()
        },
    );
    type_text(&mut model, "/tool-debug");

    let effect = model.update(AppEvent::Key(KeyCode::Enter.into()));

    assert_eq!(effect, None);
    assert_eq!(model.composer_text(), "");
    let rows = render_trimmed_rows(&mut model, 80, 16);
    assert!(
        rows.iter().any(|row| row.contains("Tool Approval:")),
        "expected /tool-debug preview panel, got: {rows:?}"
    );
    assert!(
        rows.iter().all(|row| !row.contains("Preview")),
        "preview marker should not be rendered inside the approval panel: {rows:?}"
    );
    assert!(
        rows.iter().all(|row| !row.contains("Preview tool request")),
        "preview panel should not render a synthetic preview request title: {rows:?}"
    );
    assert!(
        rows.iter().all(|row| !row.contains("Tool   :")),
        "tool label row should not be rendered: {rows:?}"
    );
    assert!(
        rows.iter().all(|row| !row.contains("Shell command")),
        "tool name should not be rendered as a separate row: {rows:?}"
    );
    assert!(
        rows.iter().all(|row| !row.contains("Request:")),
        "request label row should not be rendered: {rows:?}"
    );
    assert_ordered_rows(&rows, &["Tool Approval:", "sed -n", "1. Yes"]);
    assert_ordered_rows(
        &rows,
        &[
            "1. Yes",
            "2. Yes, allow similar requests during this session",
            "3. No",
            "4. No, reject similar requests during this session",
        ],
    );
    assert!(
        rows.iter().all(|row| !row.contains("Actions:")),
        "tool approval preview should use vertical choices without the old actions heading: {rows:?}"
    );
    assert!(
        rows.iter().all(|row| !row.contains("Reason")),
        "preview panel should not synthesize a reason row: {rows:?}"
    );
    assert_blank_row_after(&rows, "Tool Approval:");
    assert_gap_between_rows(&rows, "sed -n", "1. Yes", 1);
    assert!(
        rows.iter()
            .any(|row| { row.contains("Esc to cancel · Enter to choose") }),
        "approval footer should use concise key hint copy: {rows:?}"
    );
    let buffer = render_buffer(&mut model, 80, 16);
    assert_text_cells_are_bold(&buffer, "sed -n '1,80p' src/main.rs");
    assert_text_cells_use_multiple_colors(&buffer, "sed -n '1,80p' src/main.rs");
    assert!(
        rows.iter().all(|row| !row.contains('›')),
        "composer prompt should be hidden while preview panel is open: {rows:?}"
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
fn command_panel_tab_completion_can_restore_external_editor_helper_after_panel_exits() {
    let mut model = ready_model(
        12,
        12,
        ModelOptions {
            style_mode: StyleMode::Cx,
            external_editor_hint: "code".to_string(),
            ..ModelOptions::default()
        },
    );
    type_text(&mut model, "/q");

    model.update(AppEvent::Key(KeyCode::Tab.into()));
    type_text(&mut model, "xxxxxxxxxxxxxxxxxxxx");

    let rows = render_trimmed_rows(&mut model, 12, 12);
    assert!(
        rows.iter().any(|row| row.contains("ctrl+g")),
        "external editor helper should reappear after tab completion leaves command mode: {rows:?}"
    );
}

#[test]
fn command_panel_text_can_be_drag_selected_and_copied() {
    let mut model = ready_model(
        48,
        12,
        ModelOptions {
            style_mode: StyleMode::Cx,
            copy_on_mouse_selection_release: true,
            ..ModelOptions::default()
        },
    );
    type_text(&mut model, "/");

    let (row, column) = find_cell_containing(&mut model, 48, 12, "/exit");
    assert!(
        model
            .update(AppEvent::MouseDown {
                button: crossterm::event::MouseButton::Left,
                column: u16::try_from(column).unwrap(),
                row: u16::try_from(row).unwrap(),
            })
            .is_none()
    );
    assert!(
        model
            .update(AppEvent::MouseDrag {
                button: crossterm::event::MouseButton::Left,
                column: u16::try_from(column + 5).unwrap(),
                row: u16::try_from(row).unwrap(),
            })
            .is_none()
    );

    let effect = model.update(AppEvent::MouseUp {
        button: crossterm::event::MouseButton::Left,
        column: u16::try_from(column + 5).unwrap(),
        row: u16::try_from(row).unwrap(),
    });

    assert_eq!(effect, Some(AppEffect::CopySelection("/exit".to_string())));
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
        render_trimmed_rows(model, width, height)
    )
}

fn render_buffer(model: &mut Model, width: u16, height: u16) -> Buffer {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).expect("test backend should initialize");
    terminal
        .draw(|frame| model.render(frame))
        .expect("model should render on test backend");

    terminal.backend().buffer().clone()
}

fn assert_text_cells_use_color(buffer: &Buffer, text: &str, expected: Color) {
    let (row, column) = find_cell_containing_buffer(buffer, text);
    for offset in 0..text.chars().count() {
        assert_eq!(
            buffer[(column + offset as u16, row)].fg,
            expected,
            "expected {text:?} to use {expected:?} at offset {offset}"
        );
    }
}

fn assert_text_cells_do_not_use_color(buffer: &Buffer, text: &str, rejected: Color) {
    let (row, column) = find_cell_containing_buffer(buffer, text);
    for offset in 0..text.chars().count() {
        assert_ne!(
            buffer[(column + offset as u16, row)].fg,
            rejected,
            "expected {text:?} not to use {rejected:?} at offset {offset}"
        );
    }
}

fn assert_text_cells_are_bold(buffer: &Buffer, text: &str) {
    let (row, column) = find_cell_containing_buffer(buffer, text);
    for offset in 0..text.chars().count() {
        assert!(
            buffer[(column + offset as u16, row)]
                .modifier
                .contains(Modifier::BOLD),
            "expected {text:?} to be bold at offset {offset}"
        );
    }
}

fn assert_text_cells_use_multiple_colors(buffer: &Buffer, text: &str) {
    let (row, column) = find_cell_containing_buffer(buffer, text);
    let mut colors = Vec::new();
    for offset in 0..text.chars().count() {
        let color = buffer[(column + offset as u16, row)].fg;
        if !colors.contains(&color) {
            colors.push(color);
        }
    }
    assert!(
        colors.len() > 1,
        "expected {text:?} to use syntax-highlighted colors, got {colors:?}"
    );
}

fn find_cell_containing_buffer(buffer: &Buffer, needle: &str) -> (u16, u16) {
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

fn test_environment_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

fn lock_test_environment() -> std::sync::MutexGuard<'static, ()> {
    test_environment_lock()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
}

fn temp_test_dir(prefix: &str) -> PathBuf {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time should be after unix epoch")
        .as_nanos();
    let path = env::temp_dir().join(format!("lumos-rust-{prefix}-{unique}"));
    fs::create_dir_all(&path).expect("temp test dir should be created");
    path
}

fn write_git_head(repo_dir: &Path, head_contents: &str) {
    let git_dir = repo_dir.join(".git");
    fs::create_dir_all(&git_dir).expect("git dir should exist");
    fs::write(git_dir.join("HEAD"), head_contents).expect("git head should be written");
}
