use runtime_domain::dynamic_environment::DynamicEnvironmentSnapshotKind;
use runtime_domain::prompt_assembly::persistence::PromptAssemblyScope;
use runtime_domain::prompt_assembly::{PromptAssemblyEditorTarget, PromptSourceKind};

use super::dialog::PromptOverlayDialog;
use super::preview;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PromptOverlayFocus {
    Active,
    Inactive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PromptOverlayInactiveTab {
    LongLivedSkills,
    ExtraPrompts,
    Tools,
    Dynamic,
}

/// `PromptOverlayToolColumn` 表示 Tools tab 当前选中的开关列，交互对照 Dynamic tab 的
/// snapshot 列选择：`←/→` 切列、`x` 切换当前列。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PromptOverlayToolColumn {
    /// 工具本体启停（On 列）。
    Enablement,
    /// guidelines 注入选择（Guide 列）。
    Guidelines,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum PromptOverlayExpandedRow {
    ActiveSource {
        reference_id: String,
        kind: PromptSourceKind,
    },
    InactiveExtraPrompt {
        reference_id: String,
    },
    InactiveDiscoveredSkill {
        skill_name: String,
    },
}

impl PromptOverlayInactiveTab {
    pub(super) const ALL: [Self; 4] = [
        Self::LongLivedSkills,
        Self::ExtraPrompts,
        Self::Tools,
        Self::Dynamic,
    ];

    pub(super) fn next(self) -> Self {
        match self {
            Self::LongLivedSkills => Self::ExtraPrompts,
            Self::ExtraPrompts => Self::Tools,
            Self::Tools => Self::Dynamic,
            Self::Dynamic => Self::LongLivedSkills,
        }
    }

    pub(super) fn previous(self) -> Self {
        match self {
            Self::LongLivedSkills => Self::Dynamic,
            Self::ExtraPrompts => Self::LongLivedSkills,
            Self::Tools => Self::ExtraPrompts,
            Self::Dynamic => Self::Tools,
        }
    }

    pub(super) fn label(self) -> &'static str {
        match self {
            Self::LongLivedSkills => "Skill",
            Self::ExtraPrompts => "Prompts",
            Self::Tools => "Tools",
            Self::Dynamic => "Dynamic",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PromptOverlayState {
    pub(crate) focus: PromptOverlayFocus,
    pub(crate) active_selected: usize,
    pub(crate) active_scroll: usize,
    pub(crate) active_selected_row_id: Option<String>,
    pub(crate) inactive_tab: PromptOverlayInactiveTab,
    pub(crate) inactive_selected: usize,
    pub(crate) inactive_scroll: usize,
    pub(crate) inactive_selected_row_id: Option<String>,
    pub(super) dynamic_selected_snapshot_kind: DynamicEnvironmentSnapshotKind,
    pub(super) tool_selected_column: PromptOverlayToolColumn,
    pub(super) expanded_row: Option<PromptOverlayExpandedRow>,
    pub(super) dialog: Option<PromptOverlayDialog>,
    pub(crate) preview: Option<preview::PromptOverlayPreviewState>,
    pub(super) shortcut_help_open: bool,
    pub(crate) draft_scope: PromptAssemblyScope,
    pub(crate) pending_editor: Option<PromptOverlayPendingEditor>,
}

impl Default for PromptOverlayState {
    fn default() -> Self {
        Self {
            focus: PromptOverlayFocus::Active,
            active_selected: 0,
            active_scroll: 0,
            active_selected_row_id: None,
            inactive_tab: PromptOverlayInactiveTab::LongLivedSkills,
            inactive_selected: 0,
            inactive_scroll: 0,
            inactive_selected_row_id: None,
            dynamic_selected_snapshot_kind: DynamicEnvironmentSnapshotKind::Baseline,
            tool_selected_column: PromptOverlayToolColumn::Enablement,
            expanded_row: None,
            dialog: None,
            preview: None,
            shortcut_help_open: false,
            draft_scope: PromptAssemblyScope::Project,
            pending_editor: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PromptOverlayPendingEditor {
    pub(crate) target: PromptAssemblyEditorTarget,
    pub(crate) original_draft: String,
    pub(crate) cleanup_path_after_finish: bool,
}
