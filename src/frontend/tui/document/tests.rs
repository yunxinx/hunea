use crossterm::event::{KeyCode, KeyEvent};
use ratatui::text::Line;
use std::hint::black_box;
use std::rc::Rc;

use super::layout::{compose_document_layout, compose_document_viewport, visible_document_lines};
use super::*;
use crate::frontend::tui::{
    HeroOptions, Model, Sender, StatusLineItem, StyleMode,
    selection::SelectableLineRange as DocumentSelectable, theme::default_palette,
};

#[test]
fn build_document_layout_combines_transcript_and_composer_snapshots() {
    let mut model = ready_document_model(20, 4);
    model
        .transcript_mut()
        .append_message(Sender::Assistant, "history");
    model.sync_transcript_render();
    model.composer_mut().set_text_for_test("x");
    model.sync_composer_height();

    let layout = model.build_document_layout();

    assert_eq!(
        layout.all_plain_lines(),
        vec!["history".to_string(), String::new(), "┃ x".to_string(),]
    );
    assert_eq!(layout.composer_start_line, 2);
    assert_eq!(layout.composer_line_count, 1);
    assert_eq!(layout.cursor_x, 3);
    assert_eq!(layout.cursor_y, 2);
    assert_eq!(
        layout.line_at(1).map(|line| line.selectable),
        Some(DocumentSelectable::default())
    );
    assert!(
        layout
            .line_at(2)
            .is_some_and(|line| line.selectable.has_content())
    );
}

#[test]
fn composed_document_layout_and_viewport_match_the_model_snapshot_behavior() {
    let mut model = ready_document_model(20, 4);
    model
        .transcript_mut()
        .append_message(Sender::Assistant, "history");
    model.sync_transcript_render();
    model.composer_mut().set_text_for_test("x");
    model.sync_composer_height();

    let input = model.current_document_layout_input();
    let layout = compose_document_layout(input);
    let viewport = compose_document_viewport(&layout, 0, 4);

    assert_eq!(
        layout.all_plain_lines(),
        vec!["history".to_string(), String::new(), "┃ x".to_string()]
    );
    assert_eq!(layout.composer_start_line, 2);
    assert_eq!(layout.composer_line_count, 1);
    assert_eq!(layout.cursor_x, 3);
    assert_eq!(layout.cursor_y, 2);
    assert_eq!(viewport.resolved_offset, 0);
    assert_eq!(
        viewport.plain_lines,
        vec!["history".to_string(), String::new(), "┃ x".to_string()]
    );
}

#[test]
fn status_line_selectable_range_skips_leading_inset() {
    let mut model = ready_document_model(20, 4);
    model.status_line_items = vec![StatusLineItem::GitBranch];
    model.git_branch = "main".to_string();

    let layout = model.build_document_layout();
    let status_line = (0..layout.line_count())
        .find(|index| {
            layout
                .line_anchor_at(*index)
                .is_some_and(|anchor| matches!(anchor.region, DocumentAnchorRegion::StatusLine))
        })
        .expect("status line should be present");

    assert_eq!(
        layout
            .line_at(status_line)
            .and_then(|line| line.selectable.content_columns().map(|(start, _)| start)),
        Some(2)
    );
}

#[test]
fn visible_document_lines_tracks_cursor_visibility() {
    let layout = DocumentLayout {
        tail_lines: vec![Line::raw("a"), Line::raw("b"), Line::raw("c")],
        tail_plain_lines: vec!["a".to_string(), "b".to_string(), "c".to_string()],
        cursor_x: 4,
        cursor_y: 1,
        ..DocumentLayout::default()
    };

    let (visible_lines, _, visible_offset) = visible_document_lines(&layout, 0, 2);
    assert_eq!(visible_lines.len(), 2);
    assert_eq!(visible_offset, 0);
    assert!(cursor_visible_in_document_viewport(
        &layout,
        visible_offset,
        visible_lines.len()
    ));

    let (hidden_lines, _, hidden_offset) = visible_document_lines(&layout, 2, 1);
    assert_eq!(hidden_lines.len(), 1);
    assert_eq!(hidden_offset, 2);
    assert!(!cursor_visible_in_document_viewport(
        &layout,
        hidden_offset,
        hidden_lines.len()
    ));
}

