use super::*;

#[test]
fn entry_tree_body_rows_use_two_cell_padding_and_one_cell_graph_gap() {
    let mut model = ready_model();
    model.open_entry_tree_loading();
    model.apply_entry_tree_payload(SessionTreePayload {
        rows: vec![tree_row(
            "user-a",
            SessionTreeRowKind::User,
            "this is a deliberately long tree row summary that should be clipped",
            Some("this is a deliberately long tree row summary that should be clipped".to_string()),
            Some("user-a"),
        )],
        current_row_id: Some("user-a".to_string()),
    });

    let buffer = render_model_buffer(&mut model, 32, 6);
    let body_top = ENTRY_TREE_HEADER_HEIGHT + ENTRY_TREE_HEADER_RULE_HEIGHT;
    let graph_column = column_of_text(&buffer, body_top, "●");
    let kind_column = column_of_text(&buffer, body_top, "user");

    assert_eq!(graph_column, 2, "tree body should keep 2-cell left padding");
    assert_eq!(
        kind_column,
        graph_column + 2,
        "graph symbol and kind column should be separated by exactly one cell"
    );
    assert_eq!(
        buffer[(30, body_top)].symbol(),
        " ",
        "tree body should reserve the first right-padding cell"
    );
    assert_eq!(
        buffer[(31, body_top)].symbol(),
        " ",
        "tree body should reserve the second right-padding cell"
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
        current_row_id: None,
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
    let inactive_branch_prefix = rendered_prefix_before(&buffer, body_top + 1, "assistant");
    let active_branch_prefix = rendered_prefix_before(&buffer, body_top + 2, "assistant");
    assert!(
        inactive_branch_prefix.contains('│')
            && !inactive_branch_prefix.contains('○')
            && !inactive_branch_prefix.contains("├─")
            && !inactive_branch_prefix.contains("╰─"),
        "skipped sibling rows should keep the parent lane continuous without drawing branch symbols: {rows:?}"
    );
    assert!(
        active_branch_prefix.contains('·')
            && !active_branch_prefix.contains('╰')
            && !active_branch_prefix.contains('├')
            && rows[usize::from(body_top + 2)].contains("active answer"),
        "a later selected sibling should render as a branch node without bend chrome: {rows:?}"
    );
    assert_eq!(
        column_of_text(&buffer, body_top, "@"),
        column_of_text(&buffer, body_top + 1, "│"),
        "parent lane should continue through skipped siblings directly below @"
    );
    assert_eq!(
        column_of_text(&buffer, body_top, "@"),
        column_of_text(&buffer, body_top + 2, "·"),
        "selected sibling branch node should align with the parent lane column as @"
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
    let branch_parent_column = column_of_text(&buffer, body_top, "@");
    assert_eq!(
        buffer[(branch_parent_column, body_top)].fg,
        accent_text_style(palette)
            .fg
            .expect("default palette should provide accent color"),
        "@ should keep the accent color as the key fork node"
    );
    assert_eq!(
        buffer[(current_graph_column, body_top + 3)].fg,
        tertiary_text_style(palette)
            .fg
            .expect("default palette should provide tertiary color"),
        "non-key graph nodes should use the weak graph color"
    );
    assert!(
        !buffer[(current_graph_column, body_top + 3)]
            .modifier
            .contains(Modifier::REVERSED),
        "selected row reverse video should not apply to the graph prefix"
    );
}

#[test]
fn entry_tree_marks_path_only_fork_parent_with_at() {
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
                    branch_choice("assistant-c", "user-d", "current follow up", true),
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
                1,
                true,
                true,
            ),
        ],
        current_row_id: Some("user-d".to_string()),
    });

    let buffer = render_model_buffer(&mut model, 72, 8);
    let rows = rendered_rows(&buffer);
    let body_top = ENTRY_TREE_HEADER_HEIGHT + ENTRY_TREE_HEADER_RULE_HEIGHT;

    assert!(
        rendered_prefix_before(&buffer, body_top, "user").contains('@'),
        "path-only fork parent should still render @ using branch metadata: {rows:?}"
    );
    assert!(
        rendered_prefix_before(&buffer, body_top + 1, "assistant").contains('·')
            && !rendered_prefix_before(&buffer, body_top + 1, "assistant").contains('╰'),
        "visible branch child should use a plain branch node in path-only mode: {rows:?}"
    );
}

