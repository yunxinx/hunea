use crossterm::event::{KeyCode, KeyEvent, MouseButton};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier},
};
use runtime_domain::session::{SessionTreePayload, SessionTreeRow, SessionTreeRowKind};

use super::{ENTRY_TREE_HEADER_HEIGHT, ENTRY_TREE_HEADER_RULE_HEIGHT};
use crate::runner::TerminalMouseModePreference;
use crate::{
    AppEffect, AppEvent, Model, StartupBannerOptions,
    theme::{
        accent_text_style, command_accent_text_style, default_palette, muted_text_style,
        primary_text_style, tertiary_text_style,
    },
};

#[test]
fn entry_tree_defaults_to_latest_row_on_last_page() {
    let mut model = ready_model();
    model.open_entry_tree_loading();
    model.apply_entry_tree_payload(SessionTreePayload {
        rows: (0..6).map(numbered_tree_row).collect(),
    });

    let rows = rendered_rows(&render_model_buffer(&mut model, 60, 8));

    assert!(
        rows[0].starts_with("  Session Tree (6 of 6)"),
        "header should show latest selected position by default: {rows:?}"
    );
    assert!(
        rows[6].contains(" Page 2/2 "),
        "tree should open on the page containing the latest row: {rows:?}"
    );
    assert!(
        rows.iter().any(|row| row.contains("message 4"))
            && rows.iter().any(|row| row.contains("message 5"))
            && rows.iter().all(|row| !row.contains("message 0")),
        "body should render the last page, not the first page: {rows:?}"
    );
}

#[test]
fn entry_tree_enter_selects_logical_row_with_prefill() {
    let mut model = ready_model();
    model.open_entry_tree_loading();
    model.apply_entry_tree_payload(SessionTreePayload {
        rows: vec![
            tree_row(
                "assistant-a",
                SessionTreeRowKind::Assistant,
                "alpha answer",
                None,
                Some("assistant-a"),
            ),
            tree_row(
                "user-b",
                SessionTreeRowKind::User,
                "beta question",
                Some("beta question".to_string()),
                Some("assistant-a"),
            ),
        ],
    });

    let effect = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Enter)));

    assert_eq!(
        effect,
        Some(AppEffect::SelectEntryRewind {
            entry_id: "user-b".to_string(),
            prefill: Some("beta question".to_string()),
        })
    );
    assert!(!model.entry_tree_active());
}

#[test]
fn entry_tree_enter_ignores_non_rewindable_reasoning_row() {
    let mut model = ready_model();
    model.open_entry_tree_loading();
    model.apply_entry_tree_payload(SessionTreePayload {
        rows: vec![tree_row(
            "reason-a",
            SessionTreeRowKind::Reasoning,
            "partial thought",
            None,
            None,
        )],
    });

    let effect = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Enter)));
    let rows = rendered_rows(&render_model_buffer(&mut model, 60, 8));

    assert_eq!(effect, None);
    assert!(model.entry_tree_active());
    assert!(
        rows[7].contains("Space preview") && !rows[7].contains("Enter rewind"),
        "non-rewindable reasoning should not advertise Enter rewind: {rows:?}"
    );
}

#[test]
fn entry_tree_left_right_page_and_up_down_move_selection() {
    let mut model = ready_model();
    model.open_entry_tree_loading();
    model.apply_entry_tree_payload(SessionTreePayload {
        rows: (0..8).map(numbered_tree_row).collect(),
    });

    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Left)));
    let previous_page_rows = rendered_rows(&render_model_buffer(&mut model, 60, 8));
    assert!(
        previous_page_rows[0].starts_with("  Session Tree (1 of 8)"),
        "Left should jump to the first row of the previous page: {previous_page_rows:?}"
    );
    assert!(
        previous_page_rows[6].contains(" Page 1/2 "),
        "page label should remain on the selected page: {previous_page_rows:?}"
    );

    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Down)));
    let moved_down_rows = rendered_rows(&render_model_buffer(&mut model, 60, 8));
    assert!(
        moved_down_rows[0].starts_with("  Session Tree (2 of 8)"),
        "Down should move selection by one logical row: {moved_down_rows:?}"
    );

    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Up)));
    let moved_rows = rendered_rows(&render_model_buffer(&mut model, 60, 8));
    assert!(
        moved_rows[0].starts_with("  Session Tree (1 of 8)"),
        "Up should move selection by one logical row: {moved_rows:?}"
    );

    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Right)));
    let next_page_rows = rendered_rows(&render_model_buffer(&mut model, 60, 8));
    assert!(
        next_page_rows[0].starts_with("  Session Tree (5 of 8)"),
        "Right should move to the next page and select its first row: {next_page_rows:?}"
    );
}

