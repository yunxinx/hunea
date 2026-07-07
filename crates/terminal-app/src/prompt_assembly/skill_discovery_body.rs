use super::*;

pub(super) fn resolve_skill_discovery_body(
    scope: PromptAssemblyScope,
    skill_state: &[PersistedSkillDiscoverySkillEntry],
    skills_by_name: &HashMap<String, DiscoveredSkill>,
    global_state: &PromptAssemblyScopeState,
    project_state: &PromptAssemblyScopeState,
) -> String {
    let generated_body = render_skill_discovery_generated_body(skill_state, skills_by_name);
    let override_body = match scope {
        PromptAssemblyScope::Global => global_state.skill_discovery_override(),
        PromptAssemblyScope::Project => project_state
            .skill_discovery_override()
            .or(global_state.skill_discovery_override()),
    };
    match override_body {
        Some(override_body) => rebuild_skill_discovery_override(override_body, &generated_body),
        None => generated_body,
    }
}

fn render_skill_discovery_generated_body(
    skill_state: &[PersistedSkillDiscoverySkillEntry],
    skills_by_name: &HashMap<String, DiscoveredSkill>,
) -> String {
    let skills = skill_state
        .iter()
        .filter(|entry| entry.enabled)
        .filter_map(|entry| skills_by_name.get(&entry.skill_name).cloned())
        .collect::<Vec<_>>();
    format_skill_discovery_body(skills)
}

fn rebuild_skill_discovery_override(existing: &str, generated_body: &str) -> String {
    if let Some((_, suffix)) = parse_skill_discovery_override(existing) {
        let mut sections = vec![
            SKILL_DISCOVERY_GENERATED_START.to_string(),
            generated_body.to_string(),
            SKILL_DISCOVERY_GENERATED_END.to_string(),
        ];
        if !suffix.is_empty() {
            sections.push(suffix.to_string());
        }
        return sections.join("\n");
    }
    existing.to_string()
}

fn parse_skill_discovery_override(content: &str) -> Option<(&str, &str)> {
    let (_, after_start) = content.split_once(SKILL_DISCOVERY_GENERATED_START)?;
    let (_, after_end) = after_start.split_once(SKILL_DISCOVERY_GENERATED_END)?;
    Some((after_start, after_end.trim_start_matches('\n')))
}
