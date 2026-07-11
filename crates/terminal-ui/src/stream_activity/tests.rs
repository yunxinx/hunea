use ratatui::style::Modifier;

use super::*;
use crate::{StartupBannerOptions, theme::default_palette, transcript::TranscriptItem};
use runtime_domain::session::{
    RuntimeToolActivity, RuntimeToolActivityContent, RuntimeToolActivityStatus, RuntimeToolKind,
};

#[test]
fn stream_activity_tail_cache_key_changes_when_elapsed_text_changes() {
    let mut model = Model::new(StartupBannerOptions::default());
    model.set_window(50, 6);
    model.set_palette(default_palette(), true);
    model.show_stream_activity_with_header("Working");

    let started_at = model.stream_activity.as_ref().unwrap().started_at;
    let initial_key = model.stream_activity_frame_key(started_at);
    let later_key =
        model.stream_activity_frame_key(started_at + std::time::Duration::from_millis(1_200));

    assert_ne!(
        initial_key, later_key,
        "activity cache key must change when the visible elapsed timer changes"
    );
}

#[test]
fn stream_activity_line_animates_without_changing_plain_text() {
    let mut model = Model::new(StartupBannerOptions::default());
    model.set_window(50, 6);
    model.set_palette(default_palette(), true);
    model.show_stream_activity_with_header("Working");

    let started_at = model.stream_activity.as_ref().unwrap().started_at;
    let first = model.current_stream_activity_render_result_at(started_at);
    let second = model.current_stream_activity_render_result_at(
        started_at + std::time::Duration::from_millis(900),
    );
    let first_line = first.line.expect("activity line should render");
    let second_line = second.line.expect("activity line should render");

    assert_eq!(first.plain_line, "• Working (0s • esc 2x to interrupt)");
    assert_eq!(
        first.selectable.content_columns().map(|(start, _)| start),
        Some(0)
    );
    assert_eq!(second.plain_line, first.plain_line);
    assert_eq!(
        first_line.spans.first().map(|span| span.content.as_ref()),
        Some("•")
    );
    assert!(
        first_line.spans.len() > 8,
        "header shimmer should still style the running text per character"
    );
    assert!(
        first_line
            .spans
            .iter()
            .any(|span| span.style.add_modifier.contains(Modifier::BOLD))
    );
    assert!(
        !first_line
            .spans
            .iter()
            .all(|span| span.style.add_modifier.contains(Modifier::ITALIC))
    );
    assert_ne!(
        first_line
            .spans
            .iter()
            .map(|span| span.style)
            .collect::<Vec<_>>(),
        second_line
            .spans
            .iter()
            .map(|span| span.style)
            .collect::<Vec<_>>(),
        "shimmer styles should advance while the visible text stays stable"
    );
}

#[test]
fn stream_activity_glyph_uses_breathing_style() {
    let palette = default_palette();
    let started_at = std::time::Instant::now();
    let first = activity_glyph_span_at(palette, started_at, started_at);
    let second = activity_glyph_span_at(
        palette,
        started_at,
        started_at + std::time::Duration::from_millis(800),
    );

    assert_eq!(first.content.as_ref(), "•");
    assert_eq!(second.content.as_ref(), "•");
    assert_ne!(
        first.style, second.style,
        "glyph breathing should animate the marker independently from header shimmer"
    );
}

#[test]
fn clear_stream_activity_completes_open_exploration_marker() {
    let palette = default_palette();
    let mut model = Model::new(StartupBannerOptions::default());
    model.set_palette(palette, true);
    model.show_stream_activity_with_header("Working");
    model.append_runtime_tool_activity_from_runtime(RuntimeToolActivity {
        activity_id: "call-list".to_string(),
        title: "List Directory crates".to_string(),
        kind: RuntimeToolKind::Search,
        status: RuntimeToolActivityStatus::Completed,
        content: vec![RuntimeToolActivityContent::Text("tui/".to_string())],
        locations: Vec::new(),
        raw_input: Some(serde_json::json!({ "path": "crates" }).into()),
        raw_output: Some("tui/".into()),
    });

    assert_eq!(
        first_tool_result_marker_color(&mut model),
        Some(palette.main)
    );

    model.clear_stream_activity();

    assert_eq!(
        first_tool_result_marker_color(&mut model),
        Some(palette.quote)
    );
}

