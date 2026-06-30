use crossterm::event::{KeyCode, KeyEvent};
use runtime_domain::prompt_assembly::{
    PromptAssemblyDiscoveredSkill, PromptAssemblyLifecycle, PromptAssemblyManagerSnapshot,
    PromptAssemblyMoveDirection, PromptAssemblySnapshot, PromptPreludeSnapshot,
    PromptSourceInactiveReason, PromptSourceKind, PromptSourceOrigin, PromptSourceStatus,
    ResolvedPromptSource,
};

use crate::{
    AppEffect, AppEvent, Model, ModelOptions, StartupBannerOptions,
    modal_layer::ModalLayer,
    runtime::RuntimeEventApply,
    test_helpers::{render_model_buffer, rendered_rows},
    theme::default_palette,
};

use super::{PromptOverlayInactiveTab, prompt_overlay_inactive_rendered_rows};

fn prompt_source(
    reference_id: &str,
    title: &str,
    kind: PromptSourceKind,
    origin: Option<PromptSourceOrigin>,
    status: PromptSourceStatus,
) -> ResolvedPromptSource {
    ResolvedPromptSource {
        reference_id: reference_id.to_string(),
        title: title.to_string(),
        kind,
        origin,
        status,
    }
}

fn prompt_snapshot() -> PromptAssemblySnapshot {
    PromptAssemblySnapshot {
        lifecycle: PromptAssemblyLifecycle::NextNewSession,
        active_sources: vec![
            prompt_source(
                "core-system",
                "Core system prompt",
                PromptSourceKind::CoreSystemPrompt,
                Some(PromptSourceOrigin::Builtin),
                PromptSourceStatus::Active { order: 0 },
            ),
            prompt_source(
                "repo-rules",
                "repo-rules",
                PromptSourceKind::ExtraPrompt,
                Some(PromptSourceOrigin::Project),
                PromptSourceStatus::Active { order: 1 },
            ),
        ],
        inactive_sources: vec![
            prompt_source(
                "disabled-extra",
                "disabled-extra",
                PromptSourceKind::ExtraPrompt,
                Some(PromptSourceOrigin::Project),
                PromptSourceStatus::Inactive {
                    reason: PromptSourceInactiveReason::Disabled,
                },
            ),
            prompt_source(
                "missing-discovery",
                "missing-discovery",
                PromptSourceKind::SkillDiscovery,
                None,
                PromptSourceStatus::Inactive {
                    reason: PromptSourceInactiveReason::Missing,
                },
            ),
            prompt_source(
                "shadowed-skill",
                "shadowed-skill",
                PromptSourceKind::LongLivedSkill,
                Some(PromptSourceOrigin::Global),
                PromptSourceStatus::Inactive {
                    reason: PromptSourceInactiveReason::Shadowed,
                },
            ),
        ],
    }
}

fn ready_model() -> Model {
    let mut model = Model::new_with_options(
        StartupBannerOptions::default(),
        ModelOptions {
            prompt_assembly: Some(PromptAssemblyManagerSnapshot {
                snapshot: prompt_snapshot(),
                prelude: PromptPreludeSnapshot::default(),
                sources: Vec::new(),
                discovered_skills: vec![PromptAssemblyDiscoveredSkill {
                    skill_name: "repo-bootstrap".to_string(),
                    title: "repo-bootstrap".to_string(),
                    description: "Bootstrap repo".to_string(),
                    origin: PromptSourceOrigin::Project,
                    skill_path: "/tmp/repo-bootstrap/SKILL.md".to_string(),
                    body: "# Repo Bootstrap\n\nUse this skill.".to_string(),
                }],
                manual_skills: vec![PromptAssemblyDiscoveredSkill {
                    skill_name: "repo-bootstrap".to_string(),
                    title: "repo-bootstrap".to_string(),
                    description: "Bootstrap repo".to_string(),
                    origin: PromptSourceOrigin::Project,
                    skill_path: "/tmp/repo-bootstrap/SKILL.md".to_string(),
                    body: "# Repo Bootstrap\n\nUse this skill.".to_string(),
                }],
                builtin_core_system_body: "builtin core".to_string(),
                global_core_system_override: None,
                project_core_system_override: None,
            }),
            ..ModelOptions::default()
        },
    );
    model.set_window(90, 16);
    model.set_palette(default_palette(), true);
    model
}

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
fn prompt_command_opens_fullscreen_overlay_and_blocks_composer_input() {
    let mut model = ready_model();
    model.composer_mut().set_text_for_test("/prompt");
    model.sync_command_panel_navigation();

    let effect = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Enter)));

    assert_eq!(effect, None);
    assert_eq!(model.top_modal_layer(), Some(ModalLayer::PromptOverlay));
    assert!(model.blocks_composer_input());
    assert_eq!(model.composer_text(), "");
}

