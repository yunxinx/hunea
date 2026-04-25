const EXPECTED_MAX_RECENT_RENDER_BLOCKS: usize = 48;

use ratatui::text::Span;

use super::*;
use crate::frontend::tui::transcript::{
    render_markdown_metrics_call_count, reset_render_markdown_metrics_call_count,
};
use crate::frontend::tui::{
    HeroOptions, StyleMode,
    message::{
        message_item_render_cache_key_call_count, reset_message_item_render_cache_key_call_count,
        reset_user_message_projection_plain_line_len_call_count,
        user_message_projection_plain_line_len_call_count,
    },
    theme::{default_palette, terminal_default_palette},
};

#[test]
fn render_returns_content_lines_and_line_count() {
    let mut transcript = Transcript::new(default_palette());
    transcript.items = Rc::new(vec![
        Rc::new(TranscriptItem::Message(MessageItem::new(
            Sender::Assistant,
            "one\ntwo",
        ))),
        Rc::new(TranscriptItem::Message(MessageItem::new(
            Sender::Assistant,
            "three",
        ))),
    ]);

    let result = transcript.render();
    let rendered = result
        .lines_for_range(0, result.line_count)
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>();

    assert_eq!(rendered, vec!["one", "two", "", "three"]);
    assert_eq!(result.line_count, 4);
}

#[test]
fn item_metrics_index_maps_offsets_and_item_ranges_without_full_render_result() {
    let mut transcript = Transcript::new(default_palette());
    transcript.items = Rc::new(vec![
        Rc::new(TranscriptItem::Message(MessageItem::new(
            Sender::Assistant,
            "one\ntwo",
        ))),
        Rc::new(TranscriptItem::Message(MessageItem::new(
            Sender::Assistant,
            "three",
        ))),
    ]);

    let index = transcript.item_metrics_index();

    assert_eq!(index.line_count, 4);
    assert_eq!(
        index.item_lines(0),
        Some(super::super::render_state::RenderItemLines {
            content_start_line: 0,
            content_line_count: 2,
            total_line_count: 3,
        })
    );
    assert_eq!(
        index.item_lines(1),
        Some(super::super::render_state::RenderItemLines {
            content_start_line: 3,
            content_line_count: 1,
            total_line_count: 1,
        })
    );
    assert_eq!(index.item_index_for_line(0), Some(0));
    assert_eq!(index.item_index_for_line(2), Some(0));
    assert_eq!(index.item_index_for_line(3), Some(1));
}

#[test]
fn item_metrics_index_matches_materialized_block_metrics_for_mixed_item_types() {
    let palette = default_palette();
    let mut transcript = Transcript::new(palette);
    transcript.set_gap(1);
    transcript.set_width(18);
    transcript.append_hero(HeroOptions {
        app_name: Some("Lumos".to_string()),
        version: Some("v0.1.0".to_string()),
        work_dir: Some("/tmp/phase-e-metrics".to_string()),
        width: 0,
    });
    transcript.append_message(Sender::Assistant, "## Wrapped heading\n\nassistant body");
    transcript.append_message_with_style_mode(
        Sender::User,
        "user message keeps metrics-only rebuild honest",
        StyleMode::Cx,
    );

    let index = transcript.item_metrics_index();

    for (item_index, item) in transcript.items.iter().enumerate() {
        let block = materialize_transcript_item_render_block(
            item.as_ref(),
            transcript.render_width(),
            palette,
        );
        let metrics = index.metrics[item_index];

        assert_eq!(
            metrics.content_line_count,
            block.line_count(),
            "metrics-only path should preserve line_count for item {item_index}"
        );
        assert_eq!(
            metrics.content_char_len, block.plain_text_char_len,
            "metrics-only path should preserve plain_text_char_len for item {item_index}"
        );
    }
}

