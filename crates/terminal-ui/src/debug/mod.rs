mod tool_approval_preview;

use super::command_panel::{CommandPanelAction, CommandPanelItem};

pub(super) fn command_panel_items() -> Vec<CommandPanelItem> {
    vec![CommandPanelItem {
        name: "/tool-debug".to_string(),
        aliases: Vec::new(),
        description: "Preview tool approval panel".to_string(),
        action: CommandPanelAction::OpenToolApprovalDebug,
    }]
}
