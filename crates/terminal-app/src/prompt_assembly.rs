use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use color_eyre::eyre::{Result, WrapErr};
use runtime_domain::prompt_assembly::persistence::{
    PersistedPromptAssemblyEntry, PromptAssemblyScope, PromptAssemblyScopeState, StoredPromptBody,
    load_project_prompt_assembly_state, save_project_prompt_assembly_state,
};
use runtime_domain::prompt_assembly::{
    CoreSystemPromptInput, PromptAssemblyDiscoveredSkill, PromptAssemblyEditorTarget,
    PromptAssemblyInput, PromptAssemblyManagerSnapshot, PromptAssemblyManagerSource,
    PromptAssemblyMoveDirection, PromptAssemblyMutation, PromptPreludeSection,
    PromptPreludeSnapshot, PromptSourceCandidate, PromptSourceInactiveReason, PromptSourceKind,
    PromptSourceOrigin, PromptSourceStatus, resolve_prompt_assembly,
};
use session_store::SessionStore;

const BUILTIN_CORE_SYSTEM_PROMPT: &str =
    "You are Hunea, a terminal-based AI assistant. Be direct, precise, and action-oriented.";
const SKILL_FILE_NAME: &str = "SKILL.md";

#[derive(Debug, Clone, PartialEq, Eq)]
struct DiscoveredSkill {
    name: String,
    description: String,
    skill_path: PathBuf,
    body: String,
    origin: PromptSourceOrigin,
    disable_model_invocation: bool,
}

/// `ManualSkillPromptUse` 表示一次 `$skill` 当前轮注入解析后的 skill 使用项。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ManualSkillPromptUse {
    pub(crate) skill_name: String,
    pub(crate) origin: PromptSourceOrigin,
    pub(crate) skill_path: PathBuf,
    pub(crate) body: String,
}

/// `ManualSkillMessageAssembly` 表示手动 skill 注入后的 provider-visible 用户消息。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ManualSkillMessageAssembly {
    pub(crate) provider_visible_user_text: String,
    pub(crate) uses: Vec<ManualSkillPromptUse>,
}

#[derive(Debug, Clone)]
struct PromptCandidateBody {
    body: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PromptEntryAddress {
    scope: PromptAssemblyScope,
    index: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PromptAssemblyMissingSourcesCheck {
    pub(crate) missing_count: usize,
}

impl PromptAssemblyMissingSourcesCheck {
    fn from_manager(manager: &PromptAssemblyManagerSnapshot) -> Self {
        Self {
            missing_count: manager
                .snapshot
                .inactive_sources
                .iter()
                .filter(|source| {
                    matches!(
                        source.status,
                        PromptSourceStatus::Inactive {
                            reason: PromptSourceInactiveReason::Missing
                        }
                    )
                })
                .count(),
        }
    }
}

pub(crate) fn load_initial_prompt_assembly(
    store: Arc<dyn SessionStore>,
    work_dir: &Path,
) -> Result<PromptAssemblyManagerSnapshot> {
    load_prompt_assembly_manager_snapshot(store, work_dir)
}

pub(crate) fn load_prompt_assembly_manager_snapshot(
    store: Arc<dyn SessionStore>,
    work_dir: &Path,
) -> Result<PromptAssemblyManagerSnapshot> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .wrap_err("start prompt assembly runtime")?;
    let global_state = runtime
        .block_on(store.load_global_prompt_assembly_state())
        .wrap_err("load global prompt assembly state")?;
    let project_state = load_project_prompt_assembly_state(work_dir)
        .wrap_err("load project prompt assembly state")?;
    Ok(resolve_prompt_assembly_manager_snapshot(
        work_dir,
        &global_state,
        &project_state,
    ))
}

pub(crate) fn apply_prompt_assembly_mutation(
    store: Arc<dyn SessionStore>,
    work_dir: &Path,
    mutation: PromptAssemblyMutation,
) -> Result<PromptAssemblyManagerSnapshot> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .wrap_err("start prompt assembly runtime")?;
    let mut global_state = runtime
        .block_on(store.load_global_prompt_assembly_state())
        .wrap_err("load global prompt assembly state")?;
    let mut project_state = load_project_prompt_assembly_state(work_dir)
        .wrap_err("load project prompt assembly state")?;

    apply_mutation_to_scope_states(work_dir, &mut global_state, &mut project_state, mutation)?;

    runtime
        .block_on(store.save_global_prompt_assembly_state(&global_state))
        .wrap_err("save global prompt assembly state")?;
    save_project_prompt_assembly_state(work_dir, &project_state)
        .wrap_err("save project prompt assembly state")?;

    Ok(resolve_prompt_assembly_manager_snapshot(
        work_dir,
        &global_state,
        &project_state,
    ))
}

pub(crate) fn check_prompt_assembly_missing_sources_from_states(
    work_dir: &Path,
    global_state: &PromptAssemblyScopeState,
    project_state: &PromptAssemblyScopeState,
) -> PromptAssemblyMissingSourcesCheck {
    let manager = resolve_prompt_assembly_manager_snapshot(work_dir, global_state, project_state);
    PromptAssemblyMissingSourcesCheck::from_manager(&manager)
}

/// `assemble_manual_skill_message` 解析当前用户消息里的 `$skill` 提及并拼装 provider-visible 文本。
pub(crate) fn assemble_manual_skill_message(
    work_dir: &Path,
    user_text: &str,
) -> ManualSkillMessageAssembly {
    let discovered_skills = discover_skills(work_dir, None);
    let skills_by_name = discovered_skills
        .iter()
        .map(|skill| (skill.name.clone(), skill))
        .collect::<HashMap<_, _>>();
    let mut seen_names = std::collections::HashSet::new();
    let mut uses = Vec::new();
    for skill_name in extract_manual_skill_mentions(user_text) {
        if !seen_names.insert(skill_name.clone()) {
            continue;
        }
        let Some(skill) = skills_by_name.get(&skill_name) else {
            continue;
        };
        uses.push(ManualSkillPromptUse {
            skill_name: skill.name.clone(),
            origin: skill.origin,
            skill_path: skill.skill_path.clone(),
            body: format_long_lived_skill_body(skill),
        });
    }

    let provider_visible_user_text = if uses.is_empty() {
        user_text.to_string()
    } else {
        let mut sections = uses
            .iter()
            .map(|skill| skill.body.clone())
            .collect::<Vec<_>>();
        let trimmed_user_text = user_text.trim();
        if !trimmed_user_text.is_empty() {
            sections.push(trimmed_user_text.to_string());
        }
        sections.join("\n\n")
    };

    ManualSkillMessageAssembly {
        provider_visible_user_text,
        uses,
    }
}

