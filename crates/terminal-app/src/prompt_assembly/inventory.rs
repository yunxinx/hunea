use super::*;

pub(super) fn discovered_skill_inventory(
    discovered_skills: &[DiscoveredSkill],
    global_state: &PromptAssemblyScopeState,
    project_state: &PromptAssemblyScopeState,
    skill_state: &[PersistedSkillDiscoverySkillEntry],
) -> Vec<PromptAssemblyDiscoveredSkill> {
    let selection_scope =
        skill_discovery_scope(global_state, project_state, PromptAssemblyScope::Project);
    let global_state_by_name = global_state
        .skill_discovery_skills()
        .iter()
        .map(|entry| (entry.skill_name.as_str(), entry))
        .collect::<HashMap<_, _>>();
    let project_state_by_name = project_state
        .skill_discovery_skills()
        .iter()
        .map(|entry| (entry.skill_name.as_str(), entry))
        .collect::<HashMap<_, _>>();
    let merged_state_by_name = skill_state
        .iter()
        .map(|entry| (entry.skill_name.as_str(), entry))
        .collect::<HashMap<_, _>>();
    let selected_order_by_name = skill_state
        .iter()
        .enumerate()
        .map(|(index, entry)| (entry.skill_name.as_str(), index + 1))
        .collect::<HashMap<_, _>>();
    let inventory = discovered_skills
        .iter()
        .map(|skill| PromptAssemblyDiscoveredSkill {
            skill_name: skill.name.clone(),
            title: skill.name.clone(),
            description: skill.description.clone(),
            origin: skill.origin,
            selection_scope,
            skill_path: skill.skill_path.display().to_string(),
            body: format_long_lived_skill_body(skill),
            can_select_for_discovery: skill.can_select_for_discovery(),
            selected: prompt_overlay_skill_state_for_origin(
                skill.origin,
                skill.name.as_str(),
                &global_state_by_name,
                &project_state_by_name,
            )
            .or_else(|| merged_state_by_name.get(skill.name.as_str()).copied())
            .map(|entry| entry.enabled)
            .unwrap_or(skill.can_select_for_discovery()),
            selected_order: selected_order_by_name.get(skill.name.as_str()).copied(),
        })
        .collect::<Vec<_>>();
    let (mut discovery_eligible, mut manual_only): (Vec<_>, Vec<_>) = inventory
        .into_iter()
        .partition(|skill| skill.can_select_for_discovery);
    discovery_eligible.sort_by(|left, right| {
        left.selected_order
            .unwrap_or(usize::MAX)
            .cmp(&right.selected_order.unwrap_or(usize::MAX))
            .then_with(|| natural_sort_text_cmp(&left.title, &right.title))
            .then_with(|| natural_sort_text_cmp(&left.skill_name, &right.skill_name))
            .then_with(|| {
                prompt_source_origin_sort_key(left.origin)
                    .cmp(&prompt_source_origin_sort_key(right.origin))
            })
    });
    manual_only.sort_by(|left, right| {
        natural_sort_text_cmp(&left.title, &right.title)
            .then_with(|| natural_sort_text_cmp(&left.skill_name, &right.skill_name))
            .then_with(|| {
                prompt_source_origin_sort_key(left.origin)
                    .cmp(&prompt_source_origin_sort_key(right.origin))
            })
    });
    discovery_eligible.extend(manual_only);
    discovery_eligible
}

pub(super) fn prompt_overlay_skill_state_for_origin<'a>(
    origin: PromptSourceOrigin,
    skill_name: &str,
    global_state_by_name: &HashMap<&'a str, &'a PersistedSkillDiscoverySkillEntry>,
    project_state_by_name: &HashMap<&'a str, &'a PersistedSkillDiscoverySkillEntry>,
) -> Option<&'a PersistedSkillDiscoverySkillEntry> {
    match origin {
        PromptSourceOrigin::Global => global_state_by_name.get(skill_name).copied(),
        PromptSourceOrigin::Project => project_state_by_name.get(skill_name).copied(),
        PromptSourceOrigin::Builtin => None,
    }
}

pub(super) fn prompt_source_origin_sort_key(origin: PromptSourceOrigin) -> u8 {
    match origin {
        PromptSourceOrigin::Project => 0,
        PromptSourceOrigin::Global => 1,
        PromptSourceOrigin::Builtin => 2,
    }
}

pub(super) fn manual_skill_inventory(
    discovered_skills: &[DiscoveredSkill],
) -> Vec<PromptAssemblyDiscoveredSkill> {
    discovered_skills
        .iter()
        .map(|skill| PromptAssemblyDiscoveredSkill {
            skill_name: skill.name.clone(),
            title: skill.name.clone(),
            description: skill.description.clone(),
            origin: skill.origin,
            selection_scope: PromptAssemblyScope::Project,
            skill_path: skill.skill_path.display().to_string(),
            body: format_long_lived_skill_body(skill),
            can_select_for_discovery: skill.can_select_for_discovery(),
            selected: false,
            selected_order: None,
        })
        .collect()
}

