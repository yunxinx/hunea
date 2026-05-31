use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{buffer::Buffer, layout::Rect};

use crate::{AppEvent, Model, Sender, StartupBannerOptions, theme::default_palette};
use runtime_domain::session::{
    RuntimeToolActivity, RuntimeToolActivityContent, RuntimeToolActivityStatus, RuntimeToolKind,
};

#[test]
fn overlay_scroll_boundary() {
    let mut model = Model::new(StartupBannerOptions::default());
    model.set_window(20, 10);
    model.set_palette(default_palette(), true);
    model.transcript_mut().clear();
    // 追加一条长消息，确保总行数大于内容高度
    model.transcript_mut().append_message(
        Sender::Assistant,
        "line1\nline2\nline3\nline4\nline5\nline6\nline7\nline8\nline9\nline10\nline11\nline12",
    );
    model.sync_transcript_render();

    model.open_transcript_overlay();
    assert!(model.transcript_overlay_active());

    // 高度 10，底部 2（1 rule + 1 hint），内容区 = 8
    let overlay = model.transcript_overlay.as_ref().unwrap();
    assert_eq!(overlay.scroll_offset, 0);

    // Home 应该保持在 0
    model.handle_transcript_overlay_key(KeyEvent::from(KeyCode::Home));
    assert_eq!(model.transcript_overlay.as_ref().unwrap().scroll_offset, 0);

    // End 应该滚动到底部
    model.handle_transcript_overlay_key(KeyEvent::from(KeyCode::End));
    let total_lines = model.transcript.item_metrics_index().line_count;
    let max_offset = total_lines.saturating_sub(8);
    assert_eq!(
        model.transcript_overlay.as_ref().unwrap().scroll_offset,
        max_offset
    );

    // q 关闭
    model.handle_transcript_overlay_key(KeyEvent::from(KeyCode::Char('q')));
    assert!(!model.transcript_overlay_active());
}

#[test]
fn overlay_vim_jk_navigation() {
    let mut model = Model::new(StartupBannerOptions::default());
    model.set_window(20, 10);
    model.set_palette(default_palette(), true);
    model.transcript_mut().clear();
    model.transcript_mut().append_message(
        Sender::Assistant,
        "line1\nline2\nline3\nline4\nline5\nline6\nline7\nline8\nline9\nline10\nline11\nline12",
    );
    model.sync_transcript_render();

    model.open_transcript_overlay();
    assert_eq!(model.transcript_overlay.as_ref().unwrap().scroll_offset, 0);

    // j 向下滚动一行
    model.handle_transcript_overlay_key(KeyEvent::from(KeyCode::Char('j')));
    assert_eq!(model.transcript_overlay.as_ref().unwrap().scroll_offset, 1);

    // k 向上滚动一行
    model.handle_transcript_overlay_key(KeyEvent::from(KeyCode::Char('k')));
    assert_eq!(model.transcript_overlay.as_ref().unwrap().scroll_offset, 0);
}

#[test]
fn overlay_vim_ctrl_ud_half_page() {
    let mut model = Model::new(StartupBannerOptions::default());
    model.set_window(20, 10);
    model.set_palette(default_palette(), true);
    model.transcript_mut().clear();
    // 12 行内容，高度 10，内容区 = 8，max_offset = 4
    model.transcript_mut().append_message(
        Sender::Assistant,
        "line1\nline2\nline3\nline4\nline5\nline6\nline7\nline8\nline9\nline10\nline11\nline12",
    );
    model.sync_transcript_render();

    model.open_transcript_overlay();
    // 半页 = 8 / 2 = 4
    // Ctrl+D 向下滚动 4 行
    model.handle_transcript_overlay_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL));
    assert_eq!(model.transcript_overlay.as_ref().unwrap().scroll_offset, 4);

    // Ctrl+U 向上滚动 4 行
    model.handle_transcript_overlay_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
    assert_eq!(model.transcript_overlay.as_ref().unwrap().scroll_offset, 0);
}

