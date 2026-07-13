use std::time::Duration;

use crossterm::event::{KeyCode, KeyEvent, MouseButton};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier},
};
use runtime_domain::model_catalog::ModelSelection;
use terminal_ui::theme::default_palette;
use terminal_ui::{
    AppEffect, AppEvent, Model, ModelOptions, RequestMetrics, StartupBannerOptions, StatusLineItem,
};

mod common;

use common::single_model_catalog;

#[test]
fn drag_selection_highlights_text_and_copies_on_release_when_enabled() {
    let mut model = ready_selection_model(true);
    submit_message(&mut model, "alpha");
    submit_message(&mut model, "beta");

    let (alpha_row, alpha_column) = find_cell_containing(&mut model, 24, 6, "alpha");
    let (beta_row, beta_column) = find_cell_containing(&mut model, 24, 6, "beta");

    assert!(
        model
            .update(AppEvent::MouseDown {
                button: MouseButton::Left,
                column: u16::try_from(alpha_column + 1).unwrap(),
                row: u16::try_from(alpha_row).unwrap(),
            })
            .is_none()
    );
    assert!(
        model
            .update(AppEvent::MouseDrag {
                button: MouseButton::Left,
                column: u16::try_from(beta_column + 2).unwrap(),
                row: u16::try_from(beta_row).unwrap(),
            })
            .is_none()
    );

    let buffer = render_buffer(&mut model, 24, 6);
    assert!(
        buffer[(
            u16::try_from(alpha_column + 1).unwrap(),
            u16::try_from(alpha_row).unwrap()
        )]
            .modifier
            .contains(Modifier::REVERSED)
    );
    assert!(
        buffer[(
            u16::try_from(beta_column + 1).unwrap(),
            u16::try_from(beta_row).unwrap()
        )]
            .modifier
            .contains(Modifier::REVERSED)
    );

    let effect = model.update(AppEvent::MouseUp {
        button: MouseButton::Left,
        column: u16::try_from(beta_column + 2).unwrap(),
        row: u16::try_from(beta_row).unwrap(),
    });

    assert_eq!(
        effect,
        Some(AppEffect::CopySelection("lpha\nbe".to_string()))
    );
}

#[test]
fn user_message_selection_starts_at_content_not_prompt() {
    let mut model = ready_selection_model(true);
    submit_message(&mut model, "alpha");
    submit_message(&mut model, "beta");

    let (alpha_row, alpha_column) = find_cell_containing(&mut model, 24, 6, "alpha");
    let (beta_row, beta_column) = find_cell_containing(&mut model, 24, 6, "beta");

    assert!(
        model
            .update(AppEvent::MouseDown {
                button: MouseButton::Left,
                column: u16::try_from(alpha_column).unwrap(),
                row: u16::try_from(alpha_row).unwrap(),
            })
            .is_none()
    );
    assert!(
        model
            .update(AppEvent::MouseDrag {
                button: MouseButton::Left,
                column: u16::try_from(beta_column + 4).unwrap(),
                row: u16::try_from(beta_row).unwrap(),
            })
            .is_none()
    );

    let effect = model.update(AppEvent::MouseUp {
        button: MouseButton::Left,
        column: u16::try_from(beta_column + 4).unwrap(),
        row: u16::try_from(beta_row).unwrap(),
    });

    assert_eq!(
        effect,
        Some(AppEffect::CopySelection("alpha\nbeta".to_string()))
    );
}

#[test]
fn user_message_selection_can_start_from_prompt_area_without_copying_it() {
    let mut model = ready_selection_model(true);
    submit_message(&mut model, "alpha");
    submit_message(&mut model, "beta");

    let (alpha_row, alpha_column) = find_cell_containing(&mut model, 24, 6, "alpha");
    let (beta_row, beta_column) = find_cell_containing(&mut model, 24, 6, "beta");
    let prompt_column = alpha_column.saturating_sub(2);

    assert!(
        model
            .update(AppEvent::MouseDown {
                button: MouseButton::Left,
                column: u16::try_from(prompt_column).unwrap(),
                row: u16::try_from(alpha_row).unwrap(),
            })
            .is_none()
    );
    assert!(
        model
            .update(AppEvent::MouseDrag {
                button: MouseButton::Left,
                column: u16::try_from(beta_column + 4).unwrap(),
                row: u16::try_from(beta_row).unwrap(),
            })
            .is_none()
    );

    let effect = model.update(AppEvent::MouseUp {
        button: MouseButton::Left,
        column: u16::try_from(beta_column + 4).unwrap(),
        row: u16::try_from(beta_row).unwrap(),
    });

    assert_eq!(
        effect,
        Some(AppEffect::CopySelection("alpha\nbeta".to_string()))
    );
}