#[test]
fn entry_tree_aligns_branch_lane_columns_across_rows() {
    let mut model = ready_model();
    model.open_entry_tree_loading();
    model.apply_entry_tree_payload(SessionTreePayload {
        rows: vec![
            tree_row_with_parent_at_depth(
                "user-a",
                None,
                SessionTreeRowKind::User,
                "root",
                0,
                true,
                false,
            ),
            tree_row_with_parent_at_depth(
                "assistant-b",
                Some("user-a"),
                SessionTreeRowKind::Assistant,
                "inactive",
                1,
                false,
                false,
            ),
            tree_row_with_parent_at_depth(
                "assistant-c",
                Some("user-a"),
                SessionTreeRowKind::Assistant,
                "active branch",
                1,
                true,
                false,
            ),
            tree_row_with_parent_at_depth(
                "user-d",
                Some("assistant-c"),
                SessionTreeRowKind::User,
                "branch user",
                1,
                true,
                false,
            ),
            tree_row_with_parent_at_depth(
                "reason-d",
                Some("user-d"),
                SessionTreeRowKind::Reasoning,
                "think",
                1,
                true,
                false,
            ),
            tree_row_with_parent_at_depth(
                "assistant-e",
                Some("reason-d"),
                SessionTreeRowKind::Assistant,
                "branch answer",
                1,
                true,
                true,
            ),
        ],
        current_row_id: None,
    });

    let buffer = render_model_buffer(&mut model, 96, 10);
    let body_top = ENTRY_TREE_HEADER_HEIGHT + ENTRY_TREE_HEADER_RULE_HEIGHT;
    let branch_lane_column = column_of_text(&buffer, body_top + 4, "│");
    let branch_user_marker_column = column_of_text(&buffer, body_top + 3, "·");

    assert_eq!(
        branch_user_marker_column, branch_lane_column,
        "branch fork and continuation rows should share the same graph column"
    );
    let branch_pipe_count = rendered_prefix_before(&buffer, body_top + 4, "reasoning")
        .chars()
        .filter(|ch| *ch == '│')
        .count();
    assert_eq!(
        branch_pipe_count, 1,
        "each branch row should draw exactly one vertical lane character"
    );
    let root_kind_column = column_of_text(&buffer, body_top, "user");
    let branch_kind_column = column_of_text(&buffer, body_top + 3, "user");
    assert_eq!(
        root_kind_column, branch_kind_column,
        "branch rows should share the root kind column in path-only view"
    );
    assert!(
        !rendered_prefix_before(&buffer, body_top, "user").contains('❯'),
        "tree rows should not render a separate selection marker column"
    );
}

#[test]
fn entry_tree_keeps_same_lane_depth_within_linear_branch_chain() {
    let mut model = ready_model();
    model.open_entry_tree_loading();
    model.apply_entry_tree_payload(SessionTreePayload {
        rows: vec![
            tree_row_with_parent_at_depth(
                "user-a",
                None,
                SessionTreeRowKind::User,
                "root",
                0,
                true,
                false,
            ),
            tree_row_with_parent_at_depth(
                "assistant-b",
                Some("user-a"),
                SessionTreeRowKind::Assistant,
                "inactive",
                1,
                false,
                false,
            ),
            tree_row_with_parent_at_depth(
                "assistant-c",
                Some("user-a"),
                SessionTreeRowKind::Assistant,
                "active branch",
                1,
                true,
                false,
            ),
            tree_row_with_parent_at_depth(
                "user-d",
                Some("assistant-c"),
                SessionTreeRowKind::User,
                "branch follow up",
                1,
                true,
                false,
            ),
            tree_row_with_parent_at_depth(
                "reason-d",
                Some("user-d"),
                SessionTreeRowKind::Reasoning,
                "thinking",
                1,
                true,
                false,
            ),
            tree_row_with_parent_at_depth(
                "assistant-e",
                Some("reason-d"),
                SessionTreeRowKind::Assistant,
                "branch tail",
                1,
                true,
                true,
            ),
        ],
        current_row_id: None,
    });

    let buffer = render_model_buffer(&mut model, 96, 10);
    let body_top = ENTRY_TREE_HEADER_HEIGHT + ENTRY_TREE_HEADER_RULE_HEIGHT;
    let root_kind_column = column_of_text(&buffer, body_top, "user");
    let branch_fork_kind = column_of_text(&buffer, body_top + 2, "assistant");
    let branch_user_kind = column_of_text(&buffer, body_top + 3, "user");
    let branch_reasoning_kind = column_of_text(&buffer, body_top + 4, "reasoning");
    let branch_tail_kind = column_of_text(&buffer, body_top + 5, "assistant");

    assert_eq!(
        root_kind_column, branch_fork_kind,
        "forked branch should keep the same kind column as the trunk"
    );
    assert_eq!(
        branch_user_kind, branch_reasoning_kind,
        "reasoning inside a branch should not shift the graph column"
    );
    assert_eq!(
        branch_reasoning_kind, branch_tail_kind,
        "linear branch descendants should stay on the same lane column"
    );
}

#[test]
fn entry_tree_uses_fixed_graph_prefix_without_depth_gutter() {
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
        current_row_id: None,
    });

    let buffer = render_model_buffer(&mut model, 72, 7);
    let body_top = ENTRY_TREE_HEADER_HEIGHT + ENTRY_TREE_HEADER_RULE_HEIGHT;
    let root_kind_column = column_of_text(&buffer, body_top, "user");
    let branch_kind_column = column_of_text(&buffer, body_top + 1, "assistant");

    assert_eq!(
        root_kind_column, branch_kind_column,
        "branch rows should not indent relative to the root trunk"
    );
}