pub(super) fn discover_instruction_files(
    work_dir: &Path,
    global_instructions_path_override: Option<&Path>,
) -> (
    Vec<DiscoveredInstructionsFile>,
    Vec<PromptAssemblyDiagnostic>,
) {
    let mut discovered = Vec::new();
    let mut diagnostics = Vec::new();

    if let Some(global_file) = global_instructions_path_override
        .map(Path::to_path_buf)
        .or_else(global_instructions_file_path)
        .filter(|path| path.is_file())
    {
        match load_instructions_file(
            "instructions:global",
            "Global AGENTS.md".to_string(),
            &global_file,
            PromptSourceOrigin::Global,
        ) {
            Ok(Some(file)) => discovered.push(file),
            Ok(None) => {}
            Err(diagnostic) => diagnostics.push(diagnostic),
        }
    }

    let project_root = git_root(work_dir);
    let search_dirs = match project_root.as_deref() {
        Some(root) => project_instruction_search_dirs(root, work_dir),
        None => vec![work_dir.to_path_buf()],
    };

    for directory in search_dirs {
        let Some(path) = first_instruction_file_in_dir(&directory) else {
            continue;
        };
        let reference_id = project_instruction_reference_id(project_root.as_deref(), &directory);
        let title = project_instruction_title(project_root.as_deref(), &path);
        match load_instructions_file(&reference_id, title, &path, PromptSourceOrigin::Project) {
            Ok(Some(file)) => discovered.push(file),
            Ok(None) => {}
            Err(diagnostic) => diagnostics.push(diagnostic),
        }
    }

    (discovered, diagnostics)
}

pub(super) fn load_instructions_file(
    reference_id: &str,
    title: String,
    path: &Path,
    origin: PromptSourceOrigin,
) -> Result<Option<DiscoveredInstructionsFile>, PromptAssemblyDiagnostic> {
    let body = fs::read_to_string(path).map_err(|error| PromptAssemblyDiagnostic {
        origin: Some(origin),
        path: Some(path.to_path_buf()),
        message: format!("read instructions file: {error}"),
    })?;
    let body = body.trim().to_string();
    if body.is_empty() {
        return Ok(None);
    }

    Ok(Some(DiscoveredInstructionsFile {
        reference_id: reference_id.to_string(),
        title,
        path: path.to_path_buf(),
        body,
        origin,
    }))
}

pub(super) fn global_instructions_file_path() -> Option<PathBuf> {
    hunea_config_dir().map(|dir| dir.join(GLOBAL_INSTRUCTIONS_FILE_NAME))
}

pub(super) fn project_instruction_search_dirs(
    project_root: &Path,
    work_dir: &Path,
) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let mut cursor = Some(work_dir);
    while let Some(dir) = cursor {
        dirs.push(dir.to_path_buf());
        if dir == project_root {
            break;
        }
        cursor = dir.parent();
    }
    dirs.reverse();
    dirs
}

pub(super) fn first_instruction_file_in_dir(dir: &Path) -> Option<PathBuf> {
    PROJECT_INSTRUCTIONS_FILE_NAMES
        .iter()
        .map(|name| dir.join(name))
        .find(|path| path.is_file())
}

pub(super) fn project_instruction_reference_id(
    project_root: Option<&Path>,
    directory: &Path,
) -> String {
    let relative = project_root
        .and_then(|root| directory.strip_prefix(root).ok())
        .map(path_component_key)
        .unwrap_or_else(|| ".".to_string());
    format!("instructions:project:{relative}")
}

pub(super) fn project_instruction_title(project_root: Option<&Path>, path: &Path) -> String {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(GLOBAL_INSTRUCTIONS_FILE_NAME);
    let Some(parent) = path.parent() else {
        return file_name.to_string();
    };
    let relative_directory = project_root
        .and_then(|root| parent.strip_prefix(root).ok())
        .map(path_component_key)
        .unwrap_or_else(|| ".".to_string());
    if relative_directory == "." {
        file_name.to_string()
    } else {
        format!("{relative_directory}/{file_name}")
    }
}

pub(super) fn path_component_key(path: &Path) -> String {
    let components = path
        .components()
        .filter_map(|component| match component {
            std::path::Component::Normal(value) => value.to_str(),
            _ => None,
        })
        .collect::<Vec<_>>();
    if components.is_empty() {
        ".".to_string()
    } else {
        components.join("/")
    }
}

