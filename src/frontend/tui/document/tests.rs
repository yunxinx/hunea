use crossterm::event::{KeyCode, KeyEvent};
use ratatui::text::Line;
use std::hint::black_box;
use std::rc::Rc;

use super::layout::{compose_document_layout, compose_document_viewport, visible_document_lines};
use super::*;
use crate::frontend::tui::{
    HeroOptions, Model, Sender, StatusLineItem, StyleMode,
    selection::SelectableLineRange as DocumentSelectable, theme::default_palette,
    transcript::LineAnchorKind,
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
fn document_tail_layout_cache_reuses_tail_when_transcript_append_keeps_tail_inputs_stable() {
    let mut model = ready_document_model(20, 4);
    model
        .transcript_mut()
        .append_message(Sender::Assistant, "history");
    model.sync_transcript_render();
    model.composer_mut().set_text_for_test("draft");
    model.sync_composer_height();

    let first = model.build_document_tail_layout();

    model
        .transcript_mut()
        .append_message(Sender::Assistant, "new history");
    model.sync_transcript_render();

    let second = model.build_document_tail_layout();

    assert!(
        Rc::ptr_eq(&first, &second),
        "tail layout should stay cached when transcript append does not change tail inputs"
    );
}

#[test]
fn document_layout_cache_invalidates_on_height_only_resize_when_command_panel_rows_change() {
    let mut model = ready_document_model(20, 4);
    model.composer_mut().set_text_for_test("/");
    model.sync_command_panel_navigation();
    model.sync_composer_height();

    let first = model.build_document_layout();
    let first_command_panel_rows = first
        .all_line_anchors()
        .into_iter()
        .filter(|anchor| anchor.region == DocumentAnchorRegion::CommandPanel)
        .count();

    model.set_window(20, 10);

    let second = model.build_document_layout();
    let second_command_panel_rows = second
        .all_line_anchors()
        .into_iter()
        .filter(|anchor| anchor.region == DocumentAnchorRegion::CommandPanel)
        .count();

    assert!(
        !Rc::ptr_eq(&first, &second),
        "height-only resize should invalidate the layout cache when command panel rows depend on viewport height"
    );
    assert_eq!(first_command_panel_rows, 3);
    assert_eq!(second_command_panel_rows, 7);
    assert_eq!(
        second_command_panel_rows,
        model.command_panel_list_visible_rows(),
        "layout should rebuild command panel rows for the new viewport height"
    );
}

#[test]
fn document_tail_layout_cache_invalidates_on_height_only_resize_when_command_panel_rows_change() {
    let mut model = ready_document_model(20, 4);
    model.composer_mut().set_text_for_test("/");
    model.sync_command_panel_navigation();
    model.sync_composer_height();

    let first = model.build_document_tail_layout();
    let first_command_panel_rows = first
        .anchors
        .iter()
        .filter(|anchor| anchor.region == DocumentAnchorRegion::CommandPanel)
        .count();

    model.set_window(20, 10);

    let second = model.build_document_tail_layout();
    let second_command_panel_rows = second
        .anchors
        .iter()
        .filter(|anchor| anchor.region == DocumentAnchorRegion::CommandPanel)
        .count();

    assert!(
        !Rc::ptr_eq(&first, &second),
        "height-only resize should invalidate the tail cache when command panel rows depend on viewport height"
    );
    assert_eq!(first_command_panel_rows, 3);
    assert_eq!(second_command_panel_rows, 7);
    assert_eq!(
        second_command_panel_rows,
        model.command_panel_list_visible_rows(),
        "tail should rebuild command panel rows for the new viewport height"
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
fn current_document_transcript_snapshot_reuses_render_storage() {
    let mut model = ready_document_model(20, 4);
    model.transcript_mut().clear();
    model
        .transcript_mut()
        .append_message(Sender::Assistant, "alpha\nbeta\ngamma");
    model.sync_transcript_render();

    let snapshot = model.current_document_transcript_snapshot();

    assert!(
        Rc::ptr_eq(&snapshot.render, &model.transcript_render),
        "document transcript snapshot should share the transcript render result instead of cloning it"
    );
}

#[test]
fn transcript_line_access_uses_structured_render_result() {
    let mut model = ready_document_model(20, 4);
    model.transcript_mut().clear();
    model
        .transcript_mut()
        .append_message(Sender::Assistant, "alpha\nbeta");
    model.sync_transcript_render();

    let layout = model.build_document_layout();
    let first_line = layout
        .line_at(0)
        .expect("transcript line should resolve from the structured render result");

    assert_eq!(first_line.plain_line, "alpha");
    assert_eq!(first_line.anchor.transcript.item_index, 0);
    assert_eq!(
        layout.transcript.render.all_line_anchors()[1]
            .item_anchor
            .rendered_line,
        1
    );
}

#[test]
fn visible_document_lines_tracks_cursor_visibility() {
    let layout = DocumentLayout {
        tail: Rc::new(DocumentTailLayout {
            lines: vec![Line::raw("a"), Line::raw("b"), Line::raw("c")],
            text_lines: vec!["a".to_string(), "b".to_string(), "c".to_string()],
            ..DocumentTailLayout::default()
        }),
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
    let viewport_state = ViewportState::bottom_follow(3, 4, 20);
    let mut restore = RestoreState::default();

    assert_eq!(restore.target(), ManualDocumentScrollRestoreTarget::None);
    assert_eq!(restore.viewport_state(), &ViewportState::default());
    assert!(!restore.is_pending());

    restore.track_bottom_follow();
    assert_eq!(
        restore.target(),
        ManualDocumentScrollRestoreTarget::BottomFollow
    );
    assert_eq!(restore.viewport_state(), &ViewportState::default());
    assert!(restore.is_pending());

    restore.track_composer_cursor(Some(viewport_state.clone()));
    assert_eq!(
        restore.target(),
        ManualDocumentScrollRestoreTarget::ComposerCursor
    );
    assert_eq!(restore.viewport_state(), &viewport_state);

    restore.track_composer_cursor(None);
    assert_eq!(
        restore.target(),
        ManualDocumentScrollRestoreTarget::ComposerCursor
    );
    assert_eq!(restore.viewport_state(), &ViewportState::default());

    restore.clear();
    assert_eq!(restore.target(), ManualDocumentScrollRestoreTarget::None);
    assert_eq!(restore.viewport_state(), &ViewportState::default());
    assert!(!restore.is_pending());
}

#[test]
fn scroll_document_by_restores_composer_viewport_when_crossing_restore_target() {
    let mut model = ready_document_model(20, 4);
    model.composer_mut().set_text_for_test("1\n2\n3\n4\n5\n6");
    model.sync_composer_height();
    model.sync_document_viewport_for_composer_cursor();
    let restore_viewport_state = model.current_document_viewport_state();
    let layout = model.build_document_layout();
    model.apply_document_viewport_position(&layout, 0, 0, false, true);
    model
        .manual_scroll_restore
        .track_composer_cursor(Some(restore_viewport_state));

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

    let layout = model.build_document_layout();
    model.apply_document_viewport_position(&layout, 0, 0, false, true);
    model.manual_scroll_restore.track_bottom_follow();

    let preserved_viewport_state = model.current_document_viewport_state();
    let original_document_offset = model.document_viewport_y;
    let original_composer_offset = model.composer.viewport_offset();

    model
        .transcript_mut()
        .append_message(Sender::Assistant, "new history line");
    model.sync_transcript_render();
    model.sync_document_viewport_after_transcript_refresh(Some(preserved_viewport_state));

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
fn transcript_viewport_anchor_classifies_rendered_lines_by_semantic_position() {
    let mut model = ready_document_model(18, 3);
    model.transcript_mut().append_message(
        Sender::Assistant,
        "alpha beta gamma delta epsilon zeta eta theta iota kappa",
    );
    model.sync_transcript_render();

    let layout = model.build_document_layout();
    let item_lines = layout
        .transcript_item_lines(0)
        .expect("wrapped assistant item should produce transcript lines");
    assert!(
        item_lines.content_line_count > 1,
        "test fixture should wrap into multiple rendered lines"
    );

    let anchor = document_viewport_anchor_at_line(
        &layout,
        item_lines.content_start_line + item_lines.content_line_count - 1,
    )
    .expect("last transcript line should produce a viewport anchor");

    assert_eq!(
        anchor.transcript_semantic_position,
        TranscriptSemanticPosition::End
    );
}

#[test]
fn viewport_state_preserves_semantic_anchor_offset_across_transcript_append() {
    let mut model = ready_document_model(20, 2);
    model
        .transcript_mut()
        .append_message(Sender::Assistant, "history");
    model.sync_transcript_render();
    model.composer_mut().set_text_for_test("draft");
    model.sync_composer_height();

    let layout = model.build_document_layout();
    let state = ViewportState::capture(
        &layout,
        &[1, 2],
        1,
        false,
        true,
        model.document_viewport_height(),
        model.width,
    );

    assert_eq!(state.anchor_viewport_offset(), 1);
    assert!(matches!(
        state.anchor(),
        ViewAnchor::Line(anchor)
            if anchor.line_anchor.region == DocumentAnchorRegion::Composer
    ));

    model
        .transcript_mut()
        .append_message(Sender::Assistant, "new history");
    model.sync_transcript_render();
    let updated_layout = model.build_document_layout();

    assert_eq!(
        state.resolve_offset(&updated_layout, model.document_viewport_height()),
        3,
        "appending transcript content above the anchor should keep the same semantic line on the same viewport row"
    );
}

#[test]
fn viewport_state_restores_rendered_transcript_anchor_by_semantic_position_after_resize() {
    let mut model = ready_document_model(18, 3);
    model.transcript_mut().append_message(
        Sender::Assistant,
        "alpha beta gamma delta epsilon zeta eta theta iota kappa",
    );
    model.sync_transcript_render();

    let layout = model.build_document_layout();
    let item_lines = layout
        .transcript_item_lines(0)
        .expect("wrapped assistant item should produce transcript lines");
    let document_offset = item_lines.content_start_line + item_lines.content_line_count - 1;
    let state = model.capture_viewport_state_with_layout(&layout, document_offset, false, true);

    assert!(matches!(
        state.anchor(),
        ViewAnchor::Line(anchor)
            if anchor.transcript_semantic_position == TranscriptSemanticPosition::End
    ));

    model.set_window(12, 3);
    let resized_layout = model.build_document_layout();
    let resolved_offset = state.resolve_offset(&resized_layout, model.document_viewport_height());
    let restored_anchor = document_viewport_anchor_at_line(&resized_layout, resolved_offset)
        .expect("resolved viewport offset should still point at a transcript anchor");

    assert_eq!(restored_anchor.line_anchor.transcript.item_index, 0);
    assert_eq!(
        restored_anchor.transcript_semantic_position,
        TranscriptSemanticPosition::End
    );
}

#[test]
fn viewport_state_restores_transcript_separator_anchor_after_resize() {
    let mut model = ready_document_model(18, 1);
    model.transcript_mut().append_message(
        Sender::Assistant,
        "alpha beta gamma delta epsilon zeta eta theta iota kappa",
    );
    model
        .transcript_mut()
        .append_message(Sender::Assistant, "omega");
    model.sync_transcript_render();

    let layout = model.build_document_layout();
    let separator_offset = (0..layout.line_count())
        .find(|&index| {
            layout.line_anchor_at(index).is_some_and(|anchor| {
                matches!(anchor.region, DocumentAnchorRegion::Transcript)
                    && anchor.transcript.item_index == 0
                    && matches!(anchor.transcript.item_anchor.kind, LineAnchorKind::ItemGap)
            })
        })
        .expect("separator line should exist between transcript items");
    let state = model.capture_viewport_state_with_layout(&layout, separator_offset, false, true);

    assert!(matches!(
        state.anchor(),
        ViewAnchor::Line(anchor)
            if matches!(anchor.line_anchor.region, DocumentAnchorRegion::Transcript)
                && anchor.line_anchor.transcript.item_index == 0
                && matches!(anchor.line_anchor.transcript.item_anchor.kind, LineAnchorKind::ItemGap)
    ));

    model.set_window(12, 1);
    let resized_layout = model.build_document_layout();
    let expected_separator_offset = (0..resized_layout.line_count())
        .find(|&index| {
            resized_layout.line_anchor_at(index).is_some_and(|anchor| {
                matches!(anchor.region, DocumentAnchorRegion::Transcript)
                    && anchor.transcript.item_index == 0
                    && matches!(anchor.transcript.item_anchor.kind, LineAnchorKind::ItemGap)
            })
        })
        .expect("separator line should survive resize");
    assert_ne!(
        separator_offset, expected_separator_offset,
        "test fixture should move the separator after resize"
    );

    let resolved_offset = state.resolve_offset(&resized_layout, model.document_viewport_height());
    let restored_anchor = document_viewport_anchor_at_line(&resized_layout, resolved_offset)
        .expect("resolved viewport offset should still point at a transcript line");

    assert_eq!(resolved_offset, expected_separator_offset);
    assert_eq!(restored_anchor.line_anchor.transcript.item_index, 0);
    assert!(matches!(
        restored_anchor.line_anchor.transcript.item_anchor.kind,
        LineAnchorKind::ItemGap
    ));
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

    let layout = model.build_document_layout();
    let expected = compose_document_layout(model.current_document_layout_input());

    assert_eq!(layout.all_plain_lines(), expected.all_plain_lines());
    assert_eq!(layout.all_line_anchors(), expected.all_line_anchors());
    assert_eq!(layout.tail.selectable, expected.tail.selectable);
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

    let layout = model.build_document_layout();
    let expected = compose_document_layout(model.current_document_layout_input());

    assert_eq!(layout.all_plain_lines(), expected.all_plain_lines());
    assert_eq!(layout.all_line_anchors(), expected.all_line_anchors());
    assert_eq!(layout.tail.selectable, expected.tail.selectable);
    assert_eq!(layout.composer_slot, expected.composer_slot);
    assert_eq!(layout.cursor_y, expected.cursor_y);
}

#[test]
fn build_document_layout_after_multiple_pending_transcript_appends_matches_compose_document_layout()
{
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

    let layout = model.build_document_layout();
    let expected = compose_document_layout(model.current_document_layout_input());

    assert_eq!(layout.all_plain_lines(), expected.all_plain_lines());
    assert_eq!(layout.all_line_anchors(), expected.all_line_anchors());
    assert_eq!(layout.tail.selectable, expected.tail.selectable);
    assert_eq!(layout.composer_slot, expected.composer_slot);
    assert_eq!(layout.cursor_y, expected.cursor_y);
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