#[test]
fn manual_scroll_restore_state_tracks_target_specific_anchor() {
    let anchor = DocumentViewportAnchor {
        line_text: "anchor".to_string(),
        transcript_item_line_count: 3,
        ..DocumentViewportAnchor::default()
    };
    let mut restore = RestoreState::default();

    assert_eq!(restore.target(), ManualDocumentScrollRestoreTarget::None);
    assert_eq!(restore.anchor(), &DocumentViewportAnchor::default());
    assert!(!restore.is_pending());

    restore.track_bottom_follow();
    assert_eq!(
        restore.target(),
        ManualDocumentScrollRestoreTarget::BottomFollow
    );
    assert_eq!(restore.anchor(), &DocumentViewportAnchor::default());
    assert!(restore.is_pending());

    restore.track_composer_cursor(Some(anchor.clone()));
    assert_eq!(
        restore.target(),
        ManualDocumentScrollRestoreTarget::ComposerCursor
    );
    assert_eq!(restore.anchor(), &anchor);

    restore.track_composer_cursor(None);
    assert_eq!(
        restore.target(),
        ManualDocumentScrollRestoreTarget::ComposerCursor
    );
    assert_eq!(restore.anchor(), &DocumentViewportAnchor::default());

    restore.clear();
    assert_eq!(restore.target(), ManualDocumentScrollRestoreTarget::None);
    assert_eq!(restore.anchor(), &DocumentViewportAnchor::default());
    assert!(!restore.is_pending());
}

#[test]
fn scroll_document_by_restores_composer_viewport_when_crossing_restore_target() {
    let mut model = ready_document_model(20, 4);
    model.composer_mut().set_text_for_test("1\n2\n3\n4\n5\n6");
    model.sync_composer_height();
    model.document_viewport_y = 0;
    model.composer.set_viewport_offset(0);
    model.follow_bottom = false;
    model.manual_document_scroll = true;
    model
        .manual_scroll_restore
        .track_composer_cursor(Some(DocumentViewportAnchor::default()));

    model.scroll_document_by(Model::document_mouse_wheel_delta());

    assert!(!model.follow_bottom);
    assert!(!model.manual_document_scroll);
    assert_eq!(model.document_viewport_y, 2);
    assert_eq!(model.composer.viewport_offset(), 2);
    assert_eq!(
        model.manual_scroll_restore.target(),
        ManualDocumentScrollRestoreTarget::None
    );
}

#[test]
fn moving_cursor_back_to_draft_end_restores_bottom_follow() {
    let mut model = ready_document_model(20, 4);
    model.composer_mut().set_text_for_test("1\n2\n3\n4\n5\n6");
    model.sync_composer_height();
    model.composer_mut().handle_key(KeyEvent::from(KeyCode::Up));
    model.composer_mut().handle_key(KeyEvent::from(KeyCode::Up));
    model.follow_bottom = false;
    model.manual_document_scroll = false;
    model.sync_document_viewport_for_composer_cursor();

    let old_value = model.composer_text().to_string();
    let old_line = model.composer.line();
    let old_column = model.composer.column();

    model.composer_mut().move_to_end();
    model.sync_composer_height();
    let layout = model.build_document_layout();
    let (expected_document_offset, expected_composer_offset) =
        model.bottom_follow_viewport_offsets(&layout);

    model.sync_document_viewport_after_composer_interaction(&old_value, old_line, old_column);

    assert!(model.follow_bottom);
    assert!(!model.manual_document_scroll);
    assert_eq!(model.document_viewport_y, expected_document_offset);
    assert_eq!(model.composer.viewport_offset(), expected_composer_offset);
}

