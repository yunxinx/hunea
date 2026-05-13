use crossterm::event::KeyCode;

use super::*;
use crate::{HeroOptions, ModelOptions, StatusLineItem, StyleMode, document::DocumentAnchorRegion};

#[test]
fn single_unmatched_character_keeps_command_panel_active() {
    assert_eq!(command_panel_query("/h"), Some("h".to_string()));
    assert_eq!(command_panel_query("/he"), None);
    assert_eq!(command_panel_query("/ "), None);
}

#[test]
fn current_status_notice_still_renders_below_command_panel() {
    let mut model = Model::new_with_options(
        HeroOptions::default(),
        ModelOptions {
            style_mode: StyleMode::Cx,
            status_line_items: vec![StatusLineItem::GitBranch],
            ..ModelOptions::default()
        },
    );
    model.set_window(24, 6);
    model.show_transient_status_notice("Selection copied");
    model.composer.replace_text_and_move_to_end("/");
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
        HeroOptions::default(),
        ModelOptions {
            style_mode: StyleMode::Cx,
            status_line_items: vec![StatusLineItem::GitBranch],
            ..ModelOptions::default()
        },
    );
    model.set_window(32, 8);
    model.composer.replace_text_and_move_to_end("/");
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
fn rendered_command_panel_line_respects_available_width() {
    let model = Model::new(HeroOptions::default());

    let item = base_command_panel_items_for_query("exit")
        .into_iter()
        .next()
        .expect("exit item should exist");
    let (_, plain_line, selectable) =
        model.render_command_panel_line(&item, true, 8, item.name.width());

    assert_eq!(plain_line.width(), 8);
    assert_eq!(
        selectable.content_columns().map(|(start, _)| start),
        Some(2)
    );
}

#[test]
fn acp_commands_merge_alphabetically_and_keep_acp_on_collision() {
    let mut model = Model::new(HeroOptions::default());
    model.selected_acp_agent = Some("Kimi Code CLI".to_string());
    model.apply_acp_available_commands(
        "Kimi Code CLI",
        vec![
            mo_acp::AcpAvailableCommand {
                name: "web".to_string(),
                description: "Search the web".to_string(),
                input: Some(mo_acp::AcpAvailableCommandInput::Unstructured {
                    hint: "query to search for".to_string(),
                }),
            },
            mo_acp::AcpAvailableCommand {
                name: "clear".to_string(),
                description: "ACP clear".to_string(),
                input: None,
            },
        ],
    );

    let items = model.filter_command_panel_items("");
    let names = items
        .iter()
        .map(|item| item.name.as_str())
        .collect::<Vec<_>>();

    assert_eq!(names, vec!["/clear", "/exit", "/models", "/web"]);
    assert_eq!(
        items
            .iter()
            .find(|item| item.name == "/clear")
            .expect("ACP clear command should be visible")
            .description,
        "ACP clear"
    );
}

#[test]
fn acp_command_enter_completion_keeps_trailing_space() {
    let mut model = Model::new(HeroOptions::default());
    model.selected_acp_agent = Some("Kimi Code CLI".to_string());
    model.apply_acp_available_commands(
        "Kimi Code CLI",
        vec![mo_acp::AcpAvailableCommand {
            name: "web".to_string(),
            description: "Search the web".to_string(),
            input: Some(mo_acp::AcpAvailableCommandInput::Unstructured {
                hint: "query to search for".to_string(),
            }),
        }],
    );
    model.composer_mut().replace_text_and_move_to_end("/we");
    model.sync_command_panel_navigation();

    let item = model
        .filter_command_panel_items("we")
        .into_iter()
        .next()
        .expect("ACP web command should be filtered");

    let effect = model.execute_command_panel_item(item);

    assert_eq!(model.composer_text(), "/web ");
    assert!(effect.is_none());
}

