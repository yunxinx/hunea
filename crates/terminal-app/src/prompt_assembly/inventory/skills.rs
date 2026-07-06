use super::*;

pub(in crate::prompt_assembly) fn discovered_skill_inventory(
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
            skill_path: skill.skill_path.clone(),
            body: format_long_lived_skill_body(skill),
            selection: PromptAssemblySelectionState::from_parts(
                skill.can_select_for_discovery(),
                prompt_overlay_skill_state_for_origin(
                    skill.origin,
                    skill.name.as_str(),
                    &global_state_by_name,
                    &project_state_by_name,
                )
                .or_else(|| merged_state_by_name.get(skill.name.as_str()).copied())
                .map(|entry| entry.enabled)
                .unwrap_or(skill.can_select_for_discovery()),
                selected_order_by_name.get(skill.name.as_str()).copied(),
            ),
        })
        .collect::<Vec<_>>();
    let (mut discovery_eligible, mut manual_only): (Vec<_>, Vec<_>) = inventory
        .into_iter()
        .partition(|skill| skill.selection.can_select());
    discovery_eligible.sort_by(|left, right| {
        left.selection
            .selected_order()
            .unwrap_or(usize::MAX)
            .cmp(&right.selection.selected_order().unwrap_or(usize::MAX))
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

pub(in crate::prompt_assembly) fn prompt_overlay_skill_state_for_origin<'a>(
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

pub(in crate::prompt_assembly) fn prompt_source_origin_sort_key(origin: PromptSourceOrigin) -> u8 {
    match origin {
        PromptSourceOrigin::Project => 0,
        PromptSourceOrigin::Global => 1,
        PromptSourceOrigin::Builtin => 2,
    }
}

pub(in crate::prompt_assembly) fn manual_skill_inventory(
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
            skill_path: skill.skill_path.clone(),
            body: format_long_lived_skill_body(skill),
            selection: PromptAssemblySelectionState::from_parts(
                skill.can_select_for_discovery(),
                false,
                None,
            ),
        })
        .collect()
}

pub(in crate::prompt_assembly) fn discover_skills(
    work_dir: &Path,
    global_skill_root_override: Option<&Path>,
) -> Vec<DiscoveredSkill> {
    discover_skills_with_diagnostics(work_dir, global_skill_root_override).0
}

pub(in crate::prompt_assembly) fn discover_skills_with_diagnostics(
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

pub(in crate::prompt_assembly) fn discover_skills_from_root(
    root: &Path,
    origin: PromptSourceOrigin,
    discovered: &mut Vec<DiscoveredSkill>,
    seen_names: &mut HashMap<(String, PromptSourceOrigin), usize>,
    diagnostics: &mut Vec<PromptAssemblyDiagnostic>,
) {
    if !root.is_dir() {
        return;
    }

    for path in sorted_skill_child_dirs(
        root,
        origin,
        diagnostics,
        "read skill directory",
        "read skill directory entry",
    ) {
        discover_skill_dir(&path, origin, discovered, seen_names, diagnostics);
    }
}

pub(in crate::prompt_assembly) fn discover_skill_dir(
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

    for path in sorted_skill_child_dirs(
        dir,
        origin,
        diagnostics,
        "read nested skill directory",
        "read nested skill directory entry",
    ) {
        discover_skill_dir(&path, origin, discovered, seen_names, diagnostics);
    }
}

fn sorted_skill_child_dirs(
    dir: &Path,
    origin: PromptSourceOrigin,
    diagnostics: &mut Vec<PromptAssemblyDiagnostic>,
    read_error_context: &str,
    entry_error_context: &str,
) -> Vec<PathBuf> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) => {
            diagnostics.push(PromptAssemblyDiagnostic {
                origin: Some(origin),
                path: Some(dir.to_path_buf()),
                message: format!("{read_error_context}: {error}"),
            });
            return Vec::new();
        }
    };

    let mut child_dirs = Vec::new();
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                diagnostics.push(PromptAssemblyDiagnostic {
                    origin: Some(origin),
                    path: Some(dir.to_path_buf()),
                    message: format!("{entry_error_context}: {error}"),
                });
                continue;
            }
        };
        let path = entry.path();
        if path.is_dir() {
            child_dirs.push(path);
        }
    }
    child_dirs.sort_by(|left, right| {
        natural_sort_text_cmp(&skill_dir_sort_key(left), &skill_dir_sort_key(right))
            .then_with(|| left.cmp(right))
    });
    child_dirs
}

fn skill_dir_sort_key(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path_component_key(path))
}

pub(in crate::prompt_assembly) fn parse_skill_file(
    skill_path: &Path,
    origin: PromptSourceOrigin,
) -> Result<DiscoveredSkill, SkillParseError> {
    let content = fs::read_to_string(skill_path).map_err(|source| SkillParseError::Read {
        path: skill_path.to_path_buf(),
        source,
    })?;
    let (frontmatter, body) =
        split_frontmatter(&content).ok_or(SkillParseError::MissingFrontmatter)?;
    let frontmatter: SkillFrontmatter = yaml_serde::from_str(frontmatter.as_str())
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
pub(in crate::prompt_assembly) enum SkillParseError {
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
        source: yaml_serde::Error,
    },
    #[error("missing required name")]
    MissingName,
    #[error("missing required description")]
    MissingDescription,
}

pub(in crate::prompt_assembly) fn split_frontmatter(content: &str) -> Option<(String, String)> {
    let normalized = content.replace("\r\n", "\n").replace('\r', "\n");
    let trimmed = normalized.strip_prefix("---\n")?;
    let (frontmatter, body) = trimmed.split_once("\n---\n")?;
    Some((frontmatter.to_string(), body.to_string()))
}

pub(in crate::prompt_assembly) fn format_skill_discovery_body(
    skills: Vec<DiscoveredSkill>,
) -> String {
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

pub(in crate::prompt_assembly) fn format_long_lived_skill_body(skill: &DiscoveredSkill) -> String {
    format!(
        "<skill>\n<name>{}</name>\n<path>{}</path>\n{}\n</skill>",
        skill.name,
        skill.skill_path.display(),
        skill.body.trim()
    )
}

pub(in crate::prompt_assembly) fn project_skill_search_dirs(work_dir: &Path) -> Vec<PathBuf> {
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

pub(in crate::prompt_assembly) fn global_skill_root() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join(".agents").join("skills"))
}
