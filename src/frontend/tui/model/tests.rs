use std::rc::Rc;

use super::*;
use crate::frontend::tui::{Sender, StyleMode, document::DocumentAnchorRegion};

fn progressive_exactization_fixture() -> Model {
    let mut model = Model::new_with_style_mode(HeroOptions::default(), StyleMode::Ms);
    model.transcript_mut().clear();
    model.transcript_mut().set_gap(0);
    for index in 0..40 {
        let content = match index % 4 {
            0 => {
                format!("# Overview {index} alpha beta gamma delta epsilon zeta eta theta iota")
            }
            1 => format!(
                "```rust\nfn helper_{index}() {{ println!(\"alpha beta gamma delta epsilon zeta eta theta iota\"); }}\n```"
            ),
            2 => format!(
                "| key | value |\n| --- | --- |\n| alpha beta gamma {index} | delta epsilon zeta eta theta |\n| iota kappa lambda | mu nu xi omicron pi |"
            ),
            _ => format!(
                "__init__ item {index} keeps markdown emphasis and heading-like text wrapped across the viewport"
            ),
        };
        model
            .transcript_mut()
            .append_message(Sender::Assistant, content);
    }
    model.set_window(18, 6);
    model.set_palette(default_palette(), true);
    model.sync_transcript_render();
    model
}

fn idle_refinement_fixture() -> Model {
    let mut model = progressive_exactization_fixture();
    model
        .composer_mut()
        .set_text_for_test("draft line one\ndraft line two\ndraft line three");
    model.composer_mut().move_to_begin_for_test();
    model.sync_composer_height();
    model
}

fn apply_scrolled_offset(model: &mut Model, offset: usize, manual_scroll: bool) {
    let layout = model.build_document_layout();
    let composer_offset = model.current_composer_viewport_offset(&layout, offset);
    model.apply_document_viewport_position(&layout, offset, composer_offset, false, manual_scroll);
}

#[test]
fn overflowing_document_bottom_slice_keeps_full_draft_height() {
    let mut model = Model::new_with_style_mode(HeroOptions::default(), StyleMode::Ms);
    model.set_window(20, 4);
    model.set_palette(default_palette(), true);
    model.composer_mut().set_text_for_test("1\n2\n3");
    model.sync_composer_height();
    model.sync_document_viewport_to_bottom();

    let layout = model.build_document_layout();
    assert_eq!(layout.composer_line_count, 3);

    let viewport = model.build_document_viewport(&layout);
    let rendered = viewport.plain_lines.clone();
    assert_eq!(
        rendered,
        vec![
            String::new(),
            "┃ 1".to_string(),
            "┃ 2".to_string(),
            "┃ 3".to_string(),
        ]
    );
}

#[test]
fn transcript_plain_items_use_assistant_markdown_render_path() {
    let mut model = Model::new(HeroOptions::default());
    model.transcript_mut().clear();
    model
        .transcript_mut()
        .append_message(Sender::Assistant, "# Overview of the API");

    assert_eq!(model.transcript_plain_items(), vec!["Overview of the API"]);
}

#[test]
fn height_only_resize_keeps_transcript_render_stable() {
    let mut model = Model::new_with_style_mode(HeroOptions::default(), StyleMode::Ms);
    model.transcript_mut().clear();
    model
        .transcript_mut()
        .append_message(Sender::Assistant, "alpha\nbeta\ngamma\ndelta");
    model.set_window(20, 4);
    model.set_palette(default_palette(), true);
    model.composer_mut().set_text_for_test("1\n2\n3\n4\n5\n6");
    model.sync_composer_height();

    let before_render_version = model.transcript_render_version;
    let before_render = Rc::clone(&model.transcript_render);
    let before_composer_height = model.composer.visible_height();

    model.set_window(20, 8);

    assert_eq!(
        model.transcript_render_version, before_render_version,
        "height-only resize should not trigger a transcript rerender"
    );
    assert!(
        Rc::ptr_eq(&before_render, &model.transcript_render),
        "height-only resize should keep reusing the current transcript render result"
    );
    assert!(
        model.composer.visible_height() > before_composer_height,
        "height-only resize should still update the tail/composer layout"
    );
}