#[cfg(test)]
pub(crate) fn load_initial_prompt_prelude(
    store: Arc<dyn SessionStore>,
    work_dir: &Path,
) -> Result<PromptPreludeSnapshot> {
    Ok(load_initial_prompt_assembly(store, work_dir)?.prelude)
}

#[cfg(test)]
fn resolve_initial_prompt_prelude(
    work_dir: &Path,
    global_state: &PromptAssemblyScopeState,
    project_state: &PromptAssemblyScopeState,
) -> PromptPreludeSnapshot {
    resolve_prompt_assembly_manager_snapshot(work_dir, global_state, project_state).prelude
}

fn resolve_prompt_assembly_manager_snapshot(
    work_dir: &Path,
    global_state: &PromptAssemblyScopeState,
    project_state: &PromptAssemblyScopeState,
) -> PromptAssemblyManagerSnapshot {
    resolve_prompt_assembly_manager_snapshot_with_global_skill_root(
        work_dir,
        global_state,
        project_state,
        None,
    )
}

fn resolve_prompt_assembly_manager_snapshot_with_global_skill_root(
    work_dir: &Path,
    global_state: &PromptAssemblyScopeState,
    project_state: &PromptAssemblyScopeState,
    global_skill_root_override: Option<&Path>,
) -> PromptAssemblyManagerSnapshot {
    let discovered_skills = discover_skills(work_dir, global_skill_root_override);
    let extra_prompt_bodies = indexed_extra_prompt_bodies(global_state, project_state);
    let skills_by_name = discovered_skills
        .iter()
        .map(|skill| (skill.name.clone(), skill.clone()))
        .collect::<HashMap<_, _>>();

    let mut candidate_bodies = HashMap::new();
    let mut candidates = Vec::new();
    extend_candidates(
        &mut candidates,
        &mut candidate_bodies,
        global_state,
        &extra_prompt_bodies,
        &skills_by_name,
    );
    extend_candidates(
        &mut candidates,
        &mut candidate_bodies,
        project_state,
        &extra_prompt_bodies,
        &skills_by_name,
    );

    let snapshot = resolve_prompt_assembly(&PromptAssemblyInput {
        core_system: CoreSystemPromptInput {
            global_override_present: global_state.core_system_override.is_some(),
            project_override_present: project_state.core_system_override.is_some(),
        },
        candidates,
    });
    let mut sources = vec![PromptAssemblyManagerSource {
        reference_id: "core-system".to_string(),
        kind: PromptSourceKind::CoreSystemPrompt,
        title: "Core system prompt".to_string(),
        origin: Some(resolve_core_system_origin(global_state, project_state)),
        resolved_body_origin: Some(resolve_core_system_origin(global_state, project_state)),
        body: Some(resolved_core_system_body(global_state, project_state)),
    }];
    sources.extend(materialized_sources_for_state(
        global_state,
        &extra_prompt_bodies,
        &skills_by_name,
    ));
    sources.extend(materialized_sources_for_state(
        project_state,
        &extra_prompt_bodies,
        &skills_by_name,
    ));

    let mut sections = Vec::new();
    for source in &snapshot.active_sources {
        if !matches!(source.status, PromptSourceStatus::Active { .. }) {
            continue;
        }

        let body = match source.kind {
            PromptSourceKind::CoreSystemPrompt => {
                resolved_core_system_body(global_state, project_state)
            }
            _ => candidate_bodies
                .get(&candidate_body_key(
                    source.origin,
                    source.kind,
                    &source.reference_id,
                ))
                .map(|candidate| candidate.body.clone())
                .unwrap_or_default(),
        };
        if body.trim().is_empty() {
            continue;
        }

        sections.push(PromptPreludeSection {
            reference_id: source.reference_id.clone(),
            kind: source.kind,
            title: source.title.clone(),
            origin: source.origin,
            body,
        });
    }

    PromptAssemblyManagerSnapshot {
        snapshot,
        prelude: PromptPreludeSnapshot { sections },
        sources,
        discovered_skills: discovered_skill_inventory(
            &discovered_skills,
            global_state,
            project_state,
        ),
        builtin_core_system_body: BUILTIN_CORE_SYSTEM_PROMPT.to_string(),
        global_core_system_override: global_state.core_system_override.clone(),
        project_core_system_override: project_state.core_system_override.clone(),
    }
}

fn apply_mutation_to_scope_states(
    work_dir: &Path,
    global_state: &mut PromptAssemblyScopeState,
    project_state: &mut PromptAssemblyScopeState,
    mutation: PromptAssemblyMutation,
) -> Result<()> {
    match mutation {
        PromptAssemblyMutation::SaveEditorTarget { target, content } => {
            apply_save_editor_target(work_dir, global_state, project_state, target, content)
        }
        PromptAssemblyMutation::ActivateLongLivedSkill { scope, skill_name } => {
            activate_long_lived_skill(global_state, project_state, scope, &skill_name);
            Ok(())
        }
        PromptAssemblyMutation::CreateExtraPrompt { scope, content } => {
            let state = scope_state_mut(global_state, project_state, scope);
            let title = derive_extra_prompt_title(&content, "New prompt");
            let reference_id = generate_extra_prompt_reference_id(&title);
            let requested_order = next_requested_order(&state.entries);
            state.entries.push(PersistedPromptAssemblyEntry {
                reference_id: reference_id.clone(),
                kind: PromptSourceKind::ExtraPrompt,
                title: title.clone(),
                enabled: true,
                requested_order: Some(requested_order),
            });
            state
                .extra_prompts
                .retain(|prompt| prompt.reference_id != reference_id);
            state.extra_prompts.push(StoredPromptBody {
                reference_id,
                title,
                body: content,
            });
            Ok(())
        }
        PromptAssemblyMutation::DeleteExtraPrompt {
            scope,
            reference_id,
        } => {
            let state = scope_state_mut(global_state, project_state, scope);
            state.entries.retain(|entry| {
                !(entry.kind == PromptSourceKind::ExtraPrompt && entry.reference_id == reference_id)
            });
            state
                .extra_prompts
                .retain(|prompt| prompt.reference_id != reference_id);
            Ok(())
        }
        PromptAssemblyMutation::RemovePromptSource {
            scope,
            kind,
            reference_id,
        } => {
            remove_prompt_source(
                scope_state_mut(global_state, project_state, scope),
                kind,
                &reference_id,
            );
            Ok(())
        }
        PromptAssemblyMutation::MoveActiveSource {
            scope,
            kind,
            reference_id,
            direction,
        } => move_active_source(
            global_state,
            project_state,
            scope,
            kind,
            &reference_id,
            direction,
        ),
        PromptAssemblyMutation::RestoreCoreSystemOverride { scope } => {
            scope_state_mut(global_state, project_state, scope).core_system_override = None;
            Ok(())
        }
    }
}

