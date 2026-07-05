use std::{
    collections::{BTreeMap, BTreeSet},
    fs, io,
    path::{Component, Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::dynamic_environment::DynamicEnvironmentSourceSelection;

use crate::text::natural_sort_text_cmp;

use super::{PromptSourceKind, derive_extra_prompt_title, requested_order_sort_key};

pub const PROJECT_PROMPT_ASSEMBLY_FILE_NAME: &str = "prompt-assembly.toml";
pub const PROJECT_PROMPTS_DIR_NAME: &str = "prompts";
pub const PROJECT_CUSTOM_PROMPTS_DIR_NAME: &str = "custom";
pub const PROJECT_CORE_SYSTEM_OVERRIDE_FILE_NAME: &str = "__core-system__.md";
pub const PROJECT_SKILL_DISCOVERY_OVERRIDE_FILE_NAME: &str = "__skill-discovery__.md";
pub const PROJECT_TOOL_GUIDELINES_OVERRIDE_FILE_NAME: &str = "__tool-guidelines__.md";
const PROJECT_RESERVED_PROMPT_REFERENCE_IDS: [&str; 3] = [
    "__core-system__",
    "__skill-discovery__",
    "__tool-guidelines__",
];
const PROJECT_RESERVED_PROMPT_FILE_NAMES: [&str; 3] = [
    PROJECT_CORE_SYSTEM_OVERRIDE_FILE_NAME,
    PROJECT_SKILL_DISCOVERY_OVERRIDE_FILE_NAME,
    PROJECT_TOOL_GUIDELINES_OVERRIDE_FILE_NAME,
];
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

/// `PersistedSkillDiscoverySkillEntry` 表示 skill discovery 里单个 skill 的选中与顺序。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersistedSkillDiscoverySkillEntry {
    pub skill_name: String,
    pub enabled: bool,
    pub requested_order: Option<u16>,
}

/// `PersistedToolSelectionEntry` 表示 tool guidelines 里单个工具的选中与顺序。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersistedToolSelectionEntry {
    pub tool_name: String,
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
    scope: PromptAssemblyScope,
    core_system_override: Option<String>,
    skill_discovery_override: Option<String>,
    tool_guidelines_override: Option<String>,
    entries: Vec<PersistedPromptAssemblyEntry>,
    skill_discovery_skills: Vec<PersistedSkillDiscoverySkillEntry>,
    tool_selections: Vec<PersistedToolSelectionEntry>,
    dynamic_environment_sources: Vec<DynamicEnvironmentSourceSelection>,
    extra_prompts: Vec<StoredPromptBody>,
}

impl PromptAssemblyScopeState {
    /// `new` 构造一个空 scope 状态。
    #[must_use]
    pub fn new(scope: PromptAssemblyScope) -> Self {
        Self {
            scope,
            core_system_override: None,
            skill_discovery_override: None,
            tool_guidelines_override: None,
            entries: Vec::new(),
            skill_discovery_skills: Vec::new(),
            tool_selections: Vec::new(),
            dynamic_environment_sources: Vec::new(),
            extra_prompts: Vec::new(),
        }
    }

    /// `scope` 返回该状态所属的 prompt assembly scope。
    #[must_use]
    pub const fn scope(&self) -> PromptAssemblyScope {
        self.scope
    }

    /// `core_system_override` 返回 core system override 文本。
    #[must_use]
    pub fn core_system_override(&self) -> Option<&str> {
        self.core_system_override.as_deref()
    }

    /// `set_core_system_override` 设置 core system override 文本。
    pub fn set_core_system_override(&mut self, body: Option<String>) {
        self.core_system_override = body;
    }

    /// `skill_discovery_override` 返回 skill discovery override 文本。
    #[must_use]
    pub fn skill_discovery_override(&self) -> Option<&str> {
        self.skill_discovery_override.as_deref()
    }

    /// `set_skill_discovery_override` 设置 skill discovery override 文本。
    pub fn set_skill_discovery_override(&mut self, body: Option<String>) {
        self.skill_discovery_override = body;
    }

    /// `tool_guidelines_override` 返回 tool guidelines override 文本。
    #[must_use]
    pub fn tool_guidelines_override(&self) -> Option<&str> {
        self.tool_guidelines_override.as_deref()
    }

    /// `set_tool_guidelines_override` 设置 tool guidelines override 文本。
    pub fn set_tool_guidelines_override(&mut self, body: Option<String>) {
        self.tool_guidelines_override = body;
    }