#[test]
fn entry_tree_maps_mouse_wheel_to_selection_if_delivered() {
    let mut model = ready_model();
    model.open_entry_tree_loading();
    model.apply_entry_tree_payload(SessionTreePayload {
        rows: (0..4).map(numbered_tree_row).collect(),
    });

    model.update(AppEvent::MouseWheel { delta_lines: -3 });
    let up_rows = rendered_rows(&render_model_buffer(&mut model, 60, 8));
    assert!(
        up_rows[0].starts_with("  Session Tree (3 of 4)"),
        "mouse wheel up should move selection up by one row: {up_rows:?}"
    );

    model.update(AppEvent::MouseWheel { delta_lines: 3 });
    let down_rows = rendered_rows(&render_model_buffer(&mut model, 60, 8));
    assert!(
        down_rows[0].starts_with("  Session Tree (4 of 4)"),
        "mouse wheel down should move selection down by one row: {down_rows:?}"
    );
}

#[test]
fn entry_tree_space_opens_single_message_preview_and_enter_is_ignored() {
    let mut model = ready_model();
    model.set_window(60, 8);
    model.open_entry_tree_loading();
    model.apply_entry_tree_payload(SessionTreePayload {
        rows: vec![tree_row(
            "assistant-a",
            SessionTreeRowKind::Assistant,
            &(0..12)
                .map(|index| format!("preview line {index}"))
                .collect::<Vec<_>>()
                .join("\n"),
            None,
            Some("assistant-a"),
        )],
    });

    let effect = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char(' '))));
    assert_eq!(effect, None);
    assert!(model.entry_tree_preview_active());

    let preview_rows = rendered_rows(&render_model_buffer(&mut model, 60, 8));
    assert!(
        preview_rows
            .iter()
            .any(|row| row.contains("preview line 11")),
        "preview should show the selected row's full message and start on latest page: {preview_rows:?}"
    );
    assert!(
        preview_rows.iter().any(|row| row.contains(" Page ")),
        "preview should use the same page rule style as resume preview: {preview_rows:?}"
    );

    let enter_effect = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Enter)));
    assert_eq!(
        enter_effect, None,
        "Enter inside preview must not execute rewind"
    );
    assert!(model.entry_tree_preview_active());
    assert!(model.entry_tree_active());

    let close_effect = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char(' '))));
    assert_eq!(close_effect, None);
    assert!(!model.entry_tree_preview_active());
    assert!(model.entry_tree_active());
}

#[test]
fn entry_tree_preview_maps_wheel_to_page_navigation_if_delivered() {
    let mut model = ready_model();
    model.set_window(60, 8);
    model.open_entry_tree_loading();
    model.apply_entry_tree_payload(SessionTreePayload {
        rows: vec![tree_row(
            "assistant-a",
            SessionTreeRowKind::Assistant,
            &(0..12)
                .map(|index| format!("preview line {index}"))
                .collect::<Vec<_>>()
                .join("\n"),
            None,
            Some("assistant-a"),
        )],
    });
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char(' '))));

    model.update(AppEvent::MouseWheel { delta_lines: -3 });
    let previous_page_rows = rendered_rows(&render_model_buffer(&mut model, 60, 8));
    assert!(
        previous_page_rows
            .iter()
            .any(|row| row.contains(" Page 1/2 ")),
        "wheel up should page back inside tree preview: {previous_page_rows:?}"
    );

    model.update(AppEvent::MouseWheel { delta_lines: 3 });
    let next_page_rows = rendered_rows(&render_model_buffer(&mut model, 60, 8));
    assert!(
        next_page_rows.iter().any(|row| row.contains(" Page 2/2 ")),
        "wheel down should page forward inside tree preview: {next_page_rows:?}"
    );
}

#[test]
fn entry_tree_preview_keeps_terminal_native_mouse_selection() {
    let mut model = ready_model();
    model.open_entry_tree_loading();
    model.apply_entry_tree_payload(SessionTreePayload {
        rows: (0..3).map(numbered_tree_row).collect(),
    });

    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char(' '))));

    assert!(model.entry_tree_preview_active());
    assert_eq!(
        model.mouse_mode_preference(),
        TerminalMouseModePreference::NativeWithAlternateScroll,
        "tree preview should keep terminal-native selection while preserving alternate-scroll wheel delivery"
    );

    model.update(AppEvent::MouseDown {
        button: MouseButton::Left,
        column: 12,
        row: ENTRY_TREE_HEADER_HEIGHT + ENTRY_TREE_HEADER_RULE_HEIGHT,
    });
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char(' '))));
    let rows = rendered_rows(&render_model_buffer(&mut model, 60, 8));
    assert!(
        rows[0].starts_with("  Session Tree (3 of 3)"),
        "clicking inside preview must not change the underlying tree selection: {rows:?}"
    );
}

