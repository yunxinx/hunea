use crossterm::event::{KeyCode, KeyEvent};
use runtime_domain::prompt_assembly::{
    PromptAssemblyDiscoveredSkill, PromptAssemblyExtraPromptCandidate, PromptAssemblyLifecycle,
    PromptAssemblyManagedSource, PromptAssemblyManagerSnapshot, PromptAssemblyMoveDirection,
    PromptAssemblyMutation, PromptAssemblySnapshot, PromptPreludeSnapshot,
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

    assert!(rows.contains("Prompt Assembly · scope=project"));
    assert!(!rows.contains("Next New Session"));
    assert!(rows.contains("[Skill]"));
    assert!(rows.contains("Custom"));
    assert!(rows.contains("Sel"));
    assert!(rows.contains("Ord"));
    assert!(rows.contains("Source"));
    assert!(rows.contains("Type"));
    assert!(rows.contains("Scope"));
    assert!(rows.contains("●"));
    assert!(!rows.contains("Active Sources"));
    assert!(!rows.contains("Inactive Sources"));
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
        .find(|row| row.contains("builtin") && row.contains("sys"))
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
fn skill_num_column_uses_visible_discovery_order_after_sorting() {
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

    let rows = rendered_rows(&render_model_buffer(&mut model, 120, 20)).join("\n");
    assert!(rows.contains("  1  caveman"));
    assert!(rows.contains("  2  codebase-design"));
    assert!(rows.contains("  M  ask-matt"));
    assert!(!rows.contains(" 21  caveman"));
    assert!(!rows.contains("  8  codebase-design"));
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
    assert!(manual_row.contains('-'));
    assert!(manual_row.contains('M'));
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
fn skills_tab_uses_num_column_label_instead_of_ord() {
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

    assert!(right_pane.contains("Num"));
    assert!(!right_pane.contains("Ord"));
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
        .find(|row| row.contains("Num") && row.contains("Name") && row.contains("Scope"))
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
    let trailing_index = skill_index + "[Skill] Custom".chars().count();

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
        .find(|row| row.contains("builtin") && row.contains("sys"))
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
        .find(|row| row.contains("builtin") && row.contains("sys"))
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
                        runtime_domain::prompt_assembly::persistence::PromptAssemblyScope::Project,
                    skill_name: "repo-bootstrap".to_string(),
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

    assert!(!rows.contains("a/A add custom"));
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

    assert!(rows.contains("a/A add custom"));
    assert!(!rows.contains("a/A add extra"));
    assert!(!rows.contains("i/I add skill"));
    assert!(rows.contains("d remove"));
    assert!(!rows.contains("J/K reorder"));
}

#[test]
fn footer_hides_custom_edit_and_remove_actions_on_skills_tab() {
    let mut model = ready_model();
    model.set_window(140, 16);
    model.open_prompt_overlay();
    model.set_prompt_overlay_focus(super::PromptOverlayFocus::Inactive);

    let rows = rendered_rows(&render_model_buffer(&mut model, 140, 16)).join("\n");

    assert!(!rows.contains("a/A add custom"));
    assert!(!rows.contains("i/I add skill"));
    assert!(!rows.contains("d remove"));
    assert!(!rows.contains("e/ctrl+g edit"));
    assert!(rows.contains("x disable"));
    assert!(!rows.contains("J/K reorder"));
}
