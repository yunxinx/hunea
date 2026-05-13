use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    style::Modifier,
    text::{Line, Span},
};
use unicode_width::UnicodeWidthStr;

use super::super::{
    Model,
    inline_panel::{InlinePanelRenderResult, inline_panel_render_result, inline_panel_rule_line},
    status_line::truncate_display_width_with_ellipsis,
    theme::{
        command_accent_text_style, primary_text_style, secondary_text_style, tertiary_text_style,
    },
};

pub(super) const ACP_DEBUG_PANEL_VISIBLE_ROWS: usize = 12;

const ACP_DEBUG_LIST_VISIBLE_ROWS: usize = 8;
const ACP_DEBUG_COMMAND_INSET_WIDTH: usize = 2;
const ACP_DEBUG_DESCRIPTION_GAP: usize = 4;
const ACP_DEBUG_SELECTED_MARKER: &str = "➜ ";
const ACP_DEBUG_PROTOCOL_VERSION_SYSTEM_MSG: &str = "protocolVersion-system-msg";
const ACP_DEBUG_AGENT_CAPABILITIES_SYSTEM_MSG: &str = "agent-capabilities-system-msg";

/// `AcpDebugPanelState` 保存 ACP debug 面板的导航状态。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct AcpDebugPanelState {
    pub(crate) is_open: bool,
    pub(crate) selected: usize,
    pub(crate) scroll: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct AcpDebugItem {
    name: &'static str,
    description: &'static str,
}

impl Model {
    pub(super) fn handle_acp_debug_panel_key(
        &mut self,
        key: KeyEvent,
    ) -> Option<Option<super::super::AppEffect>> {
        match key.code {
            KeyCode::Esc => {
                self.close_acp_panel();
                Some(None)
            }
            KeyCode::Up if key.modifiers.is_empty() => {
                self.move_acp_debug_panel_selection(-1);
                Some(None)
            }
            KeyCode::Down if key.modifiers.is_empty() => {
                self.move_acp_debug_panel_selection(1);
                Some(None)
            }
            KeyCode::Enter if key.modifiers.is_empty() => {
                self.select_current_acp_debug_panel_item();
                Some(None)
            }
            _ => Some(None),
        }
    }

    pub(super) fn sync_acp_debug_panel_scroll(&mut self) {
        let item_count = acp_debug_items().len();
        if item_count == 0 {
            self.acp_debug_panel.selected = 0;
            self.acp_debug_panel.scroll = 0;
            return;
        }

        let selected = self.acp_debug_panel.selected.min(item_count - 1);
        let max_scroll = item_count.saturating_sub(ACP_DEBUG_LIST_VISIBLE_ROWS);
        let mut scroll = self.acp_debug_panel.scroll.min(max_scroll);
        if selected < scroll {
            scroll = selected;
        }
        if selected >= scroll + ACP_DEBUG_LIST_VISIBLE_ROWS {
            scroll = selected + 1 - ACP_DEBUG_LIST_VISIBLE_ROWS;
        }
        self.acp_debug_panel.selected = selected;
        self.acp_debug_panel.scroll = scroll;
    }

    pub(super) fn current_inline_acp_debug_panel_render_result(&self) -> InlinePanelRenderResult {
        let visible_rows = self.acp_panel_visible_rows();
        let width = usize::from(self.width.max(1));
        let mut lines = build_debug_panel_lines(self, width);
        if lines.len() > visible_rows {
            lines.truncate(visible_rows);
        }
        inline_panel_render_result(lines)
    }

    fn move_acp_debug_panel_selection(&mut self, delta: isize) {
        let last_index = acp_debug_items().len().saturating_sub(1);
        self.acp_debug_panel.selected = if delta.is_negative() {
            self.acp_debug_panel
                .selected
                .saturating_sub(delta.unsigned_abs())
        } else {
            self.acp_debug_panel
                .selected
                .saturating_add(delta as usize)
                .min(last_index)
        };
        self.sync_acp_debug_panel_scroll();
    }

    fn select_current_acp_debug_panel_item(&mut self) {
        let Some(item) = acp_debug_items().get(self.acp_debug_panel.selected) else {
            return;
        };

        match item.name {
            ACP_DEBUG_PROTOCOL_VERSION_SYSTEM_MSG => {
                self.close_acp_panel();
                self.append_system_message_from_runtime(
                    ::mo_acp::debug_protocol_version_system_message(),
                );
            }
            ACP_DEBUG_AGENT_CAPABILITIES_SYSTEM_MSG => {
                self.close_acp_panel();
                self.append_system_message_from_runtime(agent_capabilities_system_message(self));
            }
            _ => {}
        }
    }
}