#[test]
fn setting_the_same_palette_keeps_transcript_render_stable() {
    let mut model = Model::new_with_style_mode(HeroOptions::default(), StyleMode::Ms);
    model.transcript_mut().clear();
    model
        .transcript_mut()
        .append_message(Sender::Assistant, "alpha\nbeta");
    model.set_window(20, 4);
    model.set_palette(default_palette(), true);

    let before_render_version = model.transcript_render_version;
    let before_render = Rc::clone(&model.transcript_render);

    model.set_palette(default_palette(), true);

    assert_eq!(
        model.transcript_render_version, before_render_version,
        "setting the same palette should not trigger a transcript rerender"
    );
    assert!(
        Rc::ptr_eq(&before_render, &model.transcript_render),
        "setting the same palette should keep the existing transcript render result"
    );
}

#[test]
fn current_visible_transcript_window_matches_actual_viewport_line_indices() {
    #[derive(Clone, Copy)]
    enum TailState {
        Plain,
        StatusLine,
        CommandPanel,
    }

    for (name, style_mode, height, composer_text, tail_state) in [
        ("plain draft", StyleMode::Ms, 6, "draft", TailState::Plain),
        (
            "status line with tall draft",
            StyleMode::Ms,
            6,
            "1\n2\n3\n4\n5\n6\n7\n8",
            TailState::StatusLine,
        ),
        (
            "command panel",
            StyleMode::Ms,
            6,
            "/",
            TailState::CommandPanel,
        ),
        ("framed draft", StyleMode::Cc, 3, "draft", TailState::Plain),
        (
            "framed tall draft",
            StyleMode::Cc,
            4,
            "1\n2\n3\n4\n5\n6",
            TailState::Plain,
        ),
    ] {
        let mut model = Model::new_with_style_mode(HeroOptions::default(), style_mode);
        model.transcript_mut().clear();
        model.transcript_mut().set_gap(0);
        for index in 0..48 {
            model
                .transcript_mut()
                .append_message(Sender::Assistant, format!("item {index}"));
        }
        model.set_window(24, height);
        model.set_palette(default_palette(), true);
        match tail_state {
            TailState::Plain => {}
            TailState::StatusLine => {
                model.status_line_items = vec![StatusLineItem::GitBranch];
                model.git_branch = "main".to_string();
            }
            TailState::CommandPanel => {}
        }
        model.composer_mut().set_text_for_test(composer_text);
        model.sync_command_panel_navigation();
        model.sync_composer_height();
        model.sync_transcript_render();
        model.sync_document_viewport_to_bottom();

        let layout = model.build_document_layout();
        let visible_transcript_indices = model
            .document_viewport_line_indices(&layout)
            .into_iter()
            .filter(|line_index| *line_index < layout.transcript_line_count)
            .collect::<Vec<_>>();
        let expected_window = visible_transcript_indices
            .first()
            .copied()
            .map(|start| (start, visible_transcript_indices.len()));

        assert_eq!(
            model.current_visible_transcript_window(layout.transcript_line_count),
            expected_window,
            "{name} should derive the warmed transcript window from the actual viewport line indices"
        );
    }
}

#[test]
fn sync_transcript_render_evicts_warmed_transcript_blocks_during_metrics_only_refresh() {
    let mut model = Model::new_with_style_mode(HeroOptions::default(), StyleMode::Ms);
    model.set_window(24, 6);
    model.set_palette(default_palette(), true);
    model.transcript_mut().clear();
    model.transcript_mut().set_gap(0);
    for index in 0..96 {
        model
            .transcript_mut()
            .append_message(Sender::Assistant, format!("item {index}"));
    }

    model.sync_transcript_render();
    assert!(
        model
            .transcript
            .cached_screen_blocks_snapshot()
            .borrow()
            .is_empty(),
        "metrics-only sync should keep transcript blocks cold before any viewport materialization"
    );

    model.document_runtime.transcript_cache = Default::default();
    let _snapshot = model.current_document_transcript_snapshot();
    assert!(
        !model
            .transcript
            .cached_screen_blocks_snapshot()
            .borrow()
            .is_empty(),
        "document transcript snapshot should prewarm the visible transcript neighborhood"
    );

    model.sync_transcript_render();
    assert!(
        model
            .transcript
            .cached_screen_blocks_snapshot()
            .borrow()
            .is_empty(),
        "metrics-only refresh should evict warmed transcript blocks from the previous viewport snapshot"
    );
}

