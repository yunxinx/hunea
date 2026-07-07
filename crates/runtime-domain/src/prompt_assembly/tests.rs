use super::*;

fn extra_prompt(
    reference_id: &str,
    title: &str,
    origin: PromptSourceOrigin,
    requested_order: Option<u16>,
) -> PromptSourceCandidate {
    PromptSourceCandidate {
        reference_id: reference_id.to_string(),
        kind: PromptSourceKind::ExtraPrompt,
        title: title.to_string(),
        origin: Some(origin),
        collision_key: Some(reference_id.to_string()),
        state: PromptSourceCandidateState::Enabled,
        requested_order,
    }
}

fn long_lived_skill(
    reference_id: &str,
    title: &str,
    origin: PromptSourceOrigin,
    collision_key: &str,
    requested_order: Option<u16>,
) -> PromptSourceCandidate {
    PromptSourceCandidate {
        reference_id: reference_id.to_string(),
        kind: PromptSourceKind::LongLivedSkill,
        title: title.to_string(),
        origin: Some(origin),
        collision_key: Some(collision_key.to_string()),
        state: PromptSourceCandidateState::Enabled,
        requested_order,
    }
}

fn skill_discovery(requested_order: Option<u16>) -> PromptSourceCandidate {
    PromptSourceCandidate {
        reference_id: "skill-discovery".to_string(),
        kind: PromptSourceKind::SkillDiscovery,
        title: "Skill discovery source".to_string(),
        origin: None,
        collision_key: None,
        state: PromptSourceCandidateState::Enabled,
        requested_order,
    }
}

#[test]
fn resolution_is_scoped_to_next_new_session_and_keeps_core_first() {
    let snapshot = resolve_prompt_assembly(&PromptAssemblyInput {
        core_system: CoreSystemPromptInput::default(),
        candidates: vec![
            extra_prompt(
                "repo-review-rules",
                "repo-review-rules",
                PromptSourceOrigin::Project,
                Some(20),
            ),
            skill_discovery(Some(5)),
            long_lived_skill(
                "code-review-project",
                "code-review",
                PromptSourceOrigin::Project,
                "code-review",
                Some(10),
            ),
        ],
    });

    assert_eq!(snapshot.lifecycle, PromptAssemblyLifecycle::NextNewSession);
    assert_eq!(snapshot.active_sources.len(), 4);
    assert_eq!(
        snapshot.active_sources[0],
        ResolvedPromptSource {
            reference_id: "core-system".to_string(),
            kind: PromptSourceKind::CoreSystemPrompt,
            title: "Core system prompt".to_string(),
            origin: Some(PromptSourceOrigin::Builtin),
            status: PromptSourceStatus::Active { order: 0 },
        }
    );
    assert_eq!(
        snapshot
            .active_sources
            .iter()
            .map(|source| source.reference_id.as_str())
            .collect::<Vec<_>>(),
        vec![
            "core-system",
            "skill-discovery",
            "code-review-project",
            "repo-review-rules",
        ]
    );
}

#[test]
fn core_system_prefers_project_override_then_global_override() {
    let project_snapshot = resolve_prompt_assembly(&PromptAssemblyInput {
        core_system: CoreSystemPromptInput {
            global_override_present: true,
            project_override_present: true,
        },
        candidates: Vec::new(),
    });
    assert_eq!(
        project_snapshot.active_sources[0].origin,
        Some(PromptSourceOrigin::Project)
    );

    let global_snapshot = resolve_prompt_assembly(&PromptAssemblyInput {
        core_system: CoreSystemPromptInput {
            global_override_present: true,
            project_override_present: false,
        },
        candidates: Vec::new(),
    });
    assert_eq!(
        global_snapshot.active_sources[0].origin,
        Some(PromptSourceOrigin::Global)
    );
}

