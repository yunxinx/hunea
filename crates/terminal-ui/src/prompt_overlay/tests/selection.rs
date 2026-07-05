use super::*;

#[test]
fn prompt_overlay_candidate_group_helpers_tolerate_empty_groups() {
    assert!(super::prompt_overlay_partition_extra_candidates(Vec::new()).is_none());
    assert!(super::prompt_overlay_extra_candidate_winner(&[]).is_none());
    assert!(super::prompt_overlay_partition_discovered_skills(Vec::new()).is_none());
    assert!(super::prompt_overlay_discovered_skill_winner(&[]).is_none());
}

#[test]
fn skills_tab_uses_discovered_skill_inventory() {
    let mut model = ready_model();
    model.open_prompt_overlay();
    model.set_prompt_overlay_focus(super::PromptOverlayFocus::Inactive);

    assert_eq!(
        model
            .prompt_overlay
            .as_ref()
            .map(|state| state.inactive_tab),
        Some(PromptOverlayInactiveTab::LongLivedSkills)
    );
    assert_eq!(
        model.prompt_overlay_inactive_source_count(PromptOverlayInactiveTab::LongLivedSkills),
        2
    );
    assert_eq!(
        model.prompt_assembly.candidates.discovered_skills[0].skill_name,
        "repo-bootstrap"
    );
}

#[test]
fn extra_tab_filters_to_extra_candidates_only() {
    let model = ready_model();
    let source_ids = model
        .prompt_assembly
        .candidates
        .extra_prompts
        .iter()
        .map(|source| source.reference_id.clone())
        .collect::<Vec<_>>();

    assert_eq!(source_ids, vec!["global-extra".to_string()]);
}

#[test]
fn ctrl_e_expands_shadowed_detail_under_selected_winner() {
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

    let collapsed_rows = rendered_rows(&render_model_buffer(&mut model, 120, 16)).join("\n");
    assert!(collapsed_rows.contains("+1 shadowed"));
    assert!(!collapsed_rows.contains("shadowed global"));

    let _ = model.handle_prompt_overlay_key(KeyEvent::new(
        KeyCode::Char('e'),
        crossterm::event::KeyModifiers::CONTROL,
    ));
    let expanded_rows = rendered_rows(&render_model_buffer(&mut model, 120, 16)).join("\n");
    assert!(expanded_rows.contains("shadowed global"));
}

#[test]
fn ctrl_e_expands_shadowed_extra_candidate_under_winner() {
    let mut model = ready_model();
    model
        .prompt_assembly
        .candidates
        .extra_prompts
        .push(PromptAssemblyExtraPromptCandidate {
            reference_id: "global-extra".to_string(),
            title: "global-extra".to_string(),
            origin: PromptSourceOrigin::Project,
            body: "# Project Extra\n".to_string(),
            selected: true,
        });
    model.open_prompt_overlay();
    model.set_prompt_overlay_focus(super::PromptOverlayFocus::Inactive);
    let _ = model.handle_prompt_overlay_key(KeyEvent::from(KeyCode::Tab));

    let collapsed_rows = rendered_rows(&render_model_buffer(&mut model, 120, 16)).join("\n");
    assert!(collapsed_rows.contains("+1 shadowed"));
    assert!(!collapsed_rows.contains("shadowed global"));

    let _ = model.handle_prompt_overlay_key(KeyEvent::new(
        KeyCode::Char('e'),
        crossterm::event::KeyModifiers::CONTROL,
    ));
    let expanded_rows = rendered_rows(&render_model_buffer(&mut model, 120, 16)).join("\n");
    assert!(expanded_rows.contains("shadowed global"));
}

