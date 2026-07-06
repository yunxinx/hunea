use super::*;

pub(in crate::prompt_assembly) fn scope_origin(scope: PromptAssemblyScope) -> PromptSourceOrigin {
    match scope {
        PromptAssemblyScope::Global => PromptSourceOrigin::Global,
        PromptAssemblyScope::Project => PromptSourceOrigin::Project,
    }
}

pub(in crate::prompt_assembly) fn entry_origin(
    scope: PromptAssemblyScope,
    kind: PromptSourceKind,
) -> PromptSourceOrigin {
    if matches!(
        kind,
        PromptSourceKind::ToolGuidelines
            | PromptSourceKind::DynamicEnvironmentBaseline
            | PromptSourceKind::DynamicEnvironmentChanges
    ) {
        PromptSourceOrigin::Builtin
    } else {
        scope_origin(scope)
    }
}

pub(in crate::prompt_assembly) fn scope_state_mut<'a>(
    global_state: &'a mut PromptAssemblyScopeState,
    project_state: &'a mut PromptAssemblyScopeState,
    scope: PromptAssemblyScope,
) -> &'a mut PromptAssemblyScopeState {
    match scope {
        PromptAssemblyScope::Global => global_state,
        PromptAssemblyScope::Project => project_state,
    }
}

pub(in crate::prompt_assembly) fn prompt_source_scope_state_mut<'a>(
    global_state: &'a mut PromptAssemblyScopeState,
    project_state: &'a mut PromptAssemblyScopeState,
    scope: PromptAssemblyScope,
    kind: PromptSourceKind,
) -> &'a mut PromptAssemblyScopeState {
    let scope = prompt_source_effective_scope(kind, global_state, project_state, scope);
    scope_state_mut(global_state, project_state, scope)
}

pub(in crate::prompt_assembly) fn prompt_source_effective_scope(
    kind: PromptSourceKind,
    global_state: &PromptAssemblyScopeState,
    project_state: &PromptAssemblyScopeState,
    fallback: PromptAssemblyScope,
) -> PromptAssemblyScope {
    match kind {
        PromptSourceKind::SkillDiscovery => {
            skill_discovery_scope(global_state, project_state, fallback)
        }
        PromptSourceKind::ToolGuidelines => {
            tool_guidelines_scope(global_state, project_state, fallback)
        }
        PromptSourceKind::DynamicEnvironmentBaseline
        | PromptSourceKind::DynamicEnvironmentChanges => {
            let _ = (global_state, project_state, fallback);
            PromptAssemblyScope::Global
        }
        PromptSourceKind::CoreSystemPrompt
        | PromptSourceKind::InstructionsFile
        | PromptSourceKind::ExtraPrompt
        | PromptSourceKind::LongLivedSkill => fallback,
    }
}

pub(in crate::prompt_assembly) fn skill_discovery_scope(
    global_state: &PromptAssemblyScopeState,
    project_state: &PromptAssemblyScopeState,
    fallback: PromptAssemblyScope,
) -> PromptAssemblyScope {
    if project_state.skill_discovery_override().is_some()
        || !project_state.skill_discovery_skills().is_empty()
        || project_state
            .entries()
            .iter()
            .any(|entry| entry.kind == PromptSourceKind::SkillDiscovery)
    {
        PromptAssemblyScope::Project
    } else if global_state.skill_discovery_override().is_some()
        || !global_state.skill_discovery_skills().is_empty()
        || global_state
            .entries()
            .iter()
            .any(|entry| entry.kind == PromptSourceKind::SkillDiscovery)
    {
        PromptAssemblyScope::Global
    } else {
        fallback
    }
}

pub(in crate::prompt_assembly) fn tool_guidelines_scope(
    global_state: &PromptAssemblyScopeState,
    project_state: &PromptAssemblyScopeState,
    fallback: PromptAssemblyScope,
) -> PromptAssemblyScope {
    if project_state.tool_guidelines_override().is_some()
        || !project_state.tool_selections().is_empty()
        || project_state
            .entries()
            .iter()
            .any(|entry| entry.kind == PromptSourceKind::ToolGuidelines)
    {
        PromptAssemblyScope::Project
    } else if global_state.tool_guidelines_override().is_some()
        || !global_state.tool_selections().is_empty()
        || global_state
            .entries()
            .iter()
            .any(|entry| entry.kind == PromptSourceKind::ToolGuidelines)
    {
        PromptAssemblyScope::Global
    } else {
        fallback
    }
}
