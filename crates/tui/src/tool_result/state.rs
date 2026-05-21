use std::{
    collections::{BTreeMap, hash_map::DefaultHasher},
    hash::{Hash, Hasher},
};

use mo_core::session::{
    RuntimeTerminalSnapshot, RuntimeToolActivity, RuntimeToolActivityContent,
    RuntimeToolActivityStatus, RuntimeToolActivityUpdate,
};

use super::{ToolActivityRenderMode, ToolResultBody, acp::acp_tool_call_content_byte_len};

pub(super) fn runtime_tool_activity_source_byte_len(call: &RuntimeToolActivity) -> usize {
    call.title.len()
        + call
            .raw_input
            .as_ref()
            .map(|raw_input| raw_input.display_byte_len())
            .unwrap_or(0)
        + call
            .raw_output
            .as_ref()
            .map(|raw_output| raw_output.display_byte_len())
            .unwrap_or(0)
        + call
            .content
            .iter()
            .map(acp_tool_call_content_byte_len)
            .sum::<usize>()
}

pub(super) fn apply_runtime_tool_activity_update(
    call: &mut RuntimeToolActivity,
    update: RuntimeToolActivityUpdate,
    permission_waiting: &mut bool,
    terminal_snapshots: &mut BTreeMap<String, RuntimeTerminalSnapshot>,
) {
    if let Some(title) = update.title {
        call.title = title;
    }
    if let Some(kind) = update.kind {
        call.kind = kind;
    }
    if let Some(status) = update.status {
        call.status = status;
        if status != RuntimeToolActivityStatus::Pending {
            *permission_waiting = false;
        }
    }
    if let Some(content) = update.content {
        call.content = content;
        terminal_snapshots.retain(|terminal_id, _| {
            call.content.iter().any(|content| {
                matches!(content, RuntimeToolActivityContent::Terminal { terminal_id: content_terminal_id } if content_terminal_id == terminal_id)
            })
        });
    }
    if let Some(locations) = update.locations {
        call.locations = locations;
    }
    if let Some(raw_input) = update.raw_input {
        call.raw_input = Some(raw_input);
    }
    if let Some(raw_output) = update.raw_output {
        call.raw_output = Some(raw_output);
    }
}

pub(super) fn active_marker_started_at_for_body(
    body: &ToolResultBody,
    terminal_snapshots: &BTreeMap<String, RuntimeTerminalSnapshot>,
) -> bool {
    match body {
        ToolResultBody::RuntimeToolActivity(call) => {
            active_marker_started_at_for_call(call, terminal_snapshots)
        }
        ToolResultBody::Exploration(calls) => calls.iter().any(|call| {
            matches!(
                call.status,
                RuntimeToolActivityStatus::Pending | RuntimeToolActivityStatus::InProgress
            )
        }),
        ToolResultBody::Approval { .. } => false,
    }
}

fn active_marker_started_at_for_call(
    call: &RuntimeToolActivity,
    terminal_snapshots: &BTreeMap<String, RuntimeTerminalSnapshot>,
) -> bool {
    if !matches!(
        call.status,
        RuntimeToolActivityStatus::Pending | RuntimeToolActivityStatus::InProgress
    ) {
        return false;
    }
    let terminal_ids = call
        .content
        .iter()
        .filter_map(|content| match content {
            RuntimeToolActivityContent::Terminal { terminal_id } => Some(terminal_id),
            _ => None,
        })
        .collect::<Vec<_>>();
    if terminal_ids.is_empty() {
        return true;
    }
    terminal_ids.iter().any(|terminal_id| {
        terminal_snapshots
            .get(*terminal_id)
            .is_none_or(|snapshot| snapshot.exit_status.is_none() && !snapshot.released)
    })
}

pub(super) fn tool_result_render_cache_key(
    body: &ToolResultBody,
    render_mode: ToolActivityRenderMode,
    exploration_open: bool,
    approval_suspended: bool,
    permission_waiting: bool,
    terminal_snapshots: &BTreeMap<String, RuntimeTerminalSnapshot>,
) -> u64 {
    let mut hasher = DefaultHasher::new();
    "tool_result".hash(&mut hasher);
    render_mode.hash(&mut hasher);
    exploration_open.hash(&mut hasher);
    approval_suspended.hash(&mut hasher);
    permission_waiting.hash(&mut hasher);
    terminal_snapshots.hash(&mut hasher);
    body.hash(&mut hasher);
    hasher.finish()
}
