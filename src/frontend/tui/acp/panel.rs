use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    style::Modifier,
    text::{Line, Span},
};

use super::super::{
    AppEffect, Model,
    inline_panel::{
        InlinePanelRenderResult, append_wrapped_inline_value, inline_panel_render_result,
        inline_panel_rule_line, inline_panel_visible_rows,
    },
    theme::{primary_text_style, secondary_text_style, tertiary_text_style},
};

const ACP_LIST_VISIBLE_ROWS: usize = 8;
const ACP_PANEL_VISIBLE_ROWS: usize = 12;
const ACP_DEBUG_LIST_VISIBLE_ROWS: usize = 8;
const ACP_DEBUG_PANEL_VISIBLE_ROWS: usize = 12;
const ACP_DEBUG_PROTOCOL_VERSION_SYSTEM_MSG: &str = "protocolVersion-system-msg";

/// `AcpPanelState` 保存 ACP agent 面板的导航状态。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(in crate::frontend::tui) struct AcpPanelState {
    pub(in crate::frontend::tui) is_open: bool,
    pub(in crate::frontend::tui) selected: usize,
    pub(in crate::frontend::tui) scroll: usize,
}

/// `AcpDebugPanelState` 保存 ACP debug 面板的导航状态。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(in crate::frontend::tui) struct AcpDebugPanelState {
    pub(in crate::frontend::tui) is_open: bool,
    pub(in crate::frontend::tui) selected: usize,
    pub(in crate::frontend::tui) scroll: usize,
}

pub(crate) type AcpPanelRenderResult = InlinePanelRenderResult;

impl Model {
    pub(crate) fn acp_panel_active(&self) -> bool {
        self.acp_panel.is_open || self.acp_debug_panel.is_open
    }

    pub(crate) fn open_acp_panel(&mut self) {
        let old_value = self.composer_text().to_string();
        let old_line = self.composer.line();
        let old_column = self.composer.column();

        self.composer.replace_text_and_move_to_end(String::new());
        self.model_panel.is_open = false;
        if self.tool_approval_panel.is_open {
            self.tool_approval_panel = Default::default();
            self.pending_acp_permission = None;
            self.tool_approval_panel_revision = self.tool_approval_panel_revision.saturating_add(1);
        }
        self.acp_debug_panel = Default::default();
        self.acp_panel.is_open = true;
        self.sync_acp_panel_to_selection();
        self.sync_command_panel_navigation();
        self.sync_external_editor_helper_after_draft_change(&old_value);
        self.sync_composer_height();
        self.sync_document_viewport_after_composer_interaction(&old_value, old_line, old_column);
    }

    pub(crate) fn close_acp_panel(&mut self) {
        if !self.acp_panel.is_open && !self.acp_debug_panel.is_open {
            return;
        }

        self.acp_panel.is_open = false;
        self.acp_debug_panel.is_open = false;
        self.sync_composer_height();
        self.sync_document_viewport_for_composer_cursor();
    }

    pub(crate) fn handle_acp_panel_key(&mut self, key: KeyEvent) -> Option<Option<AppEffect>> {
        if self.acp_debug_panel.is_open {
            return self.handle_acp_debug_panel_key(key);
        }

        if !self.acp_panel.is_open {
            return None;
        }

        match key.code {
            KeyCode::Esc => {
                self.close_acp_panel();
                Some(None)
            }
            KeyCode::Up if key.modifiers.is_empty() => {
                self.move_acp_panel_selection(-1);
                Some(None)
            }
            KeyCode::Down if key.modifiers.is_empty() => {
                self.move_acp_panel_selection(1);
                Some(None)
            }
            KeyCode::Enter if key.modifiers.is_empty() => {
                let effect = self.select_current_acp_panel_agent();
                Some(effect)
            }
            _ => Some(None),
        }
    }

    pub(crate) fn current_inline_acp_panel_render_result(&self) -> AcpPanelRenderResult {
        if self.acp_debug_panel.is_open {
            return self.current_inline_acp_debug_panel_render_result();
        }

        if !self.acp_panel.is_open {
            return AcpPanelRenderResult::default();
        }

        let visible_rows = self.acp_panel_visible_rows();
        let width = usize::from(self.width.max(1));
        let mut lines = build_panel_lines(self, width);
        if lines.len() > visible_rows {
            lines.truncate(visible_rows);
        }
        inline_panel_render_result(lines)
    }

    pub(crate) fn acp_panel_visible_rows(&self) -> usize {
        let panel_rows = if self.acp_debug_panel.is_open {
            ACP_DEBUG_PANEL_VISIBLE_ROWS
        } else {
            ACP_PANEL_VISIBLE_ROWS
        };
        inline_panel_visible_rows(self, panel_rows)
    }

