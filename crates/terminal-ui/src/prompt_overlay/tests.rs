use std::{fs, path::PathBuf};

use crossterm::event::{KeyCode, KeyEvent, MouseButton};
use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Layout, Rect},
    style::Modifier,
};
use runtime_domain::prompt_assembly::{
    PromptAssemblyDiscoveredSkill, PromptAssemblyExtraPromptCandidate, PromptAssemblyLifecycle,
    PromptAssemblyManagedSource, PromptAssemblyManagerSnapshot, PromptAssemblyMoveDirection,
    PromptAssemblyMutation, PromptAssemblySnapshot, PromptPreludeSnapshot,
    PromptSourceInactiveReason, PromptSourceKind, PromptSourceOrigin, PromptSourceStatus,
    ResolvedPromptSource,
};

use crate::{
    AppEffect, AppEvent, Model, ModelOptions, StartupBannerOptions,
    fullscreen_list_chrome::fullscreen_list_chrome_rects,
    modal_layer::ModalLayer,
    runtime::RuntimeEventApply,
    test_helpers::{render_model_buffer, rendered_rows},
    theme::default_palette,
};

use super::PromptOverlayInactiveTab;

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
                "skill-discovery",
                "Skill discovery",
                PromptSourceKind::SkillDiscovery,
                Some(PromptSourceOrigin::Builtin),
                PromptSourceStatus::Active { order: 1 },
            ),
            prompt_source(
                "repo-rules",
                "repo-rules",
                PromptSourceKind::ExtraPrompt,
                Some(PromptSourceOrigin::Project),
                PromptSourceStatus::Active { order: 2 },
            ),
            prompt_source(
                "safety-policy",
                "safety-policy",
                PromptSourceKind::ExtraPrompt,
                Some(PromptSourceOrigin::Global),
                PromptSourceStatus::Inactive {
                    reason: PromptSourceInactiveReason::Disabled,
                },
            ),
        ],
        inactive_sources: vec![
            prompt_source(
                "global-extra",
                "global-extra",
                PromptSourceKind::ExtraPrompt,
                Some(PromptSourceOrigin::Global),
                PromptSourceStatus::Inactive {
                    reason: PromptSourceInactiveReason::Shadowed,
                },
            ),
            prompt_source(
                "global-skill",
                "global-skill",
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
                managed_sources: vec![
                    PromptAssemblyManagedSource {
                        reference_id: "core-system".to_string(),
                        kind: PromptSourceKind::CoreSystemPrompt,
                        title: "Core system prompt".to_string(),
                        origin: Some(PromptSourceOrigin::Builtin),
                        enabled: true,
                        order: 1,
                    },
                    PromptAssemblyManagedSource {
                        reference_id: "skill-discovery".to_string(),
                        kind: PromptSourceKind::SkillDiscovery,
                        title: "Skill discovery".to_string(),
                        origin: Some(PromptSourceOrigin::Builtin),
                        enabled: true,
                        order: 2,
                    },
                    PromptAssemblyManagedSource {
                        reference_id: "repo-rules".to_string(),
                        kind: PromptSourceKind::ExtraPrompt,
                        title: "repo-rules".to_string(),
                        origin: Some(PromptSourceOrigin::Project),
                        enabled: true,
                        order: 3,
                    },
                    PromptAssemblyManagedSource {
                        reference_id: "safety-policy".to_string(),
                        kind: PromptSourceKind::ExtraPrompt,
                        title: "safety-policy".to_string(),
                        origin: Some(PromptSourceOrigin::Global),
                        enabled: false,
                        order: 4,
                    },
                ],
                sources: Vec::new(),
                extra_prompt_candidates: vec![PromptAssemblyExtraPromptCandidate {
                    reference_id: "global-extra".to_string(),
                    title: "global-extra".to_string(),
                    origin: PromptSourceOrigin::Global,
                    body: "# Global Extra\n".to_string(),
                    selected: false,
                }],
                discovered_skills: vec![
                    PromptAssemblyDiscoveredSkill {
                        skill_name: "repo-bootstrap".to_string(),
                        title: "repo-bootstrap".to_string(),
                        description: "Bootstrap repo".to_string(),
                        origin: PromptSourceOrigin::Project,
                        skill_path: "/tmp/repo-bootstrap/SKILL.md".to_string(),
                        body: "# Repo Bootstrap\n\nUse this skill.".to_string(),
                        can_select_for_discovery: true,
                        selected: true,
                        selected_order: Some(1),
                    },
                    PromptAssemblyDiscoveredSkill {
                        skill_name: "code-review".to_string(),
                        title: "code-review".to_string(),
                        description: "Review code".to_string(),
                        origin: PromptSourceOrigin::Global,
                        skill_path: "/tmp/code-review/SKILL.md".to_string(),
                        body: "# Code Review\n\nUse this skill.".to_string(),
                        can_select_for_discovery: true,
                        selected: true,
                        selected_order: Some(2),
                    },
                ],
                manual_skills: vec![PromptAssemblyDiscoveredSkill {
                    skill_name: "repo-bootstrap".to_string(),
                    title: "repo-bootstrap".to_string(),
                    description: "Bootstrap repo".to_string(),
                    origin: PromptSourceOrigin::Project,
                    skill_path: "/tmp/repo-bootstrap/SKILL.md".to_string(),
                    body: "# Repo Bootstrap\n\nUse this skill.".to_string(),
                    can_select_for_discovery: true,
                    selected: false,
                    selected_order: None,
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

fn left_source_cell_text(row: &str, width: usize) -> String {
    let source_width = super::prompt_overlay_left_source_width(width);
    let source_start = super::PROMPT_OVERLAY_OUTER_PADDING
        + super::PROMPT_OVERLAY_LEFT_SEL_WIDTH
        + super::PROMPT_OVERLAY_COLUMN_GAP
        + super::PROMPT_OVERLAY_LEFT_ORD_WIDTH
        + super::PROMPT_OVERLAY_COLUMN_GAP;

    row.chars()
        .skip(source_start)
        .take(source_width)
        .collect::<String>()
}

fn find_text_position(rows: &[String], needle: &str) -> Option<(u16, u16)> {
    rows.iter().enumerate().find_map(|(row_index, row)| {
        row.find(needle).map(|byte_index| {
            let column = row[..byte_index].chars().count();
            (
                u16::try_from(column).expect("column should fit in u16"),
                u16::try_from(row_index).expect("row should fit in u16"),
            )
        })
    })
}

fn find_buffer_text_position(buffer: &Buffer, needle: &str) -> Option<(u16, u16)> {
    find_text_position(&rendered_rows(buffer), needle)
}

fn temp_test_file(prefix: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "hunea-prompt-overlay-{prefix}-{}-{}.md",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system time should be after unix epoch")
            .as_nanos()
    ))
}

fn click_left(model: &mut Model, column: u16, row: u16) {
    let effect = model.update(AppEvent::MouseDown {
        button: MouseButton::Left,
        column,
        row,
    });
    assert_eq!(effect, None);
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
        model.prompt_assembly.discovered_skills[0].skill_name,
        "repo-bootstrap"
    );
}

#[test]
fn extra_tab_filters_to_extra_candidates_only() {
    let model = ready_model();
    let source_ids = model
        .prompt_assembly
        .extra_prompt_candidates
        .iter()
        .map(|source| source.reference_id.clone())
        .collect::<Vec<_>>();

    assert_eq!(source_ids, vec!["global-extra".to_string()]);
}

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
        .managed_sources
        .push(PromptAssemblyManagedSource {
            reference_id: "missing-skill".to_string(),
            kind: PromptSourceKind::LongLivedSkill,
            title: "missing-skill".to_string(),
            origin: Some(PromptSourceOrigin::Project),
            enabled: true,
            order: 5,
        });
    model
        .prompt_assembly
        .snapshot
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
    let source = ready_model().prompt_assembly.managed_sources[2].clone();
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
    let source = ready_model().prompt_assembly.managed_sources[2].clone();
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
fn ctrl_e_expands_shadowed_detail_under_selected_winner() {
    let mut model = ready_model();
    model
        .prompt_assembly
        .snapshot
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
        .extra_prompt_candidates
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
    model.prompt_assembly.discovered_skills = vec![
        PromptAssemblyDiscoveredSkill {
            skill_name: "repo-bootstrap".to_string(),
            title: "repo-bootstrap".to_string(),
            description: "Project bootstrap".to_string(),
            origin: PromptSourceOrigin::Project,
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
fn shadowed_detail_row_delete_targets_shadowed_source() {
    let mut model = ready_model();
    model
        .prompt_assembly
        .snapshot
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
            mutation: PromptAssemblyMutation::RemovePromptSource {
                scope: runtime_domain::prompt_assembly::persistence::PromptAssemblyScope::Global,
                kind: PromptSourceKind::ExtraPrompt,
                reference_id: "repo-rules".to_string(),
            },
        })
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
            mutation: PromptAssemblyMutation::CreateExtraPrompt {
                scope: runtime_domain::prompt_assembly::persistence::PromptAssemblyScope::Global,
                content: "# New prompt 1\n".to_string(),
            },
        })
    );
}

