use std::{
    collections::{BTreeMap, BTreeSet},
    fs, io,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use super::super::{PromptSourceKind, derive_extra_prompt_title};
use super::reference_id::{
    InvalidProjectReferenceId, PROJECT_CORE_SYSTEM_OVERRIDE_FILE_NAME,
    PROJECT_CUSTOM_PROMPTS_DIR_NAME, PROJECT_PROMPT_ASSEMBLY_FILE_NAME, PROJECT_PROMPTS_DIR_NAME,
    PROJECT_SKILL_DISCOVERY_OVERRIDE_FILE_NAME, PROJECT_TOOL_GUIDELINES_OVERRIDE_FILE_NAME,
    project_extra_prompt_file_name, project_extra_prompt_reference_id_from_file_name,
};
use super::scope::PromptAssemblyScope;
use super::sort::{
    sorted_prompt_assembly_entries, sorted_prompt_assembly_entry_refs,
    sorted_skill_discovery_skill_entries, sorted_skill_discovery_skill_entry_refs,
    sorted_tool_selection_entries, sorted_tool_selection_entry_refs,
};
use super::state::{
    PersistedPromptAssemblyEntry, PersistedSkillDiscoverySkillEntry, PersistedToolSelectionEntry,
    PromptAssemblyScopeState, StoredPromptBody,
};

const PROJECT_PROMPT_ASSEMBLY_VERSION: u32 = 1;

/// `ProjectPromptAssemblyError` 描述项目级 prompt assembly TOML / prompt 文件的读写失败。
#[derive(Debug, thiserror::Error)]
pub enum ProjectPromptAssemblyError {
    #[error("read project prompt assembly file {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("write project prompt assembly file {path}: {source}")]
    Write {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("decode project prompt assembly file {path}: {source}")]
    Decode {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },
    #[error("encode project prompt assembly file {path}: {source}")]
    Encode {
        path: PathBuf,
        #[source]
        source: toml::ser::Error,
    },
    #[error("project prompt assembly file {path} uses unsupported version {version}")]
    InvalidVersion { path: PathBuf, version: u32 },
    #[error("{source}")]
    InvalidProjectReferenceId {
        #[from]
        source: InvalidProjectReferenceId,
    },
    #[error("project prompt assembly persistence only accepts project scope, got {}", scope.as_stored_value())]
    UnexpectedScope { scope: PromptAssemblyScope },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProjectPromptAssemblyFile {
    version: u32,
    #[serde(default)]
    entries: Vec<PersistedPromptAssemblyEntry>,
    #[serde(default)]
    skill_discovery_skills: Vec<PersistedSkillDiscoverySkillEntry>,
    #[serde(default)]
    tool_selections: Vec<PersistedToolSelectionEntry>,
}

#[derive(Debug, Serialize)]
struct ProjectPromptAssemblyFileForWrite<'a> {
    version: u32,
    entries: Vec<&'a PersistedPromptAssemblyEntry>,
    skill_discovery_skills: Vec<&'a PersistedSkillDiscoverySkillEntry>,
    tool_selections: Vec<&'a PersistedToolSelectionEntry>,
}

/// `project_prompt_assembly_path` 返回项目级 prompt assembly TOML 路径。
#[must_use]
pub fn project_prompt_assembly_path(work_dir: &Path) -> PathBuf {
    work_dir
        .join(".hunea")
        .join(PROJECT_PROMPT_ASSEMBLY_FILE_NAME)
}

/// `project_prompts_dir` 返回项目级 prompt body 目录。
#[must_use]
pub fn project_prompts_dir(work_dir: &Path) -> PathBuf {
    work_dir.join(".hunea").join(PROJECT_PROMPTS_DIR_NAME)
}

/// `project_custom_prompts_dir` 返回项目级 custom prompt body 目录。
#[must_use]
pub fn project_custom_prompts_dir(work_dir: &Path) -> PathBuf {
    project_prompts_dir(work_dir).join(PROJECT_CUSTOM_PROMPTS_DIR_NAME)
}

/// `load_project_prompt_assembly_state` 读取项目级 prompt assembly TOML 与 prompt bodies。
pub fn load_project_prompt_assembly_state(
    work_dir: &Path,
) -> Result<PromptAssemblyScopeState, ProjectPromptAssemblyError> {
    let config_path = project_prompt_assembly_path(work_dir);
    let config = read_project_prompt_assembly_file(&config_path)?;
    let prompts_dir = project_prompts_dir(work_dir);
    let custom_prompts_dir = project_custom_prompts_dir(work_dir);

    let entries = sorted_prompt_assembly_entries(config.entries);
    let skill_discovery_skills =
        sorted_skill_discovery_skill_entries(config.skill_discovery_skills);
    let core_system_override =
        read_optional_text_file(&prompts_dir.join(PROJECT_CORE_SYSTEM_OVERRIDE_FILE_NAME))?;
    let skill_discovery_override =
        read_optional_text_file(&prompts_dir.join(PROJECT_SKILL_DISCOVERY_OVERRIDE_FILE_NAME))?;
    let tool_guidelines_override =
        read_optional_text_file(&prompts_dir.join(PROJECT_TOOL_GUIDELINES_OVERRIDE_FILE_NAME))?;
    let tool_selections = sorted_tool_selection_entries(config.tool_selections);
    let entry_titles = entries
        .iter()
        .filter(|entry| entry.kind == PromptSourceKind::ExtraPrompt)
        .map(|entry| (entry.reference_id.as_str(), entry.title.as_str()))
        .collect::<BTreeMap<_, _>>();
    let extra_prompts = load_project_extra_prompt_bodies(&custom_prompts_dir, &entry_titles)?;

    let mut state = PromptAssemblyScopeState::new(PromptAssemblyScope::Project);
    state.set_core_system_override(core_system_override);
    state.set_skill_discovery_override(skill_discovery_override);
    state.set_tool_guidelines_override(tool_guidelines_override);
    state.set_entries(entries);
    state.set_skill_discovery_skills(skill_discovery_skills);
    state.set_tool_selections(tool_selections);
    state.set_extra_prompts(extra_prompts);
    Ok(state)
}

/// `save_project_prompt_assembly_state` 把项目级 prompt assembly 状态写入 `.hunea/`。
pub fn save_project_prompt_assembly_state(
    work_dir: &Path,
    state: &PromptAssemblyScopeState,
) -> Result<(), ProjectPromptAssemblyError> {
    if state.scope() != PromptAssemblyScope::Project {
        return Err(ProjectPromptAssemblyError::UnexpectedScope {
            scope: state.scope(),
        });
    }

    let config_path = project_prompt_assembly_path(work_dir);
    let prompts_dir = project_prompts_dir(work_dir);
    let custom_prompts_dir = project_custom_prompts_dir(work_dir);
    fs::create_dir_all(&prompts_dir).map_err(|source| ProjectPromptAssemblyError::Write {
        path: prompts_dir.clone(),
        source,
    })?;
    fs::create_dir_all(&custom_prompts_dir).map_err(|source| {
        ProjectPromptAssemblyError::Write {
            path: custom_prompts_dir.clone(),
            source,
        }
    })?;

    let mut desired_custom_prompt_files = BTreeSet::new();
    for prompt in state.extra_prompts() {
        let file_name = project_extra_prompt_file_name(&prompt.reference_id)?;
        let path = custom_prompts_dir.join(&file_name);
        write_text_file(&path, &prompt.body)?;
        desired_custom_prompt_files.insert(file_name);
    }

    let core_override_path = prompts_dir.join(PROJECT_CORE_SYSTEM_OVERRIDE_FILE_NAME);
    match state.core_system_override() {
        Some(body) => {
            write_text_file(&core_override_path, body)?;
        }
        None => remove_file_if_exists(&core_override_path)?,
    }

    let skill_discovery_override_path =
        prompts_dir.join(PROJECT_SKILL_DISCOVERY_OVERRIDE_FILE_NAME);
    match state.skill_discovery_override() {
        Some(body) => {
            write_text_file(&skill_discovery_override_path, body)?;
        }
        None => remove_file_if_exists(&skill_discovery_override_path)?,
    }

    let tool_guidelines_override_path =
        prompts_dir.join(PROJECT_TOOL_GUIDELINES_OVERRIDE_FILE_NAME);
    match state.tool_guidelines_override() {
        Some(body) => {
            write_text_file(&tool_guidelines_override_path, body)?;
        }
        None => remove_file_if_exists(&tool_guidelines_override_path)?,
    }

    prune_stale_prompt_files(&custom_prompts_dir, &desired_custom_prompt_files)?;

    let config = ProjectPromptAssemblyFileForWrite {
        version: PROJECT_PROMPT_ASSEMBLY_VERSION,
        entries: sorted_prompt_assembly_entry_refs(state.entries()),
        skill_discovery_skills: sorted_skill_discovery_skill_entry_refs(
            state.skill_discovery_skills(),
        ),
        tool_selections: sorted_tool_selection_entry_refs(state.tool_selections()),
    };
    let encoded =
        toml::to_string_pretty(&config).map_err(|source| ProjectPromptAssemblyError::Encode {
            path: config_path.clone(),
            source,
        })?;
    write_text_file(&config_path, &encoded)?;

    Ok(())
}

fn read_project_prompt_assembly_file(
    path: &Path,
) -> Result<ProjectPromptAssemblyFile, ProjectPromptAssemblyError> {
    let content = match fs::read_to_string(path) {
        Ok(content) => content,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(ProjectPromptAssemblyFile {
                version: PROJECT_PROMPT_ASSEMBLY_VERSION,
                entries: Vec::new(),
                skill_discovery_skills: Vec::new(),
                tool_selections: Vec::new(),
            });
        }
        Err(source) => {
            return Err(ProjectPromptAssemblyError::Read {
                path: path.to_path_buf(),
                source,
            });
        }
    };

    let file = toml::from_str::<ProjectPromptAssemblyFile>(&content).map_err(|source| {
        ProjectPromptAssemblyError::Decode {
            path: path.to_path_buf(),
            source,
        }
    })?;
    if file.version != PROJECT_PROMPT_ASSEMBLY_VERSION {
        return Err(ProjectPromptAssemblyError::InvalidVersion {
            path: path.to_path_buf(),
            version: file.version,
        });
    }

    Ok(file)
}