fn apply_save_editor_target(
    work_dir: &Path,
    global_state: &mut PromptAssemblyScopeState,
    project_state: &mut PromptAssemblyScopeState,
    target: PromptAssemblyEditorTarget,
    content: String,
) -> Result<()> {
    match target {
        PromptAssemblyEditorTarget::CoreSystemOverride { scope } => {
            let trimmed = content.trim();
            scope_state_mut(global_state, project_state, scope).core_system_override =
                (!trimmed.is_empty()).then_some(content);
            Ok(())
        }
        PromptAssemblyEditorTarget::ExtraPrompt {
            scope,
            reference_id,
        } => {
            let state = scope_state_mut(global_state, project_state, scope);
            let title = derive_extra_prompt_title(&content, &reference_id);
            if let Some(entry) = state.entries.iter_mut().find(|entry| {
                entry.kind == PromptSourceKind::ExtraPrompt && entry.reference_id == reference_id
            }) {
                entry.title = title.clone();
            } else {
                state.entries.push(PersistedPromptAssemblyEntry {
                    reference_id: reference_id.clone(),
                    kind: PromptSourceKind::ExtraPrompt,
                    title: title.clone(),
                    enabled: true,
                    requested_order: Some(next_requested_order(&state.entries)),
                });
            }

            if let Some(prompt) = state
                .extra_prompts
                .iter_mut()
                .find(|prompt| prompt.reference_id == reference_id)
            {
                prompt.title = title;
                prompt.body = content;
            } else {
                state.extra_prompts.push(StoredPromptBody {
                    reference_id,
                    title,
                    body: content,
                });
            }
            Ok(())
        }
        PromptAssemblyEditorTarget::SkillFile { skill_name, origin } => {
            let discovered = discover_skills(work_dir, None);
            let skill = discovered
                .iter()
                .find(|skill| skill.name == skill_name && skill.origin == origin)
                .ok_or_else(|| color_eyre::eyre::eyre!("skill file `{skill_name}` is missing"))?;
            fs::write(&skill.skill_path, content)
                .wrap_err_with(|| format!("write skill file {}", skill.skill_path.display()))?;
            Ok(())
        }
    }
}

fn resolved_core_system_body(
    global_state: &PromptAssemblyScopeState,
    project_state: &PromptAssemblyScopeState,
) -> String {
    project_state
        .core_system_override
        .as_deref()
        .or(global_state.core_system_override.as_deref())
        .unwrap_or(BUILTIN_CORE_SYSTEM_PROMPT)
        .trim()
        .to_string()
}

fn resolve_core_system_origin(
    global_state: &PromptAssemblyScopeState,
    project_state: &PromptAssemblyScopeState,
) -> PromptSourceOrigin {
    if project_state.core_system_override.is_some() {
        PromptSourceOrigin::Project
    } else if global_state.core_system_override.is_some() {
        PromptSourceOrigin::Global
    } else {
        PromptSourceOrigin::Builtin
    }
}

fn materialized_sources_for_state(
    state: &PromptAssemblyScopeState,
    extra_prompt_bodies: &HashMap<String, String>,
    skills_by_name: &HashMap<String, DiscoveredSkill>,
) -> Vec<PromptAssemblyManagerSource> {
    let origin = Some(scope_origin(state.scope));
    state
        .entries
        .iter()
        .map(|entry| PromptAssemblyManagerSource {
            reference_id: entry.reference_id.clone(),
            kind: entry.kind,
            title: entry.title.clone(),
            origin,
            resolved_body_origin: resolved_body_origin_for_entry(entry, skills_by_name),
            body: body_for_entry(entry, state.scope, extra_prompt_bodies, skills_by_name),
        })
        .collect()
}

fn extend_candidates(
    candidates: &mut Vec<PromptSourceCandidate>,
    candidate_bodies: &mut HashMap<String, PromptCandidateBody>,
    state: &PromptAssemblyScopeState,
    extra_prompt_bodies: &HashMap<String, String>,
    skills_by_name: &HashMap<String, DiscoveredSkill>,
) {
    let origin = Some(scope_origin(state.scope));
    for entry in &state.entries {
        let reference_id = entry.reference_id.clone();
        let candidate = PromptSourceCandidate {
            reference_id: reference_id.clone(),
            kind: entry.kind,
            title: entry.title.clone(),
            origin,
            collision_key: Some(reference_id.clone()),
            enabled: entry.enabled,
            resolvable: resolvable_for_entry(
                entry,
                state.scope,
                extra_prompt_bodies,
                skills_by_name,
            ),
            requested_order: entry.requested_order,
        };
        if let Some(body) = body_for_entry(entry, state.scope, extra_prompt_bodies, skills_by_name)
        {
            candidate_bodies.insert(
                candidate_body_key(origin, entry.kind, &reference_id),
                PromptCandidateBody { body },
            );
        }
        candidates.push(candidate);
    }
}

fn resolvable_for_entry(
    entry: &PersistedPromptAssemblyEntry,
    scope: PromptAssemblyScope,
    extra_prompt_bodies: &HashMap<String, String>,
    skills_by_name: &HashMap<String, DiscoveredSkill>,
) -> bool {
    match entry.kind {
        PromptSourceKind::ExtraPrompt => {
            extra_prompt_bodies.contains_key(&scope_reference_key(scope, &entry.reference_id))
        }
        PromptSourceKind::SkillDiscovery => true,
        PromptSourceKind::LongLivedSkill => skills_by_name.contains_key(&entry.reference_id),
        PromptSourceKind::CoreSystemPrompt => true,
    }
}

fn body_for_entry(
    entry: &PersistedPromptAssemblyEntry,
    scope: PromptAssemblyScope,
    extra_prompt_bodies: &HashMap<String, String>,
    skills_by_name: &HashMap<String, DiscoveredSkill>,
) -> Option<String> {
    match entry.kind {
        PromptSourceKind::ExtraPrompt => extra_prompt_bodies
            .get(&scope_reference_key(scope, &entry.reference_id))
            .cloned(),
        PromptSourceKind::SkillDiscovery => Some(format_skill_discovery_body(
            skills_by_name
                .values()
                .filter(|skill| !skill.disable_model_invocation)
                .cloned()
                .collect(),
        )),
        PromptSourceKind::LongLivedSkill => skills_by_name
            .get(&entry.reference_id)
            .map(format_long_lived_skill_body),
        PromptSourceKind::CoreSystemPrompt => None,
    }
}

fn resolved_body_origin_for_entry(
    entry: &PersistedPromptAssemblyEntry,
    skills_by_name: &HashMap<String, DiscoveredSkill>,
) -> Option<PromptSourceOrigin> {
    match entry.kind {
        PromptSourceKind::LongLivedSkill => skills_by_name
            .get(&entry.reference_id)
            .map(|skill| skill.origin),
        _ => None,
    }
}