#[test]
fn transcript_refresh_keeps_manual_scrollback_before_restore_target() {
    let mut model = ready_document_model(20, 4);
    for index in 0..8 {
        model
            .transcript_mut()
            .append_message(Sender::Assistant, format!("history {index}"));
    }
    model.sync_transcript_render();

    model.follow_bottom = false;
    model.manual_document_scroll = true;
    model.document_viewport_y = 0;
    model.composer.set_viewport_offset(0);
    model.manual_scroll_restore.track_bottom_follow();

    let anchor = model
        .current_document_viewport_anchor()
        .expect("manual scrollback should have a viewport anchor");
    let original_document_offset = model.document_viewport_y;
    let original_composer_offset = model.composer.viewport_offset();

    model
        .transcript_mut()
        .append_message(Sender::Assistant, "new history line");
    model.sync_transcript_render();
    model.sync_document_viewport_after_transcript_refresh(Some(anchor));

    assert!(!model.follow_bottom);
    assert!(model.manual_document_scroll);
    assert_eq!(model.document_viewport_y, original_document_offset);
    assert_eq!(model.composer.viewport_offset(), original_composer_offset);
    assert_eq!(
        model.manual_scroll_restore.target(),
        ManualDocumentScrollRestoreTarget::BottomFollow
    );
}

#[test]
fn build_document_layout_matches_full_compose_after_transcript_append() {
    let mut model = ready_document_model(24, 6);
    model.composer_mut().set_text_for_test("draft");
    model.sync_composer_height();

    let _ = model.build_document_layout();

    model
        .transcript_mut()
        .append_message(Sender::Assistant, "history");
    model.sync_transcript_render();

    let key = model.current_document_layout_key();
    let (layout, _) = model
        .build_document_layout_from_transcript_append(&key)
        .expect("single append should extend the cached document layout");
    let expected = compose_document_layout(model.current_document_layout_input());

    assert_eq!(layout.all_plain_lines(), expected.all_plain_lines());
    assert_eq!(layout.all_line_anchors(), expected.all_line_anchors());
    assert_eq!(layout.tail_selectable, expected.tail_selectable);
    assert_eq!(layout.composer_slot, expected.composer_slot);
    assert_eq!(layout.cursor_y, expected.cursor_y);
}

#[test]
fn build_document_layout_cache_hit_reuses_cached_allocation() {
    let mut model = ready_document_model(24, 6);
    model
        .transcript_mut()
        .append_message(Sender::Assistant, "history");
    model.sync_transcript_render();
    model.composer_mut().set_text_for_test("draft");
    model.sync_composer_height();

    let first = model.build_document_layout();
    let second = model.build_document_layout();

    assert!(Rc::ptr_eq(&first, &second));
}

#[test]
fn build_document_viewport_cache_hit_reuses_cached_allocation() {
    let mut model = ready_document_model(24, 6);
    model
        .transcript_mut()
        .append_message(Sender::Assistant, "history");
    model.sync_transcript_render();
    model.composer_mut().set_text_for_test("draft");
    model.sync_composer_height();

    let layout = model.build_document_layout();
    let first = model.build_document_viewport(&layout);
    let second = model.build_document_viewport(&layout);

    assert!(Rc::ptr_eq(&first, &second));
}

#[test]
fn build_document_viewport_tracks_plain_text_len_without_recomputing_from_lines() {
    let mut model = ready_document_model(24, 6);
    model
        .transcript_mut()
        .append_message(Sender::Assistant, "history");
    model.sync_transcript_render();
    model.composer_mut().set_text_for_test("draft");
    model.sync_composer_height();

    let layout = model.build_document_layout();
    let viewport = model.build_document_viewport(&layout);
    let expected = viewport.plain_lines.iter().map(String::len).sum::<usize>()
        + viewport.plain_lines.len().saturating_sub(1);

    assert_eq!(viewport.plain_text_len, expected);
}

#[test]
fn build_document_layout_matches_full_compose_after_appending_to_non_empty_transcript() {
    let mut model = ready_document_model(24, 6);
    model
        .transcript_mut()
        .append_message(Sender::Assistant, "history");
    model.sync_transcript_render();
    model.composer_mut().set_text_for_test("draft");
    model.sync_composer_height();

    let _ = model.build_document_layout();

    model
        .transcript_mut()
        .append_message(Sender::Assistant, "next");
    model.sync_transcript_render();

    let key = model.current_document_layout_key();
    let (layout, _) = model
        .build_document_layout_from_transcript_append(&key)
        .expect("tail append should extend before the composer gap");
    let expected = compose_document_layout(model.current_document_layout_input());

    assert_eq!(layout.all_plain_lines(), expected.all_plain_lines());
    assert_eq!(layout.all_line_anchors(), expected.all_line_anchors());
    assert_eq!(layout.tail_selectable, expected.tail_selectable);
    assert_eq!(layout.composer_slot, expected.composer_slot);
    assert_eq!(layout.cursor_y, expected.cursor_y);
}