#[test]
fn item_metrics_index_tracks_invalidation_boundaries() {
    let mut transcript = Transcript::new(default_palette());
    transcript.items = Rc::new(vec![
        Rc::new(TranscriptItem::Message(MessageItem::new(
            Sender::Assistant,
            "first",
        ))),
        Rc::new(TranscriptItem::Message(MessageItem::new(
            Sender::Assistant,
            "second",
        ))),
    ]);

    let _ = transcript.item_metrics_index();
    assert_eq!(transcript.item_metrics_dirty_from_for_test(), 2);
    assert_eq!(transcript.item_positions_dirty_from_for_test(), 2);

    transcript.append_message(Sender::Assistant, "third");
    assert_eq!(transcript.item_metrics_dirty_from_for_test(), 2);
    assert_eq!(transcript.item_positions_dirty_from_for_test(), 2);

    let _ = transcript.item_metrics_index();
    transcript.replace_item_for_test(1, TranscriptItem::Message(static_message("updated")));
    assert_eq!(transcript.item_metrics_dirty_from_for_test(), 1);
    assert_eq!(transcript.item_positions_dirty_from_for_test(), 1);

    let _ = transcript.item_metrics_index();
    transcript.set_gap(2);
    assert_eq!(transcript.item_metrics_dirty_from_for_test(), 3);
    assert_eq!(transcript.item_positions_dirty_from_for_test(), 0);

    let _ = transcript.item_metrics_index();
    transcript.set_width(48);
    assert_eq!(transcript.item_metrics_dirty_from_for_test(), 0);
    assert_eq!(transcript.item_positions_dirty_from_for_test(), 0);
}

#[test]
fn render_append_path_keeps_gap_anchor_on_previous_item() {
    let mut transcript = Transcript::new(default_palette());
    transcript.items = Rc::new(vec![Rc::new(TranscriptItem::Message(static_message(
        "first",
    )))]);
    let _ = transcript.render();

    transcript.append_message(Sender::Assistant, "second");
    let result = transcript.render();
    let line_anchors = result.all_line_anchors();

    assert_eq!(line_anchors.len(), 3);
    assert_eq!(line_anchors[1].item_index, 0);
    assert_eq!(line_anchors[1].item_anchor.kind, LineAnchorKind::ItemGap);
}

#[test]
fn render_append_path_marks_append_start_line() {
    let mut transcript = Transcript::new(default_palette());
    transcript.items = Rc::new(vec![Rc::new(TranscriptItem::Message(static_message(
        "first",
    )))]);
    let _ = transcript.render();

    transcript.append_message(Sender::Assistant, "second");
    let result = transcript.render();

    assert_eq!(result.append_start_line, 1);
    assert_eq!(result.all_plain_lines(), vec!["first", "", "second"]);
}

#[test]
fn render_builds_gap_anchor_between_visible_blocks() {
    let mut transcript = Transcript::new(default_palette());
    transcript.items = Rc::new(vec![
        Rc::new(TranscriptItem::Message(static_message("one"))),
        Rc::new(TranscriptItem::Message(static_message("two"))),
    ]);

    let result = transcript.render();
    let line_anchors = result.all_line_anchors();

    assert_eq!(line_anchors.len(), 3);
    assert_eq!(line_anchors[1].item_index, 0);
    assert_eq!(line_anchors[1].item_anchor.kind, LineAnchorKind::ItemGap);
    assert_eq!(line_anchors[2].item_index, 1);
}

#[test]
#[ignore = "performance smoke test"]
fn render_perf_smoke_for_large_cached_transcript() {
    use std::hint::black_box;

    let mut transcript = Transcript::new(default_palette());
    transcript.set_width(72);

    for index in 0..64 {
        Rc::make_mut(&mut transcript.items).push(Rc::new(TranscriptItem::Message(
                static_message(&format!(
                "item {index:02}\nalpha beta gamma alpha beta gamma\ndelta epsilon zeta delta epsilon zeta"
            )),
            )));
    }

    for _ in 0..128 {
        black_box(transcript.render());
    }
}

