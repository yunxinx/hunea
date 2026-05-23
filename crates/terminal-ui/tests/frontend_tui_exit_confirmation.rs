use std::{
    env, fs,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
    time::{SystemTime, UNIX_EPOCH},
};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{Terminal, backend::TestBackend, buffer::Buffer};
use terminal_ui::{
    AppEvent, Model, ModelOptions, StartupBannerOptions, StatusLineItem, StyleMode,
    theme::default_palette,
};

#[test]
fn first_ctrl_c_renders_exit_confirmation_notice_in_status_slot() {
    let _guard = lock_test_environment();
    let original_dir = env::current_dir().expect("current directory should be available");

    let repo_dir = temp_test_dir("exit-confirmation-render");
    write_git_head(&repo_dir, "ref: refs/heads/main\n");
    env::set_current_dir(&repo_dir).expect("should switch into repo directory");

    let mut model = ready_model();
    let _ = model.update(AppEvent::Key(KeyEvent::new(
        KeyCode::Char('c'),
        KeyModifiers::CONTROL,
    )));

    assert_eq!(
        render_trimmed_rows(&mut model, 40, 4)
            .last()
            .map(String::as_str),
        Some("  Press again to exit")
    );
    assert!(!model.is_quitting());

    env::set_current_dir(original_dir).expect("should restore original directory");
}

#[test]
fn status_notice_timeout_restores_previous_status_line_content() {
    let _guard = lock_test_environment();
    let original_dir = env::current_dir().expect("current directory should be available");

    let repo_dir = temp_test_dir("exit-confirmation-timeout");
    write_git_head(&repo_dir, "ref: refs/heads/main\n");
    env::set_current_dir(&repo_dir).expect("should switch into repo directory");

    let mut model = ready_model();
    let _ = model.update(AppEvent::Key(KeyEvent::new(
        KeyCode::Char('c'),
        KeyModifiers::CONTROL,
    )));
    let _ = model.update(AppEvent::StatusNoticeTimeout { token: 1 });

    assert_eq!(
        render_trimmed_rows(&mut model, 40, 4)
            .last()
            .map(String::as_str),
        Some("  main")
    );
    assert!(!model.is_quitting());

    env::set_current_dir(original_dir).expect("should restore original directory");
}

#[test]
fn ctrl_c_clears_existing_draft_before_showing_exit_confirmation() {
    let mut model = ready_model();

    for character in "hello".chars() {
        let _ = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char(character))));
    }

    let _ = model.update(AppEvent::Key(KeyEvent::new(
        KeyCode::Char('c'),
        KeyModifiers::CONTROL,
    )));
    assert_eq!(model.composer_text(), "");
    assert!(!model.is_quitting());

    let _ = model.update(AppEvent::Key(KeyEvent::new(
        KeyCode::Char('c'),
        KeyModifiers::CONTROL,
    )));
    assert!(!model.is_quitting());

    let _ = model.update(AppEvent::Key(KeyEvent::new(
        KeyCode::Char('c'),
        KeyModifiers::CONTROL,
    )));
    assert!(model.is_quitting());
}

#[test]
fn ctrl_c_keeps_exit_confirmation_behavior_when_clear_feature_is_disabled() {
    let mut model = ready_model_with_options(ModelOptions {
        ctrl_c_clears_input: false,
        ..ModelOptions::default()
    });

    for character in "hello".chars() {
        let _ = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char(character))));
    }

    let _ = model.update(AppEvent::Key(KeyEvent::new(
        KeyCode::Char('c'),
        KeyModifiers::CONTROL,
    )));
    assert_eq!(model.composer_text(), "hello");
    assert!(!model.is_quitting());

    let _ = model.update(AppEvent::Key(KeyEvent::new(
        KeyCode::Char('c'),
        KeyModifiers::CONTROL,
    )));
    assert!(model.is_quitting());
}

fn ready_model() -> Model {
    ready_model_with_options(ModelOptions {
        style_mode: StyleMode::Cx,
        status_line_items: vec![StatusLineItem::GitBranch],
        ..ModelOptions::default()
    })
}

fn ready_model_with_options(options: ModelOptions) -> Model {
    let mut model = Model::new_with_options(StartupBannerOptions::default(), options);
    let _ = model.update(AppEvent::Resized {
        width: 40,
        height: 4,
    });
    let _ = model.update(AppEvent::DetectedPalette {
        palette: default_palette(),
        has_dark_background: true,
    });
    model
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

fn write_git_head(repo_dir: &Path, head: &str) {
    fs::create_dir_all(repo_dir.join(".git")).expect("git dir should exist");
    fs::write(repo_dir.join(".git").join("HEAD"), head).expect("HEAD should be written");
}