#[test]
fn entry_tree_keeps_nested_branch_on_fixed_kind_column_with_reasoning() {
    let mut model = ready_model();
    model.open_entry_tree_loading();
    model.apply_entry_tree_payload(SessionTreePayload {
        rows: vec![
            tree_row_with_parent_at_depth(
                "user-a",
                None,
                SessionTreeRowKind::User,
                "root",
                0,
                true,
                false,
            ),
            tree_row_with_parent_at_depth(
                "reason-a",
                Some("user-a"),
                SessionTreeRowKind::Reasoning,
                "think root",
                0,
                true,
                false,
            ),
            tree_row_with_parent_at_depth(
                "assistant-b",
                Some("reason-a"),
                SessionTreeRowKind::Assistant,
                "fork parent",
                0,
                true,
                false,
            ),
            tree_row_with_parent_at_depth(
                "user-c",
                Some("assistant-b"),
                SessionTreeRowKind::User,
                "skipped branch",
                1,
                false,
                false,
            ),
            tree_row_with_parent_at_depth(
                "user-d",
                Some("assistant-b"),
                SessionTreeRowKind::User,
                "outer branch",
                1,
                true,
                false,
            ),
            tree_row_with_parent_at_depth(
                "reason-d",
                Some("user-d"),
                SessionTreeRowKind::Reasoning,
                "think outer",
                1,
                true,
                false,
            ),
            tree_row_with_parent_at_depth(
                "assistant-e",
                Some("reason-d"),
                SessionTreeRowKind::Assistant,
                "nested fork parent",
                1,
                true,
                false,
            ),
            tree_row_with_parent_at_depth(
                "assistant-f",
                Some("assistant-e"),
                SessionTreeRowKind::Assistant,
                "nested inactive",
                2,
                false,
                false,
            ),
            tree_row_with_parent_at_depth(
                "assistant-g",
                Some("assistant-e"),
                SessionTreeRowKind::Assistant,
                "nested selected",
                2,
                true,
                false,
            ),
            tree_row_with_parent_at_depth(
                "user-h",
                Some("assistant-g"),
                SessionTreeRowKind::User,
                "nested tail",
                2,
                true,
                true,
            ),
        ],
        current_row_id: None,
    });

    let buffer = render_model_buffer(&mut model, 110, 16);
    let body_top = ENTRY_TREE_HEADER_HEIGHT + ENTRY_TREE_HEADER_RULE_HEIGHT;
    let outer_user_column = column_of_text(&buffer, body_top + 4, "user");
    let nested_fork_column = column_of_text(&buffer, body_top + 8, "·");
    let nested_parent_column = column_of_text(&buffer, body_top + 6, "@");
    let nested_tail_column = column_of_text(&buffer, body_top + 9, "user");
    let nested_selected_prefix = rendered_prefix_before(&buffer, body_top + 8, "assistant");
    let skipped_nested_prefix = rendered_prefix_before(&buffer, body_top + 7, "assistant");

    assert_eq!(
        nested_tail_column, outer_user_column,
        "nested branch tail should keep the same kind column as the outer branch user row"
    );
    assert_eq!(
        nested_parent_column, nested_fork_column,
        "nested fork node should align with the nested @ parent column"
    );
    assert!(
        nested_selected_prefix.contains('·') && !nested_selected_prefix.contains('╰'),
        "nested selected sibling should render as a plain branch node: {nested_selected_prefix:?}"
    );
    assert!(
        skipped_nested_prefix.contains('│') && !skipped_nested_prefix.contains('╰'),
        "skipped nested sibling should keep inner fork lanes continuous without its own connector: {skipped_nested_prefix:?}"
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
        current_row_id: None,
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
        selected_tail_prefix.contains("●")
            && !selected_tail_prefix.contains("├─")
            && !selected_tail_prefix.contains("╰─"),
        "selected branch tail should render as the selected node without extra depth chrome: {rows:?}"
    );

    let outer_lane_column = column_of_text(&buffer, body_top + 4, "│");
    let nested_tail_kind_column = column_of_text(&buffer, body_top + 6, "user");
    let outer_branch_kind_column = column_of_text(&buffer, body_top + 3, "user");
    assert_eq!(
        nested_tail_kind_column, outer_branch_kind_column,
        "nested branch tail should keep the same kind column as the outer branch"
    );
    assert_eq!(
        buffer[(outer_lane_column, body_top + 4)].fg,
        tertiary_text_style(palette)
            .fg
            .expect("default palette should provide tertiary color"),
        "the lane that keeps the branch connected should use the weak graph color"
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
        current_row_id: None,
    });

    let buffer = render_model_buffer(&mut model, 84, 9);
    let rows = rendered_rows(&buffer);
    let body_top = ENTRY_TREE_HEADER_HEIGHT + ENTRY_TREE_HEADER_RULE_HEIGHT;

    assert!(
        rendered_prefix_before(&buffer, body_top + 2, "assistant").contains("·"),
        "selected branch start at a fork should use the middle-path marker, not a branch endpoint: {rows:?}"
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
fn entry_tree_hides_inactive_path_lanes_when_sibling_branch_is_selected() {
    let mut model = ready_model();
    model.open_entry_tree_loading();
    model.apply_entry_tree_payload(SessionTreePayload {
        rows: vec![
            tree_row_with_parent_at_depth(
                "user-a",
                None,
                SessionTreeRowKind::User,
                "hello",
                0,
                true,
                false,
            ),
            tree_row_with_parent_at_depth(
                "assistant-b",
                Some("user-a"),
                SessionTreeRowKind::Assistant,
                "answer",
                0,
                true,
                false,
            ),
            tree_row_with_parent_at_depth(
                "user-c",
                Some("assistant-b"),
                SessionTreeRowKind::User,
                "branch question",
                1,
                true,
                false,
            ),
            tree_row_with_parent_at_depth(
                "assistant-d",
                Some("user-c"),
                SessionTreeRowKind::Assistant,
                "branch answer",
                1,
                true,
                false,
            ),
            tree_row_with_parent_at_depth(
                "user-e",
                Some("assistant-b"),
                SessionTreeRowKind::User,
                "active follow up",
                1,
                true,
                true,
            ),
            tree_row_with_parent_at_depth(
                "reason-e",
                Some("user-e"),
                SessionTreeRowKind::Reasoning,
                "thinking",
                1,
                true,
                true,
            ),
            tree_row_with_parent_at_depth(
                "assistant-f",
                Some("reason-e"),
                SessionTreeRowKind::Assistant,
                "active answer",
                1,
                true,
                true,
            ),
        ],
        current_row_id: None,
    });
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Up)));
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Up)));
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Up)));

    let buffer = render_model_buffer(&mut model, 96, 11);
    let body_top = ENTRY_TREE_HEADER_HEIGHT + ENTRY_TREE_HEADER_RULE_HEIGHT;
    let inactive_follow_up_prefix = rendered_prefix_before(&buffer, body_top + 4, "user");
    let inactive_reasoning_prefix = rendered_prefix_before(&buffer, body_top + 5, "reasoning");
    let selected_branch_lane_prefix = rendered_prefix_before(&buffer, body_top + 3, "assistant");

    assert!(
        rendered_prefix_before(&buffer, body_top + 1, "assistant").contains('@'),
        "fork parent should use @ at the branch point"
    );
    assert!(
        rendered_prefix_before(&buffer, body_top + 2, "user").contains('·')
            && !rendered_prefix_before(&buffer, body_top + 2, "user").contains('●'),
        "fork child should not use a branch endpoint marker"
    );
    assert!(
        !inactive_follow_up_prefix.contains('│') && !inactive_reasoning_prefix.contains('│'),
        "inactive path rows outside the selected branch must not draw lane lines: \
         follow_up={inactive_follow_up_prefix:?}, reasoning={inactive_reasoning_prefix:?}"
    );
    assert!(
        selected_branch_lane_prefix.contains('│') || selected_branch_lane_prefix.contains('●'),
        "selected branch rows should still keep lane continuity: {selected_branch_lane_prefix:?}"
    );
}