#[test]
fn disabled_sources_become_inactive_without_shadowing_enabled_candidates() {
    let mut disabled_project = long_lived_skill(
        "code-review-project",
        "code-review",
        PromptSourceOrigin::Project,
        "code-review",
        Some(1),
    );
    disabled_project.state = PromptSourceCandidateState::Disabled;

    let snapshot = resolve_prompt_assembly(&PromptAssemblyInput {
        core_system: CoreSystemPromptInput::default(),
        candidates: vec![
            disabled_project,
            long_lived_skill(
                "code-review-global",
                "code-review",
                PromptSourceOrigin::Global,
                "code-review",
                Some(2),
            ),
        ],
    });

    assert!(
        snapshot
            .active_sources
            .iter()
            .any(|source| source.reference_id == "code-review-global")
    );
    assert!(snapshot.inactive_sources.contains(&ResolvedPromptSource {
        reference_id: "code-review-project".to_string(),
        kind: PromptSourceKind::LongLivedSkill,
        title: "code-review".to_string(),
        origin: Some(PromptSourceOrigin::Project),
        status: PromptSourceStatus::Inactive {
            reason: PromptSourceInactiveReason::Disabled,
        },
    }));
}

#[test]
fn project_collision_winner_can_be_missing_and_shadow_global_candidate() {
    let mut missing_project = long_lived_skill(
        "repo-bootstrap-project",
        "repo-bootstrap",
        PromptSourceOrigin::Project,
        "repo-bootstrap",
        Some(1),
    );
    missing_project.state = PromptSourceCandidateState::Missing;

    let snapshot = resolve_prompt_assembly(&PromptAssemblyInput {
        core_system: CoreSystemPromptInput::default(),
        candidates: vec![
            long_lived_skill(
                "repo-bootstrap-global",
                "repo-bootstrap",
                PromptSourceOrigin::Global,
                "repo-bootstrap",
                Some(2),
            ),
            missing_project,
        ],
    });

    assert!(snapshot.inactive_sources.contains(&ResolvedPromptSource {
        reference_id: "repo-bootstrap-project".to_string(),
        kind: PromptSourceKind::LongLivedSkill,
        title: "repo-bootstrap".to_string(),
        origin: Some(PromptSourceOrigin::Project),
        status: PromptSourceStatus::Inactive {
            reason: PromptSourceInactiveReason::Missing,
        },
    }));
    assert!(snapshot.inactive_sources.contains(&ResolvedPromptSource {
        reference_id: "repo-bootstrap-global".to_string(),
        kind: PromptSourceKind::LongLivedSkill,
        title: "repo-bootstrap".to_string(),
        origin: Some(PromptSourceOrigin::Global),
        status: PromptSourceStatus::Inactive {
            reason: PromptSourceInactiveReason::Shadowed,
        },
    }));
}

#[test]
fn project_extra_prompt_overrides_global_extra_prompt_with_same_collision_key() {
    let global_prompt = extra_prompt(
        "review-rules-global",
        "review-rules",
        PromptSourceOrigin::Global,
        Some(20),
    );
    let mut project_prompt = extra_prompt(
        "review-rules-project",
        "review-rules",
        PromptSourceOrigin::Project,
        Some(10),
    );
    project_prompt.collision_key = Some("review-rules".to_string());
    let mut global_prompt = global_prompt;
    global_prompt.collision_key = Some("review-rules".to_string());

    let snapshot = resolve_prompt_assembly(&PromptAssemblyInput {
        core_system: CoreSystemPromptInput::default(),
        candidates: vec![global_prompt, project_prompt],
    });

    assert!(
        snapshot
            .active_sources
            .iter()
            .any(|source| source.reference_id == "review-rules-project")
    );
    assert!(snapshot.inactive_sources.contains(&ResolvedPromptSource {
        reference_id: "review-rules-global".to_string(),
        kind: PromptSourceKind::ExtraPrompt,
        title: "review-rules".to_string(),
        origin: Some(PromptSourceOrigin::Global),
        status: PromptSourceStatus::Inactive {
            reason: PromptSourceInactiveReason::Shadowed,
        },
    }));
}

