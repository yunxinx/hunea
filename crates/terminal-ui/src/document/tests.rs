use crossterm::event::{KeyCode, KeyEvent};
use ratatui::text::Line;
use std::{cell::RefCell, collections::HashMap, hint::black_box, rc::Rc};

use super::layout::{
    DocumentLayoutInput, compose_document_layout, compose_document_viewport, visible_document_lines,
};
use super::slot_frame::SlotFrame;
use super::*;
use crate::{
    Model, Sender, StartupBannerOptions, StatusLineItem, StyleMode,
    frame_time::FrameRenderContext,
    selection::SelectableLineRange as DocumentSelectable,
    theme::{default_palette, terminal_default_palette},
    tool_approval_panel::ToolApprovalSource,
    transcript::{
        CachedLineAnchors, CachedRenderBlock, ItemLineAnchor, LineAnchorKind, RenderItemSummary,
        TranscriptItemMetricsIndex, new_render_result, reset_tracked_cached_render_block_access,
        tracked_cached_render_block_access,
    },
};
use runtime_domain::session::RuntimeTarget;

#[test]
fn document_layout_owns_the_exact_frame_key_used_by_viewport_cache() {
    let mut model = ready_document_model(40, 8);
    model.show_stream_activity_with_header("Working");
    let context = FrameRenderContext::new(std::time::Instant::now());
    let expected_key = model.current_document_layout_key(context);

    let layout = model.build_document_layout(context);
    let _viewport = model.build_document_viewport(&layout, context);

    assert_eq!(layout.key(), &expected_key);
    assert_eq!(
        &model.document_runtime.viewport_cache.key.layout_key,
        layout.key(),
        "viewport cache must reuse the key that actually produced the layout",
    );
}

#[test]
fn build_document_layout_combines_transcript_and_composer_snapshots() {
    let mut model = ready_document_model(20, 4);
    model
        .transcript_mut()
        .append_message(Sender::Assistant, "history");
    model.sync_transcript_render();
    model.composer_mut().set_text_for_test("x");
    model.sync_composer_height();

    let layout = model.build_document_layout(crate::frame_time::FrameRenderContext::capture());

    assert_eq!(
        layout.all_plain_lines(FrameRenderContext::capture()),
        vec!["history".to_string(), String::new(), "┃ x".to_string(),]
    );
    assert_eq!(layout.composer_start_line, 2);
    assert_eq!(layout.composer_line_count, 1);
    assert_eq!(layout.cursor_x, 3);
    assert_eq!(layout.cursor_y, 2);
    assert_eq!(
        layout
            .line_at(1, FrameRenderContext::capture())
            .map(|line| line.selectable),
        Some(DocumentSelectable::default())
    );
    assert!(
        layout
            .line_at(2, FrameRenderContext::capture())
            .is_some_and(|line| line.selectable.has_content())
    );
}

#[test]
fn segmented_tail_accessors_cross_activity_boundaries_without_flattening() {
    let mut model = ready_document_model(40, 8);
    model
        .transcript_mut()
        .append_message(Sender::Assistant, "history");
    model.sync_transcript_render();
    model.composer_mut().set_text_for_test("draft");
    model.sync_composer_height();
    model.show_stream_activity_with_header("Working");
    let context = FrameRenderContext::capture();
    let layout = model.build_document_layout(context);
    let tail_start = layout.transcript_line_count;
    let tail_line_count = layout.tail.line_count();
    let text_lines = layout.line_texts_for_range(tail_start, tail_line_count, context);
    let styled_lines = layout
        .lines_for_range(tail_start, tail_line_count, context)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();
    let anchors = layout.tail.all_anchors();

    assert_eq!(tail_line_count, 4);
    assert_eq!(styled_lines, text_lines);
    assert_eq!(text_lines[0], "");
    assert!(text_lines[1].contains("Working"));
    assert_eq!(text_lines[2], "");
    assert_eq!(text_lines[3], "┃ draft");
    assert_eq!(
        anchors
            .iter()
            .map(|anchor| anchor.region)
            .collect::<Vec<_>>(),
        vec![
            DocumentAnchorRegion::TranscriptComposerGap,
            DocumentAnchorRegion::StreamActivity,
            DocumentAnchorRegion::StreamActivityComposerGap,
            DocumentAnchorRegion::Composer,
        ]
    );
    assert_eq!(
        layout.plain_text_len_for_range(tail_start, tail_line_count, context),
        text_lines.join("\n").len()
    );

    for (tail_index, anchor) in anchors.into_iter().enumerate() {
        assert_eq!(
            layout.line_index_for_anchor(anchor, context),
            Some(tail_start + tail_index)
        );
        assert_eq!(
            layout
                .selection_line_at(tail_start + tail_index, context)
                .map(|line| (line.anchor, line.selectable)),
            Some((
                anchor,
                layout.tail.selectable_at(tail_index).unwrap_or_default()
            ))
        );
    }
}

#[test]
fn tail_full_range_plain_text_len_matches_line_materialization_for_multiline_composer() {
    let mut model = ready_document_model(40, 8);
    model
        .composer_mut()
        .set_text_for_test("draft one\ndraft two\ndraft three");
    model.sync_composer_height();
    model.show_stream_activity_with_header("Working");
    let context = FrameRenderContext::capture();
    let layout = model.build_document_layout(context);
    let tail_start = layout.transcript_line_count;
    let tail_line_count = layout.tail.line_count();
    let text_lines = layout.line_texts_for_range(tail_start, tail_line_count, context);

    assert_eq!(
        layout.plain_text_len_for_range(tail_start, tail_line_count, context),
        text_lines.join("\n").len(),
    );
}

