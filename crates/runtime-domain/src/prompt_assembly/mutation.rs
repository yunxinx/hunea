use std::path::PathBuf;

use crate::dynamic_environment::{DynamicEnvironmentSnapshotKind, DynamicEnvironmentSourceKind};

use super::persistence::PromptAssemblyScope;
use super::types::{PromptSourceKind, PromptSourceOrigin};

/// `PromptAssemblyEditorTarget` 标识一次外部编辑器保存要落到哪里。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromptAssemblyEditorTarget {
    CoreSystemOverride {
        scope: PromptAssemblyScope,
    },
    SkillDiscovery {
        scope: PromptAssemblyScope,
    },
    ToolGuidelines {
        scope: PromptAssemblyScope,
    },
    InstructionsFile {
        path: PathBuf,
    },
    ExtraPrompt {
        scope: PromptAssemblyScope,
        reference_id: String,
    },
    SkillFile {
        skill_name: String,
        origin: PromptSourceOrigin,
    },
}

/// `PromptAssemblyMutation` 描述 `/prompt` 发起的一次持久化变更。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromptAssemblyMutation {
    SaveEditorTarget {
        target: PromptAssemblyEditorTarget,
        content: String,
    },
    Scoped(PromptAssemblyScopedMutation),
    SetDynamicEnvironmentSourceSelected {
        snapshot_kind: DynamicEnvironmentSnapshotKind,
        source_kind: DynamicEnvironmentSourceKind,
        selected: bool,
    },
}

impl PromptAssemblyMutation {
    /// `scoped` 构造带明确 scope 的 prompt assembly mutation。
    #[must_use]
    pub fn scoped(scope: PromptAssemblyScope, kind: PromptAssemblyScopedMutationKind) -> Self {
        Self::Scoped(PromptAssemblyScopedMutation { scope, kind })
    }
}

/// `PromptAssemblyScopedMutation` 表示作用于单个 prompt assembly scope 的变更。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptAssemblyScopedMutation {
    pub scope: PromptAssemblyScope,
    pub kind: PromptAssemblyScopedMutationKind,
}

/// `PromptAssemblyScopedMutationKind` 描述 scope 内部的具体变更。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromptAssemblyScopedMutationKind {
    SetExtraPromptSelected {
        reference_id: String,
        selected: bool,
    },
    SetPromptSourceEnabled {
        kind: PromptSourceKind,
        reference_id: String,
        enabled: bool,
    },
    SetDiscoveredSkillSelected {
        skill_name: String,
        selected: bool,
    },
    MoveDiscoveredSkill {
        skill_name: String,
        direction: PromptAssemblyMoveDirection,
    },
    ResetDiscoveredSkillOrder,
    SetToolSelected {
        tool_name: String,
        selected: bool,
    },
    MoveTool {
        tool_name: String,
        direction: PromptAssemblyMoveDirection,
    },
    /// 切换工具本体启用/禁用；禁用的工具不进入 provider 请求，guidelines 随之不注入。
    SetToolEnabled {
        tool_name: String,
        enabled: bool,
    },
    ActivateLongLivedSkill {
        skill_name: String,
    },
    CreateExtraPrompt {
        content: String,
    },
    RemovePromptSource {
        kind: PromptSourceKind,
        reference_id: String,
    },
    MoveActiveSource {
        kind: PromptSourceKind,
        reference_id: String,
        direction: PromptAssemblyMoveDirection,
    },
    DeleteExtraPrompt {
        reference_id: String,
    },
    RestoreCoreSystemOverride,
}

/// `PromptAssemblyMoveDirection` 描述 active source 的排序方向。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptAssemblyMoveDirection {
    Up,
    Down,
}