#[test]
fn inactive_sources_are_grouped_by_reason_then_sorted_by_title() {
    let mut disabled_beta = extra_prompt(
        "z-disabled",
        "z-disabled",
        PromptSourceOrigin::Project,
        Some(1),
    );
    disabled_beta.state = PromptSourceCandidateState::Disabled;

    let mut disabled_alpha = extra_prompt(
        "a-disabled",
        "a-disabled",
        PromptSourceOrigin::Project,
        Some(2),
    );
    disabled_alpha.state = PromptSourceCandidateState::Disabled;

    let mut missing_skill = long_lived_skill(
        "missing-project",
        "missing-skill",
        PromptSourceOrigin::Project,
        "missing-skill",
        Some(3),
    );
    missing_skill.state = PromptSourceCandidateState::Missing;

    let snapshot = resolve_prompt_assembly(&PromptAssemblyInput {
        core_system: CoreSystemPromptInput::default(),
        candidates: vec![
            disabled_beta,
            missing_skill,
            long_lived_skill(
                "shadowed-global",
                "shadowed-skill",
                PromptSourceOrigin::Global,
                "shadowed-skill",
                Some(5),
            ),
            long_lived_skill(
                "shadowed-project",
                "shadowed-skill",
                PromptSourceOrigin::Project,
                "shadowed-skill",
                Some(4),
            ),
            disabled_alpha,
        ],
    });

    assert_eq!(
        snapshot
            .inactive_sources
            .iter()
            .map(|source| source.reference_id.as_str())
            .collect::<Vec<_>>(),
        vec![
            "a-disabled",
            "z-disabled",
            "missing-project",
            "shadowed-global",
        ]
    );
}

#[test]
fn inactive_sources_keep_reason_grouping_and_use_natural_title_order() {
    let snapshot = resolve_prompt_assembly(&PromptAssemblyInput {
        core_system: CoreSystemPromptInput::default(),
        candidates: vec![
            PromptSourceCandidate {
                reference_id: "disabled-10".to_string(),
                kind: PromptSourceKind::ExtraPrompt,
                title: "alpha 10".to_string(),
                origin: Some(PromptSourceOrigin::Project),
                collision_key: None,
                state: PromptSourceCandidateState::Disabled,
                requested_order: None,
            },
            PromptSourceCandidate {
                reference_id: "disabled-2".to_string(),
                kind: PromptSourceKind::ExtraPrompt,
                title: "alpha 2".to_string(),
                origin: Some(PromptSourceOrigin::Project),
                collision_key: None,
                state: PromptSourceCandidateState::Disabled,
                requested_order: None,
            },
            PromptSourceCandidate {
                reference_id: "missing".to_string(),
                kind: PromptSourceKind::LongLivedSkill,
                title: "missing".to_string(),
                origin: Some(PromptSourceOrigin::Global),
                collision_key: None,
                state: PromptSourceCandidateState::Missing,
                requested_order: None,
            },
            PromptSourceCandidate {
                reference_id: "skill-shadow-global".to_string(),
                kind: PromptSourceKind::LongLivedSkill,
                title: "shadowed".to_string(),
                origin: Some(PromptSourceOrigin::Global),
                collision_key: Some("shared".to_string()),
                state: PromptSourceCandidateState::Enabled,
                requested_order: None,
            },
            PromptSourceCandidate {
                reference_id: "skill-shadow-project".to_string(),
                kind: PromptSourceKind::LongLivedSkill,
                title: "shadowed".to_string(),
                origin: Some(PromptSourceOrigin::Project),
                collision_key: Some("shared".to_string()),
                state: PromptSourceCandidateState::Enabled,
                requested_order: None,
            },
        ],
    });

    assert_eq!(
        snapshot
            .inactive_sources
            .iter()
            .map(|source| source.reference_id.as_str())
            .collect::<Vec<_>>(),
        vec![
            "disabled-2",
            "disabled-10",
            "missing",
            "skill-shadow-global",
        ]
    );
}

#[test]
fn active_sources_with_equal_requested_order_use_natural_title_order() {
    let snapshot = resolve_prompt_assembly(&PromptAssemblyInput {
        core_system: CoreSystemPromptInput::default(),
        candidates: vec![
            extra_prompt(
                "new-prompt-10",
                "New prompt 10",
                PromptSourceOrigin::Project,
                Some(10),
            ),
            extra_prompt(
                "new-prompt-2",
                "New prompt 2",
                PromptSourceOrigin::Project,
                Some(10),
            ),
            extra_prompt(
                "new-prompt-1",
                "New prompt 1",
                PromptSourceOrigin::Project,
                Some(10),
            ),
        ],
    });

    assert_eq!(
        snapshot
            .active_sources
            .iter()
            .skip(1)
            .map(|source| source.reference_id.as_str())
            .collect::<Vec<_>>(),
        vec!["new-prompt-1", "new-prompt-2", "new-prompt-10"]
    );
}