#[test]
fn cached_render_result_can_be_reused_when_item_cache_keys_are_stable() {
    let mut transcript = Transcript::new(default_palette());
    transcript.items = Rc::new(vec![Rc::new(TranscriptItem::Message(static_message(
        "cached",
    )))]);

    let _ = transcript.render();

    assert!(transcript.can_reuse_cached_render_result(transcript.render_width()));
}

#[test]
fn cached_render_result_becomes_stale_after_item_content_changes() {
    let mut transcript = Transcript::new(default_palette());
    transcript.items = Rc::new(vec![Rc::new(TranscriptItem::Message(static_message(
        "one",
    )))]);

    let _ = transcript.render();
    transcript.replace_item_for_test(0, TranscriptItem::Message(static_message("two")));

    assert!(!transcript.can_reuse_cached_render_result(transcript.render_width()));
}

#[test]
fn render_cache_hit_reuses_underlying_result_storage() {
    let mut transcript = Transcript::new(default_palette());
    transcript.items = Rc::new(vec![Rc::new(TranscriptItem::Message(static_message(
        "cached",
    )))]);

    let first = transcript.render();
    let second = transcript.render();

    assert_eq!(first.items.as_ptr(), second.items.as_ptr());
}

#[test]
fn render_cache_hit_does_not_rehash_message_content() {
    let mut transcript = Transcript::new(default_palette());
    transcript.items = Rc::new(vec![Rc::new(TranscriptItem::Message(static_message(
        "cached",
    )))]);
    reset_message_item_render_cache_key_call_count();

    let _ = transcript.render();
    let after_first_render = message_item_render_cache_key_call_count();
    let _ = transcript.render();
    let after_second_render = message_item_render_cache_key_call_count();

    assert_eq!(after_first_render, 0);
    assert_eq!(after_second_render, 0);
}

#[test]
fn append_does_not_preallocate_dense_render_cache_slots() {
    let mut transcript = Transcript::new(default_palette());

    for index in 0..64 {
        transcript.append_message(Sender::Assistant, format!("item {index}"));
    }

    assert_eq!(
        transcript.screen_cache.items.borrow().len(),
        0,
        "append should not grow dense render cache slots before any render happens"
    );
}

#[test]
fn assistant_render_blocks_use_generated_anchors_without_eager_plain_text_cache() {
    let mut transcript = Transcript::new(default_palette());
    transcript.set_width(12);
    transcript.items = Rc::new(vec![Rc::new(TranscriptItem::Message(static_message(
        "alpha beta gamma delta epsilon",
    )))]);

    let render = transcript.render();
    let block = render
        .items
        .first()
        .expect("assistant item should produce a render block")
        .block
        .as_ref();

    assert!(
        !block.stores_plain_lines(),
        "assistant blocks should not keep a second plain-text copy for every rendered line"
    );
    assert!(
        block.uses_generated_rendered_line_anchors(),
        "assistant blocks should synthesize rendered-line anchors instead of storing a fallback anchor vec"
    );
}

#[test]
fn generated_anchor_blocks_still_round_trip_plain_text_and_anchor_lookup() {
    let mut transcript = Transcript::new(default_palette());
    transcript.set_width(12);
    transcript.items = Rc::new(vec![Rc::new(TranscriptItem::Message(static_message(
        "alpha beta gamma delta epsilon",
    )))]);

    let render = transcript.render();
    let rendered = render
        .line_at(1)
        .expect("wrapped assistant message should expose multiple rendered lines");

    assert!(
        !rendered.plain_line.is_empty(),
        "plain text should still be recoverable when the block only stores structured render data"
    );
    assert_eq!(render.line_index_for_anchor(rendered.anchor), Some(1));
}