fn indexed_extra_prompt_bodies(
    global_state: &PromptAssemblyScopeState,
    project_state: &PromptAssemblyScopeState,
) -> HashMap<String, String> {
    let mut bodies = HashMap::new();
    insert_extra_prompt_bodies(&mut bodies, global_state);
    insert_extra_prompt_bodies(&mut bodies, project_state);
    bodies
}

fn insert_extra_prompt_bodies(
    indexed: &mut HashMap<String, String>,
    state: &PromptAssemblyScopeState,
) {
    for StoredPromptBody {
        reference_id, body, ..
    } in &state.extra_prompts
    {
        indexed.insert(
            scope_reference_key(state.scope, reference_id),
            body.trim().to_string(),
        );
    }
}

fn candidate_body_key(
    origin: Option<PromptSourceOrigin>,
    kind: PromptSourceKind,
    reference_id: &str,
) -> String {
    format!(
        "{}::{:?}::{reference_id}",
        match origin {
            Some(PromptSourceOrigin::Builtin) => "builtin",
            Some(PromptSourceOrigin::Global) => "global",
            Some(PromptSourceOrigin::Project) => "project",
            None => "none",
        },
        kind
    )
}

fn scope_origin(scope: PromptAssemblyScope) -> PromptSourceOrigin {
    match scope {
        PromptAssemblyScope::Global => PromptSourceOrigin::Global,
        PromptAssemblyScope::Project => PromptSourceOrigin::Project,
    }
}

fn scope_state_mut<'a>(
    global_state: &'a mut PromptAssemblyScopeState,
    project_state: &'a mut PromptAssemblyScopeState,
    scope: PromptAssemblyScope,
) -> &'a mut PromptAssemblyScopeState {
    match scope {
        PromptAssemblyScope::Global => global_state,
        PromptAssemblyScope::Project => project_state,
    }
}

fn derive_extra_prompt_title(body: &str, fallback: &str) -> String {
    for line in body.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Some(heading) = trimmed.strip_prefix('#') {
            let title = heading.trim_start_matches('#').trim();
            if !title.is_empty() {
                return truncate_title(title);
            }
        }
        return truncate_title(trimmed);
    }
    truncate_title(fallback)
}

fn truncate_title(title: &str) -> String {
    const TITLE_LIMIT: usize = 80;
    let mut result = String::new();
    for character in title.chars().take(TITLE_LIMIT) {
        result.push(character);
    }
    result
}

fn generate_extra_prompt_reference_id(title: &str) -> String {
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
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    format!("{slug}-{stamp}")
}

fn next_requested_order(entries: &[PersistedPromptAssemblyEntry]) -> u16 {
    entries
        .iter()
        .filter_map(|entry| entry.requested_order)
        .max()
        .unwrap_or(0)
        .saturating_add(10)
}

fn activate_long_lived_skill(
    global_state: &mut PromptAssemblyScopeState,
    project_state: &mut PromptAssemblyScopeState,
    scope: PromptAssemblyScope,
    skill_name: &str,
) {
    let state = scope_state_mut(global_state, project_state, scope);
    let next_order = next_requested_order(&state.entries);
    if let Some(entry) = state.entries.iter_mut().find(|entry| {
        entry.kind == PromptSourceKind::LongLivedSkill && entry.reference_id == skill_name
    }) {
        entry.enabled = true;
        if entry.requested_order.is_none() {
            entry.requested_order = Some(next_order);
        }
        return;
    }

    state.entries.push(PersistedPromptAssemblyEntry {
        reference_id: skill_name.to_string(),
        kind: PromptSourceKind::LongLivedSkill,
        title: skill_name.to_string(),
        enabled: true,
        requested_order: Some(next_order),
    });
}

fn remove_prompt_source(
    state: &mut PromptAssemblyScopeState,
    kind: PromptSourceKind,
    reference_id: &str,
) {
    state
        .entries
        .retain(|entry| !(entry.kind == kind && entry.reference_id == reference_id));
}

fn move_active_source(
    global_state: &mut PromptAssemblyScopeState,
    project_state: &mut PromptAssemblyScopeState,
    scope: PromptAssemblyScope,
    kind: PromptSourceKind,
    reference_id: &str,
    direction: PromptAssemblyMoveDirection,
) -> Result<()> {
    let Some(current) = find_entry_address(global_state, project_state, scope, kind, reference_id)
    else {
        return Ok(());
    };
    let ordered = ordered_non_core_entry_addresses(global_state, project_state);
    let Some(position) = ordered.iter().position(|address| *address == current) else {
        return Ok(());
    };
    let neighbor_position = match direction {
        PromptAssemblyMoveDirection::Up => position.checked_sub(1),
        PromptAssemblyMoveDirection::Down => (position + 1 < ordered.len()).then_some(position + 1),
    };
    let Some(neighbor_position) = neighbor_position else {
        return Ok(());
    };
    let neighbor = ordered[neighbor_position];

    let current_order = entry_requested_order(global_state, project_state, current);
    let neighbor_order = entry_requested_order(global_state, project_state, neighbor);
    set_entry_requested_order(global_state, project_state, current, neighbor_order);
    set_entry_requested_order(global_state, project_state, neighbor, current_order);
    normalize_requested_orders(global_state, project_state);
    Ok(())
}

fn find_entry_address(
    global_state: &PromptAssemblyScopeState,
    project_state: &PromptAssemblyScopeState,
    scope: PromptAssemblyScope,
    kind: PromptSourceKind,
    reference_id: &str,
) -> Option<PromptEntryAddress> {
    let state = match scope {
        PromptAssemblyScope::Global => global_state,
        PromptAssemblyScope::Project => project_state,
    };
    state
        .entries
        .iter()
        .position(|entry| entry.kind == kind && entry.reference_id == reference_id)
        .map(|index| PromptEntryAddress { scope, index })
}

fn ordered_non_core_entry_addresses(
    global_state: &PromptAssemblyScopeState,
    project_state: &PromptAssemblyScopeState,
) -> Vec<PromptEntryAddress> {
    let mut addresses = Vec::new();
    addresses.extend(state_entry_addresses(global_state));
    addresses.extend(state_entry_addresses(project_state));
    addresses.sort_by(|left, right| {
        let left_entry = entry_ref(global_state, project_state, *left);
        let right_entry = entry_ref(global_state, project_state, *right);
        left_entry
            .requested_order
            .unwrap_or(u16::MAX)
            .cmp(&right_entry.requested_order.unwrap_or(u16::MAX))
            .then_with(|| left_entry.reference_id.cmp(&right_entry.reference_id))
            .then_with(|| {
                left.scope
                    .as_stored_value()
                    .cmp(right.scope.as_stored_value())
            })
    });
    addresses
}

