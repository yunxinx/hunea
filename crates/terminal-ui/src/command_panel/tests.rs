use super::*;
use crate::{
    AppEvent, ModelOptions, StartupBannerOptions, StatusLineItem, StyleMode,
    document::DocumentAnchorRegion,
};

#[test]
fn single_unmatched_character_keeps_command_panel_active() {
    assert_eq!(command_panel_query("/h"), Some("h".to_string()));
    assert_eq!(command_panel_query("/he"), None);
    assert_eq!(command_panel_query("/ "), None);
}

#[test]
fn non_prefix_subsequence_query_matches_command_name() {
    assert_eq!(command_panel_query("/del"), Some("del".to_string()));

    let names = base_command_panel_items_for_query("del")
        .into_iter()
        .map(|item| item.name)
        .collect::<Vec<_>>();
    assert_eq!(names, vec!["/models"]);
}

#[test]
fn current_status_notice_still_renders_below_command_panel() {
    let mut model = Model::new_with_options(
        StartupBannerOptions::default(),
        ModelOptions {
            style_mode: StyleMode::Cx,
            status_line_items: vec![StatusLineItem::GitBranch],
            ..ModelOptions::default()
        },
    );
    model.set_window(24, 6);
    model.show_transient_status_notice("Selection copied");
    model.composer.reset_text_and_move_to_end("/");
    model.sync_command_panel_navigation();
    model.sync_composer_height();

    let status_line = model.current_status_line_render_result();

    assert!(status_line.has_content);
    let panel = model.current_inline_command_panel_render_result();
    assert!(panel.has_content);
}

#[test]
fn command_panel_rows_are_inserted_into_document_before_status_notice() {
    let mut model = Model::new_with_options(
        StartupBannerOptions::default(),
        ModelOptions {
            style_mode: StyleMode::Cx,
            status_line_items: vec![StatusLineItem::GitBranch],
            ..ModelOptions::default()
        },
    );
    model.set_window(32, 8);
    model.composer.reset_text_and_move_to_end("/");
    model.sync_command_panel_navigation();
    model.show_transient_status_notice("Selection copied");

    let layout = model.build_document_layout();
    let plain_lines = layout.all_plain_lines();
    let panel_line = plain_lines
        .iter()
        .position(|line| line.contains("/exit"))
        .expect("command panel row should exist");
    let notice_line = plain_lines
        .iter()
        .position(|line| line.contains("Selection copied"))
        .expect("status notice should exist");
    let command_panel_rows = layout
        .all_line_anchors()
        .into_iter()
        .filter(|anchor| anchor.region == DocumentAnchorRegion::CommandPanel)
        .count();

    assert!(panel_line < notice_line);
    assert_eq!(command_panel_rows, model.command_panel_list_visible_rows());
}

#[test]
fn command_panel_completion_is_undoable_to_the_query() {
    let mut model = Model::new(StartupBannerOptions::default());

    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char('/'))));
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Tab)));
    assert_eq!(model.composer_text(), "/exit");

    model.update(AppEvent::Key(KeyEvent::new(
        KeyCode::Char('z'),
        KeyModifiers::CONTROL,
    )));

    assert_eq!(model.composer_text(), "/");
}

#[test]
fn rendered_command_panel_line_respects_available_width() {
    let model = Model::new(StartupBannerOptions::default());

    let item = base_command_panel_items_for_query("exit")
        .into_iter()
        .next()
        .expect("exit item should exist");
    let (_, plain_line, selectable) =
        model.render_command_panel_line(&item, true, 8, display_width(&item.name));

    assert_eq!(display_width(&plain_line), 8);
    assert_eq!(
        selectable.content_columns().map(|(start, _)| start),
        Some(2)
    );
}