#[test]
fn user_render_blocks_project_lines_without_eager_styled_line_storage() {
    let mut transcript = Transcript::new(default_palette());
    transcript.set_width(16);
    transcript.items = Rc::new(vec![Rc::new(TranscriptItem::Message(
        MessageItem::new_with_style_mode(
            Sender::User,
            "user message keeps wrapped projection stable across renders",
            StyleMode::Cx,
        ),
    ))]);

    let render = transcript.render();
    let block = render
        .items
        .first()
        .expect("user item should produce a render block")
        .block
        .as_ref();

    assert!(
        block.lines.is_empty(),
        "user blocks should keep a compact projection and materialize styled lines on demand"
    );
}

#[test]
fn projected_user_render_block_reuses_plain_line_lengths_during_cache_population() {
    let mut transcript = Transcript::new(default_palette());
    transcript.set_width(16);
    transcript.items = Rc::new(vec![Rc::new(TranscriptItem::Message(
        MessageItem::new_with_style_mode(
            Sender::User,
            "user message keeps wrapped projection stable across renders",
            StyleMode::Cx,
        ),
    ))]);
    reset_user_message_projection_plain_line_len_call_count();

    let render = transcript.render();
    let block = render
        .items
        .first()
        .expect("user item should produce a render block")
        .block
        .as_ref();

    assert_eq!(
        user_message_projection_plain_line_len_call_count(),
        block.line_count(),
        "projected user cache population should compute each plain line length only once"
    );
}

#[test]
fn projected_user_blocks_still_round_trip_plain_text_and_anchor_lookup() {
    let mut transcript = Transcript::new(default_palette());
    transcript.set_width(16);
    transcript.items = Rc::new(vec![Rc::new(TranscriptItem::Message(
        MessageItem::new_with_style_mode(
            Sender::User,
            "user message keeps wrapped projection stable across renders",
            StyleMode::Cx,
        ),
    ))]);

    let expected_plain_lines = transcript.items[0]
        .render_lines(16, default_palette())
        .iter()
        .map(line_to_plain_text)
        .collect::<Vec<_>>();
    let render = transcript.render();
    let actual_plain_lines = (0..render.line_count)
        .map(|index| {
            render
                .line_at(index)
                .expect("projected user block should materialize every visible line")
                .plain_line
        })
        .collect::<Vec<_>>();
    let anchor = render
        .line_at(1)
        .expect("projected user block should expose wrapped content lines")
        .anchor;

    assert_eq!(actual_plain_lines, expected_plain_lines);
    assert_eq!(render.line_index_for_anchor(anchor), Some(1));
}

#[test]
fn precomputed_render_cache_key_changes_with_message_content_and_style() {
    let assistant_one = TranscriptItem::Message(MessageItem::new(Sender::Assistant, "one"));
    let assistant_two = TranscriptItem::Message(MessageItem::new(Sender::Assistant, "two"));
    let user_cx = TranscriptItem::Message(MessageItem::new_with_style_mode(
        Sender::User,
        "same",
        StyleMode::Cx,
    ));
    let user_cc = TranscriptItem::Message(MessageItem::new_with_style_mode(
        Sender::User,
        "same",
        StyleMode::Cc,
    ));

    assert_ne!(
        assistant_one.render_cache_key(),
        assistant_two.render_cache_key()
    );
    assert_ne!(user_cx.render_cache_key(), user_cc.render_cache_key());
}

#[test]
fn render_refreshes_after_item_content_changes() {
    let mut transcript = Transcript::new(default_palette());
    transcript.items = Rc::new(vec![Rc::new(TranscriptItem::Message(static_message(
        "one",
    )))]);

    let first = transcript.render();
    assert_eq!(first.all_plain_lines(), vec!["one"]);

    transcript.replace_item_for_test(0, TranscriptItem::Message(static_message("two")));

    let second = transcript.render();
    assert_eq!(second.all_plain_lines(), vec!["two"]);
}