fn state_entry_addresses(state: &PromptAssemblyScopeState) -> Vec<PromptEntryAddress> {
    state
        .entries
        .iter()
        .enumerate()
        .map(|(index, _)| PromptEntryAddress {
            scope: state.scope,
            index,
        })
        .collect()
}

fn entry_ref<'a>(
    global_state: &'a PromptAssemblyScopeState,
    project_state: &'a PromptAssemblyScopeState,
    address: PromptEntryAddress,
) -> &'a PersistedPromptAssemblyEntry {
    match address.scope {
        PromptAssemblyScope::Global => &global_state.entries[address.index],
        PromptAssemblyScope::Project => &project_state.entries[address.index],
    }
}

fn entry_mut<'a>(
    global_state: &'a mut PromptAssemblyScopeState,
    project_state: &'a mut PromptAssemblyScopeState,
    address: PromptEntryAddress,
) -> &'a mut PersistedPromptAssemblyEntry {
    match address.scope {
        PromptAssemblyScope::Global => &mut global_state.entries[address.index],
        PromptAssemblyScope::Project => &mut project_state.entries[address.index],
    }
}

fn entry_requested_order(
    global_state: &PromptAssemblyScopeState,
    project_state: &PromptAssemblyScopeState,
    address: PromptEntryAddress,
) -> Option<u16> {
    entry_ref(global_state, project_state, address).requested_order
}

fn set_entry_requested_order(
    global_state: &mut PromptAssemblyScopeState,
    project_state: &mut PromptAssemblyScopeState,
    address: PromptEntryAddress,
    requested_order: Option<u16>,
) {
    entry_mut(global_state, project_state, address).requested_order = requested_order;
}

fn normalize_requested_orders(
    global_state: &mut PromptAssemblyScopeState,
    project_state: &mut PromptAssemblyScopeState,
) {
    let ordered = ordered_non_core_entry_addresses(global_state, project_state);
    for (index, address) in ordered.into_iter().enumerate() {
        let normalized = u16::try_from((index + 1) * 10).unwrap_or(u16::MAX);
        set_entry_requested_order(global_state, project_state, address, Some(normalized));
    }
}

fn discovered_skill_inventory(
    discovered_skills: &[DiscoveredSkill],
    global_state: &PromptAssemblyScopeState,
    project_state: &PromptAssemblyScopeState,
) -> Vec<PromptAssemblyDiscoveredSkill> {
    discovered_skills
        .iter()
        .filter(|skill| {
            !has_long_lived_skill_entry(global_state, &skill.name)
                && !has_long_lived_skill_entry(project_state, &skill.name)
        })
        .map(|skill| PromptAssemblyDiscoveredSkill {
            skill_name: skill.name.clone(),
            title: skill.name.clone(),
            description: skill.description.clone(),
            origin: skill.origin,
            body: format_long_lived_skill_body(skill),
        })
        .collect()
}

fn has_long_lived_skill_entry(state: &PromptAssemblyScopeState, skill_name: &str) -> bool {
    state.entries.iter().any(|entry| {
        entry.kind == PromptSourceKind::LongLivedSkill && entry.reference_id == skill_name
    })
}

fn discover_skills(
    work_dir: &Path,
    global_skill_root_override: Option<&Path>,
) -> Vec<DiscoveredSkill> {
    let mut discovered = Vec::new();
    let mut seen_names = HashMap::<String, usize>::new();

    for path in project_skill_search_dirs(work_dir) {
        discover_skills_from_root(
            &path,
            PromptSourceOrigin::Project,
            &mut discovered,
            &mut seen_names,
        );
    }

    if let Some(global_root) = global_skill_root_override
        .map(Path::to_path_buf)
        .or_else(global_skill_root)
    {
        discover_skills_from_root(
            &global_root,
            PromptSourceOrigin::Global,
            &mut discovered,
            &mut seen_names,
        );
    }

    discovered
}

fn discover_skills_from_root(
    root: &Path,
    origin: PromptSourceOrigin,
    discovered: &mut Vec<DiscoveredSkill>,
    seen_names: &mut HashMap<String, usize>,
) {
    if !root.is_dir() {
        return;
    }

    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        discover_skill_dir(&path, origin, discovered, seen_names);
    }
}

fn discover_skill_dir(
    dir: &Path,
    origin: PromptSourceOrigin,
    discovered: &mut Vec<DiscoveredSkill>,
    seen_names: &mut HashMap<String, usize>,
) {
    let skill_path = dir.join(SKILL_FILE_NAME);
    if skill_path.is_file() {
        if let Some(skill) = parse_skill_file(&skill_path, origin) {
            if seen_names.contains_key(&skill.name) {
                return;
            }
            let next_index = discovered.len();
            seen_names.insert(skill.name.clone(), next_index);
            discovered.push(skill);
        }
        return;
    }

    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            discover_skill_dir(&path, origin, discovered, seen_names);
        }
    }
}

fn parse_skill_file(skill_path: &Path, origin: PromptSourceOrigin) -> Option<DiscoveredSkill> {
    let content = fs::read_to_string(skill_path).ok()?;
    let (frontmatter, body) = split_frontmatter(&content)?;
    let name = frontmatter
        .get("name")
        .map(String::as_str)?
        .trim()
        .to_string();
    let description = frontmatter
        .get("description")
        .map(String::as_str)?
        .trim()
        .to_string();
    if name.is_empty() || description.is_empty() {
        return None;
    }

    Some(DiscoveredSkill {
        name,
        description,
        skill_path: skill_path.to_path_buf(),
        body: body.trim().to_string(),
        origin,
        disable_model_invocation: frontmatter
            .get("disable-model-invocation")
            .is_some_and(|value| value.trim() == "true"),
    })
}

fn split_frontmatter(content: &str) -> Option<(HashMap<String, String>, &str)> {
    let trimmed = content.strip_prefix("---\n")?;
    let (frontmatter, body) = trimmed.split_once("\n---\n")?;
    let mut parsed = HashMap::new();
    for line in frontmatter.lines() {
        let (key, value) = line.split_once(':')?;
        parsed.insert(key.trim().to_string(), value.trim().to_string());
    }
    Some((parsed, body))
}

