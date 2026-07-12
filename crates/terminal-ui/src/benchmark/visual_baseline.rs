use std::fmt::Write as _;

use ratatui::{
    buffer::{Buffer, Cell, CellDiffOption},
    layout::{Position, Rect},
};

use crate::{
    Model, ModelOptions, Sender, StartupBannerOptions, StyleMode,
    display_width::display_width,
    theme::{default_palette, palette_from_background},
};

use super::{large_composer_draft_fixture, large_rust_code_block_fixture};

#[test]
fn canonical_frame_snapshot_records_non_default_diff_option() {
    let area = Rect::new(0, 0, 1, 1);
    let mut buffer = Buffer::empty(area);
    buffer[(0, 0)].diff_option = CellDiffOption::AlwaysUpdate;

    let snapshot = canonical_frame_snapshot(&buffer, None);

    assert!(snapshot.contains("diff=AlwaysUpdate"));
}

#[test]
fn cx_dark_conversation_matches_full_frame_golden() {
    let width = 64;
    let height = 18;
    let mut model = baseline_model(StyleMode::Cx, width, height, true);
    model.transcript_mut().append_message_with_style_mode(
        Sender::User,
        "请检查宽字符：界面 👨‍👩‍👧",
        StyleMode::Cx,
    );
    model.transcript_mut().append_message_with_style_mode(
        Sender::Assistant,
        "## Render review\n\nWide glyphs stay aligned beside `code`.\n\n```rust\nfn display_width(label: &str) -> usize {\n    label.chars().count()\n}\n```",
        StyleMode::Cx,
    );
    model.sync_transcript_render();
    model
        .composer_mut()
        .reset_text_and_move_to_end("继续 review：中文 + 👨‍👩‍👧");
    model.sync_composer_height();

    assert_frame_golden(
        &mut model,
        width,
        height,
        include_str!("goldens/cx_dark_conversation.golden"),
    );
}

#[test]
fn ms_light_transcript_overlay_matches_full_frame_golden() {
    let width = 64;
    let height = 18;
    let mut model = baseline_model(StyleMode::Ms, width, height, false);
    model.transcript_mut().append_message_with_style_mode(
        Sender::User,
        "Overlay keeps 中文 history visible",
        StyleMode::Ms,
    );
    model.transcript_mut().append_message_with_style_mode(
        Sender::Assistant,
        "## Transcript overlay\n\n- light palette\n- fixed footer\n- scroll percentage\n\n`Esc` closes this view.",
        StyleMode::Ms,
    );
    model.sync_transcript_render();
    model
        .composer_mut()
        .reset_text_and_move_to_end("hidden overlay draft");
    model.sync_composer_height();
    model.open_transcript_overlay();

    assert_frame_golden(
        &mut model,
        width,
        height,
        include_str!("goldens/ms_light_transcript_overlay.golden"),
    );
}

#[test]
fn large_composer_viewport_matches_full_frame_golden() {
    let width = 48;
    let height = 12;
    let mut model = baseline_model(StyleMode::Cx, width, height, true);
    model
        .composer_mut()
        .reset_text_and_move_to_end(large_composer_draft_fixture(8 * 1024));
    model.sync_composer_height();

    assert_frame_golden(
        &mut model,
        width,
        height,
        include_str!("goldens/large_composer_viewport.golden"),
    );
}

#[test]
fn long_assistant_projection_matches_full_frame_golden() {
    let width = 72;
    let height = 10;
    let mut model = baseline_model(StyleMode::Cx, width, height, true);
    let content = large_rust_code_block_fixture(256);
    assert!(content.len() > 4 * 1024);
    model.transcript_mut().append_message_with_style_mode(
        Sender::Assistant,
        content,
        StyleMode::Cx,
    );
    model.sync_transcript_render();

    assert_frame_golden(
        &mut model,
        width,
        height,
        include_str!("goldens/long_assistant_projection.golden"),
    );
}

fn baseline_model(
    style_mode: StyleMode,
    width: u16,
    height: u16,
    has_dark_background: bool,
) -> Model {
    let mut model = Model::new_with_options(
        StartupBannerOptions {
            app_name: Some("hunea".to_string()),
            version: Some("baseline".to_string()),
            model_name: Some("fixture-model".to_string()),
            work_dir: Some("/workspace/hunea".to_string()),
            width: 0,
        },
        ModelOptions {
            style_mode,
            ..ModelOptions::default()
        },
    );
    model.transcript_mut().clear();
    model.sync_transcript_render();
    model.set_window(width, height);
    let palette = if has_dark_background {
        default_palette()
    } else {
        palette_from_background(false, None)
    };
    model.set_palette(palette, has_dark_background);
    model
}

fn assert_frame_golden(model: &mut Model, width: u16, height: u16, expected: &str) {
    let area = Rect::new(0, 0, width, height);
    let mut buffer = Buffer::empty(area);
    let cursor = model.render_to_buffer_at(std::time::Instant::now(), area, &mut buffer);
    let actual = canonical_frame_snapshot(&buffer, cursor);

    if actual != expected {
        panic!("frame golden mismatch; actual snapshot follows:\n{actual}");
    }
}

fn canonical_frame_snapshot(buffer: &Buffer, cursor: Option<Position>) -> String {
    let mut snapshot = String::new();
    let _ = writeln!(
        snapshot,
        "size={}x{}",
        buffer.area.width, buffer.area.height
    );
    match cursor {
        Some(cursor) => {
            let _ = writeln!(snapshot, "cursor={},{}", cursor.x, cursor.y);
        }
        None => snapshot.push_str("cursor=none\n"),
    }
    snapshot.push_str("cells:\n");

    for row_offset in 0..buffer.area.height {
        let y = buffer.area.y + row_offset;
        let _ = write!(snapshot, "{row_offset:02}|");
        let mut continuation_cells = 0usize;
        for column_offset in 0..buffer.area.width {
            let x = buffer.area.x + column_offset;
            let cell = &buffer[(x, y)];
            if continuation_cells > 0 {
                continuation_cells -= 1;
                continue;
            }
            if matches!(cell.diff_option, CellDiffOption::Skip) {
                continue;
            }
            if cell.symbol() == " " {
                snapshot.push('·');
            } else {
                snapshot.push_str(cell.symbol());
            }
            continuation_cells = display_width(cell.symbol()).saturating_sub(1);
        }
        snapshot.push_str("|\n");
    }

    snapshot.push_str("styles:\n");
    let default_signature = style_signature(&Cell::default());
    for row_offset in 0..buffer.area.height {
        let y = buffer.area.y + row_offset;
        let mut run_start = 0u16;
        let first = &buffer[(buffer.area.x, y)];
        let mut run_signature = style_signature(first);

        for column_offset in 1..=buffer.area.width {
            let next_signature = (column_offset < buffer.area.width)
                .then(|| style_signature(&buffer[(buffer.area.x + column_offset, y)]));
            if next_signature.as_ref() == Some(&run_signature) {
                continue;
            }

            if run_signature != default_signature {
                let run_end = column_offset - 1;
                let _ = writeln!(
                    snapshot,
                    "{row_offset:02}|{run_start:02}..{run_end:02} {run_signature}"
                );
            }

            if let Some(next_signature) = next_signature {
                run_start = column_offset;
                run_signature = next_signature;
            }
        }
    }

    snapshot
}

fn style_signature(cell: &Cell) -> String {
    format!(
        "fg={:?} bg={:?} underline={:?} modifiers={:?} diff={:?}",
        cell.fg, cell.bg, cell.underline_color, cell.modifier, cell.diff_option
    )
}