#[test]
fn ctrl_e_expands_shadowed_skill_under_winner() {
    let mut model = ready_model();
    model.prompt_assembly.candidates.discovered_skills = vec![
        PromptAssemblyDiscoveredSkill {
            skill_name: "repo-bootstrap".to_string(),
            title: "repo-bootstrap".to_string(),
            description: "Project bootstrap".to_string(),
            origin: PromptSourceOrigin::Project,
            selection_scope: PromptAssemblyScope::Project,
            skill_path: "/tmp/project-repo-bootstrap/SKILL.md".to_string(),
            body: "# Project Repo Bootstrap\n".to_string(),
            can_select_for_discovery: true,
            selected: true,
            selected_order: Some(1),
        },
        PromptAssemblyDiscoveredSkill {
            skill_name: "repo-bootstrap".to_string(),
            title: "repo-bootstrap".to_string(),
            description: "Global bootstrap".to_string(),
            origin: PromptSourceOrigin::Global,
            selection_scope: PromptAssemblyScope::Project,
            skill_path: "/tmp/global-repo-bootstrap/SKILL.md".to_string(),
            body: "# Global Repo Bootstrap\n".to_string(),
            can_select_for_discovery: true,
            selected: false,
            selected_order: Some(1),
        },
    ];
    model.open_prompt_overlay();
    model.set_prompt_overlay_focus(super::PromptOverlayFocus::Inactive);

    let collapsed_rows = rendered_rows(&render_model_buffer(&mut model, 120, 16)).join("\n");
    assert!(collapsed_rows.contains("+1 shadowed"));
    assert!(!collapsed_rows.contains("shadowed global"));

    let _ = model.handle_prompt_overlay_key(KeyEvent::new(
        KeyCode::Char('e'),
        crossterm::event::KeyModifiers::CONTROL,
    ));
    let expanded_rows = rendered_rows(&render_model_buffer(&mut model, 120, 16)).join("\n");
    assert!(expanded_rows.contains("shadowed global"));
}

#[test]
fn create_extra_prompt_uses_next_numbered_default_title() {
    let mut model = ready_model();
    model
        .prompt_assembly
        .candidates
        .extra_prompts
        .push(PromptAssemblyExtraPromptCandidate {
            reference_id: "new-prompt-1".to_string(),
            title: "New prompt 1".to_string(),
            origin: PromptSourceOrigin::Project,
            body: "# New prompt 1\n".to_string(),
            selected: false,
        });

    model.open_prompt_overlay();
    model.set_prompt_overlay_focus(super::PromptOverlayFocus::Inactive);
    let _ = model.handle_prompt_overlay_key(KeyEvent::from(KeyCode::Tab));

    assert_eq!(
        model.handle_prompt_overlay_key(KeyEvent::from(KeyCode::Char('a'))),
        super::OverlayInputResult::Handled
    );
    assert_eq!(
        model.handle_prompt_overlay_key(KeyEvent::from(KeyCode::Enter)),
        super::OverlayInputResult::Effect(AppEffect::MutatePromptAssembly {
            mutation: PromptAssemblyMutation::scoped(
                PromptAssemblyScope::Project,
                PromptAssemblyScopedMutationKind::CreateExtraPrompt {
                    content: "# New prompt 2\n".to_string(),
                },
            ),
        })
    );
}

#[test]
fn manual_only_skill_does_not_emit_selection_mutation() {
    let mut model = ready_model();
    model.prompt_assembly.candidates.discovered_skills = vec![PromptAssemblyDiscoveredSkill {
        skill_name: "ask-matt".to_string(),
        title: "ask-matt".to_string(),
        description: "Ask which skill fits".to_string(),
        origin: PromptSourceOrigin::Project,
        selection_scope: PromptAssemblyScope::Project,
        skill_path: "/tmp/ask-matt/SKILL.md".to_string(),
        body: "# Ask Matt".to_string(),
        can_select_for_discovery: false,
        selected: false,
        selected_order: None,
    }];
    model.open_prompt_overlay();
    model.set_prompt_overlay_focus(super::PromptOverlayFocus::Inactive);

    assert_eq!(
        model.handle_prompt_overlay_key(KeyEvent::from(KeyCode::Char('i'))),
        super::OverlayInputResult::Handled
    );
}