#[test]
fn entry_tree_styles_rows_by_message_kind_without_zebra_background() {
    let mut model = ready_model();
    model.open_entry_tree_loading();
    model.apply_entry_tree_payload(SessionTreePayload {
        rows: vec![
            tree_row(
                "user-a",
                SessionTreeRowKind::User,
                "user body",
                Some("user body".to_string()),
                Some("user-a"),
            ),
            tree_row(
                "reason-b",
                SessionTreeRowKind::Reasoning,
                "reason body",
                None,
                Some("assistant-d"),
            ),
            tree_row(
                "tool-c",
                SessionTreeRowKind::Tool,
                "tool body",
                None,
                Some("tool-c"),
            ),
            tree_row(
                "assistant-d",
                SessionTreeRowKind::Assistant,
                "assistant body",
                None,
                Some("assistant-d"),
            ),
        ],
    });

    let palette = *model.palette();
    let buffer = render_model_buffer(&mut model, 60, 8);
    let user_row = ENTRY_TREE_HEADER_HEIGHT + ENTRY_TREE_HEADER_RULE_HEIGHT;
    let reason_row = user_row + 1;
    let tool_row = user_row + 2;
    let selected_assistant_row = user_row + 3;

    let user_content_column = column_of_text(&buffer, user_row, "user body");
    assert_eq!(
        buffer[(user_content_column, user_row)].fg,
        command_accent_text_style(palette)
            .fg
            .expect("default palette should provide command accent"),
        "user message content should use the same color as the selected prompt marker"
    );

    let reason_content_column = column_of_text(&buffer, reason_row, "reason body");
    let reasoning_label_column = column_of_text(&buffer, reason_row, "reasoning");
    assert_ne!(
        reasoning_label_column, reason_content_column,
        "reasoning prefix should be rendered separately from content"
    );
    assert_eq!(
        buffer[(reason_content_column, reason_row)].fg,
        tertiary_text_style(palette)
            .fg
            .expect("default palette should provide tertiary color"),
        "reasoning content should use the existing weakened reasoning color"
    );
    assert!(
        buffer[(reason_content_column, reason_row)]
            .modifier
            .contains(Modifier::ITALIC),
        "reasoning content should use the existing italic reasoning style"
    );

    let tool_label_column = column_of_text(&buffer, tool_row, "tool");
    assert_eq!(
        buffer[(tool_label_column, tool_row)].bg,
        palette.accent,
        "tool prefix should use the palette accent background, which differs from user command accent"
    );
    assert_ne!(
        Some(palette.accent),
        command_accent_text_style(palette).fg,
        "tool prefix background must not reuse the user content color"
    );

    let tool_content_column = column_of_text(&buffer, tool_row, "tool body");
    assert_eq!(
        buffer[(tool_content_column, tool_row)].fg,
        muted_text_style(palette)
            .fg
            .expect("default palette should provide muted color"),
        "tool content should use the weakened text color"
    );

    for row in [user_row, reason_row, tool_row] {
        assert_eq!(
            buffer[(59, row)].bg,
            Color::Reset,
            "unselected rows should not keep zebra/surface trailing backgrounds"
        );
    }

    let selected_content_column = column_of_text(&buffer, selected_assistant_row, "assistant body");
    assert_eq!(
        buffer[(selected_content_column, selected_assistant_row)].fg,
        primary_text_style(palette)
            .fg
            .expect("default palette should provide primary text color"),
        "selected assistant content should keep the assistant text color instead of being recolored by selection"
    );
    assert!(
        buffer[(selected_content_column, selected_assistant_row)]
            .modifier
            .contains(Modifier::BOLD),
        "selected assistant content should keep its existing bold emphasis"
    );
    assert!(
        buffer[(selected_content_column, selected_assistant_row)]
            .modifier
            .contains(Modifier::REVERSED),
        "selected message content should use reverse video"
    );
    assert!(
        !buffer[(0, selected_assistant_row)]
            .modifier
            .contains(Modifier::REVERSED),
        "selected row marker/prefix area should not be reversed"
    );
    assert!(
        !buffer[(
            selected_content_column.saturating_sub(1),
            selected_assistant_row
        )]
            .modifier
            .contains(Modifier::REVERSED),
        "selected row should not reverse the space before message content"
    );
    assert!(
        !buffer[(59, selected_assistant_row)]
            .modifier
            .contains(Modifier::REVERSED),
        "selected row trailing blank area should not use reverse video"
    );
    assert_eq!(
        buffer[(59, selected_assistant_row)].bg,
        Color::Reset,
        "selected row trailing blank area should not use a selected background"
    );
}