#[test]
fn entry_tree_keeps_parent_lane_continuous_across_skipped_sibling_branches() {
    let mut model = ready_model();
    model.open_entry_tree_loading();
    model.apply_entry_tree_payload(SessionTreePayload {
        rows: vec![
            tree_row_with_parent_at_depth(
                "user-a",
                None,
                SessionTreeRowKind::User,
                "root",
                0,
                true,
                false,
            ),
            tree_row_with_parent_at_depth(
                "reason-a",
                Some("user-a"),
                SessionTreeRowKind::Reasoning,
                "think root",
                0,
                true,
                false,
            ),
            tree_row_with_parent_at_depth(
                "assistant-b",
                Some("reason-a"),
                SessionTreeRowKind::Assistant,
                "fork parent",
                0,
                true,
                false,
            ),
            tree_row_with_parent_at_depth(
                "user-c",
                Some("assistant-b"),
                SessionTreeRowKind::User,
                "skipped branch",
                1,
                false,
                false,
            ),
            tree_row_with_parent_at_depth(
                "reason-c",
                Some("user-c"),
                SessionTreeRowKind::Reasoning,
                "think skipped",
                1,
                false,
                false,
            ),
            tree_row_with_parent_at_depth(
                "assistant-d",
                Some("reason-c"),
                SessionTreeRowKind::Assistant,
                "skipped tail",
                1,
                false,
                false,
            ),
            tree_row_with_parent_at_depth(
                "user-e",
                Some("assistant-b"),
                SessionTreeRowKind::User,
                "selected branch",
                1,
                true,
                false,
            ),
            tree_row_with_parent_at_depth(
                "reason-e",
                Some("user-e"),
                SessionTreeRowKind::Reasoning,
                "think selected",
                1,
                true,
                false,
            ),
            tree_row_with_parent_at_depth(
                "assistant-f",
                Some("reason-e"),
                SessionTreeRowKind::Assistant,
                "selected tail",
                1,
                true,
                true,
            ),
        ],
        current_row_id: None,
    });

    let buffer = render_model_buffer(&mut model, 96, 14);
    let body_top = ENTRY_TREE_HEADER_HEIGHT + ENTRY_TREE_HEADER_RULE_HEIGHT;
    let fork_column = column_of_text(&buffer, body_top + 2, "@");
    let skipped_branch_prefixes = [
        rendered_prefix_before(&buffer, body_top + 3, "user"),
        rendered_prefix_before(&buffer, body_top + 4, "reasoning"),
        rendered_prefix_before(&buffer, body_top + 5, "assistant"),
    ];
    let selected_branch_prefix = rendered_prefix_before(&buffer, body_top + 6, "user");

    for (index, prefix) in skipped_branch_prefixes.iter().enumerate() {
        assert!(
            prefix.contains('│') && !prefix.contains('├') && !prefix.contains('╰'),
            "skipped sibling branch row {index} should keep the parent lane continuous: {prefix:?}"
        );
        assert_eq!(
            column_of_text(&buffer, body_top + 3 + index as u16, "│"),
            fork_column,
            "parent lane should stay aligned with @ through skipped rows"
        );
    }
    assert!(
        selected_branch_prefix.contains('·')
            && !selected_branch_prefix.contains('╰')
            && !selected_branch_prefix.contains('├'),
        "selected sibling branch after a skipped branch should render as a plain branch node: {selected_branch_prefix:?}"
    );
    assert_eq!(
        fork_column,
        column_of_text(&buffer, body_top + 6, "·"),
        "selected branch node should align with the parent lane below @"
    );
}