    /// `entries` 返回该 scope 下持久化的 source entries。
    #[must_use]
    pub fn entries(&self) -> &[PersistedPromptAssemblyEntry] {
        &self.entries
    }

    /// `entries_mut` 返回该 scope 下持久化 source entries 的可变集合。
    pub fn entries_mut(&mut self) -> &mut Vec<PersistedPromptAssemblyEntry> {
        &mut self.entries
    }

    /// `set_entries` 替换该 scope 下持久化的 source entries。
    pub fn set_entries(&mut self, entries: Vec<PersistedPromptAssemblyEntry>) {
        self.entries = entries;
    }

    /// `skill_discovery_skills` 返回 skill discovery 中单个 skill 的持久化选择。
    #[must_use]
    pub fn skill_discovery_skills(&self) -> &[PersistedSkillDiscoverySkillEntry] {
        &self.skill_discovery_skills
    }

    /// `skill_discovery_skills_mut` 返回 skill discovery skill 选择的可变集合。
    pub fn skill_discovery_skills_mut(&mut self) -> &mut Vec<PersistedSkillDiscoverySkillEntry> {
        &mut self.skill_discovery_skills
    }

    /// `set_skill_discovery_skills` 替换 skill discovery skill 选择。
    pub fn set_skill_discovery_skills(&mut self, skills: Vec<PersistedSkillDiscoverySkillEntry>) {
        self.skill_discovery_skills = skills;
    }

    /// `tool_selections` 返回 tool guideline 中单个 tool 的持久化选择。
    #[must_use]
    pub fn tool_selections(&self) -> &[PersistedToolSelectionEntry] {
        &self.tool_selections
    }

    /// `tool_selections_mut` 返回 tool guideline tool 选择的可变集合。
    pub fn tool_selections_mut(&mut self) -> &mut Vec<PersistedToolSelectionEntry> {
        &mut self.tool_selections
    }

    /// `set_tool_selections` 替换 tool guideline tool 选择。
    pub fn set_tool_selections(&mut self, tool_selections: Vec<PersistedToolSelectionEntry>) {
        self.tool_selections = tool_selections;
    }

    /// `dynamic_environment_sources` 返回 dynamic environment source 选择。
    #[must_use]
    pub fn dynamic_environment_sources(&self) -> &[DynamicEnvironmentSourceSelection] {
        &self.dynamic_environment_sources
    }

    /// `dynamic_environment_sources_mut` 返回 dynamic environment source 选择的可变集合。
    pub fn dynamic_environment_sources_mut(
        &mut self,
    ) -> &mut Vec<DynamicEnvironmentSourceSelection> {
        &mut self.dynamic_environment_sources
    }

    /// `set_dynamic_environment_sources` 替换 dynamic environment source 选择。
    pub fn set_dynamic_environment_sources(
        &mut self,
        sources: Vec<DynamicEnvironmentSourceSelection>,
    ) {
        self.dynamic_environment_sources = sources;
    }

    /// `extra_prompts` 返回该 scope 下持久化的 custom prompt bodies。
    #[must_use]
    pub fn extra_prompts(&self) -> &[StoredPromptBody] {
        &self.extra_prompts
    }

    /// `extra_prompts_mut` 返回该 scope 下 custom prompt bodies 的可变集合。
    pub fn extra_prompts_mut(&mut self) -> &mut Vec<StoredPromptBody> {
        &mut self.extra_prompts
    }

