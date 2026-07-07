use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton};
use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Clear, Paragraph, Widget},
};
use runtime_domain::session::{
    SessionBranchTreeNode, SessionBranchTreePayload, SessionLoadRequestId, SessionTreeBranchChoice,
    SessionTreePayload, SessionTreeRow, SessionTreeRowKind,
};

use crate::{
    AppEffect, Model,
    display_width::display_width,
    fullscreen_list_chrome::{
        FULLSCREEN_LIST_CHROME_HEIGHT as ENTRY_TREE_CHROME_HEIGHT,
        FULLSCREEN_LIST_HEADER_HEIGHT as ENTRY_TREE_HEADER_HEIGHT,
        FULLSCREEN_LIST_HEADER_RULE_HEIGHT as ENTRY_TREE_HEADER_RULE_HEIGHT,
        fullscreen_list_body_visible_offset_for_row, fullscreen_list_chrome_rects,
        fullscreen_list_page_size_for_height,
    },
    list_selection::ListNavigationDirection,
    overlay_input_result::OverlayInputResult,
    render_frame::RenderFrame,
    session_tree_preview_replay::SessionTreePreviewReplay,
    session_tree_row_kind_view::{
        SESSION_TREE_ROW_KIND_PREFIX_WIDTH, SESSION_TREE_ROW_KIND_WIDTH,
        TreeRowKindPrefixAlignment, session_tree_row_kind_label_style,
        session_tree_row_kind_prefix,
    },
    status_line::truncate_display_width_with_ellipsis,
    theme::{
        TerminalPalette, accent_text_style, approval_rejected_text_style,
        command_accent_text_style, primary_text_style, secondary_text_style, subtle_rule_line,
        system_error_text_style, table_header_text_style, tertiary_text_style,
    },
    time::current_unix_timestamp_ms,
    toast::ToastSeverity,
    tool_result::ToolActivityRenderMode,
    transcript::{ReasoningRenderMode, preview_page_offset as entry_tree_preview_page_offset},
    transcript_overlay::{
        TranscriptOverlayProgressStyle, TranscriptOverlayRenderOptions,
        render_transcript_overlay_view,
    },
};

#[cfg(test)]
mod tests;

const BRANCH_TREE_ROOT_HEIGHT: u16 = 1;
const BRANCH_TREE_SUMMARY_GAP_HEIGHT: u16 = 1;
const BRANCH_TREE_SUMMARY_HEIGHT: u16 = 1;
pub(crate) const BRANCH_PICKER_LIST_ROWS_MIN: u16 = 3;
pub(crate) const BRANCH_PICKER_LIST_ROWS_MAX: u16 = 14;
pub(crate) const BRANCH_PICKER_LIST_ROWS_DEFAULT: u16 = 7;
const ENTRY_TREE_BODY_HORIZONTAL_PADDING: usize = 2;
const ENTRY_TREE_KIND_WIDTH: usize = SESSION_TREE_ROW_KIND_WIDTH;
const ENTRY_TREE_KIND_PREFIX_WIDTH: usize = SESSION_TREE_ROW_KIND_PREFIX_WIDTH;
const ENTRY_TREE_GRAPH_MAX_WIDTH: usize = 12;
const ENTRY_TREE_GRAPH_MIN_WIDTH: usize = 2;
const ENTRY_TREE_GRAPH_FLAT_WIDTH: usize = 2;
const ENTRY_TREE_GRAPH_LANE_WIDTH: usize = 3;
const ENTRY_TREE_GRAPH_CELL_WIDTH: usize = 3;
const ENTRY_TREE_MIN_SUMMARY_WIDTH: usize = 22;
const BRANCH_PICKER_CHROME_HEIGHT: u16 = 3;
const BRANCH_PICKER_ITEM_TOP_OFFSET: u16 = 2;
const BRANCH_PICKER_METADATA_LEFT_PADDING: usize = 2;
const BRANCH_PICKER_RIGHT_PADDING: u16 = 2;
const BRANCH_PICKER_MSGS_WIDTH: usize = 4;
const BRANCH_PICKER_TIME_WIDTH: usize = 7;

mod branch_tree;
mod graph;
mod input;
mod render;
mod state;

use branch_tree::{branch_tree_connector_prefixes, branch_tree_display_order_nodes};
use graph::{EntryTreeGraphLine, entry_tree_graph_lines, entry_tree_graph_span_style};
pub(crate) use state::EntryTreeState;
use state::{
    EntryTreeBranchPickerState, EntryTreeBranchPreviewMetadata, EntryTreeBranchPreviewSource,
    EntryTreeBranchPreviewState, EntryTreeBranchTreeState, EntryTreePreviewState,
};

fn entry_tree_branch_tree_page_size_for_height(height: u16) -> usize {
    usize::from(
        height
            .saturating_sub(ENTRY_TREE_CHROME_HEIGHT)
            .saturating_sub(BRANCH_TREE_ROOT_HEIGHT)
            .saturating_sub(BRANCH_TREE_SUMMARY_GAP_HEIGHT)
            .saturating_sub(BRANCH_TREE_SUMMARY_HEIGHT),
    )
    .max(1)
}

fn entry_tree_branch_picker_area_for_state(
    state: &EntryTreeState,
    area: Rect,
    list_rows: usize,
) -> Rect {
    let popup_height = u16::try_from(list_rows)
        .unwrap_or(u16::MAX)
        .saturating_add(BRANCH_PICKER_CHROME_HEIGHT)
        .min(area.height);
    let page_size = entry_tree_page_size_for_height(area.height);
    let selected_visible_offset = state
        .page_indices(page_size)
        .position(|row_index| row_index == state.selected)
        .unwrap_or_default();
    let anchor_y = area
        .y
        .saturating_add(ENTRY_TREE_HEADER_HEIGHT + ENTRY_TREE_HEADER_RULE_HEIGHT)
        .saturating_add(u16::try_from(selected_visible_offset).unwrap_or(u16::MAX));

    entry_tree_branch_picker_popup_area(area, anchor_y, popup_height)
}

fn entry_tree_branch_picker_popup_area(area: Rect, anchor_y: u16, popup_height: u16) -> Rect {
    let popup_height = popup_height.min(area.height);
    let area_bottom = area.y.saturating_add(area.height);
    let below_y = anchor_y.saturating_add(1);
    let popup_y = if below_y.saturating_add(popup_height) <= area_bottom {
        below_y
    } else if anchor_y >= area.y.saturating_add(popup_height) {
        anchor_y - popup_height
    } else {
        area_bottom.saturating_sub(popup_height).max(area.y)
    };

    Rect::new(area.x, popup_y, area.width, popup_height)
}

const fn rect_contains(area: Rect, column: u16, row: u16) -> bool {
    column >= area.x && column < area.right() && row >= area.y && row < area.bottom()
}

pub(crate) fn entry_tree_page_size_for_height(height: u16) -> usize {
    fullscreen_list_page_size_for_height(height)
}

fn is_entry_tree_branch_tree_shortcut(key: KeyEvent) -> bool {
    key.code == KeyCode::Char('A')
        && !key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
}