#[test]
fn finish_stream_activity_skips_work_duration_until_timer_exceeds_thirty_seconds() {
    let mut model = Model::new(StartupBannerOptions::default());
    model.transcript_mut().clear();
    model.show_stream_activity_with_header("Working");

    let started_at = model.stream_activity.as_ref().unwrap().started_at;
    model.finish_stream_activity_with_work_summary_at(started_at + Duration::from_secs(30));

    assert!(model.transcript_plain_items().is_empty());
    assert!(!model.current_stream_activity_render_result().has_content);
}

#[test]
fn finish_stream_activity_appends_work_duration_after_thirty_seconds() {
    let mut model = Model::new(StartupBannerOptions::default());
    model.transcript_mut().clear();
    model.set_window(32, 6);
    model.show_stream_activity_with_header("Working");

    let started_at = model.stream_activity.as_ref().unwrap().started_at;
    model.finish_stream_activity_with_work_summary_at(started_at + Duration::from_secs(31));

    assert_eq!(
        model.transcript_plain_items(),
        vec!["─ Worked for 31s ───────────────".to_string()]
    );
    assert!(!model.current_stream_activity_render_result().has_content);
}

fn first_tool_result_marker_color(model: &mut Model) -> Option<ratatui::style::Color> {
    let palette = model.palette;
    let items = model.transcript_mut().items_snapshot();
    let item = items.iter().find_map(|item| match item.as_ref() {
        TranscriptItem::ToolResult(item) => Some(item),
        _ => None,
    })?;
    item.render_lines(80, palette)
        .first()
        .and_then(|line| line.spans.first())
        .and_then(|span| span.style.fg)
}

#[test]
fn stream_activity_pause_hides_and_resume_excludes_paused_duration() {
    let mut model = Model::new(StartupBannerOptions::default());
    model.set_window(50, 6);
    model.set_palette(default_palette(), true);
    model.show_stream_activity_with_header("Working");
    let started_at = model.stream_activity.as_ref().unwrap().started_at;
    let pause_at = started_at + Duration::from_secs(2);
    let resume_at = pause_at + Duration::from_secs(30);

    model.pause_stream_activity_at(pause_at);
    assert!(
        !model
            .current_stream_activity_render_result_at(resume_at)
            .has_content,
        "paused activity should be hidden"
    );
    assert_eq!(
        model.stream_activity_next_frame_deadline_at(resume_at),
        None
    );

    model.resume_stream_activity_at(resume_at);
    let resumed = model
        .current_stream_activity_render_result_at(resume_at + Duration::from_secs(1))
        .plain_line;
    assert!(
        resumed.contains("(3s"),
        "activity should resume from the elapsed time before approval wait: {resumed}"
    );
    assert!(
        !resumed.contains("33s"),
        "approval wait should not be counted into elapsed time: {resumed}"
    );
}

#[test]
fn stream_activity_retry_header_freezes_elapsed_timer() {
    let mut model = Model::new(StartupBannerOptions::default());
    model.set_window(70, 6);
    model.set_palette(default_palette(), true);
    model.show_stream_activity_with_header("Working");
    model.backdate_stream_activity_started_at_for_test(Duration::from_secs(10));

    model.show_stream_activity_retry_header("Reconnecting... 1/3");

    let retry_line = model
        .current_stream_activity_render_result_at(Instant::now() + Duration::from_secs(20))
        .plain_line;
    assert!(retry_line.contains("Reconnecting... 1/3"));
    assert!(retry_line.contains("(10s"));
    assert!(!retry_line.contains("30s"));
}

#[test]
fn stream_activity_multiple_retry_windows_are_excluded_from_elapsed_timer() {
    let mut model = Model::new(StartupBannerOptions::default());
    model.set_window(70, 6);
    model.set_palette(default_palette(), true);
    model.show_stream_activity_with_header("Working");

    let started_at = model.stream_activity.as_ref().unwrap().started_at;
    let first_retry_at = started_at + Duration::from_secs(10);
    let first_resume_at = first_retry_at + Duration::from_secs(5);
    let second_retry_at = first_resume_at + Duration::from_secs(7);
    let second_resume_at = second_retry_at + Duration::from_secs(3);

    model.show_stream_activity_retry_header_at("Reconnecting... 1/3", first_retry_at);
    model.clear_stream_activity_retry_header_at(first_resume_at);
    model.show_stream_activity_retry_header_at("Reconnecting... 2/3", second_retry_at);
    model.clear_stream_activity_retry_header_at(second_resume_at);

    let resumed = model
        .current_stream_activity_render_result_at(second_resume_at + Duration::from_secs(2))
        .plain_line;
    assert!(resumed.contains("(19s"));
    assert!(!resumed.contains("27s"));
}