#[test]
fn entry_tree_marks_branch_parent_with_at_and_keeps_branch_descendants_on_lane() {
    let mut model = ready_model();
    model.open_entry_tree_loading();
    model.apply_entry_tree_payload(SessionTreePayload {
        rows: vec![
            tree_row_with_parent_at_depth(
                "user-a",
                None,
                SessionTreeRowKind::User,
                "root question",
                0,
                true,
                false,
            ),
            tree_row_with_parent_at_depth(
                "assistant-b",
                Some("user-a"),
                SessionTreeRowKind::Assistant,
                "inactive answer",
                1,
                false,
                false,
            ),
            tree_row_with_parent_at_depth(
                "assistant-c",
                Some("user-a"),
                SessionTreeRowKind::Assistant,
                "active answer",
                1,
                true,
                false,
            ),
            tree_row_with_parent_at_depth(
                "user-d",
                Some("assistant-c"),
                SessionTreeRowKind::User,
                "current follow up",
                2,
                true,
                true,
            ),
        ],
    });

    let palette = *model.palette();
    let buffer = render_model_buffer(&mut model, 72, 8);
    let rows = rendered_rows(&buffer);
    let body_top = ENTRY_TREE_HEADER_HEIGHT + ENTRY_TREE_HEADER_RULE_HEIGHT;

    assert!(
        rows[usize::from(body_top)].contains("@")
            && rows[usize::from(body_top)].contains("root question"),
        "branch parent should use @, not the current row: {rows:?}"
    );
    assert!(
        !rendered_prefix_before(&buffer, body_top + 1, "assistant").contains("○")
            && !rendered_prefix_before(&buffer, body_top + 1, "assistant").contains("├─")
            && rows[usize::from(body_top + 1)].contains("inactive answer"),
        "inactive sibling branch should not draw its own graph node or connector: {rows:?}"
    );
    assert!(
        rows[usize::from(body_top + 2)].contains("╰─●")
            && rows[usize::from(body_top + 2)].contains("active answer"),
        "active sibling branch should use an active path graph node: {rows:?}"
    );
    assert!(
        !rendered_prefix_before(&buffer, body_top + 3, "user").contains("@")
            && rows[usize::from(body_top + 3)].contains("current follow up"),
        "current row should not use @ unless it is itself a branch parent: {rows:?}"
    );
    assert!(
        !rendered_prefix_before(&buffer, body_top + 3, "user").contains("╰─"),
        "linear descendants inside a branch should stay on the same branch lane instead of adding another nested connector: {rows:?}"
    );

    let current_graph_column = column_of_text(&buffer, body_top + 3, "●");
    assert_eq!(
        buffer[(current_graph_column, body_top + 3)].fg,
        accent_text_style(palette)
            .fg
            .expect("default palette should provide accent color"),
        "active/current graph branches should use the accent color"
    );
    assert!(
        !buffer[(current_graph_column, body_top + 3)]
            .modifier
            .contains(Modifier::REVERSED),
        "selected row reverse video should not apply to the graph prefix"
    );
}

#[test]
fn entry_tree_uses_dynamic_graph_indent_instead_of_fixed_gutter() {
    let mut model = ready_model();
    model.open_entry_tree_loading();
    model.apply_entry_tree_payload(SessionTreePayload {
        rows: vec![
            tree_row_with_parent_at_depth(
                "user-a",
                None,
                SessionTreeRowKind::User,
                "root question",
                0,
                true,
                false,
            ),
            tree_row_with_parent_at_depth(
                "assistant-b",
                Some("user-a"),
                SessionTreeRowKind::Assistant,
                "inactive answer",
                1,
                false,
                false,
            ),
            tree_row_with_parent_at_depth(
                "assistant-c",
                Some("user-a"),
                SessionTreeRowKind::Assistant,
                "active answer",
                1,
                true,
                true,
            ),
        ],
    });

    let buffer = render_model_buffer(&mut model, 72, 7);
    let body_top = ENTRY_TREE_HEADER_HEIGHT + ENTRY_TREE_HEADER_RULE_HEIGHT;
    let root_kind_column = column_of_text(&buffer, body_top, "user");
    let branch_kind_column = column_of_text(&buffer, body_top + 1, "assistant");

    assert!(
        root_kind_column < branch_kind_column,
        "root rows should not be padded to a fixed graph gutter when a branch row is wider"
    );
}

