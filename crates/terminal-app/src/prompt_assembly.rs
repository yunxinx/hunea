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
    CoreSystemPromptInput, PromptAssemblyEditorTarget, PromptAssemblyInput,
    PromptAssemblyManagerSnapshot, PromptAssemblyManagerSource, PromptAssemblyMutation,
    PromptPreludeSection, PromptPreludeSnapshot, PromptSourceCandidate, PromptSourceKind,
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

#[derive(Debug, Clone)]
struct PromptCandidateBody {
    body: String,
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
    let discovered_skills = discover_skills(work_dir);
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
            let discovered = discover_skills(work_dir);
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

fn discover_skills(work_dir: &Path) -> Vec<DiscoveredSkill> {
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

    if let Some(global_root) = global_skill_root() {
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
}
