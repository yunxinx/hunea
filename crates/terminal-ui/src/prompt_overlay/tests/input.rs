use super::*;

#[test]
fn command_panel_lists_prompt_command() {
    let mut model = ready_model();
    model.composer_mut().set_text_for_test("/pro");
    model.sync_command_panel_navigation();

    let rows = model
        .current_inline_command_panel_render_result()
        .plain_lines;

    assert!(rows.iter().any(|row| row.contains("/prompt")));
}

#[test]
fn prompt_command_opens_overlay_and_requests_reload() {
    let mut model = ready_model();
    model.composer_mut().set_text_for_test("/prompt");
    model.sync_command_panel_navigation();

    let effect = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Enter)));

    assert_eq!(effect, Some(AppEffect::ReloadPromptAssembly));
    assert_eq!(model.top_modal_layer(), Some(ModalLayer::PromptOverlay));
    assert!(model.blocks_composer_input());
    assert_eq!(model.composer_text(), "");
}

#[test]
fn tab_only_switches_tabs_when_right_pane_is_focused() {
    let mut model = ready_model();
    model.open_prompt_overlay();

    assert_eq!(
        model
            .prompt_overlay
            .as_ref()
            .map(|state| state.inactive_tab),
        Some(PromptOverlayInactiveTab::LongLivedSkills)
    );

    model.set_prompt_overlay_focus(super::PromptOverlayFocus::Inactive);
    let _ = model.handle_prompt_overlay_key(KeyEvent::from(KeyCode::Tab));
    assert_eq!(
        model
            .prompt_overlay
            .as_ref()
            .map(|state| state.inactive_tab),
        Some(PromptOverlayInactiveTab::ExtraPrompts)
    );

    model.set_prompt_overlay_focus(super::PromptOverlayFocus::Active);
    let _ = model.handle_prompt_overlay_key(KeyEvent::from(KeyCode::Tab));
    assert_eq!(
        model
            .prompt_overlay
            .as_ref()
            .map(|state| state.inactive_tab),
        Some(PromptOverlayInactiveTab::ExtraPrompts)
    );
}

#[test]
fn a_on_custom_tab_opens_scope_picker_instead_of_creating_immediately() {
    let mut model = ready_model();
    model.open_prompt_overlay();
    model.set_prompt_overlay_focus(super::PromptOverlayFocus::Inactive);
    let _ = model.handle_prompt_overlay_key(KeyEvent::from(KeyCode::Tab));

    assert_eq!(
        model.handle_prompt_overlay_key(KeyEvent::from(KeyCode::Char('a'))),
        super::OverlayInputResult::Handled
    );

    let rows = rendered_rows(&render_model_buffer(&mut model, 100, 16)).join("\n");
    assert!(rows.contains("Create custom prompt in"));
    assert!(rows.contains("Project"));
    assert!(rows.contains("Global"));
}

#[test]
fn scope_picker_confirms_selected_scope_for_custom_creation() {
    let mut model = ready_model();
    model.open_prompt_overlay();
    model.set_prompt_overlay_focus(super::PromptOverlayFocus::Inactive);
    let _ = model.handle_prompt_overlay_key(KeyEvent::from(KeyCode::Tab));
    let _ = model.handle_prompt_overlay_key(KeyEvent::from(KeyCode::Char('a')));
    let _ = model.handle_prompt_overlay_key(KeyEvent::from(KeyCode::Right));

    assert_eq!(
        model.handle_prompt_overlay_key(KeyEvent::from(KeyCode::Enter)),
        super::OverlayInputResult::Effect(AppEffect::MutatePromptAssembly {
            mutation: PromptAssemblyMutation::scoped(
                PromptAssemblyScope::Global,
                PromptAssemblyScopedMutationKind::CreateExtraPrompt {
                    content: "# New prompt 1\n".to_string(),
                },
            ),
        })
    );
}

#[test]
fn e_does_not_edit_discovered_skill() {
    let mut model = ready_model();
    model.open_prompt_overlay();
    model.set_prompt_overlay_focus(super::PromptOverlayFocus::Inactive);

    assert_eq!(
        model.handle_prompt_overlay_key(KeyEvent::from(KeyCode::Char('e'))),
        super::OverlayInputResult::Handled
    );
}

#[test]
fn a_and_i_do_not_act_from_skills_tab() {
    let mut model = ready_model();
    model.open_prompt_overlay();
    model.set_prompt_overlay_focus(super::PromptOverlayFocus::Inactive);

    assert_eq!(
        model.handle_prompt_overlay_key(KeyEvent::from(KeyCode::Char('a'))),
        super::OverlayInputResult::Handled
    );
    assert_eq!(
        model.handle_prompt_overlay_key(KeyEvent::from(KeyCode::Char('i'))),
        super::OverlayInputResult::Handled
    );
    assert_eq!(
        model.handle_prompt_overlay_key(KeyEvent::from(KeyCode::Char('I'))),
        super::OverlayInputResult::Handled
    );
}

