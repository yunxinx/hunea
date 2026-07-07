use super::*;

pub(in crate::prompt_assembly) fn generate_extra_prompt_reference_id_slug(title: &str) -> String {
    let slug = title
        .chars()
        .flat_map(char::to_lowercase)
        .map(|character| match character {
            'a'..='z' | '0'..='9' => character,
            _ => '-',
        })
        .collect::<String>()
        .split('-')
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    let slug = if slug.is_empty() {
        "prompt"
    } else {
        slug.as_str()
    };
    slug.to_string()
}

pub(in crate::prompt_assembly) fn generate_unique_extra_prompt_reference_id(
    state: &PromptAssemblyScopeState,
    title: &str,
) -> String {
    let base = generate_extra_prompt_reference_id_slug(title);
    let existing_reference_ids = state
        .entries()
        .iter()
        .filter(|entry| entry.kind == PromptSourceKind::ExtraPrompt)
        .map(|entry| entry.reference_id.as_str())
        .chain(
            state
                .extra_prompts()
                .iter()
                .map(|prompt| prompt.reference_id.as_str()),
        )
        .collect::<std::collections::HashSet<_>>();
    if !existing_reference_ids.contains(base.as_str()) {
        return base;
    }

    let mut suffix = 2usize;
    loop {
        let candidate = format!("{base}-{suffix}");
        if !existing_reference_ids.contains(candidate.as_str()) {
            return candidate;
        }
        suffix = suffix.saturating_add(1);
    }
}

pub(in crate::prompt_assembly) fn next_requested_order(
    entries: &[PersistedPromptAssemblyEntry],
) -> u16 {
    entries
        .iter()
        .filter_map(|entry| entry.requested_order)
        .max()
        .unwrap_or(DEFAULT_SKILL_DISCOVERY_REQUESTED_ORDER)
        .saturating_add(10)
}

pub(in crate::prompt_assembly) fn activate_long_lived_skill(
    global_state: &mut PromptAssemblyScopeState,
    project_state: &mut PromptAssemblyScopeState,
    scope: PromptAssemblyScope,
    skill_name: &str,
) {
    let state = scope_state_mut(global_state, project_state, scope);
    let next_order = next_requested_order(state.entries());
    if let Some(entry) = state.entry_mut(PromptSourceKind::LongLivedSkill, skill_name) {
        entry.enabled = true;
        if entry.requested_order.is_none() {
            entry.requested_order = Some(next_order);
        }
        return;
    }

    state.upsert_entry(PersistedPromptAssemblyEntry {
        reference_id: skill_name.to_string(),
        kind: PromptSourceKind::LongLivedSkill,
        title: skill_name.to_string(),
        enabled: true,
        requested_order: Some(next_order),
    });
}

pub(in crate::prompt_assembly) fn ensure_skill_discovery_entry_exists(
    state: &mut PromptAssemblyScopeState,
) {
    if state
        .entries()
        .iter()
        .any(|entry| entry.kind == PromptSourceKind::SkillDiscovery)
    {
        return;
    }
    let requested_order = default_skill_discovery_requested_order(state.entries());
    state.upsert_entry(PersistedPromptAssemblyEntry {
        reference_id: "skill-discovery".to_string(),
        kind: PromptSourceKind::SkillDiscovery,
        title: "Skill discovery".to_string(),
        enabled: true,
        requested_order: Some(requested_order),
    });
}

pub(in crate::prompt_assembly) fn ensure_default_skill_discovery_source(
    global_state: &mut PromptAssemblyScopeState,
    project_state: &mut PromptAssemblyScopeState,
) {
    if global_state
        .entries()
        .iter()
        .chain(project_state.entries().iter())
        .any(|entry| entry.kind == PromptSourceKind::SkillDiscovery)
    {
        return;
    }

    let target = if scope_state_has_prompt_content(project_state)
        || !scope_state_has_prompt_content(global_state)
    {
        project_state
    } else {
        global_state
    };
    ensure_skill_discovery_entry_exists(target);
}

pub(in crate::prompt_assembly) fn scope_state_has_prompt_content(
    state: &PromptAssemblyScopeState,
) -> bool {
    state.core_system_override().is_some()
        || state.skill_discovery_override().is_some()
        || state.tool_guidelines_override().is_some()
        || !state.entries().is_empty()
        || !state.skill_discovery_skills().is_empty()
        || !state.tool_selections().is_empty()
        || !state.extra_prompts().is_empty()
}

pub(in crate::prompt_assembly) fn default_skill_discovery_requested_order(
    entries: &[PersistedPromptAssemblyEntry],
) -> u16 {
    entries
        .iter()
        .find(|entry| entry.kind == PromptSourceKind::SkillDiscovery)
        .and_then(|entry| entry.requested_order)
        .unwrap_or(DEFAULT_SKILL_DISCOVERY_REQUESTED_ORDER)
}

