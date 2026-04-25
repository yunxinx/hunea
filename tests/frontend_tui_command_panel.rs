use std::{
    env, fs,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
    time::{SystemTime, UNIX_EPOCH},
};

use crossterm::event::KeyCode;
use lumos::frontend::tui::{
    AppEffect, AppEvent, HeroOptions, Model, ModelOptions, StatusLineItem, StyleMode,
    theme::default_palette,
};
use ratatui::{Terminal, backend::TestBackend, buffer::Buffer};

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
            .any(|row| row.contains("/exit    Exit the application")),
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
fn command_panel_enter_executes_exit_command() {
    let mut model = ready_model(48, 12, ModelOptions::default());
    type_text(&mut model, "/quit");

    model.update(AppEvent::Key(KeyCode::Enter.into()));

    assert!(model.is_quitting());
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
    let acp_row = rows
        .iter()
        .find(|row| row.contains("/acp"))
        .expect("/acp command should render");

    assert_eq!(
        exit_row.find("Exit the application"),
        acp_row.find("Select ACP agent for this session"),
        "command descriptions should start in the same column: {rows:?}"
    );
}

#[test]
fn command_panel_always_lists_acp_command() {
    let mut model = ready_model(64, 12, ModelOptions::default());
    type_text(&mut model, "/");

    let rows = render_trimmed_rows(&mut model, 64, 12);

    assert!(
        rows.iter()
            .any(|row| row.contains("/acp") && row.contains("ACP")),
        "expected /acp command without ACP config, got: {rows:?}"
    );
}

#[test]
fn command_panel_shows_empty_acp_message_without_configured_agents() {
    let mut model = ready_model(64, 12, ModelOptions::default());
    type_text(&mut model, "/acp");

    let rows = render_trimmed_rows(&mut model, 64, 12);

    assert!(
        rows.iter()
            .any(|row| row.contains("No ACP agents configured")),
        "expected empty ACP configuration message, got: {rows:?}"
    );
    assert!(
        rows.iter()
            .all(|row| !row.contains("Create acp.toml to enable ACP")),
        "empty ACP menu should not suggest config creation inline: {rows:?}"
    );

    model.update(AppEvent::Key(KeyCode::Enter.into()));

    assert_eq!(model.composer_text(), "");
    let rows = render_trimmed_rows(&mut model, 64, 12);
    assert!(
        rows.iter()
            .all(|row| !row.contains("No ACP agents configured")),
        "empty ACP action should not render a transient status notice: {rows:?}"
    );
}

#[test]
fn command_panel_lists_configured_acp_agents_after_acp_command() {
    let mut model = ready_model(
        64,
        12,
        ModelOptions {
            acp_agent_servers: vec!["kimi".to_string(), "codex-acp".to_string()],
            ..ModelOptions::default()
        },
    );
    type_text(&mut model, "/acp");

    let rows = render_trimmed_rows(&mut model, 64, 12);

    assert!(
        rows.iter()
            .any(|row| row.contains("kimi") && row.contains("ACP")),
        "expected kimi ACP choice, got: {rows:?}"
    );
    assert!(
        rows.iter()
            .any(|row| row.contains("codex-acp") && row.contains("ACP")),
        "expected codex-acp ACP choice, got: {rows:?}"
    );
}

#[test]
fn command_panel_enter_on_acp_command_opens_acp_picker() {
    let mut model = ready_model(
        64,
        12,
        ModelOptions {
            acp_agent_servers: vec!["kimi".to_string()],
            ..ModelOptions::default()
        },
    );
    type_text(&mut model, "/");
    model.update(AppEvent::Key(KeyCode::Down.into()));

    model.update(AppEvent::Key(KeyCode::Enter.into()));

    assert_eq!(model.composer_text(), "/acp");
    assert_eq!(model.selected_acp_agent(), None);
    let rows = render_trimmed_rows(&mut model, 64, 12);
    assert!(
        rows.iter()
            .any(|row| row.contains("kimi") && row.contains("ACP")),
        "expected ACP picker after /acp command, got: {rows:?}"
    );
}

#[test]
fn command_panel_enter_selects_acp_agent_for_current_session() {
    let mut model = ready_model(
        64,
        12,
        ModelOptions {
            acp_agent_servers: vec!["kimi".to_string(), "codex-acp".to_string()],
            ..ModelOptions::default()
        },
    );
    type_text(&mut model, "/acp");
    model.update(AppEvent::Key(KeyCode::Down.into()));

    model.update(AppEvent::Key(KeyCode::Enter.into()));

    assert_eq!(model.selected_acp_agent(), Some("codex-acp"));
    assert_eq!(model.composer_text(), "");
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