#[test]
fn entry_tree_highlights_the_entire_selected_visible_branch() {
    let mut model = ready_model();
    model.open_entry_tree_loading();
    model.apply_entry_tree_payload(SessionTreePayload {
        rows: vec![
            tree_row_with_parent_at_depth(
                "user-a",
                None,
                SessionTreeRowKind::User,
                "root question",
                0,
                false,
                false,
            ),
            tree_row_with_parent_at_depth(
                "assistant-b",
                Some("user-a"),
                SessionTreeRowKind::Assistant,
                "inactive sibling",
                1,
                false,
                false,
            ),
            tree_row_with_parent_at_depth(
                "assistant-c",
                Some("user-a"),
                SessionTreeRowKind::Assistant,
                "selected branch",
                1,
                false,
                false,
            ),
            tree_row_with_parent_at_depth(
                "user-d",
                Some("assistant-c"),
                SessionTreeRowKind::User,
                "same branch after selected",
                2,
                false,
                false,
            ),
            tree_row_with_parent_at_depth(
                "assistant-e",
                Some("user-d"),
                SessionTreeRowKind::Assistant,
                "same branch tail",
                2,
                false,
                false,
            ),
        ],
    });
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Up)));
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Up)));

    let palette = *model.palette();
    let buffer = render_model_buffer(&mut model, 84, 9);
    let body_top = ENTRY_TREE_HEADER_HEIGHT + ENTRY_TREE_HEADER_RULE_HEIGHT;
    let rows = rendered_rows(&buffer);
    let inactive_sibling_prefix = rendered_prefix_before(&buffer, body_top + 1, "assistant");
    let same_branch_after_selection_graph_column = column_of_text(&buffer, body_top + 3, "·");

    assert!(
        !inactive_sibling_prefix.contains("○")
            && !inactive_sibling_prefix.contains("├─")
            && !inactive_sibling_prefix.contains("╰─"),
        "sibling branches outside the selected branch should not draw graph symbols: {rows:?}"
    );
    assert_eq!(
        buffer[(same_branch_after_selection_graph_column, body_top + 3)].fg,
        accent_text_style(palette)
            .fg
            .expect("default palette should provide accent color"),
        "rows after the selected item but inside the same visible branch should be highlighted"
    );
}

#[test]
fn entry_tree_keeps_selected_branch_lane_continuous_through_nested_branches() {
    let mut model = ready_model();
    model.open_entry_tree_loading();
    model.apply_entry_tree_payload(SessionTreePayload {
        rows: vec![
            tree_row_with_parent_at_depth(
                "user-a",
                None,
                SessionTreeRowKind::User,
                "root question",
                0,
                true,
                false,
            ),
            tree_row_with_parent_at_depth(
                "assistant-b",
                Some("user-a"),
                SessionTreeRowKind::Assistant,
                "outer inactive sibling",
                1,
                false,
                false,
            ),
            tree_row_with_parent_at_depth(
                "assistant-c",
                Some("user-a"),
                SessionTreeRowKind::Assistant,
                "outer selected branch",
                1,
                true,
                false,
            ),
            tree_row_with_parent_at_depth(
                "user-d",
                Some("assistant-c"),
                SessionTreeRowKind::User,
                "nested branch parent",
                1,
                true,
                false,
            ),
            tree_row_with_parent_at_depth(
                "assistant-e",
                Some("user-d"),
                SessionTreeRowKind::Assistant,
                "nested inactive sibling",
                2,
                false,
                false,
            ),
            tree_row_with_parent_at_depth(
                "assistant-f",
                Some("user-d"),
                SessionTreeRowKind::Assistant,
                "nested selected branch",
                2,
                true,
                false,
            ),
            tree_row_with_parent_at_depth(
                "user-g",
                Some("assistant-f"),
                SessionTreeRowKind::User,
                "selected branch tail",
                2,
                true,
                true,
            ),
        ],
    });

    let palette = *model.palette();
    let buffer = render_model_buffer(&mut model, 96, 11);
    let rows = rendered_rows(&buffer);
    let body_top = ENTRY_TREE_HEADER_HEIGHT + ENTRY_TREE_HEADER_RULE_HEIGHT;
    let nested_sibling_prefix = rendered_prefix_before(&buffer, body_top + 4, "assistant");
    let selected_tail_prefix = rendered_prefix_before(&buffer, body_top + 6, "user");

    assert!(
        nested_sibling_prefix.contains("│")
            && !nested_sibling_prefix.contains("○")
            && !nested_sibling_prefix.contains("·")
            && !nested_sibling_prefix.contains("├─")
            && !nested_sibling_prefix.contains("╰─"),
        "inactive nested sibling should only preserve the selected branch lane and should not draw its own graph symbols: {rows:?}"
    );
    assert!(
        selected_tail_prefix.contains("│") && selected_tail_prefix.contains("●"),
        "selected branch tail should still show a continuous lane after a nested branch: {rows:?}"
    );

    let outer_lane_column = column_of_text(&buffer, body_top + 4, "│");
    assert_eq!(
        buffer[(outer_lane_column, body_top + 4)].fg,
        accent_text_style(palette)
            .fg
            .expect("default palette should provide accent color"),
        "the lane that keeps the selected branch connected should use the selected branch color"
    );
}

