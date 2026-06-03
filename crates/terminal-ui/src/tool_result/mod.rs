mod activity;
mod approval;
mod exploration;
mod item;
mod state;

#[cfg(test)]
mod tests;

#[cfg(test)]
pub(super) use runtime_domain::session::{
    RuntimeToolActivity, RuntimeToolActivityContent, RuntimeToolActivityLocation,
    RuntimeToolActivityStatus, RuntimeToolActivityUpdate,
};

pub(crate) use item::{
    TOOL_ACTIVITY_ACTIVE_MARKER_BLINK_INTERVAL, ToolActivityRenderMode, ToolResultItem,
    ToolResultKind,
};
use item::{
    TOOL_ACTIVITY_COMPACT_EDGE_LINES, TOOL_ACTIVITY_DIFF_LINE_NUMBER_WIDTH,
    TOOL_EXPLORATION_BRANCH_PREFIX, TOOL_EXPLORATION_CHILD_PREFIX, ToolResultBody,
};
