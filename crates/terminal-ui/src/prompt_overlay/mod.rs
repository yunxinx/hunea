mod actions;
mod dialog;
mod editor;
mod input;
mod preview;
mod render;
mod render_cells;
mod render_rows;
mod render_support;
mod selection;
mod state;
mod sync;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton};
use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Clear, Paragraph, Widget},
};
use runtime_domain::dynamic_environment::DynamicEnvironmentSnapshotKind;
use runtime_domain::prompt_assembly::persistence::PromptAssemblyScope;
use runtime_domain::prompt_assembly::{
    PromptAssemblyDiscoveredSkill, PromptAssemblyDynamicEnvironmentCandidate,
    PromptAssemblyExtraPromptCandidate, PromptAssemblyManagedSource, PromptAssemblyManagerSource,
    PromptAssemblyMoveDirection, PromptAssemblyMutation, PromptAssemblyScopedMutationKind,
    PromptAssemblyToolCandidate, PromptSourceKind, PromptSourceOrigin, PromptSourceStatus,
    ResolvedPromptSource, SKILL_DISCOVERY_GENERATED_END, SKILL_DISCOVERY_GENERATED_START,
    TOOL_GUIDELINES_GENERATED_END, TOOL_GUIDELINES_GENERATED_START, default_extra_prompt_body,
    next_default_extra_prompt_title,
};
use runtime_domain::text::natural_sort_text_cmp;

use crate::{
    AppEffect, Model,
    display_width::display_width,
    fullscreen_list_chrome::fullscreen_list_chrome_rects,
    list_selection::{ListNavigationDirection, VisibleWindowSelection},
    overlay_input_result::OverlayInputResult,
    relative_age::left_pad_display_width,
    render_frame::RenderFrame,
    shortcut_help_popover::{ShortcutHelpEntry, ShortcutHelpPopover, aligned_shortcut_help_lines},
    status_line::truncate_display_width_with_ellipsis,
    styled_text::render_line_with_full_width_background,
    theme::{
        build_labeled_rule, command_accent_text_style, panel_block, primary_text_style,
        secondary_text_style, subtle_rule_line, surface_text_style, table_header_text_style,
        tertiary_text_style,
    },
};
use dialog::PromptOverlayDialog;
use render_cells::*;
use render_rows::*;
use render_support::*;
use state::PromptOverlayExpandedRow;
#[cfg(test)]
pub(crate) use state::PromptOverlayPendingEditor;
pub(crate) use state::{PromptOverlayFocus, PromptOverlayInactiveTab, PromptOverlayState};

#[cfg(test)]
mod tests;