#[test]
fn manual_only_skills_sort_after_discovery_eligible_skills() {
    let mut model = ready_model();
    model.prompt_assembly.candidates.discovered_skills = vec![
        PromptAssemblyDiscoveredSkill {
            skill_name: "aaa-discovery".to_string(),
            title: "aaa-discovery".to_string(),
            description: "discovery".to_string(),
            origin: PromptSourceOrigin::Project,
            selection_scope: PromptAssemblyScope::Project,
            skill_path: "/tmp/aaa-discovery/SKILL.md".to_string(),
            body: "# aaa-discovery".to_string(),
            can_select_for_discovery: true,
            selected: true,
            selected_order: Some(1),
        },
        PromptAssemblyDiscoveredSkill {
            skill_name: "zzz-manual".to_string(),
            title: "zzz-manual".to_string(),
            description: "manual".to_string(),
            origin: PromptSourceOrigin::Project,
            selection_scope: PromptAssemblyScope::Project,
            skill_path: "/tmp/zzz-manual/SKILL.md".to_string(),
            body: "# zzz-manual".to_string(),
            can_select_for_discovery: false,
            selected: false,
            selected_order: None,
        },
    ];

    assert_eq!(
        model.prompt_assembly.candidates.discovered_skills[0].skill_name,
        "aaa-discovery"
    );
    assert_eq!(
        model.prompt_assembly.candidates.discovered_skills[1].skill_name,
        "zzz-manual"
    );
}

#[test]
fn prompt_runtime_update_replaces_manager_snapshot() {
    let mut model = ready_model();
    let next_snapshot = PromptAssemblySnapshot {
        lifecycle: PromptAssemblyLifecycle::NextNewSession,
        active_sources: vec![prompt_source(
            "core-system",
            "Core system prompt",
            PromptSourceKind::CoreSystemPrompt,
            Some(PromptSourceOrigin::Project),
            PromptSourceStatus::Active { order: 0 },
        )],
        inactive_sources: Vec::new(),
    };
    let mut manager = PromptAssemblyManagerSnapshot::default();
    manager.resolution.assembly = next_snapshot;
    manager.core_system.builtin_body = "builtin core".to_string();
    manager.core_system.project_override = Some("project core".to_string());

    model.apply_runtime_event(
        runtime_domain::session::RuntimeEvent::PromptAssemblyUpdated {
            manager,
            notice: None,
        },
    );

    assert_eq!(
        model.prompt_assembly.resolution.assembly.active_sources[0].origin,
        Some(PromptSourceOrigin::Project)
    );
}

#[test]
fn active_selection_follows_reordered_source_after_runtime_update() {
    let mut model = ready_model();
    model.open_prompt_overlay();
    let _ = model.handle_prompt_overlay_key(KeyEvent::from(KeyCode::Down));
    let _ = model.handle_prompt_overlay_key(KeyEvent::from(KeyCode::Down));
    assert_eq!(
        model
            .selected_prompt_overlay_managed_source()
            .as_ref()
            .map(|source| source.reference_id.as_str()),
        Some("repo-rules")
    );
    let mut manager = model.prompt_assembly.clone();
    manager.sources.managed = vec![
        PromptAssemblyManagedSource {
            reference_id: "core-system".to_string(),
            kind: PromptSourceKind::CoreSystemPrompt,
            title: "Core system prompt".to_string(),
            origin: Some(PromptSourceOrigin::Builtin),
            scope: None,
            enabled: true,
            order: 1,
        },
        PromptAssemblyManagedSource {
            reference_id: "skill-discovery".to_string(),
            kind: PromptSourceKind::SkillDiscovery,
            title: "Skill discovery".to_string(),
            origin: Some(PromptSourceOrigin::Project),
            scope: Some(PromptAssemblyScope::Project),
            enabled: true,
            order: 2,
        },
        PromptAssemblyManagedSource {
            reference_id: "safety-policy".to_string(),
            kind: PromptSourceKind::ExtraPrompt,
            title: "safety-policy".to_string(),
            origin: Some(PromptSourceOrigin::Global),
            scope: Some(PromptAssemblyScope::Global),
            enabled: false,
            order: 3,
        },
        PromptAssemblyManagedSource {
            reference_id: "repo-rules".to_string(),
            kind: PromptSourceKind::ExtraPrompt,
            title: "repo-rules".to_string(),
            origin: Some(PromptSourceOrigin::Project),
            scope: Some(PromptAssemblyScope::Project),
            enabled: true,
            order: 4,
        },
    ];

    model.apply_runtime_event(
        runtime_domain::session::RuntimeEvent::PromptAssemblyUpdated {
            manager,
            notice: None,
        },
    );

    assert_eq!(
        model
            .selected_prompt_overlay_managed_source()
            .as_ref()
            .map(|source| source.reference_id.as_str()),
        Some("repo-rules")
    );
    assert_eq!(
        model
            .prompt_overlay
            .as_ref()
            .map(|state| state.active_selected),
        Some(3)
    );
}