#[test]
fn current_visible_transcript_window_reresolves_manual_scroll_viewport_after_resize() {
    let mut model = Model::new_with_style_mode(HeroOptions::default(), StyleMode::Ms);
    model.transcript_mut().clear();
    model.transcript_mut().set_gap(0);
    model.transcript_mut().append_message(
            Sender::Assistant,
            "alpha beta gamma delta epsilon zeta eta theta iota kappa lambda mu nu xi omicron pi rho sigma tau upsilon phi chi psi omega",
        );
    model
        .transcript_mut()
        .append_message(Sender::Assistant, "target item");
    model
        .transcript_mut()
        .append_message(Sender::Assistant, "tail item");
    model.set_window(24, 4);
    model.set_palette(default_palette(), true);
    model.sync_transcript_render();

    let layout = model.build_document_layout();
    let target_document_line = (0..layout.line_count())
        .find(|&line_index| {
            layout.line_anchor_at(line_index).is_some_and(|anchor| {
                anchor.region == DocumentAnchorRegion::Transcript
                    && anchor.transcript.item_index == 1
            })
        })
        .expect("target item should exist in the initial transcript layout");
    let document_offset = target_document_line;
    model.apply_document_viewport_position(&layout, document_offset, 0, false, true);
    let preserved_viewport_state = model.current_document_viewport_state();

    model.set_window(12, 4);

    let transcript_line_count = model.transcript.item_metrics_index().line_count;
    let resized_layout = model.build_document_layout();
    let resized_target_document_line = (0..resized_layout.line_count())
        .find(|&line_index| {
            resized_layout
                .line_anchor_at(line_index)
                .is_some_and(|anchor| {
                    anchor.region == DocumentAnchorRegion::Transcript
                        && anchor.transcript.item_index == 1
                })
        })
        .expect("target item should still exist after resize");
    let expected_offset =
        preserved_viewport_state.resolve_offset(&resized_layout, model.document_viewport_height());
    let stale_offset = preserved_viewport_state.resolved_offset();
    let expected_window = model
        .document_viewport_line_indices_for_mode(
            &resized_layout,
            expected_offset,
            preserved_viewport_state.follow_bottom(),
            preserved_viewport_state.manual_scroll(),
        )
        .into_iter()
        .filter(|line_index| *line_index < transcript_line_count)
        .collect::<Vec<_>>();
    let stale_window = model
        .document_viewport_line_indices_for_mode(
            &resized_layout,
            stale_offset,
            preserved_viewport_state.follow_bottom(),
            preserved_viewport_state.manual_scroll(),
        )
        .into_iter()
        .filter(|line_index| *line_index < transcript_line_count)
        .collect::<Vec<_>>();
    let expected_window = expected_window
        .first()
        .copied()
        .map(|start| (start, expected_window.len()));

    assert_ne!(
        expected_offset, stale_offset,
        "test fixture should force manual-scroll restore to resolve a different offset after reflow (before={target_document_line}, after={resized_target_document_line})"
    );
    assert_ne!(
        stale_window
            .first()
            .copied()
            .map(|start| (start, stale_window.len())),
        expected_window,
        "test fixture should expose a mismatch between stale and re-resolved viewport windows"
    );
    assert_eq!(
        model.current_visible_transcript_window(transcript_line_count),
        expected_window,
        "manual-scroll prewarm should follow the re-resolved viewport that will be restored after resize"
    );
}