pub(super) fn ensure_discovered_instruction_entries(
    global_state: &mut PromptAssemblyScopeState,
    project_state: &mut PromptAssemblyScopeState,
    discovered_instruction_files: &[DiscoveredInstructionsFile],
) {
    let mut global_files = discovered_instruction_files
        .iter()
        .filter(|file| file.origin == PromptSourceOrigin::Global)
        .cloned()
        .collect::<Vec<_>>();
    let mut project_files = discovered_instruction_files
        .iter()
        .filter(|file| file.origin == PromptSourceOrigin::Project)
        .cloned()
        .collect::<Vec<_>>();
    global_files.sort_by(|left, right| left.reference_id.cmp(&right.reference_id));
    project_files.sort_by(|left, right| left.reference_id.cmp(&right.reference_id));

    ensure_scope_instruction_entries(
        global_state,
        &global_files,
        DEFAULT_INSTRUCTIONS_REQUESTED_ORDER_START,
    );
    ensure_scope_instruction_entries(
        project_state,
        &project_files,
        DEFAULT_INSTRUCTIONS_REQUESTED_ORDER_START
            .saturating_add(u16::try_from(global_files.len()).unwrap_or(u16::MAX)),
    );
}

pub(super) fn ensure_scope_instruction_entries(
    state: &mut PromptAssemblyScopeState,
    discovered_instruction_files: &[DiscoveredInstructionsFile],
    starting_order: u16,
) {
    let mut next_instruction_order = state
        .entries()
        .iter()
        .filter(|entry| entry.kind == PromptSourceKind::InstructionsFile)
        .filter_map(|entry| entry.requested_order)
        .max()
        .map(|order| order.saturating_add(1))
        .unwrap_or(starting_order);

    for file in discovered_instruction_files {
        if let Some(entry) = state.entries_mut().iter_mut().find(|entry| {
            entry.kind == PromptSourceKind::InstructionsFile
                && entry.reference_id == file.reference_id
        }) {
            entry.title = file.title.clone();
            continue;
        }

        state.entries_mut().push(PersistedPromptAssemblyEntry {
            reference_id: file.reference_id.clone(),
            kind: PromptSourceKind::InstructionsFile,
            title: file.title.clone(),
            enabled: true,
            requested_order: Some(next_instruction_order),
        });
        next_instruction_order = next_instruction_order.saturating_add(1);
    }
}

pub(super) fn discover_skills(
    work_dir: &Path,
    global_skill_root_override: Option<&Path>,
) -> Vec<DiscoveredSkill> {
    discover_skills_with_diagnostics(work_dir, global_skill_root_override).0
}

pub(super) fn discover_skills_with_diagnostics(
    work_dir: &Path,
    global_skill_root_override: Option<&Path>,
) -> (Vec<DiscoveredSkill>, Vec<PromptAssemblyDiagnostic>) {
    let mut discovered = Vec::new();
    let mut diagnostics = Vec::new();
    let mut seen_names = HashMap::<(String, PromptSourceOrigin), usize>::new();

    for path in project_skill_search_dirs(work_dir) {
        discover_skills_from_root(
            &path,
            PromptSourceOrigin::Project,
            &mut discovered,
            &mut seen_names,
            &mut diagnostics,
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
            &mut diagnostics,
        );
    }

    (discovered, diagnostics)
}

pub(super) fn discover_skills_from_root(
    root: &Path,
    origin: PromptSourceOrigin,
    discovered: &mut Vec<DiscoveredSkill>,
    seen_names: &mut HashMap<(String, PromptSourceOrigin), usize>,
    diagnostics: &mut Vec<PromptAssemblyDiagnostic>,
) {
    if !root.is_dir() {
        return;
    }

    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) => {
            diagnostics.push(PromptAssemblyDiagnostic {
                origin: Some(origin),
                path: Some(root.to_path_buf()),
                message: format!("read skill directory: {error}"),
            });
            return;
        }
    };
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                diagnostics.push(PromptAssemblyDiagnostic {
                    origin: Some(origin),
                    path: Some(root.to_path_buf()),
                    message: format!("read skill directory entry: {error}"),
                });
                continue;
            }
        };
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        discover_skill_dir(&path, origin, discovered, seen_names, diagnostics);
    }
}

pub(super) fn discover_skill_dir(
    dir: &Path,
    origin: PromptSourceOrigin,
    discovered: &mut Vec<DiscoveredSkill>,
    seen_names: &mut HashMap<(String, PromptSourceOrigin), usize>,
    diagnostics: &mut Vec<PromptAssemblyDiagnostic>,
) {
    let skill_path = dir.join(SKILL_FILE_NAME);
    if skill_path.is_file() {
        match parse_skill_file(&skill_path, origin) {
            Ok(skill) => {
                let seen_key = (skill.name.clone(), origin);
                if seen_names.contains_key(&seen_key) {
                    return;
                }
                let next_index = discovered.len();
                seen_names.insert(seen_key, next_index);
                discovered.push(skill);
            }
            Err(error) => diagnostics.push(PromptAssemblyDiagnostic {
                origin: Some(origin),
                path: Some(skill_path),
                message: error.to_string(),
            }),
        }
        return;
    }

    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) => {
            diagnostics.push(PromptAssemblyDiagnostic {
                origin: Some(origin),
                path: Some(dir.to_path_buf()),
                message: format!("read nested skill directory: {error}"),
            });
            return;
        }
    };
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                diagnostics.push(PromptAssemblyDiagnostic {
                    origin: Some(origin),
                    path: Some(dir.to_path_buf()),
                    message: format!("read nested skill directory entry: {error}"),
                });
                continue;
            }
        };
        let path = entry.path();
        if path.is_dir() {
            discover_skill_dir(&path, origin, discovered, seen_names, diagnostics);
        }
    }
}

