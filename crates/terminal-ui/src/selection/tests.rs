use super::*;
use crate::{
    AppEffect, Sender, StartupBannerOptions,
    document::{DocumentAnchorRegion, DocumentLineAnchor},
    theme::default_palette,
    transcript::{LineAnchor, index_only_render_result, materialize_transcript_item_render_block},
};

#[test]
fn transcript_selection_survives_append_and_copies_using_anchor_bound_range() {
    let mut model = Model::new(StartupBannerOptions::default());
    model.set_window(24, 6);
    model.set_palette(default_palette(), true);
    model.transcript_mut().clear();
    model
        .transcript_mut()
        .append_message(Sender::Assistant, "alpha");
    model.sync_transcript_render();

    let layout = model.build_document_layout();
    let anchor = model
        .selection_point_for_mouse_with_layout(3, 0, &layout)
        .expect("selection should start inside the transcript line");
    let focus = model
        .selection_point_for_drag_mouse(7, 0)
        .expect("drag selection should clamp to the line end");
    model.start_selection(anchor);
    model.finish_selection(focus);

    model
        .transcript_mut()
        .append_message(Sender::Assistant, "beta");
    model.sync_transcript_render();

    assert_eq!(
        model.request_copy_selection(),
        Some(AppEffect::CopySelection("lpha".to_string()))
    );
}

#[test]
fn assistant_selection_uses_visual_inset_as_display_only_offset() {
    let mut model = Model::new(StartupBannerOptions::default());
    model.set_window(24, 6);
    model.set_palette(default_palette(), true);
    model.transcript_mut().clear();
    model
        .transcript_mut()
        .append_message(Sender::Assistant, "alpha");
    model.sync_transcript_render();

    let layout = model.build_document_layout();

    let inset_start = model
        .selection_point_for_mouse_with_layout(1, 0, &layout)
        .expect("assistant visual inset should be usable as a selection handle");
    let text_start = model
        .selection_point_for_mouse_with_layout(2, 0, &layout)
        .expect("the first visible assistant character should start selection");
    let end = model
        .selection_point_for_drag_mouse(7, 0)
        .expect("dragging past assistant text should clamp to content end");

    assert_eq!(inset_start.column(), 0);
    assert_eq!(text_start.column(), 0);
    assert_eq!(end.column(), 5);

    model.start_selection(inset_start);
    model.finish_selection(end);

    assert_eq!(
        model.request_copy_selection(),
        Some(AppEffect::CopySelection("alpha".to_string()))
    );
}

#[test]
fn request_copy_selection_exactizes_the_selected_transcript_range_on_demand() {
    let mut model = Model::new(StartupBannerOptions::default());
    model.set_window(18, 6);
    model.set_palette(default_palette(), true);
    model.transcript_mut().clear();
    model.transcript_mut().set_gap(0);
    for index in 0..48 {
        model.transcript_mut().append_message(
            Sender::Assistant,
            format!(
                "item {index}: alpha beta gamma delta epsilon zeta eta theta iota kappa lambda mu"
            ),
        );
    }
    model.sync_transcript_render();

    let start_block = materialize_transcript_item_render_block(
        model.transcript.item(0).expect("item 0 should exist"),
        18,
        default_palette(),
    );
    let end_block = materialize_transcript_item_render_block(
        model.transcript.item(2).expect("item 2 should exist"),
        18,
        default_palette(),
    );
    let start = SelectionPoint::new(
        DocumentLineAnchor {
            region: DocumentAnchorRegion::Transcript,
            transcript: LineAnchor {
                item_index: 0,
                item_anchor: start_block
                    .anchor_at(0)
                    .expect("first item should expose anchors"),
            },
            ..DocumentLineAnchor::default()
        },
        0,
    );
    let end = SelectionPoint::new(
        DocumentLineAnchor {
            region: DocumentAnchorRegion::Transcript,
            transcript: LineAnchor {
                item_index: 2,
                item_anchor: end_block
                    .anchor_at(0)
                    .expect("third item should expose anchors"),
            },
            ..DocumentLineAnchor::default()
        },
        4,
    );

    model.start_selection(start);
    model.finish_selection(end);

    let copied = model.request_copy_selection();
    assert!(
        matches!(copied, Some(AppEffect::CopySelection(text)) if !text.is_empty()),
        "copying a transcript selection should still produce text after on-demand exactization"
    );
    assert!(
        model.transcript_render.index.metrics[0..3]
            .iter()
            .all(|metrics| metrics.is_exact()),
        "selection copy should exactize the selected transcript item range before reading it"
    );
    assert!(
        model
            .transcript_render
            .index
            .metrics
            .iter()
            .enumerate()
            .any(|(item_index, metrics)| { item_index > 8 && metrics.is_estimated() }),
        "selection-driven exactization should stay local instead of settling the whole transcript"
    );
}