#[test]
fn current_visible_transcript_window_rebuilds_manual_scroll_index_when_reflow_keeps_line_count() {
    let mut model = Model::new_with_style_mode(HeroOptions::default(), StyleMode::Ms);
    model.transcript_mut().clear();
    model.transcript_mut().set_gap(0);
    model.transcript_mut().append_message(
            Sender::Assistant,
            "alpha beta gamma delta epsilon zeta eta theta iota kappa lambda mu nu xi omicron pi rho sigma tau upsilon phi chi psi omega",
        );
    model
        .transcript_mut()
        .append_message(Sender::Assistant, "target item");
    model
        .transcript_mut()
        .append_message(Sender::Assistant, "tail item");
    model.set_window(24, 4);
    model.set_palette(default_palette(), true);
    model.sync_transcript_render();

    let layout = model.build_document_layout();
    let target_document_line = (0..layout.line_count())
        .find(|&line_index| {
            layout.line_anchor_at(line_index).is_some_and(|anchor| {
                anchor.region == DocumentAnchorRegion::Transcript
                    && anchor.transcript.item_index == 1
            })
        })
        .expect("target item should exist in the initial transcript layout");
    model.apply_document_viewport_position(&layout, target_document_line, 0, false, true);

    let preserved_viewport_state = model.current_document_viewport_state();
    let stale_index = model.transcript_render.index.clone();
    model.width = 12;
    model.transcript.set_width(12);
    model.composer.set_width(12);
    let resized_index = model.transcript.progressive_item_metrics_index();
    let resized_layout = model.document_layout_for_transcript_index(resized_index.clone());
    let expected_offset =
        preserved_viewport_state.resolve_offset(&resized_layout, model.document_viewport_height());
    let expected_window_lines = model
        .document_viewport_line_indices_for_mode(
            &resized_layout,
            expected_offset,
            preserved_viewport_state.follow_bottom(),
            preserved_viewport_state.manual_scroll(),
        )
        .into_iter()
        .filter(|line_index| *line_index < resized_index.line_count)
        .collect::<Vec<_>>();
    let expected_window = expected_window_lines
        .first()
        .copied()
        .map(|start| (start, expected_window_lines.len()));
    let forced_stale_index = crate::frontend::tui::transcript::TranscriptItemMetricsIndex {
        line_count: resized_index.line_count,
        ..stale_index
    };
    model.transcript_render = Rc::new(index_only_render_result(forced_stale_index));

    assert_eq!(
        model.current_visible_transcript_window(resized_index.line_count),
        expected_window,
        "line_count equality alone should not let manual-scroll reuse a stale transcript index after reflow"
    );
}

#[test]
fn sync_transcript_render_keeps_transcript_blocks_cold_when_document_viewport_is_unavailable() {
    #[derive(Clone, Copy)]
    enum ViewportState {
        MissingWindow,
        ZeroHeight,
    }

    for viewport_state in [ViewportState::MissingWindow, ViewportState::ZeroHeight] {
        let mut model = Model::new_with_style_mode(HeroOptions::default(), StyleMode::Ms);
        model.set_window(24, 6);
        model.set_palette(default_palette(), true);
        model.transcript_mut().clear();
        model.transcript_mut().set_gap(0);
        for index in 0..96 {
            model
                .transcript_mut()
                .append_message(Sender::Assistant, format!("item {index}"));
        }

        model.sync_transcript_render();
        assert!(
            model
                .transcript
                .cached_screen_blocks_snapshot()
                .borrow()
                .is_empty(),
            "Phase E sync_transcript_render should stop after metrics rebuild even while the viewport is still available"
        );

        model.document_runtime.transcript_cache = Default::default();
        let _snapshot = model.current_document_transcript_snapshot();
        assert!(
            !model
                .transcript
                .cached_screen_blocks_snapshot()
                .borrow()
                .is_empty(),
            "test fixture should warm transcript blocks before making the viewport unavailable"
        );

        match viewport_state {
            ViewportState::MissingWindow => {
                model.has_window = false;
            }
            ViewportState::ZeroHeight => {
                model.height = 0;
            }
        }

        assert_eq!(model.document_viewport_height(), 0);
        let transcript_line_count = model.transcript.item_metrics_index().line_count;
        assert_eq!(
            model.current_visible_transcript_window(transcript_line_count),
            None,
            "unavailable viewport should not report any transcript line as visible"
        );

        model.sync_transcript_render();
        assert!(
            model
                .transcript
                .cached_screen_blocks_snapshot()
                .borrow()
                .is_empty(),
            "sync_transcript_render should keep transcript blocks cold when no viewport is available"
        );

        model.document_runtime.transcript_cache = Default::default();
        let _snapshot = model.current_document_transcript_snapshot();
        assert!(
            model
                .transcript
                .cached_screen_blocks_snapshot()
                .borrow()
                .is_empty(),
            "document transcript snapshots should not retain transcript blocks when no viewport is available"
        );
    }
}