#[test]
fn render_viewport_refreshes_after_item_content_changes() {
    let mut transcript = Transcript::new(default_palette());
    transcript.items = Rc::new(vec![Rc::new(TranscriptItem::Message(static_message(
        "one\ntwo",
    )))]);

    let first = transcript.render_viewport(1, 1);
    assert_eq!(first.plain_lines, vec!["two"]);

    transcript.replace_item_for_test(0, TranscriptItem::Message(static_message("alpha\nbeta")));

    let second = transcript.render_viewport(1, 1);
    assert_eq!(second.plain_lines, vec!["beta"]);
}

#[test]
fn item_metrics_index_keeps_recent_render_block_cache_bounded() {
    let mut transcript = Transcript::new(default_palette());
    transcript.set_gap(0);
    transcript.set_width(32);

    for index in 0..96 {
        transcript.append_message(Sender::Assistant, format!("item {index}"));
    }

    let _ = transcript.item_metrics_index();
    assert!(
        transcript.screen_cache.items.borrow().is_empty(),
        "Phase E metrics rebuild should stay metrics-only and avoid materializing render blocks"
    );
}

#[test]
fn item_metrics_index_avoids_linear_recent_cache_bookkeeping_for_large_batches() {
    let mut transcript = Transcript::new(default_palette());
    transcript.set_gap(0);
    transcript.set_width(32);

    for index in 0..96 {
        transcript.append_message(Sender::Assistant, format!("item {index}"));
    }

    transcript.screen_cache.reset_recent_item_tracking_work();
    let _ = transcript.item_metrics_index();
    let work = transcript.screen_cache.recent_item_tracking_work();

    assert_eq!(
        work.linear_scan_steps, 0,
        "recent cache tracking should not linearly scan bookkeeping state during large metrics batches: {work:?}"
    );
    assert_eq!(
        work.shifted_entries, 0,
        "recent cache tracking should not shift bookkeeping entries during large metrics batches: {work:?}"
    );
}

#[test]
fn progressive_metrics_resize_keeps_assistant_markdown_on_fast_estimate_path() {
    let mut transcript = Transcript::new(default_palette());
    transcript.set_width(80);

    for index in 0..4 {
        transcript.append_message(
                Sender::Assistant,
                format!(
                    "## Assistant {index}\n\n- keep estimate cheap\n- keep width changes stable\n\n```rust\nfn item_{index}() {{}}\n```"
                ),
            );
    }

    let _ = transcript.item_metrics_index();
    reset_render_markdown_metrics_call_count();

    transcript.set_width(120);
    let (index, breakdown) = transcript.progressive_item_metrics_index_with_breakdown();

    assert_eq!(
        breakdown.assistant_resize_reuse_count, 4,
        "resize should report semantic reuse for every assistant item whose previous metrics were reused"
    );
    assert_eq!(
        render_markdown_metrics_call_count(),
        0,
        "assistant resize should stay on the fast estimate path instead of reparsing Markdown metrics for every cached item"
    );
    assert!(
        index
            .metrics
            .iter()
            .all(TranscriptItemMetrics::is_estimated),
        "resize reuse should keep assistant metrics estimated until the visible window is exactized"
    );
}

#[test]
fn progressive_metrics_assistant_estimate_skips_exact_markdown_metrics_on_cold_resume() {
    let mut transcript = Transcript::new(default_palette());
    transcript.set_width(80);
    transcript.append_message(
        Sender::Assistant,
        "## Overview\n\n- keep the fast path cheap\n- render exactly later",
    );

    reset_render_markdown_metrics_call_count();
    let _ = transcript.progressive_item_metrics_index();

    assert_eq!(
        render_markdown_metrics_call_count(),
        0,
        "progressive assistant metrics should stay on the fast estimate path instead of paying exact Markdown metrics during cold resume"
    );
}

#[test]
fn progressive_metrics_resize_keeps_assistant_line_count_equal_to_exact_metrics() {
    let mut transcript = Transcript::new(default_palette());
    transcript.set_gap(0);
    transcript.set_width(10);
    transcript.append_message(Sender::Assistant, "foo  bar baz");

    let _ = transcript.progressive_item_metrics_index();

    transcript.set_width(5);
    let estimated_line_count = transcript.progressive_item_metrics_index().line_count;
    let exact_line_count = transcript.item_metrics_index().line_count;

    assert_eq!(estimated_line_count, exact_line_count);
    assert_eq!(exact_line_count, 3);
}