    fn sync_acp_panel_to_selection(&mut self) {
        if self.acp_agent_servers.is_empty() {
            self.acp_panel.selected = 0;
            self.acp_panel.scroll = 0;
            return;
        }

        self.acp_panel.selected = self
            .selected_acp_agent
            .as_ref()
            .and_then(|selected| {
                self.acp_agent_servers
                    .iter()
                    .position(|agent_id| agent_id == selected)
            })
            .unwrap_or(self.acp_panel.selected)
            .min(self.acp_agent_servers.len() - 1);
        self.sync_acp_panel_scroll();
    }

    fn move_acp_panel_selection(&mut self, delta: isize) {
        if self.acp_agent_servers.is_empty() {
            self.acp_panel.selected = 0;
            self.acp_panel.scroll = 0;
            return;
        }

        let last_index = self.acp_agent_servers.len() - 1;
        self.acp_panel.selected = if delta.is_negative() {
            self.acp_panel.selected.saturating_sub(delta.unsigned_abs())
        } else {
            self.acp_panel
                .selected
                .saturating_add(delta as usize)
                .min(last_index)
        };
        self.sync_acp_panel_scroll();
    }

    fn sync_acp_panel_scroll(&mut self) {
        if self.acp_agent_servers.is_empty() {
            self.acp_panel.scroll = 0;
            return;
        }

        let selected = self
            .acp_panel
            .selected
            .min(self.acp_agent_servers.len() - 1);
        let max_scroll = self
            .acp_agent_servers
            .len()
            .saturating_sub(ACP_LIST_VISIBLE_ROWS);
        let mut scroll = self.acp_panel.scroll.min(max_scroll);
        if selected < scroll {
            scroll = selected;
        }
        if selected >= scroll + ACP_LIST_VISIBLE_ROWS {
            scroll = selected + 1 - ACP_LIST_VISIBLE_ROWS;
        }
        self.acp_panel.selected = selected;
        self.acp_panel.scroll = scroll;
    }

    fn select_current_acp_panel_agent(&mut self) -> Option<AppEffect> {
        let agent_id = self.acp_agent_servers.get(self.acp_panel.selected)?.clone();
        self.selected_acp_agent = Some(agent_id.clone());
        self.activate_acp_model_scope(&agent_id);
        self.show_transient_status_notice(&format!("ACP agent selected: {agent_id}"));
        self.close_acp_panel();
        Some(AppEffect::StartAcpSession { agent_id })
    }

    pub(crate) fn open_acp_debug_panel(&mut self) {
        let old_value = self.composer_text().to_string();
        let old_line = self.composer.line();
        let old_column = self.composer.column();

        self.composer.replace_text_and_move_to_end(String::new());
        self.model_panel.is_open = false;
        self.acp_panel = Default::default();
        if self.tool_approval_panel.is_open {
            self.tool_approval_panel = Default::default();
            self.pending_acp_permission = None;
            self.tool_approval_panel_revision = self.tool_approval_panel_revision.saturating_add(1);
        }
        self.acp_debug_panel.is_open = true;
        self.sync_acp_debug_panel_scroll();
        self.sync_command_panel_navigation();
        self.sync_external_editor_helper_after_draft_change(&old_value);
        self.sync_composer_height();
        self.sync_document_viewport_after_composer_interaction(&old_value, old_line, old_column);
    }

    fn handle_acp_debug_panel_key(&mut self, key: KeyEvent) -> Option<Option<AppEffect>> {
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

    fn sync_acp_debug_panel_scroll(&mut self) {
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

    fn select_current_acp_debug_panel_item(&mut self) {
        let Some(item) = acp_debug_items().get(self.acp_debug_panel.selected) else {
            return;
        };

        if item.name == ACP_DEBUG_PROTOCOL_VERSION_SYSTEM_MSG {
            self.close_acp_panel();
            self.append_system_message_from_runtime(
                crate::runtime::acp::debug_protocol_version_system_message(),
            );
        }
    }

    fn current_inline_acp_debug_panel_render_result(&self) -> AcpPanelRenderResult {
        let visible_rows = self.acp_panel_visible_rows();
        let width = usize::from(self.width.max(1));
        let mut lines = build_debug_panel_lines(self, width);
        if lines.len() > visible_rows {
            lines.truncate(visible_rows);
        }
        inline_panel_render_result(lines)
    }
}

fn build_panel_lines(model: &Model, width: usize) -> Vec<Line<'static>> {
    let width = width.max(1);
    let mut lines = vec![
        inline_panel_rule_line(width, model.palette),
        acp_header_line(model),
        Line::raw(""),
    ];
    lines.push(Line::styled(
        "  Available Agents:",
        secondary_text_style(model.palette).bold(),
    ));
    append_agent_lines(model, width, &mut lines);
    lines.push(Line::raw(""));
    lines.push(Line::styled(
        "  Press Enter to select · Esc to exit · ↑↓ to navigate",
        tertiary_text_style(model.palette).add_modifier(Modifier::ITALIC),
    ));

    lines
}

fn acp_header_line(model: &Model) -> Line<'static> {
    Line::from(vec![
        Span::raw("  "),
        Span::styled("ACP Agents:", primary_text_style(model.palette)),
    ])
}