#[test]
fn entry_tree_uses_dot_for_middle_nodes_on_the_selected_branch() {
    let mut model = ready_model();
    model.open_entry_tree_loading();
    model.apply_entry_tree_payload(SessionTreePayload {
        rows: vec![
            tree_row_with_parent_at_depth(
                "user-a",
                None,
                SessionTreeRowKind::User,
                "root question",
                0,
                true,
                false,
            ),
            tree_row_with_parent_at_depth(
                "assistant-b",
                Some("user-a"),
                SessionTreeRowKind::Assistant,
                "inactive sibling",
                1,
                false,
                false,
            ),
            tree_row_with_parent_at_depth(
                "assistant-c",
                Some("user-a"),
                SessionTreeRowKind::Assistant,
                "selected branch start",
                1,
                true,
                false,
            ),
            tree_row_with_parent_at_depth(
                "user-d",
                Some("assistant-c"),
                SessionTreeRowKind::User,
                "middle question",
                1,
                true,
                false,
            ),
            tree_row_with_parent_at_depth(
                "assistant-e",
                Some("user-d"),
                SessionTreeRowKind::Assistant,
                "selected branch tail",
                1,
                true,
                true,
            ),
        ],
    });

    let buffer = render_model_buffer(&mut model, 84, 9);
    let rows = rendered_rows(&buffer);
    let body_top = ENTRY_TREE_HEADER_HEIGHT + ENTRY_TREE_HEADER_RULE_HEIGHT;

    assert!(
        rendered_prefix_before(&buffer, body_top + 2, "assistant").contains("●"),
        "selected branch start should keep the strong node marker: {rows:?}"
    );
    assert!(
        rendered_prefix_before(&buffer, body_top + 3, "user").contains("·"),
        "middle selected branch nodes should use the lighter dot marker: {rows:?}"
    );
    assert!(
        rendered_prefix_before(&buffer, body_top + 4, "assistant").contains("●"),
        "selected branch tail/current node should keep the strong node marker: {rows:?}"
    );
}

#[test]
fn entry_tree_renders_reasoning_and_tool_as_graph_continuation_rows() {
    let mut model = ready_model();
    model.open_entry_tree_loading();
    model.apply_entry_tree_payload(SessionTreePayload {
        rows: vec![
            tree_row_with_parent_at_depth(
                "user-a",
                None,
                SessionTreeRowKind::User,
                "question",
                0,
                true,
                false,
            ),
            tree_row_with_parent_at_depth(
                "reason-b",
                Some("user-a"),
                SessionTreeRowKind::Reasoning,
                "thinking",
                0,
                true,
                false,
            ),
            tree_row_with_parent_at_depth(
                "tool-c",
                Some("reason-b"),
                SessionTreeRowKind::Tool,
                "output",
                0,
                true,
                false,
            ),
            tree_row_with_parent_at_depth(
                "assistant-d",
                Some("tool-c"),
                SessionTreeRowKind::Assistant,
                "answer",
                0,
                true,
                false,
            ),
        ],
    });

    let buffer = render_model_buffer(&mut model, 72, 8);
    let rows = rendered_rows(&buffer);
    let body_top = ENTRY_TREE_HEADER_HEIGHT + ENTRY_TREE_HEADER_RULE_HEIGHT;
    let reasoning_prefix = rendered_prefix_before(&buffer, body_top + 1, "reasoning");
    let tool_prefix = rendered_prefix_before(&buffer, body_top + 2, "tool");

    assert!(
        rows[usize::from(body_top)].contains("●") && rows[usize::from(body_top + 3)].contains("●"),
        "user/assistant rows should remain graph nodes: {rows:?}"
    );
    assert!(
        reasoning_prefix.contains("│")
            && !reasoning_prefix.contains("@")
            && !reasoning_prefix.contains("●")
            && !reasoning_prefix.contains("○"),
        "reasoning should continue the graph lane without adding a node: {rows:?}"
    );
    assert!(
        tool_prefix.contains("│")
            && !tool_prefix.contains("@")
            && !tool_prefix.contains("●")
            && !tool_prefix.contains("○"),
        "tool rows should continue the graph lane without adding a node: {rows:?}"
    );
}