#[test]
fn progressive_metrics_keep_plain_text_prefix_sums_equal_to_exact_for_tabs() {
    let mut transcript = Transcript::new(default_palette());
    transcript.set_gap(0);
    transcript.set_width(9);
    transcript.append_message(Sender::Assistant, "a\tb");
    transcript.append_message(Sender::Assistant, "tail");

    let estimated_index = transcript.progressive_item_metrics_index();
    let exact_index = transcript.item_metrics_index();

    assert_eq!(
        estimated_index.metrics[0].content_char_len,
        exact_index.metrics[0].content_char_len
    );
    assert_eq!(
        estimated_index.content_prefix_sums,
        exact_index.content_prefix_sums
    );
    assert_eq!(
        estimated_index.content_char_len,
        exact_index.content_char_len
    );
}

#[test]
fn progressive_metrics_resize_defers_tabbed_markdown_prefix_sum_exactization() {
    let mut transcript = Transcript::new(default_palette());
    transcript.set_gap(0);
    transcript.set_width(20);
    transcript.append_message(Sender::Assistant, "- item with a tab\tand tail");
    transcript.append_message(Sender::Assistant, "tail");

    let exact_before_resize = transcript.item_metrics_index();
    reset_render_markdown_metrics_call_count();

    transcript.set_width(10);
    let estimated_index = transcript.progressive_item_metrics_index();
    assert_eq!(
        render_markdown_metrics_call_count(),
        0,
        "progressive resize should keep tabbed Markdown on the fast estimate path"
    );
    assert!(estimated_index.metrics[0].is_estimated());

    let exact_index = transcript.item_metrics_index();
    assert_eq!(
        render_markdown_metrics_call_count(),
        2,
        "exactization should pay the Markdown metrics cost only when the exact path is requested, including the remaining assistant items in range"
    );

    assert!(exact_index.metrics[0].is_exact());
    assert!(
        estimated_index.metrics[0].content_char_len
            >= exact_before_resize.metrics[0].content_char_len,
        "resize reuse should preserve the previous assistant plain-text length floor until exactization"
    );
    assert!(
        estimated_index.content_char_len >= exact_before_resize.content_char_len,
        "full-range plain-text totals should not shrink while resize reuse is still estimated"
    );
    assert!(estimated_index.metrics[0].content_char_len <= exact_index.metrics[0].content_char_len);
}

#[test]
fn progressive_metrics_breakdown_counts_assistant_semantic_resize_reuse() {
    let mut transcript = Transcript::new(default_palette());
    transcript.set_width(80);
    transcript.append_message(Sender::Assistant, "make the handler return early");

    let _ = transcript.progressive_item_metrics_index();

    transcript.set_width(20);
    let (_, breakdown) = transcript.progressive_item_metrics_index_with_breakdown();

    assert_eq!(breakdown.assistant_item_count, 1);
    assert_eq!(breakdown.assistant_resize_reuse_count, 1);
}

#[test]
fn progressive_metrics_resize_keeps_reused_assistant_metrics_estimated() {
    let mut transcript = Transcript::new(default_palette());
    transcript.set_width(80);
    transcript.append_message(
        Sender::Assistant,
        "## Resize\n\n- keep resize cheap\n- exactize only the visible window later",
    );

    let _ = transcript.item_metrics_index();
    reset_render_markdown_metrics_call_count();

    transcript.set_width(24);
    let index = transcript.progressive_item_metrics_index();

    assert!(index.metrics[0].is_estimated());
    assert_eq!(
        render_markdown_metrics_call_count(),
        0,
        "progressive resize should not pay a Markdown metrics pass before the visible window requests exactization"
    );
}