fn format_skill_discovery_body(skills: Vec<DiscoveredSkill>) -> String {
    if skills.is_empty() {
        return String::new();
    }

    let mut lines = vec![
        "The following skills provide specialized instructions for specific tasks.".to_string(),
        "Use the read tool to load a skill's file when the task matches its description."
            .to_string(),
        "When a skill file references a relative path, resolve it against the skill directory."
            .to_string(),
        String::new(),
        "<available_skills>".to_string(),
    ];
    for skill in skills {
        lines.push("  <skill>".to_string());
        lines.push(format!("    <name>{}</name>", escape_xml(&skill.name)));
        lines.push(format!(
            "    <description>{}</description>",
            escape_xml(&skill.description)
        ));
        lines.push(format!(
            "    <location>{}</location>",
            escape_xml(&skill.skill_path.display().to_string())
        ));
        lines.push("  </skill>".to_string());
    }
    lines.push("</available_skills>".to_string());
    lines.join("\n")
}

fn format_long_lived_skill_body(skill: &DiscoveredSkill) -> String {
    format!(
        "<skill>\n<name>{}</name>\n<path>{}</path>\n{}\n</skill>",
        skill.name,
        skill.skill_path.display(),
        skill.body.trim()
    )
}

fn extract_manual_skill_mentions(user_text: &str) -> Vec<String> {
    user_text
        .split_whitespace()
        .filter_map(|token| token.strip_prefix('$'))
        .filter(|skill_name| {
            !skill_name.is_empty()
                && skill_name.chars().all(|character| {
                    character.is_ascii_lowercase()
                        || character.is_ascii_digit()
                        || matches!(character, '-' | ':')
                })
        })
        .map(str::to_string)
        .collect()
}

fn escape_xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn project_skill_search_dirs(work_dir: &Path) -> Vec<PathBuf> {
    let canonical = fs::canonicalize(work_dir).unwrap_or_else(|_| work_dir.to_path_buf());
    let stop_dir = git_root(&canonical);
    let mut dirs = Vec::new();
    let mut current = Some(canonical.as_path());
    while let Some(dir) = current {
        dirs.push(dir.join(".agents").join("skills"));
        if stop_dir.as_deref() == Some(dir) {
            break;
        }
        current = dir.parent();
    }
    dirs
}

fn git_root(start: &Path) -> Option<PathBuf> {
    let mut current = Some(start);
    while let Some(dir) = current {
        let git_path = dir.join(".git");
        if git_path.is_dir() || git_path.is_file() {
            return Some(dir.to_path_buf());
        }
        current = dir.parent();
    }
    None
}

fn global_skill_root() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join(".agents").join("skills"))
}