pub(super) fn parse_skill_file(
    skill_path: &Path,
    origin: PromptSourceOrigin,
) -> Result<DiscoveredSkill, SkillParseError> {
    let content = fs::read_to_string(skill_path).map_err(|source| SkillParseError::Read {
        path: skill_path.to_path_buf(),
        source,
    })?;
    let (frontmatter, body) =
        split_frontmatter(&content).ok_or(SkillParseError::MissingFrontmatter)?;
    let frontmatter: SkillFrontmatter = serde_yaml::from_str(frontmatter.as_str())
        .map_err(|source| SkillParseError::DecodeFrontmatter { source })?;
    let name = frontmatter
        .name
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .ok_or(SkillParseError::MissingName)?
        .to_string();
    let description = frontmatter
        .description
        .as_deref()
        .map(str::trim)
        .filter(|description| !description.is_empty())
        .ok_or(SkillParseError::MissingDescription)?
        .to_string();
    Ok(DiscoveredSkill {
        name,
        description,
        skill_path: skill_path.to_path_buf(),
        body: body.trim().to_string(),
        origin,
        disable_model_invocation: frontmatter.disable_model_invocation,
    })
}

#[derive(Debug, thiserror::Error)]
pub(super) enum SkillParseError {
    #[error("read skill file '{}': {source}", path.display())]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("missing YAML frontmatter")]
    MissingFrontmatter,
    #[error("decode skill frontmatter: {source}")]
    DecodeFrontmatter {
        #[source]
        source: serde_yaml::Error,
    },
    #[error("missing required name")]
    MissingName,
    #[error("missing required description")]
    MissingDescription,
}

pub(super) fn split_frontmatter(content: &str) -> Option<(String, String)> {
    let normalized = content.replace("\r\n", "\n").replace('\r', "\n");
    let trimmed = normalized.strip_prefix("---\n")?;
    let (frontmatter, body) = trimmed.split_once("\n---\n")?;
    Some((frontmatter.to_string(), body.to_string()))
}