#[test]
fn entry_tree_collapses_graph_prefix_when_branch_depth_exceeds_budget() {
    let mut model = ready_model();
    model.open_entry_tree_loading();
    let mut rows = vec![tree_row_with_parent_at_depth(
        "row-0",
        None,
        SessionTreeRowKind::User,
        "deep message 0",
        0,
        true,
        false,
    )];
    let mut branch_parent = "row-0".to_string();
    for depth in 1..9 {
        rows.push(tree_row_with_parent_at_depth(
            &format!("side-{depth}"),
            Some(&branch_parent),
            SessionTreeRowKind::Assistant,
            &format!("side branch {depth}"),
            depth,
            false,
            false,
        ));
        let row_id = format!("row-{depth}");
        rows.push(tree_row_with_parent_at_depth(
            &row_id,
            Some(&branch_parent),
            SessionTreeRowKind::User,
            &format!("deep message {depth}"),
            depth,
            true,
            depth == 8,
        ));
        branch_parent = row_id;
    }

    model.apply_entry_tree_payload(SessionTreePayload { rows });

    let rows = rendered_rows(&render_model_buffer(&mut model, 60, 22));
    let selected_row = rows
        .iter()
        .find(|row| row.contains("deep message 8"))
        .expect("deep current row should be visible");

    assert!(
        selected_row.contains("…"),
        "deep graph prefixes should collapse with an ellipsis counter instead of consuming the whole row: {rows:?}"
    );
    assert!(
        selected_row.contains("deep message 8"),
        "collapsed graph prefix must preserve message content: {rows:?}"
    );
}

#[test]
fn entry_tree_captures_mouse_for_click_selection_and_keeps_coalescing() {
    let mut model = ready_model();
    model.open_entry_tree_loading();
    model.apply_entry_tree_payload(SessionTreePayload {
        rows: (0..3).map(numbered_tree_row).collect(),
    });

    assert_eq!(
        model.mouse_mode_preference(),
        TerminalMouseModePreference::CaptureWithAlternateScroll,
        "tree should capture mouse clicks while keeping alternate-scroll behavior for wheel navigation"
    );
    assert!(
        model
            .terminal_input_coalescing()
            .has_page_scroll_burst_coalescing,
        "tree should coalesce high-frequency wheel bursts like resume picker"
    );
}

#[test]
fn entry_tree_left_click_selects_visible_body_row() {
    let mut model = ready_model();
    model.open_entry_tree_loading();
    model.apply_entry_tree_payload(SessionTreePayload {
        rows: (0..4).map(numbered_tree_row).collect(),
    });

    let effect = model.update(AppEvent::MouseDown {
        button: MouseButton::Left,
        column: 12,
        row: ENTRY_TREE_HEADER_HEIGHT + ENTRY_TREE_HEADER_RULE_HEIGHT + 1,
    });
    let rows = rendered_rows(&render_model_buffer(&mut model, 60, 8));

    assert_eq!(effect, None);
    assert!(
        rows[0].starts_with("  Session Tree (2 of 4)"),
        "clicking the second visible body row should select the second logical row: {rows:?}"
    );

    model.update(AppEvent::MouseDown {
        button: MouseButton::Left,
        column: 12,
        row: ENTRY_TREE_HEADER_HEIGHT,
    });
    let after_header_click_rows = rendered_rows(&render_model_buffer(&mut model, 60, 8));
    assert!(
        after_header_click_rows[0].starts_with("  Session Tree (2 of 4)"),
        "clicking tree chrome should not move selection: {after_header_click_rows:?}"
    );
}