    /// `set_extra_prompts` 替换该 scope 下持久化的 custom prompt bodies。
    pub fn set_extra_prompts(&mut self, prompts: Vec<StoredPromptBody>) {
        self.extra_prompts = prompts;
    }
}

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
    #[error("project prompt reference id must be a single safe file component: {reference_id}")]
    InvalidProjectReferenceId { reference_id: String },
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

    Ok(PromptAssemblyScopeState {
        scope: PromptAssemblyScope::Project,
        core_system_override,
        skill_discovery_override,
        tool_guidelines_override,
        entries,
        skill_discovery_skills,
        tool_selections,
        dynamic_environment_sources: Vec::new(),
        extra_prompts,
    })
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
    for prompt in &state.extra_prompts {
        let file_name = project_extra_prompt_file_name(&prompt.reference_id)?;
        let path = custom_prompts_dir.join(&file_name);
        write_text_file(&path, &prompt.body)?;
        desired_custom_prompt_files.insert(file_name);
    }

    let core_override_path = prompts_dir.join(PROJECT_CORE_SYSTEM_OVERRIDE_FILE_NAME);
    match state.core_system_override.as_deref() {
        Some(body) => {
            write_text_file(&core_override_path, body)?;
        }
        None => remove_file_if_exists(&core_override_path)?,
    }

    let skill_discovery_override_path =
        prompts_dir.join(PROJECT_SKILL_DISCOVERY_OVERRIDE_FILE_NAME);
    match state.skill_discovery_override.as_deref() {
        Some(body) => {
            write_text_file(&skill_discovery_override_path, body)?;
        }
        None => remove_file_if_exists(&skill_discovery_override_path)?,
    }

    let tool_guidelines_override_path =
        prompts_dir.join(PROJECT_TOOL_GUIDELINES_OVERRIDE_FILE_NAME);
    match state.tool_guidelines_override.as_deref() {
        Some(body) => {
            write_text_file(&tool_guidelines_override_path, body)?;
        }
        None => remove_file_if_exists(&tool_guidelines_override_path)?,
    }

    prune_stale_prompt_files(&custom_prompts_dir, &desired_custom_prompt_files)?;

    let config = ProjectPromptAssemblyFile {
        version: PROJECT_PROMPT_ASSEMBLY_VERSION,
        entries: sorted_prompt_assembly_entries(state.entries.clone()),
        skill_discovery_skills: sorted_skill_discovery_skill_entries(
            state.skill_discovery_skills.clone(),
        ),
        tool_selections: sorted_tool_selection_entries(state.tool_selections.clone()),
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

fn project_extra_prompt_file_name(
    reference_id: &str,
) -> Result<String, ProjectPromptAssemblyError> {
    validate_project_reference_id(reference_id)?;
    Ok(format!("{reference_id}.md"))
}

fn project_extra_prompt_reference_id_from_file_name(file_name: &str) -> Option<&str> {
    if PROJECT_RESERVED_PROMPT_FILE_NAMES.contains(&file_name) {
        return None;
    }

    let reference_id = file_name.strip_suffix(".md")?;
    validate_project_reference_id(reference_id).ok()?;
    Some(reference_id)
}

fn validate_project_reference_id(reference_id: &str) -> Result<(), ProjectPromptAssemblyError> {
    if reference_id.is_empty() || PROJECT_RESERVED_PROMPT_REFERENCE_IDS.contains(&reference_id) {
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

/// `sort_prompt_assembly_entries` 按领域展示顺序就地排序 persisted source entries。
pub fn sort_prompt_assembly_entries(entries: &mut [PersistedPromptAssemblyEntry]) {
    entries.sort_by(|left, right| {
        requested_order_sort_key(left.requested_order)
            .cmp(&requested_order_sort_key(right.requested_order))
            .then_with(|| natural_sort_text_cmp(&left.title, &right.title))
            .then_with(|| left.reference_id.cmp(&right.reference_id))
    });
}

fn sorted_prompt_assembly_entries(
    mut entries: Vec<PersistedPromptAssemblyEntry>,
) -> Vec<PersistedPromptAssemblyEntry> {
    sort_prompt_assembly_entries(&mut entries);
    entries
}

/// `sort_skill_discovery_skill_entries` 按领域展示顺序就地排序 skill discovery entries。
pub fn sort_skill_discovery_skill_entries(entries: &mut [PersistedSkillDiscoverySkillEntry]) {
    entries.sort_by(|left, right| {
        requested_order_sort_key(left.requested_order)
            .cmp(&requested_order_sort_key(right.requested_order))
            .then_with(|| natural_sort_text_cmp(&left.skill_name, &right.skill_name))
    });
}

fn sorted_skill_discovery_skill_entries(
    mut entries: Vec<PersistedSkillDiscoverySkillEntry>,
) -> Vec<PersistedSkillDiscoverySkillEntry> {
    sort_skill_discovery_skill_entries(&mut entries);
    entries
}

/// `sort_tool_selection_entries` 按领域展示顺序就地排序 tool selection entries。
pub fn sort_tool_selection_entries(entries: &mut [PersistedToolSelectionEntry]) {
    entries.sort_by(|left, right| {
        requested_order_sort_key(left.requested_order)
            .cmp(&requested_order_sort_key(right.requested_order))
            .then_with(|| natural_sort_text_cmp(&left.tool_name, &right.tool_name))
    });
}

fn sorted_tool_selection_entries(
    mut entries: Vec<PersistedToolSelectionEntry>,
) -> Vec<PersistedToolSelectionEntry> {
    sort_tool_selection_entries(&mut entries);
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
            skill_discovery_override: None,
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
            skill_discovery_skills: Vec::new(),
            extra_prompts: vec![StoredPromptBody {
                reference_id: "repo-review-rules".to_string(),
                title: "repo-review-rules".to_string(),
                body: "always check tests".to_string(),
            }],
            tool_guidelines_override: None,
            tool_selections: Vec::new(),
            dynamic_environment_sources: Vec::new(),
        }
    }

    #[test]
    fn scope_state_new_builds_state_through_domain_api() {
        let mut state = PromptAssemblyScopeState::new(PromptAssemblyScope::Global);
        state.set_core_system_override(Some("core".to_string()));
        state.entries_mut().push(PersistedPromptAssemblyEntry {
            reference_id: "skill-discovery".to_string(),
            kind: PromptSourceKind::SkillDiscovery,
            title: "Skill discovery".to_string(),
            enabled: true,
            requested_order: Some(1),
        });

        assert_eq!(state.scope(), PromptAssemblyScope::Global);
        assert_eq!(state.core_system_override(), Some("core"));
        assert_eq!(state.entries()[0].reference_id, "skill-discovery");
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
            skill_discovery_override: None,
            entries: vec![PersistedPromptAssemblyEntry {
                reference_id: "skill-discovery".to_string(),
                kind: PromptSourceKind::SkillDiscovery,
                title: "Skill discovery source".to_string(),
                enabled: true,
                requested_order: Some(10),
            }],
            skill_discovery_skills: Vec::new(),
            extra_prompts: Vec::new(),
            tool_guidelines_override: None,
            tool_selections: Vec::new(),
            dynamic_environment_sources: Vec::new(),
        };
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
        let state = PromptAssemblyScopeState {
            scope: PromptAssemblyScope::Project,
            core_system_override: None,
            skill_discovery_override: None,
            entries: vec![PersistedPromptAssemblyEntry {
                reference_id: "missing-prompt".to_string(),
                kind: PromptSourceKind::ExtraPrompt,
                title: "missing-prompt".to_string(),
                enabled: true,
                requested_order: Some(10),
            }],
            skill_discovery_skills: Vec::new(),
            extra_prompts: Vec::new(),
            tool_guidelines_override: None,
            tool_selections: Vec::new(),
            dynamic_environment_sources: Vec::new(),
        };

        save_project_prompt_assembly_state(&work_dir, &state)
            .expect("project prompt assembly with missing body should save");
        let loaded = load_project_prompt_assembly_state(&work_dir)
            .expect("project prompt assembly should load");

        assert_eq!(loaded.entries, state.entries);
        assert!(loaded.extra_prompts.is_empty());
    }

    #[test]
    fn project_scope_roundtrip_persists_unselected_extra_prompt_body_in_custom_dir() {
        let work_dir = temp_test_dir("project-unselected-extra-prompt");
        let state = PromptAssemblyScopeState {
            scope: PromptAssemblyScope::Project,
            core_system_override: None,
            skill_discovery_override: None,
            entries: vec![PersistedPromptAssemblyEntry {
                reference_id: "skill-discovery".to_string(),
                kind: PromptSourceKind::SkillDiscovery,
                title: "Skill discovery source".to_string(),
                enabled: true,
                requested_order: Some(10),
            }],
            skill_discovery_skills: Vec::new(),
            extra_prompts: vec![StoredPromptBody {
                reference_id: "review-rules".to_string(),
                title: "Review rules".to_string(),
                body: "# Review rules\nAlways verify tests.\n".to_string(),
            }],
            tool_guidelines_override: None,
            tool_selections: Vec::new(),
            dynamic_environment_sources: Vec::new(),
        };

        save_project_prompt_assembly_state(&work_dir, &state)
            .expect("project prompt assembly should save");
        let loaded = load_project_prompt_assembly_state(&work_dir)
            .expect("project prompt assembly should load");

        assert_eq!(loaded.extra_prompts, state.extra_prompts);
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
            state.extra_prompts = vec![StoredPromptBody {
                reference_id: reference_id.to_string(),
                title: reference_id.to_string(),
                body: "reserved body must not be saved as a custom prompt".to_string(),
            }];

            let error = save_project_prompt_assembly_state(&work_dir, &state)
                .expect_err("reserved custom prompt reference id should be rejected");

            assert!(matches!(
                error,
                ProjectPromptAssemblyError::InvalidProjectReferenceId {
                    reference_id: rejected
                } if rejected == reference_id
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
            loaded.extra_prompts.is_empty(),
            "root-level prompt files should no longer be loaded"
        );
    }
}
