use std::{
    collections::BTreeMap,
    env,
    path::{Component, Path, PathBuf},
    time::Instant,
};

use ratatui::style::{Color, Modifier, Style};

use crate::{
    runtime::tool_activity_preview::{
        runtime_display_path, runtime_write_tool_activity_target,
        should_collapse_runtime_write_tool_activity,
    },
    theme::{TerminalPalette, secondary_text_style},
    transcript::{TRANSCRIPT_DETAIL_HINT, markdown_highlight::HighlightChunk},
};
use runtime_domain::envinfo::shorten_home_prefix;
use runtime_domain::session::{
    RuntimeTerminalSnapshot, RuntimeToolActivity, RuntimeToolActivityContent,
    RuntimeToolActivityLocation, RuntimeToolActivityStatus, RuntimeToolKind,
};

use super::{
    TOOL_ACTIVITY_ACTIVE_MARKER_BLINK_INTERVAL, TOOL_ACTIVITY_COMPACT_EDGE_LINES,
    TOOL_ACTIVITY_DIFF_LINE_NUMBER_WIDTH, ToolActivityRenderMode,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum RuntimeToolActivityDetailBlock {
    Text(Vec<String>),
    SecondaryText(Vec<String>),
    Diff(Vec<RuntimeDiffDetailLine>),
    ExecuteTranscript(RuntimeExecuteTranscriptBlock),
    ExecuteFooter(RuntimeExecuteFooterLine),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RuntimeExecuteTranscriptBlock {
    pub(super) command: String,
    pub(super) output_lines: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RuntimeExecuteFooterLine {
    pub(super) status: RuntimeExecuteFooterStatus,
    pub(super) marker: &'static str,
    pub(super) suffix: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RuntimeExecuteFooterStatus {
    Success,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RuntimeDiffDetailLine {
    pub(super) line_number: Option<usize>,
    pub(super) text: String,
    pub(super) kind: RuntimeDiffDetailLineKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RuntimeDiffDetailLineKind {
    Context,
    Insert,
    Delete,
    Separator,
    Omitted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RuntimeDiffChangeKind {
    Added,
    Edited,
    Deleted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ToolActivityGroupFamily {
    Exploration,
    SkillUsage,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RuntimeSkillUsageDescriptor {
    pub(super) display_name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ReadToolLineRange {
    start_line: usize,
    end_line: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RuntimeDiffSummary {
    path: String,
    added: usize,
    removed: usize,
    change_kind: RuntimeDiffChangeKind,
}

const TOOL_ACTIVITY_EXECUTE_COMPACT_EDGE_LINES: usize = 2;

fn is_full_detail_render_mode(render_mode: ToolActivityRenderMode) -> bool {
    matches!(
        render_mode,
        ToolActivityRenderMode::Detailed | ToolActivityRenderMode::DebugDetailed
    )
}

fn is_debug_detailed_render_mode(render_mode: ToolActivityRenderMode) -> bool {
    render_mode == ToolActivityRenderMode::DebugDetailed
}

pub(super) fn runtime_tool_activity_detail_blocks(
    call: &RuntimeToolActivity,
    render_mode: ToolActivityRenderMode,
    permission_waiting: bool,
    terminal_snapshots: &BTreeMap<String, RuntimeTerminalSnapshot>,
) -> Vec<RuntimeToolActivityDetailBlock> {
    if !is_debug_detailed_render_mode(render_mode) {
        if should_collapse_runtime_read_tool_activity(call) {
            return Vec::new();
        }
        if should_collapse_runtime_write_tool_activity(call) {
            return Vec::new();
        }
        if should_collapse_list_dir_tool_call(call) {
            return Vec::new();
        }
        if should_collapse_specific_search_tool_activity(call) {
            return Vec::new();
        }
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
        if call.status == RuntimeToolActivityStatus::Failed
            && let RuntimeToolActivityContent::Text(text) = content
        {
            blocks.push(RuntimeToolActivityDetailBlock::SecondaryText(
                truncate_detail_block(text_lines(text), render_mode),
            ));
            continue;
        }
        blocks.extend(runtime_tool_activity_content_blocks(
            content,
            render_mode,
            terminal_snapshots,
        ));
    }
    if call.status != RuntimeToolActivityStatus::Failed
        && !runtime_tool_activity_has_diff_content(call)
    {
        if let Some(raw_input) = call.raw_input.as_ref().and_then(|raw| raw.display_text()) {
            blocks.push(RuntimeToolActivityDetailBlock::Text(labeled_detail_block(
                "Input",
                &raw_input,
                render_mode,
            )));
        }
        if let Some(raw_output) = call.raw_output.as_ref().and_then(|raw| raw.display_text()) {
            blocks.push(RuntimeToolActivityDetailBlock::SecondaryText(
                truncate_detail_block(text_lines(&raw_output), render_mode),
            ));
        }
    }

    blocks
}

fn execute_tool_call_detail_blocks(
    call: &RuntimeToolActivity,
    render_mode: ToolActivityRenderMode,
    permission_waiting: bool,
    terminal_snapshots: &BTreeMap<String, RuntimeTerminalSnapshot>,
) -> Vec<RuntimeToolActivityDetailBlock> {
    if should_defer_active_execute_details(call, permission_waiting) {
        let terminal_blocks = active_execute_terminal_blocks(call, render_mode, terminal_snapshots);
        if !terminal_blocks.is_empty() {
            return terminal_blocks;
        }
        return vec![RuntimeToolActivityDetailBlock::SecondaryText(vec![
            "Waiting...".to_string(),
        ])];
    }

    if let Some(raw_output) = call.raw_output.as_ref().and_then(|raw| raw.display_text()) {
        let output_lines = execute_tool_call_output_lines(&raw_output);
        if is_full_detail_render_mode(render_mode) {
            let mut blocks = vec![RuntimeToolActivityDetailBlock::ExecuteTranscript(
                RuntimeExecuteTranscriptBlock {
                    command: execute_tool_call_display_command(call),
                    output_lines,
                },
            )];
            if let Some(footer) = execute_result_footer_line(call, render_mode) {
                blocks.push(RuntimeToolActivityDetailBlock::ExecuteFooter(footer));
            }
            return blocks;
        }

        let mut blocks = vec![RuntimeToolActivityDetailBlock::SecondaryText(
            truncate_execute_detail_block(output_lines, render_mode),
        )];
        if let Some(footer) = execute_result_footer_line(call, render_mode) {
            blocks.push(RuntimeToolActivityDetailBlock::ExecuteFooter(footer));
        }
        return blocks;
    }

    let mut blocks = Vec::new();
    for content in &call.content {
        if should_hide_execute_text_content(content) {
            continue;
        }
        if call.status == RuntimeToolActivityStatus::Failed
            && let RuntimeToolActivityContent::Text(text) = content
        {
            blocks.push(RuntimeToolActivityDetailBlock::SecondaryText(
                truncate_execute_detail_block(text_lines(text), render_mode),
            ));
            continue;
        }
        blocks.extend(runtime_tool_activity_content_blocks(
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
) -> Vec<RuntimeToolActivityDetailBlock> {
    call.content
        .iter()
        .filter(|content| matches!(content, RuntimeToolActivityContent::Terminal { .. }))
        .map(|content| {
            let RuntimeToolActivityContent::Terminal { terminal_id } = content else {
                unreachable!("content is filtered to terminal blocks");
            };
            let snapshot = terminal_snapshots.get(terminal_id);
            if is_full_detail_render_mode(render_mode) {
                return RuntimeToolActivityDetailBlock::ExecuteTranscript(
                    RuntimeExecuteTranscriptBlock {
                        command: execute_terminal_snapshot_command(call, snapshot),
                        output_lines: terminal_transcript_output_lines(snapshot),
                    },
                );
            }

            RuntimeToolActivityDetailBlock::SecondaryText(terminal_detail_lines_with_edge(
                snapshot,
                render_mode,
                TOOL_ACTIVITY_EXECUTE_COMPACT_EDGE_LINES,
            ))
        })
        .collect()
}

fn is_active_tool_call_status(status: RuntimeToolActivityStatus) -> bool {
    matches!(
        status,
        RuntimeToolActivityStatus::Pending | RuntimeToolActivityStatus::InProgress
    )
}

pub(super) fn is_execute_like_tool_call(call: &RuntimeToolActivity) -> bool {
    call.kind == RuntimeToolKind::Execute
        || call.title.trim_start().starts_with("Shell:")
        || call.title.trim_start().starts_with("Run ")
        || call
            .raw_input
            .as_ref()
            .and_then(|raw_input| raw_input.string_field(&["command", "cmd"]))
            .is_some()
}

pub(super) fn execute_tool_call_display_command(call: &RuntimeToolActivity) -> String {
    execute_tool_call_shell_command(call).unwrap_or_else(|| {
        let title = runtime_tool_activity_display_title(call);
        title
            .strip_prefix("Run ")
            .map(str::trim_start)
            .filter(|command| !command.is_empty())
            .unwrap_or(&title)
            .to_string()
    })
}

pub(super) fn execute_tool_call_shell_command(call: &RuntimeToolActivity) -> Option<String> {
    call.raw_input
        .as_ref()
        .and_then(|raw_input| raw_input.string_field(&["command", "cmd"]))
        .map(|command| command.trim().to_string())
        .filter(|command| !command.is_empty())
        .or_else(|| shell_command_from_title(&call.title))
}

fn shell_command_from_title(title: &str) -> Option<String> {
    let title = title.trim();
    ["Shell:", "Shell "].into_iter().find_map(|prefix| {
        title
            .strip_prefix(prefix)
            .map(str::trim_start)
            .filter(|command| !command.is_empty())
            .map(ToOwned::to_owned)
    })
}

fn execute_terminal_snapshot_command(
    call: &RuntimeToolActivity,
    snapshot: Option<&RuntimeTerminalSnapshot>,
) -> String {
    snapshot
        .and_then(|snapshot| snapshot.command.as_deref())
        .map(str::trim)
        .filter(|command| !command.is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| execute_tool_call_display_command(call))
}

fn terminal_transcript_output_lines(snapshot: Option<&RuntimeTerminalSnapshot>) -> Vec<String> {
    let Some(snapshot) = snapshot else {
        return Vec::new();
    };

    let mut lines = Vec::new();
    if snapshot.truncated {
        lines.push("... output truncated ...".to_string());
    }
    if !snapshot.output.is_empty() {
        lines.extend(text_lines(&snapshot.output));
    }
    lines
}

fn execute_tool_call_output_lines(raw_output: &str) -> Vec<String> {
    text_lines(raw_output)
}

fn execute_result_footer_line(
    call: &RuntimeToolActivity,
    render_mode: ToolActivityRenderMode,
) -> Option<RuntimeExecuteFooterLine> {
    if !is_full_detail_render_mode(render_mode) {
        return None;
    }
    let details = call.raw_output.as_ref()?.tool_result_details()?;
    let duration = details
        .get("duration_ms")
        .and_then(serde_json::Value::as_u64)
        .map(format_execution_duration_ms)?;
    let (status, marker, status_suffix) = execute_result_footer_status(details);
    let suffix = format!("{status_suffix} • {duration}");

    Some(RuntimeExecuteFooterLine {
        status,
        marker,
        suffix,
    })
}

fn execute_result_footer_status(
    details: &serde_json::Value,
) -> (RuntimeExecuteFooterStatus, &'static str, String) {
    if details
        .get("timed_out")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
    {
        return (
            RuntimeExecuteFooterStatus::Failed,
            "✗",
            " (timed out)".to_string(),
        );
    }
    if details
        .get("cancelled")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
    {
        return (
            RuntimeExecuteFooterStatus::Failed,
            "✗",
            " (cancelled)".to_string(),
        );
    }

    match details.get("exit_code").and_then(serde_json::Value::as_i64) {
        Some(0) => (RuntimeExecuteFooterStatus::Success, "✓", String::new()),
        Some(code) => (
            RuntimeExecuteFooterStatus::Failed,
            "✗",
            format!(" (exit {code})"),
        ),
        None => (RuntimeExecuteFooterStatus::Failed, "✗", String::new()),
    }
}

fn format_execution_duration_ms(duration_ms: u64) -> String {
    if duration_ms < 1_000 {
        format!("{duration_ms}ms")
    } else if duration_ms < 60_000 {
        format!("{:.2}s", duration_ms as f64 / 1_000.0)
    } else {
        let minutes = duration_ms / 60_000;
        let seconds = (duration_ms % 60_000) / 1_000;
        format!("{minutes}m {seconds:02}s")
    }
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

fn runtime_tool_activity_content_blocks(
    content: &RuntimeToolActivityContent,
    render_mode: ToolActivityRenderMode,
    terminal_snapshots: &BTreeMap<String, RuntimeTerminalSnapshot>,
) -> Vec<RuntimeToolActivityDetailBlock> {
    match content {
        RuntimeToolActivityContent::Text(text) => {
            vec![RuntimeToolActivityDetailBlock::Text(truncate_detail_block(
                text_lines(text),
                render_mode,
            ))]
        }
        RuntimeToolActivityContent::Image { mime_type, uri } => {
            vec![RuntimeToolActivityDetailBlock::Text(vec![match uri {
                Some(uri) => format!("Image: {mime_type} {uri}"),
                None => format!("Image: {mime_type}"),
            }])]
        }
        RuntimeToolActivityContent::Audio { mime_type } => {
            vec![RuntimeToolActivityDetailBlock::Text(vec![format!(
                "Audio: {mime_type}"
            )])]
        }
        RuntimeToolActivityContent::ResourceLink { uri, name, title } => {
            vec![RuntimeToolActivityDetailBlock::Text(vec![match title {
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
                return vec![RuntimeToolActivityDetailBlock::Text(vec![label])];
            };
            let mut lines = vec![label];
            lines.extend(text_lines(text));
            vec![RuntimeToolActivityDetailBlock::Text(truncate_detail_block(
                lines,
                render_mode,
            ))]
        }
        RuntimeToolActivityContent::Diff {
            path: _,
            old_text,
            new_text,
            is_truncated,
        } => vec![RuntimeToolActivityDetailBlock::Diff(
            truncate_diff_detail_block(
                diff_detail_lines_with_truncation(old_text.as_deref(), new_text, *is_truncated),
                render_mode,
            ),
        )],
        RuntimeToolActivityContent::Terminal { terminal_id } => {
            vec![RuntimeToolActivityDetailBlock::SecondaryText(
                terminal_detail_lines(terminal_snapshots.get(terminal_id), render_mode),
            )]
        }
        RuntimeToolActivityContent::Unknown(label) => {
            vec![RuntimeToolActivityDetailBlock::Text(vec![format!(
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
    terminal_detail_lines_with_edge(snapshot, render_mode, TOOL_ACTIVITY_COMPACT_EDGE_LINES)
}

fn terminal_detail_lines_with_edge(
    snapshot: Option<&RuntimeTerminalSnapshot>,
    render_mode: ToolActivityRenderMode,
    edge: usize,
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

    truncate_detail_block_with_edge(lines, render_mode, edge)
}

fn truncate_detail_block(lines: Vec<String>, render_mode: ToolActivityRenderMode) -> Vec<String> {
    truncate_detail_block_with_edge(lines, render_mode, TOOL_ACTIVITY_COMPACT_EDGE_LINES)
}

fn truncate_execute_detail_block(
    lines: Vec<String>,
    render_mode: ToolActivityRenderMode,
) -> Vec<String> {
    truncate_detail_block_with_edge(lines, render_mode, TOOL_ACTIVITY_EXECUTE_COMPACT_EDGE_LINES)
}

fn truncate_detail_block_with_edge(
    lines: Vec<String>,
    render_mode: ToolActivityRenderMode,
    edge: usize,
) -> Vec<String> {
    if is_full_detail_render_mode(render_mode) {
        return lines;
    }
    let limit = edge.saturating_mul(2);
    if lines.len() <= limit {
        return lines;
    }

    let omitted = lines.len().saturating_sub(limit);
    let mut truncated = Vec::with_capacity(limit + 1);
    truncated.extend(lines.iter().take(edge).cloned());
    truncated.push(format!("… +{omitted} lines ({TRANSCRIPT_DETAIL_HINT})"));
    truncated.extend(lines.iter().skip(lines.len().saturating_sub(edge)).cloned());
    truncated
}

fn truncate_diff_detail_block(
    lines: Vec<RuntimeDiffDetailLine>,
    render_mode: ToolActivityRenderMode,
) -> Vec<RuntimeDiffDetailLine> {
    if is_full_detail_render_mode(render_mode) {
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
    truncated.push(RuntimeDiffDetailLine {
        line_number: None,
        text: format!("⋮ +{omitted} lines ({TRANSCRIPT_DETAIL_HINT})"),
        kind: RuntimeDiffDetailLineKind::Omitted,
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

fn diff_detail_lines(old_text: Option<&str>, new_text: &str) -> Vec<RuntimeDiffDetailLine> {
    let Some(old_text) = old_text else {
        return text_lines(new_text)
            .into_iter()
            .enumerate()
            .map(|(index, line)| RuntimeDiffDetailLine {
                line_number: Some(index + 1),
                text: line,
                kind: RuntimeDiffDetailLineKind::Insert,
            })
            .collect();
    };

    let patch = diffy::create_patch(old_text, new_text);
    let mut lines = Vec::new();
    for (hunk_index, hunk) in patch.hunks().iter().enumerate() {
        if hunk_index > 0 {
            lines.push(RuntimeDiffDetailLine {
                line_number: None,
                text: "⋮".to_string(),
                kind: RuntimeDiffDetailLineKind::Separator,
            });
        }

        let mut old_line = hunk.old_range().start();
        let mut new_line = hunk.new_range().start();
        for line in hunk.lines() {
            match line {
                diffy::Line::Insert(text) => {
                    lines.push(RuntimeDiffDetailLine {
                        line_number: Some(new_line),
                        text: text.trim_end_matches('\n').to_string(),
                        kind: RuntimeDiffDetailLineKind::Insert,
                    });
                    new_line += 1;
                }
                diffy::Line::Delete(text) => {
                    lines.push(RuntimeDiffDetailLine {
                        line_number: Some(old_line),
                        text: text.trim_end_matches('\n').to_string(),
                        kind: RuntimeDiffDetailLineKind::Delete,
                    });
                    old_line += 1;
                }
                diffy::Line::Context(text) => {
                    lines.push(RuntimeDiffDetailLine {
                        line_number: Some(new_line),
                        text: text.trim_end_matches('\n').to_string(),
                        kind: RuntimeDiffDetailLineKind::Context,
                    });
                    old_line += 1;
                    new_line += 1;
                }
            }
        }
    }

    lines
}

fn diff_detail_lines_with_truncation(
    old_text: Option<&str>,
    new_text: &str,
    is_truncated: bool,
) -> Vec<RuntimeDiffDetailLine> {
    let mut lines = diff_detail_lines(old_text, new_text);
    if is_truncated {
        lines.insert(
            0,
            RuntimeDiffDetailLine {
                line_number: None,
                text: "⋮ preview truncated; showing partial diff".to_string(),
                kind: RuntimeDiffDetailLineKind::Omitted,
            },
        );
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

pub(super) fn runtime_tool_activity_diff_header_chunks(
    call: &RuntimeToolActivity,
    palette: TerminalPalette,
) -> Option<Vec<HighlightChunk>> {
    let summaries = call
        .content
        .iter()
        .filter_map(runtime_diff_summary)
        .collect::<Vec<_>>();
    if summaries.is_empty() {
        return None;
    }

    let title_style = Style::new().add_modifier(Modifier::BOLD);
    let mut chunks = Vec::new();
    if let [summary] = summaries.as_slice() {
        chunks.push(HighlightChunk {
            text: runtime_diff_change_kind_label(summary.change_kind).to_string(),
            style: title_style,
        });
        chunks.push(HighlightChunk {
            text: format!(" {} ", summary.path),
            style: Style::new(),
        });
        chunks.extend(runtime_diff_count_chunks(
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
    chunks.extend(runtime_diff_count_chunks(added, removed, palette));
    Some(chunks)
}

fn runtime_diff_summary(content: &RuntimeToolActivityContent) -> Option<RuntimeDiffSummary> {
    let RuntimeToolActivityContent::Diff {
        path,
        old_text,
        new_text,
        ..
    } = content
    else {
        return None;
    };
    let old_line_count = old_text.as_deref().map(line_count).unwrap_or(0);
    let new_line_count = line_count(new_text);
    let (added, removed) = runtime_diff_added_removed(old_text.as_deref(), new_text);
    let change_kind = if old_line_count == 0 && new_line_count > 0 {
        RuntimeDiffChangeKind::Added
    } else if old_line_count > 0 && new_line_count == 0 {
        RuntimeDiffChangeKind::Deleted
    } else {
        RuntimeDiffChangeKind::Edited
    };

    Some(RuntimeDiffSummary {
        path: runtime_display_path(path),
        added,
        removed,
        change_kind,
    })
}

fn runtime_diff_added_removed(old_text: Option<&str>, new_text: &str) -> (usize, usize) {
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

fn runtime_diff_change_kind_label(kind: RuntimeDiffChangeKind) -> &'static str {
    match kind {
        RuntimeDiffChangeKind::Added => "Added",
        RuntimeDiffChangeKind::Edited => "Edited",
        RuntimeDiffChangeKind::Deleted => "Deleted",
    }
}

fn runtime_diff_count_chunks(
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

pub(super) fn runtime_tool_activity_has_diff_content(call: &RuntimeToolActivity) -> bool {
    call.content
        .iter()
        .any(|content| matches!(content, RuntimeToolActivityContent::Diff { .. }))
}

pub(super) fn runtime_diff_line_prefix(
    line_number: Option<usize>,
    kind: RuntimeDiffDetailLineKind,
) -> String {
    let sign = match kind {
        RuntimeDiffDetailLineKind::Insert => "+",
        RuntimeDiffDetailLineKind::Delete => "-",
        RuntimeDiffDetailLineKind::Context
        | RuntimeDiffDetailLineKind::Separator
        | RuntimeDiffDetailLineKind::Omitted => " ",
    };
    match line_number {
        Some(line_number) => format!(
            "{line_number:>width$} {sign}  ",
            width = TOOL_ACTIVITY_DIFF_LINE_NUMBER_WIDTH
        ),
        None => " ".repeat(TOOL_ACTIVITY_DIFF_LINE_NUMBER_WIDTH.saturating_sub(1)),
    }
}

pub(super) fn runtime_tool_activity_location_suffix(
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

pub(super) fn runtime_tool_activity_display_title(call: &RuntimeToolActivity) -> String {
    let title = call.title.trim();
    // runtime 标题常带有 `Shell:` 一类传输前缀，紧凑头部只显示真正命令。
    let title = title
        .strip_prefix("Shell:")
        .map(str::trim_start)
        .unwrap_or(title);

    if title.is_empty() {
        runtime_tool_kind_label(call.kind).to_string()
    } else {
        title.to_string()
    }
}

pub(super) fn runtime_read_tool_activity_title_chunks(
    call: &RuntimeToolActivity,
) -> Vec<HighlightChunk> {
    if let Some(skill_usage) = runtime_skill_usage_descriptor(call) {
        return runtime_skill_usage_title_chunks(&skill_usage.display_name);
    }
    let title_style = Style::new().add_modifier(Modifier::BOLD);
    let Some(target) = runtime_read_tool_activity_target(call) else {
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

pub(super) fn runtime_skill_usage_title_chunks(display_name: &str) -> Vec<HighlightChunk> {
    let title_style = Style::new().add_modifier(Modifier::BOLD);
    vec![
        HighlightChunk {
            text: "Use".to_string(),
            style: title_style,
        },
        HighlightChunk {
            text: format!(" {display_name} Skill"),
            style: Style::new(),
        },
    ]
}

pub(super) fn runtime_write_tool_activity_title_chunks(
    call: &RuntimeToolActivity,
) -> Vec<HighlightChunk> {
    let title_style = Style::new().add_modifier(Modifier::BOLD);
    let Some(target) = runtime_write_tool_activity_target(call) else {
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

pub(super) fn list_dir_tool_call_title_chunks(call: &RuntimeToolActivity) -> Vec<HighlightChunk> {
    let title_style = Style::new().add_modifier(Modifier::BOLD);
    vec![
        HighlightChunk {
            text: "List".to_string(),
            style: title_style,
        },
        HighlightChunk {
            text: format!(" {}", list_dir_tool_call_target(call)),
            style: Style::new(),
        },
    ]
}

pub(super) struct SpecificSearchToolActivityParts {
    pub(super) action: &'static str,
    pub(super) pattern: String,
    pub(super) path: String,
}

impl SpecificSearchToolActivityParts {
    pub(super) fn detail_chunks(&self, palette: TerminalPalette) -> Vec<HighlightChunk> {
        vec![
            HighlightChunk {
                text: self.pattern.clone(),
                style: Style::new(),
            },
            HighlightChunk {
                text: " in ".to_string(),
                style: secondary_text_style(palette),
            },
            HighlightChunk {
                text: self.path.clone(),
                style: Style::new(),
            },
        ]
    }
}

pub(super) fn specific_search_tool_activity_parts(
    call: &RuntimeToolActivity,
) -> Option<SpecificSearchToolActivityParts> {
    let action = specific_search_tool_activity_action(call)?;
    let raw_input = call.raw_input.as_ref()?;
    let pattern = raw_input
        .string_field(&["pattern"])
        .map(|pattern| pattern.trim().to_string())
        .filter(|pattern| !pattern.is_empty())?;
    let path = raw_input
        .string_field(&["path"])
        .map(|path| path.trim().to_string())
        .filter(|path| !path.is_empty())
        .unwrap_or_else(|| ".".to_string());

    Some(SpecificSearchToolActivityParts {
        action,
        pattern,
        path: list_dir_display_path(&path),
    })
}

pub(super) fn specific_search_tool_activity_title_chunks(
    call: &RuntimeToolActivity,
    palette: TerminalPalette,
) -> Option<Vec<HighlightChunk>> {
    let parts = specific_search_tool_activity_parts(call)?;
    let mut chunks = vec![HighlightChunk {
        text: parts.action.to_string(),
        style: Style::new().add_modifier(Modifier::BOLD),
    }];
    chunks.push(HighlightChunk {
        text: " ".to_string(),
        style: Style::new(),
    });
    chunks.extend(parts.detail_chunks(palette));

    Some(chunks)
}

fn specific_search_tool_activity_action(call: &RuntimeToolActivity) -> Option<&'static str> {
    if call.kind != RuntimeToolKind::Search {
        return None;
    }

    let title = runtime_tool_activity_display_title(call);
    let title = title.trim();
    if search_tool_title_uses_action(title, "Grep") {
        return Some("Grep");
    }
    if search_tool_title_uses_action(title, "Find") {
        return Some("Find");
    }

    None
}

fn search_tool_title_uses_action(title: &str, action: &str) -> bool {
    title == action
        || title
            .strip_prefix(action)
            .is_some_and(|suffix| suffix.starts_with([' ', ':']))
}

fn runtime_read_tool_activity_target(call: &RuntimeToolActivity) -> Option<String> {
    let line_range = read_tool_line_range(call);
    if call.kind == RuntimeToolKind::Read
        && let [location] = call.locations.as_slice()
        && !location.path.trim().is_empty()
    {
        return Some(read_tool_target_with_line_range(
            runtime_display_path(location.path.trim()),
            line_range,
        ));
    }

    runtime_read_tool_activity_title_target(&call.title)
        .map(|target| read_tool_target_with_line_range(target, line_range))
}

fn runtime_read_tool_activity_title_target(title: &str) -> Option<String> {
    runtime_tool_activity_title_target(title, &["ReadFile:", "Read File:", "Read:", "Read "])
}

fn runtime_tool_activity_title_target(title: &str, prefixes: &[&str]) -> Option<String> {
    let title = title.trim();
    prefixes.iter().find_map(|prefix| {
        title.strip_prefix(prefix).and_then(|target| {
            let target = target.trim();
            (!target.is_empty()).then(|| runtime_display_path(target))
        })
    })
}

pub(super) fn is_runtime_read_tool_activity(call: &RuntimeToolActivity) -> bool {
    call.kind == RuntimeToolKind::Read
        || runtime_read_tool_activity_title_target(&call.title).is_some()
}

pub(super) fn tool_activity_group_family(
    call: &RuntimeToolActivity,
) -> Option<ToolActivityGroupFamily> {
    if runtime_skill_usage_descriptor(call).is_some() {
        return Some(ToolActivityGroupFamily::SkillUsage);
    }
    if is_runtime_read_tool_activity(call)
        || is_list_dir_tool_call(call)
        || call.kind == RuntimeToolKind::Search
    {
        return Some(ToolActivityGroupFamily::Exploration);
    }
    None
}

fn read_tool_target_with_line_range(
    mut target: String,
    line_range: Option<ReadToolLineRange>,
) -> String {
    if let Some(line_range) = line_range {
        target.push_str(&format!(
            "({}~{})",
            line_range.start_line, line_range.end_line
        ));
    }

    target
}

fn read_tool_line_range(call: &RuntimeToolActivity) -> Option<ReadToolLineRange> {
    let details = call.raw_output.as_ref()?.tool_result_details()?;
    let start_line = usize::try_from(details.get("start_line")?.as_u64()?).ok()?;
    let end_line = usize::try_from(details.get("end_line")?.as_u64()?).ok()?;
    let total_lines = usize::try_from(details.get("total_lines")?.as_u64()?).ok()?;
    if start_line == 0 || end_line < start_line || end_line > total_lines || total_lines == 0 {
        return None;
    }

    let has_next_offset = details
        .get("next_offset")
        .is_some_and(|next_offset| !next_offset.is_null());
    let covers_complete_file = start_line == 1 && end_line == total_lines && !has_next_offset;
    (!covers_complete_file).then_some(ReadToolLineRange {
        start_line,
        end_line,
    })
}

pub(super) fn is_list_dir_tool_call(call: &RuntimeToolActivity) -> bool {
    let title = call.title.trim();
    call.kind == RuntimeToolKind::Search
        && (title == "List Directory" || title.starts_with("List Directory "))
}

fn should_collapse_runtime_read_tool_activity(call: &RuntimeToolActivity) -> bool {
    is_runtime_read_tool_activity(call) && call.status != RuntimeToolActivityStatus::Failed
}

fn should_collapse_list_dir_tool_call(call: &RuntimeToolActivity) -> bool {
    is_list_dir_tool_call(call) && call.status != RuntimeToolActivityStatus::Failed
}

fn should_collapse_specific_search_tool_activity(call: &RuntimeToolActivity) -> bool {
    specific_search_tool_activity_parts(call).is_some()
        && call.status != RuntimeToolActivityStatus::Failed
}

pub(super) fn runtime_skill_usage_descriptor(
    call: &RuntimeToolActivity,
) -> Option<RuntimeSkillUsageDescriptor> {
    let name_from_metadata = call
        .raw_input
        .as_ref()
        .and_then(|raw_input| raw_input.string_field(&["hunea_skill_name"]));
    let origin_from_metadata = call
        .raw_input
        .as_ref()
        .and_then(|raw_input| raw_input.string_field(&["hunea_skill_origin"]));
    if let Some(skill_name) = name_from_metadata {
        return Some(RuntimeSkillUsageDescriptor {
            display_name: format_skill_usage_display_name(
                &skill_name,
                origin_from_metadata.as_deref(),
            ),
        });
    }

    let raw_path = runtime_read_tool_activity_raw_path(call)?;
    skill_usage_descriptor_from_path(&raw_path)
}

fn runtime_read_tool_activity_raw_path(call: &RuntimeToolActivity) -> Option<String> {
    if call.kind == RuntimeToolKind::Read
        && let [location] = call.locations.as_slice()
        && !location.path.trim().is_empty()
    {
        return Some(location.path.trim().to_string());
    }

    call.raw_input
        .as_ref()
        .and_then(|raw_input| raw_input.string_field(&["path"]))
        .or_else(|| runtime_read_tool_activity_title_target(&call.title))
}

fn skill_usage_descriptor_from_path(path: &str) -> Option<RuntimeSkillUsageDescriptor> {
    let resolved_path = resolve_skill_usage_path(path);
    let file_name = resolved_path.file_name()?.to_str()?;
    if file_name != "SKILL.md" {
        return None;
    }
    let skill_name = resolved_path.parent()?.file_name()?.to_str()?;
    let origin = infer_skill_usage_origin(&resolved_path);
    Some(RuntimeSkillUsageDescriptor {
        display_name: format_skill_usage_display_name(skill_name, origin),
    })
}

fn resolve_skill_usage_path(path: &str) -> PathBuf {
    let path = PathBuf::from(path);
    if path.is_absolute() {
        path
    } else if let Ok(cwd) = env::current_dir() {
        cwd.join(path)
    } else {
        path
    }
}

fn infer_skill_usage_origin(path: &Path) -> Option<&'static str> {
    let home_dir = env::var_os("HOME").map(PathBuf::from)?;
    let global_root = home_dir.join(".agents").join("skills");
    if path.starts_with(&global_root) {
        return Some("global");
    }
    None
}

fn format_skill_usage_display_name(skill_name: &str, origin: Option<&str>) -> String {
    if origin == Some("global") {
        format!("{skill_name}(global)")
    } else {
        skill_name.to_string()
    }
}

fn list_dir_tool_call_target(call: &RuntimeToolActivity) -> String {
    let target = call
        .raw_input
        .as_ref()
        .and_then(|raw_input| raw_input.string_field(&["path"]))
        .or_else(|| {
            let title = call.title.trim();
            title
                .strip_prefix("List Directory ")
                .map(str::trim)
                .filter(|path| !path.is_empty())
                .map(str::to_string)
        })
        .or_else(|| {
            let [location] = call.locations.as_slice() else {
                return None;
            };
            (!location.path.trim().is_empty()).then(|| location.path.clone())
        })
        .unwrap_or_else(|| ".".to_string());

    list_dir_display_path(&target)
}

fn list_dir_display_path(path: &str) -> String {
    let path = path.trim();
    if path.is_empty() || path == "." {
        return ".".to_string();
    }

    let path_ref = Path::new(path);
    if !path_ref.is_absolute() {
        return relative_display_path(path_ref);
    }

    if let Ok(cwd) = env::current_dir() {
        if path_ref == cwd {
            return ".".to_string();
        }
        if let Ok(stripped) = path_ref.strip_prefix(cwd)
            && !stripped.as_os_str().is_empty()
        {
            return relative_display_path(stripped);
        }
    }

    detect_home_dir()
        .map(|home_dir| shorten_home_prefix(path_ref, &home_dir))
        .unwrap_or_else(|| path_ref.display().to_string())
}

fn relative_display_path(path: &Path) -> String {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(part) => normalized.push(part),
            Component::ParentDir => normalized.push(".."),
            Component::RootDir | Component::Prefix(_) => normalized.push(component.as_os_str()),
        }
    }

    if normalized.as_os_str().is_empty() {
        ".".to_string()
    } else {
        normalized.display().to_string()
    }
}

fn detect_home_dir() -> Option<PathBuf> {
    env::var_os("HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("USERPROFILE").map(PathBuf::from))
        .or_else(|| {
            let home_drive = env::var_os("HOMEDRIVE")?;
            let home_path = env::var_os("HOMEPATH")?;
            let mut path = PathBuf::from(home_drive);
            path.push(home_path);
            Some(path)
        })
}

pub(super) fn runtime_tool_activity_diff_line_style(
    kind: RuntimeDiffDetailLineKind,
    palette: TerminalPalette,
) -> Style {
    match kind {
        RuntimeDiffDetailLineKind::Context => Style::new(),
        RuntimeDiffDetailLineKind::Insert => Style::new().fg(palette.quote),
        RuntimeDiffDetailLineKind::Delete => Style::new().fg(palette.system_error),
        RuntimeDiffDetailLineKind::Separator | RuntimeDiffDetailLineKind::Omitted => {
            Style::new().fg(palette.tertiary)
        }
    }
}

pub(super) fn runtime_tool_activity_diff_row_style(
    kind: RuntimeDiffDetailLineKind,
    palette: TerminalPalette,
) -> Style {
    runtime_tool_activity_diff_background(kind, palette)
        .map(|background| Style::new().bg(background))
        .unwrap_or_default()
}

fn runtime_tool_activity_diff_background(
    kind: RuntimeDiffDetailLineKind,
    palette: TerminalPalette,
) -> Option<Color> {
    match kind {
        RuntimeDiffDetailLineKind::Context => None,
        RuntimeDiffDetailLineKind::Insert => runtime_tool_activity_diff_tint(palette, true),
        RuntimeDiffDetailLineKind::Delete => runtime_tool_activity_diff_tint(palette, false),
        RuntimeDiffDetailLineKind::Separator | RuntimeDiffDetailLineKind::Omitted => None,
    }
}

fn runtime_tool_activity_diff_tint(palette: TerminalPalette, is_insert: bool) -> Option<Color> {
    let _surface = palette.surface?;

    Some(match (palette.has_dark_background(), is_insert) {
        (true, true) => Color::Rgb(38, 58, 44),
        (true, false) => Color::Rgb(64, 44, 44),
        (false, true) => Color::Rgb(228, 242, 230),
        (false, false) => Color::Rgb(247, 229, 229),
    })
}

fn runtime_tool_kind_label(kind: RuntimeToolKind) -> &'static str {
    match kind {
        RuntimeToolKind::Read => "Read",
        RuntimeToolKind::Write => "Write",
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

pub(super) fn runtime_tool_activity_status_color(
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

pub(super) fn runtime_tool_activity_content_byte_len(
    content: &RuntimeToolActivityContent,
) -> usize {
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
            ..
        } => path.len() + old_text.as_deref().map(str::len).unwrap_or(0) + new_text.len(),
        RuntimeToolActivityContent::Terminal { terminal_id } => terminal_id.len(),
        RuntimeToolActivityContent::Unknown(label) => label.len(),
    }
}
