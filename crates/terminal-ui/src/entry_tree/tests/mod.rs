use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton};
use ratatui::{
    buffer::Buffer,
    style::{Color, Modifier},
};
use runtime_domain::session::{
    RuntimeEvent, RuntimeToolActivity, RuntimeToolActivityStatus, RuntimeToolKind,
    SessionLoadRequestId, SessionTreePayload, SessionTreeRow, SessionTreeRowKind,
    TranscriptReplayItem, TranscriptReplayRole,
};

use super::{
    BRANCH_PICKER_ITEM_TOP_OFFSET, ENTRY_TREE_HEADER_HEIGHT, ENTRY_TREE_HEADER_RULE_HEIGHT,
};
use crate::runner::TerminalMouseModePreference;
use crate::test_helpers::{
    branch_choice, branch_choice_with_metadata, branch_tree_payload, numbered_tree_row,
    render_model_buffer, rendered_rows, tree_row, tree_row_with_branch_choices,
    tree_row_with_parent_at_depth, tree_row_with_preview_replay_items,
};
use crate::time::current_unix_timestamp_ms;
use crate::{
    AppEffect, AppEvent, Model, ModelOptions, StartupBannerOptions,
    overlay_input_result::OverlayInputResult,
    runtime::RuntimeEventApply,
    theme::{
        accent_text_style, approval_rejected_text_style, command_accent_text_style,
        default_palette, muted_text_style, primary_text_style, table_header_text_style,
        terminal_default_palette, tertiary_text_style,
    },
};
mod branch_picker;
mod branch_preview;
mod branch_tree;
mod graph;
mod input;
mod preview;