pub(in crate::prompt_assembly) fn set_prompt_source_enabled(
    state: &mut PromptAssemblyScopeState,
    kind: PromptSourceKind,
    reference_id: &str,
    enabled: bool,
) {
    if let Some(entry) = state.entry_mut(kind, reference_id) {
        entry.enabled = enabled;
    }
}

pub(in crate::prompt_assembly) fn set_extra_prompt_selected(
    global_state: &mut PromptAssemblyScopeState,
    project_state: &mut PromptAssemblyScopeState,
    scope: PromptAssemblyScope,
    reference_id: &str,
    selected: bool,
) {
    let state = scope_state_mut(global_state, project_state, scope);
    let Some(prompt) = state
        .extra_prompts()
        .iter()
        .find(|prompt| prompt.reference_id == reference_id)
        .cloned()
    else {
        return;
    };

    if selected {
        if state.entries().iter().any(|entry| {
            entry.kind == PromptSourceKind::ExtraPrompt && entry.reference_id == reference_id
        }) {
            return;
        }
        let requested_order = next_requested_order(state.entries());
        state.upsert_entry(PersistedPromptAssemblyEntry {
            reference_id: prompt.reference_id,
            kind: PromptSourceKind::ExtraPrompt,
            title: prompt.title,
            enabled: true,
            requested_order: Some(requested_order),
        });
        return;
    }

    state.remove_entry(PromptSourceKind::ExtraPrompt, reference_id);
}

pub(in crate::prompt_assembly) fn set_discovered_skill_selected(
    state: &mut PromptAssemblyScopeState,
    skill_name: &str,
    selected: bool,
) {
    let next_order = next_skill_discovery_requested_order(state.skill_discovery_skills());
    if let Some(entry) = state.skill_discovery_skill_mut(skill_name) {
        entry.enabled = selected;
        if selected && entry.requested_order.is_none() {
            entry.requested_order = Some(next_order);
        }
        return;
    }

    state.upsert_skill_discovery_skill(PersistedSkillDiscoverySkillEntry {
        skill_name: skill_name.to_string(),
        enabled: selected,
        requested_order: Some(next_order),
    });
}

pub(in crate::prompt_assembly) fn move_discovered_skill(
    state: &mut PromptAssemblyScopeState,
    skill_name: &str,
    direction: PromptAssemblyMoveDirection,
) -> Result<()> {
    let Some(position) = state
        .skill_discovery_skills()
        .iter()
        .position(|entry| entry.skill_name == skill_name)
    else {
        return Ok(());
    };
    let Some(neighbor) = (match direction {
        PromptAssemblyMoveDirection::Up => position.checked_sub(1),
        PromptAssemblyMoveDirection::Down => {
            (position + 1 < state.skill_discovery_skills().len()).then_some(position + 1)
        }
    }) else {
        return Ok(());
    };
    state.swap_skill_discovery_skills(position, neighbor);
    normalize_skill_discovery_requested_orders(state);
    Ok(())
}

pub(in crate::prompt_assembly) fn reset_discovered_skill_order(
    state: &mut PromptAssemblyScopeState,
    discovered_skills: &[DiscoveredSkill],
) {
    let enabled_by_name = state
        .skill_discovery_skills()
        .iter()
        .map(|entry| (entry.skill_name.as_str(), entry.enabled))
        .collect::<HashMap<_, _>>();
    state.set_skill_discovery_skills(
        discovered_skills
            .iter()
            .filter(|skill| skill.can_select_for_discovery())
            .enumerate()
            .map(|(index, skill)| PersistedSkillDiscoverySkillEntry {
                skill_name: skill.name.clone(),
                enabled: enabled_by_name
                    .get(skill.name.as_str())
                    .copied()
                    .unwrap_or(true),
                requested_order: Some(u16::try_from(index + 1).unwrap_or(u16::MAX)),
            })
            .collect(),
    );
}

pub(in crate::prompt_assembly) fn next_skill_discovery_requested_order(
    entries: &[PersistedSkillDiscoverySkillEntry],
) -> u16 {
    entries
        .iter()
        .filter_map(|entry| entry.requested_order)
        .max()
        .unwrap_or(0)
        .saturating_add(1)
}

pub(in crate::prompt_assembly) fn normalize_skill_discovery_requested_orders(
    state: &mut PromptAssemblyScopeState,
) {
    for index in 0..state.skill_discovery_skills().len() {
        if let Some(entry) = state.skill_discovery_skill_at_mut(index) {
            entry.requested_order = Some(u16::try_from(index + 1).unwrap_or(u16::MAX));
        }
    }
}
