use std::{
    collections::{BTreeMap, BTreeSet},
    fmt, fs, io,
    path::{Component, Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use super::PromptSourceKind;

pub const PROJECT_PROMPT_ASSEMBLY_FILE_NAME: &str = "prompt-assembly.toml";
pub const PROJECT_PROMPTS_DIR_NAME: &str = "prompts";
pub const PROJECT_CORE_SYSTEM_OVERRIDE_FILE_NAME: &str = "__core-system__.md";
const PROJECT_PROMPT_ASSEMBLY_VERSION: u32 = 1;

/// `PromptAssemblyScope` 表示 prompt assembly 配置的生效范围。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptAssemblyScope {
    Global,
    Project,
}

impl PromptAssemblyScope {
    /// `as_stored_value` 返回稳定的持久化值。
    pub const fn as_stored_value(self) -> &'static str {
        match self {
            Self::Global => "global",
            Self::Project => "project",
        }
    }

    /// `from_stored_value` 从稳定持久化值恢复 scope。
    #[must_use]
    pub fn from_stored_value(value: &str) -> Option<Self> {
        match value {
            "global" => Some(Self::Global),
            "project" => Some(Self::Project),
            _ => None,
        }
    }
}

/// `PersistedPromptAssemblyEntry` 表示一个可排序、可启停的 prompt source 引用。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersistedPromptAssemblyEntry {
    pub reference_id: String,
    pub kind: PromptSourceKind,
    pub title: String,
    pub enabled: bool,
    pub requested_order: Option<u16>,
}

/// `StoredPromptBody` 表示持久化的 prompt 文本实体。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredPromptBody {
    pub reference_id: String,
    pub title: String,
    pub body: String,
}

/// `PromptAssemblyScopeState` 表示单个 scope 下完整的 prompt assembly 持久化状态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptAssemblyScopeState {
    pub scope: PromptAssemblyScope,
    pub core_system_override: Option<String>,
    pub entries: Vec<PersistedPromptAssemblyEntry>,
    pub extra_prompts: Vec<StoredPromptBody>,
}

impl PromptAssemblyScopeState {
    /// `empty` 构造一个空 scope 状态。
    #[must_use]
    pub fn empty(scope: PromptAssemblyScope) -> Self {
        Self {
            scope,
            core_system_override: None,
            entries: Vec::new(),
            extra_prompts: Vec::new(),
        }
    }
}

/// `ProjectPromptAssemblyError` 描述项目级 prompt assembly TOML / prompt 文件的读写失败。
#[derive(Debug)]
pub enum ProjectPromptAssemblyError {
    Read {
        path: PathBuf,
        source: io::Error,
    },
    Write {
        path: PathBuf,
        source: io::Error,
    },
    Decode {
        path: PathBuf,
        source: toml::de::Error,
    },
    Encode {
        path: PathBuf,
        source: toml::ser::Error,
    },
    InvalidVersion {
        path: PathBuf,
        version: u32,
    },
    InvalidProjectReferenceId {
        reference_id: String,
    },
    UnexpectedScope {
        scope: PromptAssemblyScope,
    },
    OrphanExtraPromptBody {
        reference_id: String,
    },
}

impl fmt::Display for ProjectPromptAssemblyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read { path, source } => {
                write!(
                    f,
                    "read project prompt assembly file {}: {source}",
                    path.display()
                )
            }
            Self::Write { path, source } => {
                write!(
                    f,
                    "write project prompt assembly file {}: {source}",
                    path.display()
                )
            }
            Self::Decode { path, source } => {
                write!(
                    f,
                    "decode project prompt assembly file {}: {source}",
                    path.display()
                )
            }
            Self::Encode { path, source } => {
                write!(
                    f,
                    "encode project prompt assembly file {}: {source}",
                    path.display()
                )
            }
            Self::InvalidVersion { path, version } => write!(
                f,
                "project prompt assembly file {} uses unsupported version {version}",
                path.display()
            ),
            Self::InvalidProjectReferenceId { reference_id } => write!(
                f,
                "project prompt reference id must be a single safe file component: {reference_id}"
            ),
            Self::UnexpectedScope { scope } => write!(
                f,
                "project prompt assembly persistence only accepts project scope, got {}",
                scope.as_stored_value()
            ),
            Self::OrphanExtraPromptBody { reference_id } => write!(
                f,
                "extra prompt body `{reference_id}` has no matching extra prompt entry"
            ),
        }
    }
}

