use super::*;

#[test]
fn render_uses_single_header_row_with_right_aligned_tabs_and_table_headers() {
    let mut model = ready_model();
    model.open_prompt_overlay();

    let rows = rendered_rows(&render_model_buffer(&mut model, 90, 16)).join("\n");

    assert!(rows.contains("Prompt Assembly"));
    assert!(!rows.contains("active ·"));
    assert!(!rows.contains("candidates"));
    assert!(!rows.contains("View:"));
    assert!(!rows.contains("scope=project"));
    assert!(!rows.contains("scope=global"));
    assert!(!rows.contains("Next New Session"));
    assert!(rows.contains("[Skill]"));
    assert!(rows.contains("Custom Prompts"));
    assert!(rows.contains("Sel"));
    assert!(rows.contains("Ord"));
    assert!(rows.contains("Source"));
    assert!(rows.contains("Type"));
    assert!(rows.contains("Scope"));
    assert!(!rows.contains("Num"));
    assert!(rows.contains("●"));
    assert!(!rows.contains("Active Sources"));
    assert!(!rows.contains("Inactive Sources"));
}

#[test]
fn focused_page_label_uses_selection_counts_instead_of_pages() {
    let mut model = ready_model();
    model.set_window(90, 16);
    model.open_prompt_overlay();

    let active_label = {
        let state = model
            .prompt_overlay
            .as_ref()
            .expect("prompt overlay should open");
        model.prompt_overlay_focused_page_label(state, 16)
    };
    assert_eq!(active_label, " Active 1/4 ");

    model.set_prompt_overlay_focus(super::PromptOverlayFocus::Inactive);
    let inactive_skill_label = {
        let state = model
            .prompt_overlay
            .as_ref()
            .expect("prompt overlay should stay open");
        model.prompt_overlay_focused_page_label(state, 16)
    };
    assert_eq!(inactive_skill_label, " 1/2 ");

    let _ = model.handle_prompt_overlay_key(KeyEvent::from(KeyCode::Tab));
    let inactive_custom_label = {
        let state = model
            .prompt_overlay
            .as_ref()
            .expect("prompt overlay should stay open");
        model.prompt_overlay_focused_page_label(state, 16)
    };
    assert_eq!(inactive_custom_label, " 1/1 ");
}

#[test]
fn default_active_list_keeps_disabled_and_missing_sources_visible() {
    let mut model = ready_model();
    model
        .prompt_assembly
        .sources
        .managed
        .push(PromptAssemblyManagedSource {
            reference_id: "missing-skill".to_string(),
            kind: PromptSourceKind::LongLivedSkill,
            title: "missing-skill".to_string(),
            origin: Some(PromptSourceOrigin::Project),
            scope: Some(PromptAssemblyScope::Project),
            enabled: true,
            order: 5,
        });
    model
        .prompt_assembly
        .resolution
        .assembly
        .inactive_sources
        .push(prompt_source(
            "missing-skill",
            "missing-skill",
            PromptSourceKind::LongLivedSkill,
            Some(PromptSourceOrigin::Project),
            PromptSourceStatus::Inactive {
                reason: PromptSourceInactiveReason::Missing,
            },
        ));
    model.set_window(140, 16);
    model.open_prompt_overlay();

    let rows = rendered_rows(&render_model_buffer(&mut model, 140, 16)).join("\n");

    assert!(rows.contains("safety-policy"));
    assert!(rows.contains("missing-skill"));
    assert!(rows.contains("missing"));
}

#[test]
fn disabled_source_row_does_not_repeat_disabled_label_in_effective_view() {
    let mut model = ready_model();
    model.set_window(200, 16);
    model.open_prompt_overlay();

    let rows = rendered_rows(&render_model_buffer(&mut model, 200, 16));
    let disabled_row = rows
        .iter()
        .find(|row| row.contains("safety-policy"))
        .expect("disabled source row should render");

    assert!(!disabled_row.contains("disabled"));
}

#[test]
fn source_status_marker_renders_at_right_edge_of_source_column() {
    let source = ready_model().prompt_assembly.sources.managed[2].clone();
    let width = 60;
    let row = super::prompt_overlay_active_row_text(
        &source,
        super::PromptOverlayManagedStatus::Missing,
        0,
        width,
    );
    let source_cell = left_source_cell_text(&row, width);
    let expected = format!(
        "{:<padding$}missing",
        source.title,
        padding = super::prompt_overlay_left_source_width(width) - "missing".len(),
    );

    assert_eq!(source_cell, expected);
    assert!(!source_cell.contains('·'));
}

#[test]
fn source_shadowed_count_marker_renders_at_right_edge_of_source_column() {
    let source = ready_model().prompt_assembly.sources.managed[2].clone();
    let width = 60;
    let row = super::prompt_overlay_active_row_text(
        &source,
        super::PromptOverlayManagedStatus::Active,
        2,
        width,
    );
    let source_cell = left_source_cell_text(&row, width);
    let expected = format!(
        "{:<padding$}+2 shadowed",
        source.title,
        padding = super::prompt_overlay_left_source_width(width) - "+2 shadowed".len(),
    );

    assert_eq!(source_cell, expected);
    assert!(!source_cell.contains('·'));
}

#[test]
fn shadowed_detail_row_delete_targets_shadowed_source() {
    let mut model = ready_model();
    model
        .prompt_assembly
        .resolution
        .assembly
        .inactive_sources
        .push(prompt_source(
            "repo-rules",
            "repo-rules",
            PromptSourceKind::ExtraPrompt,
            Some(PromptSourceOrigin::Global),
            PromptSourceStatus::Inactive {
                reason: PromptSourceInactiveReason::Shadowed,
            },
        ));
    model.open_prompt_overlay();
    let _ = model.handle_prompt_overlay_key(KeyEvent::from(KeyCode::Down));
    let _ = model.handle_prompt_overlay_key(KeyEvent::from(KeyCode::Down));
    let _ = model.handle_prompt_overlay_key(KeyEvent::new(
        KeyCode::Char('e'),
        crossterm::event::KeyModifiers::CONTROL,
    ));
    let _ = model.handle_prompt_overlay_key(KeyEvent::from(KeyCode::Down));

    assert_eq!(
        model.handle_prompt_overlay_key(KeyEvent::from(KeyCode::Char('d'))),
        super::OverlayInputResult::Effect(AppEffect::MutatePromptAssembly {
            mutation: PromptAssemblyMutation::scoped(
                PromptAssemblyScope::Global,
                PromptAssemblyScopedMutationKind::RemovePromptSource {
                    kind: PromptSourceKind::ExtraPrompt,
                    reference_id: "repo-rules".to_string(),
                },
            ),
        })
    );
}