#[test]
fn deleting_modified_extra_prompt_opens_confirmation_dialog_first() {
    let mut model = ready_model();
    model.open_prompt_overlay();
    model.set_prompt_overlay_focus(super::PromptOverlayFocus::Inactive);
    let _ = model.handle_prompt_overlay_key(KeyEvent::from(KeyCode::Tab));

    assert_eq!(
        model.handle_prompt_overlay_key(KeyEvent::from(KeyCode::Char('d'))),
        super::OverlayInputResult::Handled
    );

    let buffer = render_model_buffer(&mut model, 100, 16);
    let rows = rendered_rows(&buffer);
    let joined = rows.join("\n");
    let message_row_index = rows
        .iter()
        .position(|row| row.contains("Delete global-extra permanently?"))
        .expect("delete confirmation message should render");
    let footer_row_index = rows
        .iter()
        .position(|row| row.contains("Enter confirm") && row.contains("Esc cancel"))
        .expect("delete confirmation footer should render");
    let message_row = &rows[message_row_index];
    let title_byte_index = message_row
        .find("global-extra")
        .expect("delete confirmation should render prompt title");
    let title_column = u16::try_from(message_row[..title_byte_index].chars().count())
        .expect("title column should fit in u16");
    let title_row = u16::try_from(message_row_index).expect("title row should fit in u16");

    assert_text_cells_use_color_at(
        &buffer,
        "global-extra",
        title_row,
        title_column,
        default_palette().command_accent,
    );
    assert!(
        footer_row_index >= message_row_index + 2,
        "delete confirmation should keep a blank line before the footer: rows={rows:?}"
    );
    assert!(joined.contains("Delete custom prompt"));
    assert!(joined.contains("global-extra"));
    assert!(joined.contains("Enter confirm"));
    assert!(joined.contains("Esc cancel"));
}

#[test]
fn deleting_modified_extra_prompt_confirms_on_enter() {
    let mut model = ready_model();
    model.open_prompt_overlay();
    model.set_prompt_overlay_focus(super::PromptOverlayFocus::Inactive);
    let _ = model.handle_prompt_overlay_key(KeyEvent::from(KeyCode::Tab));
    let _ = model.handle_prompt_overlay_key(KeyEvent::from(KeyCode::Char('d')));

    assert_eq!(
        model.handle_prompt_overlay_key(KeyEvent::from(KeyCode::Enter)),
        super::OverlayInputResult::Effect(AppEffect::MutatePromptAssembly {
            mutation: PromptAssemblyMutation::scoped(
                PromptAssemblyScope::Global,
                PromptAssemblyScopedMutationKind::DeleteExtraPrompt {
                    reference_id: "global-extra".to_string(),
                },
            ),
        })
    );
}

#[test]
fn deleting_modified_extra_prompt_can_cancel_confirmation() {
    let mut model = ready_model();
    model.open_prompt_overlay();
    model.set_prompt_overlay_focus(super::PromptOverlayFocus::Inactive);
    let _ = model.handle_prompt_overlay_key(KeyEvent::from(KeyCode::Tab));
    let _ = model.handle_prompt_overlay_key(KeyEvent::from(KeyCode::Char('d')));

    assert_eq!(
        model.handle_prompt_overlay_key(KeyEvent::from(KeyCode::Esc)),
        super::OverlayInputResult::Handled
    );
    assert_eq!(
        model
            .prompt_overlay
            .as_ref()
            .and_then(|state| state.dialog.as_ref()),
        None
    );
}