fn acp_debug_items() -> &'static [AcpDebugItem] {
    &[
        AcpDebugItem {
            name: ACP_DEBUG_PROTOCOL_VERSION_SYSTEM_MSG,
            description: "Append protocolVersion warning system message",
        },
        AcpDebugItem {
            name: ACP_DEBUG_AGENT_CAPABILITIES_SYSTEM_MSG,
            description: "Append selected agent capabilities system message",
        },
    ]
}

fn build_debug_panel_lines(model: &Model, width: usize) -> Vec<Line<'static>> {
    let width = width.max(1);
    let mut lines = vec![
        inline_panel_rule_line(width, model.palette),
        acp_debug_header_line(model),
        Line::raw(""),
    ];
    lines.push(Line::styled(
        "  Test Items:",
        secondary_text_style(model.palette).bold(),
    ));
    append_acp_debug_item_lines(model, width, &mut lines);
    lines.push(Line::raw(""));
    lines.push(Line::styled(
        "  Press Enter to run · Esc to exit · ↑↓ to navigate",
        tertiary_text_style(model.palette).add_modifier(Modifier::ITALIC),
    ));

    lines
}

fn acp_debug_header_line(model: &Model) -> Line<'static> {
    Line::from(vec![
        Span::raw("  "),
        Span::styled("ACP Debug:", primary_text_style(model.palette)),
    ])
}

fn append_acp_debug_item_lines(model: &Model, width: usize, lines: &mut Vec<Line<'static>>) {
    let items = acp_debug_items();
    let start = model.acp_debug_panel.scroll;
    let end = (start + ACP_DEBUG_LIST_VISIBLE_ROWS).min(items.len());
    let command_column_width = acp_debug_command_column_width(&items[start..end]);
    for (offset, item) in items[start..end].iter().enumerate() {
        let index = start + offset;
        append_acp_debug_item_entry_lines(
            model,
            item,
            index == model.acp_debug_panel.selected,
            width,
            command_column_width,
            lines,
        );
    }
}

fn append_acp_debug_item_entry_lines(
    model: &Model,
    item: &AcpDebugItem,
    selected: bool,
    width: usize,
    command_column_width: usize,
    lines: &mut Vec<Line<'static>>,
) {
    let width = width.max(1);
    let inset_width = ACP_DEBUG_COMMAND_INSET_WIDTH.min(width);
    let marker_width = ACP_DEBUG_SELECTED_MARKER.width();
    let marker = if selected {
        ACP_DEBUG_SELECTED_MARKER.to_string()
    } else {
        " ".repeat(marker_width)
    };
    let used_before_command = inset_width + marker_width;
    let available_after_marker = width.saturating_sub(used_before_command);
    let command_column_width = command_column_width.min(available_after_marker);
    let command_text = truncate_display_width_with_ellipsis(item.name, command_column_width);
    let command_padding_width = command_column_width.saturating_sub(command_text.width());
    let remaining_after_command = available_after_marker.saturating_sub(command_column_width);
    let gap_width = ACP_DEBUG_DESCRIPTION_GAP.min(remaining_after_command);
    let description_width = remaining_after_command.saturating_sub(gap_width);
    let description_text =
        truncate_display_width_with_ellipsis(item.description, description_width);
    let trailing_padding = width.saturating_sub(
        inset_width
            + marker_width
            + command_text.width()
            + command_padding_width
            + gap_width
            + description_text.width(),
    );

    let marker_style = if selected {
        primary_text_style(model.palette).bold()
    } else {
        tertiary_text_style(model.palette)
    };
    let command_style = if selected {
        command_accent_text_style(model.palette).bold()
    } else {
        secondary_text_style(model.palette)
    };
    let description_style = if selected {
        primary_text_style(model.palette)
    } else {
        secondary_text_style(model.palette)
    };

    lines.push(Line::from(vec![
        Span::raw(" ".repeat(inset_width)),
        Span::styled(marker, marker_style),
        Span::styled(command_text, command_style),
        Span::raw(" ".repeat(command_padding_width + gap_width)),
        Span::styled(description_text, description_style),
        Span::raw(" ".repeat(trailing_padding)),
    ]));
}