fn write_text_file(path: &Path, content: &str) -> Result<(), ProjectPromptAssemblyError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|source| ProjectPromptAssemblyError::Write {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    fs::write(path, content).map_err(|source| ProjectPromptAssemblyError::Write {
        path: path.to_path_buf(),
        source,
    })
}

fn read_optional_text_file(path: &Path) -> Result<Option<String>, ProjectPromptAssemblyError> {
    match fs::read_to_string(path) {
        Ok(content) => Ok(Some(content)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(ProjectPromptAssemblyError::Read {
            path: path.to_path_buf(),
            source,
        }),
    }
}

fn load_project_extra_prompt_bodies(
    custom_prompts_dir: &Path,
    entry_titles: &BTreeMap<&str, &str>,
) -> Result<Vec<StoredPromptBody>, ProjectPromptAssemblyError> {
    let mut prompts_by_reference = BTreeMap::new();
    collect_project_extra_prompt_bodies(
        custom_prompts_dir,
        entry_titles,
        &mut prompts_by_reference,
    )?;
    Ok(prompts_by_reference.into_values().collect())
}

fn collect_project_extra_prompt_bodies(
    dir: &Path,
    entry_titles: &BTreeMap<&str, &str>,
    prompts_by_reference: &mut BTreeMap<String, StoredPromptBody>,
) -> Result<(), ProjectPromptAssemblyError> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(source) => {
            return Err(ProjectPromptAssemblyError::Read {
                path: dir.to_path_buf(),
                source,
            });
        }
    };

    for entry in entries {
        let entry = entry.map_err(|source| ProjectPromptAssemblyError::Read {
            path: dir.to_path_buf(),
            source,
        })?;
        let path = entry.path();
        let file_type = entry
            .file_type()
            .map_err(|source| ProjectPromptAssemblyError::Read {
                path: path.clone(),
                source,
            })?;
        if !file_type.is_file() {
            continue;
        }
        let file_name = entry.file_name();
        let file_name = file_name.to_string_lossy();
        let Some(reference_id) = project_extra_prompt_reference_id_from_file_name(&file_name)
        else {
            continue;
        };
        if prompts_by_reference.contains_key(reference_id) {
            continue;
        }

        let body =
            fs::read_to_string(&path).map_err(|source| ProjectPromptAssemblyError::Read {
                path: path.clone(),
                source,
            })?;
        let title = entry_titles
            .get(reference_id)
            .copied()
            .map(str::to_string)
            .unwrap_or_else(|| derive_extra_prompt_title(&body, reference_id));
        prompts_by_reference.insert(
            reference_id.to_string(),
            StoredPromptBody {
                reference_id: reference_id.to_string(),
                title,
                body,
            },
        );
    }

    Ok(())
}