#[test]
fn scope_picker_renders_rounded_border_selected_scope_background_and_spacing() {
    let mut model = ready_model();
    model.open_prompt_overlay();
    model.set_prompt_overlay_focus(super::PromptOverlayFocus::Inactive);
    let _ = model.handle_prompt_overlay_key(KeyEvent::from(KeyCode::Tab));
    let _ = model.handle_prompt_overlay_key(KeyEvent::from(KeyCode::Char('a')));

    let buffer = render_model_buffer(&mut model, 100, 16);
    let rows = rendered_rows(&buffer);
    let joined = rows.join("\n");
    assert!(joined.contains("╭"));
    assert!(joined.contains("╮"));
    assert!(joined.contains("╰"));
    assert!(joined.contains("╯"));

    let scope_row_index = rows
        .iter()
        .position(|row| row.contains("[Project]") && row.contains("Global"))
        .expect("scope row should render");
    let footer_row_index = rows
        .iter()
        .position(|row| {
            row.contains("←/→/h/l select")
                && row.contains("Enter confirm")
                && row.contains("Esc cancel")
        })
        .expect("footer row should render");
    assert!(
        footer_row_index >= scope_row_index + 2,
        "scope row and footer row should have a blank line between them: rows={rows:?}"
    );

    let scope_row = &rows[scope_row_index];
    let scope_byte_index = scope_row
        .find("[Project]")
        .expect("selected scope should render");
    let scope_column = scope_row[..scope_byte_index].chars().count();
    assert_eq!(
        buffer[(
            u16::try_from(scope_column).expect("scope column should fit"),
            u16::try_from(scope_row_index).expect("scope row index should fit")
        )]
            .bg,
        default_palette()
            .surface
            .expect("default palette should expose a surface background"),
    );
}

#[test]
fn render_uses_fixed_width_table_columns_with_right_heavier_split() {
    let mut model = ready_model();
    model.set_window(120, 16);
    model.open_prompt_overlay();
    model.set_prompt_overlay_focus(super::PromptOverlayFocus::Inactive);
    let _ = model.handle_prompt_overlay_key(KeyEvent::from(KeyCode::Tab));

    let rows = rendered_rows(&render_model_buffer(&mut model, 120, 16));
    let left_header = rows
        .iter()
        .find(|row| row.contains("Sel") && row.contains("Ord") && row.contains("Scope"))
        .expect("left header should render");
    let left_row = rows
        .iter()
        .find(|row| row.contains("builtin") && row.contains("system"))
        .unwrap_or_else(|| panic!("left row should render: {rows:?}"))
        .replace('█', " ");
    let right_header = rows
        .iter()
        .find(|row| row.contains("Name") && row.contains("Scope"))
        .expect("right header should render");
    let right_row = rows
        .iter()
        .find(|row| row.contains("global-extra"))
        .expect("right row should render")
        .replace('█', " ");

    let [left_header_pane, right_header_pane]: [&str; 2] = right_header
        .split('│')
        .collect::<Vec<_>>()
        .try_into()
        .expect("prompt overlay should render two panes");
    assert!(
        right_header_pane.contains("Sel"),
        "skills pane should render a Sel column: {right_header:?}"
    );
    let divider_column = right_header
        .chars()
        .position(|character| character == '│')
        .expect("prompt overlay should render pane divider");
    let total_columns = right_header.chars().count();
    let right_pane_width = total_columns.saturating_sub(divider_column + 1);
    assert!(
        right_pane_width > divider_column,
        "right pane should be wider than left: left={left_header_pane:?}, right={right_header_pane:?}"
    );

    let left_header_ord = left_header.find("Ord").expect("Ord col should exist");
    let left_row_ord = left_row
        .find("1 ")
        .expect("left order value should render")
        .saturating_sub(4);
    assert_eq!(left_header_ord, left_row_ord);

    let left_header_scope = left_header.find("Scope").expect("Scope col should exist");
    assert!(
        left_row
            .get(left_header_scope..)
            .is_some_and(|tail| tail.trim_start().starts_with("builtin")),
        "left scope column should start with builtin: {left_row:?}"
    );

    let right_header_scope = right_header
        .rfind("Scope")
        .expect("right scope col should exist");
    assert!(
        right_row
            .get(right_header_scope..)
            .is_some_and(|tail| tail.trim_start().starts_with("global")),
        "right scope column should start with global: {right_row:?}"
    );
}

#[test]
fn skills_tab_orders_rows_by_selected_order_before_manual_only_suffix() {
    let mut model = ready_model();
    model.set_window(120, 20);
    model.prompt_assembly.candidates.discovered_skills = vec![
        PromptAssemblyDiscoveredSkill {
            skill_name: "caveman".to_string(),
            title: "caveman".to_string(),
            description: "Be brief".to_string(),
            origin: PromptSourceOrigin::Project,
            selection_scope: PromptAssemblyScope::Project,
            skill_path: "/tmp/caveman/SKILL.md".into(),
            body: "# caveman".to_string(),
            selection: PromptAssemblySelectionState::from_parts(true, true, Some(21)),
        },
        PromptAssemblyDiscoveredSkill {
            skill_name: "codebase-design".to_string(),
            title: "codebase-design".to_string(),
            description: "Design modules".to_string(),
            origin: PromptSourceOrigin::Project,
            selection_scope: PromptAssemblyScope::Project,
            skill_path: "/tmp/codebase-design/SKILL.md".into(),
            body: "# codebase-design".to_string(),
            selection: PromptAssemblySelectionState::from_parts(true, true, Some(8)),
        },
        PromptAssemblyDiscoveredSkill {
            skill_name: "ask-matt".to_string(),
            title: "ask-matt".to_string(),
            description: "Ask which skill fits".to_string(),
            origin: PromptSourceOrigin::Project,
            selection_scope: PromptAssemblyScope::Project,
            skill_path: "/tmp/ask-matt/SKILL.md".into(),
            body: "# ask-matt".to_string(),
            selection: PromptAssemblySelectionState::from_parts(false, false, None),
        },
    ];
    model.open_prompt_overlay();
    model.set_prompt_overlay_focus(super::PromptOverlayFocus::Inactive);

    let rows = rendered_rows(&render_model_buffer(&mut model, 120, 20));
    let caveman_row = rows
        .iter()
        .position(|row| row.contains("caveman"))
        .expect("caveman row should render");
    let codebase_design_row = rows
        .iter()
        .position(|row| row.contains("codebase-design"))
        .expect("codebase-design row should render");
    let ask_matt_row = rows
        .iter()
        .position(|row| row.contains("ask-matt") && row.contains("(manual)"))
        .expect("manual skill row should render");

    assert!(codebase_design_row < caveman_row);
    assert!(caveman_row < ask_matt_row);
    assert!(rows.join("\n").contains(" 21 "));
    assert!(rows.join("\n").contains("  8 "));
}

