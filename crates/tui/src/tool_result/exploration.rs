use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};
use unicode_width::UnicodeWidthStr;

use crate::{
    theme::{TerminalPalette, secondary_text_style},
    transcript::markdown_highlight::{HighlightChunk, wrap_highlight_chunks_soft},
};
use mo_core::session::{
    RuntimeToolActivity, RuntimeToolActivityContent, RuntimeToolActivityStatus, RuntimeToolKind,
};

use super::{
    TOOL_EXPLORATION_BRANCH_PREFIX, TOOL_EXPLORATION_CHILD_PREFIX,
    activity::{
        is_list_dir_tool_call, is_runtime_read_tool_activity, list_dir_tool_call_title_chunks,
        runtime_read_tool_activity_title_chunks, runtime_tool_activity_display_title,
        style_for_color,
    },
};

#[derive(Debug, Clone)]
pub(super) struct ExplorationDisplayLine {
    action: &'static str,
    chunks: Vec<HighlightChunk>,
}

pub(super) fn is_groupable_exploration_tool_call(call: &RuntimeToolActivity) -> bool {
    call.status != RuntimeToolActivityStatus::Failed
        && exploration_display_line_for_call(call).is_some()
}

pub(super) fn standalone_exploration_tool_call(
    calls: &[RuntimeToolActivity],
) -> Option<&RuntimeToolActivity> {
    match calls {
        [call] if is_groupable_exploration_tool_call(call) => Some(call),
        _ => None,
    }
}

pub(super) fn exploration_display_lines(
    calls: &[RuntimeToolActivity],
) -> Vec<ExplorationDisplayLine> {
    calls
        .iter()
        .filter(|call| call.status != RuntimeToolActivityStatus::Failed)
        .filter_map(exploration_display_line_for_call)
        .collect()
}

fn exploration_display_line_for_call(call: &RuntimeToolActivity) -> Option<ExplorationDisplayLine> {
    if is_runtime_read_tool_activity(call) {
        return Some(ExplorationDisplayLine {
            action: "Read",
            chunks: title_detail_chunks(runtime_read_tool_activity_title_chunks(call), "Read"),
        });
    }

    if is_list_dir_tool_call(call) {
        return Some(ExplorationDisplayLine {
            action: "List",
            chunks: title_detail_chunks(list_dir_tool_call_title_chunks(call), "List"),
        });
    }

    if call.kind == RuntimeToolKind::Search {
        return Some(ExplorationDisplayLine {
            action: "Search",
            chunks: search_tool_call_detail_chunks(call),
        });
    }

    None
}

fn title_detail_chunks(
    mut title_chunks: Vec<HighlightChunk>,
    action: &'static str,
) -> Vec<HighlightChunk> {
    if title_chunks
        .first()
        .is_some_and(|chunk| chunk.text.as_str() == action)
    {
        title_chunks.remove(0);
    }
    if let Some(first) = title_chunks.first_mut() {
        first.text = first.text.trim_start().to_string();
    }
    title_chunks.retain(|chunk| !chunk.text.is_empty());
    title_chunks
}

fn search_tool_call_detail_chunks(call: &RuntimeToolActivity) -> Vec<HighlightChunk> {
    let title = runtime_tool_activity_display_title(call);
    let detail = title
        .strip_prefix("Search:")
        .or_else(|| title.strip_prefix("Search "))
        .map(str::trim)
        .unwrap_or_else(|| title.trim());
    if detail.is_empty() || detail == "Search" {
        return Vec::new();
    }

    vec![HighlightChunk {
        text: detail.to_string(),
        style: Style::new(),
    }]
}

pub(super) fn coalesce_adjacent_target_display_lines(lines: &mut Vec<ExplorationDisplayLine>) {
    let mut coalesced: Vec<ExplorationDisplayLine> = Vec::with_capacity(lines.len());
    for line in lines.drain(..) {
        if is_target_list_action(line.action)
            && let Some(previous) = coalesced.last_mut()
            && previous.action == line.action
        {
            previous.chunks.push(HighlightChunk {
                text: ", ".to_string(),
                style: Style::new(),
            });
            previous.chunks.extend(line.chunks);
            continue;
        }

        coalesced.push(line);
    }

    *lines = coalesced;
}