#[test]
fn stream_activity_line_tweens_output_token_estimate_to_target() {
    let mut model = Model::new(StartupBannerOptions::default());
    model.set_window(70, 6);
    model.set_palette(default_palette(), true);
    model.show_stream_activity_with_header("Working");

    let started_at = model.stream_activity.as_ref().unwrap().started_at;
    model.set_stream_activity_output_tokens_at(24, started_at);

    let early = model
        .current_stream_activity_render_result_at(started_at + std::time::Duration::from_millis(80))
        .plain_line;
    let settled = model
        .current_stream_activity_render_result_at(
            started_at + std::time::Duration::from_millis(120),
        )
        .plain_line;

    assert!(
        early.contains("tokens"),
        "activity should expose streaming token feedback before settling"
    );
    assert!(
        !early.contains("24 tokens"),
        "token feedback should tween instead of jumping to the target"
    );
    assert!(
        settled.contains("24 tokens"),
        "token feedback should eventually reach the latest target"
    );
}

#[test]
fn stream_activity_token_indicator_uses_single_directional_total() {
    let mut model = Model::new(StartupBannerOptions::default());
    model.set_window(80, 6);
    model.set_palette(default_palette(), true);
    model.show_stream_activity_with_header("Working");

    let started_at = model.stream_activity.as_ref().unwrap().started_at;
    model.set_stream_activity_output_tokens_at(200, started_at);
    let output_line = model
        .current_stream_activity_render_result_at(started_at + Duration::from_millis(120))
        .plain_line;
    assert!(output_line.contains("↓ 200 tokens"));

    model.add_stream_activity_input_tokens_at(100, started_at + Duration::from_millis(140));
    let input_line = model
        .current_stream_activity_render_result_at(started_at + Duration::from_millis(260))
        .plain_line;
    assert!(input_line.contains("↑ 300 tokens"));
    assert!(!input_line.contains("↓ 200 tokens"));
}

#[test]
fn stream_activity_thinking_segment_renders_between_timer_and_tokens() {
    let mut model = Model::new(StartupBannerOptions::default());
    model.set_window(80, 6);
    model.set_palette(default_palette(), true);
    model.show_stream_activity_with_header("Working");

    let started_at = model.stream_activity.as_ref().unwrap().started_at;
    model.set_stream_activity_thinking(true);
    model.set_stream_activity_output_tokens_at(12, started_at);

    let thinking_line = model
        .current_stream_activity_render_result_at(started_at + Duration::from_millis(120))
        .plain_line;
    assert!(thinking_line.contains("(0s • thinking • ↓ 12 tokens"));

    model.set_stream_activity_thinking(false);
    let content_line = model
        .current_stream_activity_render_result_at(started_at + Duration::from_millis(140))
        .plain_line;
    assert!(!content_line.contains("thinking"));
    assert!(content_line.contains("(0s • ↓ 12 tokens"));
}

#[test]
fn stream_activity_token_indicator_compacts_thousands_to_k_unit() {
    let mut model = Model::new(StartupBannerOptions::default());
    model.set_window(80, 6);
    model.set_palette(default_palette(), true);
    model.show_stream_activity_with_header("Working");

    let started_at = model.stream_activity.as_ref().unwrap().started_at;
    model.set_stream_activity_output_tokens_at(999, started_at);
    let under_k_line = model
        .current_stream_activity_render_result_at(started_at + Duration::from_millis(120))
        .plain_line;
    assert!(under_k_line.contains("↓ 999 tokens"));

    model.set_stream_activity_output_tokens_at(1_200, started_at + Duration::from_millis(140));
    let k_line = model
        .current_stream_activity_render_result_at(started_at + Duration::from_millis(260))
        .plain_line;
    assert!(k_line.contains("↓ 1.2k tokens"));
    assert!(!k_line.contains("1200 tokens"));
}

#[test]
fn stream_activity_token_indicator_uses_fast_tick_until_target_or_stale() {
    let mut model = Model::new(StartupBannerOptions::default());
    model.set_window(80, 6);
    model.set_palette(default_palette(), true);
    model.show_stream_activity_with_header("Working");

    let started_at = model.stream_activity.as_ref().unwrap().started_at;
    model.set_stream_activity_output_tokens_at(36, started_at);

    assert_eq!(
        model
            .stream_activity
            .as_ref()
            .unwrap()
            .frame_interval_at(started_at + Duration::from_millis(33)),
        Duration::from_millis(33)
    );
    assert_eq!(
        model
            .stream_activity
            .as_ref()
            .unwrap()
            .frame_interval_at(started_at + Duration::from_millis(130)),
        Duration::from_millis(80),
        "token tick should stop once the displayed value catches the target"
    );

    model.set_stream_activity_output_tokens_at(72, started_at + Duration::from_millis(200));
    assert_eq!(
        model
            .stream_activity
            .as_ref()
            .unwrap()
            .frame_interval_at(started_at + Duration::from_millis(600)),
        Duration::from_millis(80),
        "stale token snapshots should not keep the fast tick alive"
    );
}