#[test]
fn manual_only_skill_stays_visible_with_manual_marker() {
    let mut model = ready_model();
    model
        .prompt_assembly
        .candidates
        .discovered_skills
        .push(PromptAssemblyDiscoveredSkill {
            skill_name: "ask-matt".to_string(),
            title: "ask-matt".to_string(),
            description: "Ask which skill fits".to_string(),
            origin: PromptSourceOrigin::Project,
            selection_scope: PromptAssemblyScope::Project,
            skill_path: "/tmp/ask-matt/SKILL.md".into(),
            body: "# Ask Matt".to_string(),
            selection: PromptAssemblySelectionState::from_parts(false, false, None),
        });
    model.open_prompt_overlay();
    model.set_prompt_overlay_focus(super::PromptOverlayFocus::Inactive);
    let _ = model.handle_prompt_overlay_key(KeyEvent::from(KeyCode::Down));
    let _ = model.handle_prompt_overlay_key(KeyEvent::from(KeyCode::Down));

    let rows = rendered_rows(&render_model_buffer(&mut model, 120, 16)).join("\n");
    assert!(rows.contains("ask-matt"));
    assert!(rows.contains("(manual)"));
    let manual_row = rows
        .lines()
        .find(|row| row.contains("ask-matt") && row.contains("(manual)"))
        .expect("manual skill row should render");
    assert!(manual_row.contains("(manual)"));
}

#[test]
fn manual_only_skill_preview_shows_notice_above_body() {
    let mut model = ready_model();
    model.prompt_assembly.candidates.discovered_skills = vec![PromptAssemblyDiscoveredSkill {
        skill_name: "ask-matt".to_string(),
        title: "ask-matt".to_string(),
        description: "Ask which skill fits".to_string(),
        origin: PromptSourceOrigin::Project,
        selection_scope: PromptAssemblyScope::Project,
        skill_path: "/tmp/ask-matt/SKILL.md".into(),
        body: "# Ask Matt".to_string(),
        selection: PromptAssemblySelectionState::from_parts(false, false, None),
    }];
    model.open_prompt_overlay();
    model.set_prompt_overlay_focus(super::PromptOverlayFocus::Inactive);

    let _ = model.handle_prompt_overlay_key(KeyEvent::from(KeyCode::Char(' ')));
    let rows = rendered_rows(&render_model_buffer(&mut model, 100, 12));

    let notice_index = rows
        .iter()
        .position(|row| row.contains("Manual-only skill:"))
        .expect("manual preview notice should render");
    let body_index = rows
        .iter()
        .position(|row| row.contains("# Ask Matt"))
        .expect("manual preview body should render");
    assert_eq!(body_index, notice_index + 2);
}

#[test]
fn skills_tab_shows_ord_column() {
    let mut model = ready_model();
    model.set_window(120, 16);
    model.open_prompt_overlay();
    model.set_prompt_overlay_focus(super::PromptOverlayFocus::Inactive);

    let rows = rendered_rows(&render_model_buffer(&mut model, 120, 16));
    let right_header = rows
        .iter()
        .find(|row| row.contains("Ord") && row.contains("Name") && row.contains("Scope"))
        .expect("right header should render");
    let right_pane = right_header
        .split('│')
        .nth(1)
        .expect("right pane should exist");

    assert!(right_pane.contains("Ord"));
}

#[test]
fn empty_extra_candidates_state_aligns_with_sel_column() {
    let mut model = ready_model();
    model.set_window(120, 16);
    model.prompt_assembly.candidates.extra_prompts.clear();
    model.open_prompt_overlay();
    model.set_prompt_overlay_focus(super::PromptOverlayFocus::Inactive);
    let _ = model.handle_prompt_overlay_key(KeyEvent::from(KeyCode::Tab));

    let rows = rendered_rows(&render_model_buffer(&mut model, 120, 16));
    let right_header = rows
        .iter()
        .find(|row| row.contains("Name") && row.contains("Scope"))
        .expect("right header should render");
    let empty_row = rows
        .iter()
        .find(|row| row.contains("No candidates"))
        .expect("empty candidates row should render");
    let right_header_pane = right_header
        .split('│')
        .nth(1)
        .expect("right pane should exist");
    let right_empty_pane = empty_row
        .split('│')
        .nth(1)
        .expect("right pane should exist");

    assert_eq!(
        right_header_pane.find("Sel"),
        right_empty_pane.find("No candidates"),
        "empty state should align with Sel column: header={right_header_pane:?}, row={right_empty_pane:?}"
    );
}

#[test]
fn empty_skills_state_aligns_with_sel_column() {
    let mut model = ready_model();
    model.set_window(120, 16);
    model.prompt_assembly.candidates.discovered_skills.clear();
    model.open_prompt_overlay();
    model.set_prompt_overlay_focus(super::PromptOverlayFocus::Inactive);

    let rows = rendered_rows(&render_model_buffer(&mut model, 120, 16));
    let right_header = rows
        .iter()
        .find(|row| row.contains("Sel") && row.contains("Name") && row.contains("Scope"))
        .expect("skills header should render");
    let empty_row = rows
        .iter()
        .find(|row| row.contains("No discovered skills"))
        .expect("empty skills row should render");
    let right_header_pane = right_header
        .split('│')
        .nth(1)
        .expect("right pane should exist");
    let right_empty_pane = empty_row
        .split('│')
        .nth(1)
        .expect("right pane should exist");

    assert_eq!(
        right_header_pane.find("Sel"),
        right_empty_pane.find("No discovered skills"),
        "empty state should align with Sel column: header={right_header_pane:?}, row={right_empty_pane:?}"
    );
}

#[test]
fn selected_header_tab_uses_surface_background_and_trailing_padding() {
    let mut model = ready_model();
    model.open_prompt_overlay();

    let buffer = render_model_buffer(&mut model, 90, 16);
    let header_row = rendered_rows(&buffer)
        .into_iter()
        .next()
        .expect("header row should render");
    let skill_byte_index = header_row
        .find("[Skill]")
        .expect("selected skill tab should render");
    let skill_index = header_row[..skill_byte_index].chars().count();
    let trailing_index = skill_index + "[Skill] Custom Prompts Tools Dynamic".chars().count();

    assert_eq!(
        buffer[(u16::try_from(skill_index).expect("tab index should fit"), 0)].bg,
        default_palette()
            .surface
            .expect("default palette should expose a surface background"),
    );
    assert_eq!(
        buffer[(
            u16::try_from(trailing_index).expect("padding index should fit"),
            0
        )]
            .symbol(),
        " "
    );
    assert_eq!(
        buffer[(
            u16::try_from(trailing_index + 1).expect("padding index should fit"),
            0
        )]
            .symbol(),
        " "
    );
}

#[test]
fn right_header_tabs_are_all_underlined() {
    let mut model = ready_model();
    model.open_prompt_overlay();

    let buffer = render_model_buffer(&mut model, 90, 16);
    for label in ["[Skill]", "Custom Prompts", "Tools", "Dynamic"] {
        let (column, row) =
            find_buffer_text_position(&buffer, label).expect("header tab should render");
        assert_text_cells_are_underlined_at(&buffer, label, row, column);
    }
}

#[test]
fn type_column_uses_full_words_and_fits_discovery_label() {
    let mut model = ready_model();
    model.set_window(120, 16);
    model.open_prompt_overlay();

    let rows = rendered_rows(&render_model_buffer(&mut model, 120, 16));
    let skill_discovery_row = rows
        .iter()
        .find(|row| row.contains("Skill discovery") && row.contains("discovery"))
        .expect("skill discovery row should render");

    let discovery_index = skill_discovery_row
        .find("discovery")
        .expect("type label should render");
    let project_index = skill_discovery_row
        .find("project")
        .expect("scope label should render");
    assert!(
        discovery_index < project_index,
        "Type column should render the full discovery label before scope: {skill_discovery_row:?}"
    );
}

