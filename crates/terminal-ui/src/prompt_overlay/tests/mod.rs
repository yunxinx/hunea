use std::{fs, path::PathBuf};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton};
use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Layout, Rect},
    style::Color,
    style::Modifier,
};
use runtime_domain::dynamic_environment::{
    DynamicEnvironmentSnapshotKind, DynamicEnvironmentSourceKind,
};
use runtime_domain::prompt_assembly::persistence::PromptAssemblyScope;
use runtime_domain::prompt_assembly::{
    PromptAssemblyDiscoveredSkill, PromptAssemblyDynamicEnvironmentCandidate,
    PromptAssemblyExtraPromptCandidate, PromptAssemblyLifecycle, PromptAssemblyManagedSource,
    PromptAssemblyManagerSnapshot, PromptAssemblyManagerSource, PromptAssemblyMoveDirection,
    PromptAssemblyMutation, PromptAssemblyScopedMutationKind, PromptAssemblySnapshot,
    PromptAssemblyToolCandidate, PromptSourceInactiveReason, PromptSourceKind, PromptSourceOrigin,
    PromptSourceStatus, ResolvedPromptSource,
};
use runtime_domain::session::PromptAssemblyUpdateNotice;

use crate::{
    AppEffect, AppEvent, Model, ModelOptions, StartupBannerOptions,
    fullscreen_list_chrome::fullscreen_list_chrome_rects,
    modal_layer::ModalLayer,
    runtime::RuntimeEventApply,
    test_helpers::{render_model_buffer, rendered_rows},
    theme::default_palette,
};

use super::*;

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
    let mut prompt_assembly = PromptAssemblyManagerSnapshot::default();
    prompt_assembly.resolution.assembly = prompt_snapshot();
    prompt_assembly.sources.managed = vec![
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
            reference_id: "repo-rules".to_string(),
            kind: PromptSourceKind::ExtraPrompt,
            title: "repo-rules".to_string(),
            origin: Some(PromptSourceOrigin::Project),
            scope: Some(PromptAssemblyScope::Project),
            enabled: true,
            order: 3,
        },
        PromptAssemblyManagedSource {
            reference_id: "safety-policy".to_string(),
            kind: PromptSourceKind::ExtraPrompt,
            title: "safety-policy".to_string(),
            origin: Some(PromptSourceOrigin::Global),
            scope: Some(PromptAssemblyScope::Global),
            enabled: false,
            order: 4,
        },
    ];
    prompt_assembly.candidates.extra_prompts = vec![PromptAssemblyExtraPromptCandidate {
        reference_id: "global-extra".to_string(),
        title: "global-extra".to_string(),
        origin: PromptSourceOrigin::Global,
        body: "# Global Extra\n".to_string(),
        selected: false,
    }];
    prompt_assembly.candidates.discovered_skills = vec![
        PromptAssemblyDiscoveredSkill {
            skill_name: "repo-bootstrap".to_string(),
            title: "repo-bootstrap".to_string(),
            description: "Bootstrap repo".to_string(),
            origin: PromptSourceOrigin::Project,
            selection_scope: PromptAssemblyScope::Project,
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
            selection_scope: PromptAssemblyScope::Project,
            skill_path: "/tmp/code-review/SKILL.md".to_string(),
            body: "# Code Review\n\nUse this skill.".to_string(),
            can_select_for_discovery: true,
            selected: true,
            selected_order: Some(2),
        },
    ];
    prompt_assembly.candidates.manual_skills = vec![PromptAssemblyDiscoveredSkill {
        skill_name: "repo-bootstrap".to_string(),
        title: "repo-bootstrap".to_string(),
        description: "Bootstrap repo".to_string(),
        origin: PromptSourceOrigin::Project,
        selection_scope: PromptAssemblyScope::Project,
        skill_path: "/tmp/repo-bootstrap/SKILL.md".to_string(),
        body: "# Repo Bootstrap\n\nUse this skill.".to_string(),
        can_select_for_discovery: true,
        selected: false,
        selected_order: None,
    }];
    prompt_assembly.core_system.builtin_body = "builtin core".to_string();

    let mut model = Model::new_with_options(
        StartupBannerOptions::default(),
        ModelOptions {
            prompt_assembly: Some(prompt_assembly),
            ..ModelOptions::default()
        },
    );
    model.set_window(90, 16);
    model.set_palette(default_palette(), true);
    model
}

