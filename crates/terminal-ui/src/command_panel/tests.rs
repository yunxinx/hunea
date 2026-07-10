use super::*;
use crate::{
    AppEvent, ModelOptions, Sender, StartupBannerOptions, StatusLineItem, StyleMode,
    document::DocumentAnchorRegion, theme::default_palette,
};
use ratatui::style::Modifier;

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
fn command_panel_ranks_contiguous_subsequence_above_spread_subsequence() {
    // /se 在 /resend 中是连续子串（score 0），在 /resume 中是离散子序列（score 2），
    // 连续匹配应排在前面。
    let names = base_command_panel_items_for_query("se")
        .into_iter()
        .map(|item| item.name)
        .collect::<Vec<_>>();
    assert_eq!(names, vec!["/resend", "/resume"]);
}

#[test]
fn command_panel_ranks_exact_match_above_prefix_match() {
    // "exit" 精确匹配 /exit（score -1100），应排在所有其他命令前面。
    let names = base_command_panel_items_for_query("exit")
        .into_iter()
        .map(|item| item.name)
        .collect::<Vec<_>>();
    assert_eq!(names.first(), Some(&"/exit".to_string()));
}

#[test]
fn command_panel_match_score_uses_alias_when_primary_does_not_match() {
    // /qu 不匹配 /exit 的主名 "exit"，但匹配别名 "quit" 的前缀。
    let names = base_command_panel_items_for_query("qu")
        .into_iter()
        .map(|item| item.name)
        .collect::<Vec<_>>();
    assert_eq!(names, vec!["/exit"]);
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
    model.show_transient_status_notice("Press Esc again to interrupt");
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
    model.show_transient_status_notice("Press Esc again to interrupt");

    let layout = model.build_document_layout(crate::frame_time::FrameRenderContext::capture());
    let plain_lines = layout.all_plain_lines(crate::frame_time::FrameRenderContext::capture());
    let panel_line = plain_lines
        .iter()
        .position(|line| line.contains("/exit"))
        .expect("command panel row should exist");
    let notice_line = plain_lines
        .iter()
        .position(|line| line.contains("Press Esc again to interrupt"))
        .expect("status notice should exist");
    let command_panel_rows = layout
        .all_line_anchors(crate::frame_time::FrameRenderContext::capture())
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
        model.render_command_panel_line(&item, "", true, 8, display_width(&item.name));

    assert_eq!(display_width(&plain_line), 8);
    assert_eq!(
        selectable.content_columns().map(|(start, _)| start),
        Some(2)
    );
}

#[test]
fn command_panel_highlights_subsequence_matches_in_command_name() {
    let mut model = Model::new(StartupBannerOptions::default());
    model.set_palette(default_palette(), true);
    model.set_window(80, 12);
    model.composer.reset_text_and_move_to_end("/md");
    model.sync_command_panel_navigation();

    let panel = model.current_inline_command_panel_render_result();
    let command_line_index = panel
        .plain_lines
        .iter()
        .position(|line| line.contains("/models"))
        .expect("models command should render");
    let command_line = &panel.lines[command_line_index];

    let highlighted_text = command_line
        .spans
        .iter()
        .filter(|span| {
            span.style.bg == default_palette().surface
                || span.style.add_modifier.contains(Modifier::REVERSED)
        })
        .map(|span| span.content.as_ref())
        .collect::<String>();

    assert_eq!(highlighted_text, "md");
}

#[test]
fn sends_back_command_opens_coarse_rewind_when_esc_mode_is_entry() {
    let mut model = Model::new_with_options(
        StartupBannerOptions::default(),
        ModelOptions {
            esc_rewind_mode: crate::EscRewindMode::Entry,
            ..ModelOptions::default()
        },
    );
    model
        .transcript_mut()
        .append_message(Sender::User, "first question");
    model
        .transcript_mut()
        .append_message(Sender::Assistant, "first answer");
    model.composer.reset_text_and_move_to_end("/sends-back");
    model.sync_command_panel_navigation();

    let effect = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Enter)));

    assert_eq!(effect, None);
    assert!(model.transcript_overlay_active());
    assert_eq!(model.composer_text(), "");
}