#[test]
fn sync_transcript_render_keeps_current_viewport_exact_without_settling_entire_transcript() {
    let mut model = Model::new_with_style_mode(HeroOptions::default(), StyleMode::Ms);
    model.transcript_mut().clear();
    model.transcript_mut().set_gap(0);
    for index in 0..96 {
        model.transcript_mut().append_message(
            Sender::Assistant,
            format!(
                "item {index}: alpha beta gamma delta epsilon zeta eta theta iota kappa lambda mu"
            ),
        );
    }
    model.set_window(18, 6);
    model.set_palette(default_palette(), true);

    model.sync_transcript_render();

    let index = model.transcript_render.index.clone();
    let (start, count) = model
        .current_visible_transcript_window(index.line_count)
        .expect("bottom-follow viewport should expose a visible transcript window");
    let overscan_lines = crate::frontend::tui::transcript::viewport_overscan_line_budget(count);
    let (start_position, end_position) = index
        .summary_positions_for_line_window(start, count, overscan_lines)
        .expect("visible transcript window should resolve to summary positions");
    let exact_items = index.visible_items[start_position..=end_position]
        .iter()
        .map(|position| position.item_index)
        .collect::<Vec<_>>();

    assert!(
        !exact_items.is_empty(),
        "test fixture should expose at least one visible transcript item"
    );
    assert!(
        exact_items
            .iter()
            .all(|item_index| index.metrics[*item_index].is_exact()),
        "visible transcript window should be exact after sync_transcript_render"
    );
    assert!(
        index
            .metrics
            .iter()
            .enumerate()
            .any(|(item_index, metrics)| {
                !exact_items.contains(&item_index) && metrics.is_estimated()
            }),
        "progressive sync should leave non-visible transcript history estimated instead of settling the whole transcript"
    );
}

#[test]
fn composer_cursor_only_layout_refresh_reuses_long_composer_document() {
    let mut model = Model::new_with_style_mode(HeroOptions::default(), StyleMode::Cx);
    model.set_window(80, 24);
    model.set_palette(default_palette(), true);
    model
        .composer_mut()
        .replace_text_and_move_to_end("中英 mixed long composer text ".repeat(120));
    model.sync_composer_height();
    let _ = model.build_document_layout();

    crate::frontend::tui::composer::reset_render_document_call_count();
    model.composer_mut().move_to_begin();
    model.sync_document_viewport_for_composer_cursor();
    let _ = model.build_document_layout();

    assert_eq!(
        crate::frontend::tui::composer::render_document_call_count(),
        0,
        "cursor-only layout refresh should reuse the cached long composer document"
    );
}

#[test]
fn sync_transcript_render_does_not_schedule_idle_history_refinement() {
    let mut model = Model::new_with_style_mode(HeroOptions::default(), StyleMode::Ms);
    model.transcript_mut().clear();
    model.transcript_mut().set_gap(0);
    for index in 0..32 {
        model.transcript_mut().append_message(
            Sender::User,
            format!(
                "message {index}: {}",
                "long user text should stay estimated unless it enters the viewport ".repeat(10)
            ),
        );
    }
    model.set_window(24, 6);
    model.set_palette(default_palette(), true);

    model.sync_transcript_render();

    assert!(
        model.next_timeout_deadline().is_none(),
        "sync should not install a background timer that competes with scroll input"
    );
}