#[test]
fn entry_tree_keeps_repeated_rewinds_on_fixed_kind_column_through_hidden_config_chain() {
    // 模拟用户在同一个 root assistant 上连续 rewind 4 次，每次 rewind 都把新的
    // ConfigChange 挂到上一次 ConfigChange 上（隐藏的 fork 链）。session-store 此时会把
    // 第 N 个 sub-branch 的 display_depth 算成 N（最后一次因为后面还没接新 ConfigChange，
    // 与上一个并列）；path-only tree 只保留固定 graph 前缀，不再按 display_depth 推右。
    let mut model = ready_model();
    model.set_window(120, 40);
    model.open_entry_tree_loading();
    model.apply_entry_tree_payload(SessionTreePayload {
        rows: vec![
            tree_row_with_parent_at_depth(
                "user-root",
                None,
                SessionTreeRowKind::User,
                "root question",
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
                "root reply",
                0,
                true,
                false,
            ),
            tree_row_with_parent_at_depth(
                "user-branch-1",
                Some("assistant-root"),
                SessionTreeRowKind::User,
                "branch 1 user",
                1,
                true,
                false,
            ),
            tree_row_with_parent_at_depth(
                "reason-branch-1",
                Some("user-branch-1"),
                SessionTreeRowKind::Reasoning,
                "think 1",
                1,
                true,
                false,
            ),
            tree_row_with_parent_at_depth(
                "assistant-branch-1",
                Some("reason-branch-1"),
                SessionTreeRowKind::Assistant,
                "branch 1 reply",
                1,
                true,
                false,
            ),
            tree_row_with_parent_at_depth(
                "user-branch-2",
                Some("assistant-root"),
                SessionTreeRowKind::User,
                "branch 2 user",
                2,
                false,
                false,
            ),
            tree_row_with_parent_at_depth(
                "reason-branch-2",
                Some("user-branch-2"),
                SessionTreeRowKind::Reasoning,
                "think 2",
                2,
                false,
                false,
            ),
            tree_row_with_parent_at_depth(
                "assistant-branch-2",
                Some("reason-branch-2"),
                SessionTreeRowKind::Assistant,
                "branch 2 reply",
                2,
                false,
                false,
            ),
            tree_row_with_parent_at_depth(
                "user-branch-3",
                Some("assistant-root"),
                SessionTreeRowKind::User,
                "branch 3 user",
                3,
                false,
                false,
            ),
            tree_row_with_parent_at_depth(
                "reason-branch-3",
                Some("user-branch-3"),
                SessionTreeRowKind::Reasoning,
                "think 3",
                3,
                false,
                false,
            ),
            tree_row_with_parent_at_depth(
                "assistant-branch-3",
                Some("reason-branch-3"),
                SessionTreeRowKind::Assistant,
                "branch 3 reply",
                3,
                false,
                false,
            ),
            tree_row_with_parent_at_depth(
                "user-branch-4",
                Some("assistant-root"),
                SessionTreeRowKind::User,
                "branch 4 user",
                3,
                false,
                true,
            ),
            tree_row_with_parent_at_depth(
                "reason-branch-4",
                Some("user-branch-4"),
                SessionTreeRowKind::Reasoning,
                "think 4",
                3,
                false,
                false,
            ),
            tree_row_with_parent_at_depth(
                "assistant-branch-4",
                Some("reason-branch-4"),
                SessionTreeRowKind::Assistant,
                "branch 4 reply",
                3,
                false,
                false,
            ),
        ],
        current_row_id: None,
    });

    let buffer = render_model_buffer(&mut model, 120, 40);
    let body_top = ENTRY_TREE_HEADER_HEIGHT + ENTRY_TREE_HEADER_RULE_HEIGHT;

    let root_user_column = column_of_text(&buffer, body_top, "user");
    let branch_1_user_column = column_of_text(&buffer, body_top + 3, "user");
    let branch_2_user_column = column_of_text(&buffer, body_top + 6, "user");
    let branch_3_user_column = column_of_text(&buffer, body_top + 9, "user");
    let branch_4_user_column = column_of_text(&buffer, body_top + 12, "user");

    assert_eq!(
        branch_1_user_column, root_user_column,
        "first rewind branch should keep the root kind column"
    );
    assert_eq!(
        branch_2_user_column, branch_1_user_column,
        "second rewind branch should not add horizontal depth"
    );
    assert_eq!(
        branch_3_user_column, branch_2_user_column,
        "third rewind branch should not add horizontal depth"
    );
    assert_eq!(
        branch_4_user_column, branch_3_user_column,
        "fourth rewind branch should keep the same fixed kind column"
    );

    let branch_4_prefix = rendered_prefix_before(&buffer, body_top + 12, "user");
    assert!(
        branch_4_prefix.contains('·') && !branch_4_prefix.contains('╰'),
        "selected branch 4 should render as a plain branch node: {branch_4_prefix:?}"
    );
}