fn scope_reference_key(scope: PromptAssemblyScope, reference_id: &str) -> String {
    format!("{}::{reference_id}", scope.as_stored_value())
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtime_domain::prompt_assembly::persistence::{
        PersistedPromptAssemblyEntry, PromptAssemblyScope, PromptAssemblyScopeState,
        StoredPromptBody, save_project_prompt_assembly_state,
    };
    use session_store::InMemorySessionStore;

    fn temp_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "hunea-terminal-app-prompt-assembly-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock should be after epoch")
                .as_nanos()
        ));
        fs::create_dir_all(&dir).expect("temp dir should exist");
        dir
    }

    #[test]
    fn resolve_initial_prompt_prelude_orders_core_extra_discovery_and_long_lived_skill() {
        let work_dir = temp_dir("resolve");
        let project_skill_dir = work_dir.join(".agents/skills/repo-bootstrap");
        fs::create_dir_all(&project_skill_dir).expect("skill dir should exist");
        fs::write(
            project_skill_dir.join(SKILL_FILE_NAME),
            "---\nname: repo-bootstrap\ndescription: Bootstrap repo\ndisable-model-invocation: false\n---\n# Repo Bootstrap\n\nUse this skill.\n",
        )
        .expect("skill file should exist");

        let global_state = PromptAssemblyScopeState {
            scope: PromptAssemblyScope::Global,
            core_system_override: Some("global core".to_string()),
            entries: vec![
                PersistedPromptAssemblyEntry {
                    reference_id: "skill-discovery".to_string(),
                    kind: PromptSourceKind::SkillDiscovery,
                    title: "Skill discovery source".to_string(),
                    enabled: true,
                    requested_order: Some(20),
                },
                PersistedPromptAssemblyEntry {
                    reference_id: "repo-bootstrap".to_string(),
                    kind: PromptSourceKind::LongLivedSkill,
                    title: "repo-bootstrap".to_string(),
                    enabled: true,
                    requested_order: Some(30),
                },
            ],
            extra_prompts: Vec::new(),
        };
        let project_state = PromptAssemblyScopeState {
            scope: PromptAssemblyScope::Project,
            core_system_override: None,
            entries: vec![PersistedPromptAssemblyEntry {
                reference_id: "repo-rules".to_string(),
                kind: PromptSourceKind::ExtraPrompt,
                title: "repo-rules".to_string(),
                enabled: true,
                requested_order: Some(10),
            }],
            extra_prompts: vec![StoredPromptBody {
                reference_id: "repo-rules".to_string(),
                title: "repo-rules".to_string(),
                body: "project rules".to_string(),
            }],
        };

        let prelude = resolve_initial_prompt_prelude(&work_dir, &global_state, &project_state);

        assert_eq!(prelude.sections.len(), 4);
        assert_eq!(prelude.sections[0].kind, PromptSourceKind::CoreSystemPrompt);
        assert_eq!(prelude.sections[1].reference_id, "repo-rules");
        assert_eq!(prelude.sections[2].reference_id, "skill-discovery");
        assert_eq!(prelude.sections[3].reference_id, "repo-bootstrap");
        let effective = prelude
            .effective_system_prompt()
            .expect("effective prompt should exist");
        assert!(effective.starts_with("global core\n\nproject rules\n\n"));
        assert!(effective.contains("<available_skills>"));
        assert!(effective.contains("<name>repo-bootstrap</name>"));
        assert!(effective.contains("<skill>\n<name>repo-bootstrap</name>"));
    }

    #[test]
    fn resolve_initial_prompt_assembly_keeps_inactive_sources_for_manager_view() {
        let work_dir = temp_dir("snapshot");
        let resolved = resolve_prompt_assembly_manager_snapshot(
            &work_dir,
            &PromptAssemblyScopeState {
                scope: PromptAssemblyScope::Global,
                core_system_override: None,
                entries: vec![
                    PersistedPromptAssemblyEntry {
                        reference_id: "disabled".to_string(),
                        kind: PromptSourceKind::ExtraPrompt,
                        title: "disabled".to_string(),
                        enabled: false,
                        requested_order: Some(10),
                    },
                    PersistedPromptAssemblyEntry {
                        reference_id: "missing".to_string(),
                        kind: PromptSourceKind::LongLivedSkill,
                        title: "missing".to_string(),
                        enabled: true,
                        requested_order: Some(20),
                    },
                ],
                extra_prompts: Vec::new(),
            },
            &PromptAssemblyScopeState::empty(PromptAssemblyScope::Project),
        );

        assert_eq!(resolved.snapshot.active_sources.len(), 1);
        assert_eq!(
            resolved
                .snapshot
                .inactive_sources
                .iter()
                .map(|source| source.reference_id.as_str())
                .collect::<Vec<_>>(),
            vec!["disabled", "missing"]
        );
    }

    #[test]
    fn load_initial_prompt_prelude_reads_global_and_project_state() {
        let work_dir = temp_dir("load");
        let global_store: Arc<dyn SessionStore> = Arc::new(InMemorySessionStore::new());
        let project_state = PromptAssemblyScopeState {
            scope: PromptAssemblyScope::Project,
            core_system_override: Some("project core".to_string()),
            entries: vec![PersistedPromptAssemblyEntry {
                reference_id: "repo-rules".to_string(),
                kind: PromptSourceKind::ExtraPrompt,
                title: "repo-rules".to_string(),
                enabled: true,
                requested_order: Some(10),
            }],
            extra_prompts: vec![StoredPromptBody {
                reference_id: "repo-rules".to_string(),
                title: "repo-rules".to_string(),
                body: "project rules".to_string(),
            }],
        };
        save_project_prompt_assembly_state(&work_dir, &project_state)
            .expect("project state should save");
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime should build");
        runtime
            .block_on(
                global_store.save_global_prompt_assembly_state(&PromptAssemblyScopeState {
                    scope: PromptAssemblyScope::Global,
                    core_system_override: Some("global core".to_string()),
                    entries: Vec::new(),
                    extra_prompts: Vec::new(),
                }),
            )
            .expect("global state should save");

        let prelude =
            load_initial_prompt_prelude(global_store, &work_dir).expect("prelude should load");

        assert_eq!(
            prelude.effective_system_prompt().as_deref(),
            Some("project core\n\nproject rules")
        );
    }

    #[test]
    fn load_initial_prompt_assembly_reads_snapshot_and_prelude() {
        let work_dir = temp_dir("load-snapshot");
        let global_store: Arc<dyn SessionStore> = Arc::new(InMemorySessionStore::new());
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime should build");
        runtime
            .block_on(
                global_store.save_global_prompt_assembly_state(&PromptAssemblyScopeState {
                    scope: PromptAssemblyScope::Global,
                    core_system_override: Some("global core".to_string()),
                    entries: vec![PersistedPromptAssemblyEntry {
                        reference_id: "disabled".to_string(),
                        kind: PromptSourceKind::ExtraPrompt,
                        title: "disabled".to_string(),
                        enabled: false,
                        requested_order: Some(10),
                    }],
                    extra_prompts: Vec::new(),
                }),
            )
            .expect("global state should save");

        let loaded =
            load_initial_prompt_assembly(global_store, &work_dir).expect("snapshot should load");

        assert_eq!(
            loaded.prelude.effective_system_prompt().as_deref(),
            Some("global core")
        );
        assert_eq!(
            loaded
                .snapshot
                .inactive_sources
                .iter()
                .map(|source| source.reference_id.as_str())
                .collect::<Vec<_>>(),
            vec!["disabled"]
        );
    }

    #[test]
    fn activate_long_lived_skill_persists_reference_and_expands_in_prelude() {
        let work_dir = temp_dir("activate-skill");
        let skill_dir = work_dir.join(".agents/skills/repo-bootstrap");
        fs::create_dir_all(&skill_dir).expect("skill dir should exist");
        fs::write(
            skill_dir.join(SKILL_FILE_NAME),
            "---\nname: repo-bootstrap\ndescription: Bootstrap repo\n---\n# Repo Bootstrap\n\nUse this skill.\n",
        )
        .expect("skill file should exist");

        let store: Arc<dyn SessionStore> = Arc::new(InMemorySessionStore::new());
        let snapshot = apply_prompt_assembly_mutation(
            store.clone(),
            &work_dir,
            PromptAssemblyMutation::ActivateLongLivedSkill {
                scope: PromptAssemblyScope::Project,
                skill_name: "repo-bootstrap".to_string(),
            },
        )
        .expect("mutation should succeed");

        assert_eq!(
            snapshot
                .snapshot
                .active_sources
                .iter()
                .map(|source| source.reference_id.as_str())
                .collect::<Vec<_>>(),
            vec!["core-system", "repo-bootstrap"]
        );
        assert!(
            !snapshot
                .discovered_skills
                .iter()
                .any(|skill| skill.skill_name == "repo-bootstrap")
        );
        assert!(
            snapshot
                .prelude
                .effective_system_prompt()
                .expect("effective prompt should exist")
                .contains("<name>repo-bootstrap</name>")
        );

        let project_state = load_project_prompt_assembly_state(&work_dir)
            .expect("project prompt assembly state should load");
        assert_eq!(
            project_state.entries,
            vec![PersistedPromptAssemblyEntry {
                reference_id: "repo-bootstrap".to_string(),
                kind: PromptSourceKind::LongLivedSkill,
                title: "repo-bootstrap".to_string(),
                enabled: true,
                requested_order: Some(10),
            }]
        );
    }

    #[test]
    fn active_long_lived_skill_prefers_project_skill_when_names_collide() {
        let work_dir = temp_dir("skill-precedence");
        let project_skill_dir = work_dir.join(".agents/skills/repo-bootstrap");
        fs::create_dir_all(&project_skill_dir).expect("project skill dir should exist");
        fs::write(
            project_skill_dir.join(SKILL_FILE_NAME),
            "---\nname: repo-bootstrap\ndescription: Project bootstrap\n---\n# Project Bootstrap\n\nproject body\n",
        )
        .expect("project skill file should exist");

        let home_dir = temp_dir("skill-precedence-home");
        let global_skill_dir = home_dir.join(".agents/skills/repo-bootstrap");
        fs::create_dir_all(&global_skill_dir).expect("global skill dir should exist");
        fs::write(
            global_skill_dir.join(SKILL_FILE_NAME),
            "---\nname: repo-bootstrap\ndescription: Global bootstrap\n---\n# Global Bootstrap\n\nglobal body\n",
        )
        .expect("global skill file should exist");
        let snapshot = resolve_prompt_assembly_manager_snapshot_with_global_skill_root(
            &work_dir,
            &PromptAssemblyScopeState {
                scope: PromptAssemblyScope::Global,
                core_system_override: None,
                entries: vec![PersistedPromptAssemblyEntry {
                    reference_id: "repo-bootstrap".to_string(),
                    kind: PromptSourceKind::LongLivedSkill,
                    title: "repo-bootstrap".to_string(),
                    enabled: true,
                    requested_order: Some(10),
                }],
                extra_prompts: Vec::new(),
            },
            &PromptAssemblyScopeState::empty(PromptAssemblyScope::Project),
            Some(&home_dir.join(".agents").join("skills")),
        );

        let skill_source = snapshot
            .sources
            .iter()
            .find(|source| {
                source.kind == PromptSourceKind::LongLivedSkill
                    && source.reference_id == "repo-bootstrap"
            })
            .expect("long-lived skill source should exist");
        assert_eq!(skill_source.origin, Some(PromptSourceOrigin::Global));
        assert_eq!(
            skill_source.resolved_body_origin,
            Some(PromptSourceOrigin::Project)
        );
        assert!(
            skill_source
                .body
                .as_deref()
                .expect("skill body should exist")
                .contains("project body")
        );
    }

    #[test]
    fn move_active_source_reorders_non_core_entries() {
        let work_dir = temp_dir("move-order");
        let store: Arc<dyn SessionStore> = Arc::new(InMemorySessionStore::new());
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime should build");
        runtime
            .block_on(
                store.save_global_prompt_assembly_state(&PromptAssemblyScopeState {
                    scope: PromptAssemblyScope::Global,
                    core_system_override: None,
                    entries: vec![PersistedPromptAssemblyEntry {
                        reference_id: "shared-rules".to_string(),
                        kind: PromptSourceKind::ExtraPrompt,
                        title: "shared-rules".to_string(),
                        enabled: true,
                        requested_order: Some(10),
                    }],
                    extra_prompts: vec![StoredPromptBody {
                        reference_id: "shared-rules".to_string(),
                        title: "shared-rules".to_string(),
                        body: "global rules".to_string(),
                    }],
                }),
            )
            .expect("global state should save");
        save_project_prompt_assembly_state(
            &work_dir,
            &PromptAssemblyScopeState {
                scope: PromptAssemblyScope::Project,
                core_system_override: None,
                entries: vec![PersistedPromptAssemblyEntry {
                    reference_id: "repo-rules".to_string(),
                    kind: PromptSourceKind::ExtraPrompt,
                    title: "repo-rules".to_string(),
                    enabled: true,
                    requested_order: Some(20),
                }],
                extra_prompts: vec![StoredPromptBody {
                    reference_id: "repo-rules".to_string(),
                    title: "repo-rules".to_string(),
                    body: "project rules".to_string(),
                }],
            },
        )
        .expect("project state should save");

        let snapshot = apply_prompt_assembly_mutation(
            store.clone(),
            &work_dir,
            PromptAssemblyMutation::MoveActiveSource {
                scope: PromptAssemblyScope::Project,
                kind: PromptSourceKind::ExtraPrompt,
                reference_id: "repo-rules".to_string(),
                direction: PromptAssemblyMoveDirection::Up,
            },
        )
        .expect("move should succeed");

        assert_eq!(
            snapshot
                .snapshot
                .active_sources
                .iter()
                .map(|source| source.reference_id.as_str())
                .collect::<Vec<_>>(),
            vec!["core-system", "repo-rules", "shared-rules"]
        );
    }

    #[test]
    fn missing_source_check_counts_missing_entries_without_blocking_snapshot_resolution() {
        let manager = resolve_prompt_assembly_manager_snapshot(
            &temp_dir("missing-check"),
            &PromptAssemblyScopeState {
                scope: PromptAssemblyScope::Global,
                core_system_override: None,
                entries: vec![
                    PersistedPromptAssemblyEntry {
                        reference_id: "missing-skill".to_string(),
                        kind: PromptSourceKind::LongLivedSkill,
                        title: "missing-skill".to_string(),
                        enabled: true,
                        requested_order: Some(10),
                    },
                    PersistedPromptAssemblyEntry {
                        reference_id: "disabled-extra".to_string(),
                        kind: PromptSourceKind::ExtraPrompt,
                        title: "disabled-extra".to_string(),
                        enabled: false,
                        requested_order: Some(20),
                    },
                ],
                extra_prompts: Vec::new(),
            },
            &PromptAssemblyScopeState::empty(PromptAssemblyScope::Project),
        );

        let check = PromptAssemblyMissingSourcesCheck::from_manager(&manager);

        assert_eq!(check.missing_count, 1);
        assert!(manager.snapshot.inactive_sources.iter().any(|source| {
            source.reference_id == "missing-skill"
                && matches!(
                    source.status,
                    PromptSourceStatus::Inactive {
                        reason:
                            runtime_domain::prompt_assembly::PromptSourceInactiveReason::Missing
                    }
                )
        }));
    }

    #[test]
    fn assemble_manual_skill_message_expands_unique_mentions_in_first_use_order() {
        let work_dir = temp_dir("manual-skill-assembly");
        let repo_bootstrap_dir = work_dir.join(".agents/skills/repo-bootstrap");
        fs::create_dir_all(&repo_bootstrap_dir).expect("repo-bootstrap dir should exist");
        fs::write(
            repo_bootstrap_dir.join(SKILL_FILE_NAME),
            "---\nname: repo-bootstrap\ndescription: Bootstrap repo\n---\n# Repo Bootstrap\n\nBootstrap steps.\n",
        )
        .expect("repo-bootstrap skill should write");
        let code_review_dir = work_dir.join(".agents/skills/code-review");
        fs::create_dir_all(&code_review_dir).expect("code-review dir should exist");
        fs::write(
            code_review_dir.join(SKILL_FILE_NAME),
            "---\nname: code-review\ndescription: Review code\n---\n# Code Review\n\nReview carefully.\n",
        )
        .expect("code-review skill should write");

        let assembled = assemble_manual_skill_message(
            &work_dir,
            "Please use $repo-bootstrap before $code-review and repeat $repo-bootstrap",
        );

        assert_eq!(
            assembled
                .uses
                .iter()
                .map(|skill| skill.skill_name.as_str())
                .collect::<Vec<_>>(),
            vec!["repo-bootstrap", "code-review"]
        );
        assert_eq!(
            assembled.provider_visible_user_text,
            format!(
                "{}\n\n{}\n\nPlease use $repo-bootstrap before $code-review and repeat $repo-bootstrap",
                format_long_lived_skill_body(&DiscoveredSkill {
                    name: "repo-bootstrap".to_string(),
                    description: "Bootstrap repo".to_string(),
                    skill_path: repo_bootstrap_dir.join(SKILL_FILE_NAME),
                    body: "# Repo Bootstrap\n\nBootstrap steps.".to_string(),
                    origin: PromptSourceOrigin::Project,
                    disable_model_invocation: false,
                }),
                format_long_lived_skill_body(&DiscoveredSkill {
                    name: "code-review".to_string(),
                    description: "Review code".to_string(),
                    skill_path: code_review_dir.join(SKILL_FILE_NAME),
                    body: "# Code Review\n\nReview carefully.".to_string(),
                    origin: PromptSourceOrigin::Project,
                    disable_model_invocation: false,
                }),
            )
        );
    }
}