#[test]
fn metrics_rebuild_keeps_screen_block_cache_cold_until_render_materialization() {
    let mut transcript = Transcript::new(default_palette());
    transcript.set_gap(0);
    transcript.set_width(32);

    for index in 0..96 {
        transcript.append_message(Sender::Assistant, format!("item {index}"));
    }

    let _ = transcript.item_metrics_index();
    assert!(
        transcript.screen_cache.items.borrow().is_empty(),
        "metrics rebuild should not prewarm render blocks before a real materialization path asks for them"
    );

    let render = transcript.render();
    assert!(
        !transcript.screen_cache.items.borrow().is_empty(),
        "full render should still populate render blocks once the materialization path runs"
    );
    assert_eq!(render.items.len(), 96);
}

#[test]
fn retained_block_memory_summary_counts_result_owned_blocks_after_full_render() {
    let mut transcript = Transcript::new(default_palette());
    transcript.set_gap(0);
    transcript.set_width(32);

    for index in 0..96 {
        transcript.append_message(
            Sender::Assistant,
            format!("item {index}\nalpha beta gamma delta epsilon"),
        );
    }

    let render = transcript.render();
    let summary = transcript.retained_block_memory_summary();
    let expected = retained_block_memory_summary_for_render(&render, summary);

    assert!(
        render.items.len() > EXPECTED_MAX_RECENT_RENDER_BLOCKS,
        "test fixture should exceed the bounded recent cache size"
    );
    assert_eq!(
        summary.estimated_render_ui_bytes, expected.estimated_render_ui_bytes,
        "retained memory should count every unique block still owned by the render result"
    );
    assert_eq!(
        summary.estimated_plain_line_bytes, expected.estimated_plain_line_bytes,
        "retained memory should include plain-line metadata for result-owned blocks"
    );
    assert_eq!(
        summary.estimated_anchor_bytes, expected.estimated_anchor_bytes,
        "retained memory should include anchor metadata for result-owned blocks"
    );
}

#[test]
fn render_viewport_prewarms_overscan_neighbors_once_metrics_are_warm() {
    let mut transcript = Transcript::new(default_palette());
    transcript.set_gap(0);
    transcript.set_width(32);

    for index in 0..10 {
        transcript.append_message(Sender::Assistant, format!("item {index}"));
    }

    let _ = transcript.item_metrics_index();
    transcript.screen_cache.items.borrow_mut().clear();
    transcript.screen_cache.result = Rc::new(RenderResult::default());
    transcript.screen_cache.valid = false;

    let viewport = transcript.render_viewport(5, 1);

    assert_eq!(viewport.plain_lines, vec!["item 5".to_string()]);
    assert_eq!(
        transcript.screen_cache.items.borrow().len(),
        9,
        "viewport materialization should prewarm a bounded overscan neighborhood"
    );
    for expected in 1..=9 {
        assert!(
            transcript
                .screen_cache
                .items
                .borrow()
                .contains_key(&expected),
            "overscan neighborhood should keep item {expected} warm"
        );
    }
}

#[test]
fn render_viewport_keeps_large_visible_window_warm() {
    let mut transcript = Transcript::new(default_palette());
    transcript.set_gap(0);
    transcript.set_width(32);

    for index in 0..96 {
        transcript.append_message(Sender::Assistant, format!("item {index}"));
    }

    let visible_count = EXPECTED_MAX_RECENT_RENDER_BLOCKS + 16;
    let viewport = transcript.render_viewport(0, visible_count);

    assert_eq!(viewport.plain_lines.len(), visible_count);
    for expected in 0..visible_count {
        assert!(
            transcript
                .screen_cache
                .items
                .borrow()
                .contains_key(&expected),
            "large viewport warm cache should retain visible item {expected}"
        );
    }
}

