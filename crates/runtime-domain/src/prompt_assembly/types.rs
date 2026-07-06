use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::dynamic_environment::DynamicEnvironmentSourceKind;

use super::persistence::PromptAssemblyScope;
use super::resolution::resolve_prompt_assembly;

/// `PromptAssemblyLifecycle` 表示 prompt assembly 生效的生命周期边界。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptAssemblyLifecycle {
    /// 仅影响下一次全新 session 的 prompt 装配，不回写当前 transcript。
    NextNewSession,
}

/// `PromptSourceKind` 表示 prompt source 的领域类别。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptSourceKind {
    CoreSystemPrompt,
    InstructionsFile,
    ExtraPrompt,
    SkillDiscovery,
    /// 长期注入型 skill，和当前消息里的 `$skill` 临时注入不同。
    LongLivedSkill,
    /// 工具使用指南，body 从工具注册表动态生成。
    ToolGuidelines,
    /// 首轮注入的环境基线。
    DynamicEnvironmentBaseline,
    /// 后续轮次按变化注入的环境差异。
    DynamicEnvironmentChanges,
}

/// `PromptSourceOrigin` 表示 prompt source 的来源层级。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptSourceOrigin {
    Builtin,
    Global,
    Project,
}

impl PromptSourceOrigin {
    /// `as_str` 返回适合序列化到 tool metadata 的稳定来源标签。
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Builtin => "builtin",
            Self::Global => "global",
            Self::Project => "project",
        }
    }
}

/// `PromptSourceInactiveReason` 表示 source 为何没有进入 active assembly。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptSourceInactiveReason {
    Disabled,
    Missing,
    Shadowed,
}

/// `PromptSourceStatus` 表示 source 当前是 active 还是 inactive。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromptSourceStatus {
    Active { order: usize },
    Inactive { reason: PromptSourceInactiveReason },
}

/// `CoreSystemPromptInput` 描述 core system prompt 的 override 输入。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CoreSystemPromptInput {
    pub global_override_present: bool,
    pub project_override_present: bool,
}

/// `PromptSourceCandidate` 描述参与 resolution 的非 core prompt source。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptSourceCandidate {
    pub reference_id: String,
    pub kind: PromptSourceKind,
    pub title: String,
    pub origin: Option<PromptSourceOrigin>,
    pub collision_key: Option<String>,
    pub state: PromptSourceCandidateState,
    pub requested_order: Option<u16>,
}

/// `PromptSourceCandidateState` 表示 candidate 自身是否可进入 resolution。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptSourceCandidateState {
    Enabled,
    Disabled,
    Missing,
}

impl PromptSourceCandidateState {
    /// `from_materialized_source` 把持久化启停与实体可解析性收敛为合法状态。
    #[must_use]
    pub const fn from_materialized_source(enabled: bool, resolvable: bool) -> Self {
        if !enabled {
            Self::Disabled
        } else if !resolvable {
            Self::Missing
        } else {
            Self::Enabled
        }
    }
}

/// `PromptAssemblyInput` 描述一次 next-new-session prompt assembly resolution 的输入。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PromptAssemblyInput {
    pub core_system: CoreSystemPromptInput,
    pub candidates: Vec<PromptSourceCandidate>,
}

/// `ResolvedPromptSource` 表示 resolution 之后的 prompt source。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedPromptSource {
    pub reference_id: String,
    pub kind: PromptSourceKind,
    pub title: String,
    pub origin: Option<PromptSourceOrigin>,
    pub status: PromptSourceStatus,
}

/// `PromptAssemblySnapshot` 表示共享 prompt assembly resolution 结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptAssemblySnapshot {
    pub lifecycle: PromptAssemblyLifecycle,
    pub active_sources: Vec<ResolvedPromptSource>,
    pub inactive_sources: Vec<ResolvedPromptSource>,
}

/// `PromptPreludeSection` 表示落入单个 session 的稳定 prompt prelude section。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptPreludeSection {
    pub reference_id: String,
    pub kind: PromptSourceKind,
    pub title: String,
    pub origin: Option<PromptSourceOrigin>,
    pub body: String,
}

/// `PromptPreludeSnapshot` 表示某个 session 启动时已经解析完成的 prompt prelude。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct PromptPreludeSnapshot {
    pub sections: Vec<PromptPreludeSection>,
}

impl PromptPreludeSnapshot {
    /// `effective_system_prompt` 返回 provider 实际看到的拼装结果。
    #[must_use]
    pub fn effective_system_prompt(&self) -> Option<String> {
        let sections = self
            .sections
            .iter()
            .map(|section| section.body.trim())
            .filter(|body| !body.is_empty())
            .collect::<Vec<_>>();
        (!sections.is_empty()).then(|| sections.join("\n\n"))
    }
}

/// `PromptAssemblyManagerSource` 是 `/prompt` 管理器使用的可预览 source 物化视图。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptAssemblyManagerSource {
    pub reference_id: String,
    pub kind: PromptSourceKind,
    pub title: String,
    pub origin: Option<PromptSourceOrigin>,
    pub resolved_body_origin: Option<PromptSourceOrigin>,
    pub backing_file_path: Option<PathBuf>,
    pub body: Option<String>,
}