#[test]
fn acp_command_tab_completion_keeps_trailing_space() {
    let mut model = Model::new(HeroOptions::default());
    model.selected_acp_agent = Some("Kimi Code CLI".to_string());
    model.apply_acp_available_commands(
        "Kimi Code CLI",
        vec![mo_acp::AcpAvailableCommand {
            name: "web".to_string(),
            description: "Search the web".to_string(),
            input: Some(mo_acp::AcpAvailableCommandInput::Unstructured {
                hint: "query to search for".to_string(),
            }),
        }],
    );
    model.composer_mut().replace_text_and_move_to_end("/we");
    model.sync_command_panel_navigation();

    let handled = model.handle_command_panel_key(KeyCode::Tab.into());

    assert_eq!(model.composer_text(), "/web ");
    assert_eq!(handled, Some(None));
}

#[test]
fn background_terminal_commands_show_when_acp_terminal_is_active() {
    let mut model = Model::new(HeroOptions::default());
    model.selected_acp_agent = Some("Kimi Code CLI".to_string());
    model.apply_acp_terminal_snapshot_from_runtime(mo_acp::AcpTerminalSnapshot {
        terminal_id: "term-1".to_string(),
        command: Some("npm run dev".to_string()),
        cwd: Some("/tmp/project".to_string()),
        output: "ready on http://localhost:3000\n".to_string(),
        truncated: false,
        exit_status: None,
        released: false,
    });

    let names = model
        .filter_command_panel_items("")
        .into_iter()
        .map(|item| item.name)
        .collect::<Vec<_>>();

    assert!(names.contains(&"/ps".to_string()));
    assert!(names.contains(&"/stop".to_string()));
}

#[test]
fn ps_command_appends_background_terminal_summary() {
    let mut model = Model::new(HeroOptions::default());
    model.transcript_mut().clear();
    model.selected_acp_agent = Some("Kimi Code CLI".to_string());
    model.apply_acp_terminal_snapshot_from_runtime(mo_acp::AcpTerminalSnapshot {
        terminal_id: "term-1".to_string(),
        command: Some("npm run dev".to_string()),
        cwd: Some("/tmp/project".to_string()),
        output: "ready on http://localhost:3000\n".to_string(),
        truncated: false,
        exit_status: None,
        released: false,
    });
    model.composer_mut().replace_text_and_move_to_end("/ps");
    model.sync_command_panel_navigation();

    let item = model
        .filter_command_panel_items("ps")
        .into_iter()
        .next()
        .expect("/ps command should be filtered");
    let effect = model.execute_command_panel_item(item);

    let plain = model.transcript_plain_items().join("\n");
    assert!(effect.is_none());
    assert_eq!(model.composer_text(), "");
    assert!(plain.contains("Background terminals:"));
    assert!(plain.contains("npm run dev"));
    assert!(plain.contains("/tmp/project"));
    assert!(plain.contains("ready on http://localhost:3000"));
}

#[test]
fn stop_command_requests_background_terminal_stop() {
    let mut model = Model::new(HeroOptions::default());
    model.transcript_mut().clear();
    model.selected_acp_agent = Some("Kimi Code CLI".to_string());
    model.apply_acp_terminal_snapshot_from_runtime(mo_acp::AcpTerminalSnapshot {
        terminal_id: "term-1".to_string(),
        command: Some("npm run dev".to_string()),
        cwd: None,
        output: String::new(),
        truncated: false,
        exit_status: None,
        released: false,
    });
    model.composer_mut().replace_text_and_move_to_end("/stop");
    model.sync_command_panel_navigation();

    let item = model
        .filter_command_panel_items("stop")
        .into_iter()
        .next()
        .expect("/stop command should be filtered");
    let effect = model.execute_command_panel_item(item);

    assert_eq!(effect, Some(AppEffect::StopAcpBackgroundTerminals));
    assert_eq!(model.composer_text(), "");
    assert!(
        model
            .transcript_plain_items()
            .join("\n")
            .contains("Stopping all background terminals.")
    );
}