#[test]
fn entry_tree_keeps_rewinded_user_branch_on_fixed_kind_column_under_outer_assistant() {
    let mut model = ready_model();
    model.set_window(110, 40);
    model.open_entry_tree_loading();
    model.apply_entry_tree_payload(SessionTreePayload {
        rows: vec![
            tree_row_with_parent_at_depth(
                "user-root",
                None,
                SessionTreeRowKind::User,
                "你好哦",
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
                "root reply",
                0,
                true,
                false,
            ),
            tree_row_with_parent_at_depth(
                "user-inactive",
                Some("assistant-root"),
                SessionTreeRowKind::User,
                "skipped branch",
                1,
                false,
                false,
            ),
            tree_row_with_parent_at_depth(
                "user-a",
                Some("assistant-root"),
                SessionTreeRowKind::User,
                "你是谁",
                1,
                true,
                false,
            ),
            tree_row_with_parent_at_depth(
                "reason-a",
                Some("user-a"),
                SessionTreeRowKind::Reasoning,
                "think a",
                1,
                true,
                false,
            ),
            tree_row_with_parent_at_depth(
                "assistant-a",
                Some("reason-a"),
                SessionTreeRowKind::Assistant,
                "你好！",
                1,
                true,
                false,
            ),
            tree_row_with_parent_at_depth(
                "user-b",
                Some("assistant-a"),
                SessionTreeRowKind::User,
                "linear follow up",
                2,
                false,
                false,
            ),
            tree_row_with_parent_at_depth(
                "reason-b",
                Some("user-b"),
                SessionTreeRowKind::Reasoning,
                "think b",
                2,
                false,
                false,
            ),
            tree_row_with_parent_at_depth(
                "assistant-b",
                Some("reason-b"),
                SessionTreeRowKind::Assistant,
                "linear reply",
                2,
                false,
                false,
            ),
            tree_row_with_parent_at_depth(
                "user-c",
                Some("assistant-a"),
                SessionTreeRowKind::User,
                "你能做什么",
                2,
                true,
                false,
            ),
            tree_row_with_parent_at_depth(
                "reason-c",
                Some("user-c"),
                SessionTreeRowKind::Reasoning,
                "think c",
                2,
                true,
                false,
            ),
            tree_row_with_parent_at_depth(
                "assistant-c",
                Some("reason-c"),
                SessionTreeRowKind::Assistant,
                "nested tail",
                2,
                true,
                true,
            ),
        ],
        current_row_id: None,
    });

    let buffer = render_model_buffer(&mut model, 110, 40);
    let body_top = ENTRY_TREE_HEADER_HEIGHT + ENTRY_TREE_HEADER_RULE_HEIGHT;
    let outer_user_column = column_of_text(&buffer, body_top + 4, "user");
    let nested_user_column = column_of_text(&buffer, body_top + 10, "user");
    let nested_prefix = rendered_prefix_before(&buffer, body_top + 10, "user");

    assert_eq!(
        nested_user_column, outer_user_column,
        "rewound user branch under outer assistant should keep the same kind column as the first sub-branch"
    );
    assert!(
        nested_prefix.contains('·') && !nested_prefix.contains('╰'),
        "nested user branch should render as a plain branch node: {nested_prefix:?}"
    );
}