fn ready_model_with_external_editor() -> Model {
    let prompt_assembly = ready_model().prompt_assembly.clone();
    let mut model = Model::new_with_options(
        StartupBannerOptions::default(),
        ModelOptions {
            prompt_assembly: Some(prompt_assembly),
            external_editor: vec![
                "sh".to_string(),
                "-c".to_string(),
                "cat \"$1\" >/dev/null".to_string(),
            ],
            external_editor_hint: "sh".to_string(),
            ..ModelOptions::default()
        },
    );
    model.set_window(90, 16);
    model.set_palette(default_palette(), true);
    model
}

fn tool_guidelines_managed_source() -> PromptAssemblyManagedSource {
    PromptAssemblyManagedSource {
        reference_id: "tool-guidelines".to_string(),
        kind: PromptSourceKind::ToolGuidelines,
        title: "Tool guidelines".to_string(),
        origin: Some(PromptSourceOrigin::Builtin),
        scope: Some(PromptAssemblyScope::Global),
        enabled: true,
        order: 2,
    }
}

fn dynamic_environment_baseline_managed_source() -> PromptAssemblyManagedSource {
    PromptAssemblyManagedSource {
        reference_id: "env-baseline".to_string(),
        kind: PromptSourceKind::DynamicEnvironmentBaseline,
        title: "Env baseline".to_string(),
        origin: Some(PromptSourceOrigin::Builtin),
        scope: Some(PromptAssemblyScope::Global),
        enabled: true,
        order: 2,
    }
}

fn tool_guidelines_source() -> PromptAssemblyManagerSource {
    PromptAssemblyManagerSource {
        reference_id: "tool-guidelines".to_string(),
        kind: PromptSourceKind::ToolGuidelines,
        title: "Tool guidelines".to_string(),
        origin: Some(PromptSourceOrigin::Builtin),
        resolved_body_origin: Some(PromptSourceOrigin::Builtin),
        backing_file_path: None,
        body: Some("generated tool guidance".to_string()),
    }
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

fn column_in_row(row: &str, needle: &str) -> usize {
    let byte_index = row.find(needle).expect("needle should exist in row");
    row[..byte_index].chars().count()
}

fn find_buffer_text_position(buffer: &Buffer, needle: &str) -> Option<(u16, u16)> {
    find_text_position(&rendered_rows(buffer), needle)
}

fn assert_text_cells_use_color_at(
    buffer: &Buffer,
    text: &str,
    row: u16,
    column: u16,
    expected: Color,
) {
    for (offset, character) in text.chars().enumerate() {
        let cell = &buffer[(
            column + u16::try_from(offset).expect("text offset should fit in u16"),
            row,
        )];
        assert_eq!(
            cell.fg, expected,
            "expected `{character}` in `{text}` to use {expected:?}, got {:?}",
            cell.fg
        );
    }
}

fn assert_text_cells_are_underlined_at(buffer: &Buffer, text: &str, row: u16, column: u16) {
    for (offset, character) in text.chars().enumerate() {
        let cell = &buffer[(
            column + u16::try_from(offset).expect("text offset should fit in u16"),
            row,
        )];
        assert!(
            cell.modifier.contains(Modifier::UNDERLINED),
            "expected `{character}` in `{text}` to be underlined, got {:?}",
            cell.modifier
        );
    }
}

fn assert_cell_is_not_underlined(buffer: &Buffer, row: u16, column: u16) {
    let cell = &buffer[(column, row)];
    assert!(
        !cell.modifier.contains(Modifier::UNDERLINED),
        "expected cell at ({column}, {row}) to not be underlined, got {:?}",
        cell.modifier
    );
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

mod editor;
mod input;
mod render;
mod selection;