impl std::error::Error for ProjectPromptAssemblyError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Read { source, .. } | Self::Write { source, .. } => Some(source),
            Self::Decode { source, .. } => Some(source),
            Self::Encode { source, .. } => Some(source),
            Self::InvalidVersion { .. }
            | Self::InvalidProjectReferenceId { .. }
            | Self::UnexpectedScope { .. }
            | Self::OrphanExtraPromptBody { .. } => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProjectPromptAssemblyFile {
    version: u32,
    #[serde(default)]
    entries: Vec<PersistedPromptAssemblyEntry>,
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

/// `load_project_prompt_assembly_state` 读取项目级 prompt assembly TOML 与 prompt bodies。
pub fn load_project_prompt_assembly_state(
    work_dir: &Path,
) -> Result<PromptAssemblyScopeState, ProjectPromptAssemblyError> {
    let config_path = project_prompt_assembly_path(work_dir);
    let config = read_project_prompt_assembly_file(&config_path)?;
    let prompts_dir = project_prompts_dir(work_dir);

    let mut state = PromptAssemblyScopeState::empty(PromptAssemblyScope::Project);
    state.entries = sort_entries(config.entries);
    state.core_system_override =
        read_optional_text_file(&prompts_dir.join(PROJECT_CORE_SYSTEM_OVERRIDE_FILE_NAME))?;
    state.extra_prompts = state
        .entries
        .iter()
        .filter(|entry| entry.kind == PromptSourceKind::ExtraPrompt)
        .filter_map(|entry| {
            let body_path =
                prompts_dir.join(project_extra_prompt_file_name(&entry.reference_id).ok()?);
            let body = read_optional_text_file(&body_path).ok()??;
            Some(StoredPromptBody {
                reference_id: entry.reference_id.clone(),
                title: entry.title.clone(),
                body,
            })
        })
        .collect();

    Ok(state)
}

/// `save_project_prompt_assembly_state` 把项目级 prompt assembly 状态写入 `.hunea/`。
pub fn save_project_prompt_assembly_state(
    work_dir: &Path,
    state: &PromptAssemblyScopeState,
) -> Result<(), ProjectPromptAssemblyError> {
    if state.scope != PromptAssemblyScope::Project {
        return Err(ProjectPromptAssemblyError::UnexpectedScope { scope: state.scope });
    }

    let config_path = project_prompt_assembly_path(work_dir);
    let prompts_dir = project_prompts_dir(work_dir);
    fs::create_dir_all(&prompts_dir).map_err(|source| ProjectPromptAssemblyError::Write {
        path: prompts_dir.clone(),
        source,
    })?;

    let extra_prompt_entry_ids = state
        .entries
        .iter()
        .filter(|entry| entry.kind == PromptSourceKind::ExtraPrompt)
        .map(|entry| entry.reference_id.as_str())
        .collect::<BTreeSet<_>>();

    let mut desired_files = BTreeSet::new();
    for prompt in &state.extra_prompts {
        if !extra_prompt_entry_ids.contains(prompt.reference_id.as_str()) {
            return Err(ProjectPromptAssemblyError::OrphanExtraPromptBody {
                reference_id: prompt.reference_id.clone(),
            });
        }

        let file_name = project_extra_prompt_file_name(&prompt.reference_id)?;
        let path = prompts_dir.join(&file_name);
        write_text_file(&path, &prompt.body)?;
        desired_files.insert(file_name);
    }

    let core_override_path = prompts_dir.join(PROJECT_CORE_SYSTEM_OVERRIDE_FILE_NAME);
    match state.core_system_override.as_deref() {
        Some(body) => {
            write_text_file(&core_override_path, body)?;
            desired_files.insert(PROJECT_CORE_SYSTEM_OVERRIDE_FILE_NAME.to_string());
        }
        None => remove_file_if_exists(&core_override_path)?,
    }

    prune_stale_prompt_files(&prompts_dir, &desired_files)?;

    let config = ProjectPromptAssemblyFile {
        version: PROJECT_PROMPT_ASSEMBLY_VERSION,
        entries: sort_entries(state.entries.clone()),
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
    prompts_dir: &Path,
    desired_files: &BTreeSet<String>,
) -> Result<(), ProjectPromptAssemblyError> {
    let entries = match fs::read_dir(prompts_dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(source) => {
            return Err(ProjectPromptAssemblyError::Read {
                path: prompts_dir.to_path_buf(),
                source,
            });
        }
    };

    for entry in entries {
        let entry = entry.map_err(|source| ProjectPromptAssemblyError::Read {
            path: prompts_dir.to_path_buf(),
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

fn project_extra_prompt_file_name(
    reference_id: &str,
) -> Result<String, ProjectPromptAssemblyError> {
    validate_project_reference_id(reference_id)?;
    Ok(format!("{reference_id}.md"))
}

fn validate_project_reference_id(reference_id: &str) -> Result<(), ProjectPromptAssemblyError> {
    if reference_id.is_empty() || reference_id == "__core-system__" {
        return Err(ProjectPromptAssemblyError::InvalidProjectReferenceId {
            reference_id: reference_id.to_string(),
        });
    }

    let path = Path::new(reference_id);
    let mut components = path.components();
    let Some(Component::Normal(_)) = components.next() else {
        return Err(ProjectPromptAssemblyError::InvalidProjectReferenceId {
            reference_id: reference_id.to_string(),
        });
    };
    if components.next().is_some() {
        return Err(ProjectPromptAssemblyError::InvalidProjectReferenceId {
            reference_id: reference_id.to_string(),
        });
    }

    Ok(())
}

fn sort_entries(
    mut entries: Vec<PersistedPromptAssemblyEntry>,
) -> Vec<PersistedPromptAssemblyEntry> {
    entries.sort_by(|left, right| {
        left.requested_order
            .unwrap_or(u16::MAX)
            .cmp(&right.requested_order.unwrap_or(u16::MAX))
            .then_with(|| left.reference_id.cmp(&right.reference_id))
    });
    entries
}

/// `extra_prompt_bodies_by_reference` 把 extra prompt bodies 投影成按 reference_id 索引的映射。
#[must_use]
pub fn extra_prompt_bodies_by_reference(
    state: &PromptAssemblyScopeState,
) -> BTreeMap<&str, &StoredPromptBody> {
    state
        .extra_prompts
        .iter()
        .map(|prompt| (prompt.reference_id.as_str(), prompt))
        .collect()
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
        PromptAssemblyScopeState {
            scope: PromptAssemblyScope::Project,
            core_system_override: Some("project core override".to_string()),
            entries: vec![
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
            ],
            extra_prompts: vec![StoredPromptBody {
                reference_id: "repo-review-rules".to_string(),
                title: "repo-review-rules".to_string(),
                body: "always check tests".to_string(),
            }],
        }
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

        let cleared_state = PromptAssemblyScopeState {
            scope: PromptAssemblyScope::Project,
            core_system_override: None,
            entries: vec![PersistedPromptAssemblyEntry {
                reference_id: "skill-discovery".to_string(),
                kind: PromptSourceKind::SkillDiscovery,
                title: "Skill discovery source".to_string(),
                enabled: true,
                requested_order: Some(10),
            }],
            extra_prompts: Vec::new(),
        };
        save_project_prompt_assembly_state(&work_dir, &cleared_state)
            .expect("updated project prompt assembly should save");

        let prompts_dir = project_prompts_dir(&work_dir);
        assert!(
            !prompts_dir.join("repo-review-rules.md").exists(),
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
        let state = PromptAssemblyScopeState {
            scope: PromptAssemblyScope::Project,
            core_system_override: None,
            entries: vec![PersistedPromptAssemblyEntry {
                reference_id: "missing-prompt".to_string(),
                kind: PromptSourceKind::ExtraPrompt,
                title: "missing-prompt".to_string(),
                enabled: true,
                requested_order: Some(10),
            }],
            extra_prompts: Vec::new(),
        };

        save_project_prompt_assembly_state(&work_dir, &state)
            .expect("project prompt assembly with missing body should save");
        let loaded = load_project_prompt_assembly_state(&work_dir)
            .expect("project prompt assembly should load");

        assert_eq!(loaded.entries, state.entries);
        assert!(loaded.extra_prompts.is_empty());
    }
}