#[test]
fn entry_tree_hides_trunk_continuation_lanes_below_selected_fork_branch() {
    let mut model = ready_model();
    model.open_entry_tree_loading();
    model.apply_entry_tree_payload(SessionTreePayload {
        rows: vec![
            tree_row_with_parent_at_depth(
                "user-a",
                None,
                SessionTreeRowKind::User,
                "root",
                0,
                true,
                false,
            ),
            tree_row_with_parent_at_depth(
                "reason-a",
                Some("user-a"),
                SessionTreeRowKind::Reasoning,
                "think root",
                0,
                true,
                false,
            ),
            tree_row_with_parent_at_depth(
                "assistant-b",
                Some("reason-a"),
                SessionTreeRowKind::Assistant,
                "fork parent",
                0,
                true,
                false,
            ),
            tree_row_with_parent_at_depth(
                "user-c",
                Some("assistant-b"),
                SessionTreeRowKind::User,
                "selected branch",
                1,
                true,
                false,
            ),
            tree_row_with_parent_at_depth(
                "reason-c",
                Some("user-c"),
                SessionTreeRowKind::Reasoning,
                "think branch",
                1,
                true,
                false,
            ),
            tree_row_with_parent_at_depth(
                "assistant-d",
                Some("reason-c"),
                SessionTreeRowKind::Assistant,
                "selected tail",
                1,
                true,
                true,
            ),
            tree_row_with_parent_at_depth(
                "user-e",
                Some("assistant-b"),
                SessionTreeRowKind::User,
                "trunk follow up",
                1,
                true,
                true,
            ),
            tree_row_with_parent_at_depth(
                "reason-e",
                Some("user-e"),
                SessionTreeRowKind::Reasoning,
                "think trunk",
                1,
                true,
                true,
            ),
            tree_row_with_parent_at_depth(
                "assistant-f",
                Some("reason-e"),
                SessionTreeRowKind::Assistant,
                "trunk tail",
                1,
                true,
                true,
            ),
        ],
        current_row_id: None,
    });
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Up)));
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Up)));
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Up)));

    let buffer = render_model_buffer(&mut model, 96, 14);
    let body_top = ENTRY_TREE_HEADER_HEIGHT + ENTRY_TREE_HEADER_RULE_HEIGHT;
    let trunk_follow_up_prefix = rendered_prefix_before(&buffer, body_top + 6, "user");
    let trunk_reasoning_prefix = rendered_prefix_before(&buffer, body_top + 7, "reasoning");
    let selected_branch_reason_prefix = rendered_prefix_before(&buffer, body_top + 4, "reasoning");

    assert!(
        !trunk_follow_up_prefix.contains('│') && !trunk_reasoning_prefix.contains('│'),
        "trunk continuation on the inactive path must not draw lane lines: \
         follow_up={trunk_follow_up_prefix:?}, reasoning={trunk_reasoning_prefix:?}"
    );
    assert!(
        selected_branch_reason_prefix.contains('│'),
        "selected branch should still draw a continuous lane: {selected_branch_reason_prefix:?}"
    );
    assert!(
        rendered_prefix_before(&buffer, body_top + 3, "user").contains('·')
            && !rendered_prefix_before(&buffer, body_top + 3, "user").contains('╰')
            && !rendered_prefix_before(&buffer, body_top + 3, "user").contains('├'),
        "selected fork child should render as a plain branch node when inactive siblings are hidden"
    );
    let fork_parent_column = column_of_text(&buffer, body_top + 2, "@");
    let fork_child_column = column_of_text(&buffer, body_top + 3, "·");
    assert_eq!(
        fork_parent_column, fork_child_column,
        "fork child node should align directly below @ in the parent lane column"
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
            SessionTreeRow {
                rewind_target_id: None,
                ..tree_row_with_parent_at_depth(
                    "tool-c",
                    Some("reason-b"),
                    SessionTreeRowKind::Tool,
                    "output",
                    0,
                    true,
                    false,
                )
            },
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
        current_row_id: None,
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
fn entry_tree_renders_only_rewindable_tool_batch_tail_as_graph_node() {
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
            SessionTreeRow {
                rewind_target_id: None,
                ..tree_row_with_parent_at_depth(
                    "assistant-b",
                    Some("user-a"),
                    SessionTreeRowKind::Assistant,
                    "tool calls",
                    0,
                    true,
                    false,
                )
            },
            SessionTreeRow {
                rewind_target_id: None,
                ..tree_row_with_parent_at_depth(
                    "tool-c",
                    Some("assistant-b"),
                    SessionTreeRowKind::Tool,
                    "first output",
                    0,
                    true,
                    false,
                )
            },
            tree_row_with_parent_at_depth(
                "tool-d",
                Some("tool-c"),
                SessionTreeRowKind::Tool,
                "final output",
                0,
                true,
                false,
            ),
            tree_row_with_parent_at_depth(
                "assistant-e",
                Some("tool-d"),
                SessionTreeRowKind::Assistant,
                "answer",
                0,
                true,
                false,
            ),
        ],
        current_row_id: None,
    });

    let buffer = render_model_buffer(&mut model, 72, 9);
    let rows = rendered_rows(&buffer);
    let body_top = ENTRY_TREE_HEADER_HEIGHT + ENTRY_TREE_HEADER_RULE_HEIGHT;
    let attached_assistant_prefix = rendered_prefix_before(&buffer, body_top + 1, "assistant");
    let intermediate_tool_prefix = rendered_prefix_before(&buffer, body_top + 2, "tool");
    let final_tool_prefix = rendered_prefix_before(&buffer, body_top + 3, "tool");

    assert!(
        attached_assistant_prefix.contains("│")
            && !attached_assistant_prefix.contains("·")
            && !attached_assistant_prefix.contains("●"),
        "assistant tool-call rows should render as attached continuation rows: {rows:?}"
    );
    assert!(
        intermediate_tool_prefix.contains("│")
            && !intermediate_tool_prefix.contains("·")
            && !intermediate_tool_prefix.contains("●"),
        "intermediate tool results should render as attached continuation rows: {rows:?}"
    );
    assert!(
        final_tool_prefix.contains("·") && !final_tool_prefix.contains("●"),
        "the final rewindable tool result should render as a normal rewind node: {rows:?}"
    );
}