#[test]
fn build_document_layout_exactizes_a_newly_scrolled_transcript_window() {
    let mut model = Model::new_with_style_mode(HeroOptions::default(), StyleMode::Ms);
    model.transcript_mut().clear();
    model.transcript_mut().set_gap(0);
    for index in 0..96 {
        model.transcript_mut().append_message(
            Sender::Assistant,
            format!(
                "item {index}: alpha beta gamma delta epsilon zeta eta theta iota kappa lambda mu"
            ),
        );
    }
    model.set_window(18, 6);
    model.set_palette(default_palette(), true);
    model.sync_transcript_render();

    let tail_layout = model.build_document_layout();
    model.apply_document_viewport_position(&tail_layout, 0, 0, false, true);

    let _top_layout = model.build_document_layout();
    let index = model.transcript_render.index.clone();
    let (start, count) = model
        .current_visible_transcript_window(index.line_count)
        .expect("manually scrolled viewport should expose a visible transcript window");
    let overscan_lines = crate::frontend::tui::transcript::viewport_overscan_line_budget(count);

    assert!(
        index.line_window_is_exact(start, count, overscan_lines),
        "building a layout for a newly scrolled viewport should exactize that transcript window before document rendering"
    );
    assert!(
        index
            .metrics
            .iter()
            .enumerate()
            .any(|(item_index, metrics)| { item_index > 16 && metrics.is_estimated() }),
        "scroll-driven exactization should stay local instead of settling the whole transcript"
    );
}

#[test]
fn build_document_layout_stable_exactization_loop_keeps_visible_window_exact() {
    let base = progressive_exactization_fixture();
    let layout = base.clone().build_document_layout();
    let max_offset = layout
        .line_count()
        .saturating_sub(base.document_viewport_height());

    for manual_scroll in [false, true] {
        for offset in 0..=max_offset {
            let mut model = base.clone();
            apply_scrolled_offset(&mut model, offset, manual_scroll);

            let index = model.transcript_render.index.clone();
            let Some((start, count)) = model.current_visible_transcript_window_for_index(&index)
            else {
                continue;
            };
            let overscan_lines =
                crate::frontend::tui::transcript::viewport_overscan_line_budget(count);
            if index.line_window_is_exact(start, count, overscan_lines) {
                continue;
            }

            let index = model.exactize_visible_transcript_window_until_stable(index);
            let Some((next_start, next_count)) =
                model.current_visible_transcript_window_for_index(&index)
            else {
                continue;
            };
            let next_overscan_lines =
                crate::frontend::tui::transcript::viewport_overscan_line_budget(next_count);
            assert!(
                index.line_window_is_exact(next_start, next_count, next_overscan_lines),
                "stable exactization should converge the visible transcript window to exact metrics at offset {offset} (manual_scroll={manual_scroll})"
            );
        }
    }
}

#[test]
fn exactize_line_window_keeps_manual_scroll_window_local_after_reflow() {
    let mut model = progressive_exactization_fixture();
    let offset = 10;
    apply_scrolled_offset(&mut model, offset, true);

    let index = model.transcript_render.index.clone();
    let (start, count) = model
        .current_visible_transcript_window_for_index(&index)
        .expect("manual-scroll viewport should expose a visible transcript window");
    let overscan_lines = crate::frontend::tui::transcript::viewport_overscan_line_budget(count);
    assert!(
        !index.line_window_is_exact(start, count, overscan_lines),
        "test fixture should keep manual offset {offset} on the progressive path before render-time exactization"
    );

    let expected_item_range = index
        .item_range_for_line_window(start, count, overscan_lines)
        .expect("visible transcript window should resolve to an item range");
    let actual_item_range = model
        .transcript
        .exactize_line_window(start, count, overscan_lines)
        .expect("exactization should cover the visible transcript items");

    assert_eq!(
        actual_item_range, expected_item_range,
        "exactize_line_window should only exactize the item range resolved for the requested line window before reflow"
    );
}