#[test]
fn finish_recent_render_block_batch_evicts_all_warmed_blocks_when_visible_window_is_empty() {
    let mut transcript = Transcript::new(default_palette());
    transcript.set_gap(0);
    transcript.set_width(32);

    for index in 0..96 {
        transcript.append_message(Sender::Assistant, format!("item {index}"));
    }

    let visible_count = EXPECTED_MAX_RECENT_RENDER_BLOCKS + 16;
    let viewport = transcript.render_viewport(0, visible_count);
    assert_eq!(viewport.plain_lines.len(), visible_count);
    assert!(
        !transcript.screen_cache.items.borrow().is_empty(),
        "test fixture should start from an already warmed block cache"
    );

    transcript.begin_recent_render_block_batch();
    transcript.finish_recent_render_block_batch(0);

    assert!(
        transcript.screen_cache.items.borrow().is_empty(),
        "empty visible window should evict every warmed block instead of retaining the default recent limit"
    );
}

#[test]
fn cloned_transcript_does_not_reuse_screen_blocks_from_a_different_palette() {
    let mut original = Transcript::new(default_palette());
    original.set_width(20);
    original.items = Rc::new(vec![Rc::new(TranscriptItem::Message(MessageItem::new(
        Sender::User,
        "hello",
    )))]);

    let mut cloned = original.clone();
    cloned.set_palette(terminal_default_palette());

    let original_render = original.render();
    assert_eq!(original_render.line_count, 3);

    let cloned_render = cloned.render();
    assert_eq!(cloned_render.line_count, 1);
    assert_eq!(
        cloned_render.all_plain_lines(),
        vec!["› hello             "]
    );
}

#[test]
fn palette_change_invalidates_item_metrics_when_render_shape_changes() {
    let mut transcript = Transcript::new(default_palette());
    transcript.set_width(20);
    transcript.items = Rc::new(vec![Rc::new(TranscriptItem::Message(MessageItem::new(
        Sender::User,
        "hello",
    )))]);

    let initial_index = transcript.item_metrics_index();
    assert_eq!(initial_index.line_count, 3);
    assert_eq!(
        initial_index
            .item_lines(0)
            .map(|lines| lines.content_line_count),
        Some(3)
    );

    transcript.set_palette(terminal_default_palette());

    let updated_index = transcript.item_metrics_index();
    assert_eq!(updated_index.line_count, 1);
    assert_eq!(
        updated_index
            .item_lines(0)
            .map(|lines| lines.content_line_count),
        Some(1)
    );

    let render = transcript.render();
    assert_eq!(render.line_count, 1);
    assert_eq!(render.all_plain_lines(), vec!["› hello             "]);
}

fn static_message(content: &str) -> MessageItem {
    MessageItem::new(Sender::Assistant, content)
}

#[allow(dead_code)]
fn styled_line(text: &str) -> Line<'static> {
    Line::from(Span::raw(text.to_string()))
}

fn retained_block_memory_summary_for_render(
    render: &RenderResult,
    actual: super::super::RetainedBlockMemorySummary,
) -> super::super::RetainedBlockMemorySummary {
    let mut summary = super::super::RetainedBlockMemorySummary {
        estimated_cache_slot_bytes: actual.estimated_cache_slot_bytes,
        ..super::super::RetainedBlockMemorySummary::default()
    };
    let mut seen = std::collections::HashSet::new();

    for item in render.items.iter() {
        let block_ptr = Rc::as_ptr(&item.block) as usize;
        if !seen.insert(block_ptr) {
            continue;
        }

        let block = item.block.as_ref();
        summary.estimated_render_ui_bytes += block.estimated_render_ui_bytes();
        summary.estimated_plain_line_bytes +=
            std::mem::size_of_val(block.plain_line_byte_lens.as_slice());
        summary.estimated_anchor_bytes += match &block.anchors {
            super::super::CachedLineAnchors::Explicit(anchors) => {
                std::mem::size_of_val(anchors.as_slice())
            }
            super::super::CachedLineAnchors::GeneratedRenderedLines => 0,
        };
    }

    summary
}