#[test]
fn deleting_default_template_extra_prompt_also_opens_confirmation_dialog() {
    let mut model = ready_model();
    model.prompt_assembly.candidates.extra_prompts[0].title = "new-prompt-1".to_string();
    model.prompt_assembly.candidates.extra_prompts[0].body = "# New prompt 1\n".to_string();
    model.open_prompt_overlay();
    model.set_prompt_overlay_focus(super::PromptOverlayFocus::Inactive);
    let _ = model.handle_prompt_overlay_key(KeyEvent::from(KeyCode::Tab));

    assert_eq!(
        model.handle_prompt_overlay_key(KeyEvent::from(KeyCode::Char('d'))),
        super::OverlayInputResult::Handled
    );

    let rows = rendered_rows(&render_model_buffer(&mut model, 100, 16)).join("\n");
    assert!(rows.contains("Delete custom prompt"));
    assert!(rows.contains("Enter confirm"));
}

#[test]
fn d_does_not_remove_discovered_skill() {
    let mut model = ready_model();
    model.open_prompt_overlay();
    model.set_prompt_overlay_focus(super::PromptOverlayFocus::Inactive);

    assert_eq!(
        model.handle_prompt_overlay_key(KeyEvent::from(KeyCode::Char('d'))),
        super::OverlayInputResult::Handled
    );
}

#[test]
fn removing_active_disabled_extra_prompt_emits_generic_remove_mutation() {
    let mut model = ready_model();
    model.open_prompt_overlay();
    let _ = model.handle_prompt_overlay_key(KeyEvent::from(KeyCode::Down));
    let _ = model.handle_prompt_overlay_key(KeyEvent::from(KeyCode::Down));
    let _ = model.handle_prompt_overlay_key(KeyEvent::from(KeyCode::Down));

    assert_eq!(
        model.handle_prompt_overlay_key(KeyEvent::from(KeyCode::Char('d'))),
        super::OverlayInputResult::Effect(AppEffect::MutatePromptAssembly {
            mutation: PromptAssemblyMutation::scoped(
                PromptAssemblyScope::Global,
                PromptAssemblyScopedMutationKind::RemovePromptSource {
                    kind: PromptSourceKind::ExtraPrompt,
                    reference_id: "safety-policy".to_string(),
                },
            ),
        })
    );
}

#[test]
fn moving_active_source_emits_reorder_mutation() {
    let mut model = ready_model();
    model.open_prompt_overlay();
    let _ = model.handle_prompt_overlay_key(KeyEvent::from(KeyCode::Down));
    let _ = model.handle_prompt_overlay_key(KeyEvent::from(KeyCode::Down));

    assert_eq!(
        model.handle_prompt_overlay_key(KeyEvent::from(KeyCode::Char('J'))),
        super::OverlayInputResult::Effect(AppEffect::MutatePromptAssembly {
            mutation: PromptAssemblyMutation::scoped(
                PromptAssemblyScope::Project,
                PromptAssemblyScopedMutationKind::MoveActiveSource {
                    kind: PromptSourceKind::ExtraPrompt,
                    reference_id: "repo-rules".to_string(),
                    direction: PromptAssemblyMoveDirection::Down,
                },
            ),
        })
    );
}

#[test]
fn shifted_j_and_k_reorder_active_source() {
    let mut model = ready_model();
    model.open_prompt_overlay();
    let _ = model.handle_prompt_overlay_key(KeyEvent::from(KeyCode::Down));
    let _ = model.handle_prompt_overlay_key(KeyEvent::from(KeyCode::Down));

    assert_eq!(
        model.handle_prompt_overlay_key(KeyEvent::new(
            KeyCode::Char('K'),
            crossterm::event::KeyModifiers::SHIFT,
        )),
        super::OverlayInputResult::Effect(AppEffect::MutatePromptAssembly {
            mutation: PromptAssemblyMutation::scoped(
                PromptAssemblyScope::Project,
                PromptAssemblyScopedMutationKind::MoveActiveSource {
                    kind: PromptSourceKind::ExtraPrompt,
                    reference_id: "repo-rules".to_string(),
                    direction: PromptAssemblyMoveDirection::Up,
                },
            ),
        })
    );

    assert_eq!(
        model.handle_prompt_overlay_key(KeyEvent::new(
            KeyCode::Char('J'),
            crossterm::event::KeyModifiers::SHIFT,
        )),
        super::OverlayInputResult::Effect(AppEffect::MutatePromptAssembly {
            mutation: PromptAssemblyMutation::scoped(
                PromptAssemblyScope::Project,
                PromptAssemblyScopedMutationKind::MoveActiveSource {
                    kind: PromptSourceKind::ExtraPrompt,
                    reference_id: "repo-rules".to_string(),
                    direction: PromptAssemblyMoveDirection::Down,
                },
            ),
        })
    );
}