#[test]
fn entry_tree_keeps_branch_lane_continuous_through_reasoning() {
    let mut model = ready_model();
    model.open_entry_tree_loading();
    model.apply_entry_tree_payload(SessionTreePayload {
        rows: vec![
            tree_row_with_parent_at_depth(
                "user-a",
                None,
                SessionTreeRowKind::User,
                "root",
                0,
                true,
                false,
            ),
            tree_row_with_parent_at_depth(
                "reason-a",
                Some("user-a"),
                SessionTreeRowKind::Reasoning,
                "think root",
                0,
                true,
                false,
            ),
            tree_row_with_parent_at_depth(
                "assistant-b",
                Some("reason-a"),
                SessionTreeRowKind::Assistant,
                "answer root",
                0,
                true,
                false,
            ),
            tree_row_with_parent_at_depth(
                "assistant-c",
                Some("user-a"),
                SessionTreeRowKind::Assistant,
                "inactive branch",
                1,
                false,
                false,
            ),
            tree_row_with_parent_at_depth(
                "assistant-d",
                Some("user-a"),
                SessionTreeRowKind::Assistant,
                "active branch",
                1,
                true,
                false,
            ),
            tree_row_with_parent_at_depth(
                "user-e",
                Some("assistant-d"),
                SessionTreeRowKind::User,
                "branch user",
                1,
                true,
                false,
            ),
            tree_row_with_parent_at_depth(
                "reason-e",
                Some("user-e"),
                SessionTreeRowKind::Reasoning,
                "think branch",
                1,
                true,
                false,
            ),
            tree_row_with_parent_at_depth(
                "assistant-f",
                Some("reason-e"),
                SessionTreeRowKind::Assistant,
                "branch answer",
                1,
                true,
                true,
            ),
        ],
        current_row_id: None,
    });

    let palette = *model.palette();
    let buffer = render_model_buffer(&mut model, 96, 12);
    let body_top = ENTRY_TREE_HEADER_HEIGHT + ENTRY_TREE_HEADER_RULE_HEIGHT;
    let root_user_prefix = rendered_prefix_before(&buffer, body_top, "user");
    let root_reasoning_prefix = rendered_prefix_before(&buffer, body_top + 1, "reasoning");
    let branch_reasoning_prefix = rendered_prefix_before(&buffer, body_top + 6, "reasoning");
    let branch_assistant_prefix = rendered_prefix_before(&buffer, body_top + 7, "assistant");
    let branch_lane_column = column_of_text(&buffer, body_top + 6, "│");

    assert!(
        root_user_prefix.contains('@')
            && root_reasoning_prefix.contains('│')
            && !rendered_prefix_before(&buffer, body_top + 2, "assistant").contains("├─"),
        "linear trunk reasoning should continue the root graph lane without branch connectors: \
         user={root_user_prefix:?}, reasoning={root_reasoning_prefix:?}"
    );
    assert!(
        branch_reasoning_prefix.contains('│')
            && branch_assistant_prefix.contains('●')
            && rendered_prefix_before(&buffer, body_top + 5, "user").contains('·'),
        "branch reasoning and assistant should continue the same lane as the branch user: \
         user={:?}, reasoning={branch_reasoning_prefix:?}, assistant={branch_assistant_prefix:?}",
        rendered_prefix_before(&buffer, body_top + 5, "user")
    );
    assert_eq!(
        buffer[(branch_lane_column, body_top + 6)].fg,
        tertiary_text_style(palette)
            .fg
            .expect("default palette should provide tertiary color"),
        "branch lane continuation should use the weak graph color"
    );
}

#[test]
fn entry_tree_keeps_deep_graph_prefix_flat_when_branch_depth_exceeds_budget() {
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

    model.apply_entry_tree_payload(SessionTreePayload {
        rows,
        current_row_id: None,
    });

    let rows = rendered_rows(&render_model_buffer(&mut model, 60, 22));
    let selected_row = rows
        .iter()
        .find(|row| row.contains("deep message 8"))
        .expect("deep current row should be visible");

    assert!(
        !selected_row.contains("…"),
        "deep graph prefixes should stay flat instead of exposing an ellipsis counter: {rows:?}"
    );
    assert!(
        selected_row.contains("deep message 8"),
        "flat graph prefix must preserve message content: {rows:?}"
    );
}
