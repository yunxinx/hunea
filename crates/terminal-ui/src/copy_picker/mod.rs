//! `/copy` 覆盖层的状态、输入与渲染。

use std::collections::BTreeSet;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton};
use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Clear, Paragraph, Widget},
};
use runtime_domain::session::{
    SessionTreePayload, SessionTreeRow, TranscriptReplayItem, TranscriptReplayRole,
};

use crate::{
    AppEffect, Model,
    display_width::display_width,
    fullscreen_list_chrome::{
        fullscreen_list_body_visible_offset_for_row, fullscreen_list_chrome_rects,
        fullscreen_list_page_size_for_height,
    },
    list_selection::{ListNavigationDirection, PagedSelection, row_index_by_id},
    overlay_key_result::OverlayKeyResult,
    render_frame::RenderFrame,
    session_tree_row_kind_view::{
        CopyableSessionTreeRowKind, TreeRowKindPrefixAlignment, session_tree_row_kind_is_copyable,
        session_tree_row_kind_label_style, session_tree_row_kind_prefix,
    },
    status_line::truncate_display_width_with_ellipsis,
    styled_text::render_line_with_full_width_background,
    theme::{
        approval_rejected_text_style, build_page_rule, command_accent_text_style,
        primary_text_style, secondary_text_style, subtle_rule_line, surface_text_style,
        tertiary_text_style,
    },
    toast::ToastSeverity,
    tool_result::ToolActivityRenderMode,
    transcript::{
        ReasoningRenderMode, latest_preview_offset as latest_copy_picker_preview_offset,
        preview_page_offset as copy_picker_preview_page_offset,
    },
    transcript_overlay::{
        TranscriptOverlayProgressStyle, TranscriptOverlayRenderOptions,
        render_transcript_overlay_view,
    },
    transcript_preview::TranscriptPreviewState,
};

const COPY_PICKER_BODY_HORIZONTAL_PADDING: usize = 2;
const COPY_PICKER_MARKER_WIDTH: usize = 2;
const COPY_PICKER_JOIN_SEPARATOR: &str = "\n\n\n";
const COPY_PICKER_EMPTY_TOAST: &str = "No user or assistant messages to copy";

mod input;
mod render;
mod state;

#[cfg(test)]
mod tests;

pub(crate) use state::CopyPickerState;
use state::{CopyPickerPreviewState, CopyPickerRow, CopyPickerTextFormat};