#[test]
fn build_document_layout_keeps_manual_scroll_viewport_stable_without_exactization_reflow() {
    let base = progressive_exactization_fixture();
    let layout = base.clone().build_document_layout();
    let max_offset = layout
        .line_count()
        .saturating_sub(base.document_viewport_height());

    for offset in 0..=max_offset {
        let mut model = base.clone();
        apply_scrolled_offset(&mut model, offset, true);
        let preserved_viewport_state = model.document_runtime.viewport_state.clone();

        let layout = model.build_document_layout();
        let expected_offset =
            preserved_viewport_state.resolve_offset(&layout, model.document_viewport_height());
        let viewport = model.build_document_viewport(&layout);

        assert_eq!(
            model.document_runtime.viewport_y, expected_offset,
            "manual-scroll viewport should stay aligned with the preserved transcript anchor at offset {offset}"
        );
        assert_eq!(
            model.document_runtime.viewport_state.resolved_offset(),
            expected_offset,
            "viewport state should store the stable manual-scroll offset at offset {offset}"
        );
        assert_eq!(
            viewport.resolved_offset, expected_offset,
            "document viewport materialization should keep using the resolved manual-scroll offset at offset {offset}"
        );
    }
}

#[test]
fn build_document_layout_resyncs_idle_viewport_after_exactization_reflow() {
    let base = idle_refinement_fixture();
    let layout = base.clone().build_document_layout();
    let max_offset = layout
        .line_count()
        .saturating_sub(base.document_viewport_height());
    let mut candidate = None;

    for offset in 0..=max_offset {
        let mut probe = base.clone();
        apply_scrolled_offset(&mut probe, offset, false);
        if probe.document_runtime.follow_bottom || probe.document_runtime.manual_scroll {
            continue;
        }

        let stale_offset = probe.document_runtime.viewport_state.resolved_offset();
        let mut exactized = probe.clone();
        let layout = exactized.build_document_layout();
        let cursor_hidden_with_stale_offset = layout.cursor_y < stale_offset
            || layout.cursor_y >= stale_offset.saturating_add(exactized.document_viewport_height());

        let mut expected = exactized.clone();
        expected.sync_document_viewport_for_composer_cursor();
        if cursor_hidden_with_stale_offset && expected.document_runtime.viewport_y != stale_offset {
            candidate = Some(offset);
            break;
        }
    }

    let offset = candidate.expect(
            "test fixture should expose a non-follow-bottom viewport whose stale offset hides the composer cursor after render-time exactization",
        );

    let mut model = base;
    apply_scrolled_offset(&mut model, offset, false);

    let mut expected = model.clone();
    let _ = expected.build_document_layout();
    expected.sync_document_viewport_for_composer_cursor();

    let layout = model.build_document_layout();
    let viewport = model.build_document_viewport(&layout);

    assert_eq!(
        model.document_runtime.viewport_y, expected.document_runtime.viewport_y,
        "render-time exactization should immediately rerun the idle viewport cursor sync"
    );
    assert_eq!(
        model.composer.viewport_offset(),
        expected.composer.viewport_offset(),
        "composer viewport should stay aligned with the cursor-tracking sync after exactization"
    );
    assert_eq!(
        model.document_runtime.viewport_state.resolved_offset(),
        expected.document_runtime.viewport_y,
        "viewport state should store the cursor-tracking offset after exactization"
    );
    assert_eq!(
        viewport.resolved_offset, expected.document_runtime.viewport_y,
        "document viewport materialization should use the cursor-tracking offset after exactization"
    );
    assert!(
        layout.cursor_y >= viewport.resolved_offset
            && layout.cursor_y
                < viewport
                    .resolved_offset
                    .saturating_add(model.document_viewport_height()),
        "render-time exactization should leave the active composer cursor inside the visible document viewport"
    );
}