#[test]
fn composer_selection_can_start_from_prompt_area_without_copying_it() {
    let mut model = ready_selection_model(true);
    type_text(&mut model, "alpha");

    let (row, content_column) = find_cell_containing(&mut model, 24, 6, "alpha");
    let prompt_column = content_column.saturating_sub(2);

    assert!(
        model
            .update(AppEvent::MouseDown {
                button: MouseButton::Left,
                column: u16::try_from(prompt_column).unwrap(),
                row: u16::try_from(row).unwrap(),
            })
            .is_none()
    );
    assert!(
        model
            .update(AppEvent::MouseDrag {
                button: MouseButton::Left,
                column: u16::try_from(content_column + 5).unwrap(),
                row: u16::try_from(row).unwrap(),
            })
            .is_none()
    );

    let effect = model.update(AppEvent::MouseUp {
        button: MouseButton::Left,
        column: u16::try_from(content_column + 5).unwrap(),
        row: u16::try_from(row).unwrap(),
    });

    assert_eq!(effect, Some(AppEffect::CopySelection("alpha".to_string())));
}

#[test]
fn mixed_transcript_composer_selection_is_not_consumed_by_composer_editing() {
    let mut model = ready_selection_model(false);
    submit_message(&mut model, "alpha");
    type_text(&mut model, "beta");

    let (alpha_row, alpha_column) = find_cell_containing(&mut model, 24, 6, "alpha");
    let (beta_row, beta_column) = find_cell_containing(&mut model, 24, 6, "beta");
    assert!(
        model
            .update(AppEvent::MouseDown {
                button: MouseButton::Left,
                column: u16::try_from(alpha_column + 1).unwrap(),
                row: u16::try_from(alpha_row).unwrap(),
            })
            .is_none()
    );
    assert!(
        model
            .update(AppEvent::MouseDrag {
                button: MouseButton::Left,
                column: u16::try_from(beta_column + 2).unwrap(),
                row: u16::try_from(beta_row).unwrap(),
            })
            .is_none()
    );
    assert!(
        model
            .update(AppEvent::MouseUp {
                button: MouseButton::Left,
                column: u16::try_from(beta_column + 2).unwrap(),
                row: u16::try_from(beta_row).unwrap(),
            })
            .is_none()
    );

    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Backspace)));

    assert_eq!(model.composer_text(), "bet");
}

#[test]
fn keycap_selection_highlights_the_full_wide_grapheme() {
    let mut model = ready_selection_model(false);
    type_text(&mut model, "2️⃣");

    let before = render_buffer(&mut model, 24, 6);
    let (row, column) =
        find_exact_symbol_in_buffer(&before, "2️⃣").expect("keycap should be rendered");

    assert!(
        model
            .update(AppEvent::MouseDown {
                button: MouseButton::Left,
                column: u16::try_from(column).unwrap(),
                row: u16::try_from(row).unwrap(),
            })
            .is_none()
    );
    assert!(
        model
            .update(AppEvent::MouseDrag {
                button: MouseButton::Left,
                column: u16::try_from(column + 2).unwrap(),
                row: u16::try_from(row).unwrap(),
            })
            .is_none()
    );

    let after = render_buffer(&mut model, 24, 6);
    assert_eq!(
        after[(u16::try_from(column).unwrap(), u16::try_from(row).unwrap())].symbol(),
        "2️⃣"
    );
    assert!(
        after[(u16::try_from(column).unwrap(), u16::try_from(row).unwrap())]
            .modifier
            .contains(Modifier::REVERSED)
    );
    assert_eq!(
        after[(
            u16::try_from(column + 1).unwrap(),
            u16::try_from(row).unwrap()
        )]
            .symbol(),
        " "
    );
    assert!(
        !after[(
            u16::try_from(column + 1).unwrap(),
            u16::try_from(row).unwrap()
        )]
            .modifier
            .contains(Modifier::REVERSED),
        "Ratatui hidden tail must stay unstyled; TerminalSurface prefill owns the second selected column"
    );
}