#[test]
fn moving_tool_guidelines_emits_reorder_mutation_with_managed_scope() {
    let mut model = ready_model();
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
    model.open_prompt_overlay();
    let _ = model.handle_prompt_overlay_key(KeyEvent::from(KeyCode::Down));

    assert_eq!(
        model.handle_prompt_overlay_key(KeyEvent::new(
            KeyCode::Char('J'),
            crossterm::event::KeyModifiers::SHIFT,
        )),
        super::OverlayInputResult::Effect(AppEffect::MutatePromptAssembly {
            mutation: PromptAssemblyMutation::scoped(
                PromptAssemblyScope::Global,
                PromptAssemblyScopedMutationKind::MoveActiveSource {
                    kind: PromptSourceKind::ToolGuidelines,
                    reference_id: "tool-guidelines".to_string(),
                    direction: PromptAssemblyMoveDirection::Down,
                },
            ),
        })
    );
}

#[test]
fn toggling_tool_guidelines_emits_scope_aware_disable_mutation() {
    let mut model = ready_model();
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
    model.open_prompt_overlay();
    let _ = model.handle_prompt_overlay_key(KeyEvent::from(KeyCode::Down));

    assert_eq!(
        model.handle_prompt_overlay_key(KeyEvent::from(KeyCode::Char('x'))),
        super::OverlayInputResult::Effect(AppEffect::MutatePromptAssembly {
            mutation: PromptAssemblyMutation::scoped(
                PromptAssemblyScope::Global,
                PromptAssemblyScopedMutationKind::SetPromptSourceEnabled {
                    kind: PromptSourceKind::ToolGuidelines,
                    reference_id: "tool-guidelines".to_string(),
                    enabled: false,
                },
            ),
        })
    );
}

#[test]
fn restore_hint_only_shows_for_selected_core_system_prompt() {
    let mut model = ready_model();
    model.open_prompt_overlay();

    let core_rows = rendered_rows(&render_model_buffer(&mut model, 90, 16)).join("\n");
    assert!(core_rows.contains("r restore"));

    let _ = model.handle_prompt_overlay_key(KeyEvent::from(KeyCode::Down));
    let non_core_rows = rendered_rows(&render_model_buffer(&mut model, 90, 16)).join("\n");
    assert!(!non_core_rows.contains("r restore"));
}

#[test]
fn x_on_active_non_core_source_emits_disable_mutation() {
    let mut model = ready_model();
    model.open_prompt_overlay();
    let _ = model.handle_prompt_overlay_key(KeyEvent::from(KeyCode::Down));
    let _ = model.handle_prompt_overlay_key(KeyEvent::from(KeyCode::Down));

    assert_eq!(
        model.handle_prompt_overlay_key(KeyEvent::from(KeyCode::Char('x'))),
        super::OverlayInputResult::Effect(AppEffect::MutatePromptAssembly {
            mutation: PromptAssemblyMutation::scoped(
                PromptAssemblyScope::Project,
                PromptAssemblyScopedMutationKind::SetPromptSourceEnabled {
                    kind: PromptSourceKind::ExtraPrompt,
                    reference_id: "repo-rules".to_string(),
                    enabled: false,
                },
            ),
        })
    );
}

#[test]
fn x_on_active_skill_discovery_emits_disable_mutation() {
    let mut model = ready_model();
    model.open_prompt_overlay();
    let _ = model.handle_prompt_overlay_key(KeyEvent::from(KeyCode::Down));

    assert_eq!(
        model.handle_prompt_overlay_key(KeyEvent::from(KeyCode::Char('x'))),
        super::OverlayInputResult::Effect(AppEffect::MutatePromptAssembly {
            mutation: PromptAssemblyMutation::scoped(
                PromptAssemblyScope::Project,
                PromptAssemblyScopedMutationKind::SetPromptSourceEnabled {
                    kind: PromptSourceKind::SkillDiscovery,
                    reference_id: "skill-discovery".to_string(),
                    enabled: false,
                },
            ),
        })
    );
}