fn acp_debug_command_column_width(items: &[AcpDebugItem]) -> usize {
    items
        .iter()
        .map(|item| item.name.width())
        .max()
        .unwrap_or_default()
}

fn agent_capabilities_system_message(model: &Model) -> String {
    let Some(agent_id) = model.selected_acp_agent.as_deref() else {
        return "ACP agent capabilities: no ACP agent selected.".to_string();
    };
    let Some(identity) = model.acp_agent_identities.get(agent_id) else {
        return format!("ACP agent capabilities: no initialize result recorded for {agent_id}.");
    };

    let capabilities = serde_json::to_string_pretty(&identity.agent_capabilities)
        .unwrap_or_else(|error| format!("{{\"error\":\"failed to serialize: {error}\"}}"));
    format!("ACP agent capabilities for {agent_id}:\n{capabilities}")
}

#[cfg(test)]
mod tests {
    use agent_client_protocol::schema::{AgentCapabilities, PromptCapabilities};

    use crate::{HeroOptions, ModelOptions, theme::default_palette};
    use ::mo_acp::AcpAgentIdentity;

    use super::*;

    #[test]
    fn acp_debug_panel_lists_agent_capabilities_system_message_item_without_inline_json() {
        let mut model = Model::new_with_options(
            HeroOptions::default(),
            ModelOptions {
                acp_agent_servers: vec!["kimi".to_string()],
                ..ModelOptions::default()
            },
        );
        model.palette = default_palette();
        model.selected_acp_agent = Some("kimi".to_string());
        model.apply_acp_agent_identity(
            "kimi",
            AcpAgentIdentity {
                name: Some("kimi".to_string()),
                title: Some("Kimi Code CLI".to_string()),
                version: Some("1.39.0".to_string()),
                agent_capabilities: AgentCapabilities::new()
                    .load_session(true)
                    .prompt_capabilities(PromptCapabilities::new().image(true).audio(true)),
            },
        );

        let lines = build_debug_panel_lines(&model, 96)
            .into_iter()
            .map(|line| {
                line.spans
                    .into_iter()
                    .map(|span| span.content.into_owned())
                    .collect::<String>()
            })
            .collect::<Vec<_>>();

        assert!(
            lines
                .iter()
                .any(|line| line.contains(ACP_DEBUG_AGENT_CAPABILITIES_SYSTEM_MSG)),
            "debug panel should include a capabilities system-message item: {lines:?}"
        );
        assert!(
            lines
                .iter()
                .all(|line| !line.contains("\"loadSession\": true")),
            "debug panel should not render capabilities JSON inline: {lines:?}"
        );
    }

    #[test]
    fn acp_debug_agent_capabilities_item_appends_system_message() {
        let mut model = Model::new_with_options(
            HeroOptions::default(),
            ModelOptions {
                acp_agent_servers: vec!["kimi".to_string()],
                ..ModelOptions::default()
            },
        );
        model.palette = default_palette();
        model.selected_acp_agent = Some("kimi".to_string());
        model.apply_acp_agent_identity(
            "kimi",
            AcpAgentIdentity {
                name: Some("kimi".to_string()),
                title: Some("Kimi Code CLI".to_string()),
                version: Some("1.39.0".to_string()),
                agent_capabilities: AgentCapabilities::new()
                    .load_session(true)
                    .prompt_capabilities(PromptCapabilities::new().image(true).audio(true)),
            },
        );
        model.open_acp_debug_panel();
        model.acp_debug_panel.selected = acp_debug_items()
            .iter()
            .position(|item| item.name == ACP_DEBUG_AGENT_CAPABILITIES_SYSTEM_MSG)
            .expect("agent capabilities debug item should exist");

        model.select_current_acp_debug_panel_item();

        let transcript_items = model.transcript_plain_items();
        let system_message = transcript_items
            .last()
            .expect("debug item should append a system message")
            .as_str();
        assert!(
            system_message.contains("ACP agent capabilities for kimi:"),
            "expected selected agent id in system message, got: {system_message:?}"
        );
        assert!(
            system_message.contains("\"loadSession\": true"),
            "expected loadSession capability in system message, got: {system_message:?}"
        );
        assert!(
            system_message.contains("\"image\": true"),
            "expected image capability in system message, got: {system_message:?}"
        );
        assert!(
            system_message.contains("\"audio\": true"),
            "expected audio capability in system message, got: {system_message:?}"
        );
        assert!(!model.acp_panel_active());
    }