#[test]
fn left_sel_column_starts_two_cells_after_focus_marker() {
    let mut model = ready_model();
    model.set_window(120, 16);
    model.open_prompt_overlay();

    let active_rows = rendered_rows(&render_model_buffer(&mut model, 120, 16));
    let left_header = active_rows
        .iter()
        .find(|row| row.contains("Sel") && row.contains("Ord") && row.contains("Scope"))
        .expect("left header should render")
        .split('│')
        .next()
        .expect("left pane should render");
    let active_row = active_rows
        .iter()
        .find(|row| row.contains("builtin") && row.contains("system"))
        .expect("active row should render")
        .split('│')
        .next()
        .expect("left pane should render");
    let active_marker_index = active_row
        .chars()
        .position(|symbol| symbol == '█')
        .expect("focus marker should render");
    let sel_index = left_header.find("Sel").expect("Sel column should render");
    assert_eq!(
        sel_index.saturating_sub(active_marker_index + 1),
        2,
        "Sel column should start two cells after focus marker: header={left_header:?}, row={active_row:?}"
    );
}

#[test]
fn active_focus_only_shows_selection_marker_in_focused_pane() {
    let mut model = ready_model();
    model.set_window(120, 16);
    model.open_prompt_overlay();

    let active_rows = rendered_rows(&render_model_buffer(&mut model, 120, 16)).join("\n");
    assert!(active_rows.contains("█"));

    model.set_prompt_overlay_focus(super::PromptOverlayFocus::Inactive);
    let _ = model.handle_prompt_overlay_key(KeyEvent::from(KeyCode::Tab));
    let inactive_rows = rendered_rows(&render_model_buffer(&mut model, 120, 16));
    let left_row = inactive_rows
        .iter()
        .find(|row| row.contains("builtin") && row.contains("system"))
        .expect("left row should render");
    let right_row = inactive_rows
        .iter()
        .find(|row| row.contains("global-extra"))
        .expect("right row should render");
    let left_pane = left_row.split('│').next().expect("left pane should exist");
    let right_pane = right_row
        .split('│')
        .nth(1)
        .expect("right pane should exist");
    assert!(!left_pane.contains('█'));
    assert!(right_pane.contains('█'));
}

#[test]
fn unfocused_inactive_row_does_not_keep_selected_text_style() {
    let mut model = ready_model();
    model.set_window(120, 16);
    model.open_prompt_overlay();
    model.set_prompt_overlay_focus(super::PromptOverlayFocus::Inactive);
    let _ = model.handle_prompt_overlay_key(KeyEvent::from(KeyCode::Tab));

    let focused_buffer = render_model_buffer(&mut model, 120, 16);
    let (column, row) = find_buffer_text_position(&focused_buffer, "global-extra")
        .expect("custom prompt row should render");
    assert!(
        focused_buffer[(column, row)]
            .modifier
            .contains(Modifier::BOLD),
        "focused right-pane selection should still use selected text style"
    );

    model.set_prompt_overlay_focus(super::PromptOverlayFocus::Active);
    let unfocused_buffer = render_model_buffer(&mut model, 120, 16);
    let (column, row) = find_buffer_text_position(&unfocused_buffer, "global-extra")
        .expect("custom prompt row should keep rendering after focus switches away");
    assert!(
        !unfocused_buffer[(column, row)]
            .modifier
            .contains(Modifier::BOLD),
        "remembered right-pane selection should not stay visually selected after focus returns left"
    );
}

#[test]
fn mouse_click_on_right_header_tab_switches_focus_and_tab() {
    let mut model = ready_model();
    model.set_window(120, 16);
    model.open_prompt_overlay();

    let buffer = render_model_buffer(&mut model, 120, 16);
    let (column, row) =
        find_buffer_text_position(&buffer, "Custom Prompts").expect("header tab should render");
    click_left(&mut model, column, row);

    let state = model
        .prompt_overlay
        .as_ref()
        .expect("prompt overlay should remain open");
    assert_eq!(state.focus, super::PromptOverlayFocus::Inactive);
    assert_eq!(state.inactive_tab, PromptOverlayInactiveTab::ExtraPrompts);
}

#[test]
fn mouse_click_on_right_row_switches_focus_and_selects_item() {
    let mut model = ready_model();
    model.set_window(120, 16);
    model.open_prompt_overlay();

    let buffer = render_model_buffer(&mut model, 120, 16);
    let (column, row) = find_buffer_text_position(&buffer, "repo-bootstrap")
        .expect("second discovered skill should render");
    click_left(&mut model, column, row);

    let state = model
        .prompt_overlay
        .as_ref()
        .expect("prompt overlay should remain open");
    assert_eq!(state.focus, super::PromptOverlayFocus::Inactive);
    assert_eq!(
        state.inactive_tab,
        PromptOverlayInactiveTab::LongLivedSkills
    );
    assert_eq!(
        state.inactive_selected_row_id.as_deref(),
        Some("skill:repo-bootstrap:project")
    );
}

#[test]
fn mouse_click_on_left_row_switches_focus_and_selects_item() {
    let mut model = ready_model();
    model.set_window(120, 16);
    model.open_prompt_overlay();
    model.set_prompt_overlay_focus(super::PromptOverlayFocus::Inactive);

    let buffer = render_model_buffer(&mut model, 120, 16);
    let (column, row) =
        find_buffer_text_position(&buffer, "repo-rules").expect("active row should render");
    click_left(&mut model, column, row);

    let state = model
        .prompt_overlay
        .as_ref()
        .expect("prompt overlay should remain open");
    assert_eq!(state.focus, super::PromptOverlayFocus::Active);
    assert_eq!(state.active_selected, 2);
}

#[test]
fn custom_prompt_scope_dialog_is_centered_within_right_pane() {
    let mut model = ready_model();
    model.set_window(100, 16);
    model.open_prompt_overlay();
    model.set_prompt_overlay_focus(super::PromptOverlayFocus::Inactive);
    let _ = model.handle_prompt_overlay_key(KeyEvent::from(KeyCode::Tab));
    let _ = model.handle_prompt_overlay_key(KeyEvent::from(KeyCode::Char('a')));

    let buffer = render_model_buffer(&mut model, 100, 16);
    let rows = rendered_rows(&buffer);
    let (actual_x, actual_y) =
        find_text_position(&rows, "╭").expect("dialog top-left corner should render");

    let chrome = fullscreen_list_chrome_rects(Rect::new(0, 0, 100, 16)).expect("chrome should fit");
    let [_left_pane, _gutter, right_pane] = Layout::horizontal([
        Constraint::Ratio(
            super::PROMPT_OVERLAY_LEFT_PANE_RATIO_NUMERATOR,
            super::PROMPT_OVERLAY_PANE_RATIO_DENOMINATOR,
        ),
        Constraint::Length(1),
        Constraint::Ratio(
            super::PROMPT_OVERLAY_RIGHT_PANE_RATIO_NUMERATOR,
            super::PROMPT_OVERLAY_PANE_RATIO_DENOMINATOR,
        ),
    ])
    .areas(chrome.body);
    let dialog_width = right_pane.width.min(52);
    let dialog_height = 7u16.min(right_pane.height);
    let expected_x = right_pane.x + right_pane.width.saturating_sub(dialog_width) / 2;
    let expected_y = right_pane.y + right_pane.height.saturating_sub(dialog_height) / 2;

    assert_eq!(actual_x, expected_x);
    assert_eq!(actual_y, expected_y);
}