fn is_target_list_action(action: &str) -> bool {
    matches!(action, "Read" | "List")
}

pub(super) fn failed_tool_call_detail_text(call: &RuntimeToolActivity) -> String {
    let reason = call
        .content
        .iter()
        .find_map(|content| match content {
            RuntimeToolActivityContent::Text(text) => text.lines().find_map(|line| {
                let line = line.trim();
                (!line.is_empty()).then_some(line)
            }),
            _ => None,
        })
        .unwrap_or("Failed");

    let reason = reason
        .strip_prefix("Failed:")
        .map(str::trim)
        .unwrap_or(reason);
    let reason = compact_failed_tool_call_reason(reason);
    if reason.is_empty() {
        "Failed".to_string()
    } else {
        format!("Failed: {reason}")
    }
}

fn compact_failed_tool_call_reason(reason: &str) -> &str {
    let Some((message, target)) = reason.split_once(": ") else {
        return reason;
    };
    if target.trim().is_empty() {
        return reason;
    }

    if [
        "File not found",
        "Directory not found",
        "Path is a directory",
        "Path is not a regular file",
        "Path is a file",
        "Path is outside workspace",
        "Could not inspect path",
        "Could not read file",
        "Could not list directory",
    ]
    .contains(&message)
    {
        return message;
    }

    reason
}

pub(super) fn wrap_failed_exploration_detail_line(
    detail_text: &str,
    width: usize,
    palette: TerminalPalette,
) -> Vec<Line<'static>> {
    let prefix_width = UnicodeWidthStr::width(TOOL_EXPLORATION_BRANCH_PREFIX);
    let content_width = width.saturating_sub(prefix_width).max(1);
    let wrapped = wrap_highlight_chunks_soft(
        &[vec![HighlightChunk {
            text: detail_text.to_string(),
            style: secondary_text_style(palette),
        }]],
        content_width,
    );

    if wrapped.is_empty() {
        return vec![Line::from(vec![Span::styled(
            TOOL_EXPLORATION_BRANCH_PREFIX,
            style_for_color(palette.tertiary),
        )])];
    }

    wrapped
        .into_iter()
        .enumerate()
        .map(|(index, content_spans)| {
            let prefix = if index == 0 {
                TOOL_EXPLORATION_BRANCH_PREFIX
            } else {
                TOOL_EXPLORATION_CHILD_PREFIX
            };
            let mut spans = Vec::with_capacity(content_spans.len() + 1);
            spans.push(Span::styled(prefix, style_for_color(palette.tertiary)));
            spans.extend(content_spans);
            Line::from(spans)
        })
        .collect()
}

pub(super) fn wrap_exploration_display_line(
    display_line: &ExplorationDisplayLine,
    line_prefix: &'static str,
    continuation_prefix: &'static str,
    width: usize,
    palette: TerminalPalette,
) -> Vec<Line<'static>> {
    let prefix_width = UnicodeWidthStr::width(line_prefix);
    let content_width = width.saturating_sub(prefix_width).max(1);
    let mut chunks = vec![HighlightChunk {
        text: display_line.action.to_string(),
        style: style_for_color(palette.command_accent).add_modifier(Modifier::BOLD),
    }];
    if !display_line.chunks.is_empty() {
        chunks.push(HighlightChunk {
            text: " ".to_string(),
            style: Style::new(),
        });
        chunks.extend(display_line.chunks.clone());
    }

    let wrapped = wrap_highlight_chunks_soft(&[chunks], content_width);
    if wrapped.is_empty() {
        return vec![Line::from(vec![Span::styled(
            line_prefix,
            style_for_color(palette.tertiary),
        )])];
    }

    wrapped
        .into_iter()
        .enumerate()
        .map(|(index, content_spans)| {
            let prefix = if index == 0 {
                line_prefix
            } else {
                continuation_prefix
            };
            let mut spans = Vec::with_capacity(content_spans.len() + 1);
            spans.push(Span::styled(prefix, style_for_color(palette.tertiary)));
            spans.extend(content_spans);
            Line::from(spans)
        })
        .collect()
}