#[test]
fn model_panel_activity_segment_has_no_composer_gap_or_slot_offset() {
    let mut model = ready_document_model(60, 24);
    model.show_stream_activity_with_header("Working");
    let stable_composer = model.build_document_tail_layout(FrameRenderContext::capture());
    model.open_model_panel();
    let context = FrameRenderContext::capture();
    let panel_tail = model.build_document_tail_layout(context);
    let regions = panel_tail
        .all_anchors()
        .into_iter()
        .map(|anchor| anchor.region)
        .collect::<Vec<_>>();

    assert!(!stable_composer.shares_stable_layout_with(&panel_tail));
    assert_eq!(regions.first(), Some(&DocumentAnchorRegion::StreamActivity));
    assert_eq!(regions.get(1), Some(&DocumentAnchorRegion::ModelPanel));
    assert!(!regions.contains(&DocumentAnchorRegion::StreamActivityComposerGap));
    assert_eq!(panel_tail.composer_slot, SlotFrame::empty());
    assert_eq!(
        panel_tail.cursor_y,
        panel_tail.line_count().saturating_add(1)
    );
}

#[test]
fn context_budget_activity_segment_has_no_composer_gap_or_slot_offset() {
    let mut model = ready_document_model(60, 24);
    model.show_stream_activity_with_header("Working");
    model.open_context_budget_loading();
    let tail = model.build_document_tail_layout(FrameRenderContext::capture());
    let regions = tail
        .all_anchors()
        .into_iter()
        .map(|anchor| anchor.region)
        .collect::<Vec<_>>();

    assert_eq!(regions.first(), Some(&DocumentAnchorRegion::StreamActivity));
    assert_eq!(
        regions.get(1),
        Some(&DocumentAnchorRegion::ContextBudgetPanel)
    );
    assert!(!regions.contains(&DocumentAnchorRegion::StreamActivityComposerGap));
    assert_eq!(tail.composer_slot, SlotFrame::empty());
    assert_eq!(tail.cursor_y, tail.line_count().saturating_add(1));
}

#[test]
fn tool_approval_activity_segment_has_no_composer_gap_or_slot_offset() {
    let mut model = ready_document_model(60, 24);
    model.show_stream_activity_with_header("Working");
    model.open_tool_approval_panel(
        ToolApprovalSource::RuntimePermission {
            target: RuntimeTarget::provider("local", "qwen3"),
            request_id: "permission-1".to_string(),
            allow_option_id: Some("allow-once".to_string()),
            allow_always_option_id: None,
            reject_option_id: Some("reject-once".to_string()),
            reject_always_option_id: None,
        },
        "Write file".to_string(),
        Vec::new(),
    );
    model.resume_stream_activity();
    let tail = model.build_document_tail_layout(FrameRenderContext::capture());
    let regions = tail
        .all_anchors()
        .into_iter()
        .map(|anchor| anchor.region)
        .collect::<Vec<_>>();

    assert_eq!(regions.first(), Some(&DocumentAnchorRegion::StreamActivity));
    assert_eq!(
        regions.get(1),
        Some(&DocumentAnchorRegion::ToolApprovalPanel)
    );
    assert!(!regions.contains(&DocumentAnchorRegion::StreamActivityComposerGap));
    assert_eq!(tail.composer_slot, SlotFrame::empty());
    assert_eq!(tail.cursor_y, tail.line_count().saturating_add(1));
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

    let first = model.build_document_tail_layout(crate::frame_time::FrameRenderContext::capture());

    model
        .transcript_mut()
        .append_message(Sender::Assistant, "new history");
    model.sync_transcript_render();

    let second = model.build_document_tail_layout(crate::frame_time::FrameRenderContext::capture());

    assert!(
        Rc::ptr_eq(&first, &second),
        "tail layout should stay cached when transcript append does not change tail inputs"
    );
}

#[test]
fn composer_content_change_invalidates_the_stable_tail_layout() {
    let mut model = ready_document_model(40, 8);
    model.composer_mut().set_text_for_test("first draft");
    model.sync_composer_height();
    let initial = model.build_document_tail_layout(FrameRenderContext::capture());

    model.composer_mut().set_text_for_test("second draft");
    model.sync_composer_height();
    let updated = model.build_document_tail_layout(FrameRenderContext::capture());

    assert!(
        !initial.shares_stable_layout_with(&updated),
        "composer content changes must invalidate the stable tail allocation"
    );
    assert!(
        (0..updated.line_count())
            .filter_map(|index| updated.text_line_at(index))
            .any(|line| line.contains("second draft"))
    );
}

#[test]
fn status_line_change_invalidates_the_stable_tail_layout() {
    let mut model = ready_document_model(40, 8);
    model.status_line_items = vec![StatusLineItem::GitBranch];
    model.git_branch = "main".to_string();
    let initial = model.build_document_tail_layout(FrameRenderContext::capture());

    model.git_branch = "feature/stable-tail".to_string();
    model.bump_status_line_revision();
    let updated = model.build_document_tail_layout(FrameRenderContext::capture());

    assert!(
        !initial.shares_stable_layout_with(&updated),
        "stable status rows must invalidate when their revision changes"
    );
    assert!(
        (0..updated.line_count())
            .filter_map(|index| updated.text_line_at(index))
            .any(|line| line.contains("feature/stable-tail"))
    );
}