    #[test]
    fn acp_debug_panel_down_key_moves_visible_selection() {
        let mut model = Model::new_with_options(
            HeroOptions::default(),
            ModelOptions {
                acp_agent_servers: vec!["kimi".to_string()],
                ..ModelOptions::default()
            },
        );
        model.palette = default_palette();
        model.open_acp_debug_panel();

        model.handle_acp_panel_key(KeyCode::Down.into());

        let lines = build_debug_panel_lines(&model, 96)
            .into_iter()
            .map(|line| {
                line.spans
                    .into_iter()
                    .map(|span| span.content.into_owned())
                    .collect::<String>()
            })
            .collect::<Vec<_>>();
        assert!(
            lines
                .iter()
                .any(|line| line.contains("➜ agent-capabilities-system-msg")),
            "Down should visibly select the capabilities debug item, got: {lines:?}"
        );
    }

    #[test]
    fn acp_debug_panel_aligns_descriptions_independent_of_command_width() {
        let mut model = Model::new_with_options(HeroOptions::default(), ModelOptions::default());
        model.palette = default_palette();
        model.open_acp_debug_panel();

        let lines = build_debug_panel_lines(&model, 120)
            .into_iter()
            .map(|line| {
                line.spans
                    .into_iter()
                    .map(|span| span.content.into_owned())
                    .collect::<String>()
            })
            .collect::<Vec<_>>();
        let protocol_line = lines
            .iter()
            .find(|line| line.contains(ACP_DEBUG_PROTOCOL_VERSION_SYSTEM_MSG))
            .expect("protocol debug item should render");
        let capabilities_line = lines
            .iter()
            .find(|line| line.contains(ACP_DEBUG_AGENT_CAPABILITIES_SYSTEM_MSG))
            .expect("capabilities debug item should render");

        assert_eq!(
            display_column_of(protocol_line, "Append"),
            display_column_of(capabilities_line, "Append"),
            "debug descriptions should start at the same column:\n{protocol_line:?}\n{capabilities_line:?}"
        );
    }

    fn display_column_of(line: &str, needle: &str) -> usize {
        let byte_index = line.find(needle).expect("needle should render");
        line[..byte_index].width()
    }

    #[test]
    fn acp_debug_panel_uses_distinct_selected_and_unselected_styles() {
        let mut model = Model::new_with_options(HeroOptions::default(), ModelOptions::default());
        model.palette = default_palette();
        model.open_acp_debug_panel();

        let lines = build_debug_panel_lines(&model, 120);
        let selected_line = lines
            .iter()
            .find(|line| {
                line.spans
                    .iter()
                    .any(|span| span.content.contains(ACP_DEBUG_PROTOCOL_VERSION_SYSTEM_MSG))
            })
            .expect("selected debug item should render");
        let unselected_line = lines
            .iter()
            .find(|line| {
                line.spans.iter().any(|span| {
                    span.content
                        .contains(ACP_DEBUG_AGENT_CAPABILITIES_SYSTEM_MSG)
                })
            })
            .expect("unselected debug item should render");
        let selected_command = selected_line
            .spans
            .iter()
            .find(|span| span.content.contains(ACP_DEBUG_PROTOCOL_VERSION_SYSTEM_MSG))
            .expect("selected command span should exist");
        let selected_description = selected_line
            .spans
            .iter()
            .find(|span| span.content.contains("Append protocolVersion"))
            .expect("selected description span should exist");
        let unselected_command = unselected_line
            .spans
            .iter()
            .find(|span| {
                span.content
                    .contains(ACP_DEBUG_AGENT_CAPABILITIES_SYSTEM_MSG)
            })
            .expect("unselected command span should exist");
        let unselected_description = unselected_line
            .spans
            .iter()
            .find(|span| span.content.contains("Append selected agent"))
            .expect("unselected description span should exist");

        assert_eq!(
            selected_command.style,
            super::super::super::theme::command_accent_text_style(model.palette).bold()
        );
        assert_eq!(
            selected_description.style,
            primary_text_style(model.palette)
        );
        assert_eq!(
            unselected_command.style,
            secondary_text_style(model.palette)
        );
        assert_eq!(
            unselected_description.style,
            secondary_text_style(model.palette)
        );
    }
}
