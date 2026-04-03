use crossterm::event::{KeyCode, KeyEvent};
use lumos::{
    app::{write_exit_transcript, write_exit_transcript_preserving_ansi},
    frontend::tui::{AppEvent, HeroOptions, Model},
};

#[test]
fn write_exit_transcript_matches_plain_exit_items_without_ansi() {
    let model = submitted_model("hello");
    let expected = model.transcript_exit_items(false).join("\n\n") + "\n";

    let mut output = Vec::new();
    write_exit_transcript(&mut output, &model).expect("exit transcript should render");

    let rendered = String::from_utf8(output).expect("exit transcript should be utf-8");
    assert_eq!(rendered, expected);
    assert!(!rendered.contains("\u{1b}["));
}

#[test]
fn write_exit_transcript_separates_items_with_blank_lines() {
    let model = submitted_model("hello");

    let mut output = Vec::new();
    write_exit_transcript(&mut output, &model).expect("exit transcript should render");

    let rendered = String::from_utf8(output).expect("exit transcript should be utf-8");
    assert!(rendered.contains("Lumos"));
    assert!(rendered.contains("\n\n> hello\n"));
}

#[test]
fn write_exit_transcript_preserving_ansi_keeps_hero_styles() {
    let mut model = Model::new(HeroOptions::default());
    model.update(AppEvent::StartupReadyTimeout);

    let mut output = Vec::new();
    write_exit_transcript_preserving_ansi(&mut output, &model)
        .expect("ansi-preserving exit transcript should render");

    let rendered = String::from_utf8(output).expect("exit transcript should be utf-8");
    assert!(rendered.contains("\u{1b}["));
}

fn submitted_model(message: &str) -> Model {
    let mut model = Model::new(HeroOptions::default());
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