#[test]
fn space_opens_prompt_source_preview() {
    let mut model = ready_model();
    model.prompt_assembly.sources.preview = vec![
        runtime_domain::prompt_assembly::PromptAssemblyManagerSource {
            reference_id: "core-system".to_string(),
            kind: PromptSourceKind::CoreSystemPrompt,
            title: "Core system prompt".to_string(),
            origin: Some(PromptSourceOrigin::Builtin),
            resolved_body_origin: Some(PromptSourceOrigin::Builtin),
            backing_file_path: None,
            body: Some("# Core\n\nHello".to_string()),
        },
    ];
    model.open_prompt_overlay();

    let result = model.handle_prompt_overlay_key(KeyEvent::from(KeyCode::Char(' ')));

    assert_eq!(result, super::OverlayInputResult::Handled);
    assert!(model.prompt_overlay_preview_active());
}

#[test]
fn p_toggles_assembled_preview_open_and_closed() {
    let mut model = ready_model();
    model.open_prompt_overlay();

    assert_eq!(
        model.handle_prompt_overlay_key(KeyEvent::from(KeyCode::Char('p'))),
        super::OverlayInputResult::Handled
    );
    assert!(model.prompt_overlay_preview_active());

    assert_eq!(
        model.handle_prompt_overlay_key(KeyEvent::from(KeyCode::Char('p'))),
        super::OverlayInputResult::Handled
    );
    assert!(!model.prompt_overlay_preview_active());
}

#[test]
fn prompt_preview_renders_markdown_source_as_plain_text() {
    let mut model = ready_model();
    model.prompt_assembly.sources.preview = vec![
        runtime_domain::prompt_assembly::PromptAssemblyManagerSource {
            reference_id: "core-system".to_string(),
            kind: PromptSourceKind::CoreSystemPrompt,
            title: "Core system prompt".to_string(),
            origin: Some(PromptSourceOrigin::Builtin),
            resolved_body_origin: Some(PromptSourceOrigin::Builtin),
            backing_file_path: None,
            body: Some("# Core Heading\n\n- keep marker\n\n`cargo test`\n".to_string()),
        },
    ];
    model.open_prompt_overlay();

    let _ = model.handle_prompt_overlay_key(KeyEvent::from(KeyCode::Char(' ')));
    let rows = rendered_rows(&render_model_buffer(&mut model, 80, 12));

    assert!(
        rows.iter().any(|row| row.contains("# Core Heading")),
        "plain preview should keep heading marker literal: {rows:?}"
    );
    assert!(
        rows.iter().any(|row| row.contains("- keep marker")),
        "plain preview should keep list marker literal: {rows:?}"
    );
    assert!(
        rows.iter().any(|row| row.contains("`cargo test`")),
        "plain preview should keep inline code markers literal: {rows:?}"
    );
}

#[test]
fn prompt_preview_rewraps_after_resize() {
    let mut model = ready_model();
    model.prompt_assembly.sources.preview = vec![
        runtime_domain::prompt_assembly::PromptAssemblyManagerSource {
            reference_id: "core-system".to_string(),
            kind: PromptSourceKind::CoreSystemPrompt,
            title: "Core system prompt".to_string(),
            origin: Some(PromptSourceOrigin::Builtin),
            resolved_body_origin: Some(PromptSourceOrigin::Builtin),
            backing_file_path: None,
            body: Some(
                "alpha beta gamma delta epsilon zeta eta theta iota kappa lambda mu".to_string(),
            ),
        },
    ];
    model.open_prompt_overlay();
    let _ = model.handle_prompt_overlay_key(KeyEvent::from(KeyCode::Char(' ')));
    assert!(model.prompt_overlay_preview_active());

    let wide_line_count = model
        .prompt_overlay_preview_wrapped_lines()
        .map(|lines| lines.len())
        .expect("preview should expose wrapped lines");

    model.update(AppEvent::Resized {
        width: 18,
        height: 16,
    });

    let narrow_line_count = model
        .prompt_overlay_preview_wrapped_lines()
        .map(|lines| lines.len())
        .expect("preview should stay open after resize");
    assert!(
        narrow_line_count > wide_line_count,
        "prompt preview should rewrap after resize: wide={wide_line_count}, narrow={narrow_line_count}"
    );
}

#[test]
fn prompt_preview_word_wraps_indented_skill_lines() {
    let mut model = ready_model();
    model.set_window(24, 12);
    model.open_prompt_overlay();
    model.open_prompt_overlay_plain_text_preview(
        "repo-bootstrap".to_string(),
        "<skill>\n    hello world from skill body\n</skill>",
        None,
    );
    let rows = rendered_rows(&render_model_buffer(&mut model, 24, 12));

    assert!(
        rows.iter().any(|row| row.contains("    hello")),
        "indented skill line should keep word wrapping instead of hard character splits: {rows:?}"
    );
    assert!(
        rows.iter().any(|row| row.contains("world from")),
        "wrapped continuation should preserve words: {rows:?}"
    );
}

#[test]
fn custom_prompt_rows_sort_titles_naturally() {
    let mut model = ready_model();
    model.prompt_assembly.candidates.extra_prompts = vec![
        PromptAssemblyExtraPromptCandidate {
            reference_id: "new-prompt-10".to_string(),
            title: "New prompt 10".to_string(),
            origin: PromptSourceOrigin::Project,
            body: "# New prompt 10\n".to_string(),
            selected: false,
        },
        PromptAssemblyExtraPromptCandidate {
            reference_id: "new-prompt-2".to_string(),
            title: "New prompt 2".to_string(),
            origin: PromptSourceOrigin::Project,
            body: "# New prompt 2\n".to_string(),
            selected: false,
        },
        PromptAssemblyExtraPromptCandidate {
            reference_id: "new-prompt-1".to_string(),
            title: "New prompt 1".to_string(),
            origin: PromptSourceOrigin::Project,
            body: "# New prompt 1\n".to_string(),
            selected: false,
        },
    ];
    model.open_prompt_overlay();
    model.set_prompt_overlay_focus(super::PromptOverlayFocus::Inactive);
    let _ = model.handle_prompt_overlay_key(KeyEvent::from(KeyCode::Tab));

    let rows = rendered_rows(&render_model_buffer(&mut model, 120, 16)).join("\n");
    let first = rows
        .find("New prompt 1")
        .expect("first prompt should render");
    let second = rows
        .find("New prompt 2")
        .expect("second prompt should render");
    let tenth = rows
        .find("New prompt 10")
        .expect("tenth prompt should render");

    assert!(
        first < second && second < tenth,
        "custom prompts should sort naturally: {rows}"
    );
}