#[test]
fn entry_tree_defaults_to_latest_row_on_last_page() {
    let mut model = ready_model();
    model.open_entry_tree_loading();
    model.apply_entry_tree_payload(SessionTreePayload {
        rows: (0..6).map(numbered_tree_row).collect(),
        current_row_id: None,
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
fn entry_tree_selects_payload_current_row_id_instead_of_last_row() {
    let mut model = ready_model();
    model.open_entry_tree_loading();
    model.apply_entry_tree_payload(SessionTreePayload {
        rows: (0..6).map(numbered_tree_row).collect(),
        current_row_id: Some("row-2".to_string()),
    });

    let rows = rendered_rows(&render_model_buffer(&mut model, 60, 8));

    assert!(
        rows[0].starts_with("  Session Tree (3 of 6)"),
        "tree should select the payload current row, not the last row: {rows:?}"
    );
    assert!(
        rows[2].contains("message 0")
            && rows[3].contains("message 1")
            && rows[4].contains("message 2"),
        "current row on the first page should keep the first page visible: {rows:?}"
    );
}

#[test]
fn entry_tree_empty_payload_renders_empty_state_not_loading() {
    let mut model = ready_model();
    model.open_entry_tree_loading();
    model.apply_entry_tree_payload(SessionTreePayload {
        rows: Vec::new(),
        current_row_id: None,
    });

    let rows = rendered_rows(&render_model_buffer(&mut model, 60, 8));

    assert!(
        rows[2].starts_with("  No messages yet"),
        "empty tree payload should render an explicit empty state: {rows:?}"
    );
    assert!(
        rows.iter().all(|row| !row.contains("Loading session tree")),
        "empty tree payload must not keep the loading copy: {rows:?}"
    );
}

#[test]
fn late_loaded_entry_tree_payload_is_ignored_after_initial_load_finishes() {
    let mut model = ready_model();
    model.open_entry_tree_loading();
    model.apply_entry_tree_payload(SessionTreePayload {
        rows: vec![tree_row(
            "current-user",
            SessionTreeRowKind::User,
            "current user",
            Some("current user".to_string()),
            Some("current-user"),
        )],
        current_row_id: Some("current-user".to_string()),
    });

    model.apply_runtime_event(RuntimeEvent::SessionTreeLoaded {
        request_id: SessionLoadRequestId::new(1),
        payload: SessionTreePayload {
            rows: vec![tree_row(
                "late-user",
                SessionTreeRowKind::User,
                "late user",
                Some("late user".to_string()),
                Some("late-user"),
            )],
            current_row_id: Some("late-user".to_string()),
        },
    });

    assert_eq!(
        model.entry_tree_row_ids_for_test(),
        vec!["current-user"],
        "a duplicate or late main-tree payload must not replace the already interactive tree"
    );
}

#[test]
fn stale_entry_tree_payload_is_ignored_after_tree_reopens_loading() {
    let mut model = ready_model();

    let stale_request_id = model.open_entry_tree_loading();
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Esc)));
    let current_request_id = model.open_entry_tree_loading();

    model.apply_runtime_event(RuntimeEvent::SessionTreeLoaded {
        request_id: stale_request_id,
        payload: SessionTreePayload {
            rows: vec![tree_row(
                "stale-user",
                SessionTreeRowKind::User,
                "stale user",
                Some("stale user".to_string()),
                Some("stale-user"),
            )],
            current_row_id: Some("stale-user".to_string()),
        },
    });

    assert!(model.entry_tree_loading());
    assert_eq!(model.entry_tree_row_ids_for_test(), Vec::<&str>::new());

    model.apply_runtime_event(RuntimeEvent::SessionTreeLoaded {
        request_id: current_request_id,
        payload: SessionTreePayload {
            rows: vec![tree_row(
                "current-user",
                SessionTreeRowKind::User,
                "current user",
                Some("current user".to_string()),
                Some("current-user"),
            )],
            current_row_id: Some("current-user".to_string()),
        },
    });

    assert!(!model.entry_tree_loading());
    assert_eq!(model.entry_tree_row_ids_for_test(), vec!["current-user"]);
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
        current_row_id: None,
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
        current_row_id: None,
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
        current_row_id: None,
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
        current_row_id: None,
    });

    let palette = *model.palette();
    let buffer = render_model_buffer(&mut model, 60, 8);
    let user_row = ENTRY_TREE_HEADER_HEIGHT + ENTRY_TREE_HEADER_RULE_HEIGHT;
    let reason_row = user_row + 1;
    let tool_row = user_row + 2;
    let selected_assistant_row = user_row + 3;

    let user_content_column = column_of_text(&buffer, user_row, "user body");
    let user_label_column = column_of_text(&buffer, user_row, "user");
    assert_eq!(
        buffer[(user_label_column, user_row)].fg,
        command_accent_text_style(palette)
            .fg
            .expect("default palette should provide command accent"),
        "user kind label should use the same foreground color as user content"
    );
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
        tool_label_column,
        user_label_column + 2,
        "tool kind label should be centered inside the fixed kind column"
    );
    assert_eq!(
        buffer[(tool_label_column, tool_row)].bg,
        Color::Reset,
        "tool kind label should not use a background accent"
    );
    assert_eq!(
        buffer[(tool_label_column, tool_row)].fg,
        muted_text_style(palette)
            .fg
            .expect("default palette should provide muted color"),
        "tool kind label should use the same foreground color as tool content"
    );

    let tool_content_column = column_of_text(&buffer, tool_row, "tool body");
    assert_eq!(
        tool_content_column, user_content_column,
        "centering the tool kind label must not shift the message content column"
    );
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
    let assistant_label_column = column_of_text(&buffer, selected_assistant_row, "assistant");
    assert_eq!(
        buffer[(assistant_label_column, selected_assistant_row)].fg,
        primary_text_style(palette)
            .fg
            .expect("default palette should provide primary text color"),
        "assistant kind label should use the same foreground color as assistant content"
    );
    assert_eq!(
        buffer[(assistant_label_column, selected_assistant_row)].bg,
        Color::Reset,
        "assistant kind label should not use a background color"
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
    let graph_prefix_column = column_of_text(&buffer, selected_assistant_row, "●")
        .min(column_of_text(&buffer, selected_assistant_row, "assistant"));
    assert!(
        !buffer[(graph_prefix_column, selected_assistant_row)]
            .modifier
            .contains(Modifier::REVERSED),
        "selected row graph prefix should not be reversed"
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
fn entry_tree_aligns_kind_column_without_path_left_indent() {
    let mut model = ready_model();
    model.open_entry_tree_loading();
    model.apply_entry_tree_payload(SessionTreePayload {
        rows: vec![
            tree_row_with_branch_choices(
                "user-a",
                SessionTreeRowKind::User,
                "root question",
                vec![
                    branch_choice("assistant-b", "assistant-b", "inactive answer", false),
                    branch_choice("assistant-c", "user-d", "active answer", true),
                ],
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
        current_row_id: Some("user-d".to_string()),
    });

    let buffer = render_model_buffer(&mut model, 72, 8);
    let body_top = ENTRY_TREE_HEADER_HEIGHT + ENTRY_TREE_HEADER_RULE_HEIGHT;
    let root_kind_column = column_of_text(&buffer, body_top, "user");
    let branch_kind_column = column_of_text(&buffer, body_top + 1, "assistant");
    let tail_kind_column = column_of_text(&buffer, body_top + 2, "user");
    let branch_prefix = rendered_prefix_before(&buffer, body_top + 1, "assistant");

    assert_eq!(
        root_kind_column, branch_kind_column,
        "path-only branch child should not be pushed right by branch depth"
    );
    assert_eq!(
        branch_kind_column, tail_kind_column,
        "linear descendants should keep the same kind column in path-only view"
    );
    assert!(
        branch_prefix.contains('·') && !branch_prefix.contains("╰─"),
        "fixed graph prefix should render a plain branch node without bend chrome: {branch_prefix:?}"
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
        current_row_id: None,
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
        tertiary_text_style(palette)
            .fg
            .expect("default palette should provide tertiary color"),
        "rows after the selected item but inside the same visible branch should use weak graph color"
    );
}

#[test]
fn entry_tree_keeps_sub_sub_branch_on_fixed_kind_column_with_skipped_nested_siblings() {
    let mut model = ready_model();
    model.set_window(110, 40);
    model.open_entry_tree_loading();
    model.apply_entry_tree_payload(SessionTreePayload {
        rows: vec![
            tree_row_with_parent_at_depth(
                "user-root",
                None,
                SessionTreeRowKind::User,
                "root",
                0,
                true,
                false,
            ),
            tree_row_with_parent_at_depth(
                "reason-root",
                Some("user-root"),
                SessionTreeRowKind::Reasoning,
                "think root",
                0,
                true,
                false,
            ),
            tree_row_with_parent_at_depth(
                "assistant-root",
                Some("reason-root"),
                SessionTreeRowKind::Assistant,
                "fork parent",
                0,
                true,
                false,
            ),
            tree_row_with_parent_at_depth(
                "user-a",
                Some("assistant-root"),
                SessionTreeRowKind::User,
                "outer branch",
                1,
                true,
                false,
            ),
            tree_row_with_parent_at_depth(
                "reason-a",
                Some("user-a"),
                SessionTreeRowKind::Reasoning,
                "think outer",
                1,
                true,
                false,
            ),
            tree_row_with_parent_at_depth(
                "assistant-a",
                Some("reason-a"),
                SessionTreeRowKind::Assistant,
                "outer tail",
                1,
                true,
                false,
            ),
            tree_row_with_parent_at_depth(
                "assistant-b",
                Some("assistant-a"),
                SessionTreeRowKind::Assistant,
                "nested inactive 1",
                2,
                false,
                false,
            ),
            tree_row_with_parent_at_depth(
                "user-b",
                Some("assistant-b"),
                SessionTreeRowKind::User,
                "nested inactive user 1",
                2,
                false,
                false,
            ),
            tree_row_with_parent_at_depth(
                "reason-b",
                Some("user-b"),
                SessionTreeRowKind::Reasoning,
                "think nested 1",
                2,
                false,
                false,
            ),
            tree_row_with_parent_at_depth(
                "assistant-b1",
                Some("reason-b"),
                SessionTreeRowKind::Assistant,
                "nested tail 1",
                2,
                false,
                false,
            ),
            tree_row_with_parent_at_depth(
                "assistant-c",
                Some("assistant-a"),
                SessionTreeRowKind::Assistant,
                "nested inactive 2",
                2,
                false,
                false,
            ),
            tree_row_with_parent_at_depth(
                "user-c",
                Some("assistant-c"),
                SessionTreeRowKind::User,
                "nested inactive user 2",
                2,
                false,
                false,
            ),
            tree_row_with_parent_at_depth(
                "reason-c",
                Some("user-c"),
                SessionTreeRowKind::Reasoning,
                "think nested 2",
                2,
                false,
                false,
            ),
            tree_row_with_parent_at_depth(
                "assistant-c1",
                Some("reason-c"),
                SessionTreeRowKind::Assistant,
                "nested tail 2",
                2,
                false,
                false,
            ),
            tree_row_with_parent_at_depth(
                "assistant-d",
                Some("assistant-a"),
                SessionTreeRowKind::Assistant,
                "nested inactive 3",
                2,
                false,
                false,
            ),
            tree_row_with_parent_at_depth(
                "user-d",
                Some("assistant-d"),
                SessionTreeRowKind::User,
                "nested inactive user 3",
                2,
                false,
                false,
            ),
            tree_row_with_parent_at_depth(
                "reason-d",
                Some("user-d"),
                SessionTreeRowKind::Reasoning,
                "think nested 3",
                2,
                false,
                false,
            ),
            tree_row_with_parent_at_depth(
                "assistant-d1",
                Some("reason-d"),
                SessionTreeRowKind::Assistant,
                "nested tail 3",
                2,
                false,
                false,
            ),
            tree_row_with_parent_at_depth(
                "user-e",
                Some("assistant-a"),
                SessionTreeRowKind::User,
                "nested selected branch",
                2,
                true,
                false,
            ),
            tree_row_with_parent_at_depth(
                "reason-e",
                Some("user-e"),
                SessionTreeRowKind::Reasoning,
                "think nested selected",
                2,
                true,
                false,
            ),
            tree_row_with_parent_at_depth(
                "assistant-e",
                Some("reason-e"),
                SessionTreeRowKind::Assistant,
                "nested selected tail",
                2,
                true,
                true,
            ),
        ],
        current_row_id: None,
    });

    let buffer = render_model_buffer(&mut model, 110, 40);
    let body_top = ENTRY_TREE_HEADER_HEIGHT + ENTRY_TREE_HEADER_RULE_HEIGHT;
    let outer_user_column = column_of_text(&buffer, body_top + 3, "user");
    let nested_selected_user_column = column_of_text(&buffer, body_top + 18, "user");
    let nested_selected_prefix = rendered_prefix_before(&buffer, body_top + 18, "user");
    let outer_user_prefix = rendered_prefix_before(&buffer, body_top + 3, "user");

    assert_eq!(
        nested_selected_user_column, outer_user_column,
        "sub-sub-branch user row should keep the same kind column as the outer branch user row"
    );
    assert!(
        nested_selected_prefix.contains('·') && !nested_selected_prefix.contains('╰'),
        "nested selected branch should render as a plain branch node: {nested_selected_prefix:?}"
    );
    assert!(
        !outer_user_prefix.contains('╰'),
        "outer branch should not gain fork padding in the default path-only tree: {outer_user_prefix:?}"
    );
}

#[test]
fn entry_tree_renders_fixed_header_rules_footer_and_one_line_rows() {
    let mut model = ready_model();
    model.open_entry_tree_loading();
    model.apply_entry_tree_payload(SessionTreePayload {
        rows: (0..3).map(numbered_tree_row).collect(),
        current_row_id: None,
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
        current_row_id: None,
    });

    let effect = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Esc)));

    assert_eq!(effect, None);
    assert!(!model.entry_tree_active());
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
    ready_model_with_options(ModelOptions::default())
}

fn assert_open_branch_preview_effect(
    model: &Model,
    effect: Option<AppEffect>,
    expected_branch_row_id: &str,
    message: &str,
) {
    let Some(AppEffect::OpenBranchPreview {
        request_id,
        branch_row_id,
    }) = effect
    else {
        panic!("{message}: expected branch preview effect");
    };
    assert_eq!(branch_row_id, expected_branch_row_id, "{message}");
    assert_eq!(
        model.entry_tree_branch_preview_pending_request_id_for_test(),
        Some(request_id),
        "{message}: effect request id must match active preview state"
    );
}

fn ready_model_with_options(options: ModelOptions) -> Model {
    let mut model = Model::new_with_options(StartupBannerOptions::default(), options);
    model.set_window(60, 8);
    model.set_palette(default_palette(), true);
    model
}
