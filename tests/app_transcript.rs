use crossterm::event::{KeyCode, KeyEvent};
use lumos::{
    app::write_exit_transcript,
    frontend::tui::{AppEvent, HeroOptions, Model},
};

#[test]
fn write_exit_transcript_separates_items_with_blank_lines() {
    let mut model = Model::new(HeroOptions::default());

    model.update(AppEvent::Resized {
        width: 80,
        height: 24,
    });
    for character in "hello".chars() {
        model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char(character))));
    }
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Enter)));

    let mut output = Vec::new();
    write_exit_transcript(&mut output, &model).expect("exit transcript should render");

    let rendered = String::from_utf8(output).expect("exit transcript should be utf-8");
    assert!(rendered.contains("Lumos"));
    assert!(rendered.contains("\n\n> hello\n"));
}