#[test]
fn status_line_selection_can_start_from_left_inset_without_copying_it() {
    let mut model = Model::new_with_options(
        StartupBannerOptions::default(),
        ModelOptions {
            copy_on_mouse_selection_release: true,
            status_line_items: vec![StatusLineItem::CurrentDir],
            ..ModelOptions::default()
        },
    );
    model.update(AppEvent::Resized {
        width: 40,
        height: 6,
    });
    model.update(AppEvent::DetectedPalette {
        palette: default_palette(),
        has_dark_background: true,
    });
    model.update(AppEvent::StartupReadyTimeout);

    let rows = render_rows(&mut model, 40, 6);
    let current_dir_marker = current_dir_marker();
    let (row, status_text) = rows
        .iter()
        .enumerate()
        .find_map(|(row, line)| {
            line.contains(&current_dir_marker)
                .then(|| (row, line.trim().to_string()))
        })
        .expect("status line should include current directory");

    assert!(
        model
            .update(AppEvent::MouseDown {
                button: MouseButton::Left,
                column: 0,
                row: u16::try_from(row).unwrap(),
            })
            .is_none()
    );
    assert!(
        model
            .update(AppEvent::MouseDrag {
                button: MouseButton::Left,
                column: 39,
                row: u16::try_from(row).unwrap(),
            })
            .is_none()
    );

    let effect = model.update(AppEvent::MouseUp {
        button: MouseButton::Left,
        column: 39,
        row: u16::try_from(row).unwrap(),
    });

    assert_eq!(effect, Some(AppEffect::CopySelection(status_text)));
}

#[test]
fn second_status_line_selection_uses_its_own_anchor() {
    let mut model = Model::new_with_options(
        StartupBannerOptions::default(),
        ModelOptions {
            copy_on_mouse_selection_release: true,
            status_line_items: vec![StatusLineItem::Latency],
            status_line_2_items: vec![StatusLineItem::CurrentDir],
            ..ModelOptions::default()
        },
    );
    model.set_last_request_metrics(Some(RequestMetrics::new(
        Duration::from_millis(530),
        139,
        Duration::from_secs(1),
    )));
    model.update(AppEvent::Resized {
        width: 40,
        height: 7,
    });
    model.update(AppEvent::DetectedPalette {
        palette: default_palette(),
        has_dark_background: true,
    });
    model.update(AppEvent::StartupReadyTimeout);

    let rows = render_rows(&mut model, 40, 7);
    let current_dir_marker = current_dir_marker();
    let (row, status_text) = rows
        .iter()
        .enumerate()
        .find_map(|(row, line)| {
            line.contains(&current_dir_marker)
                .then(|| (row, line.trim().to_string()))
        })
        .expect("second status line should include current directory");

    assert!(
        model
            .update(AppEvent::MouseDown {
                button: MouseButton::Left,
                column: 0,
                row: u16::try_from(row).unwrap(),
            })
            .is_none()
    );
    assert!(
        model
            .update(AppEvent::MouseDrag {
                button: MouseButton::Left,
                column: 39,
                row: u16::try_from(row).unwrap(),
            })
            .is_none()
    );

    let effect = model.update(AppEvent::MouseUp {
        button: MouseButton::Left,
        column: 39,
        row: u16::try_from(row).unwrap(),
    });

    assert_eq!(effect, Some(AppEffect::CopySelection(status_text)));
}

