mod anchor_match;
mod cache;
mod layout;
mod slot_frame;
mod slot_viewport;
mod sync;

pub(crate) use self::cache::{
    DocumentAnchorRegion, DocumentLayout, DocumentLayoutCache, DocumentLayoutKey,
    DocumentLineAnchor, DocumentViewport, DocumentViewportAnchor, DocumentViewportCache,
    DocumentViewportKey, ManualDocumentScrollRestoreTarget,
};
pub(crate) use self::cache::{
    DocumentLayoutCache as LayoutCache, DocumentViewportAnchor as ViewportAnchor,
    DocumentViewportCache as ViewportCache, ManualDocumentScrollRestoreTarget as RestoreTarget,
};
pub(crate) use self::slot_viewport::offset_viewport_line_indices;

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyEvent};
    use ratatui::text::Line;
    use std::hint::black_box;

    use super::layout::{
        compose_document_layout, compose_document_viewport, visible_document_lines,
    };
    use super::*;
    use crate::frontend::tui::{HeroOptions, Model, Sender, StyleMode, theme::default_palette};

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
            layout.plain_lines,
            vec!["history".to_string(), String::new(), "┃ x".to_string(),]
        );
        assert_eq!(layout.composer_start_line, 2);
        assert_eq!(layout.composer_line_count, 1);
        assert_eq!(layout.cursor_x, 3);
        assert_eq!(layout.cursor_y, 2);
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
            layout.plain_lines,
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
    fn visible_document_lines_tracks_cursor_visibility() {
        let layout = DocumentLayout {
            lines: vec![Line::raw("a"), Line::raw("b"), Line::raw("c")],
            plain_lines: vec!["a".to_string(), "b".to_string(), "c".to_string()],
            cursor_x: 4,
            cursor_y: 1,
            ..DocumentLayout::default()
        };

        let (visible_lines, _, visible_offset) =
            visible_document_lines(&layout.lines, &layout.plain_lines, 0, 2);
        assert_eq!(visible_lines.len(), 2);
        assert_eq!(visible_offset, 0);
        assert!(cursor_visible_in_document_viewport(
            &layout,
            visible_offset,
            visible_lines.len()
        ));

        let (hidden_lines, _, hidden_offset) =
            visible_document_lines(&layout.lines, &layout.plain_lines, 2, 1);
        assert_eq!(hidden_lines.len(), 1);
        assert_eq!(hidden_offset, 2);
        assert!(!cursor_visible_in_document_viewport(
            &layout,
            hidden_offset,
            hidden_lines.len()
        ));
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
        model.scroll_restore_target = RestoreTarget::ComposerCursor;
        model.scroll_restore_anchor = ViewportAnchor::default();

        model.scroll_document_by(Model::document_mouse_wheel_delta());

        assert!(!model.follow_bottom);
        assert!(!model.manual_document_scroll);
        assert_eq!(model.document_viewport_y, 2);
        assert_eq!(model.composer.viewport_offset(), 2);
        assert_eq!(model.scroll_restore_target, RestoreTarget::None);
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
        model.scroll_restore_target = RestoreTarget::BottomFollow;

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
        assert_eq!(model.scroll_restore_target, RestoreTarget::BottomFollow);
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
                format!(
                    "message {index:02}: keep scrollback anchored while the composer draft keeps growing"
                ),
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
            model.document_layout_cache.valid = false;
            model.document_viewport_cache.valid = false;
        }
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
}
