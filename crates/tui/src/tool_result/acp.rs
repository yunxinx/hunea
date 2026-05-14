use std::{collections::BTreeMap, time::Instant};

use ratatui::style::{Color, Modifier, Style};

use crate::{
    acp_tool_preview::{
        acp_display_path, acp_write_tool_call_target, should_collapse_acp_write_tool_call,
    },
    theme::TerminalPalette,
    transcript::markdown_highlight::HighlightChunk,
};
use mo_core::session::{
    RuntimeTerminalSnapshot, RuntimeToolActivity, RuntimeToolActivityContent,
    RuntimeToolActivityLocation, RuntimeToolActivityStatus, RuntimeToolKind,
};

use super::{
    TOOL_ACTIVITY_ACTIVE_MARKER_BLINK_INTERVAL, TOOL_ACTIVITY_COMPACT_EDGE_LINES,
    TOOL_ACTIVITY_DIFF_LINE_NUMBER_WIDTH, TOOL_ACTIVITY_TRANSCRIPT_HINT, ToolActivityRenderMode,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum AcpToolCallDetailBlock {
    Text(Vec<String>),
    SecondaryText(Vec<String>),
    Diff(Vec<AcpDiffDetailLine>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct AcpDiffDetailLine {
    pub(super) line_number: Option<usize>,
    pub(super) text: String,
    pub(super) kind: AcpDiffDetailLineKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AcpDiffDetailLineKind {
    Context,
    Insert,
    Delete,
    Separator,
    Omitted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AcpDiffChangeKind {
    Added,
    Edited,
    Deleted,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AcpDiffSummary {
    path: String,
    added: usize,
    removed: usize,
    change_kind: AcpDiffChangeKind,
}

pub(super) fn acp_tool_call_detail_blocks(
    call: &RuntimeToolActivity,
    render_mode: ToolActivityRenderMode,
    permission_waiting: bool,
    terminal_snapshots: &BTreeMap<String, RuntimeTerminalSnapshot>,
) -> Vec<AcpToolCallDetailBlock> {
    if should_collapse_acp_read_tool_call(call) {
        return Vec::new();
    }
    if should_collapse_acp_write_tool_call(call) {
        return Vec::new();
    }

    if is_execute_like_tool_call(call) {
        return execute_tool_call_detail_blocks(
            call,
            render_mode,
            permission_waiting,
            terminal_snapshots,
        );
    }

    let mut blocks = Vec::new();

    for content in &call.content {
        blocks.extend(acp_tool_call_content_blocks(
            content,
            render_mode,
            terminal_snapshots,
        ));
    }
    if let Some(raw_input) = call.raw_input.as_ref().and_then(|raw| raw.display_text()) {
        blocks.push(AcpToolCallDetailBlock::Text(labeled_detail_block(
            "Input",
            &raw_input,
            render_mode,
        )));
    }
    if let Some(raw_output) = call.raw_output.as_ref().and_then(|raw| raw.display_text()) {
        blocks.push(AcpToolCallDetailBlock::SecondaryText(
            truncate_detail_block(text_lines(&raw_output), render_mode),
        ));
    }

    blocks
}

fn execute_tool_call_detail_blocks(
    call: &RuntimeToolActivity,
    render_mode: ToolActivityRenderMode,
    permission_waiting: bool,
    terminal_snapshots: &BTreeMap<String, RuntimeTerminalSnapshot>,
) -> Vec<AcpToolCallDetailBlock> {
    if should_defer_active_execute_details(call, permission_waiting) {
        let terminal_blocks = active_execute_terminal_blocks(call, render_mode, terminal_snapshots);
        if !terminal_blocks.is_empty() {
            return terminal_blocks;
        }
        return vec![AcpToolCallDetailBlock::SecondaryText(vec![
            "Waiting...".to_string(),
        ])];
    }

    if let Some(raw_output) = call.raw_output.as_ref().and_then(|raw| raw.display_text()) {
        return vec![AcpToolCallDetailBlock::SecondaryText(
            truncate_detail_block(text_lines(&raw_output), render_mode),
        )];
    }

    let mut blocks = Vec::new();
    for content in &call.content {
        if should_hide_execute_text_content(content) {
            continue;
        }
        blocks.extend(acp_tool_call_content_blocks(
            content,
            render_mode,
            terminal_snapshots,
        ));
    }

    blocks
}

fn should_defer_active_execute_details(
    call: &RuntimeToolActivity,
    permission_waiting: bool,
) -> bool {
    is_active_tool_call_status(call.status)
        && (permission_waiting || is_execute_like_tool_call(call))
}

fn active_execute_terminal_blocks(
    call: &RuntimeToolActivity,
    render_mode: ToolActivityRenderMode,
    terminal_snapshots: &BTreeMap<String, RuntimeTerminalSnapshot>,
) -> Vec<AcpToolCallDetailBlock> {
    call.content
        .iter()
        .filter(|content| matches!(content, RuntimeToolActivityContent::Terminal { .. }))
        .flat_map(|content| acp_tool_call_content_blocks(content, render_mode, terminal_snapshots))
        .collect()
}

fn is_active_tool_call_status(status: RuntimeToolActivityStatus) -> bool {
    matches!(
        status,
        RuntimeToolActivityStatus::Pending | RuntimeToolActivityStatus::InProgress
    )
}

fn is_execute_like_tool_call(call: &RuntimeToolActivity) -> bool {
    call.kind == RuntimeToolKind::Execute
        || call.title.trim_start().starts_with("Shell:")
        || call.title.trim_start().starts_with("Run ")
        || call
            .raw_input
            .as_ref()
            .and_then(|raw_input| raw_input.string_field(&["command", "cmd"]))
            .is_some()
}

fn should_hide_execute_text_content(content: &RuntimeToolActivityContent) -> bool {
    matches!(
        content,
        RuntimeToolActivityContent::Text(text) if is_execute_protocol_copy_text(text)
    )
}

fn is_execute_protocol_copy_text(text: &str) -> bool {
    let normalized = text.split_whitespace().collect::<Vec<_>>().join(" ");
    normalized.starts_with("Requesting approval to perform:")
        || (normalized.starts_with("The tool call is rejected by the user.")
            && normalized.contains("Stop what you are doing")
            && normalized.contains("wait for the user to tell you how to proceed"))
}

fn acp_tool_call_content_blocks(
    content: &RuntimeToolActivityContent,
    render_mode: ToolActivityRenderMode,
    terminal_snapshots: &BTreeMap<String, RuntimeTerminalSnapshot>,
) -> Vec<AcpToolCallDetailBlock> {
    match content {
        RuntimeToolActivityContent::Text(text) => {
            vec![AcpToolCallDetailBlock::Text(truncate_detail_block(
                text_lines(text),
                render_mode,
            ))]
        }
        RuntimeToolActivityContent::Image { mime_type, uri } => {
            vec![AcpToolCallDetailBlock::Text(vec![match uri {
                Some(uri) => format!("Image: {mime_type} {uri}"),
                None => format!("Image: {mime_type}"),
            }])]
        }
        RuntimeToolActivityContent::Audio { mime_type } => {
            vec![AcpToolCallDetailBlock::Text(vec![format!(
                "Audio: {mime_type}"
            )])]
        }
        RuntimeToolActivityContent::ResourceLink { uri, name, title } => {
            vec![AcpToolCallDetailBlock::Text(vec![match title {
                Some(title) if !title.is_empty() => {
                    format!("Resource: {title} ({name}) {uri}")
                }
                _ => format!("Resource: {name} {uri}"),
            }])]
        }
        RuntimeToolActivityContent::Resource {
            uri,
            mime_type,
            text,
        } => {
            let label = match mime_type {
                Some(mime_type) => format!("Resource: {uri} ({mime_type})"),
                None => format!("Resource: {uri}"),
            };
            let Some(text) = text else {
                return vec![AcpToolCallDetailBlock::Text(vec![label])];
            };
            let mut lines = vec![label];
            lines.extend(text_lines(text));
            vec![AcpToolCallDetailBlock::Text(truncate_detail_block(
                lines,
                render_mode,
            ))]
        }
        RuntimeToolActivityContent::Diff {
            path: _,
            old_text,
            new_text,
        } => vec![AcpToolCallDetailBlock::Diff(truncate_diff_detail_block(
            diff_detail_lines(old_text.as_deref(), new_text),
            render_mode,
        ))],
        RuntimeToolActivityContent::Terminal { terminal_id } => {
            vec![AcpToolCallDetailBlock::SecondaryText(
                terminal_detail_lines(terminal_snapshots.get(terminal_id), render_mode),
            )]
        }
        RuntimeToolActivityContent::Unknown(label) => {
            vec![AcpToolCallDetailBlock::Text(vec![format!(
                "Unknown content: {label}"
            )])]
        }
    }
}

fn labeled_detail_block(
    label: &str,
    content: &str,
    render_mode: ToolActivityRenderMode,
) -> Vec<String> {
    let mut lines = text_lines(content);
    if lines.is_empty() {
        return vec![format!("{label}:")];
    }

    lines[0] = format!("{label}: {}", lines[0]);
    truncate_detail_block(lines, render_mode)
}

fn terminal_detail_lines(
    snapshot: Option<&RuntimeTerminalSnapshot>,
    render_mode: ToolActivityRenderMode,
) -> Vec<String> {
    let Some(snapshot) = snapshot else {
        return vec!["Waiting...".to_string()];
    };

    let mut lines = Vec::new();
    if snapshot.exit_status.is_none() && !snapshot.released {
        lines.push("Running...".to_string());
    }
    if snapshot.truncated {
        lines.push("... output truncated ...".to_string());
    }
    if !snapshot.output.is_empty() {
        lines.extend(text_lines(&snapshot.output));
    }
    if let Some(exit_status) = snapshot.exit_status.as_ref() {
        lines.push(
            match (exit_status.exit_code, exit_status.signal.as_deref()) {
                (Some(code), _) => format!("Exited with code {code}"),
                (None, Some(signal)) => format!("Terminated by {signal}"),
                (None, None) => "Exited".to_string(),
            },
        );
    }
    if lines.is_empty() {
        lines.push("Waiting...".to_string());
    }

    truncate_detail_block(lines, render_mode)
}

fn truncate_detail_block(lines: Vec<String>, render_mode: ToolActivityRenderMode) -> Vec<String> {
    if render_mode == ToolActivityRenderMode::Detailed {
        return lines;
    }
    let edge = TOOL_ACTIVITY_COMPACT_EDGE_LINES;
    let limit = edge.saturating_mul(2);
    if lines.len() <= limit {
        return lines;
    }

    let omitted = lines.len().saturating_sub(limit);
    let mut truncated = Vec::with_capacity(limit + 1);
    truncated.extend(lines.iter().take(edge).cloned());
    truncated.push(format!(
        "… +{omitted} lines ({TOOL_ACTIVITY_TRANSCRIPT_HINT})"
    ));
    truncated.extend(lines.iter().skip(lines.len().saturating_sub(edge)).cloned());
    truncated
}

fn truncate_diff_detail_block(
    lines: Vec<AcpDiffDetailLine>,
    render_mode: ToolActivityRenderMode,
) -> Vec<AcpDiffDetailLine> {
    if render_mode == ToolActivityRenderMode::Detailed {
        return lines;
    }
    let edge = TOOL_ACTIVITY_COMPACT_EDGE_LINES;
    let limit = edge.saturating_mul(2);
    if lines.len() <= limit {
        return lines;
    }

    let omitted = lines.len().saturating_sub(limit);
    let mut truncated = Vec::with_capacity(limit + 1);
    truncated.extend(lines.iter().take(edge).cloned());
    truncated.push(AcpDiffDetailLine {
        line_number: None,
        text: format!("⋮ +{omitted} lines ({TOOL_ACTIVITY_TRANSCRIPT_HINT})"),
        kind: AcpDiffDetailLineKind::Omitted,
    });
    truncated.extend(lines.iter().skip(lines.len().saturating_sub(edge)).cloned());
    truncated
}

fn text_lines(text: &str) -> Vec<String> {
    let lines: Vec<String> = text.lines().map(str::to_string).collect();
    if lines.is_empty() {
        vec![String::new()]
    } else {
        lines
    }
}

fn diff_detail_lines(old_text: Option<&str>, new_text: &str) -> Vec<AcpDiffDetailLine> {
    let Some(old_text) = old_text else {
        return text_lines(new_text)
            .into_iter()
            .enumerate()
            .map(|(index, line)| AcpDiffDetailLine {
                line_number: Some(index + 1),
                text: line,
                kind: AcpDiffDetailLineKind::Insert,
            })
            .collect();
    };

    let patch = diffy::create_patch(old_text, new_text);
    let mut lines = Vec::new();
    for (hunk_index, hunk) in patch.hunks().iter().enumerate() {
        if hunk_index > 0 {
            lines.push(AcpDiffDetailLine {
                line_number: None,
                text: "⋮".to_string(),
                kind: AcpDiffDetailLineKind::Separator,
            });
        }

        let mut old_line = hunk.old_range().start();
        let mut new_line = hunk.new_range().start();
        for line in hunk.lines() {
            match line {
                diffy::Line::Insert(text) => {
                    lines.push(AcpDiffDetailLine {
                        line_number: Some(new_line),
                        text: text.trim_end_matches('\n').to_string(),
                        kind: AcpDiffDetailLineKind::Insert,
                    });
                    new_line += 1;
                }
                diffy::Line::Delete(text) => {
                    lines.push(AcpDiffDetailLine {
                        line_number: Some(old_line),
                        text: text.trim_end_matches('\n').to_string(),
                        kind: AcpDiffDetailLineKind::Delete,
                    });
                    old_line += 1;
                }
                diffy::Line::Context(text) => {
                    lines.push(AcpDiffDetailLine {
                        line_number: Some(new_line),
                        text: text.trim_end_matches('\n').to_string(),
                        kind: AcpDiffDetailLineKind::Context,
                    });
                    old_line += 1;
                    new_line += 1;
                }
            }
        }
    }

    lines
}

fn line_count(text: &str) -> usize {
    if text.is_empty() {
        0
    } else {
        text.lines().count()
    }
}

pub(super) fn acp_tool_call_diff_header_chunks(
    call: &RuntimeToolActivity,
    palette: TerminalPalette,
) -> Option<Vec<HighlightChunk>> {
    let summaries = call
        .content
        .iter()
        .filter_map(acp_diff_summary)
        .collect::<Vec<_>>();
    if summaries.is_empty() {
        return None;
    }

    let title_style = Style::new().add_modifier(Modifier::BOLD);
    let mut chunks = Vec::new();
    if let [summary] = summaries.as_slice() {
        chunks.push(HighlightChunk {
            text: acp_diff_change_kind_label(summary.change_kind).to_string(),
            style: title_style,
        });
        chunks.push(HighlightChunk {
            text: format!(" {} ", summary.path),
            style: Style::new(),
        });
        chunks.extend(acp_diff_count_chunks(
            summary.added,
            summary.removed,
            palette,
        ));
        return Some(chunks);
    }

    let added = summaries.iter().map(|summary| summary.added).sum::<usize>();
    let removed = summaries
        .iter()
        .map(|summary| summary.removed)
        .sum::<usize>();
    chunks.push(HighlightChunk {
        text: "Edited".to_string(),
        style: title_style,
    });
    chunks.push(HighlightChunk {
        text: format!(" {} files ", summaries.len()),
        style: Style::new(),
    });
    chunks.extend(acp_diff_count_chunks(added, removed, palette));
    Some(chunks)
}

fn acp_diff_summary(content: &RuntimeToolActivityContent) -> Option<AcpDiffSummary> {
    let RuntimeToolActivityContent::Diff {
        path,
        old_text,
        new_text,
    } = content
    else {
        return None;
    };
    let old_line_count = old_text.as_deref().map(line_count).unwrap_or(0);
    let new_line_count = line_count(new_text);
    let (added, removed) = acp_diff_added_removed(old_text.as_deref(), new_text);
    let change_kind = if old_line_count == 0 && new_line_count > 0 {
        AcpDiffChangeKind::Added
    } else if old_line_count > 0 && new_line_count == 0 {
        AcpDiffChangeKind::Deleted
    } else {
        AcpDiffChangeKind::Edited
    };

    Some(AcpDiffSummary {
        path: acp_display_path(path),
        added,
        removed,
        change_kind,
    })
}

fn acp_diff_added_removed(old_text: Option<&str>, new_text: &str) -> (usize, usize) {
    let Some(old_text) = old_text else {
        return (line_count(new_text), 0);
    };

    diffy::create_patch(old_text, new_text)
        .hunks()
        .iter()
        .flat_map(|hunk| hunk.lines())
        .fold((0, 0), |(added, removed), line| match line {
            diffy::Line::Insert(_) => (added + 1, removed),
            diffy::Line::Delete(_) => (added, removed + 1),
            diffy::Line::Context(_) => (added, removed),
        })
}

fn acp_diff_change_kind_label(kind: AcpDiffChangeKind) -> &'static str {
    match kind {
        AcpDiffChangeKind::Added => "Added",
        AcpDiffChangeKind::Edited => "Edited",
        AcpDiffChangeKind::Deleted => "Deleted",
    }
}

fn acp_diff_count_chunks(
    added: usize,
    removed: usize,
    palette: TerminalPalette,
) -> Vec<HighlightChunk> {
    vec![
        HighlightChunk {
            text: "(".to_string(),
            style: Style::new(),
        },
        HighlightChunk {
            text: format!("+{added}"),
            style: style_for_color(palette.quote),
        },
        HighlightChunk {
            text: " ".to_string(),
            style: Style::new(),
        },
        HighlightChunk {
            text: format!("-{removed}"),
            style: style_for_color(palette.system_error),
        },
        HighlightChunk {
            text: ")".to_string(),
            style: Style::new(),
        },
    ]
}

pub(super) fn acp_tool_call_has_diff_content(call: &RuntimeToolActivity) -> bool {
    call.content
        .iter()
        .any(|content| matches!(content, RuntimeToolActivityContent::Diff { .. }))
}

pub(super) fn acp_diff_line_prefix(
    line_number: Option<usize>,
    kind: AcpDiffDetailLineKind,
) -> String {
    let sign = match kind {
        AcpDiffDetailLineKind::Insert => "+",
        AcpDiffDetailLineKind::Delete => "-",
        AcpDiffDetailLineKind::Context
        | AcpDiffDetailLineKind::Separator
        | AcpDiffDetailLineKind::Omitted => " ",
    };
    match line_number {
        Some(line_number) => format!(
            "{line_number:>width$} {sign}  ",
            width = TOOL_ACTIVITY_DIFF_LINE_NUMBER_WIDTH
        ),
        None => " ".repeat(TOOL_ACTIVITY_DIFF_LINE_NUMBER_WIDTH.saturating_sub(1)),
    }
}

pub(super) fn acp_tool_call_location_suffix(
    locations: &[RuntimeToolActivityLocation],
) -> Option<String> {
    if locations.is_empty() {
        return None;
    }

    Some(
        locations
            .iter()
            .map(|location| match location.line {
                Some(line) => format!("{}:{line}", location.path),
                None => location.path.clone(),
            })
            .collect::<Vec<_>>()
            .join(", "),
    )
}

pub(super) fn acp_tool_call_display_title(call: &RuntimeToolActivity) -> String {
    let title = call.title.trim();
    // ACP 标题常带有 `Shell:` 一类传输前缀，紧凑头部只显示真正命令。
    let title = title
        .strip_prefix("Shell:")
        .map(str::trim_start)
        .unwrap_or(title);

    if title.is_empty() {
        acp_tool_kind_label(call.kind).to_string()
    } else {
        title.to_string()
    }
}

pub(super) fn acp_read_tool_call_title_chunks(call: &RuntimeToolActivity) -> Vec<HighlightChunk> {
    let title_style = Style::new().add_modifier(Modifier::BOLD);
    let Some(target) = acp_read_tool_call_target(call) else {
        return vec![HighlightChunk {
            text: "Read".to_string(),
            style: title_style,
        }];
    };

    vec![
        HighlightChunk {
            text: "Read".to_string(),
            style: title_style,
        },
        HighlightChunk {
            text: format!(" {target}"),
            style: Style::new(),
        },
    ]
}

pub(super) fn acp_write_tool_call_title_chunks(call: &RuntimeToolActivity) -> Vec<HighlightChunk> {
    let title_style = Style::new().add_modifier(Modifier::BOLD);
    let Some(target) = acp_write_tool_call_target(call) else {
        return vec![HighlightChunk {
            text: "Write".to_string(),
            style: title_style,
        }];
    };

    vec![
        HighlightChunk {
            text: "Write".to_string(),
            style: title_style,
        },
        HighlightChunk {
            text: format!(" {target}"),
            style: Style::new(),
        },
    ]
}

fn acp_read_tool_call_target(call: &RuntimeToolActivity) -> Option<String> {
    if call.kind == RuntimeToolKind::Read
        && let [location] = call.locations.as_slice()
        && !location.path.trim().is_empty()
    {
        return Some(acp_display_path(location.path.trim()));
    }

    acp_read_tool_call_title_target(&call.title)
}

fn acp_read_tool_call_title_target(title: &str) -> Option<String> {
    acp_tool_call_title_target(title, &["ReadFile:", "Read File:", "Read:", "Read "])
}

fn acp_tool_call_title_target(title: &str, prefixes: &[&str]) -> Option<String> {
    let title = title.trim();
    prefixes.iter().find_map(|prefix| {
        title.strip_prefix(prefix).and_then(|target| {
            let target = target.trim();
            (!target.is_empty()).then(|| acp_display_path(target))
        })
    })
}

pub(super) fn is_acp_read_tool_call(call: &RuntimeToolActivity) -> bool {
    call.kind == RuntimeToolKind::Read || acp_read_tool_call_title_target(&call.title).is_some()
}

fn should_collapse_acp_read_tool_call(call: &RuntimeToolActivity) -> bool {
    is_acp_read_tool_call(call) && call.status != RuntimeToolActivityStatus::Failed
}

pub(super) fn acp_tool_call_diff_line_style(
    kind: AcpDiffDetailLineKind,
    palette: TerminalPalette,
) -> Style {
    match kind {
        AcpDiffDetailLineKind::Context => Style::new(),
        AcpDiffDetailLineKind::Insert => Style::new().fg(palette.quote),
        AcpDiffDetailLineKind::Delete => Style::new().fg(palette.system_error),
        AcpDiffDetailLineKind::Separator | AcpDiffDetailLineKind::Omitted => {
            Style::new().fg(palette.tertiary)
        }
    }
}

pub(super) fn acp_tool_call_diff_row_style(
    kind: AcpDiffDetailLineKind,
    palette: TerminalPalette,
) -> Style {
    acp_tool_call_diff_background(kind, palette)
        .map(|background| Style::new().bg(background))
        .unwrap_or_default()
}

fn acp_tool_call_diff_background(
    kind: AcpDiffDetailLineKind,
    palette: TerminalPalette,
) -> Option<Color> {
    match kind {
        AcpDiffDetailLineKind::Context => None,
        AcpDiffDetailLineKind::Insert => acp_tool_call_diff_tint(palette, true),
        AcpDiffDetailLineKind::Delete => acp_tool_call_diff_tint(palette, false),
        AcpDiffDetailLineKind::Separator | AcpDiffDetailLineKind::Omitted => None,
    }
}

fn acp_tool_call_diff_tint(palette: TerminalPalette, is_insert: bool) -> Option<Color> {
    let _surface = palette.surface?;

    let has_dark_background = palette_main_is_light_text(palette);
    Some(match (has_dark_background, is_insert) {
        (true, true) => Color::Rgb(38, 58, 44),
        (true, false) => Color::Rgb(64, 44, 44),
        (false, true) => Color::Rgb(228, 242, 230),
        (false, false) => Color::Rgb(247, 229, 229),
    })
}

fn palette_main_is_light_text(palette: TerminalPalette) -> bool {
    match palette.main {
        Color::Rgb(red, green, blue) => {
            let luminance =
                0.299 * f32::from(red) + 0.587 * f32::from(green) + 0.114 * f32::from(blue);
            luminance > 128.0
        }
        Color::Reset => true,
        _ => true,
    }
}

fn acp_tool_kind_label(kind: RuntimeToolKind) -> &'static str {
    match kind {
        RuntimeToolKind::Read => "Read",
        RuntimeToolKind::Edit => "Edit",
        RuntimeToolKind::Delete => "Delete",
        RuntimeToolKind::Move => "Move",
        RuntimeToolKind::Search => "Search",
        RuntimeToolKind::Execute => "Execute",
        RuntimeToolKind::Think => "Think",
        RuntimeToolKind::Fetch => "Fetch",
        RuntimeToolKind::SwitchMode => "SwitchMode",
        RuntimeToolKind::Other => "Other",
    }
}

pub(super) fn acp_tool_call_status_color(
    status: RuntimeToolActivityStatus,
    palette: TerminalPalette,
) -> Color {
    match status {
        RuntimeToolActivityStatus::Pending => palette.tertiary,
        RuntimeToolActivityStatus::InProgress => palette.accent,
        RuntimeToolActivityStatus::Completed => palette.quote,
        RuntimeToolActivityStatus::Failed => palette.system_error,
    }
}

pub(super) fn style_for_color(color: Color) -> Style {
    if color == Color::Reset {
        Style::new()
    } else {
        Style::new().fg(color)
    }
}

fn active_marker_frame_index(started_at: Instant, now: Instant) -> usize {
    let interval_ms = TOOL_ACTIVITY_ACTIVE_MARKER_BLINK_INTERVAL
        .as_millis()
        .max(1);
    (now.saturating_duration_since(started_at).as_millis() / interval_ms) as usize
}

pub(super) fn active_marker_visible_at(started_at: Instant, now: Instant) -> bool {
    active_marker_frame_index(started_at, now).is_multiple_of(2)
}

pub(super) fn acp_tool_call_content_byte_len(content: &RuntimeToolActivityContent) -> usize {
    match content {
        RuntimeToolActivityContent::Text(text) => text.len(),
        RuntimeToolActivityContent::Image { mime_type, uri } => {
            mime_type.len() + uri.as_deref().map(str::len).unwrap_or(0)
        }
        RuntimeToolActivityContent::Audio { mime_type } => mime_type.len(),
        RuntimeToolActivityContent::ResourceLink { uri, name, title } => {
            uri.len() + name.len() + title.as_deref().map(str::len).unwrap_or(0)
        }
        RuntimeToolActivityContent::Resource {
            uri,
            mime_type,
            text,
        } => {
            uri.len()
                + mime_type.as_deref().map(str::len).unwrap_or(0)
                + text.as_deref().map(str::len).unwrap_or(0)
        }
        RuntimeToolActivityContent::Diff {
            path,
            old_text,
            new_text,
        } => path.len() + old_text.as_deref().map(str::len).unwrap_or(0) + new_text.len(),
        RuntimeToolActivityContent::Terminal { terminal_id } => terminal_id.len(),
        RuntimeToolActivityContent::Unknown(label) => label.len(),
    }
}