#[test]
fn status_line_selection_keeps_unselected_cells_dim() {
    let mut model = Model::new_with_options(
        StartupBannerOptions::default(),
        ModelOptions {
            status_line_items: vec![StatusLineItem::CurrentDir],
            ..ModelOptions::default()
        },
    );
    model.update(AppEvent::Resized {
        width: 40,
        height: 6,
    });
    model.update(AppEvent::DetectedPalette {
        palette: default_palette(),
        has_dark_background: true,
    });
    model.update(AppEvent::StartupReadyTimeout);

    let before = render_buffer(&mut model, 40, 6);
    let current_dir_marker = current_dir_marker();
    let (row, content_column) = find_symbol_in_buffer(&before, &current_dir_marker)
        .expect("status line should include current directory");
    let unselected_column = content_column + 3;
    let original_fg = before[(
        u16::try_from(unselected_column).unwrap(),
        u16::try_from(row).unwrap(),
    )]
        .fg;
    assert_ne!(original_fg, Color::Reset);

    assert!(
        model
            .update(AppEvent::MouseDown {
                button: MouseButton::Left,
                column: u16::try_from(content_column).unwrap(),
                row: u16::try_from(row).unwrap(),
            })
            .is_none()
    );
    assert!(
        model
            .update(AppEvent::MouseDrag {
                button: MouseButton::Left,
                column: u16::try_from(content_column + 1).unwrap(),
                row: u16::try_from(row).unwrap(),
            })
            .is_none()
    );

    let after = render_buffer(&mut model, 40, 6);
    assert!(
        after[(
            u16::try_from(content_column).unwrap(),
            u16::try_from(row).unwrap(),
        )]
            .modifier
            .contains(Modifier::REVERSED)
    );
    assert_eq!(
        after[(
            u16::try_from(unselected_column).unwrap(),
            u16::try_from(row).unwrap(),
        )]
            .fg,
        original_fg
    );
    assert!(
        !after[(
            u16::try_from(unselected_column).unwrap(),
            u16::try_from(row).unwrap(),
        )]
            .modifier
            .contains(Modifier::REVERSED)
    );
}

#[test]
fn double_click_selects_word_and_middle_click_copies_it() {
    let mut model = ready_selection_model(false);
    submit_message(&mut model, "hello world");

    let (row, column) = find_cell_containing(&mut model, 24, 6, "world");
    let column = column + 1;
    let row = u16::try_from(row).unwrap();
    let column = u16::try_from(column).unwrap();

    assert!(
        model
            .update(AppEvent::MouseDown {
                button: MouseButton::Left,
                column,
                row,
            })
            .is_none()
    );
    assert!(
        model
            .update(AppEvent::MouseUp {
                button: MouseButton::Left,
                column,
                row,
            })
            .is_none()
    );
    assert!(
        model
            .update(AppEvent::MouseDown {
                button: MouseButton::Left,
                column,
                row,
            })
            .is_none()
    );

    let effect = model.update(AppEvent::MouseDown {
        button: MouseButton::Middle,
        column: 0,
        row,
    });

    assert_eq!(effect, Some(AppEffect::CopySelection("world".to_string())));
}

fn ready_selection_model(copy_on_release: bool) -> Model {
    let mut model = Model::new_with_options(
        StartupBannerOptions::default(),
        ModelOptions {
            copy_on_mouse_selection_release: copy_on_release,
            model_catalog: single_model_catalog(),
            selected_model: Some(ModelSelection::new("local", "qwen3")),
            ..ModelOptions::default()
        },
    );
    model.update(AppEvent::Resized {
        width: 24,
        height: 6,
    });
    model.update(AppEvent::StartupReadyTimeout);
    model
}

fn submit_message(model: &mut Model, text: &str) {
    type_text(model, text);
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Enter)));
}

fn type_text(model: &mut Model, text: &str) {
    for character in text.chars() {
        model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char(character))));
    }
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
        render_rows(model, width, height)
    )
}

fn current_dir_marker() -> String {
    std::env::current_dir()
        .ok()
        .and_then(|path| {
            path.file_name()
                .map(|name| name.to_string_lossy().into_owned())
        })
        .unwrap_or_else(|| "hunea".to_string())
}

fn find_symbol_in_buffer(buffer: &Buffer, needle: &str) -> Option<(usize, usize)> {
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
                return Some((usize::from(row), column));
            }
        }
    }

    None
}

fn find_exact_symbol_in_buffer(buffer: &Buffer, needle: &str) -> Option<(usize, usize)> {
    for row in 0..buffer.area.height {
        for column in 0..buffer.area.width {
            if buffer[(column, row)].symbol() == needle {
                return Some((usize::from(row), usize::from(column)));
            }
        }
    }

    None
}

fn render_rows(model: &mut Model, width: u16, height: u16) -> Vec<String> {
    let buffer = render_buffer(model, width, height);
    let mut rows = Vec::with_capacity(buffer.area.height as usize);
    for row in 0..buffer.area.height {
        let mut rendered = String::new();
        for column in 0..buffer.area.width {
            rendered.push_str(buffer[(column, row)].symbol());
        }
        rows.push(rendered);
    }
    rows
}

fn render_buffer(model: &mut Model, width: u16, height: u16) -> Buffer {
    let area = Rect::new(0, 0, width, height);
    let mut buffer = Buffer::empty(area);
    let _ = model.render_to_buffer(area, &mut buffer);
    buffer
}