#[test]
fn request_copy_selection_exactizes_transcript_tail_when_selection_crosses_into_composer() {
    let mut model = Model::new(StartupBannerOptions::default());
    model.set_window(18, 6);
    model.set_palette(default_palette(), true);
    model.transcript_mut().clear();
    model.transcript_mut().set_gap(0);
    for index in 0..48 {
        model.transcript_mut().append_message(
            Sender::Assistant,
            format!(
                "item {index}: alpha beta gamma delta epsilon zeta eta theta iota kappa lambda mu"
            ),
        );
    }
    model.sync_transcript_render();
    model
        .composer_mut()
        .set_text_for_test("draft line one\ndraft line two");
    model.sync_composer_height();

    let selected_start_item = 30;
    let start_block = materialize_transcript_item_render_block(
        model
            .transcript
            .item(selected_start_item)
            .expect("selected transcript item should exist"),
        18,
        default_palette(),
    );
    let start = SelectionPoint::new(
        DocumentLineAnchor {
            region: DocumentAnchorRegion::Transcript,
            transcript: LineAnchor {
                item_index: selected_start_item,
                item_anchor: start_block
                    .anchor_at(0)
                    .expect("selected transcript item should expose anchors"),
            },
            ..DocumentLineAnchor::default()
        },
        0,
    );

    let layout = model.build_document_layout();
    let (composer_anchor, composer_end_column) = (0..layout.line_count())
        .find_map(|line_index| {
            let line = layout.selection_line_at(line_index)?;
            if line.anchor.region != DocumentAnchorRegion::Composer {
                return None;
            }

            let (_, end_column) = line.selectable.content_columns()?;
            Some((line.anchor, end_column))
        })
        .expect("composer selection line should exist");
    let end = SelectionPoint::new(composer_anchor, composer_end_column);

    model.start_selection(start);
    model.finish_selection(end);

    let mut expected = model.clone();
    expected
        .transcript
        .exactize_item_range(selected_start_item, expected.transcript.len());
    let index = expected.transcript.progressive_item_metrics_index();
    expected.transcript_render = std::rc::Rc::new(index_only_render_result(index));
    expected.transcript_render_version += 1;
    expected.document_runtime.transcript_cache = Default::default();
    expected.document_runtime.layout_cache = Default::default();
    expected.document_runtime.viewport_cache = Default::default();

    let copied = model.request_copy_selection();
    let expected_copied = expected.request_copy_selection();

    assert_eq!(
        copied, expected_copied,
        "copying across transcript and composer should read the transcript tail with exact metrics"
    );
    assert!(
        model.transcript_render.index.metrics[selected_start_item..]
            .iter()
            .all(|metrics| metrics.is_exact()),
        "copying a mixed transcript/tail selection should exactize the covered transcript tail before reading it"
    );
    assert!(
        model.transcript_render.index.metrics[..selected_start_item]
            .iter()
            .any(|metrics| metrics.is_estimated()),
        "mixed-selection exactization should stay local to the covered transcript tail"
    );
}