#[test]
fn toggle_with_ctrl_t() {
    let mut model = Model::new(StartupBannerOptions::default());
    model.set_window(20, 10);
    model.set_palette(default_palette(), true);

    assert!(!model.transcript_overlay_active());

    model.handle_transcript_overlay_key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::CONTROL));
    assert!(model.transcript_overlay_active());

    model.handle_transcript_overlay_key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::CONTROL));
    assert!(!model.transcript_overlay_active());
}

#[test]
fn transcript_overlay_switches_tool_activity_detail_mode() {
    let mut model = Model::new(StartupBannerOptions::default());
    model.set_window(40, 10);
    model.set_palette(default_palette(), true);
    model.transcript_mut().clear();
    model
        .transcript_mut()
        .append_runtime_tool_activity(RuntimeToolActivity {
            activity_id: "call-1".to_string(),
            title: "Run tests".to_string(),
            kind: RuntimeToolKind::Execute,
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

    let compact = model.transcript_plain_items().join("\n");
    assert_contains_transcript_hint(&compact);
    assert!(!compact.contains("line 7"));

    model.open_transcript_overlay();
    let detailed = model.transcript_plain_items().join("\n");
    assert!(detailed.contains("line 7"));
    assert!(!detailed.contains("ctrl + t to view transcript"));

    model.close_transcript_overlay();
    let compact_again = model.transcript_plain_items().join("\n");
    assert_contains_transcript_hint(&compact_again);
    assert!(!compact_again.contains("line 7"));
}

fn assert_contains_transcript_hint(text: &str) {
    let compacted = text
        .chars()
        .filter(|ch| !ch.is_whitespace())
        .collect::<String>();
    assert!(
        compacted.contains("…+10lines(")
            && compacted.contains("ctrl+t")
            && compacted.contains("viewtranscript)"),
        "expected wrapped transcript hint in {text:?}; compacted={compacted:?}"
    );
}

#[test]
fn overlay_assistant_message_inset() {
    let mut model = Model::new(StartupBannerOptions::default());
    model.set_window(40, 10);
    model.set_palette(default_palette(), true);
    model.transcript_mut().clear();
    model
        .transcript_mut()
        .append_message(Sender::Assistant, "Hello assistant");
    model.sync_transcript_render();

    model.open_transcript_overlay();

    let buffer = render_model_buffer(&mut model, 40, 10);
    let mut found = false;
    for row in 0..10 {
        let mut row_text = String::new();
        for col in 0..40 {
            row_text.push_str(buffer[(col, row)].symbol());
        }
        if row_text.contains("Hello assistant") {
            // 检查前 2 列是否为空格（与主界面一致的视觉缩进）
            let first_two: String = (0..2)
                .map(|c| buffer[(c, row)].symbol().to_string())
                .collect();
            assert_eq!(
                first_two, "  ",
                "assistant message row should have 2-space visual inset at start"
            );
            found = true;
            break;
        }
    }
    assert!(found, "should find assistant message in overlay render");
}

#[test]
fn overlay_diff_line_background_fills_the_rendered_row() {
    let mut model = Model::new(StartupBannerOptions::default());
    model.set_window(48, 8);
    model.set_palette(default_palette(), true);
    model.transcript_mut().clear();
    model
        .transcript_mut()
        .append_runtime_tool_activity(RuntimeToolActivity {
            activity_id: "call-1".to_string(),
            title: "WriteFile: src/lib.rs".to_string(),
            kind: RuntimeToolKind::Edit,
            status: RuntimeToolActivityStatus::Completed,
            content: vec![RuntimeToolActivityContent::Diff {
                path: "src/lib.rs".to_string(),
                old_text: Some("one\nold\ntail\n".to_string()),
                new_text: "one\nnew\ntail\n".to_string(),
                is_truncated: false,
            }],
            locations: Vec::new(),
            raw_input: None,
            raw_output: None,
        });
    model.sync_transcript_render();
    model.open_transcript_overlay();

    let buffer = render_model_buffer(&mut model, 48, 8);
    let insert_row = (0..8)
        .find(|row| {
            let row_text = (0..48)
                .map(|column| buffer[(column, *row)].symbol())
                .collect::<String>();
            row_text.contains("+  new")
        })
        .expect("insert diff row should be rendered");

    assert_ne!(
        buffer[(47, insert_row)].bg,
        ratatui::style::Color::Reset,
        "overlay diff insert row background should fill trailing cells"
    );
}

#[test]
fn overlay_render_does_not_materialize_full_transcript_result() {
    let mut model = Model::new(StartupBannerOptions::default());
    model.set_window(40, 8);
    model.set_palette(default_palette(), true);
    model.transcript_mut().clear();
    for index in 0..96 {
        model
            .transcript_mut()
            .append_message(Sender::Assistant, format!("message {index}"));
    }
    model.sync_transcript_render();
    assert_eq!(
        model.transcript.cached_render_result_item_count_for_test(),
        0
    );

    model.open_transcript_overlay();

    let _ = render_model_buffer(&mut model, 40, 8);

    assert_eq!(
        model.transcript.cached_render_result_item_count_for_test(),
        0,
        "overlay render should use viewport/item materialization instead of full transcript render"
    );
}

fn render_model_buffer(model: &mut Model, width: u16, height: u16) -> Buffer {
    let area = Rect::new(0, 0, width, height);
    let mut buffer = Buffer::empty(area);
    let _ = model.render_to_buffer(area, &mut buffer);
    buffer
}

#[test]
fn overlay_bottom_follow_tracks_committed_append_when_at_bottom() {
    let mut model = Model::new(StartupBannerOptions::default());
    model.set_window(20, 10);
    model.set_palette(default_palette(), true);
    model.transcript_mut().clear();
    model.transcript_mut().append_message(
        Sender::Assistant,
        "line1\nline2\nline3\nline4\nline5\nline6\nline7\nline8\nline9\nline10\nline11\nline12",
    );
    model.sync_transcript_render();
    model.open_transcript_overlay();

    model.handle_transcript_overlay_key(KeyEvent::from(KeyCode::End));
    let old_offset = model.transcript_overlay.as_ref().unwrap().scroll_offset;
    let old_max_offset = overlay_max_offset(&mut model);
    assert_eq!(old_offset, old_max_offset);

    model.append_assistant_message_from_runtime("tail line one\ntail line two");

    let new_max_offset = overlay_max_offset(&mut model);
    assert!(
        new_max_offset > old_max_offset,
        "append should extend the overlay scroll range"
    );
    assert_eq!(
        model.transcript_overlay.as_ref().unwrap().scroll_offset,
        new_max_offset,
        "overlay should stay pinned to bottom when it was already at bottom"
    );
}

#[test]
fn overlay_bottom_follow_preserves_manual_scroll_on_committed_append() {
    let mut model = Model::new(StartupBannerOptions::default());
    model.set_window(20, 10);
    model.set_palette(default_palette(), true);
    model.transcript_mut().clear();
    model.transcript_mut().append_message(
        Sender::Assistant,
        "line1\nline2\nline3\nline4\nline5\nline6\nline7\nline8\nline9\nline10\nline11\nline12",
    );
    model.sync_transcript_render();
    model.open_transcript_overlay();
    model.handle_transcript_overlay_key(KeyEvent::from(KeyCode::End));
    model.handle_transcript_overlay_key(KeyEvent::from(KeyCode::PageUp));

    let manual_offset = model.transcript_overlay.as_ref().unwrap().scroll_offset;
    assert!(
        manual_offset < overlay_max_offset(&mut model),
        "test should leave overlay above the bottom"
    );

    model.append_assistant_message_from_runtime("tail line one\ntail line two");

    assert_eq!(
        model.transcript_overlay.as_ref().unwrap().scroll_offset,
        manual_offset,
        "overlay should not move when the user has scrolled away from bottom"
    );
}

#[test]
fn overlay_disables_mouse_capture_until_closed() {
    let mut model = Model::new(StartupBannerOptions::default());
    model.set_window(20, 10);
    model.set_palette(default_palette(), true);

    assert!(model.wants_mouse_capture());

    model.open_transcript_overlay();
    assert!(!model.wants_mouse_capture());

    model.close_transcript_overlay();
    assert!(model.wants_mouse_capture());
}

#[test]
fn overlay_initial_scroll_subtracts_startup_banner_lines_from_document_offset() {
    let mut model = Model::new(StartupBannerOptions::default());
    model.set_window(20, 8);
    model.set_palette(default_palette(), true);
    model.transcript_mut().append_message(
        Sender::Assistant,
        "line1\nline2\nline3\nline4\nline5\nline6\nline7\nline8\nline9\nline10\nline11\nline12",
    );
    model.sync_transcript_render();
    let index = model.transcript.progressive_item_metrics_index();
    let startup_banner_lines = model.transcript_overlay_startup_banner_lines_for_index(&index);
    assert!(
        startup_banner_lines > 0,
        "default transcript should include a startup banner item"
    );
    model.document_runtime.viewport_y = startup_banner_lines + 2;

    model.open_transcript_overlay();

    assert_eq!(
        model.transcript_overlay.as_ref().unwrap().scroll_offset,
        2,
        "opening overlay from the main transcript viewport should skip startup banner lines"
    );
}

#[test]
fn overlay_resize_keeps_bottom_pinned_after_height_decreases() {
    let mut model = Model::new(StartupBannerOptions::default());
    model.set_window(20, 12);
    model.set_palette(default_palette(), true);
    model.transcript_mut().clear();
    model.transcript_mut().append_message(
        Sender::Assistant,
        "line1\nline2\nline3\nline4\nline5\nline6\nline7\nline8\nline9\nline10\nline11\nline12\nline13\nline14\nline15",
    );
    model.sync_transcript_render();
    model.open_transcript_overlay();
    model.handle_transcript_overlay_key(KeyEvent::from(KeyCode::End));
    let old_max_offset = overlay_max_offset(&mut model);
    assert_eq!(
        model.transcript_overlay.as_ref().unwrap().scroll_offset,
        old_max_offset
    );

    model.update(AppEvent::Resized {
        width: 20,
        height: 6,
    });

    let new_max_offset = overlay_max_offset(&mut model);
    assert!(
        new_max_offset > old_max_offset,
        "smaller height should increase the overlay scroll range"
    );
    assert_eq!(
        model.transcript_overlay.as_ref().unwrap().scroll_offset,
        new_max_offset,
        "overlay should stay pinned to bottom across height-only resize"
    );
}

#[test]
fn overlay_ignores_mouse_wheel_events_to_preserve_terminal_mouse_policy() {
    let mut model = Model::new(StartupBannerOptions::default());
    model.set_window(20, 10);
    model.set_palette(default_palette(), true);
    model.transcript_mut().clear();
    model.transcript_mut().append_message(
        Sender::Assistant,
        "line1\nline2\nline3\nline4\nline5\nline6\nline7\nline8\nline9\nline10\nline11\nline12",
    );
    model.sync_transcript_render();
    model.open_transcript_overlay();
    assert_eq!(model.transcript_overlay.as_ref().unwrap().scroll_offset, 0);

    model.update(AppEvent::MouseWheel { delta_lines: 3 });

    assert_eq!(
        model.transcript_overlay.as_ref().unwrap().scroll_offset,
        0,
        "overlay should stay keyboard-pager driven if a mouse wheel event is delivered"
    );
}

fn overlay_max_offset(model: &mut Model) -> usize {
    let content_height = model.height.saturating_sub(2).max(1) as usize;
    let index = model.transcript.progressive_item_metrics_index();
    let startup_banner_lines = model.transcript_overlay_startup_banner_lines_for_index(&index);
    index
        .line_count
        .saturating_sub(startup_banner_lines)
        .saturating_sub(content_height)
}