/// `PromptAssemblyManagedSource` 表示 `/prompt` 左侧管理列表的一行。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptAssemblyManagedSource {
    pub reference_id: String,
    pub kind: PromptSourceKind,
    pub title: String,
    pub origin: Option<PromptSourceOrigin>,
    pub scope: Option<PromptAssemblyScope>,
    pub enabled: bool,
    pub order: usize,
}

/// `PromptAssemblyExtraPromptCandidate` 表示 `/prompt` 右侧 Extra tab 的候选 prompt。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptAssemblyExtraPromptCandidate {
    pub reference_id: String,
    pub title: String,
    pub origin: PromptSourceOrigin,
    pub body: String,
    pub selected: bool,
}

/// `PromptAssemblySelectionState` 用单一状态表示候选项是否可选、是否已选以及排序。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptAssemblySelectionState {
    Unselectable,
    Available,
    Selected { order: Option<usize> },
}

impl PromptAssemblySelectionState {
    #[must_use]
    pub const fn from_parts(can_select: bool, selected: bool, order: Option<usize>) -> Self {
        match (can_select, selected) {
            (false, _) => Self::Unselectable,
            (true, false) => Self::Available,
            (true, true) => Self::Selected { order },
        }
    }

    #[must_use]
    pub const fn can_select(self) -> bool {
        !matches!(self, Self::Unselectable)
    }

    #[must_use]
    pub const fn is_selected(self) -> bool {
        matches!(self, Self::Selected { .. })
    }

    #[must_use]
    pub const fn selected_order(self) -> Option<usize> {
        match self {
            Self::Selected { order } => order,
            Self::Unselectable | Self::Available => None,
        }
    }
}

/// `PromptAssemblyDiscoveredSkill` 表示 `/prompt` 右侧 Skills tab 可展示的已发现 skill。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptAssemblyDiscoveredSkill {
    pub skill_name: String,
    pub title: String,
    pub description: String,
    pub origin: PromptSourceOrigin,
    pub selection_scope: PromptAssemblyScope,
    pub skill_path: PathBuf,
    pub body: String,
    pub selection: PromptAssemblySelectionState,
}

/// `PromptAssemblyToolCandidate` 表示 `/prompt` Tools Tab 中的一个工具候选项。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptAssemblyToolCandidate {
    pub name: String,
    pub label: Option<String>,
    pub description: Option<String>,
    pub prompt_guidelines: Option<String>,
    pub origin: PromptSourceOrigin,
    pub selection_scope: PromptAssemblyScope,
    pub selection: PromptAssemblySelectionState,
}

/// `PromptAssemblyDynamicEnvironmentCandidate` 表示 Dynamic Tab 中的一个环境来源开关。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptAssemblyDynamicEnvironmentCandidate {
    pub source_kind: DynamicEnvironmentSourceKind,
    pub label: String,
    pub origin: PromptSourceOrigin,
    pub baseline_selected: bool,
    pub changes_selected: bool,
    pub baseline_preview_body: String,
    pub changes_preview_body: String,
}

/// `PromptAssemblyDiagnostic` 表示 prompt assembly 装配过程中保留的非致命诊断。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptAssemblyDiagnostic {
    pub origin: Option<PromptSourceOrigin>,
    pub path: Option<PathBuf>,
    pub message: String,
}

/// `PromptAssemblyManagerSnapshot` 表示 `/prompt` 所需的完整只读快照。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PromptAssemblyManagerSnapshot {
    pub resolution: PromptAssemblyResolvedSnapshot,
    pub sources: PromptAssemblySourceInventorySnapshot,
    pub candidates: PromptAssemblyCandidateInventorySnapshot,
    pub core_system: PromptAssemblyCoreSystemSnapshot,
    pub diagnostics: Vec<PromptAssemblyDiagnostic>,
}

/// `PromptAssemblyResolvedSnapshot` 表示最终解析结果和会进入新 session 的 prelude。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptAssemblyResolvedSnapshot {
    pub assembly: PromptAssemblySnapshot,
    pub prelude: PromptPreludeSnapshot,
}

impl Default for PromptAssemblyResolvedSnapshot {
    fn default() -> Self {
        Self {
            assembly: resolve_prompt_assembly(&PromptAssemblyInput::default()),
            prelude: PromptPreludeSnapshot::default(),
        }
    }
}

/// `PromptAssemblySourceInventorySnapshot` 表示 `/prompt` 左侧管理列表和预览源集合。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PromptAssemblySourceInventorySnapshot {
    pub managed: Vec<PromptAssemblyManagedSource>,
    pub preview: Vec<PromptAssemblyManagerSource>,
}

/// `PromptAssemblyCandidateInventorySnapshot` 表示 `/prompt` 右侧各候选列表。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PromptAssemblyCandidateInventorySnapshot {
    pub extra_prompts: Vec<PromptAssemblyExtraPromptCandidate>,
    pub discovered_skills: Vec<PromptAssemblyDiscoveredSkill>,
    pub manual_skills: Vec<PromptAssemblyDiscoveredSkill>,
    pub tools: Vec<PromptAssemblyToolCandidate>,
    pub dynamic_environment: Vec<PromptAssemblyDynamicEnvironmentCandidate>,
}

/// `PromptAssemblyCoreSystemSnapshot` 表示 core system prompt 的默认体和覆盖层。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PromptAssemblyCoreSystemSnapshot {
    pub builtin_body: String,
    pub global_override: Option<String>,
    pub project_override: Option<String>,
}
