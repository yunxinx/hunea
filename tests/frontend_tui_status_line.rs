use std::{
    env, fs,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
    time::{SystemTime, UNIX_EPOCH},
};

use crossterm::event::{KeyCode, KeyEvent};
use lumos::frontend::tui::{
    AppEvent, HeroOptions, Model, ModelOptions, StatusLineItem, StyleMode, theme::default_palette,
};
use ratatui::{Terminal, backend::TestBackend, buffer::Buffer};

#[test]
fn cx_status_line_renders_below_composer_frame() {
    let _guard = lock_test_environment();
    let original_dir = env::current_dir().expect("current directory should be available");

    let repo_dir = temp_test_dir("status-line-cx");
    write_git_head(&repo_dir, "ref: refs/heads/main\n");
    env::set_current_dir(&repo_dir).expect("should switch into repo directory");

    let mut model = ready_model(40, 4, StyleMode::Cx, vec![StatusLineItem::GitBranch]);

    assert_eq!(
        render_trimmed_rows(&mut model, 40, 4),
        vec!["", "› Enter to send Prompt", "", "  main"]
    );

    env::set_current_dir(original_dir).expect("should restore original directory");
}

#[test]
fn status_line_renders_current_dir_and_preserves_configured_order() {
    let _guard = lock_test_environment();
    let original_dir = env::current_dir().expect("current directory should be available");
    let original_home = env::var_os("HOME");

    let home_dir = temp_test_dir("status-line-home");
    let repo_dir = home_dir.join("repo");
    fs::create_dir_all(&repo_dir).expect("repo directory should exist");
    write_git_head(&repo_dir, "ref: refs/heads/main\n");
    env::set_current_dir(&repo_dir).expect("should switch into repo directory");
    unsafe {
        env::set_var("HOME", &home_dir);
    }

    let mut model = ready_model(
        40,
        4,
        StyleMode::Cx,
        vec![StatusLineItem::CurrentDir, StatusLineItem::GitBranch],
    );

    assert_eq!(
        render_trimmed_rows(&mut model, 40, 4),
        vec!["", "› Enter to send Prompt", "", "  ~/repo · main"]
    );

    env::set_current_dir(original_dir).expect("should restore original directory");
    restore_env_var("HOME", original_home);
}

#[test]
fn status_line_truncates_without_wrapping_in_narrow_viewport() {
    let _guard = lock_test_environment();
    let original_dir = env::current_dir().expect("current directory should be available");

    let repo_dir = temp_test_dir("status-line-truncate");
    write_git_head(&repo_dir, "ref: refs/heads/feature/very-long-branch-name\n");
    env::set_current_dir(&repo_dir).expect("should switch into repo directory");

    let mut model = ready_model(12, 4, StyleMode::Cx, vec![StatusLineItem::GitBranch]);

    assert_eq!(
        render_trimmed_rows(&mut model, 12, 4),
        vec!["", "› Enter to", "", "  feature/ve"]
    );

    env::set_current_dir(original_dir).expect("should restore original directory");
}

#[test]
fn status_line_refreshes_git_branch_only_after_transcript_changes() {
    let _guard = lock_test_environment();
    let original_dir = env::current_dir().expect("current directory should be available");
    let original_pwd = env::var_os("PWD");

    let repo_dir = temp_test_dir("status-line-refresh");
    write_git_head(&repo_dir, "ref: refs/heads/main\n");
    env::set_current_dir(&repo_dir).expect("should switch into repo directory");
    unsafe {
        env::set_var("PWD", &repo_dir);
    }

    let mut model = ready_model(40, 4, StyleMode::Cx, vec![StatusLineItem::GitBranch]);

    assert_eq!(
        render_trimmed_rows(&mut model, 40, 4),
        vec!["", "› Enter to send Prompt", "", "  main"]
    );

    write_git_head(&repo_dir, "ref: refs/heads/feature/refresh\n");
    model.update(AppEvent::Resized {
        width: 41,
        height: 4,
    });
    assert_eq!(
        render_trimmed_rows(&mut model, 41, 4),
        vec!["", "› Enter to send Prompt", "", "  main"]
    );

    for character in "hello".chars() {
        model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char(character))));
    }
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Enter)));

    let rows = render_trimmed_rows(&mut model, 41, 4);
    assert_eq!(rows.last().map(String::as_str), Some("  feature/refresh"));
    assert!(
        model
            .transcript_plain_items()
            .iter()
            .any(|row| row.contains("hello"))
    );

    env::set_current_dir(original_dir).expect("should restore original directory");
    restore_env_var("PWD", original_pwd);
}

fn ready_model(
    width: u16,
    height: u16,
    style_mode: StyleMode,
    status_line_items: Vec<StatusLineItem>,
) -> Model {
    let mut model = Model::new_with_options(
        HeroOptions::default(),
        ModelOptions {
            style_mode,
            status_line_items,
        },
    );
    model.update(AppEvent::Resized { width, height });
    model.update(AppEvent::DetectedPalette {
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

fn restore_env_var(key: &str, value: Option<std::ffi::OsString>) {
    match value {
        Some(value) => unsafe {
            env::set_var(key, value);
        },
        None => unsafe {
            env::remove_var(key);
        },
    }
}
