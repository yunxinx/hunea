const EXPECTED_MAX_RECENT_RENDER_BLOCKS: usize = 48;

use super::*;
use crate::transcript::{
    render_markdown_metrics_call_count, reset_render_markdown_metrics_call_count,
};
use crate::{
    StartupBannerOptions, StyleMode,
    message::{
        message_item_render_cache_key_call_count, reset_message_item_render_cache_key_call_count,
        reset_user_message_projection_plain_line_len_call_count,
        user_message_projection_plain_line_len_call_count,
    },
    theme::{default_palette, terminal_default_palette},
};
use ratatui::style::Color;
use runtime_domain::session::{
    RuntimeToolActivity, RuntimeToolActivityContent, RuntimeToolActivityStatus,
    RuntimeToolActivityUpdate, RuntimeToolKind,
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
    transcript.append_startup_banner(StartupBannerOptions {
        app_name: Some("Hunea".to_string()),
        version: Some("v0.1.0".to_string()),
        model_name: None,
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
fn tool_result_is_display_only_and_not_assistant_message() {
    let mut transcript = Transcript::new(default_palette());
    transcript.append_tool_result("Ran cargo test", ToolResultKind::Ran);

    assert_eq!(transcript.source_messages(), Vec::<(Sender, String)>::new());
    let item = transcript.item(0).expect("tool result item should exist");
    assert!(!item.is_assistant_message());
    assert_eq!(
        item.render_plain_lines(80, default_palette()),
        vec!["● Ran cargo test".to_string()]
    );
}

#[test]
fn tool_activity_uses_compact_and_detailed_rendering_modes() {
    let mut transcript = Transcript::new(default_palette());
    transcript.append_runtime_tool_activity(RuntimeToolActivity {
        activity_id: "call-1".to_string(),
        title: "Shell: cargo check".to_string(),
        kind: RuntimeToolKind::Other,
        status: RuntimeToolActivityStatus::Completed,
        content: vec![RuntimeToolActivityContent::Text("summary".to_string())],
        locations: Vec::new(),
        raw_input: None,
        raw_output: Some(
            (1..=14)
                .map(|line| format!("line {line}"))
                .collect::<Vec<_>>()
                .join("\n")
                .into(),
        ),
    });

    let compact = transcript.plain_items().join("\n");
    assert!(compact.contains("● Ran cargo check"));
    assert!(compact.contains("line 1"));
    assert!(compact.contains("line 14"));
    assert!(compact.contains("… +10 lines (ctrl + t to view transcript)"));
    assert!(!compact.contains("line 7"));
    assert!(!compact.contains("Completed"));
    assert!(!compact.contains("[Other]"));
    assert!(!compact.contains("Shell:"));

    transcript.set_tool_activity_render_mode(ToolActivityRenderMode::Detailed);
    let detailed = transcript.plain_items().join("\n");
    assert!(detailed.contains("line 7"));
    assert!(!detailed.contains("ctrl + t to view transcript"));
}

#[test]
fn assistant_display_trims_outer_blank_lines_without_mutating_source_content() {
    let mut transcript = Transcript::new(default_palette());
    transcript.set_tool_activity_render_mode(ToolActivityRenderMode::Detailed);
    transcript.append_message(Sender::Assistant, "文件已创建成功。\n\n");
    transcript.append_runtime_tool_activity(RuntimeToolActivity {
        activity_id: "call-wc".to_string(),
        title: "Shell: wc -m Temp.md".to_string(),
        kind: RuntimeToolKind::Execute,
        status: RuntimeToolActivityStatus::Completed,
        content: Vec::new(),
        locations: Vec::new(),
        raw_input: Some(serde_json::json!({ "command": "wc -m Temp.md" }).into()),
        raw_output: Some("705 Temp.md".into()),
    });

    assert_eq!(
        transcript.render().all_plain_lines(),
        vec![
            "文件已创建成功。".to_string(),
            "".to_string(),
            "$ wc -m Temp.md".to_string(),
            "705 Temp.md".to_string(),
        ]
    );
    assert_eq!(
        transcript.source_messages(),
        vec![(Sender::Assistant, "文件已创建成功。\n\n".to_string())]
    );
}

#[test]
fn single_exploration_tool_activity_renders_as_standalone_transcript_item() {
    let mut transcript = Transcript::new(default_palette());

    transcript.append_runtime_tool_activity(RuntimeToolActivity {
        activity_id: "call-list-root".to_string(),
        title: "List Directory".to_string(),
        kind: RuntimeToolKind::Search,
        status: RuntimeToolActivityStatus::Completed,
        content: vec![RuntimeToolActivityContent::Text(
            "Cargo.toml\ncrates/\ndocs/".to_string(),
        )],
        locations: Vec::new(),
        raw_input: Some(serde_json::json!({ "path": "." }).into()),
        raw_output: Some("Cargo.toml\ncrates/\ndocs/".into()),
    });

    assert_eq!(transcript.plain_items(), vec!["● List .".to_string()]);
    assert_eq!(transcript.item_metrics_index().line_count, 1);
}

#[test]
fn exploration_tool_activities_coalesce_into_single_transcript_item() {
    let mut transcript = Transcript::new(default_palette());

    transcript.append_runtime_tool_activity(RuntimeToolActivity {
        activity_id: "call-list-root".to_string(),
        title: "List Directory".to_string(),
        kind: RuntimeToolKind::Search,
        status: RuntimeToolActivityStatus::Completed,
        content: vec![RuntimeToolActivityContent::Text(
            "Cargo.toml\ncrates/\ndocs/".to_string(),
        )],
        locations: Vec::new(),
        raw_input: Some(serde_json::json!({ "path": "." }).into()),
        raw_output: Some("Cargo.toml\ncrates/\ndocs/".into()),
    });
    transcript.append_runtime_tool_activity(RuntimeToolActivity {
        activity_id: "call-read-cargo".to_string(),
        title: "Read Cargo.toml".to_string(),
        kind: RuntimeToolKind::Read,
        status: RuntimeToolActivityStatus::Completed,
        content: vec![RuntimeToolActivityContent::Text(
            "[package]\nname = \"hunea\"".to_string(),
        )],
        locations: Vec::new(),
        raw_input: Some(serde_json::json!({ "path": "Cargo.toml" }).into()),
        raw_output: Some("[package]\nname = \"hunea\"".into()),
    });
    transcript.append_runtime_tool_activity(RuntimeToolActivity {
        activity_id: "call-list-crates".to_string(),
        title: "List Directory ./crates".to_string(),
        kind: RuntimeToolKind::Search,
        status: RuntimeToolActivityStatus::Completed,
        content: vec![RuntimeToolActivityContent::Text(
            "app/\ncore/\ntui/".to_string(),
        )],
        locations: Vec::new(),
        raw_input: Some(serde_json::json!({ "path": "./crates" }).into()),
        raw_output: Some("app/\ncore/\ntui/".into()),
    });

    assert_eq!(
        transcript.plain_items(),
        vec!["● Explored\n  └ List .\n    Read Cargo.toml\n    List crates".to_string()]
    );
    assert_eq!(transcript.item_metrics_index().line_count, 4);
}

#[test]
fn exploration_group_keeps_activity_ids_and_coalesces_adjacent_reads() {
    let mut transcript = Transcript::new(default_palette());

    let search_index = transcript.append_runtime_tool_activity(RuntimeToolActivity {
        activity_id: "call-search".to_string(),
        title: "Search shimmer_spans in crates/terminal-ui".to_string(),
        kind: RuntimeToolKind::Search,
        status: RuntimeToolActivityStatus::Completed,
        content: vec![RuntimeToolActivityContent::Text(
            "crates/terminal-ui/src/shimmer.rs:12:shimmer_spans".to_string(),
        )],
        locations: Vec::new(),
        raw_input: None,
        raw_output: Some("crates/terminal-ui/src/shimmer.rs:12:shimmer_spans".into()),
    });
    let first_read_index = transcript.append_runtime_tool_activity(RuntimeToolActivity {
        activity_id: "call-read-shimmer".to_string(),
        title: "Read shimmer.rs".to_string(),
        kind: RuntimeToolKind::Read,
        status: RuntimeToolActivityStatus::Completed,
        content: vec![RuntimeToolActivityContent::Text(
            "pub fn shimmer() {}".to_string(),
        )],
        locations: Vec::new(),
        raw_input: Some(serde_json::json!({ "path": "shimmer.rs" }).into()),
        raw_output: Some("pub fn shimmer() {}".into()),
    });
    let second_read_index = transcript.append_runtime_tool_activity(RuntimeToolActivity {
        activity_id: "call-read-status".to_string(),
        title: "Read status_indicator_widget.rs".to_string(),
        kind: RuntimeToolKind::Read,
        status: RuntimeToolActivityStatus::Completed,
        content: vec![RuntimeToolActivityContent::Text(
            "pub fn status_indicator() {}".to_string(),
        )],
        locations: Vec::new(),
        raw_input: Some(serde_json::json!({ "path": "status_indicator_widget.rs" }).into()),
        raw_output: Some("pub fn status_indicator() {}".into()),
    });

    assert_eq!(search_index, 0);
    assert_eq!(first_read_index, 0);
    assert_eq!(second_read_index, 0);
    assert_eq!(
        transcript.runtime_tool_activity_index("call-read-status"),
        Some(0)
    );

    transcript.update_runtime_tool_activity(
        0,
        RuntimeToolActivityUpdate {
            activity_id: "call-read-status".to_string(),
            title: Some("Read status.rs".to_string()),
            raw_input: Some(serde_json::json!({ "path": "status.rs" }).into()),
            ..RuntimeToolActivityUpdate::default()
        },
    );

    assert_eq!(
        transcript.plain_items(),
        vec![
            "● Explored\n  └ Search shimmer_spans in crates/terminal-ui\n    Read shimmer.rs, status.rs"
                .to_string()
        ]
    );
}

#[test]
fn exploration_group_coalesces_adjacent_lists() {
    let mut transcript = Transcript::new(default_palette());

    for path in ["crates", "docs", ".docs", ".agents", ".hunea"] {
        transcript.append_runtime_tool_activity(RuntimeToolActivity {
            activity_id: format!("call-list-{path}"),
            title: format!("List Directory {path}"),
            kind: RuntimeToolKind::Search,
            status: RuntimeToolActivityStatus::Completed,
            content: vec![RuntimeToolActivityContent::Text("entry".to_string())],
            locations: Vec::new(),
            raw_input: Some(serde_json::json!({ "path": path }).into()),
            raw_output: Some("entry".into()),
        });
    }
    transcript.append_runtime_tool_activity(RuntimeToolActivity {
        activity_id: "call-read-cargo".to_string(),
        title: "Read Cargo.toml".to_string(),
        kind: RuntimeToolKind::Read,
        status: RuntimeToolActivityStatus::Completed,
        content: vec![RuntimeToolActivityContent::Text("[package]".to_string())],
        locations: Vec::new(),
        raw_input: Some(serde_json::json!({ "path": "Cargo.toml" }).into()),
        raw_output: Some("[package]".into()),
    });
    for path in ["crates/runtime-domain", "crates/runtime-domain/src"] {
        transcript.append_runtime_tool_activity(RuntimeToolActivity {
            activity_id: format!("call-list-{path}"),
            title: format!("List Directory {path}"),
            kind: RuntimeToolKind::Search,
            status: RuntimeToolActivityStatus::Completed,
            content: vec![RuntimeToolActivityContent::Text("entry".to_string())],
            locations: Vec::new(),
            raw_input: Some(serde_json::json!({ "path": path }).into()),
            raw_output: Some("entry".into()),
        });
    }

    assert_eq!(
        transcript.plain_items(),
        vec![
            "● Explored\n  └ List crates, docs, .docs, .agents, .hunea\n    Read Cargo.toml\n    List crates/runtime-domain, crates/runtime-domain/src"
                .to_string()
        ]
    );
    assert_eq!(transcript.item_metrics_index().line_count, 4);
}

#[test]
fn appending_message_closes_completed_exploration_group() {
    let palette = default_palette();
    let mut transcript = Transcript::new(palette);

    transcript.append_runtime_tool_activity(RuntimeToolActivity {
        activity_id: "call-list-src".to_string(),
        title: "List Directory src".to_string(),
        kind: RuntimeToolKind::Search,
        status: RuntimeToolActivityStatus::Completed,
        content: vec![RuntimeToolActivityContent::Text(
            "main.rs\nlib.rs".to_string(),
        )],
        locations: Vec::new(),
        raw_input: Some(serde_json::json!({ "path": "src" }).into()),
        raw_output: Some("main.rs\nlib.rs".into()),
    });

    assert_eq!(tool_result_marker_color(&transcript, 0), Some(palette.main));

    transcript.append_message(Sender::Assistant, "接着分析。");

    assert_eq!(
        tool_result_marker_color(&transcript, 0),
        Some(palette.quote)
    );
}

#[test]
fn appending_non_exploration_tool_activity_closes_previous_exploration_group() {
    let palette = default_palette();
    let mut transcript = Transcript::new(palette);

    transcript.append_runtime_tool_activity(RuntimeToolActivity {
        activity_id: "call-read-cargo".to_string(),
        title: "Read Cargo.toml".to_string(),
        kind: RuntimeToolKind::Read,
        status: RuntimeToolActivityStatus::Completed,
        content: vec![RuntimeToolActivityContent::Text("[package]".to_string())],
        locations: Vec::new(),
        raw_input: Some(serde_json::json!({ "path": "Cargo.toml" }).into()),
        raw_output: Some("[package]".into()),
    });

    assert_eq!(tool_result_marker_color(&transcript, 0), Some(palette.main));

    transcript.append_runtime_tool_activity(RuntimeToolActivity {
        activity_id: "call-shell".to_string(),
        title: "Shell: cargo check".to_string(),
        kind: RuntimeToolKind::Other,
        status: RuntimeToolActivityStatus::InProgress,
        content: Vec::new(),
        locations: Vec::new(),
        raw_input: None,
        raw_output: None,
    });

    assert_eq!(
        tool_result_marker_color(&transcript, 0),
        Some(palette.quote)
    );
}

fn tool_result_marker_color(transcript: &Transcript, item_index: usize) -> Option<Color> {
    let palette = default_palette();
    let items = transcript.items_snapshot();
    let item = items.get(item_index)?;
    let TranscriptItem::ToolResult(tool_result) = item.as_ref() else {
        return None;
    };
    tool_result
        .render_lines(80, palette)
        .first()
        .and_then(|line| line.spans.first())
        .and_then(|span| span.style.fg)
}

#[test]
fn snippet_reasoning_is_display_only_and_not_clickable() {
    let mut transcript = Transcript::new(default_palette());
    transcript.append_assistant_message_with_reasoning(
        "结论",
        "这段内容不能保留",
        ReasoningDisplayMode::Snippet,
        Some(Duration::from_secs(16)),
        StyleMode::Cx,
    );

    assert_eq!(
        transcript.plain_items(),
        vec!["• thoughts 16s".to_string(), "结论".to_string()]
    );
    assert_eq!(
        transcript.source_messages(),
        vec![(Sender::Assistant, "结论".to_string())]
    );
    assert!(!transcript.is_reasoning_header_hit(0, 0, 0));
    assert!(!transcript.toggle_reasoning_item(0));
    assert_eq!(
        transcript.plain_items(),
        vec!["• thoughts 16s".to_string(), "结论".to_string()]
    );
}

#[test]
fn snippet_reasoning_without_duration_is_not_appended() {
    let mut transcript = Transcript::new(default_palette());
    transcript.append_assistant_message_with_reasoning(
        "结论",
        "这段内容不能保留",
        ReasoningDisplayMode::Snippet,
        None,
        StyleMode::Cx,
    );

    assert_eq!(transcript.plain_items(), vec!["结论".to_string()]);
    assert_eq!(
        transcript.source_messages(),
        vec![(Sender::Assistant, "结论".to_string())]
    );
}

#[test]
fn truncate_before_item_removes_selected_and_later_history() {
    let mut transcript = Transcript::new(default_palette());
    transcript.append_message(Sender::User, "first question");
    transcript.append_message(Sender::Assistant, "first answer");
    transcript.append_message(Sender::User, "second question");
    transcript.append_message(Sender::Assistant, "second answer");
    let _ = transcript.render();

    assert!(transcript.truncate_before_item(2));

    assert_eq!(transcript.len(), 2);
    assert_eq!(
        transcript.source_messages(),
        vec![
            (Sender::User, "first question".to_string()),
            (Sender::Assistant, "first answer".to_string()),
        ]
    );
    assert_eq!(transcript.item_metrics_index().metrics.len(), 2);
    assert!(!transcript.truncate_before_item(2));
}

#[test]
fn remove_items_deletes_selected_history_and_keeps_order() {
    let mut transcript = Transcript::new(default_palette());
    transcript.append_message(Sender::User, "first question");
    transcript.append_message(Sender::Assistant, "first answer");
    transcript.append_message(Sender::User, "second question");
    transcript.append_message(Sender::Assistant, "second answer");
    let _ = transcript.render();

    assert!(transcript.remove_items(&[1, 3]));

    assert_eq!(transcript.len(), 2);
    assert_eq!(
        transcript.source_messages(),
        vec![
            (Sender::User, "first question".to_string()),
            (Sender::User, "second question".to_string()),
        ]
    );
    assert_eq!(transcript.item_metrics_index().metrics.len(), 2);
    assert!(!transcript.remove_items(&[4]));
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

    let expected_visible_lines = transcript.items[0]
        .render_lines(16, default_palette())
        .iter()
        .map(line_to_plain_text)
        .collect::<Vec<_>>();
    let mut expected_plain_lines = expected_visible_lines.clone();
    expected_plain_lines[0] = " ".repeat(16);
    let last_index = expected_plain_lines.len() - 1;
    expected_plain_lines[last_index] = " ".repeat(16);

    let render = transcript.render();
    let actual_visible_lines = render
        .lines_for_range(0, render.line_count)
        .iter()
        .map(line_to_plain_text)
        .collect::<Vec<_>>();
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

    assert_eq!(actual_visible_lines, expected_visible_lines);
    assert_eq!(actual_plain_lines, expected_plain_lines);
    assert_eq!(render.line_index_for_anchor(anchor), Some(1));
}

#[test]
fn projected_assistant_markdown_avoids_eager_styled_line_materialization() {
    let mut markdown = String::from("# Long Assistant Markdown\n\n");
    for index in 0..32 {
        markdown.push_str(&format!(
            "## Section {index}\n\n- viewport rendering stays local\n- markdown lists remain active\n\n```rust\nfn section_{index}() -> &'static str {{\n    \"paged assistant markdown projection\"\n}}\n```\n\nFollow-up prose wraps through the document viewport without forcing full-message materialization.\n\n",
        ));
    }

    let mut transcript = Transcript::new(default_palette());
    transcript.set_width(80);
    transcript.items = Rc::new(vec![Rc::new(TranscriptItem::Message(MessageItem::new(
        Sender::Assistant,
        markdown.clone(),
    )))]);

    let expected_lines = transcript.items[0].render_lines(80, default_palette());
    let expected_plain_lines = expected_lines
        .iter()
        .map(line_to_plain_text)
        .collect::<Vec<_>>();
    let render = transcript.render();
    let block = render
        .items
        .first()
        .expect("assistant item should produce a render block")
        .block
        .as_ref();

    assert!(
        block.lines.is_empty(),
        "long assistant Markdown should use a lazy projection instead of caching every styled line"
    );

    let actual_plain_lines = (0..render.line_count)
        .map(|index| {
            render
                .line_at(index)
                .expect("projected assistant block should materialize every visible line")
                .plain_line
        })
        .collect::<Vec<_>>();
    let anchor = render
        .line_at(10)
        .expect("projected assistant block should expose anchors")
        .anchor;

    assert_eq!(actual_plain_lines, expected_plain_lines);
    assert_eq!(render.lines_for_range(0, render.line_count), expected_lines);
    assert_eq!(render.line_index_for_anchor(anchor), Some(10));
}

#[test]
fn projected_assistant_fenced_code_page_matches_eager_inside_wrapped_line() {
    let mut markdown = String::from("```rust\n");
    for index in 0..40 {
        markdown.push_str(&format!(
            "let projected_value_{index} = \"this line is intentionally long enough to wrap across multiple terminal rows while keeping syntax state local to the same source line\";\n"
        ));
    }
    markdown.push_str("```\n");

    let mut transcript = Transcript::new(default_palette());
    transcript.set_width(38);
    transcript.items = Rc::new(vec![Rc::new(TranscriptItem::Message(MessageItem::new(
        Sender::Assistant,
        markdown,
    )))]);

    let expected_lines = transcript.items[0].render_lines(38, default_palette());
    let render = transcript.render();
    let block = render
        .items
        .first()
        .expect("assistant item should produce a render block")
        .block
        .as_ref();

    assert!(
        block.lines.is_empty(),
        "ordinary fenced code should stay on the paged assistant projection path"
    );
    assert!(
        render.line_count > 128,
        "fixture should cross multiple projection pages: {}",
        render.line_count
    );
    assert_eq!(
        render.lines_for_range(64, 64),
        expected_lines[64..128].to_vec(),
        "a projection page starting inside a wrapped source code line must keep styled output equal to the eager renderer"
    );
}

#[test]
fn projected_assistant_ordered_lists_match_eager_renderer() {
    let mut markdown = String::from("# Ordered assistant notes\n\n");
    for index in 0..50 {
        markdown.push_str(&format!(
            "## Step group {index}\n\n1. Keep viewport work bounded\n2. Preserve Markdown list rendering\n3. Reuse the existing eager renderer for each projected snippet\n\n"
        ));
    }

    let mut transcript = Transcript::new(default_palette());
    transcript.set_width(76);
    transcript.items = Rc::new(vec![Rc::new(TranscriptItem::Message(MessageItem::new(
        Sender::Assistant,
        markdown,
    )))]);

    let expected_lines = transcript.items[0].render_lines(76, default_palette());
    let render = transcript.render();
    let block = render
        .items
        .first()
        .expect("assistant item should produce a render block")
        .block
        .as_ref();

    assert!(
        block.lines.is_empty(),
        "top-level ordered lists should stay on the assistant projection path"
    );
    assert_eq!(render.lines_for_range(0, render.line_count), expected_lines);
}

#[test]
fn projected_assistant_list_followed_by_heading_matches_eager_spacing() {
    let mut markdown = String::from("# Long Assistant Markdown\n\n");
    for index in 0..50 {
        markdown.push_str(&format!(
            "- 当前共识 {index}：仍待验证。\n\n### 💡 为什么重要 {index}？\n1. 算力策略：提示部署路线。\n\n",
        ));
    }

    let mut transcript = Transcript::new(default_palette());
    transcript.set_width(80);
    transcript.items = Rc::new(vec![Rc::new(TranscriptItem::Message(MessageItem::new(
        Sender::Assistant,
        markdown,
    )))]);

    let expected_lines = transcript.items[0].render_lines(80, default_palette());
    let expected_plain_lines = expected_lines
        .iter()
        .map(line_to_plain_text)
        .collect::<Vec<_>>();
    let render = transcript.render();
    let block = render
        .items
        .first()
        .expect("assistant item should produce a render block")
        .block
        .as_ref();

    assert!(
        block.lines.is_empty(),
        "long assistant Markdown should stay on the projection path"
    );
    assert!(
        expected_plain_lines
            .windows(3)
            .any(|lines| lines == ["- 当前共识 0：仍待验证。", "", "### 💡 为什么重要 0？"]),
        "eager renderer should preserve the list-to-heading blank line: {expected_plain_lines:?}"
    );
    assert_eq!(
        (0..render.line_count)
            .map(|index| {
                render
                    .line_at(index)
                    .expect("projected assistant block should materialize every visible line")
                    .plain_line
            })
            .collect::<Vec<_>>(),
        expected_plain_lines
    );
}

#[test]
fn projected_assistant_fenced_code_does_not_close_on_info_text_line() {
    let mut markdown = String::from("```rust\n");
    for index in 0..80 {
        markdown.push_str(&format!(
            "let value_{index} = \"line before embedded fence marker\";\n"
        ));
        markdown.push_str("```not a closing fence because it has trailing info text\n");
        markdown.push_str(&format!(
            "let after_{index} = \"line after embedded fence marker\";\n"
        ));
    }
    markdown.push_str("```\n");

    let mut transcript = Transcript::new(default_palette());
    transcript.set_width(70);
    transcript.items = Rc::new(vec![Rc::new(TranscriptItem::Message(MessageItem::new(
        Sender::Assistant,
        markdown,
    )))]);

    let expected_lines = transcript.items[0].render_lines(70, default_palette());
    let render = transcript.render();
    let block = render
        .items
        .first()
        .expect("assistant item should produce a render block")
        .block
        .as_ref();

    assert!(
        block.lines.is_empty(),
        "a code line with non-whitespace after the fence marker is not a closing fence"
    );
    assert_eq!(render.lines_for_range(0, render.line_count), expected_lines);
}

#[test]
fn projected_assistant_fenced_code_accepts_longer_closing_fence() {
    let mut markdown = String::new();
    for index in 0..80 {
        markdown.push_str("```rust\n");
        markdown.push_str(&format!(
            "let value_{index} = \"CommonMark allows a longer closing fence\";\n"
        ));
        markdown.push_str("````\n\n");
    }

    let mut transcript = Transcript::new(default_palette());
    transcript.set_width(72);
    transcript.items = Rc::new(vec![Rc::new(TranscriptItem::Message(MessageItem::new(
        Sender::Assistant,
        markdown,
    )))]);

    let expected_lines = transcript.items[0].render_lines(72, default_palette());
    let render = transcript.render();
    let block = render
        .items
        .first()
        .expect("assistant item should produce a render block")
        .block
        .as_ref();

    assert!(
        block.lines.is_empty(),
        "a longer closing fence should not force eager rendering"
    );
    assert_eq!(render.lines_for_range(0, render.line_count), expected_lines);
}

#[test]
fn assistant_projection_falls_back_for_empty_fenced_code_blocks() {
    let mut markdown = String::new();
    for index in 0..120 {
        markdown.push_str(&format!(
            "## Empty code section {index}\n\n```rust\n```\n\n"
        ));
    }

    let mut transcript = Transcript::new(default_palette());
    transcript.set_width(80);
    transcript.items = Rc::new(vec![Rc::new(TranscriptItem::Message(MessageItem::new(
        Sender::Assistant,
        markdown,
    )))]);

    let expected_lines = transcript.items[0].render_lines(80, default_palette());
    let render = transcript.render();
    let block = render
        .items
        .first()
        .expect("assistant item should produce a render block")
        .block
        .as_ref();

    assert!(
        !block.lines.is_empty(),
        "empty fenced code blocks should preserve existing eager Markdown rendering"
    );
    assert_eq!(render.lines_for_range(0, render.line_count), expected_lines);
}

#[test]
fn assistant_projection_falls_back_for_indented_markdown_blocks() {
    let mut markdown = String::new();
    for index in 0..140 {
        markdown.push_str(&format!(
            "## Section {index}\n\n    # this is an indented code line, not a heading\n\n"
        ));
    }

    let mut transcript = Transcript::new(default_palette());
    transcript.set_width(80);
    transcript.items = Rc::new(vec![Rc::new(TranscriptItem::Message(MessageItem::new(
        Sender::Assistant,
        markdown,
    )))]);

    let expected_lines = transcript.items[0].render_lines(80, default_palette());
    let render = transcript.render();
    let block = render
        .items
        .first()
        .expect("assistant item should produce a render block")
        .block
        .as_ref();

    assert!(
        !block.lines.is_empty(),
        "indented block Markdown should fall back to the existing eager renderer"
    );
    assert_eq!(render.lines_for_range(0, render.line_count), expected_lines);
}

#[test]
fn assistant_projection_falls_back_for_complex_markdown_blocks() {
    let mut markdown = String::new();
    for index in 0..120 {
        markdown.push_str(&format!(
            "## Complex section {index}\n\n- top level\n  - nested child that should use the existing Markdown renderer\n\n| Name | Status |\n| --- | --- |\n| alpha {index} | beta {index} |\n\n"
        ));
    }

    let mut transcript = Transcript::new(default_palette());
    transcript.set_width(80);
    transcript.items = Rc::new(vec![Rc::new(TranscriptItem::Message(MessageItem::new(
        Sender::Assistant,
        markdown,
    )))]);

    let expected_lines = transcript.items[0].render_lines(80, default_palette());
    let render = transcript.render();
    let block = render
        .items
        .first()
        .expect("assistant item should produce a render block")
        .block
        .as_ref();

    assert!(
        !block.lines.is_empty(),
        "nested lists and tables should fall back to the existing eager Markdown renderer"
    );
    assert_eq!(render.lines_for_range(0, render.line_count), expected_lines);
}

#[test]
fn assistant_projection_falls_back_for_stateful_fenced_code_highlighting() {
    let mut markdown = String::from("```rust\nfn main() {\n    /*\n");
    for index in 0..180 {
        markdown.push_str(&format!(
            "     * multiline comment row {index} should keep syntax highlighting state from the opening delimiter\n"
        ));
    }
    markdown.push_str("     */\n}\n```\n");

    let mut transcript = Transcript::new(default_palette());
    transcript.set_width(64);
    transcript.items = Rc::new(vec![Rc::new(TranscriptItem::Message(MessageItem::new(
        Sender::Assistant,
        markdown,
    )))]);

    let expected_lines = transcript.items[0].render_lines(64, default_palette());
    let render = transcript.render();
    let block = render
        .items
        .first()
        .expect("assistant item should produce a render block")
        .block
        .as_ref();

    assert!(
        !block.lines.is_empty(),
        "fenced code that depends on cross-line syntax state should preserve eager Markdown rendering"
    );
    assert_eq!(render.lines_for_range(0, render.line_count), expected_lines);
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
    assert!(exact_before_resize.metrics[0].is_exact());
    assert_eq!(
        estimated_index.metrics[0].content_char_len, exact_index.metrics[0].content_char_len,
        "tabbed Markdown estimates use the current-width Markdown measurement without bumping the tracked exactization counter"
    );
    assert_eq!(
        estimated_index.content_char_len,
        exact_index.content_char_len
    );
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
fn exactize_line_window_keeps_incremental_index_self_consistent() {
    let mut transcript = Transcript::new(default_palette());
    transcript.set_gap(1);
    transcript.set_width(20);
    transcript.append_message(Sender::Assistant, "prefix");
    transcript.append_message(
        Sender::Assistant,
        concat!(
            "## Section\n\n",
            "- list item\n\n",
            "Follow-up **bold markdown content** wraps differently once Markdown metrics are exact."
        ),
    );
    transcript.append_message(Sender::Assistant, "tail");

    let estimated_index = transcript.progressive_item_metrics_index();
    let item_position = estimated_index
        .position_for_item(1)
        .expect("middle item should be visible");

    transcript.exactize_line_window(item_position.start_line, item_position.total_line_count, 0);
    let updated_index = transcript.progressive_item_metrics_index();

    assert!(
        updated_index.metrics[1].is_exact(),
        "visible exactization should update the requested item"
    );
    assert_ne!(
        estimated_index.metrics[1], updated_index.metrics[1],
        "test fixture should exercise an actual metrics update"
    );
    assert_metrics_index_self_consistent(&updated_index, transcript.gap);
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

fn assert_metrics_index_self_consistent(index: &TranscriptItemMetricsIndex, gap: usize) {
    let mut expected_prefix_sums = Vec::with_capacity(index.metrics.len() + 1);
    expected_prefix_sums.push(0);
    for metrics in index.metrics.iter() {
        expected_prefix_sums.push(
            expected_prefix_sums
                .last()
                .copied()
                .unwrap_or(0usize)
                .saturating_add(metrics.content_char_len),
        );
    }

    let mut expected_visible_positions = vec![usize::MAX; index.metrics.len()];
    let mut expected_visible_items = Vec::new();
    let mut total_lines = 0usize;
    let mut previous_visible_item_index = None;
    for (item_index, metrics) in index.metrics.iter().enumerate() {
        if metrics.content_line_count == 0 {
            continue;
        }

        let gap_before = usize::from(previous_visible_item_index.is_some()) * gap;
        let position = TranscriptItemPosition {
            item_index,
            start_line: total_lines,
            gap_before,
            content_line_count: metrics.content_line_count,
            total_line_count: gap_before + metrics.content_line_count,
            content_char_len: metrics.content_char_len,
            gap_owner_item_index: previous_visible_item_index,
        };
        expected_visible_positions[item_index] = expected_visible_items.len();
        total_lines = total_lines.saturating_add(position.total_line_count);
        previous_visible_item_index = Some(item_index);
        expected_visible_items.push(position);
    }

    assert_eq!(*index.content_prefix_sums, expected_prefix_sums);
    assert_eq!(*index.visible_positions, expected_visible_positions);
    assert_eq!(*index.visible_items, expected_visible_items);
    assert_eq!(index.line_count, total_lines);
    assert_eq!(
        index.content_char_len,
        expected_prefix_sums.last().copied().unwrap_or(0)
    );
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