#[test]
fn document_layout_cache_invalidates_on_height_only_resize_when_command_panel_rows_change() {
    let mut model = ready_document_model(20, 4);
    model.composer_mut().set_text_for_test("/");
    model.sync_command_panel_navigation();
    model.sync_composer_height();

    let first = model.build_document_layout(crate::frame_time::FrameRenderContext::capture());
    let first_command_panel_rows = first
        .all_line_anchors(FrameRenderContext::capture())
        .into_iter()
        .filter(|anchor| anchor.region == DocumentAnchorRegion::CommandPanel)
        .count();

    model.set_window(20, 10);

    let second = model.build_document_layout(crate::frame_time::FrameRenderContext::capture());
    let second_command_panel_rows = second
        .all_line_anchors(FrameRenderContext::capture())
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

    let first = model.build_document_tail_layout(crate::frame_time::FrameRenderContext::capture());
    let first_command_panel_rows = first
        .all_anchors()
        .into_iter()
        .filter(|anchor| anchor.region == DocumentAnchorRegion::CommandPanel)
        .count();

    model.set_window(20, 10);

    let second = model.build_document_tail_layout(crate::frame_time::FrameRenderContext::capture());
    let second_command_panel_rows = second
        .all_anchors()
        .into_iter()
        .filter(|anchor| anchor.region == DocumentAnchorRegion::CommandPanel)
        .count();

    assert!(
        !Rc::ptr_eq(&first, &second),
        "height-only resize should invalidate the tail cache when command panel rows depend on viewport height"
    );
    assert!(
        !first.shares_stable_layout_with(&second),
        "height-dependent command panel rows must invalidate the stable tail allocation"
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

    let context = FrameRenderContext::capture();
    let key = model.current_document_layout_key(context);
    let input = model.current_document_layout_input(context);
    let layout = compose_document_layout(key, input);
    let viewport = compose_document_viewport(&layout, 0, 4, context);

    assert_eq!(
        layout.all_plain_lines(FrameRenderContext::capture()),
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

    let layout = model.build_document_layout(crate::frame_time::FrameRenderContext::capture());
    let status_line = (0..layout.line_count())
        .find(|index| {
            layout
                .line_anchor_at(*index, FrameRenderContext::capture())
                .is_some_and(|anchor| matches!(anchor.region, DocumentAnchorRegion::StatusLine))
        })
        .expect("status line should be present");

    assert_eq!(
        layout
            .line_at(status_line, FrameRenderContext::capture())
            .and_then(|line| line.selectable.content_columns().map(|(start, _)| start)),
        Some(2)
    );
    assert_eq!(
        layout
            .line_at(status_line, FrameRenderContext::capture())
            .and_then(|line| line.selectable.hit_columns().map(|(start, _)| start)),
        Some(0)
    );
}

#[test]
fn second_status_line_viewport_anchor_resolves_to_second_status_line() {
    let mut model = ready_document_model(24, 5);
    model.status_line_items = vec![StatusLineItem::GitBranch];
    model.status_line_2_items = vec![StatusLineItem::CurrentDir];
    model.git_branch = "main".to_string();
    model.current_dir = "~/repo".to_string();

    let layout = model.build_document_layout(crate::frame_time::FrameRenderContext::capture());
    let status_lines = (0..layout.line_count())
        .filter_map(|index| {
            let anchor = layout.line_anchor_at(index, FrameRenderContext::capture())?;
            matches!(anchor.region, DocumentAnchorRegion::StatusLine).then_some((index, anchor))
        })
        .collect::<Vec<_>>();

    assert_eq!(status_lines.len(), 2);

    let (second_index, second_anchor) = status_lines[1];
    let viewport_anchor = DocumentViewportAnchor {
        line_anchor: second_anchor,
        line_text: layout
            .line_at(second_index, FrameRenderContext::capture())
            .map(|line| line.plain_line.clone())
            .unwrap_or_default(),
        ..DocumentViewportAnchor::default()
    };

    assert_eq!(
        anchor_match::find_document_offset_for_viewport_anchor(
            &layout,
            &viewport_anchor,
            FrameRenderContext::capture(),
        ),
        Some(second_index)
    );
}

#[test]
fn current_document_transcript_snapshot_stays_usable_without_full_render_storage() {
    let mut model = ready_document_model(20, 4);
    model.transcript_mut().clear();
    model
        .transcript_mut()
        .append_message(Sender::Assistant, "alpha\nbeta\ngamma");
    model.sync_transcript_render();
    model.transcript_render = Rc::new(crate::transcript::RenderResult::default());
    model.document_runtime.transcript_cache = Default::default();
    model.document_runtime.layout_cache = Default::default();
    model.document_runtime.viewport_cache = Default::default();

    let snapshot = model
        .current_document_transcript_snapshot(crate::frame_time::FrameRenderContext::capture());
    let layout = model.build_document_layout(crate::frame_time::FrameRenderContext::capture());
    let viewport = compose_document_viewport(&layout, 0, 2, FrameRenderContext::capture());

    assert_eq!(snapshot.index.line_count, 3);
    assert_eq!(
        viewport.plain_lines,
        vec!["alpha".to_string(), "beta".to_string()]
    );
}

#[test]
fn current_document_transcript_snapshot_reuses_warmed_transcript_blocks_without_cloning_cache() {
    let mut model = ready_document_model(20, 4);
    model.transcript_mut().clear();
    model
        .transcript_mut()
        .append_message(Sender::Assistant, "alpha");
    model
        .transcript_mut()
        .append_message(Sender::Assistant, "beta");
    model.sync_transcript_render();
    let warmed_render = model
        .transcript
        .render(crate::frame_time::FrameRenderContext::capture());

    let snapshot = model
        .current_document_transcript_snapshot(crate::frame_time::FrameRenderContext::capture());
    assert!(
        snapshot.item_block_cache.borrow().is_empty(),
        "document transcript snapshot should not clone the entire warmed transcript cache up front"
    );

    let first_line = snapshot
        .line_at(0, FrameRenderContext::capture())
        .expect("reading a transcript line should reuse the warmed block");
    assert_eq!(first_line.plain_line, "alpha");
    assert!(
        !warmed_render.items.is_empty(),
        "warmed render should still retain the original block"
    );
    assert_eq!(
        snapshot.item_block_cache.borrow().len(),
        0,
        "reading a warmed transcript line should not duplicate that block into the snapshot cache"
    );
}

#[test]
fn current_document_transcript_snapshot_keeps_old_palette_lines_after_palette_switch() {
    let mut model = ready_document_model(20, 4);
    model.transcript_mut().clear();
    model.transcript_mut().append_message(Sender::User, "hello");
    model.sync_transcript_render();

    let snapshot = model
        .current_document_transcript_snapshot(crate::frame_time::FrameRenderContext::capture());
    assert_eq!(
        snapshot.plain_lines_for_range(0, snapshot.line_count(), FrameRenderContext::capture(),),
        vec![
            "                    ".to_string(),
            "› hello             ".to_string(),
            "                    ".to_string(),
        ]
    );
    assert_eq!(
        snapshot.item_block_cache.borrow().len(),
        1,
        "viewport-local snapshot cache should pin the current overscan neighborhood without cloning the whole warmed cache"
    );

    model.set_palette(terminal_default_palette(), false);
    let _ = model
        .transcript
        .render(crate::frame_time::FrameRenderContext::capture());

    assert_eq!(
        snapshot.plain_lines_for_range(0, snapshot.line_count(), FrameRenderContext::capture(),),
        vec![
            "                    ".to_string(),
            "› hello             ".to_string(),
            "                    ".to_string(),
        ],
        "older document snapshots should stay pinned to the palette they were created with"
    );
}

#[test]
fn document_transcript_snapshot_bounds_item_block_cache_to_overscanned_viewport_window() {
    let mut model = ready_document_model(24, 6);
    model.transcript_mut().clear();
    model.transcript_mut().set_gap(0);

    for index in 0..20 {
        model
            .transcript_mut()
            .append_message(Sender::Assistant, format!("item {index}"));
    }

    model.sync_transcript_render();
    let snapshot = model
        .current_document_transcript_snapshot(crate::frame_time::FrameRenderContext::capture());

    let first = snapshot.viewport_snapshot(8, 1, FrameRenderContext::capture());
    assert_eq!(first.plain_lines, vec!["item 8".to_string()]);
    assert_eq!(
        sorted_cache_keys(snapshot.item_block_cache.borrow().keys().copied().collect()),
        vec![4, 5, 6, 7, 8, 9, 10, 11, 12],
        "viewport snapshot should prewarm a bounded overscan neighborhood around the visible line"
    );

    let second = snapshot.viewport_snapshot(14, 1, FrameRenderContext::capture());
    assert_eq!(second.plain_lines, vec!["item 14".to_string()]);
    assert_eq!(
        sorted_cache_keys(snapshot.item_block_cache.borrow().keys().copied().collect()),
        vec![10, 11, 12, 13, 14, 15, 16, 17, 18],
        "moving the viewport should evict blocks that fall outside the new overscan neighborhood"
    );
}

#[test]
fn width_refresh_keeps_scrolled_viewport_blocks_warm_for_snapshot_reuse() {
    let mut model = ready_document_model(24, 6);
    model.transcript_mut().clear();
    model.transcript_mut().set_gap(0);

    for index in 0..96 {
        model
            .transcript_mut()
            .append_message(Sender::Assistant, format!("item {index}"));
    }
    model.sync_transcript_render();

    let layout = model.build_document_layout(crate::frame_time::FrameRenderContext::capture());
    model.apply_document_viewport_position(&layout, 20, 0, false, true);

    model.set_window(23, 6);
    assert!(
        model
            .transcript
            .cached_screen_blocks_snapshot()
            .borrow()
            .is_empty(),
        "Phase E width refresh should stop after metrics rebuild and leave viewport prewarm to snapshot construction"
    );

    let snapshot = model
        .current_document_transcript_snapshot(crate::frame_time::FrameRenderContext::capture());

    let warmed_after_refresh = model
        .transcript
        .cached_screen_blocks_snapshot()
        .borrow()
        .get(&20)
        .cloned()
        .expect("snapshot refresh should prewarm the current scrolled viewport on demand");

    let viewport = snapshot.viewport_snapshot(20, 1, FrameRenderContext::capture());
    assert_eq!(viewport.plain_lines, vec!["item 20".to_string()]);
    let snapshot_block = snapshot
        .item_block_cache
        .borrow()
        .get(&20)
        .cloned()
        .expect("viewport snapshot should pin the current item locally");
    assert!(
        Rc::ptr_eq(&warmed_after_refresh, &snapshot_block),
        "document snapshot should reuse the warmed viewport block instead of rematerializing it"
    );
}

#[test]
fn current_document_transcript_snapshot_keeps_large_visible_window_warm() {
    const EXPECTED_MAX_RECENT_RENDER_BLOCKS: usize = 48;

    let mut model = ready_document_model(24, 72);
    model.transcript_mut().clear();
    model.transcript_mut().set_gap(0);

    for index in 0..96 {
        model
            .transcript_mut()
            .append_message(Sender::Assistant, format!("item {index}"));
    }
    model.sync_transcript_render();

    let (visible_start, visible_count) = model
        .current_visible_transcript_window(96)
        .expect("test fixture should expose a transcript viewport");
    assert!(
        visible_count > EXPECTED_MAX_RECENT_RENDER_BLOCKS,
        "test fixture should exceed the bounded recent cache size"
    );

    let snapshot = model
        .current_document_transcript_snapshot(crate::frame_time::FrameRenderContext::capture());

    for expected in visible_start..visible_start + visible_count {
        assert!(
            snapshot
                .warmed_item_block_cache
                .borrow()
                .contains_key(&expected),
            "document snapshot should retain warmed visible item {expected}"
        );
    }
}

#[test]
fn current_visible_transcript_window_tracks_tail_growth_before_viewport_sync() {
    let mut model = ready_document_model(24, 6);
    model.transcript_mut().clear();
    model.transcript_mut().set_gap(0);

    for index in 0..96 {
        model
            .transcript_mut()
            .append_message(Sender::Assistant, format!("item {index}"));
    }
    model.sync_transcript_render();
    model.sync_document_viewport_to_bottom();

    model.status_line_items = vec![StatusLineItem::GitBranch];
    model.git_branch = "main".to_string();
    model
        .composer_mut()
        .set_text_for_test("1\n2\n3\n4\n5\n6\n7\n8");
    model.sync_composer_height();

    let transcript_line_count = model.transcript.item_metrics_index().line_count;
    let layout = compose_document_layout(
        DocumentLayoutKey::default(),
        DocumentLayoutInput {
            transcript: Rc::new(DocumentTranscriptSnapshot {
                index: TranscriptItemMetricsIndex {
                    line_count: transcript_line_count,
                    ..TranscriptItemMetricsIndex::default()
                },
                ..DocumentTranscriptSnapshot::default()
            }),
            tail: model
                .build_document_tail_layout(crate::frame_time::FrameRenderContext::capture()),
        },
    );
    let visible_transcript_indices = model
        .document_viewport_line_indices(&layout)
        .into_iter()
        .filter(|line_index| *line_index < transcript_line_count)
        .collect::<Vec<_>>();
    let expected_window = visible_transcript_indices
        .first()
        .copied()
        .map(|start| (start, visible_transcript_indices.len()));

    assert_eq!(
        model.current_visible_transcript_window(transcript_line_count),
        expected_window,
        "tail growth should not leave the warmed transcript window pinned to the old viewport offset"
    );
}

#[test]
fn transcript_plain_text_len_for_range_avoids_plain_line_and_anchor_materialization() {
    const TRACKED_BLOCK_KEY: u64 = 0xD0C0_0001;

    reset_tracked_cached_render_block_access(TRACKED_BLOCK_KEY);
    let block = Rc::new(CachedRenderBlock {
        cache_key: TRACKED_BLOCK_KEY,
        width: 24,
        palette: default_palette(),
        lines: Rc::new(vec![Line::raw("alpha".to_string())]),
        projected_user: None,
        projected_assistant: None,
        line_count: 1,
        plain_line_byte_lens: Rc::new(vec![5]),
        anchors: CachedLineAnchors::Explicit(Rc::new(vec![ItemLineAnchor {
            kind: LineAnchorKind::RenderedLine,
            rendered_line: 0,
            ..ItemLineAnchor::default()
        }])),
        plain_text_char_len: 5,
    });
    let render = new_render_result(vec![RenderItemSummary {
        item_index: 0,
        start_line: 0,
        gap_before: 0,
        content_line_count: 1,
        total_line_count: 1,
        gap_owner_item_index: None,
        block: Rc::clone(&block),
    }]);
    let snapshot = DocumentTranscriptSnapshot {
        index: render.index.clone(),
        width: 24,
        palette: default_palette(),
        motion_mode: crate::MotionMode::Full,
        items: Rc::new(Vec::new()),
        warmed_item_block_cache: Rc::new(RefCell::new(HashMap::new())),
        item_block_cache: Rc::new(RefCell::new(HashMap::from([(0, Rc::clone(&block))]))),
        selection_semantic_cache: Rc::new(RefCell::new(Default::default())),
    };

    assert_eq!(
        snapshot.plain_text_len_for_range(0, 1, FrameRenderContext::capture()),
        5
    );

    let access = tracked_cached_render_block_access(TRACKED_BLOCK_KEY);
    assert_eq!(
        access.line_reads, 0,
        "full-range plain-text length should come from cached totals without materializing rendered lines"
    );
    assert_eq!(
        access.plain_line_reads, 0,
        "plain-text length should come from cached byte lengths instead of cloning strings"
    );
    assert_eq!(
        access.anchor_reads, 0,
        "range reads without selection should not walk transcript anchors"
    );
}

#[test]
fn rendered_transcript_anchor_resolve_uses_direct_line_when_item_shape_is_stable() {
    const TRACKED_BLOCK_KEY: u64 = 0xD0C0_0002;
    const LINE_COUNT: usize = 2_000;
    const TARGET_LINE: usize = 1_234;

    let lines = (0..LINE_COUNT)
        .map(|index| Line::raw(format!("line {index:04}")))
        .collect::<Vec<_>>();
    let plain_line_byte_lens = lines
        .iter()
        .map(|line| line.spans.iter().map(|span| span.content.len()).sum())
        .collect::<Vec<_>>();
    let block = Rc::new(CachedRenderBlock {
        cache_key: TRACKED_BLOCK_KEY,
        width: 32,
        palette: default_palette(),
        lines: Rc::new(lines),
        projected_user: None,
        projected_assistant: None,
        line_count: LINE_COUNT,
        plain_line_byte_lens: Rc::new(plain_line_byte_lens),
        anchors: CachedLineAnchors::GeneratedRenderedLines,
        plain_text_char_len: LINE_COUNT * "line 0000".len(),
    });
    let render = new_render_result(vec![RenderItemSummary {
        item_index: 0,
        start_line: 0,
        gap_before: 0,
        content_line_count: LINE_COUNT,
        total_line_count: LINE_COUNT,
        gap_owner_item_index: None,
        block: Rc::clone(&block),
    }]);
    let snapshot = Rc::new(DocumentTranscriptSnapshot {
        index: render.index.clone(),
        width: 32,
        palette: default_palette(),
        motion_mode: crate::MotionMode::Full,
        items: Rc::new(Vec::new()),
        warmed_item_block_cache: Rc::new(RefCell::new(HashMap::new())),
        item_block_cache: Rc::new(RefCell::new(HashMap::from([(0, Rc::clone(&block))]))),
        selection_semantic_cache: Rc::new(RefCell::new(Default::default())),
    });
    let layout = compose_document_layout(
        DocumentLayoutKey::default(),
        DocumentLayoutInput {
            transcript: snapshot,
            tail: Rc::new(DocumentTailLayout::default()),
        },
    );
    let state = ViewportState::capture(&layout, &[TARGET_LINE], TARGET_LINE, false, true, 10, 32);

    reset_tracked_cached_render_block_access(TRACKED_BLOCK_KEY);
    assert_eq!(state.resolve_offset(&layout, 10), TARGET_LINE);

    let access = tracked_cached_render_block_access(TRACKED_BLOCK_KEY);
    assert_eq!(
        access.plain_line_reads, 0,
        "stable rendered-line anchors should resolve by line number instead of scanning long item text"
    );
    assert_eq!(
        access.anchor_reads, 0,
        "stable rendered-line anchors should not scan anchors across the whole long item"
    );
}

#[test]
fn transcript_line_access_resolves_without_full_render_result() {
    let mut model = ready_document_model(20, 4);
    model.transcript_mut().clear();
    model
        .transcript_mut()
        .append_message(Sender::Assistant, "alpha\nbeta");
    model.sync_transcript_render();
    model.transcript_render = Rc::new(crate::transcript::RenderResult::default());
    model.document_runtime.transcript_cache = Default::default();
    model.document_runtime.layout_cache = Default::default();
    model.document_runtime.viewport_cache = Default::default();

    let layout = model.build_document_layout(crate::frame_time::FrameRenderContext::capture());
    let first_line = layout
        .line_at(0, FrameRenderContext::capture())
        .expect("transcript line should resolve without the full render result");
    let second_line = layout
        .line_at(1, FrameRenderContext::capture())
        .expect("second transcript line should still materialize");

    assert_eq!(first_line.plain_line, "alpha");
    assert_eq!(first_line.anchor.transcript.item_index, 0);
    assert_eq!(second_line.plain_line, "beta");
    assert_eq!(second_line.anchor.transcript.item_anchor.rendered_line, 1);
    assert_eq!(
        layout.line_index_for_anchor(second_line.anchor, FrameRenderContext::capture()),
        Some(1)
    );
}

#[test]
fn visible_document_lines_tracks_cursor_visibility() {
    let layout = DocumentLayout {
        tail: Rc::new(DocumentTailLayout::from_test_parts(
            vec![Line::raw("a"), Line::raw("b"), Line::raw("c")],
            vec!["a".to_string(), "b".to_string(), "c".to_string()],
            Vec::new(),
            Vec::new(),
            SlotFrame::default(),
            0,
            0,
        )),
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
    let layout = model.build_document_layout(crate::frame_time::FrameRenderContext::capture());
    model.apply_document_viewport_position(&layout, 0, 0, false, true);
    model
        .document_runtime
        .restore
        .track_composer_cursor(Some(restore_viewport_state));

    model.scroll_document_by(Model::document_mouse_wheel_delta());

    assert!(!model.document_runtime.follow_bottom);
    assert!(!model.document_runtime.manual_scroll);
    assert_eq!(model.document_runtime.viewport_y, 2);
    assert_eq!(model.composer.viewport_offset(), 2);
    assert_eq!(
        model.document_runtime.restore.target(),
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
    model.document_runtime.follow_bottom = false;
    model.document_runtime.manual_scroll = false;
    model.sync_document_viewport_for_composer_cursor();

    let old_value = model.composer_text().to_string();
    let old_line = model.composer.line();
    let old_column = model.composer.column();

    model.composer_mut().move_to_end();
    model.sync_composer_height();
    let layout = model.build_document_layout(crate::frame_time::FrameRenderContext::capture());
    let (expected_document_offset, expected_composer_offset) =
        model.bottom_follow_viewport_offsets(&layout);

    model.sync_document_viewport_after_composer_interaction(&old_value, old_line, old_column);

    assert!(model.document_runtime.follow_bottom);
    assert!(!model.document_runtime.manual_scroll);
    assert_eq!(model.document_runtime.viewport_y, expected_document_offset);
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

    let layout = model.build_document_layout(crate::frame_time::FrameRenderContext::capture());
    model.apply_document_viewport_position(&layout, 0, 0, false, true);
    model.document_runtime.restore.track_bottom_follow();

    let preserved_viewport_state = model.current_document_viewport_state();
    let original_document_offset = model.document_runtime.viewport_y;
    let original_composer_offset = model.composer.viewport_offset();

    model
        .transcript_mut()
        .append_message(Sender::Assistant, "new history line");
    model.sync_transcript_render();
    model.sync_document_viewport_after_transcript_refresh(Some(preserved_viewport_state));

    assert!(!model.document_runtime.follow_bottom);
    assert!(model.document_runtime.manual_scroll);
    assert_eq!(model.document_runtime.viewport_y, original_document_offset);
    assert_eq!(model.composer.viewport_offset(), original_composer_offset);
    assert_eq!(
        model.document_runtime.restore.target(),
        ManualDocumentScrollRestoreTarget::BottomFollow
    );
}

#[test]
fn transcript_refresh_preserves_viewport_state_only_for_manual_scroll_mode() {
    let mut model = ready_document_model(20, 4);
    for index in 0..8 {
        model
            .transcript_mut()
            .append_message(Sender::Assistant, format!("history {index}"));
    }
    model.sync_transcript_render();
    model
        .composer_mut()
        .set_text_for_test("draft line one\ndraft line two\ndraft line three");
    model.composer_mut().move_to_begin_for_test();
    model.sync_composer_height();
    model.document_runtime.follow_bottom = false;
    model.document_runtime.manual_scroll = false;
    model.sync_document_viewport_for_composer_cursor();

    assert!(
        model
            .preserved_viewport_state_for_transcript_refresh()
            .is_none(),
        "ordinary non-follow-bottom editing should re-sync around the composer cursor instead of preserving a transcript anchor"
    );

    let layout = model.build_document_layout(crate::frame_time::FrameRenderContext::capture());
    model.apply_document_viewport_position(&layout, 0, 0, false, true);

    assert!(
        model
            .preserved_viewport_state_for_transcript_refresh()
            .is_some(),
        "manual scroll should still preserve a semantic viewport anchor across transcript reflows"
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

    let layout = model.build_document_layout(crate::frame_time::FrameRenderContext::capture());
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
        FrameRenderContext::capture(),
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

    let layout = model.build_document_layout(crate::frame_time::FrameRenderContext::capture());
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
    let updated_layout =
        model.build_document_layout(crate::frame_time::FrameRenderContext::capture());

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

    let layout = model.build_document_layout(crate::frame_time::FrameRenderContext::capture());
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
    let resized_layout =
        model.build_document_layout(crate::frame_time::FrameRenderContext::capture());
    let resolved_offset = state.resolve_offset(&resized_layout, model.document_viewport_height());
    let restored_anchor = document_viewport_anchor_at_line(
        &resized_layout,
        resolved_offset,
        FrameRenderContext::capture(),
    )
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

    let layout = model.build_document_layout(crate::frame_time::FrameRenderContext::capture());
    let separator_offset = (0..layout.line_count())
        .find(|&index| {
            layout
                .line_anchor_at(index, FrameRenderContext::capture())
                .is_some_and(|anchor| {
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
    let resized_layout =
        model.build_document_layout(crate::frame_time::FrameRenderContext::capture());
    let expected_separator_offset = (0..resized_layout.line_count())
        .find(|&index| {
            resized_layout
                .line_anchor_at(index, FrameRenderContext::capture())
                .is_some_and(|anchor| {
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
    let restored_anchor = document_viewport_anchor_at_line(
        &resized_layout,
        resolved_offset,
        FrameRenderContext::capture(),
    )
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

    let _ = model.build_document_layout(crate::frame_time::FrameRenderContext::capture());

    model
        .transcript_mut()
        .append_message(Sender::Assistant, "history");
    model.sync_transcript_render();

    let layout = model.build_document_layout(crate::frame_time::FrameRenderContext::capture());
    let context = FrameRenderContext::capture();
    let expected = compose_document_layout(
        model.current_document_layout_key(context),
        model.current_document_layout_input(context),
    );

    assert_eq!(
        layout.all_plain_lines(FrameRenderContext::capture()),
        expected.all_plain_lines(FrameRenderContext::capture())
    );
    assert_eq!(
        layout.all_line_anchors(FrameRenderContext::capture()),
        expected.all_line_anchors(FrameRenderContext::capture())
    );
    assert_eq!(layout.tail.all_selectable(), expected.tail.all_selectable());
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

    let context = FrameRenderContext::capture();
    let first = model.build_document_layout(context);
    let second = model.build_document_layout(context);

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

    let layout = model.build_document_layout(crate::frame_time::FrameRenderContext::capture());
    let context = FrameRenderContext::capture();
    let first = model.build_document_viewport(&layout, context);
    let second = model.build_document_viewport(&layout, context);

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

    let context = crate::frame_time::FrameRenderContext::capture();
    let layout = model.build_document_layout(context);
    let viewport = model.build_document_viewport(&layout, context);
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

    let _ = model.build_document_layout(crate::frame_time::FrameRenderContext::capture());

    model
        .transcript_mut()
        .append_message(Sender::Assistant, "next");
    model.sync_transcript_render();

    let layout = model.build_document_layout(crate::frame_time::FrameRenderContext::capture());
    let context = FrameRenderContext::capture();
    let expected = compose_document_layout(
        model.current_document_layout_key(context),
        model.current_document_layout_input(context),
    );

    assert_eq!(
        layout.all_plain_lines(FrameRenderContext::capture()),
        expected.all_plain_lines(FrameRenderContext::capture())
    );
    assert_eq!(
        layout.all_line_anchors(FrameRenderContext::capture()),
        expected.all_line_anchors(FrameRenderContext::capture())
    );
    assert_eq!(layout.tail.all_selectable(), expected.tail.all_selectable());
    assert_eq!(layout.composer_slot, expected.composer_slot);
    assert_eq!(layout.cursor_y, expected.cursor_y);
}

#[test]
fn build_document_layout_after_multiple_pending_transcript_appends_matches_compose_document_layout()
{
    let mut model = ready_document_model(24, 6);
    model.composer_mut().set_text_for_test("draft");
    model.sync_composer_height();

    let _ = model.build_document_layout(crate::frame_time::FrameRenderContext::capture());

    model
        .transcript_mut()
        .append_message(Sender::Assistant, "one");
    model.sync_transcript_render();
    model
        .transcript_mut()
        .append_message(Sender::Assistant, "two");
    model.sync_transcript_render();

    let layout = model.build_document_layout(crate::frame_time::FrameRenderContext::capture());
    let context = FrameRenderContext::capture();
    let expected = compose_document_layout(
        model.current_document_layout_key(context),
        model.current_document_layout_input(context),
    );

    assert_eq!(
        layout.all_plain_lines(FrameRenderContext::capture()),
        expected.all_plain_lines(FrameRenderContext::capture())
    );
    assert_eq!(
        layout.all_line_anchors(FrameRenderContext::capture()),
        expected.all_line_anchors(FrameRenderContext::capture())
    );
    assert_eq!(layout.tail.all_selectable(), expected.tail.all_selectable());
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
        let context = crate::frame_time::FrameRenderContext::capture();
        let layout = black_box(model.build_document_layout(context));
        black_box(model.build_document_viewport(&layout, context));
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
fn viewport_anchor_reads_keep_long_transcript_selectable_ranges_lazy() {
    let mut model = ready_document_model(48, 8);
    model.transcript_mut().append_message_with_style_mode(
        Sender::User,
        "长消息 mixed english content keeps scrolling anchor capture lazy ".repeat(120),
        StyleMode::Cx,
    );
    model.sync_transcript_render();

    let layout = model.build_document_layout(crate::frame_time::FrameRenderContext::capture());
    assert!(
        layout
            .transcript
            .selection_semantic_cache
            .borrow()
            .is_empty(),
        "selectable ranges should start empty before anchor-only reads"
    );

    let viewport_lines = (0..model
        .document_viewport_height()
        .min(layout.transcript_line_count))
        .collect::<Vec<_>>();
    let _state = ViewportState::capture(
        &layout,
        &viewport_lines,
        0,
        false,
        true,
        model.document_viewport_height(),
        model.width,
    );
    let _anchor = document_viewport_anchor_at_line(&layout, 1, FrameRenderContext::capture())
        .expect("long transcript line should expose a viewport anchor");

    assert!(
        layout
            .transcript
            .selection_semantic_cache
            .borrow()
            .is_empty(),
        "scroll anchor capture must not materialize whole-item selectable ranges"
    );
}

#[test]
fn document_viewport_materialization_keeps_transcript_selectable_ranges_lazy() {
    let mut model = ready_document_model(24, 6);
    model
        .transcript_mut()
        .append_message(Sender::User, "alpha beta gamma");
    model.sync_transcript_render();

    let layout = model.build_document_layout(crate::frame_time::FrameRenderContext::capture());
    assert!(
        layout
            .transcript
            .selection_semantic_cache
            .borrow()
            .is_empty(),
        "transcript selectable cache should start empty before any selection-aware read"
    );

    let viewport = compose_document_viewport(&layout, 0, 1, FrameRenderContext::capture());

    assert_eq!(viewport.plain_lines.len(), 1);
    assert!(
        layout
            .transcript
            .selection_semantic_cache
            .borrow()
            .is_empty(),
        "plain viewport materialization should not populate transcript selectable ranges"
    );
}

#[test]
fn document_line_access_computes_transcript_selectable_ranges_lazily() {
    let mut model = ready_document_model(24, 6);
    model
        .transcript_mut()
        .append_message(Sender::User, "alpha beta gamma");
    model.sync_transcript_render();

    let layout = model.build_document_layout(crate::frame_time::FrameRenderContext::capture());
    assert_eq!(
        layout
            .transcript
            .selection_semantic_cache
            .borrow()
            .get(0)
            .map_or(0, |entry| entry.selectable_ranges.len()),
        0
    );

    let selectable_line_index = (0..layout.transcript_line_count)
        .find(|index| {
            layout
                .line_at(*index, FrameRenderContext::capture())
                .is_some_and(|line| line.selectable.has_content())
        })
        .expect("transcript lines should include selectable content");
    let line = layout
        .line_at(selectable_line_index, FrameRenderContext::capture())
        .expect("selectable transcript line should exist");
    assert!(
        line.selectable.has_content(),
        "line_at should compute a selectable range, got {:?}, cache={:?}",
        line,
        layout.transcript.selection_semantic_cache.borrow()
    );
    assert_eq!(
        layout
            .transcript
            .selection_semantic_cache
            .borrow()
            .get(0)
            .map_or(0, |entry| entry.selectable_ranges.len()),
        3
    );
}

#[test]
fn transcript_selection_semantic_cache_stays_bounded_across_long_history() {
    const EXPECTED_MAX_SELECTION_SEMANTIC_ITEMS: usize = 32;
    let mut model = ready_document_model(48, 8);
    for index in 0..(EXPECTED_MAX_SELECTION_SEMANTIC_ITEMS + 17) {
        model
            .transcript_mut()
            .append_message(Sender::User, format!("selectable item {index}"));
    }
    model.sync_transcript_render();

    let layout = model.build_document_layout(FrameRenderContext::capture());
    for position in layout.transcript.index.visible_items.iter() {
        let _ =
            (position.start_line..position.start_line + position.total_line_count).find(|line| {
                layout
                    .line_at(*line, FrameRenderContext::capture())
                    .is_some_and(|line| line.selectable.has_content())
            });
    }

    assert!(
        layout.transcript.selection_semantic_cache.borrow().len()
            <= EXPECTED_MAX_SELECTION_SEMANTIC_ITEMS,
        "selection semantic entries must have a fixed residency bound"
    );
    for position in layout.transcript.index.visible_items.iter() {
        if let Some(entry) = layout
            .transcript
            .selection_semantic_cache
            .borrow()
            .get(position.item_index)
        {
            assert_eq!(entry.plain_lines.len(), entry.selectable_ranges.len());
        }
    }
}

#[test]
fn document_layout_key_changes_when_fullscreen_modal_gates_inline_tail() {
    let mut model = ready_document_model(80, 12);
    model.composer_mut().insert_text("/model");
    let context = FrameRenderContext::capture();
    let before = model.current_document_layout_key(context);

    model.open_transcript_overlay();

    assert_ne!(
        model.current_document_layout_key(context),
        before,
        "fullscreen modal gating must invalidate the cached inline tail"
    );
}

fn ready_document_model(width: u16, height: u16) -> Model {
    let mut model = Model::new_with_style_mode(StartupBannerOptions::default(), StyleMode::Ms);
    model.transcript_mut().clear();
    model.sync_transcript_render();
    model.set_window(width, height);
    model.set_palette(default_palette(), true);
    model
}

fn sorted_cache_keys(mut keys: Vec<usize>) -> Vec<usize> {
    keys.sort_unstable();
    keys
}

fn cursor_visible_in_document_viewport(
    layout: &DocumentLayout,
    resolved_offset: usize,
    visible_line_count: usize,
) -> bool {
    layout.cursor_y >= resolved_offset
        && layout.cursor_y < resolved_offset.saturating_add(visible_line_count)
}