const PROMPT_OVERLAY_HEADER_INSET: usize = 2;
const PROMPT_OVERLAY_HEADER_TRAILING_PADDING: usize = 2;
const PROMPT_OVERLAY_ROW_PREFIX_WIDTH: usize = 1;
const PROMPT_OVERLAY_COLUMN_GAP: usize = 2;
const PROMPT_OVERLAY_OUTER_PADDING: usize = 2;
const PROMPT_OVERLAY_LEFT_SEL_WIDTH: usize = 3;
const PROMPT_OVERLAY_LEFT_ORD_WIDTH: usize = 3;
const PROMPT_OVERLAY_RIGHT_ORD_WIDTH: usize = 3;
const PROMPT_OVERLAY_DYNAMIC_CHECKBOX_WIDTH: usize = "Change".len();
const PROMPT_OVERLAY_LEFT_KIND_WIDTH: usize = "instructions".len();
const PROMPT_OVERLAY_LEFT_SCOPE_WIDTH: usize = 7;
const PROMPT_OVERLAY_RIGHT_SCOPE_WIDTH: usize = 7;
const PROMPT_OVERLAY_SCOPE_TRAILING_PADDING: usize = 2;
const PROMPT_OVERLAY_LEFT_PANE_RATIO_NUMERATOR: u32 = 9;
const PROMPT_OVERLAY_RIGHT_PANE_RATIO_NUMERATOR: u32 = 11;
const PROMPT_OVERLAY_PANE_RATIO_DENOMINATOR: u32 = 20;
const PROMPT_OVERLAY_FOOTER_MORE_LABEL: &str = "? more";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PromptOverlayManagedStatus {
    Active,
    Disabled,
    Missing,
    Shadowed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum PromptOverlayLeftRow {
    ManagedSource {
        source: PromptAssemblyManagedSource,
        status: PromptOverlayManagedStatus,
        shadowed_count: usize,
    },
    ShadowedDetail {
        source: ResolvedPromptSource,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum PromptOverlayInactiveRow {
    ExtraPromptCandidate {
        source: PromptAssemblyExtraPromptCandidate,
        shadowed_count: usize,
    },
    ExtraPromptShadowedDetail {
        source: PromptAssemblyExtraPromptCandidate,
    },
    DiscoveredSkill {
        skill: PromptAssemblyDiscoveredSkill,
        shadowed_count: usize,
    },
    DiscoveredSkillShadowedDetail {
        skill: PromptAssemblyDiscoveredSkill,
    },
    ToolCandidate {
        tool: PromptAssemblyToolCandidate,
    },
    DynamicEnvironmentCandidate {
        source: PromptAssemblyDynamicEnvironmentCandidate,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum PromptOverlaySelection {
    ManagedSource(PromptAssemblyManagedSource),
    ResolvedSource(ResolvedPromptSource),
    ExtraPromptCandidate(PromptAssemblyExtraPromptCandidate),
    DiscoveredSkill(PromptAssemblyDiscoveredSkill),
    ToolCandidate(PromptAssemblyToolCandidate),
    DynamicEnvironmentCandidate(PromptAssemblyDynamicEnvironmentCandidate),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PromptOverlayActionAvailability {
    Empty {
        can_add_custom: bool,
    },
    PromptSource {
        can_edit: bool,
        can_remove: bool,
        can_toggle_selection: bool,
        can_reorder_active: bool,
    },
    ExtraPromptCandidate {
        can_add_custom: bool,
    },
    SelectableCandidate {
        can_reorder_active: bool,
    },
    DynamicEnvironmentCandidate,
}

impl PromptOverlayActionAvailability {
    const fn can_edit(self) -> bool {
        match self {
            Self::PromptSource { can_edit, .. } => can_edit,
            Self::ExtraPromptCandidate { .. } => true,
            Self::Empty { .. }
            | Self::SelectableCandidate { .. }
            | Self::DynamicEnvironmentCandidate => false,
        }
    }

    const fn can_add_custom(self) -> bool {
        match self {
            Self::Empty { can_add_custom } | Self::ExtraPromptCandidate { can_add_custom } => {
                can_add_custom
            }
            Self::PromptSource { .. }
            | Self::SelectableCandidate { .. }
            | Self::DynamicEnvironmentCandidate => false,
        }
    }

    const fn can_remove(self) -> bool {
        match self {
            Self::PromptSource { can_remove, .. } => can_remove,
            Self::ExtraPromptCandidate { .. } => true,
            Self::Empty { .. }
            | Self::SelectableCandidate { .. }
            | Self::DynamicEnvironmentCandidate => false,
        }
    }

    const fn can_toggle_selection(self) -> bool {
        match self {
            Self::PromptSource {
                can_toggle_selection,
                ..
            } => can_toggle_selection,
            Self::ExtraPromptCandidate { .. }
            | Self::SelectableCandidate { .. }
            | Self::DynamicEnvironmentCandidate => true,
            Self::Empty { .. } => false,
        }
    }

    const fn can_reorder_active(self) -> bool {
        match self {
            Self::PromptSource {
                can_reorder_active, ..
            }
            | Self::SelectableCandidate { can_reorder_active } => can_reorder_active,
            Self::Empty { .. }
            | Self::ExtraPromptCandidate { .. }
            | Self::DynamicEnvironmentCandidate => false,
        }
    }
}

fn prompt_overlay_source_kind_can_remove(kind: PromptSourceKind) -> bool {
    !matches!(
        kind,
        PromptSourceKind::CoreSystemPrompt
            | PromptSourceKind::InstructionsFile
            | PromptSourceKind::SkillDiscovery
            | PromptSourceKind::ToolGuidelines
            | PromptSourceKind::DynamicEnvironmentBaseline
            | PromptSourceKind::DynamicEnvironmentChanges
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PromptOverlayLayoutRects {
    chrome: crate::fullscreen_list_chrome::FullscreenListChromeRects,
    left_pane: Rect,
    left_body: Rect,
    right_pane: Rect,
    right_body: Rect,
}

impl Model {
    pub(crate) fn prompt_overlay_active(&self) -> bool {
        self.prompt_overlay.is_some()
    }

    pub(crate) fn open_prompt_overlay(&mut self) {
        if self.prompt_overlay_active() {
            return;
        }

        self.close_model_panel();
        self.close_tool_approval_panel();
        self.close_composer_attached_ui();
        self.sync_composer_height();
        self.prompt_overlay = Some(PromptOverlayState::default());
        self.sync_prompt_overlay_state();
    }

    pub(crate) fn close_prompt_overlay(&mut self) {
        if self.prompt_overlay.is_none() {
            return;
        }
        self.prompt_overlay = None;
        self.pending_prompt_assembly_commit = true;
        self.sync_composer_height();
        self.present_pending_prompt_assembly_notice_if_ready();
    }

    /// `dismiss_prompt_overlay` 关闭 overlay 但不触发 commit，也不展示 pending notice。
    ///
    /// 用于 `BeginPromptAssemblyEdit` 失败等场景：从未成功进入 edit session，
    /// 不应触发后续 commit 请求，避免 commit 生命周期与真实编辑态错位。
    pub(crate) fn dismiss_prompt_overlay(&mut self) {
        if self.prompt_overlay.is_none() {
            return;
        }
        self.prompt_overlay = None;
        self.sync_composer_height();
    }

    pub(crate) fn take_prompt_assembly_commit_request(&mut self) -> bool {
        std::mem::take(&mut self.pending_prompt_assembly_commit)
    }
}