fn append_agent_lines(model: &Model, width: usize, lines: &mut Vec<Line<'static>>) {
    if model.acp_agent_servers.is_empty() {
        lines.push(Line::styled(
            "  No ACP agents configured",
            tertiary_text_style(model.palette),
        ));
        return;
    }

    let start = model.acp_panel.scroll;
    let end = (start + ACP_LIST_VISIBLE_ROWS).min(model.acp_agent_servers.len());
    for (offset, agent_id) in model.acp_agent_servers[start..end].iter().enumerate() {
        let index = start + offset;
        append_agent_entry_lines(
            model,
            agent_id,
            index == model.acp_panel.selected,
            width,
            lines,
        );
    }
}

fn append_agent_entry_lines(
    model: &Model,
    agent_id: &str,
    selected: bool,
    width: usize,
    lines: &mut Vec<Line<'static>>,
) {
    let marker = if selected { "➜ " } else { "  " };
    let style = if selected {
        primary_text_style(model.palette).bold()
    } else {
        primary_text_style(model.palette)
    };

    append_wrapped_inline_value(
        lines,
        width,
        marker,
        agent_id,
        style,
        secondary_text_style(model.palette),
    );
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct AcpDebugItem {
    name: &'static str,
    description: &'static str,
}

fn acp_debug_items() -> &'static [AcpDebugItem] {
    &[AcpDebugItem {
        name: ACP_DEBUG_PROTOCOL_VERSION_SYSTEM_MSG,
        description: "Append protocolVersion warning system message",
    }]
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
    for (offset, item) in items[start..end].iter().enumerate() {
        let index = start + offset;
        append_acp_debug_item_entry_lines(
            model,
            item,
            index == model.acp_debug_panel.selected,
            width,
            lines,
        );
    }
}

fn append_acp_debug_item_entry_lines(
    model: &Model,
    item: &AcpDebugItem,
    selected: bool,
    width: usize,
    lines: &mut Vec<Line<'static>>,
) {
    let marker = if selected { "➜ " } else { "  " };
    let style = if selected {
        primary_text_style(model.palette).bold()
    } else {
        primary_text_style(model.palette)
    };
    let label = format!("{}  {}", item.name, item.description);

    append_wrapped_inline_value(
        lines,
        width,
        marker,
        &label,
        style,
        secondary_text_style(model.palette),
    );
}

#[cfg(test)]
mod tests {
    use crate::frontend::tui::{HeroOptions, ModelOptions, theme::default_palette};
    use crate::runtime::acp::AcpAgentIdentity;

    use super::*;

    #[test]
    fn acp_panel_agent_entries_do_not_append_current_suffix() {
        let mut model = Model::new_with_options(
            HeroOptions::default(),
            ModelOptions {
                acp_agent_servers: vec!["codex-acp".to_string()],
                ..ModelOptions::default()
            },
        );
        model.palette = default_palette();
        model.selected_acp_agent = Some("codex-acp".to_string());
        model.open_acp_panel();

        let lines = build_panel_lines(&model, 72)
            .into_iter()
            .map(|line| {
                line.spans
                    .into_iter()
                    .map(|span| span.content.into_owned())
                    .collect::<String>()
            })
            .collect::<Vec<_>>();

        assert!(
            lines.iter().any(|line| line.contains("codex-acp")),
            "selected ACP agent should still render in the launcher: {lines:?}"
        );
        assert!(
            lines.iter().all(|line| !line.contains("current")),
            "ACP launcher should not append a current suffix: {lines:?}"
        );
    }

    #[test]
    fn acp_panel_agent_entry_keeps_configured_agent_id_after_initialize() {
        let mut model = Model::new_with_options(
            HeroOptions::default(),
            ModelOptions {
                acp_agent_servers: vec!["kimi".to_string()],
                ..ModelOptions::default()
            },
        );
        model.palette = default_palette();
        model.apply_acp_agent_identity(
            "kimi",
            AcpAgentIdentity {
                name: Some("kimi".to_string()),
                title: Some("Kimi Code CLI".to_string()),
                version: Some("1.39.0".to_string()),
            },
        );
        model.open_acp_panel();

        let lines = build_panel_lines(&model, 72)
            .into_iter()
            .map(|line| {
                line.spans
                    .into_iter()
                    .map(|span| span.content.into_owned())
                    .collect::<String>()
            })
            .collect::<Vec<_>>();

        assert!(
            lines.iter().any(|line| line.contains("kimi")),
            "expected configured ACP agent id, got: {lines:?}"
        );
        assert!(
            lines.iter().all(|line| !line.contains("1.39.0")),
            "ACP picker should not show runtime agent version: {lines:?}"
        );
    }
}