pub(super) fn format_skill_discovery_body(skills: Vec<DiscoveredSkill>) -> String {
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

pub(super) fn format_long_lived_skill_body(skill: &DiscoveredSkill) -> String {
    format!(
        "<skill>\n<name>{}</name>\n<path>{}</path>\n{}\n</skill>",
        skill.name,
        skill.skill_path.display(),
        skill.body.trim()
    )
}

pub(super) fn format_tool_guidelines_body(tools: &[(String, String)]) -> String {
    if tools.is_empty() {
        return String::new();
    }

    let mut lines = vec![
        "Tool usage guidelines — prefer dedicated tools over shell equivalents.".to_string(),
        String::new(),
        "<tool_guidelines>".to_string(),
    ];
    for (name, guidelines) in tools {
        lines.push(format!("  <tool name=\"{}\">", escape_xml(name)));
        lines.push(format!("    {}", escape_xml(guidelines)));
        lines.push("  </tool>".to_string());
    }
    lines.push("</tool_guidelines>".to_string());
    lines.join("\n")
}

pub(super) fn render_tool_guidelines_generated_body(
    tool_state: &[PersistedToolSelectionEntry],
    tool_defs: &[ToolDefinition],
) -> String {
    let tools = tool_state
        .iter()
        .filter(|entry| entry.enabled)
        .filter_map(|entry| {
            tool_defs
                .iter()
                .find(|def| def.name == entry.tool_name)
                .and_then(|def| def.prompt_guidelines.as_deref())
                .map(|guidelines| (entry.tool_name.clone(), guidelines.to_string()))
        })
        .collect::<Vec<_>>();
    format_tool_guidelines_body(&tools)
}

pub(super) fn resolve_tool_guidelines_body(
    scope: PromptAssemblyScope,
    tool_state: &[PersistedToolSelectionEntry],
    tool_defs: &[ToolDefinition],
    global_state: &PromptAssemblyScopeState,
    project_state: &PromptAssemblyScopeState,
) -> String {
    let generated_body = render_tool_guidelines_generated_body(tool_state, tool_defs);
    let override_body = match scope {
        PromptAssemblyScope::Global => global_state.tool_guidelines_override(),
        PromptAssemblyScope::Project => project_state
            .tool_guidelines_override()
            .or(global_state.tool_guidelines_override()),
    };
    match override_body {
        Some(override_body) => rebuild_tool_guidelines_override(override_body, &generated_body),
        None => generated_body,
    }
}

pub(super) fn rebuild_tool_guidelines_override(existing: &str, generated_body: &str) -> String {
    if let Some((_, suffix)) = parse_tool_guidelines_override(existing) {
        let mut sections = vec![
            TOOL_GUIDELINES_GENERATED_START.to_string(),
            generated_body.to_string(),
            TOOL_GUIDELINES_GENERATED_END.to_string(),
        ];
        if !suffix.is_empty() {
            sections.push(suffix.to_string());
        }
        return sections.join("\n");
    }
    existing.to_string()
}

pub(super) fn parse_tool_guidelines_override(content: &str) -> Option<(&str, &str)> {
    let (_, after_start) = content.split_once(TOOL_GUIDELINES_GENERATED_START)?;
    let (_, after_end) = after_start.split_once(TOOL_GUIDELINES_GENERATED_END)?;
    Some((after_start, after_end.trim_start_matches('\n')))
}

pub(super) fn ensure_tool_guidelines_entry_exists(state: &mut PromptAssemblyScopeState) {
    if state
        .entries()
        .iter()
        .any(|entry| entry.kind == PromptSourceKind::ToolGuidelines)
    {
        return;
    }
    let requested_order = default_tool_guidelines_requested_order(state.entries());
    state.entries_mut().push(PersistedPromptAssemblyEntry {
        reference_id: "tool-guidelines".to_string(),
        kind: PromptSourceKind::ToolGuidelines,
        title: "Tool guidelines".to_string(),
        enabled: true,
        requested_order: Some(requested_order),
    });
}

pub(super) fn ensure_default_tool_guidelines_source(
    global_state: &mut PromptAssemblyScopeState,
    project_state: &mut PromptAssemblyScopeState,
) {
    if global_state
        .entries()
        .iter()
        .chain(project_state.entries().iter())
        .any(|entry| entry.kind == PromptSourceKind::ToolGuidelines)
    {
        return;
    }

    let target = if tool_guidelines_scope(global_state, project_state, PromptAssemblyScope::Global)
        == PromptAssemblyScope::Project
    {
        project_state
    } else {
        global_state
    };
    ensure_tool_guidelines_entry_exists(target);
}

pub(super) fn ensure_dynamic_environment_entry_exists(
    state: &mut PromptAssemblyScopeState,
    kind: PromptSourceKind,
) {
    let (reference_id, title, requested_order) = match kind {
        PromptSourceKind::DynamicEnvironmentBaseline => (
            "env-baseline",
            "Env baseline",
            DEFAULT_DYNAMIC_BASELINE_REQUESTED_ORDER,
        ),
        PromptSourceKind::DynamicEnvironmentChanges => (
            "env-changes",
            "Env changes",
            DEFAULT_DYNAMIC_CHANGES_REQUESTED_ORDER,
        ),
        _ => return,
    };
    if state
        .entries()
        .iter()
        .any(|entry| entry.kind == kind && entry.reference_id == reference_id)
    {
        return;
    }
    state.entries_mut().push(PersistedPromptAssemblyEntry {
        reference_id: reference_id.to_string(),
        kind,
        title: title.to_string(),
        enabled: true,
        requested_order: Some(requested_order),
    });
}

pub(super) fn ensure_default_dynamic_environment_sources(
    global_state: &mut PromptAssemblyScopeState,
    _project_state: &mut PromptAssemblyScopeState,
) {
    ensure_dynamic_environment_entry_exists(
        global_state,
        PromptSourceKind::DynamicEnvironmentBaseline,
    );
    ensure_dynamic_environment_entry_exists(
        global_state,
        PromptSourceKind::DynamicEnvironmentChanges,
    );
}

pub(super) fn default_tool_guidelines_requested_order(
    entries: &[PersistedPromptAssemblyEntry],
) -> u16 {
    entries
        .iter()
        .find(|entry| entry.kind == PromptSourceKind::ToolGuidelines)
        .and_then(|entry| entry.requested_order)
        .unwrap_or(DEFAULT_TOOL_GUIDELINES_REQUESTED_ORDER)
}

pub(super) fn tool_guideline_definitions(
    tool_defs: &[ToolDefinition],
) -> impl Iterator<Item = &ToolDefinition> {
    tool_defs
        .iter()
        .filter(|definition| definition.prompt_guidelines.is_some())
}

pub(super) fn tool_has_prompt_guidelines(tool_defs: &[ToolDefinition], tool_name: &str) -> bool {
    tool_guideline_definitions(tool_defs).any(|definition| definition.name == tool_name)
}

pub(super) fn merged_tool_selection_state(
    global_state: &PromptAssemblyScopeState,
    project_state: &PromptAssemblyScopeState,
    tool_defs: &[ToolDefinition],
) -> Vec<PersistedToolSelectionEntry> {
    let mut state_by_name = HashMap::<String, PersistedToolSelectionEntry>::new();
    for entry in global_state.tool_selections() {
        state_by_name.insert(entry.tool_name.clone(), entry.clone());
    }
    for entry in project_state.tool_selections() {
        state_by_name.insert(entry.tool_name.clone(), entry.clone());
    }

    let mut state = tool_guideline_definitions(tool_defs)
        .enumerate()
        .map(|(index, def)| {
            state_by_name
                .get(&def.name)
                .cloned()
                .unwrap_or(PersistedToolSelectionEntry {
                    tool_name: def.name.clone(),
                    enabled: true,
                    requested_order: Some(u16::try_from(index + 1).unwrap_or(u16::MAX)),
                })
        })
        .collect::<Vec<_>>();
    state.sort_by(|left, right| {
        left.requested_order
            .unwrap_or(u16::MAX)
            .cmp(&right.requested_order.unwrap_or(u16::MAX))
            .then_with(|| natural_sort_text_cmp(&left.tool_name, &right.tool_name))
    });
    state
}

pub(super) fn tool_candidate_inventory(
    tool_defs: &[ToolDefinition],
    tool_state: &[PersistedToolSelectionEntry],
    global_state: &PromptAssemblyScopeState,
    project_state: &PromptAssemblyScopeState,
) -> Vec<PromptAssemblyToolCandidate> {
    let merged_state_by_name = tool_state
        .iter()
        .map(|entry| (entry.tool_name.as_str(), entry))
        .collect::<HashMap<_, _>>();
    let selected_order_by_name = tool_state
        .iter()
        .enumerate()
        .map(|(index, entry)| (entry.tool_name.as_str(), index + 1))
        .collect::<HashMap<_, _>>();
    let mut inventory = tool_guideline_definitions(tool_defs)
        .map(|def| PromptAssemblyToolCandidate {
            name: def.name.clone(),
            label: def.label.clone(),
            description: def.description.clone(),
            prompt_guidelines: def.prompt_guidelines.clone(),
            origin: PromptSourceOrigin::Builtin,
            selection_scope: tool_guidelines_scope(
                global_state,
                project_state,
                PromptAssemblyScope::Global,
            ),
            can_select: true,
            selected: merged_state_by_name
                .get(def.name.as_str())
                .map(|entry| entry.enabled)
                .unwrap_or(true),
            selected_order: selected_order_by_name.get(def.name.as_str()).copied(),
        })
        .collect::<Vec<_>>();
    inventory.sort_by(|left, right| {
        left.selected_order
            .unwrap_or(usize::MAX)
            .cmp(&right.selected_order.unwrap_or(usize::MAX))
            .then_with(|| natural_sort_text_cmp(&left.name, &right.name))
    });
    inventory
}

pub(super) fn merged_dynamic_environment_selection_state(
    global_state: &PromptAssemblyScopeState,
    _project_state: &PromptAssemblyScopeState,
) -> Vec<DynamicEnvironmentSourceSelection> {
    let mut selections = default_dynamic_environment_selections();
    apply_dynamic_environment_selection_overrides(
        &mut selections,
        global_state.dynamic_environment_sources(),
    );
    selections.sort_by_key(|selection| (selection.snapshot_kind, selection.source_kind));
    selections
}

pub(super) fn apply_dynamic_environment_selection_overrides(
    selections: &mut [DynamicEnvironmentSourceSelection],
    overrides: &[DynamicEnvironmentSourceSelection],
) {
    for override_selection in overrides {
        if let Some(selection) = selections.iter_mut().find(|selection| {
            selection.snapshot_kind == override_selection.snapshot_kind
                && selection.source_kind == override_selection.source_kind
        }) {
            selection.enabled = override_selection.enabled;
        }
    }
}

pub(super) fn dynamic_environment_candidate_inventory(
    observations_by_source: &HashMap<DynamicEnvironmentSourceKind, DynamicEnvironmentObservation>,
    selection_state: &[DynamicEnvironmentSourceSelection],
    _global_state: &PromptAssemblyScopeState,
    _project_state: &PromptAssemblyScopeState,
) -> Vec<PromptAssemblyDynamicEnvironmentCandidate> {
    [
        DynamicEnvironmentSourceKind::GitReference,
        DynamicEnvironmentSourceKind::GitWorkingTree,
        DynamicEnvironmentSourceKind::Date,
        DynamicEnvironmentSourceKind::Workdir,
    ]
    .into_iter()
    .map(|source_kind| {
        let baseline_selected = dynamic_environment_selection_enabled(
            selection_state,
            DynamicEnvironmentSnapshotKind::Baseline,
            source_kind,
        );
        let changes_selected = dynamic_environment_selection_enabled(
            selection_state,
            DynamicEnvironmentSnapshotKind::Changes,
            source_kind,
        );
        PromptAssemblyDynamicEnvironmentCandidate {
            source_kind,
            label: source_kind.label().to_string(),
            origin: PromptSourceOrigin::Builtin,
            baseline_selected,
            changes_selected,
            baseline_preview_body: dynamic_environment_candidate_preview_body(
                observations_by_source,
                DynamicEnvironmentSnapshotKind::Baseline,
                source_kind,
            ),
            changes_preview_body: dynamic_environment_candidate_preview_body(
                observations_by_source,
                DynamicEnvironmentSnapshotKind::Changes,
                source_kind,
            ),
        }
    })
    .collect()
}

pub(super) fn dynamic_environment_selection_enabled(
    selection_state: &[DynamicEnvironmentSourceSelection],
    snapshot_kind: DynamicEnvironmentSnapshotKind,
    source_kind: DynamicEnvironmentSourceKind,
) -> bool {
    selection_state
        .iter()
        .find(|selection| {
            selection.snapshot_kind == snapshot_kind && selection.source_kind == source_kind
        })
        .is_some_and(|selection| selection.enabled)
}

pub(super) fn set_dynamic_environment_source_selected(
    state: &mut PromptAssemblyScopeState,
    snapshot_kind: DynamicEnvironmentSnapshotKind,
    source_kind: DynamicEnvironmentSourceKind,
    selected: bool,
) {
    if let Some(selection) = state
        .dynamic_environment_sources_mut()
        .iter_mut()
        .find(|selection| {
            selection.snapshot_kind == snapshot_kind && selection.source_kind == source_kind
        })
    {
        selection.enabled = selected;
        return;
    }

    state
        .dynamic_environment_sources_mut()
        .push(DynamicEnvironmentSourceSelection {
            snapshot_kind,
            source_kind,
            enabled: selected,
        });
}

pub(super) fn dynamic_environment_preview_body(
    observations_by_source: &HashMap<DynamicEnvironmentSourceKind, DynamicEnvironmentObservation>,
    snapshot_kind: DynamicEnvironmentSnapshotKind,
    selection_state: &[DynamicEnvironmentSourceSelection],
) -> Option<String> {
    let observations = dynamic_environment_observations_for_snapshot_kind(
        observations_by_source,
        selection_state,
        snapshot_kind,
    );
    build_dynamic_environment_snapshot(snapshot_kind, observations).map(|snapshot| snapshot.body)
}

pub(super) fn observe_dynamic_environment_inventory(
    work_dir: &Path,
) -> HashMap<DynamicEnvironmentSourceKind, DynamicEnvironmentObservation> {
    [
        DynamicEnvironmentSourceKind::GitReference,
        DynamicEnvironmentSourceKind::GitWorkingTree,
        DynamicEnvironmentSourceKind::Date,
        DynamicEnvironmentSourceKind::Workdir,
    ]
    .into_iter()
    .filter_map(|source_kind| {
        crate::dynamic_environment::observe_dynamic_environment_sources(work_dir, &[source_kind])
            .ok()
            .into_iter()
            .flatten()
            .next()
            .map(|observation| (source_kind, observation))
    })
    .collect()
}

pub(super) fn dynamic_environment_observations_for_snapshot_kind(
    observations_by_source: &HashMap<DynamicEnvironmentSourceKind, DynamicEnvironmentObservation>,
    selection_state: &[DynamicEnvironmentSourceSelection],
    snapshot_kind: DynamicEnvironmentSnapshotKind,
) -> Vec<DynamicEnvironmentObservation> {
    enabled_dynamic_environment_sources(selection_state, snapshot_kind)
        .into_iter()
        .filter_map(|source_kind| observations_by_source.get(&source_kind).cloned())
        .collect()
}

pub(super) fn dynamic_environment_candidate_preview_body(
    observations_by_source: &HashMap<DynamicEnvironmentSourceKind, DynamicEnvironmentObservation>,
    snapshot_kind: DynamicEnvironmentSnapshotKind,
    source_kind: DynamicEnvironmentSourceKind,
) -> String {
    observations_by_source
        .get(&source_kind)
        .cloned()
        .and_then(|observation| {
            build_dynamic_environment_snapshot(snapshot_kind, vec![observation])
        })
        .map(|snapshot| snapshot.body)
        .unwrap_or_default()
}

pub(super) fn set_tool_selected(
    state: &mut PromptAssemblyScopeState,
    tool_name: &str,
    selected: bool,
) {
    let next_order = next_tool_requested_order(state.tool_selections());
    if let Some(entry) = state
        .tool_selections_mut()
        .iter_mut()
        .find(|entry| entry.tool_name == tool_name)
    {
        entry.enabled = selected;
        if selected && entry.requested_order.is_none() {
            entry.requested_order = Some(next_order);
        }
        return;
    }

    state
        .tool_selections_mut()
        .push(PersistedToolSelectionEntry {
            tool_name: tool_name.to_string(),
            enabled: selected,
            requested_order: Some(next_order),
        });
}

pub(super) fn move_tool(
    state: &mut PromptAssemblyScopeState,
    tool_name: &str,
    direction: PromptAssemblyMoveDirection,
) -> Result<()> {
    let Some(position) = state
        .tool_selections()
        .iter()
        .position(|entry| entry.tool_name == tool_name)
    else {
        return Ok(());
    };
    let Some(neighbor) = (match direction {
        PromptAssemblyMoveDirection::Up => position.checked_sub(1),
        PromptAssemblyMoveDirection::Down => {
            (position + 1 < state.tool_selections().len()).then_some(position + 1)
        }
    }) else {
        return Ok(());
    };
    state.tool_selections_mut().swap(position, neighbor);
    normalize_tool_requested_orders(state);
    Ok(())
}

pub(super) fn next_tool_requested_order(entries: &[PersistedToolSelectionEntry]) -> u16 {
    entries
        .iter()
        .filter_map(|entry| entry.requested_order)
        .max()
        .unwrap_or(0)
        .saturating_add(1)
}

pub(super) fn normalize_tool_requested_orders(state: &mut PromptAssemblyScopeState) {
    for (index, entry) in state.tool_selections_mut().iter_mut().enumerate() {
        entry.requested_order = Some(u16::try_from(index + 1).unwrap_or(u16::MAX));
    }
}

pub(super) fn expand_custom_prompt_bindings(
    content: &str,
    bindings: &[TranscriptCustomPromptBinding],
    prompts_by_locator: &HashMap<(String, PromptSourceOrigin), PromptAssemblyExtraPromptCandidate>,
) -> Option<String> {
    let mut sorted_bindings = bindings.iter().collect::<Vec<_>>();
    sorted_bindings.sort_by_key(|binding| binding.start_char);

    let char_boundaries = content
        .char_indices()
        .map(|(index, _)| index)
        .chain(std::iter::once(content.len()))
        .collect::<Vec<_>>();

    let mut expanded = String::with_capacity(content.len());
    let mut cursor = 0;
    let mut replaced_any = false;

    for binding in sorted_bindings {
        let Some(prompt) = prompts_by_locator.get(&(binding.reference_id.clone(), binding.origin))
        else {
            continue;
        };
        let trimmed_body = prompt.body.trim();
        if trimmed_body.is_empty() {
            continue;
        }

        let Some(&start_byte) = char_boundaries.get(binding.start_char) else {
            continue;
        };
        let Some(&end_byte) = char_boundaries.get(binding.end_char) else {
            continue;
        };
        if start_byte < cursor || end_byte < start_byte {
            continue;
        }

        expanded.push_str(&content[cursor..start_byte]);

        trim_trailing_inline_whitespace(&mut expanded);
        ensure_blank_line_before_inline_prompt(&mut expanded);
        expanded.push_str(trimmed_body);

        let skipped_after_bytes = count_leading_inline_whitespace(&content[end_byte..]);
        let trailing_text = &content[end_byte + skipped_after_bytes..];
        ensure_blank_line_after_inline_prompt(&mut expanded, trailing_text);

        cursor = end_byte + skipped_after_bytes;
        replaced_any = true;
    }

    if !replaced_any {
        return None;
    }

    expanded.push_str(&content[cursor..]);
    Some(expanded)
}

pub(super) fn trim_trailing_inline_whitespace(output: &mut String) {
    while matches!(output.chars().last(), Some(' ' | '\t')) {
        output.pop();
    }
}

pub(super) fn ensure_blank_line_before_inline_prompt(output: &mut String) {
    if output.is_empty() {
        return;
    }
    match trailing_newline_count(output) {
        0 => output.push_str("\n\n"),
        1 => output.push('\n'),
        _ => {}
    }
}

pub(super) fn ensure_blank_line_after_inline_prompt(output: &mut String, trailing_text: &str) {
    if trailing_text.is_empty() {
        return;
    }
    match leading_newline_count(trailing_text) {
        0 => output.push_str("\n\n"),
        1 => output.push('\n'),
        _ => {}
    }
}

pub(super) fn trailing_newline_count(value: &str) -> usize {
    value.chars().rev().take_while(|ch| *ch == '\n').count()
}

pub(super) fn leading_newline_count(value: &str) -> usize {
    value.chars().take_while(|ch| *ch == '\n').count()
}

pub(super) fn count_leading_inline_whitespace(value: &str) -> usize {
    value
        .char_indices()
        .take_while(|(_, ch)| matches!(ch, ' ' | '\t'))
        .map(|(index, ch)| index + ch.len_utf8())
        .last()
        .unwrap_or(0)
}

pub(super) fn escape_xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

pub(super) fn project_skill_search_dirs(work_dir: &Path) -> Vec<PathBuf> {
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

pub(super) fn git_root(start: &Path) -> Option<PathBuf> {
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

pub(super) fn global_skill_root() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join(".agents").join("skills"))
}

pub(super) fn scope_reference_key(scope: PromptAssemblyScope, reference_id: &str) -> String {
    format!("{}::{reference_id}", scope.as_stored_value())
}