#[test]
fn footer_hides_custom_and_skill_actions_on_left_pane() {
    let mut model = ready_model();
    model.set_window(140, 16);
    model.open_prompt_overlay();
    let _ = model.handle_prompt_overlay_key(KeyEvent::from(KeyCode::Down));
    let _ = model.handle_prompt_overlay_key(KeyEvent::from(KeyCode::Down));

    let rows = rendered_rows(&render_model_buffer(&mut model, 140, 16)).join("\n");

    assert!(!rows.contains("a create prompt"));
    assert!(!rows.contains("i/I add skill"));
    assert!(rows.contains("d remove"));
    assert!(rows.contains("x disable"));
    assert!(rows.contains("J/K reorder"));
    assert!(rows.contains("? more"));
    assert!(!rows.contains("Esc close"));
    assert!(!rows.contains("←/→/h/l focus panes"));
    assert!(!rows.contains("↑/↓/j/k move"));
    assert!(rows.contains("Space preview"));
    assert!(rows.contains("J/K reorder"));
    assert!(rows.contains("· ? more"));
}

#[test]
fn footer_hides_remove_for_active_skill_discovery() {
    let mut model = ready_model();
    model.set_window(140, 16);
    model.open_prompt_overlay();
    let _ = model.handle_prompt_overlay_key(KeyEvent::from(KeyCode::Down));

    let rows = rendered_rows(&render_model_buffer(&mut model, 140, 16)).join("\n");

    assert!(!rows.contains("d remove"));
    assert!(rows.contains("x disable"));
}

#[test]
fn footer_hides_remove_for_active_instruction_file() {
    let mut model = ready_model();
    model
        .prompt_assembly
        .resolution
        .assembly
        .active_sources
        .insert(
            1,
            prompt_source(
                "instructions:project:.",
                "AGENTS.md",
                PromptSourceKind::InstructionsFile,
                Some(PromptSourceOrigin::Project),
                PromptSourceStatus::Active { order: 1 },
            ),
        );
    model.prompt_assembly.sources.managed.insert(
        1,
        PromptAssemblyManagedSource {
            reference_id: "instructions:project:.".to_string(),
            kind: PromptSourceKind::InstructionsFile,
            title: "AGENTS.md".to_string(),
            origin: Some(PromptSourceOrigin::Project),
            scope: Some(PromptAssemblyScope::Project),
            enabled: true,
            order: 2,
        },
    );
    model
        .prompt_assembly
        .sources
        .preview
        .push(PromptAssemblyManagerSource {
            reference_id: "instructions:project:.".to_string(),
            kind: PromptSourceKind::InstructionsFile,
            title: "AGENTS.md".to_string(),
            origin: Some(PromptSourceOrigin::Project),
            resolved_body_origin: Some(PromptSourceOrigin::Project),
            backing_file_path: Some("/tmp/repo/AGENTS.md".into()),
            body: Some("project instructions".to_string()),
        });
    model.set_window(140, 16);
    model.open_prompt_overlay();
    let _ = model.handle_prompt_overlay_key(KeyEvent::from(KeyCode::Down));

    let rows = rendered_rows(&render_model_buffer(&mut model, 140, 16)).join("\n");

    assert!(!rows.contains("d remove"));
    assert!(rows.contains("x disable"));
    assert!(rows.contains("e/ctrl+g edit"));
}

#[test]
fn footer_shows_custom_actions_only_on_custom_tab() {
    let mut model = ready_model();
    model.set_window(140, 16);
    model.open_prompt_overlay();
    model.set_prompt_overlay_focus(super::PromptOverlayFocus::Inactive);
    let _ = model.handle_prompt_overlay_key(KeyEvent::from(KeyCode::Tab));

    let rows = rendered_rows(&render_model_buffer(&mut model, 140, 16)).join("\n");

    assert!(rows.contains("a create prompt"));
    assert!(!rows.contains("a/A add extra"));
    assert!(!rows.contains("i/I add skill"));
    assert!(rows.contains("d remove"));
    assert!(!rows.contains("J/K reorder"));
    assert!(rows.contains("? more"));
    assert!(!rows.contains("Esc close"));
    assert!(!rows.contains("←/→/h/l focus panes"));
    assert!(!rows.contains("↑/↓/j/k move"));
    assert!(rows.contains("Space preview"));
    assert!(rows.contains("Tab tabs"));
    assert!(rows.contains("· ? more"));
}

#[test]
fn footer_shows_create_prompt_on_empty_custom_tab() {
    let mut model = ready_model();
    model.prompt_assembly.candidates.extra_prompts.clear();
    model.set_window(140, 16);
    model.open_prompt_overlay();
    model.set_prompt_overlay_focus(super::PromptOverlayFocus::Inactive);
    let _ = model.handle_prompt_overlay_key(KeyEvent::from(KeyCode::Tab));

    let rows = rendered_rows(&render_model_buffer(&mut model, 140, 16)).join("\n");

    assert!(rows.contains("a create prompt"));
    assert!(!rows.contains("d remove"));
    assert!(!rows.contains("e/ctrl+g edit"));
    assert!(rows.contains("? more"));
    assert!(rows.contains("Tab tabs"));
    assert!(rows.contains("· ? more"));
}

#[test]
fn footer_hides_custom_edit_and_remove_actions_on_skills_tab() {
    let mut model = ready_model();
    model.set_window(140, 16);
    model.open_prompt_overlay();
    model.set_prompt_overlay_focus(super::PromptOverlayFocus::Inactive);

    let rows = rendered_rows(&render_model_buffer(&mut model, 140, 16)).join("\n");

    assert!(!rows.contains("a create prompt"));
    assert!(!rows.contains("i/I add skill"));
    assert!(!rows.contains("d remove"));
    assert!(!rows.contains("e/ctrl+g edit"));
    assert!(rows.contains("x disable"));
    assert!(rows.contains("J/K reorder"));
    assert!(rows.contains("? more"));
    assert!(!rows.contains("Esc close"));
    assert!(!rows.contains("←/→/h/l focus panes"));
    assert!(!rows.contains("↑/↓/j/k move"));
    assert!(rows.contains("Space preview"));
    assert!(rows.contains("J/K reorder"));
    assert!(rows.contains("Tab tabs"));
    assert!(rows.contains("· ? more"));
}

#[test]
fn footer_shows_preview_disable_and_reorder_for_dynamic_environment_source() {
    let mut model = ready_model();
    model
        .prompt_assembly
        .sources
        .managed
        .insert(1, dynamic_environment_baseline_managed_source());
    model.set_window(140, 16);
    model.open_prompt_overlay();
    let _ = model.handle_prompt_overlay_key(KeyEvent::from(KeyCode::Down));

    let rows = rendered_rows(&render_model_buffer(&mut model, 140, 16)).join("\n");

    assert!(rows.contains("Space preview"));
    assert!(rows.contains("x disable"));
    assert!(rows.contains("J/K reorder"));
}

