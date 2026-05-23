use std::{fs, path::PathBuf};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{Terminal, backend::TestBackend, buffer::Buffer};
use terminal_ui::{
    AppEffect, AppEvent, Model, ModelOptions, StartupBannerOptions, StyleMode,
    theme::default_palette,
};

#[test]
fn ctrl_g_returns_external_editor_launch_with_current_draft() {
    let mut model = ready_model(ModelOptions {
        style_mode: StyleMode::Cx,
        external_editor: vec![
            "sh".to_string(),
            "-c".to_string(),
            "cat \"$1\" >/dev/null".to_string(),
        ],
        external_editor_hint: "sh".to_string(),
        ..ModelOptions::default()
    });

    for character in "hello".chars() {
        let _ = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char(character))));
    }
    let _ = model.update(AppEvent::Key(KeyEvent::new(
        KeyCode::Char('j'),
        KeyModifiers::CONTROL,
    )));
    for character in "world".chars() {
        let _ = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char(character))));
    }

    let effect = model
        .update(AppEvent::Key(KeyEvent::new(
            KeyCode::Char('g'),
            KeyModifiers::CONTROL,
        )))
        .expect("ctrl+g should prepare an external editor launch");
    let AppEffect::LaunchExternalEditor(effect) = effect else {
        panic!("ctrl+g should return an external editor launch effect");
    };

    assert!(
        effect.command[0].ends_with("/sh") || effect.command[0] == "sh",
        "unexpected shell path: {}",
        effect.command[0]
    );
    assert_eq!(
        &effect.command[1..],
        &[
            "-c".to_string(),
            "cat \"$1\" >/dev/null".to_string(),
            "lumos".to_string(),
            effect.draft_path.to_string_lossy().into_owned(),
        ]
    );
    assert_eq!(
        fs::read_to_string(&effect.draft_path).unwrap_or_default(),
        "hello\nworld"
    );

    let _ = fs::remove_file(effect.draft_path);
}

#[test]
fn external_editor_finished_replaces_draft_and_normalizes_crlf() {
    let mut model = ready_model(ModelOptions::default());
    for character in "before".chars() {
        let _ = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char(character))));
    }

    let draft_path = temp_test_file("external-editor-finished");
    fs::write(&draft_path, "after\r\nmore\r\n").expect("draft file should be written");

    let _ = model.update(AppEvent::ExternalEditorFinished {
        draft_path: draft_path.clone(),
        original_draft: "before".to_string(),
        failed: false,
    });

    assert_eq!(model.composer_text(), "after\nmore\n");
}

#[test]
fn multiline_draft_shows_ctrl_g_helper_and_timeout_hides_it() {
    let mut model = ready_model(ModelOptions {
        style_mode: StyleMode::Cx,
        external_editor: vec!["sh".to_string()],
        external_editor_hint: "sh".to_string(),
        ..ModelOptions::default()
    });

    let _ = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char('1'))));
    let _ = model.update(AppEvent::Key(KeyEvent::new(
        KeyCode::Char('j'),
        KeyModifiers::CONTROL,
    )));
    let _ = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char('2'))));
    let _ = model.update(AppEvent::Key(KeyEvent::new(
        KeyCode::Char('j'),
        KeyModifiers::CONTROL,
    )));

    assert_eq!(
        render_trimmed_rows(&mut model, 40, 6)
            .last()
            .map(String::as_str),
        Some("  ctrl+g to edit in sh")
    );

    let _ = model.update(AppEvent::ExternalEditorHelperTimeout { token: 1 });

    assert_ne!(
        render_trimmed_rows(&mut model, 40, 6)
            .last()
            .map(String::as_str),
        Some("  ctrl+g to edit in sh")
    );
}

fn ready_model(options: ModelOptions) -> Model {
    let mut model = Model::new_with_options(StartupBannerOptions::default(), options);
    let _ = model.update(AppEvent::Resized {
        width: 40,
        height: 6,
    });
    let _ = model.update(AppEvent::DetectedPalette {
        palette: default_palette(),
        has_dark_background: true,
    });
    model
}

fn temp_test_file(prefix: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "lumos-rust-{prefix}-{}-{}.txt",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system time should be after unix epoch")
            .as_nanos()
    ))
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