#[test]
fn create_extra_prompt_uses_next_numbered_default_title() {
    let mut model = ready_model();
    model
        .prompt_assembly
        .extra_prompt_candidates
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
            mutation: PromptAssemblyMutation::CreateExtraPrompt {
                scope: runtime_domain::prompt_assembly::persistence::PromptAssemblyScope::Project,
                content: "# New prompt 2\n".to_string(),
            },
        })
    );
}

#[test]
fn render_uses_fixed_width_table_columns_with_balanced_split() {
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
        right_pane_width.abs_diff(divider_column) <= 1,
        "left and right panes should stay balanced: left={left_header_pane:?}, right={right_header_pane:?}"
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
fn skills_tab_keeps_sorted_names_after_removing_num_column() {
    let mut model = ready_model();
    model.set_window(120, 20);
    model.prompt_assembly.discovered_skills = vec![
        PromptAssemblyDiscoveredSkill {
            skill_name: "caveman".to_string(),
            title: "caveman".to_string(),
            description: "Be brief".to_string(),
            origin: PromptSourceOrigin::Project,
            skill_path: "/tmp/caveman/SKILL.md".to_string(),
            body: "# caveman".to_string(),
            can_select_for_discovery: true,
            selected: true,
            selected_order: Some(21),
        },
        PromptAssemblyDiscoveredSkill {
            skill_name: "codebase-design".to_string(),
            title: "codebase-design".to_string(),
            description: "Design modules".to_string(),
            origin: PromptSourceOrigin::Project,
            skill_path: "/tmp/codebase-design/SKILL.md".to_string(),
            body: "# codebase-design".to_string(),
            can_select_for_discovery: true,
            selected: false,
            selected_order: Some(8),
        },
        PromptAssemblyDiscoveredSkill {
            skill_name: "ask-matt".to_string(),
            title: "ask-matt".to_string(),
            description: "Ask which skill fits".to_string(),
            origin: PromptSourceOrigin::Project,
            skill_path: "/tmp/ask-matt/SKILL.md".to_string(),
            body: "# ask-matt".to_string(),
            can_select_for_discovery: false,
            selected: false,
            selected_order: None,
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

    assert!(caveman_row < codebase_design_row);
    assert!(codebase_design_row < ask_matt_row);
    assert!(!rows.join("\n").contains(" 21 "));
    assert!(!rows.join("\n").contains("  8 "));
}

#[test]
fn manual_only_skill_stays_visible_with_manual_marker() {
    let mut model = ready_model();
    model
        .prompt_assembly
        .discovered_skills
        .push(PromptAssemblyDiscoveredSkill {
            skill_name: "ask-matt".to_string(),
            title: "ask-matt".to_string(),
            description: "Ask which skill fits".to_string(),
            origin: PromptSourceOrigin::Project,
            skill_path: "/tmp/ask-matt/SKILL.md".to_string(),
            body: "# Ask Matt".to_string(),
            can_select_for_discovery: false,
            selected: false,
            selected_order: None,
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
fn manual_only_skill_does_not_emit_selection_mutation() {
    let mut model = ready_model();
    model.prompt_assembly.discovered_skills = vec![PromptAssemblyDiscoveredSkill {
        skill_name: "ask-matt".to_string(),
        title: "ask-matt".to_string(),
        description: "Ask which skill fits".to_string(),
        origin: PromptSourceOrigin::Project,
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
    model.prompt_assembly.discovered_skills = vec![
        PromptAssemblyDiscoveredSkill {
            skill_name: "aaa-discovery".to_string(),
            title: "aaa-discovery".to_string(),
            description: "discovery".to_string(),
            origin: PromptSourceOrigin::Project,
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
            skill_path: "/tmp/zzz-manual/SKILL.md".to_string(),
            body: "# zzz-manual".to_string(),
            can_select_for_discovery: false,
            selected: false,
            selected_order: None,
        },
    ];

    assert_eq!(
        model.prompt_assembly.discovered_skills[0].skill_name,
        "aaa-discovery"
    );
    assert_eq!(
        model.prompt_assembly.discovered_skills[1].skill_name,
        "zzz-manual"
    );
}

#[test]
fn manual_only_skill_preview_shows_notice_above_body() {
    let mut model = ready_model();
    model.prompt_assembly.discovered_skills = vec![PromptAssemblyDiscoveredSkill {
        skill_name: "ask-matt".to_string(),
        title: "ask-matt".to_string(),
        description: "Ask which skill fits".to_string(),
        origin: PromptSourceOrigin::Project,
        skill_path: "/tmp/ask-matt/SKILL.md".to_string(),
        body: "# Ask Matt".to_string(),
        can_select_for_discovery: false,
        selected: false,
        selected_order: None,
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
fn skills_tab_removes_num_column() {
    let mut model = ready_model();
    model.set_window(120, 16);
    model.open_prompt_overlay();
    model.set_prompt_overlay_focus(super::PromptOverlayFocus::Inactive);

    let rows = rendered_rows(&render_model_buffer(&mut model, 120, 16));
    let right_header = rows
        .iter()
        .find(|row| row.contains("Name") && row.contains("Scope"))
        .expect("right header should render");
    let right_pane = right_header
        .split('│')
        .nth(1)
        .expect("right pane should exist");

    assert!(!right_pane.contains("Ord"));
    assert!(!right_pane.contains("Num"));
}

#[test]
fn empty_extra_candidates_state_aligns_with_sel_column() {
    let mut model = ready_model();
    model.set_window(120, 16);
    model.prompt_assembly.extra_prompt_candidates.clear();
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
    model.prompt_assembly.discovered_skills.clear();
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
    let trailing_index = skill_index + "[Skill] Custom Prompts".chars().count();

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
fn type_column_uses_full_words_and_fits_discovery_label() {
    let mut model = ready_model();
    model.set_window(120, 16);
    model.open_prompt_overlay();

    let rows = rendered_rows(&render_model_buffer(&mut model, 120, 16));
    let skill_discovery_row = rows
        .iter()
        .find(|row| row.contains("Skill discovery") && row.contains("discovery"))
        .expect("skill discovery row should render");

    assert!(
        skill_discovery_row.contains("discovery  builtin"),
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
        Constraint::Percentage(50),
        Constraint::Length(1),
        Constraint::Percentage(50),
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
    model.prompt_assembly.sources = vec![
        runtime_domain::prompt_assembly::PromptAssemblyManagerSource {
            reference_id: "core-system".to_string(),
            kind: PromptSourceKind::CoreSystemPrompt,
            title: "Core system prompt".to_string(),
            origin: Some(PromptSourceOrigin::Builtin),
            resolved_body_origin: Some(PromptSourceOrigin::Builtin),
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
    model.prompt_assembly.sources = vec![
        runtime_domain::prompt_assembly::PromptAssemblyManagerSource {
            reference_id: "core-system".to_string(),
            kind: PromptSourceKind::CoreSystemPrompt,
            title: "Core system prompt".to_string(),
            origin: Some(PromptSourceOrigin::Builtin),
            resolved_body_origin: Some(PromptSourceOrigin::Builtin),
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
                managed_sources: Vec::new(),
                sources: Vec::new(),
                extra_prompt_candidates: Vec::new(),
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
fn custom_prompt_rows_sort_titles_naturally() {
    let mut model = ready_model();
    model.prompt_assembly.extra_prompt_candidates = vec![
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

    model.apply_runtime_event(
        runtime_domain::session::RuntimeEvent::PromptAssemblyUpdated {
            manager: PromptAssemblyManagerSnapshot {
                snapshot: prompt_snapshot(),
                prelude: PromptPreludeSnapshot::default(),
                managed_sources: vec![
                    PromptAssemblyManagedSource {
                        reference_id: "core-system".to_string(),
                        kind: PromptSourceKind::CoreSystemPrompt,
                        title: "Core system prompt".to_string(),
                        origin: Some(PromptSourceOrigin::Builtin),
                        enabled: true,
                        order: 1,
                    },
                    PromptAssemblyManagedSource {
                        reference_id: "skill-discovery".to_string(),
                        kind: PromptSourceKind::SkillDiscovery,
                        title: "Skill discovery".to_string(),
                        origin: Some(PromptSourceOrigin::Builtin),
                        enabled: true,
                        order: 2,
                    },
                    PromptAssemblyManagedSource {
                        reference_id: "safety-policy".to_string(),
                        kind: PromptSourceKind::ExtraPrompt,
                        title: "safety-policy".to_string(),
                        origin: Some(PromptSourceOrigin::Global),
                        enabled: false,
                        order: 3,
                    },
                    PromptAssemblyManagedSource {
                        reference_id: "repo-rules".to_string(),
                        kind: PromptSourceKind::ExtraPrompt,
                        title: "repo-rules".to_string(),
                        origin: Some(PromptSourceOrigin::Project),
                        enabled: true,
                        order: 4,
                    },
                ],
                sources: Vec::new(),
                extra_prompt_candidates: vec![PromptAssemblyExtraPromptCandidate {
                    reference_id: "global-extra".to_string(),
                    title: "global-extra".to_string(),
                    origin: PromptSourceOrigin::Global,
                    body: "# Global Extra\n".to_string(),
                    selected: false,
                }],
                discovered_skills: vec![
                    PromptAssemblyDiscoveredSkill {
                        skill_name: "repo-bootstrap".to_string(),
                        title: "repo-bootstrap".to_string(),
                        description: "Bootstrap repo".to_string(),
                        origin: PromptSourceOrigin::Project,
                        skill_path: "/tmp/repo-bootstrap/SKILL.md".to_string(),
                        body: "# Repo Bootstrap\n\nUse this skill.".to_string(),
                        can_select_for_discovery: true,
                        selected: true,
                        selected_order: Some(1),
                    },
                    PromptAssemblyDiscoveredSkill {
                        skill_name: "code-review".to_string(),
                        title: "code-review".to_string(),
                        description: "Review code".to_string(),
                        origin: PromptSourceOrigin::Global,
                        skill_path: "/tmp/code-review/SKILL.md".to_string(),
                        body: "# Code Review\n\nUse this skill.".to_string(),
                        can_select_for_discovery: true,
                        selected: true,
                        selected_order: Some(2),
                    },
                ],
                manual_skills: vec![PromptAssemblyDiscoveredSkill {
                    skill_name: "repo-bootstrap".to_string(),
                    title: "repo-bootstrap".to_string(),
                    description: "Bootstrap repo".to_string(),
                    origin: PromptSourceOrigin::Project,
                    skill_path: "/tmp/repo-bootstrap/SKILL.md".to_string(),
                    body: "# Repo Bootstrap\n\nUse this skill.".to_string(),
                    can_select_for_discovery: true,
                    selected: false,
                    selected_order: None,
                }],
                builtin_core_system_body: "builtin core".to_string(),
                global_core_system_override: None,
                project_core_system_override: None,
            },
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
fn delete_selected_extra_prompt_emits_mutation_effect() {
    let mut model = ready_model();
    model.open_prompt_overlay();
    model.set_prompt_overlay_focus(super::PromptOverlayFocus::Inactive);
    let _ = model.handle_prompt_overlay_key(KeyEvent::from(KeyCode::Tab));

    assert_eq!(
        model.handle_prompt_overlay_key(KeyEvent::from(KeyCode::Char('d'))),
        super::OverlayInputResult::Effect(AppEffect::MutatePromptAssembly {
            mutation: runtime_domain::prompt_assembly::PromptAssemblyMutation::DeleteExtraPrompt {
                scope: runtime_domain::prompt_assembly::persistence::PromptAssemblyScope::Global,
                reference_id: "global-extra".to_string(),
            },
        })
    );
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
            mutation: runtime_domain::prompt_assembly::PromptAssemblyMutation::RemovePromptSource {
                scope: runtime_domain::prompt_assembly::persistence::PromptAssemblyScope::Global,
                kind: PromptSourceKind::ExtraPrompt,
                reference_id: "safety-policy".to_string(),
            },
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
            mutation: runtime_domain::prompt_assembly::PromptAssemblyMutation::MoveActiveSource {
                scope: runtime_domain::prompt_assembly::persistence::PromptAssemblyScope::Project,
                kind: PromptSourceKind::ExtraPrompt,
                reference_id: "repo-rules".to_string(),
                direction: PromptAssemblyMoveDirection::Down,
            },
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
            mutation: runtime_domain::prompt_assembly::PromptAssemblyMutation::MoveActiveSource {
                scope: runtime_domain::prompt_assembly::persistence::PromptAssemblyScope::Project,
                kind: PromptSourceKind::ExtraPrompt,
                reference_id: "repo-rules".to_string(),
                direction: PromptAssemblyMoveDirection::Up,
            },
        })
    );

    assert_eq!(
        model.handle_prompt_overlay_key(KeyEvent::new(
            KeyCode::Char('J'),
            crossterm::event::KeyModifiers::SHIFT,
        )),
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
            mutation: PromptAssemblyMutation::SetPromptSourceEnabled {
                scope: runtime_domain::prompt_assembly::persistence::PromptAssemblyScope::Project,
                kind: PromptSourceKind::ExtraPrompt,
                reference_id: "repo-rules".to_string(),
                enabled: false,
            },
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
            mutation: PromptAssemblyMutation::SetPromptSourceEnabled {
                scope: runtime_domain::prompt_assembly::persistence::PromptAssemblyScope::Project,
                kind: PromptSourceKind::SkillDiscovery,
                reference_id: "skill-discovery".to_string(),
                enabled: false,
            },
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
fn x_on_discovered_skill_emits_selection_toggle_mutation() {
    let mut model = ready_model();
    model.open_prompt_overlay();
    model.set_prompt_overlay_focus(super::PromptOverlayFocus::Inactive);

    assert_eq!(
        model.handle_prompt_overlay_key(KeyEvent::from(KeyCode::Char('x'))),
        super::OverlayInputResult::Effect(AppEffect::MutatePromptAssembly {
            mutation:
                runtime_domain::prompt_assembly::PromptAssemblyMutation::SetDiscoveredSkillSelected {
                    scope:
                        runtime_domain::prompt_assembly::persistence::PromptAssemblyScope::Global,
                    skill_name: "code-review".to_string(),
                    selected: false,
                },
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
fn unchanged_prompt_overlay_external_editor_exit_does_not_fall_through_to_composer() {
    let mut model = ready_model();
    model.open_prompt_overlay();
    model
        .prompt_overlay
        .as_mut()
        .expect("overlay should open")
        .pending_editor = Some(super::PromptOverlayPendingEditor {
        target: runtime_domain::prompt_assembly::PromptAssemblyEditorTarget::ExtraPrompt {
            scope: runtime_domain::prompt_assembly::persistence::PromptAssemblyScope::Project,
            reference_id: "repo-rules".to_string(),
        },
        original_draft: "# Repo rules\n".to_string(),
    });
    let draft_path = temp_test_file("overlay-editor-unchanged");
    fs::write(&draft_path, "# Repo rules\n").expect("draft file should exist");

    let effect = model.update(AppEvent::ExternalEditorFinished {
        draft_path: draft_path.clone(),
        original_draft: "# Repo rules\n".to_string(),
        failed: false,
    });

    assert_eq!(effect, None);
    assert_eq!(model.active_toast_text_for_test(), None);
    assert_eq!(
        model
            .prompt_overlay
            .as_ref()
            .and_then(|state| state.pending_editor.as_ref()),
        None
    );
}

#[test]
fn changed_prompt_overlay_external_editor_exit_returns_save_mutation() {
    let mut model = ready_model();
    model.open_prompt_overlay();
    model
        .prompt_overlay
        .as_mut()
        .expect("overlay should open")
        .pending_editor = Some(super::PromptOverlayPendingEditor {
        target: runtime_domain::prompt_assembly::PromptAssemblyEditorTarget::ExtraPrompt {
            scope: runtime_domain::prompt_assembly::persistence::PromptAssemblyScope::Project,
            reference_id: "repo-rules".to_string(),
        },
        original_draft: "# Repo rules\n".to_string(),
    });
    let draft_path = temp_test_file("overlay-editor-changed");
    fs::write(&draft_path, "# Repo rules\nUse cargo nextest run.\n")
        .expect("draft file should exist");

    let effect = model.update(AppEvent::ExternalEditorFinished {
        draft_path,
        original_draft: "# Repo rules\n".to_string(),
        failed: false,
    });

    assert_eq!(
        effect,
        Some(AppEffect::MutatePromptAssembly {
            mutation: PromptAssemblyMutation::SaveEditorTarget {
                target: runtime_domain::prompt_assembly::PromptAssemblyEditorTarget::ExtraPrompt {
                    scope:
                        runtime_domain::prompt_assembly::persistence::PromptAssemblyScope::Project,
                    reference_id: "repo-rules".to_string(),
                },
                content: "# Repo rules\nUse cargo nextest run.\n".to_string(),
            },
        })
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
}

#[test]
fn footer_shows_create_prompt_on_empty_custom_tab() {
    let mut model = ready_model();
    model.prompt_assembly.extra_prompt_candidates.clear();
    model.set_window(140, 16);
    model.open_prompt_overlay();
    model.set_prompt_overlay_focus(super::PromptOverlayFocus::Inactive);
    let _ = model.handle_prompt_overlay_key(KeyEvent::from(KeyCode::Tab));

    let rows = rendered_rows(&render_model_buffer(&mut model, 140, 16)).join("\n");

    assert!(rows.contains("a create prompt"));
    assert!(!rows.contains("d remove"));
    assert!(!rows.contains("e/ctrl+g edit"));
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
    assert!(!rows.contains("J/K reorder"));
}