#[test]
fn build_document_layout_append_path_rejects_multiple_pending_transcript_appends() {
    let mut model = ready_document_model(24, 6);
    model.composer_mut().set_text_for_test("draft");
    model.sync_composer_height();

    let _ = model.build_document_layout();

    model
        .transcript_mut()
        .append_message(Sender::Assistant, "one");
    model.sync_transcript_render();
    model
        .transcript_mut()
        .append_message(Sender::Assistant, "two");
    model.sync_transcript_render();

    let key = model.current_document_layout_key();
    assert!(
        model
            .build_document_layout_from_transcript_append(&key)
            .is_none(),
        "multiple pending transcript appends should fall back to full compose"
    );
}

#[test]
#[ignore = "performance smoke test"]
fn document_layout_and_viewport_perf_smoke() {
    let mut model = ready_document_model(80, 12);
    for index in 0..24 {
        let sender = if index % 3 == 0 {
            Sender::User
        } else {
            Sender::Assistant
        };
        model.transcript_mut().append_message(
            sender,
            format!("message {index:02}: keep scrollback anchored while the composer draft keeps growing"),
        );
    }
    model.sync_transcript_render();
    model.composer_mut().set_text_for_test(
        "draft heading\nsoft wrap should stay stable under repeated rendering\n中文输入继续参与宽度计算\ncursor placement should stay near the bottom",
    );
    model.sync_composer_height();

    for _ in 0..128 {
        let layout = black_box(model.build_document_layout());
        black_box(model.build_document_viewport(&layout));
        model.invalidate_document_caches_for_test();
    }
}

#[test]
fn transcript_render_defers_transcript_selectable_ranges_to_document_access() {
    let mut model = ready_document_model(24, 6);
    model
        .transcript_mut()
        .append_message(Sender::User, "alpha beta gamma");

    model.sync_transcript_render();

    assert!(
        model.transcript_render.selectable_ranges.is_empty(),
        "transcript render should leave transcript selectable ranges empty for lazy document access"
    );
}

#[test]
fn document_line_access_computes_transcript_selectable_ranges_lazily() {
    let mut model = ready_document_model(24, 6);
    model
        .transcript_mut()
        .append_message(Sender::User, "alpha beta gamma");
    model.sync_transcript_render();

    let layout = model.build_document_layout();
    assert_eq!(
        layout
            .transcript
            .selectable_cache
            .borrow()
            .get(&0)
            .cloned()
            .unwrap_or_default()
            .len(),
        0
    );

    let selectable_line_index = (0..layout.transcript_line_count)
        .find(|index| {
            layout
                .line_at(*index)
                .is_some_and(|line| line.selectable.has_content())
        })
        .expect("transcript lines should include selectable content");
    let line = layout
        .line_at(selectable_line_index)
        .expect("selectable transcript line should exist");
    assert!(
        line.selectable.has_content(),
        "line_at should compute a selectable range, got {:?}, cache={:?}",
        line,
        layout.transcript.selectable_cache.borrow()
    );
    assert_eq!(
        layout
            .transcript
            .selectable_cache
            .borrow()
            .get(&0)
            .cloned()
            .unwrap_or_default()
            .len(),
        3
    );
}

fn ready_document_model(width: u16, height: u16) -> Model {
    let mut model = Model::new_with_style_mode(HeroOptions::default(), StyleMode::Ms);
    model.transcript_mut().clear();
    model.sync_transcript_render();
    model.set_window(width, height);
    model.set_palette(default_palette(), true);
    model
}

fn cursor_visible_in_document_viewport(
    layout: &DocumentLayout,
    resolved_offset: usize,
    visible_line_count: usize,
) -> bool {
    layout.cursor_y >= resolved_offset
        && layout.cursor_y < resolved_offset.saturating_add(visible_line_count)
}
