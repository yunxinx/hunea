use crossterm::event::{KeyCode, KeyEvent};
use runtime_domain::{
    model_catalog::{ModelCatalog, ModelEntry, ModelProvider, ModelSelection, ModelSource},
    provider::ProviderKind,
};
use terminal_app::{write_terminal_replay, write_terminal_replay_preserving_ansi};
use terminal_ui::{AppEvent, Model, ModelOptions, StartupBannerOptions, theme::default_palette};

#[test]
fn write_terminal_replay_matches_terminal_replay_items_without_ansi() {
    let model = submitted_model("hello");
    let expected = model.terminal_replay_items(false).join("\n\n") + "\n";

    let mut output = Vec::new();
    write_terminal_replay(&mut output, &model).expect("terminal replay should render");

    let rendered = String::from_utf8(output).expect("terminal replay should be utf-8");
    assert_eq!(rendered, expected);
    assert!(!rendered.contains("\u{1b}["));
}

#[test]
fn write_terminal_replay_separates_items_with_blank_lines() {
    let model = submitted_model("hello");

    let mut output = Vec::new();
    write_terminal_replay(&mut output, &model).expect("terminal replay should render");

    let rendered = String::from_utf8(output).expect("terminal replay should be utf-8");
    assert!(rendered.contains("Hunea"));
    assert!(rendered.contains("\n\n› hello"));
}

#[test]
fn write_terminal_replay_preserving_ansi_keeps_startup_banner_styles() {
    let mut model = Model::new(StartupBannerOptions::default());
    model.update(AppEvent::DetectedPalette {
        palette: default_palette(),
        has_dark_background: true,
    });
    model.update(AppEvent::StartupReadyTimeout);

    let mut output = Vec::new();
    write_terminal_replay_preserving_ansi(&mut output, &model)
        .expect("ansi-preserving terminal replay should render");

    let rendered = String::from_utf8(output).expect("terminal replay should be utf-8");
    assert!(rendered.contains("\u{1b}["));
}

fn submitted_model(message: &str) -> Model {
    let mut model = Model::new_with_options(
        StartupBannerOptions::default(),
        ModelOptions {
            model_catalog: replay_test_model_catalog(),
            selected_model: Some(ModelSelection::new("local", "qwen3")),
            ..ModelOptions::default()
        },
    );
    model.update(AppEvent::Resized {
        width: 80,
        height: 24,
    });
    model.update(AppEvent::StartupReadyTimeout);

    for character in message.chars() {
        model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char(character))));
    }
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Enter)));

    model
}

fn replay_test_model_catalog() -> ModelCatalog {
    ModelCatalog::new(vec![ModelProvider::new(
        "local",
        ProviderKind::OpenAiCompatible,
        "Local",
        Some("http://127.0.0.1:1234/v1".to_string()),
        ModelSource::Configured,
        vec![ModelEntry::new("qwen3", None, ModelSource::Configured)],
    )])
}
