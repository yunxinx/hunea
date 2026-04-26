use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use lumos::{
    frontend::tui::{
        AppEvent, HeroOptions, Model, ModelOptions,
        theme::{palette_from_background, terminal_default_palette},
    },
    runtime::models::{ModelCatalog, ModelEntry, ModelProvider, ModelSelection, ModelSource},
};
use ratatui::{Terminal, backend::TestBackend, buffer::Buffer};

#[test]
fn model_does_not_auto_select_first_catalog_model_without_default() {
    let model = Model::new_with_options(
        HeroOptions::default(),
        ModelOptions {
            model_catalog: single_model_catalog(),
            ..ModelOptions::default()
        },
    );

    assert_eq!(model.selected_model(), None);
}

#[test]
fn enter_with_required_empty_model_shows_notice_without_sending() {
    let mut model = Model::new_with_options(
        HeroOptions::default(),
        ModelOptions {
            model_catalog: single_model_catalog(),
            requires_model_selection: true,
            ..ModelOptions::default()
        },
    );
    model.update(AppEvent::Resized {
        width: 80,
        height: 24,
    });
    model.update(AppEvent::DetectedPalette {
        palette: terminal_default_palette(),
        has_dark_background: true,
    });

    for character in "hello".chars() {
        model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char(character))));
    }
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Enter)));

    assert_eq!(model.composer_text(), "hello");
    assert_eq!(model.transcript_plain_items().len(), 1);
    assert!(rendered_model_text(&mut model).contains("Select a model before sending"));
}

#[test]
fn enter_with_required_selected_model_sends_message() {
    let mut model = Model::new_with_options(
        HeroOptions::default(),
        ModelOptions {
            model_catalog: single_model_catalog(),
            selected_model: Some(ModelSelection::new("local", "qwen3")),
            requires_model_selection: true,
            ..ModelOptions::default()
        },
    );

    for character in "hello".chars() {
        model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char(character))));
    }
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Enter)));

    assert_eq!(model.composer_text(), "");
    assert!(
        model
            .transcript_plain_items()
            .iter()
            .any(|item| item.contains("hello"))
    );
}

fn single_model_catalog() -> ModelCatalog {
    ModelCatalog::new(vec![ModelProvider::new(
        "local",
        "Local",
        Some("http://127.0.0.1:1234/v1".to_string()),
        ModelSource::Configured,
        vec![ModelEntry::new("qwen3", None, ModelSource::Configured)],
    )])
}

#[test]
fn foreground_fallback_uses_light_palette_when_foreground_is_dark() {
    let mut model = Model::new(HeroOptions::default());

    model.update(AppEvent::ForegroundColorHint { is_dark: true });

    assert_eq!(model.palette(), &palette_from_background(false, None));
    assert!(model.has_palette());
}

#[test]
fn startup_timeout_allows_initial_frame_without_detected_palette() {
    let mut model = Model::new(HeroOptions::default());

    model.update(AppEvent::Resized {
        width: 80,
        height: 24,
    });
    model.update(AppEvent::StartupReadyTimeout);

    assert_eq!(model.palette(), &terminal_default_palette());
    assert!(model.is_ready());

    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).expect("test backend should initialize");
    terminal
        .draw(|frame| model.render(frame))
        .expect("model should render on test backend");

    let rendered = buffer_text(terminal.backend().buffer());
    assert!(
        !rendered.trim().is_empty(),
        "ready model should produce visible frame content"
    );
}

#[test]
fn enter_submits_raw_message_and_clears_the_composer() {
    let mut model = Model::new(HeroOptions::default());

    model.update(AppEvent::Resized {
        width: 80,
        height: 24,
    });

    for character in [' ', ' ', 'h', 'i', ' ', ' '] {
        model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char(character))));
    }
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Enter)));

    assert_eq!(model.composer_text(), "");

    let items = model.transcript_plain_items();
    assert_eq!(items.len(), 2);
    assert_eq!(items[1], "›   hi  ");
}

#[test]
fn first_ctrl_c_shows_exit_confirmation_instead_of_quitting_immediately() {
    let mut model = Model::new(HeroOptions::default());

    model.update(AppEvent::Key(KeyEvent::new(
        KeyCode::Char('c'),
        KeyModifiers::CONTROL,
    )));

    assert!(!model.is_quitting());
}

#[test]
fn second_ctrl_c_quits_while_confirmation_is_active() {
    let mut model = Model::new(HeroOptions::default());

    model.update(AppEvent::Key(KeyEvent::new(
        KeyCode::Char('c'),
        KeyModifiers::CONTROL,
    )));
    model.update(AppEvent::Key(KeyEvent::new(
        KeyCode::Char('c'),
        KeyModifiers::CONTROL,
    )));

    assert!(model.is_quitting());
}

#[test]
fn terminal_replay_items_use_the_current_width_for_the_hero() {
    let mut model = Model::new(HeroOptions {
        app_name: Some("L".repeat(120)),
        ..HeroOptions::default()
    });

    model.update(AppEvent::Resized {
        width: 20,
        height: 8,
    });

    let items = model.terminal_replay_items(false);
    assert_eq!(items.len(), 1);

    for line in items[0].split('\n') {
        assert!(line.chars().count() <= 20, "line exceeded width: {line}");
    }
}

fn buffer_text(buffer: &Buffer) -> String {
    let mut rendered = String::new();

    for row in 0..buffer.area.height {
        for column in 0..buffer.area.width {
            rendered.push_str(buffer[(column, row)].symbol());
        }
        rendered.push('\n');
    }

    rendered
}

fn rendered_model_text(model: &mut Model) -> String {
    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).expect("test backend should initialize");
    terminal
        .draw(|frame| model.render(frame))
        .expect("model should render on test backend");
    buffer_text(terminal.backend().buffer())
}