#[test]
fn tab_switches_inactive_family_filter() {
    let mut model = ready_model();
    model.open_prompt_overlay();

    let all_ids = model
        .prompt_overlay_inactive_sources_for_tab(PromptOverlayInactiveTab::All)
        .into_iter()
        .map(|source| source.reference_id.clone())
        .collect::<Vec<_>>();
    assert_eq!(
        all_ids,
        vec![
            "disabled-extra".to_string(),
            "missing-discovery".to_string(),
            "shadowed-skill".to_string()
        ]
    );

    let _ = model.handle_prompt_overlay_key(KeyEvent::from(KeyCode::Tab));
    assert_eq!(
        model
            .prompt_overlay
            .as_ref()
            .map(|state| state.inactive_tab),
        Some(PromptOverlayInactiveTab::ExtraPrompts)
    );
    let extra_ids = model
        .prompt_overlay_inactive_sources_for_tab(PromptOverlayInactiveTab::ExtraPrompts)
        .into_iter()
        .map(|source| source.reference_id.clone())
        .collect::<Vec<_>>();
    assert_eq!(extra_ids, vec!["disabled-extra".to_string()]);
}

#[test]
fn skills_tab_uses_discovered_skill_inventory() {
    let mut model = ready_model();
    model.open_prompt_overlay();

    let _ = model.handle_prompt_overlay_key(KeyEvent::from(KeyCode::Tab));
    let _ = model.handle_prompt_overlay_key(KeyEvent::from(KeyCode::Tab));
    let _ = model.handle_prompt_overlay_key(KeyEvent::from(KeyCode::Tab));

    assert_eq!(
        model
            .prompt_overlay
            .as_ref()
            .map(|state| state.inactive_tab),
        Some(PromptOverlayInactiveTab::LongLivedSkills)
    );
    assert_eq!(
        model.prompt_overlay_inactive_source_count(PromptOverlayInactiveTab::LongLivedSkills),
        1
    );
    assert_eq!(
        model.prompt_assembly.discovered_skills[0].skill_name,
        "repo-bootstrap"
    );
}

#[test]
fn inactive_rows_preserve_disabled_missing_shadowed_grouping() {
    let model = ready_model();
    let filtered = model.prompt_overlay_inactive_sources_for_tab(PromptOverlayInactiveTab::All);
    let rows = prompt_overlay_inactive_rendered_rows(&filtered);

    let labels = rows
        .iter()
        .filter_map(|row| match row {
            super::PromptOverlayRenderedRow::GroupHeader(reason) => Some(*reason),
            super::PromptOverlayRenderedRow::Source(_) => None,
        })
        .collect::<Vec<_>>();

    assert_eq!(
        labels,
        vec![
            PromptSourceInactiveReason::Disabled,
            PromptSourceInactiveReason::Missing,
            PromptSourceInactiveReason::Shadowed,
        ]
    );
}

#[test]
fn render_shows_active_and_inactive_panes() {
    let mut model = ready_model();
    model.open_prompt_overlay();

    let rows = rendered_rows(&render_model_buffer(&mut model, 90, 16)).join("\n");

    assert!(rows.contains("Active Sources"));
    assert!(rows.contains("Inactive Sources"));
    assert!(rows.contains("Disabled"));
    assert!(rows.contains("[All]"));
}

#[test]
fn space_opens_prompt_source_preview() {
    let mut model = ready_model();
    model.prompt_assembly.sources = vec![
        runtime_domain::prompt_assembly::PromptAssemblyManagerSource {
            reference_id: "core-system".to_string(),
            kind: PromptSourceKind::CoreSystemPrompt,
            title: "Core system prompt".to_string(),
            origin: Some(PromptSourceOrigin::Builtin),
            resolved_body_origin: Some(PromptSourceOrigin::Builtin),
            body: Some("# Core\n\nHello".to_string()),
        },
    ];
    model.open_prompt_overlay();

    let result = model.handle_prompt_overlay_key(KeyEvent::from(KeyCode::Char(' ')));

    assert_eq!(result, super::OverlayInputResult::Handled);
    assert!(model.prompt_overlay_preview_active());
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

    model.apply_runtime_event(
        runtime_domain::session::RuntimeEvent::PromptAssemblyUpdated {
            manager: PromptAssemblyManagerSnapshot {
                snapshot: next_snapshot,
                prelude: PromptPreludeSnapshot::default(),
                sources: Vec::new(),
                discovered_skills: Vec::new(),
                manual_skills: Vec::new(),
                builtin_core_system_body: "builtin core".to_string(),
                global_core_system_override: None,
                project_core_system_override: Some("project core".to_string()),
            },
        },
    );

    assert_eq!(
        model.prompt_assembly.snapshot.active_sources[0].origin,
        Some(PromptSourceOrigin::Project)
    );
}