#[test]
fn stream_activity_next_frame_deadline_is_anchored_to_started_at() {
    let mut model = Model::new(StartupBannerOptions::default());
    model.show_stream_activity_with_header("Working");
    let started_at = model.stream_activity.as_ref().unwrap().started_at;
    let now = started_at + Duration::from_millis(70);

    assert_eq!(
        model.stream_activity_next_frame_deadline_at(now),
        Some(started_at + STREAM_ACTIVITY_FRAME_INTERVAL),
    );
}

#[test]
fn document_layout_rebuilds_when_stream_activity_tick_changes() {
    let mut model = Model::new(StartupBannerOptions::default());
    model.transcript_mut().clear();
    model.sync_transcript_render();
    model.set_window(50, 6);
    model.set_palette(default_palette(), true);
    model.show_stream_activity_with_header("Working");

    let initial = model.build_document_layout(crate::frame_time::FrameRenderContext::capture());
    assert!(
        initial
            .tail
            .text_line_at(0)
            .is_some_and(|line| line.contains("Working (0s")),
        "activity should include the current elapsed segment"
    );

    model.stream_activity.as_mut().unwrap().started_at -= std::time::Duration::from_secs(2);
    let updated = model.build_document_layout(crate::frame_time::FrameRenderContext::capture());

    assert!(
        updated
            .tail
            .text_line_at(0)
            .is_some_and(|line| line.contains("Working (2s")),
        "outer document layout cache must not hide updated activity text"
    );
}

#[test]
fn stream_activity_frame_reuses_the_long_composer_document() {
    let mut model = Model::new(StartupBannerOptions::default());
    model.transcript_mut().clear();
    model.sync_transcript_render();
    model.set_window(80, 24);
    model.set_palette(default_palette(), true);
    model
        .composer_mut()
        .reset_text_and_move_to_end("中英 mixed long composer text ".repeat(240));
    model.sync_composer_height();
    model.show_stream_activity_with_header("Working");
    let started_at = model.stream_activity.as_ref().unwrap().started_at;
    let initial_context = crate::frame_time::FrameRenderContext::new(started_at);
    let initial = model.build_document_tail_layout(initial_context);

    crate::composer::reset_visual_lines_call_count();
    let next_frame = model
        .stream_activity_next_frame_deadline_at(started_at)
        .expect("active stream should expose its next frame");
    let updated =
        model.build_document_tail_layout(crate::frame_time::FrameRenderContext::new(next_frame));

    assert_eq!(
        crate::composer::visual_lines_call_count(),
        0,
        "activity-only frames must reuse the stable composer document"
    );
    assert!(
        initial.shares_stable_layout_with(&updated),
        "activity-only frames must share the stable tail allocation"
    );
}

#[test]
fn stream_activity_state_updates_reuse_the_stable_tail_layout() {
    let mut model = Model::new(StartupBannerOptions::default());
    model.transcript_mut().clear();
    model.sync_transcript_render();
    model.set_window(80, 24);
    model.set_palette(default_palette(), true);
    model
        .composer_mut()
        .reset_text_and_move_to_end("中英 mixed long composer text ".repeat(240));
    model.sync_composer_height();
    model.show_stream_activity_with_header("Working");
    let initial =
        model.build_document_tail_layout(crate::frame_time::FrameRenderContext::capture());

    crate::composer::reset_visual_lines_call_count();
    model.set_stream_activity_thinking(true);
    model.set_stream_activity_output_tokens(42);
    let updated =
        model.build_document_tail_layout(crate::frame_time::FrameRenderContext::capture());

    assert!(
        !std::rc::Rc::ptr_eq(&initial, &updated),
        "activity state changes must rebuild the lightweight final tail view"
    );
    assert_eq!(
        crate::composer::visual_lines_call_count(),
        0,
        "activity state changes must not rerender the stable composer document"
    );
    assert!(
        initial.shares_stable_layout_with(&updated),
        "activity state changes must share the stable tail allocation"
    );
}