#[test]
fn entry_tree_renders_fixed_header_rules_footer_and_one_line_rows() {
    let mut model = ready_model();
    model.open_entry_tree_loading();
    model.apply_entry_tree_payload(SessionTreePayload {
        rows: (0..3).map(numbered_tree_row).collect(),
    });

    let rows = rendered_rows(&render_model_buffer(&mut model, 60, 8));

    assert!(
        rows[0].starts_with("  Session Tree (3 of 3)"),
        "header should show selected position over visible row count: {rows:?}"
    );
    assert!(
        !rows[0].contains("Search:"),
        "tree header must not expose search UI: {rows:?}"
    );
    assert!(
        rows[1].trim().chars().all(|character| character == '╌'),
        "header/list separator should be fixed: {rows:?}"
    );
    assert!(
        rows[2].contains("message 0")
            && rows[3].contains("message 1")
            && rows[4].contains("message 2"),
        "each logical row should occupy exactly one line without blank separators: {rows:?}"
    );
    assert!(
        rows[6].contains(" Page 1/1 "),
        "page rule should stay fixed above the footer: {rows:?}"
    );
    assert!(
        rows[7].contains("Space preview")
            && rows[7].contains("Enter rewind")
            && !rows[7].contains("search"),
        "footer should describe tree actions without search hints: {rows:?}"
    );
}

#[test]
fn entry_tree_esc_closes_without_effect() {
    let mut model = ready_model();
    model.open_entry_tree_loading();
    model.apply_entry_tree_payload(SessionTreePayload {
        rows: vec![tree_row(
            "assistant-a",
            SessionTreeRowKind::Assistant,
            "alpha answer",
            None,
            Some("assistant-a"),
        )],
    });

    let effect = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Esc)));

    assert_eq!(effect, None);
    assert!(!model.entry_tree_active());
}

fn render_model_buffer(model: &mut Model, width: u16, height: u16) -> Buffer {
    let area = Rect::new(0, 0, width, height);
    let mut buffer = Buffer::empty(area);
    let _ = model.render_to_buffer(area, &mut buffer);
    buffer
}

fn rendered_rows(buffer: &Buffer) -> Vec<String> {
    (0..buffer.area.height)
        .map(|row| {
            let mut line = String::new();
            for column in 0..buffer.area.width {
                line.push_str(buffer[(column, row)].symbol());
            }
            line
        })
        .collect()
}

fn column_of_text(buffer: &Buffer, row: u16, expected_text: &str) -> u16 {
    for column in 0..buffer.area.width {
        let mut text = String::new();
        for text_column in column..buffer.area.width {
            text.push_str(buffer[(text_column, row)].symbol());
            if text.len() >= expected_text.len() {
                break;
            }
        }
        if text.starts_with(expected_text) {
            return column;
        }
    }

    panic!("expected to find {expected_text:?} on row {row}");
}

fn rendered_prefix_before(buffer: &Buffer, row: u16, expected_text: &str) -> String {
    let end_column = column_of_text(buffer, row, expected_text);
    let mut prefix = String::new();
    for column in 0..end_column {
        prefix.push_str(buffer[(column, row)].symbol());
    }
    prefix
}

fn ready_model() -> Model {
    let mut model = Model::new(StartupBannerOptions::default());
    model.set_window(60, 8);
    model.set_palette(default_palette(), true);
    model
}

fn numbered_tree_row(index: usize) -> SessionTreeRow {
    let kind = if index.is_multiple_of(2) {
        SessionTreeRowKind::User
    } else {
        SessionTreeRowKind::Assistant
    };
    tree_row(
        &format!("row-{index}"),
        kind,
        &format!("message {index}"),
        (kind == SessionTreeRowKind::User).then(|| format!("message {index}")),
        Some(&format!("target-{index}")),
    )
}

fn tree_row_with_parent_at_depth(
    row_id: &str,
    parent_id: Option<&str>,
    kind: SessionTreeRowKind,
    content: &str,
    display_depth: usize,
    is_active_path: bool,
    is_current: bool,
) -> SessionTreeRow {
    SessionTreeRow {
        parent_id: parent_id.map(str::to_string),
        display_depth,
        is_active_path,
        is_current,
        ..tree_row(row_id, kind, content, None, Some(row_id))
    }
}

fn tree_row(
    row_id: &str,
    kind: SessionTreeRowKind,
    content: &str,
    rewind_prefill: Option<String>,
    rewind_target_id: Option<&str>,
) -> SessionTreeRow {
    SessionTreeRow {
        row_id: row_id.to_string(),
        parent_id: None,
        display_depth: 0,
        kind,
        display_text: content.split_whitespace().collect::<Vec<_>>().join(" "),
        summary: content.split_whitespace().collect::<Vec<_>>().join(" "),
        preview_content: content.to_string(),
        rewind_target_id: rewind_target_id.map(str::to_string),
        rewind_prefill,
        is_active_path: true,
        is_current: false,
    }
}