#[test]
fn x_does_not_disable_core_system_prompt() {
    let mut model = ready_model();
    model.open_prompt_overlay();

    assert_eq!(
        model.handle_prompt_overlay_key(KeyEvent::from(KeyCode::Char('x'))),
        super::OverlayInputResult::Handled
    );
}

#[test]
fn x_on_active_dynamic_environment_baseline_emits_disable_mutation() {
    let mut model = ready_model();
    model
        .prompt_assembly
        .sources
        .managed
        .insert(1, dynamic_environment_baseline_managed_source());
    model.open_prompt_overlay();
    let _ = model.handle_prompt_overlay_key(KeyEvent::from(KeyCode::Down));

    assert_eq!(
        model.handle_prompt_overlay_key(KeyEvent::from(KeyCode::Char('x'))),
        super::OverlayInputResult::Effect(AppEffect::MutatePromptAssembly {
            mutation: PromptAssemblyMutation::scoped(
                PromptAssemblyScope::Global,
                PromptAssemblyScopedMutationKind::SetPromptSourceEnabled {
                    kind: PromptSourceKind::DynamicEnvironmentBaseline,
                    reference_id: "env-baseline".to_string(),
                    enabled: false,
                },
            ),
        })
    );
}

#[test]
fn moving_dynamic_environment_baseline_emits_reorder_mutation() {
    let mut model = ready_model();
    model
        .prompt_assembly
        .sources
        .managed
        .insert(1, dynamic_environment_baseline_managed_source());
    model.open_prompt_overlay();
    let _ = model.handle_prompt_overlay_key(KeyEvent::from(KeyCode::Down));

    assert_eq!(
        model.handle_prompt_overlay_key(KeyEvent::new(
            KeyCode::Char('J'),
            crossterm::event::KeyModifiers::SHIFT,
        )),
        super::OverlayInputResult::Effect(AppEffect::MutatePromptAssembly {
            mutation: PromptAssemblyMutation::scoped(
                PromptAssemblyScope::Global,
                PromptAssemblyScopedMutationKind::MoveActiveSource {
                    kind: PromptSourceKind::DynamicEnvironmentBaseline,
                    reference_id: "env-baseline".to_string(),
                    direction: PromptAssemblyMoveDirection::Down,
                },
            ),
        })
    );
}

#[test]
fn x_on_discovered_skill_emits_selection_toggle_mutation() {
    let mut model = ready_model();
    model.open_prompt_overlay();
    model.set_prompt_overlay_focus(super::PromptOverlayFocus::Inactive);

    assert_eq!(
        model.handle_prompt_overlay_key(KeyEvent::from(KeyCode::Char('x'))),
        super::OverlayInputResult::Effect(AppEffect::MutatePromptAssembly {
            mutation: PromptAssemblyMutation::scoped(
                PromptAssemblyScope::Project,
                PromptAssemblyScopedMutationKind::SetDiscoveredSkillSelected {
                    skill_name: "repo-bootstrap".to_string(),
                    selected: false,
                },
            ),
        })
    );
}

#[test]
fn d_does_not_remove_active_instruction_file() {
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
    model.open_prompt_overlay();
    let _ = model.handle_prompt_overlay_key(KeyEvent::from(KeyCode::Down));

    assert_eq!(
        model.handle_prompt_overlay_key(KeyEvent::from(KeyCode::Char('d'))),
        super::OverlayInputResult::Handled
    );
}

