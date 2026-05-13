use std::{
    env, fs,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use ::mo_native_agent::ProviderKind;
use crossterm::event::{KeyCode, KeyEvent};
use mo_core::model_catalog::{
    ModelCatalog, ModelEntry, ModelProvider, ModelSelection, ModelSource,
};
use mo_tui::{
    AppEvent, HeroOptions, Model, ModelOptions, RequestMetrics, StatusLineItem, StyleMode,
    theme::default_palette,
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
fn status_line_renders_current_model_when_selected() {
    let mut model = ready_model_with_options(
        48,
        4,
        ModelOptions {
            style_mode: StyleMode::Cx,
            status_line_items: vec![StatusLineItem::CurrentModel],
            model_catalog: single_model_catalog(),
            selected_model: Some(ModelSelection::new("local", "qwen3")),
            ..ModelOptions::default()
        },
    );

    assert_eq!(
        render_trimmed_rows(&mut model, 48, 4),
        vec!["", "› Enter to send Prompt", "", "  [Local] qwen3"]
    );
}

#[test]
fn status_line_uses_provider_display_name_for_current_model() {
    let mut model = ready_model_with_options(
        72,
        4,
        ModelOptions {
            style_mode: StyleMode::Cx,
            status_line_items: vec![StatusLineItem::CurrentModel],
            model_catalog: ModelCatalog::new(vec![ModelProvider::native(
                "local",
                ProviderKind::OpenAiCompatible,
                "LM Studio",
                Some("http://localhost:1234/v1".to_string()),
                ModelSource::Configured,
                vec![ModelEntry::new(
                    "qwen/qwen3-4b-2507",
                    None,
                    ModelSource::Configured,
                )],
            )]),
            selected_model: Some(ModelSelection::new("local", "qwen/qwen3-4b-2507")),
            ..ModelOptions::default()
        },
    );

    assert_eq!(
        render_trimmed_rows(&mut model, 72, 4),
        vec![
            "",
            "› Enter to send Prompt",
            "",
            "  [LM Studio] qwen/qwen3-4b-2507"
        ]
    );
}

#[test]
fn status_line_omits_current_model_when_unselected() {
    let mut model = ready_model_with_options(
        48,
        4,
        ModelOptions {
            style_mode: StyleMode::Cx,
            status_line_items: vec![StatusLineItem::CurrentModel],
            model_catalog: single_model_catalog(),
            ..ModelOptions::default()
        },
    );

    let rows = render_trimmed_rows(&mut model, 48, 4);
    assert_eq!(rows, vec!["", "", "› Enter to send Prompt"]);
    assert!(
        rows.iter().all(|row| !row.contains("local/qwen3")),
        "current-model should not render without a selected model, got: {rows:?}"
    );
}

#[test]
fn status_line_updates_current_model_after_model_panel_selection() {
    let mut model = ready_model_with_options(
        72,
        18,
        ModelOptions {
            style_mode: StyleMode::Cx,
            status_line_items: vec![StatusLineItem::CurrentModel],
            model_catalog: single_model_catalog(),
            ..ModelOptions::default()
        },
    );

    type_text(&mut model, "/models");
    model.update(AppEvent::Key(KeyCode::Enter.into()));
    model.update(AppEvent::Key(KeyCode::Enter.into()));
    model.update(AppEvent::StatusNoticeTimeout { token: 1 });

    let rows = render_trimmed_rows(&mut model, 72, 18);
    assert!(
        rows.iter().any(|row| row.contains("[Local] qwen3")),
        "current-model should reflect the selected model after panel selection, got: {rows:?}"
    );
}

#[test]
fn status_line_renders_request_metrics_in_configured_order() {
    let mut model = ready_model(
        48,
        4,
        StyleMode::Cx,
        vec![StatusLineItem::Throughput, StatusLineItem::Latency],
    );
    model.set_last_request_metrics(Some(RequestMetrics::new(
        Duration::from_millis(530),
        139,
        Duration::from_secs(1),
    )));

    assert_eq!(
        render_trimmed_rows(&mut model, 48, 4),
        vec!["", "› Enter to send Prompt", "", "  139tps · 0.53s"]
    );
}

#[test]
fn status_line_skips_request_metrics_before_successful_request() {
    let mut model = ready_model(
        48,
        4,
        StyleMode::Cx,
        vec![StatusLineItem::Throughput, StatusLineItem::Latency],
    );

    let rows = render_trimmed_rows(&mut model, 48, 4);
    assert_eq!(rows, vec!["", "", "› Enter to send Prompt"]);
    assert!(
        rows.iter()
            .all(|row| !row.contains("tps") && !row.contains("0.00s")),
        "request metrics should not render before a successful request, got: {rows:?}"
    );
}

#[test]
fn status_line_preserves_request_metrics_order_with_other_items() {
    let mut model = ready_model_with_options(
        72,
        4,
        ModelOptions {
            style_mode: StyleMode::Cx,
            status_line_items: vec![
                StatusLineItem::CurrentModel,
                StatusLineItem::Latency,
                StatusLineItem::Throughput,
            ],
            model_catalog: single_model_catalog(),
            selected_model: Some(ModelSelection::new("local", "qwen3")),
            ..ModelOptions::default()
        },
    );
    model.set_last_request_metrics(Some(RequestMetrics::new(
        Duration::from_millis(5),
        0,
        Duration::from_millis(250),
    )));

    assert_eq!(
        render_trimmed_rows(&mut model, 72, 4),
        vec![
            "",
            "› Enter to send Prompt",
            "",
            "  [Local] qwen3 · 0.01s · 0tps"
        ]
    );
}

#[test]
fn second_status_line_renders_after_first_line_and_deduplicates_first_line_items() {
    let _guard = lock_test_environment();
    let original_dir = env::current_dir().expect("current directory should be available");
    let original_home = env::var_os("HOME");

    let home_dir = temp_test_dir("status-line-second-home");
    let repo_dir = home_dir.join("repo");
    fs::create_dir_all(&repo_dir).expect("repo directory should exist");
    write_git_head(&repo_dir, "ref: refs/heads/main\n");
    env::set_current_dir(&repo_dir).expect("should switch into repo directory");
    unsafe {
        env::set_var("HOME", &home_dir);
    }

    let mut model = ready_model_with_options(
        48,
        5,
        ModelOptions {
            style_mode: StyleMode::Cx,
            status_line_items: vec![StatusLineItem::GitBranch],
            status_line_2_items: vec![StatusLineItem::CurrentDir, StatusLineItem::GitBranch],
            ..ModelOptions::default()
        },
    );

    assert_eq!(
        render_trimmed_rows(&mut model, 48, 5),
        vec!["", "› Enter to send Prompt", "", "  main", "  ~/repo"]
    );

    env::set_current_dir(original_dir).expect("should restore original directory");
    restore_env_var("HOME", original_home);
}

#[test]
fn second_status_line_inherits_gap_when_first_line_is_empty() {
    let _guard = lock_test_environment();
    let original_dir = env::current_dir().expect("current directory should be available");
    let original_home = env::var_os("HOME");

    let home_dir = temp_test_dir("status-line-second-gap-home");
    let repo_dir = home_dir.join("repo");
    fs::create_dir_all(&repo_dir).expect("repo directory should exist");
    env::set_current_dir(&repo_dir).expect("should switch into repo directory");
    unsafe {
        env::set_var("HOME", &home_dir);
    }

    let mut model = ready_model_with_options(
        48,
        4,
        ModelOptions {
            style_mode: StyleMode::Ms,
            status_line_2_items: vec![StatusLineItem::CurrentDir],
            ..ModelOptions::default()
        },
    );

    assert_eq!(
        render_trimmed_rows(&mut model, 48, 4),
        vec!["", "┃ Enter to send Prompt", "", "  ~/repo"]
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
        vec!["", "› Enter to", "", "  feature..."]
    );

    env::set_current_dir(original_dir).expect("should restore original directory");
}

#[test]
fn status_line_falls_back_to_whole_line_ellipsis_when_only_one_cell_remains() {
    let _guard = lock_test_environment();
    let original_dir = env::current_dir().expect("current directory should be available");

    let repo_dir = temp_test_dir("status-line-ellipsis");
    write_git_head(&repo_dir, "ref: refs/heads/main\n");
    env::set_current_dir(&repo_dir).expect("should switch into repo directory");

    let mut model = ready_model(
        10,
        4,
        StyleMode::Cx,
        vec![StatusLineItem::GitBranch, StatusLineItem::CurrentDir],
    );
    model.update(AppEvent::DetectedPalette {
        palette: default_palette(),
        has_dark_background: true,
    });

    assert_eq!(
        render_trimmed_rows(&mut model, 10, 4),
        vec!["", "› Enter to", "", "  main..."]
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
    ready_model_with_options(
        width,
        height,
        ModelOptions {
            style_mode,
            status_line_items,
            ..ModelOptions::default()
        },
    )
}

fn ready_model_with_options(width: u16, height: u16, options: ModelOptions) -> Model {
    let mut model = Model::new_with_options(HeroOptions::default(), options);
    model.update(AppEvent::Resized { width, height });
    model.update(AppEvent::DetectedPalette {
        palette: default_palette(),
        has_dark_background: true,
    });
    model
}

fn single_model_catalog() -> ModelCatalog {
    ModelCatalog::new(vec![ModelProvider::native(
        "local",
        ProviderKind::OpenAiCompatible,
        "Local",
        Some("http://127.0.0.1:1234/v1".to_string()),
        ModelSource::Configured,
        vec![ModelEntry::new("qwen3", None, ModelSource::Configured)],
    )])
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