#[test]
fn question_mark_opens_shortcut_help_popover() {
    let mut model = ready_model();
    model.set_window(140, 16);
    model.open_prompt_overlay();

    assert_eq!(
        model.handle_prompt_overlay_key(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::SHIFT)),
        super::OverlayInputResult::Handled
    );

    let rows = rendered_rows(&render_model_buffer(&mut model, 140, 16)).join("\n");

    assert!(rows.contains("? more"));
    assert!(rows.contains("Esc"));
    assert!(rows.contains("close"));
    assert!(rows.contains("←/→/h/l"));
    assert!(rows.contains("focus panes"));
    assert!(rows.contains("↑/↓/j/k"));
    assert!(rows.contains("move"));
    assert!(rows.contains("Space"));
    assert!(rows.contains("preview"));
    assert!(rows.contains("? / Esc"));
    assert!(rows.contains("close help"));
}

#[test]
fn shortcut_help_escape_closes_help_before_overlay() {
    let mut model = ready_model();
    model.open_prompt_overlay();
    let _ = model.handle_prompt_overlay_key(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::SHIFT));

    assert_eq!(
        model.handle_prompt_overlay_key(KeyEvent::from(KeyCode::Esc)),
        super::OverlayInputResult::Handled
    );
    assert!(model.prompt_overlay_active());
    assert!(
        !model
            .prompt_overlay
            .as_ref()
            .expect("prompt overlay should stay open")
            .shortcut_help_open
    );
}

#[test]
fn shortcut_help_closes_on_other_shortcut_and_keeps_action() {
    let mut model = ready_model();
    model.open_prompt_overlay();
    let _ = model.handle_prompt_overlay_key(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::SHIFT));

    assert_eq!(
        model.handle_prompt_overlay_key(KeyEvent::from(KeyCode::Down)),
        super::OverlayInputResult::Handled
    );

    let state = model
        .prompt_overlay
        .as_ref()
        .expect("prompt overlay should stay open");
    assert!(!state.shortcut_help_open);
    assert_eq!(state.active_selected, 1);
}

#[test]
fn shortcut_help_closes_on_non_shortcut_key() {
    let mut model = ready_model();
    model.open_prompt_overlay();
    let _ = model.handle_prompt_overlay_key(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::SHIFT));

    assert_eq!(
        model.handle_prompt_overlay_key(KeyEvent::from(KeyCode::Char('z'))),
        super::OverlayInputResult::Handled
    );

    assert!(
        !model
            .prompt_overlay
            .as_ref()
            .expect("prompt overlay should stay open")
            .shortcut_help_open
    );
}

#[test]
fn mouse_click_inside_shortcut_help_does_not_close_it() {
    let mut model = ready_model();
    model.set_window(140, 16);
    model.open_prompt_overlay();
    let _ = model.handle_prompt_overlay_key(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::SHIFT));

    let buffer = render_model_buffer(&mut model, 140, 16);
    let (column, row) =
        find_buffer_text_position(&buffer, "? / Esc").expect("shortcut help footer should render");
    click_left(&mut model, column, row);

    assert!(
        model
            .prompt_overlay
            .as_ref()
            .expect("prompt overlay should stay open")
            .shortcut_help_open
    );
}

#[test]
fn mouse_click_outside_shortcut_help_closes_it_and_continues_click_action() {
    let mut model = ready_model();
    model.set_window(140, 16);
    model.open_prompt_overlay();
    let _ = model.handle_prompt_overlay_key(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::SHIFT));

    let buffer = render_model_buffer(&mut model, 140, 16);
    let (column, row) = find_buffer_text_position(&buffer, "repo-bootstrap")
        .expect("second discovered skill should render");
    click_left(&mut model, column, row);

    let state = model
        .prompt_overlay
        .as_ref()
        .expect("prompt overlay should stay open");
    assert!(!state.shortcut_help_open);
    assert_eq!(state.focus, super::PromptOverlayFocus::Inactive);
    assert_eq!(
        state.inactive_selected_row_id.as_deref(),
        Some("skill:repo-bootstrap:project")
    );
}

#[test]
fn prompt_overlay_close_shows_toast_for_next_new_session_notice() {
    let mut model = ready_model();
    model.open_prompt_overlay();

    model.apply_runtime_event(
        runtime_domain::session::RuntimeEvent::PromptAssemblyUpdated {
            manager: model.prompt_assembly.clone(),
            notice: Some(PromptAssemblyUpdateNotice::NextNewSessionUpdated),
        },
    );

    assert_eq!(model.active_toast_text_for_test(), None);

    assert_eq!(
        model.handle_prompt_overlay_key(KeyEvent::from(KeyCode::Esc)),
        super::OverlayInputResult::Handled
    );

    assert_eq!(
        model.active_toast_text_for_test(),
        Some("Prompt updated. Applies to next new session.")
    );
    assert!(
        !model
            .transcript_plain_items()
            .iter()
            .any(|item| item.contains("Prompt updated for current empty session."))
    );
}