#[test]
fn shifted_j_and_k_reorder_discovered_skill() {
    let mut model = ready_model();
    model.open_prompt_overlay();
    model.set_prompt_overlay_focus(super::PromptOverlayFocus::Inactive);

    assert_eq!(
        model.handle_prompt_overlay_key(KeyEvent::new(
            KeyCode::Char('K'),
            crossterm::event::KeyModifiers::SHIFT,
        )),
        super::OverlayInputResult::Effect(AppEffect::MutatePromptAssembly {
            mutation: PromptAssemblyMutation::scoped(
                PromptAssemblyScope::Project,
                PromptAssemblyScopedMutationKind::MoveDiscoveredSkill {
                    skill_name: "repo-bootstrap".to_string(),
                    direction: PromptAssemblyMoveDirection::Up,
                },
            ),
        })
    );

    assert_eq!(
        model.handle_prompt_overlay_key(KeyEvent::new(
            KeyCode::Char('J'),
            crossterm::event::KeyModifiers::SHIFT,
        )),
        super::OverlayInputResult::Effect(AppEffect::MutatePromptAssembly {
            mutation: PromptAssemblyMutation::scoped(
                PromptAssemblyScope::Project,
                PromptAssemblyScopedMutationKind::MoveDiscoveredSkill {
                    skill_name: "repo-bootstrap".to_string(),
                    direction: PromptAssemblyMoveDirection::Down,
                },
            ),
        })
    );
}

#[test]
fn r_resets_discovered_skill_order() {
    let mut model = ready_model();
    model.open_prompt_overlay();
    model.set_prompt_overlay_focus(super::PromptOverlayFocus::Inactive);

    assert_eq!(
        model.handle_prompt_overlay_key(KeyEvent::new(
            KeyCode::Char('r'),
            crossterm::event::KeyModifiers::NONE,
        )),
        super::OverlayInputResult::Effect(AppEffect::MutatePromptAssembly {
            mutation: PromptAssemblyMutation::scoped(
                PromptAssemblyScope::Project,
                PromptAssemblyScopedMutationKind::ResetDiscoveredSkillOrder,
            ),
        })
    );
}

#[test]
fn global_discovered_skill_reorder_uses_selection_scope_not_item_origin() {
    let mut model = ready_model();
    model.prompt_assembly.candidates.discovered_skills = vec![PromptAssemblyDiscoveredSkill {
        skill_name: "code-review".to_string(),
        title: "code-review".to_string(),
        description: "Review code".to_string(),
        origin: PromptSourceOrigin::Global,
        selection_scope: PromptAssemblyScope::Project,
        skill_path: "/tmp/code-review/SKILL.md".to_string(),
        body: "# Code Review".to_string(),
        can_select_for_discovery: true,
        selected: true,
        selected_order: Some(1),
    }];
    model.open_prompt_overlay();
    model.set_prompt_overlay_focus(super::PromptOverlayFocus::Inactive);

    assert_eq!(
        model.handle_prompt_overlay_key(KeyEvent::new(
            KeyCode::Char('J'),
            crossterm::event::KeyModifiers::SHIFT,
        )),
        super::OverlayInputResult::Effect(AppEffect::MutatePromptAssembly {
            mutation: PromptAssemblyMutation::scoped(
                PromptAssemblyScope::Project,
                PromptAssemblyScopedMutationKind::MoveDiscoveredSkill {
                    skill_name: "code-review".to_string(),
                    direction: PromptAssemblyMoveDirection::Down,
                },
            ),
        })
    );
}

#[test]
fn global_discovered_skill_reset_uses_selection_scope_not_item_origin() {
    let mut model = ready_model();
    model.prompt_assembly.candidates.discovered_skills = vec![PromptAssemblyDiscoveredSkill {
        skill_name: "code-review".to_string(),
        title: "code-review".to_string(),
        description: "Review code".to_string(),
        origin: PromptSourceOrigin::Global,
        selection_scope: PromptAssemblyScope::Project,
        skill_path: "/tmp/code-review/SKILL.md".to_string(),
        body: "# Code Review".to_string(),
        can_select_for_discovery: true,
        selected: true,
        selected_order: Some(1),
    }];
    model.open_prompt_overlay();
    model.set_prompt_overlay_focus(super::PromptOverlayFocus::Inactive);

    assert_eq!(
        model.handle_prompt_overlay_key(KeyEvent::new(
            KeyCode::Char('r'),
            crossterm::event::KeyModifiers::NONE,
        )),
        super::OverlayInputResult::Effect(AppEffect::MutatePromptAssembly {
            mutation: PromptAssemblyMutation::scoped(
                PromptAssemblyScope::Project,
                PromptAssemblyScopedMutationKind::ResetDiscoveredSkillOrder,
            ),
        })
    );
}