#[test]
fn space_opens_dynamic_environment_candidate_preview_for_selected_snapshot_column() {
    let mut model = ready_model();
    model.prompt_assembly.candidates.dynamic_environment =
        vec![PromptAssemblyDynamicEnvironmentCandidate {
            source_kind: DynamicEnvironmentSourceKind::GitReference,
            label: "Git reference".to_string(),
            origin: PromptSourceOrigin::Builtin,
            baseline_selected: true,
            changes_selected: false,
            baseline_preview_body: "baseline preview".to_string(),
            changes_preview_body: "changes preview".to_string(),
        }];
    model.set_window(140, 16);
    model.open_prompt_overlay();
    model.set_prompt_overlay_focus(super::PromptOverlayFocus::Inactive);
    let _ = model.handle_prompt_overlay_key(KeyEvent::from(KeyCode::Tab));
    let _ = model.handle_prompt_overlay_key(KeyEvent::from(KeyCode::Tab));
    let _ = model.handle_prompt_overlay_key(KeyEvent::from(KeyCode::Tab));

    assert_eq!(
        model.handle_prompt_overlay_key(KeyEvent::from(KeyCode::Char(' '))),
        super::OverlayInputResult::Handled
    );
    let preview = model
        .prompt_overlay
        .as_ref()
        .and_then(|state| state.preview.as_ref())
        .expect("dynamic preview should open");
    assert_eq!(preview.content, "baseline preview");

    assert_eq!(
        model.handle_prompt_overlay_key(KeyEvent::from(KeyCode::Char(' '))),
        super::OverlayInputResult::Handled
    );
    let _ = model.handle_prompt_overlay_key(KeyEvent::from(KeyCode::Right));
    assert_eq!(
        model.handle_prompt_overlay_key(KeyEvent::from(KeyCode::Char(' '))),
        super::OverlayInputResult::Handled
    );
    let preview = model
        .prompt_overlay
        .as_ref()
        .and_then(|state| state.preview.as_ref())
        .expect("changes preview should open");
    assert_eq!(preview.content, "changes preview");
}

#[test]
fn footer_hides_disable_for_core_system_prompt() {
    let mut model = ready_model();
    model.set_window(140, 16);
    model.open_prompt_overlay();

    let rows = rendered_rows(&render_model_buffer(&mut model, 140, 16)).join("\n");

    assert!(!rows.contains("x disable"));
    assert!(rows.contains("r restore"));
    assert!(rows.contains("? more"));
    assert!(rows.contains("· ? more"));
}

#[test]
fn footer_hides_remove_for_active_tool_guidelines() {
    let mut model = ready_model();
    model
        .prompt_assembly
        .resolution
        .assembly
        .active_sources
        .insert(
            1,
            prompt_source(
                "tool-guidelines",
                "Tool guidelines",
                PromptSourceKind::ToolGuidelines,
                Some(PromptSourceOrigin::Builtin),
                PromptSourceStatus::Active { order: 1 },
            ),
        );
    model
        .prompt_assembly
        .sources
        .managed
        .insert(1, tool_guidelines_managed_source());
    model
        .prompt_assembly
        .sources
        .preview
        .push(tool_guidelines_source());
    model.set_window(140, 16);
    model.open_prompt_overlay();
    let _ = model.handle_prompt_overlay_key(KeyEvent::from(KeyCode::Down));

    let rows = rendered_rows(&render_model_buffer(&mut model, 140, 16)).join("\n");

    assert!(!rows.contains("d remove"));
    assert!(rows.contains("x disable"));
    assert!(rows.contains("e/ctrl+g edit"));
    assert!(rows.contains("? more"));
    assert!(rows.contains("x disable"));
    assert!(rows.contains("· ? more"));
}

#[test]
fn shortcut_help_uses_aligned_two_column_layout() {
    let mut model = ready_model();
    model.set_window(140, 16);
    model.open_prompt_overlay();
    let _ = model.handle_prompt_overlay_key(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::SHIFT));

    let rows = rendered_rows(&render_model_buffer(&mut model, 140, 16));
    let esc_row = rows
        .iter()
        .find(|row| row.contains("Esc") && row.contains("close"))
        .expect("Esc row should render");
    let focus_row = rows
        .iter()
        .find(|row| row.contains("←/→/h/l") && row.contains("focus panes"))
        .expect("focus row should render");
    let move_row = rows
        .iter()
        .find(|row| row.contains("↑/↓/j/k") && row.contains("move"))
        .expect("move row should render");
    let space_row = rows
        .iter()
        .find(|row| row.contains("Space") && row.contains("preview"))
        .expect("Space row should render");

    let close_column = column_in_row(esc_row, "close");
    let focus_column = column_in_row(focus_row, "focus panes");
    let move_column = column_in_row(move_row, "move");
    let source_column = column_in_row(space_row, "preview");

    assert_eq!(focus_column, close_column);
    assert_eq!(move_column, close_column);
    assert_eq!(source_column, close_column);
}

#[test]
fn tools_tab_shows_ord_column_and_supports_reorder() {
    let mut model = ready_model();
    model.prompt_assembly.candidates.tools = vec![
        PromptAssemblyToolCandidate {
            name: "bash".to_string(),
            label: Some("Bash".to_string()),
            description: Some("run shell commands".to_string()),
            prompt_guidelines: Some("Prefer rg over grep.".to_string()),
            origin: PromptSourceOrigin::Builtin,
            selection_scope: PromptAssemblyScope::Global,
            selection: PromptAssemblySelectionState::from_parts(true, true, Some(1)),
        },
        PromptAssemblyToolCandidate {
            name: "read_file".to_string(),
            label: Some("Read file".to_string()),
            description: Some("read workspace files".to_string()),
            prompt_guidelines: Some("Use for direct file reads.".to_string()),
            origin: PromptSourceOrigin::Builtin,
            selection_scope: PromptAssemblyScope::Global,
            selection: PromptAssemblySelectionState::from_parts(true, true, Some(2)),
        },
    ];
    model.set_window(140, 16);
    model.open_prompt_overlay();
    model.set_prompt_overlay_focus(super::PromptOverlayFocus::Inactive);
    let _ = model.handle_prompt_overlay_key(KeyEvent::from(KeyCode::Tab));
    let _ = model.handle_prompt_overlay_key(KeyEvent::from(KeyCode::Tab));

    let rows = rendered_rows(&render_model_buffer(&mut model, 140, 16)).join("\n");
    assert!(rows.contains("Ord"));
    assert!(rows.contains("J/K reorder"));

    assert_eq!(
        model.handle_prompt_overlay_key(KeyEvent::new(
            KeyCode::Char('J'),
            crossterm::event::KeyModifiers::SHIFT,
        )),
        super::OverlayInputResult::Effect(AppEffect::MutatePromptAssembly {
            mutation: PromptAssemblyMutation::scoped(
                PromptAssemblyScope::Global,
                PromptAssemblyScopedMutationKind::MoveTool {
                    tool_name: "bash".to_string(),
                    direction: PromptAssemblyMoveDirection::Down,
                },
            ),
        })
    );
}