#[test]
fn delete_selected_extra_prompt_emits_mutation_effect() {
    let mut model = ready_model();
    model.open_prompt_overlay();
    model.set_prompt_overlay_focus(super::PromptOverlayFocus::Inactive);
    let _ = model.handle_prompt_overlay_key(KeyEvent::from(KeyCode::Char('d')));

    assert_eq!(
        model.handle_prompt_overlay_key(KeyEvent::from(KeyCode::Char('d'))),
        super::OverlayInputResult::Effect(AppEffect::MutatePromptAssembly {
            mutation: runtime_domain::prompt_assembly::PromptAssemblyMutation::RemovePromptSource {
                scope: runtime_domain::prompt_assembly::persistence::PromptAssemblyScope::Project,
                kind: PromptSourceKind::ExtraPrompt,
                reference_id: "disabled-extra".to_string(),
            },
        })
    );
}

#[test]
fn discovered_skill_activation_emits_project_scoped_mutation() {
    let mut model = ready_model();
    model.open_prompt_overlay();
    model.set_prompt_overlay_focus(super::PromptOverlayFocus::Inactive);
    let _ = model.handle_prompt_overlay_key(KeyEvent::from(KeyCode::Tab));
    let _ = model.handle_prompt_overlay_key(KeyEvent::from(KeyCode::Tab));
    let _ = model.handle_prompt_overlay_key(KeyEvent::from(KeyCode::Tab));

    assert_eq!(
        model.handle_prompt_overlay_key(KeyEvent::from(KeyCode::Char('i'))),
        super::OverlayInputResult::Effect(AppEffect::MutatePromptAssembly {
            mutation:
                runtime_domain::prompt_assembly::PromptAssemblyMutation::ActivateLongLivedSkill {
                    scope:
                        runtime_domain::prompt_assembly::persistence::PromptAssemblyScope::Project,
                    skill_name: "repo-bootstrap".to_string(),
                },
        })
    );
}

#[test]
fn removing_active_long_lived_skill_emits_generic_remove_mutation() {
    let mut model = ready_model();
    model
        .prompt_assembly
        .snapshot
        .active_sources
        .push(prompt_source(
            "code-review",
            "code-review",
            PromptSourceKind::LongLivedSkill,
            Some(PromptSourceOrigin::Project),
            PromptSourceStatus::Active { order: 2 },
        ));
    model.open_prompt_overlay();
    let _ = model.handle_prompt_overlay_key(KeyEvent::from(KeyCode::Down));
    let _ = model.handle_prompt_overlay_key(KeyEvent::from(KeyCode::Down));

    assert_eq!(
        model.handle_prompt_overlay_key(KeyEvent::from(KeyCode::Char('d'))),
        super::OverlayInputResult::Effect(AppEffect::MutatePromptAssembly {
            mutation: runtime_domain::prompt_assembly::PromptAssemblyMutation::RemovePromptSource {
                scope: runtime_domain::prompt_assembly::persistence::PromptAssemblyScope::Project,
                kind: PromptSourceKind::LongLivedSkill,
                reference_id: "code-review".to_string(),
            },
        })
    );
}

#[test]
fn moving_active_source_emits_reorder_mutation() {
    let mut model = ready_model();
    model.open_prompt_overlay();
    let _ = model.handle_prompt_overlay_key(KeyEvent::from(KeyCode::Down));

    assert_eq!(
        model.handle_prompt_overlay_key(KeyEvent::from(KeyCode::Char('J'))),
        super::OverlayInputResult::Effect(AppEffect::MutatePromptAssembly {
            mutation: runtime_domain::prompt_assembly::PromptAssemblyMutation::MoveActiveSource {
                scope: runtime_domain::prompt_assembly::persistence::PromptAssemblyScope::Project,
                kind: PromptSourceKind::ExtraPrompt,
                reference_id: "repo-rules".to_string(),
                direction: PromptAssemblyMoveDirection::Down,
            },
        })
    );
}