fn remove_file_if_exists(path: &Path) -> Result<(), ProjectPromptAssemblyError> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(ProjectPromptAssemblyError::Write {
            path: path.to_path_buf(),
            source,
        }),
    }
}

fn prune_stale_prompt_files(
    custom_prompts_dir: &Path,
    desired_files: &BTreeSet<String>,
) -> Result<(), ProjectPromptAssemblyError> {
    let entries = match fs::read_dir(custom_prompts_dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(source) => {
            return Err(ProjectPromptAssemblyError::Read {
                path: custom_prompts_dir.to_path_buf(),
                source,
            });
        }
    };

    for entry in entries {
        let entry = entry.map_err(|source| ProjectPromptAssemblyError::Read {
            path: custom_prompts_dir.to_path_buf(),
            source,
        })?;
        let file_type = entry
            .file_type()
            .map_err(|source| ProjectPromptAssemblyError::Read {
                path: entry.path(),
                source,
            })?;
        if !file_type.is_file() {
            continue;
        }

        let file_name = entry.file_name();
        let file_name = file_name.to_string_lossy();
        if !desired_files.contains(file_name.as_ref()) {
            remove_file_if_exists(&entry.path())?;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_test_dir(prefix: &str) -> PathBuf {
        let unique = format!(
            "{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system time should be after unix epoch")
                .as_nanos()
        );
        let path = std::env::temp_dir().join(format!("hunea-prompt-assembly-{prefix}-{unique}"));
        fs::create_dir_all(&path).expect("temp test dir should exist");
        path
    }

    fn sample_project_state() -> PromptAssemblyScopeState {
        let mut state = PromptAssemblyScopeState::new(PromptAssemblyScope::Project);
        state.set_core_system_override(Some("project core override".to_string()));
        state.set_entries(vec![
            PersistedPromptAssemblyEntry {
                reference_id: "skill-discovery".to_string(),
                kind: PromptSourceKind::SkillDiscovery,
                title: "Skill discovery source".to_string(),
                enabled: true,
                requested_order: Some(10),
            },
            PersistedPromptAssemblyEntry {
                reference_id: "repo-review-rules".to_string(),
                kind: PromptSourceKind::ExtraPrompt,
                title: "repo-review-rules".to_string(),
                enabled: true,
                requested_order: Some(20),
            },
            PersistedPromptAssemblyEntry {
                reference_id: "code-review".to_string(),
                kind: PromptSourceKind::LongLivedSkill,
                title: "code-review".to_string(),
                enabled: false,
                requested_order: Some(30),
            },
        ]);
        state.set_extra_prompts(vec![StoredPromptBody {
            reference_id: "repo-review-rules".to_string(),
            title: "repo-review-rules".to_string(),
            body: "always check tests".to_string(),
        }]);
        state
    }

    #[test]
    fn project_scope_roundtrip_persists_entries_bodies_and_core_override() {
        let work_dir = temp_test_dir("project-roundtrip");
        let state = sample_project_state();

        save_project_prompt_assembly_state(&work_dir, &state)
            .expect("project prompt assembly should save");
        let loaded = load_project_prompt_assembly_state(&work_dir)
            .expect("project prompt assembly should load");

        assert_eq!(loaded, state);
    }

    #[test]
    fn project_scope_save_prunes_deleted_extra_prompt_files_and_core_override() {
        let work_dir = temp_test_dir("project-prune");
        save_project_prompt_assembly_state(&work_dir, &sample_project_state())
            .expect("initial project prompt assembly should save");

        let mut cleared_state = PromptAssemblyScopeState::new(PromptAssemblyScope::Project);
        cleared_state.set_entries(vec![PersistedPromptAssemblyEntry {
            reference_id: "skill-discovery".to_string(),
            kind: PromptSourceKind::SkillDiscovery,
            title: "Skill discovery source".to_string(),
            enabled: true,
            requested_order: Some(10),
        }]);

        save_project_prompt_assembly_state(&work_dir, &cleared_state)
            .expect("updated project prompt assembly should save");

        let prompts_dir = project_prompts_dir(&work_dir);
        let custom_prompts_dir = project_custom_prompts_dir(&work_dir);
        assert!(
            !custom_prompts_dir.join("repo-review-rules.md").exists(),
            "deleted extra prompt body should be pruned"
        );
        assert!(
            !prompts_dir
                .join(PROJECT_CORE_SYSTEM_OVERRIDE_FILE_NAME)
                .exists(),
            "restored core system override should delete reserved file"
        );
    }

    #[test]
    fn project_scope_can_preserve_missing_extra_prompt_reference_without_body() {
        let work_dir = temp_test_dir("project-missing-body");
        let mut state = PromptAssemblyScopeState::new(PromptAssemblyScope::Project);
        state.set_entries(vec![PersistedPromptAssemblyEntry {
            reference_id: "missing-prompt".to_string(),
            kind: PromptSourceKind::ExtraPrompt,
            title: "missing-prompt".to_string(),
            enabled: true,
            requested_order: Some(10),
        }]);

        save_project_prompt_assembly_state(&work_dir, &state)
            .expect("project prompt assembly with missing body should save");
        let loaded = load_project_prompt_assembly_state(&work_dir)
            .expect("project prompt assembly should load");

        assert_eq!(loaded.entries(), state.entries());
        assert!(loaded.extra_prompts().is_empty());
    }

    #[test]
    fn project_scope_roundtrip_persists_unselected_extra_prompt_body_in_custom_dir() {
        let work_dir = temp_test_dir("project-unselected-extra-prompt");
        let mut state = PromptAssemblyScopeState::new(PromptAssemblyScope::Project);
        state.set_entries(vec![PersistedPromptAssemblyEntry {
            reference_id: "skill-discovery".to_string(),
            kind: PromptSourceKind::SkillDiscovery,
            title: "Skill discovery source".to_string(),
            enabled: true,
            requested_order: Some(10),
        }]);
        state.set_extra_prompts(vec![StoredPromptBody {
            reference_id: "review-rules".to_string(),
            title: "Review rules".to_string(),
            body: "# Review rules\nAlways verify tests.\n".to_string(),
        }]);

        save_project_prompt_assembly_state(&work_dir, &state)
            .expect("project prompt assembly should save");
        let loaded = load_project_prompt_assembly_state(&work_dir)
            .expect("project prompt assembly should load");

        assert_eq!(loaded.extra_prompts(), state.extra_prompts());
        assert!(
            !project_prompts_dir(&work_dir)
                .join("review-rules.md")
                .exists(),
            "extra prompt body should not live at the prompts root"
        );
        assert!(
            project_custom_prompts_dir(&work_dir)
                .join("review-rules.md")
                .exists(),
            "extra prompt body should be stored under prompts/custom"
        );
    }

    #[test]
    fn project_scope_rejects_reserved_extra_prompt_reference_ids() {
        for reference_id in [
            "__core-system__",
            "__skill-discovery__",
            "__tool-guidelines__",
        ] {
            let work_dir = temp_test_dir(reference_id);
            let mut state = sample_project_state();
            state.set_extra_prompts(vec![StoredPromptBody {
                reference_id: reference_id.to_string(),
                title: reference_id.to_string(),
                body: "reserved body must not be saved as a custom prompt".to_string(),
            }]);

            let error = save_project_prompt_assembly_state(&work_dir, &state)
                .expect_err("reserved custom prompt reference id should be rejected");

            assert!(matches!(
                error,
                ProjectPromptAssemblyError::InvalidProjectReferenceId {
                    source
                } if source.reference_id() == reference_id
            ));
        }
    }

    #[test]
    fn project_scope_ignores_root_extra_prompt_files() {
        let work_dir = temp_test_dir("project-legacy-extra-prompt");
        let prompts_dir = project_prompts_dir(&work_dir);
        fs::create_dir_all(&prompts_dir).expect("legacy prompts dir should exist");
        fs::write(
            prompts_dir.join("legacy-review.md"),
            "# Legacy review\nPrefer root-cause fixes.\n",
        )
        .expect("legacy extra prompt file should write");

        let loaded = load_project_prompt_assembly_state(&work_dir)
            .expect("project prompt assembly should ignore legacy root files");

        assert!(
            loaded.extra_prompts().is_empty(),
            "root-level prompt files should no longer be loaded"
        );
    }
}