#[test]
fn dynamic_tab_groups_baseline_and_changes_columns_for_builtin_sources() {
    let mut model = ready_model();
    model.prompt_assembly.candidates.dynamic_environment =
        vec![PromptAssemblyDynamicEnvironmentCandidate {
            source_kind: DynamicEnvironmentSourceKind::GitReference,
            label: "Git reference".to_string(),
            origin: PromptSourceOrigin::Builtin,
            baseline_selected: true,
            changes_selected: false,
            baseline_preview_body: "baseline preview".to_string(),
            changes_preview_body: "changes preview".to_string(),
        }];
    model.set_window(140, 16);
    model.open_prompt_overlay();
    model.set_prompt_overlay_focus(super::PromptOverlayFocus::Inactive);
    let _ = model.handle_prompt_overlay_key(KeyEvent::from(KeyCode::Tab));
    let _ = model.handle_prompt_overlay_key(KeyEvent::from(KeyCode::Tab));
    let _ = model.handle_prompt_overlay_key(KeyEvent::from(KeyCode::Tab));

    let buffer = render_model_buffer(&mut model, 140, 16);
    let rows = rendered_rows(&buffer).join("\n");
    assert!(rows.contains("Base"));
    assert!(rows.contains("Change"));
    assert!(rows.contains("[x]"));
    assert!(rows.contains("[ ]"));
    assert!(rows.contains("Git reference"));
    assert!(rows.contains("builtin"));
    let (selected_checkbox_column, selected_checkbox_row) =
        find_buffer_text_position(&buffer, "[x]").expect("selected dynamic checkbox should render");
    assert_text_cells_are_underlined_at(
        &buffer,
        "[x]",
        selected_checkbox_row,
        selected_checkbox_column,
    );
    assert_cell_is_not_underlined(
        &buffer,
        selected_checkbox_row,
        selected_checkbox_column.saturating_sub(1),
    );
    assert_cell_is_not_underlined(&buffer, selected_checkbox_row, selected_checkbox_column + 3);

    assert_eq!(
        model.handle_prompt_overlay_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE)),
        super::OverlayInputResult::Effect(AppEffect::MutatePromptAssembly {
            mutation: PromptAssemblyMutation::SetDynamicEnvironmentSourceSelected {
                snapshot_kind: DynamicEnvironmentSnapshotKind::Baseline,
                source_kind: DynamicEnvironmentSourceKind::GitReference,
                selected: false,
            },
        })
    );

    let _ = model.handle_prompt_overlay_key(KeyEvent::from(KeyCode::Right));
    assert_eq!(
        model.handle_prompt_overlay_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE)),
        super::OverlayInputResult::Effect(AppEffect::MutatePromptAssembly {
            mutation: PromptAssemblyMutation::SetDynamicEnvironmentSourceSelected {
                snapshot_kind: DynamicEnvironmentSnapshotKind::Changes,
                source_kind: DynamicEnvironmentSourceKind::GitReference,
                selected: true,
            },
        })
    );
}

#[test]
fn mouse_click_on_dynamic_checkbox_selects_snapshot_column_for_x_toggle() {
    let mut model = ready_model();
    model.prompt_assembly.candidates.dynamic_environment =
        vec![PromptAssemblyDynamicEnvironmentCandidate {
            source_kind: DynamicEnvironmentSourceKind::GitReference,
            label: "Git reference".to_string(),
            origin: PromptSourceOrigin::Builtin,
            baseline_selected: true,
            changes_selected: false,
            baseline_preview_body: "baseline preview".to_string(),
            changes_preview_body: "changes preview".to_string(),
        }];
    model.set_window(140, 16);
    model.open_prompt_overlay();
    model.set_prompt_overlay_focus(super::PromptOverlayFocus::Inactive);
    let _ = model.handle_prompt_overlay_key(KeyEvent::from(KeyCode::Tab));
    let _ = model.handle_prompt_overlay_key(KeyEvent::from(KeyCode::Tab));
    let _ = model.handle_prompt_overlay_key(KeyEvent::from(KeyCode::Tab));

    let buffer = render_model_buffer(&mut model, 140, 16);
    let (changes_column, changes_row) =
        find_buffer_text_position(&buffer, "[ ]").expect("changes checkbox should render");
    click_left(&mut model, changes_column, changes_row);

    let state = model
        .prompt_overlay
        .as_ref()
        .expect("prompt overlay should remain open");
    assert_eq!(state.focus, super::PromptOverlayFocus::Inactive);
    assert_eq!(state.inactive_tab, PromptOverlayInactiveTab::Dynamic);
    assert_eq!(
        state.inactive_selected_row_id.as_deref(),
        Some("dynamic:GitReference")
    );
    assert_eq!(
        state.dynamic_selected_snapshot_kind,
        DynamicEnvironmentSnapshotKind::Changes
    );

    assert_eq!(
        model.handle_prompt_overlay_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE)),
        super::OverlayInputResult::Effect(AppEffect::MutatePromptAssembly {
            mutation: PromptAssemblyMutation::SetDynamicEnvironmentSourceSelected {
                snapshot_kind: DynamicEnvironmentSnapshotKind::Changes,
                source_kind: DynamicEnvironmentSourceKind::GitReference,
                selected: true,
            },
        })
    );

    let buffer = render_model_buffer(&mut model, 140, 16);
    let (baseline_column, baseline_row) =
        find_buffer_text_position(&buffer, "[x]").expect("baseline checkbox should render");
    click_left(&mut model, baseline_column, baseline_row);

    let state = model
        .prompt_overlay
        .as_ref()
        .expect("prompt overlay should remain open");
    assert_eq!(
        state.dynamic_selected_snapshot_kind,
        DynamicEnvironmentSnapshotKind::Baseline
    );
}

#[test]
fn r_still_restores_core_system_override_on_left_selection() {
    let mut model = ready_model();
    model.open_prompt_overlay();

    assert_eq!(
        model.handle_prompt_overlay_key(KeyEvent::new(
            KeyCode::Char('r'),
            crossterm::event::KeyModifiers::NONE,
        )),
        super::OverlayInputResult::Effect(AppEffect::MutatePromptAssembly {
            mutation: PromptAssemblyMutation::scoped(
                PromptAssemblyScope::Project,
                PromptAssemblyScopedMutationKind::RestoreCoreSystemOverride,
            ),
        })
    );
}

#[test]
fn prompt_overlay_close_shows_system_message_for_current_empty_session_notice() {
    let mut model = ready_model();
    model.open_prompt_overlay();

    model.apply_runtime_event(
        runtime_domain::session::RuntimeEvent::PromptAssemblyUpdated {
            manager: model.prompt_assembly.clone(),
            notice: Some(PromptAssemblyUpdateNotice::CurrentEmptySessionUpdated),
        },
    );

    assert_eq!(model.active_toast_text_for_test(), None);
    assert!(
        !model
            .transcript_plain_items()
            .iter()
            .any(|item| item.contains("Prompt updated for current empty session."))
    );

    assert_eq!(
        model.handle_prompt_overlay_key(KeyEvent::from(KeyCode::Esc)),
        super::OverlayInputResult::Handled
    );

    assert_eq!(model.active_toast_text_for_test(), None);
    assert!(
        model
            .transcript_plain_items()
            .iter()
            .any(|item| item.contains("Prompt updated for current empty session."))
    );
}
