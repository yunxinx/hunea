//! 斜杠菜单的状态、过滤与渲染逻辑。

use std::fmt::Write as _;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::text::{Line, Span};

use super::{
    AppEffect, Model, debug,
    display_width::display_width,
    selection::SelectableLineRange,
    status_line::{
        status_line_gap_before, status_line_pair_height, truncate_display_width_with_ellipsis,
    },
    theme::{
        command_accent_text_style, primary_text_style, secondary_text_style, tertiary_text_style,
    },
};

const COMMAND_PANEL_VISIBLE_ROWS: usize = 7;
const COMMAND_PANEL_INSET_WIDTH: usize = 2;
const COMMAND_PANEL_DESCRIPTION_GAP: usize = 4;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum CommandPanelAction {
    Clear,
    Exit,
    OpenModelPanel,
    OpenToolApprovalDebug,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct CommandPanelItem {
    pub(super) name: String,
    pub(super) aliases: Vec<String>,
    pub(super) description: String,
    pub(super) action: CommandPanelAction,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CommandPanelState {
    query: String,
    items: Vec<CommandPanelItem>,
    selected: usize,
    scroll: usize,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct CommandPanelRenderResult {
    pub(crate) lines: Vec<Line<'static>>,
    pub(crate) plain_lines: Vec<String>,
    pub(crate) selectable: Vec<SelectableLineRange>,
    pub(crate) has_content: bool,
}

impl Model {
    pub(crate) fn command_panel_active(&self) -> bool {
        if self.tool_approval_panel_active() {
            return false;
        }

        let Some(query) = raw_command_panel_query(self.composer_text()) else {
            return false;
        };
        !self.filter_command_panel_items(&query).is_empty() || query.chars().count() == 1
    }

    pub(crate) fn sync_command_panel_navigation(&mut self) {
        let Some(state) = self.current_command_panel_state() else {
            self.command_panel_selected = 0;
            self.command_panel_scroll = 0;
            return;
        };

        if state.items.is_empty() {
            self.command_panel_selected = 0;
            self.command_panel_scroll = 0;
            return;
        }

        self.command_panel_selected = state.selected;
        self.command_panel_scroll = state.scroll;
    }

    pub(crate) fn current_inline_command_panel_render_result(&self) -> CommandPanelRenderResult {
        let Some(state) = self.current_command_panel_state() else {
            return CommandPanelRenderResult::default();
        };

        let visible_rows = self.command_panel_list_visible_rows();
        let width = self.command_panel_content_width();
        let (lines, plain_lines, selectable) =
            self.render_command_panel_lines(&state, width, visible_rows);

        CommandPanelRenderResult {
            lines,
            plain_lines,
            selectable,
            has_content: true,
        }
    }

    pub(crate) fn handle_command_panel_key(&mut self, key: KeyEvent) -> Option<Option<AppEffect>> {
        let state = self.current_command_panel_state()?;

        match key.code {
            KeyCode::Up if key.modifiers.is_empty() => {
                if state.items.len() <= 1 {
                    return None;
                }
                self.move_command_panel_selection(-1);
                Some(None)
            }
            KeyCode::Down if key.modifiers.is_empty() => {
                if state.items.len() <= 1 {
                    return None;
                }
                self.move_command_panel_selection(1);
                Some(None)
            }
            KeyCode::Tab if key.modifiers.is_empty() => {
                let item = state.items.get(state.selected)?;
                let completion_text = command_panel_completion_text(item);
                self.complete_command_panel_selection(&completion_text);
                Some(None)
            }
            KeyCode::Enter if key.modifiers.is_empty() => {
                // 命令面板的输入是筛选条件，Enter 执行当前选中项；
                // 子序列匹配出的命令只要已被选中，就和前缀匹配一样可执行。
                let item = state.items.get(state.selected).cloned()?;
                Some(self.execute_command_panel_item(item))
            }
            KeyCode::Char('p') if key.modifiers == KeyModifiers::CONTROL => None,
            _ => None,
        }
    }

    pub(crate) fn command_panel_list_visible_rows(&self) -> usize {
        let viewport_height = self.document_viewport_height();
        if viewport_height == 0 {
            return COMMAND_PANEL_VISIBLE_ROWS;
        }

        let mut available_rows =
            viewport_height.saturating_sub(usize::from(self.composer.full_height()));
        if self.composer_uses_rendered_frame_padding() {
            available_rows = available_rows.saturating_sub(1);
        }

        let status_line = self.current_status_line_render_result();
        let status_line_2 = self.current_status_line_2_render_result();
        available_rows = available_rows.saturating_sub(status_line_pair_height(
            &status_line,
            &status_line_2,
            status_line_gap_before(self.style_mode),
        ));

        COMMAND_PANEL_VISIBLE_ROWS.min(available_rows.max(1))
    }

    fn current_command_panel_state(&self) -> Option<CommandPanelState> {
        if self.tool_approval_panel_active() {
            return None;
        }

        let query = raw_command_panel_query(self.composer_text())?;
        let items = self.filter_command_panel_items(&query);
        if items.is_empty() && query.chars().count() > 1 {
            return None;
        }
        if items.is_empty() {
            return Some(CommandPanelState {
                query,
                items,
                selected: 0,
                scroll: 0,
            });
        }

        let visible_rows = self.command_panel_list_visible_rows();
        let mut selected = self
            .command_panel_selected
            .min(items.len().saturating_sub(1));
        let max_scroll = items.len().saturating_sub(visible_rows);
        let mut scroll = self.command_panel_scroll.min(max_scroll);
        if selected < scroll {
            scroll = selected;
        }
        if selected >= scroll + visible_rows {
            scroll = selected + 1 - visible_rows;
        }
        selected = selected.min(items.len().saturating_sub(1));

        Some(CommandPanelState {
            query,
            items,
            selected,
            scroll,
        })
    }

    fn move_command_panel_selection(&mut self, delta: isize) -> bool {
        let Some(state) = self.current_command_panel_state() else {
            return false;
        };
        if state.items.is_empty() {
            return false;
        }

        let visible_rows = self.command_panel_list_visible_rows();
        let last_index = state.items.len().saturating_sub(1);
        let next_selected = if delta.is_negative() {
            state.selected.saturating_sub(delta.unsigned_abs())
        } else {
            state
                .selected
                .saturating_add(delta as usize)
                .min(last_index)
        };

        let mut next_scroll = state.scroll;
        if next_selected < next_scroll {
            next_scroll = next_selected;
        }
        if next_selected >= next_scroll + visible_rows {
            next_scroll = next_selected + 1 - visible_rows;
        }

        self.command_panel_selected = next_selected;
        self.command_panel_scroll = next_scroll;
        true
    }

    fn render_command_panel_lines(
        &self,
        state: &CommandPanelState,
        width: usize,
        visible_rows: usize,
    ) -> (Vec<Line<'static>>, Vec<String>, Vec<SelectableLineRange>) {
        let width = width.max(1);
        let visible_rows = visible_rows.max(1);
        let mut lines = Vec::with_capacity(visible_rows);
        let mut plain_lines = Vec::with_capacity(visible_rows);
        let mut selectable = Vec::with_capacity(visible_rows);

        if state.items.is_empty() {
            for row in 0..visible_rows {
                if row == 0 {
                    let plain_line = pad_display_width_right("  No commands", width);
                    lines.push(Line::styled(
                        plain_line.clone(),
                        tertiary_text_style(self.palette),
                    ));
                    plain_lines.push(plain_line.clone());
                    selectable.push(command_panel_selectable_range(&plain_line, width));
                    continue;
                }

                lines.push(Line::raw(""));
                plain_lines.push(String::new());
                selectable.push(SelectableLineRange::default());
            }

            return (lines, plain_lines, selectable);
        }

        let command_column_width = command_panel_command_column_width(state, visible_rows);

        for row in 0..visible_rows {
            let index = state.scroll + row;
            if let Some(item) = state.items.get(index) {
                let (line, plain_line, line_selectable) = self.render_command_panel_line(
                    item,
                    index == state.selected,
                    width,
                    command_column_width,
                );
                lines.push(line);
                plain_lines.push(plain_line);
                selectable.push(line_selectable);
                continue;
            }

            lines.push(Line::raw(""));
            plain_lines.push(String::new());
            selectable.push(SelectableLineRange::default());
        }

        (lines, plain_lines, selectable)
    }

    fn render_command_panel_line(
        &self,
        item: &CommandPanelItem,
        selected: bool,
        width: usize,
        command_column_width: usize,
    ) -> (Line<'static>, String, SelectableLineRange) {
        let width = width.max(1);
        let mut remaining_width = width;
        let inset_width = COMMAND_PANEL_INSET_WIDTH.min(remaining_width);
        remaining_width = remaining_width.saturating_sub(inset_width);

        let command_column_width = command_column_width.min(remaining_width);
        let command_text = truncate_display_width_with_ellipsis(&item.name, command_column_width);
        let command_padding_width =
            command_column_width.saturating_sub(display_width(&command_text));
        remaining_width = remaining_width.saturating_sub(command_column_width);

        let gap_width = COMMAND_PANEL_DESCRIPTION_GAP.min(remaining_width);
        let gap_text = " ".repeat(command_padding_width + gap_width);
        remaining_width = remaining_width.saturating_sub(gap_width);

        let description_text =
            truncate_display_width_with_ellipsis(&item.description, remaining_width);
        let mut plain_line = String::new();
        let _ = write!(
            plain_line,
            "{}{}{}{}",
            " ".repeat(inset_width),
            command_text,
            gap_text,
            description_text
        );
        let padding = width.saturating_sub(display_width(&plain_line));
        plain_line.push_str(&" ".repeat(padding));

        let name_style = if selected {
            command_accent_text_style(self.palette).bold()
        } else {
            secondary_text_style(self.palette)
        };
        let description_style = if selected {
            primary_text_style(self.palette)
        } else {
            secondary_text_style(self.palette)
        };

        (
            Line::from(vec![
                Span::raw(" ".repeat(inset_width)),
                Span::styled(command_text.clone(), name_style),
                Span::raw(gap_text),
                Span::styled(description_text, description_style),
                Span::raw(" ".repeat(padding)),
            ]),
            plain_line.clone(),
            command_panel_selectable_range(&plain_line, width),
        )
    }

    fn command_panel_content_width(&self) -> usize {
        usize::from(self.width.max(1))
    }

    fn complete_command_panel_selection(&mut self, next_value: &str) {
        let old_value = self.composer_text().to_string();
        let old_line = self.composer.line();
        let old_column = self.composer.column();
        self.composer
            .replace_text_and_move_to_end(next_value.to_string());
        self.sync_command_panel_navigation();
        self.sync_external_editor_helper_after_draft_change(&old_value);
        self.sync_composer_height();
        self.sync_document_viewport_after_composer_interaction(&old_value, old_line, old_column);
    }

    fn execute_command_panel_item(&mut self, item: CommandPanelItem) -> Option<AppEffect> {
        match item.action {
            CommandPanelAction::Clear => {
                self.reset_to_initial_tui_state();
                Some(AppEffect::ResetRuntimeSession)
            }
            CommandPanelAction::Exit => {
                self.mark_quitting();
                None
            }
            CommandPanelAction::OpenModelPanel => {
                self.open_model_panel();
                None
            }
            CommandPanelAction::OpenToolApprovalDebug => {
                self.open_tool_approval_debug_preview_panel();
                None
            }
        }
    }

    fn filter_command_panel_items(&self, query: &str) -> Vec<CommandPanelItem> {
        let items = filter_base_command_panel_items("", self.debug_commands_enabled);

        if query.is_empty() {
            return items;
        }

        items
            .into_iter()
            .filter(|item| command_panel_item_matches_query(item, query))
            .collect()
    }
}

#[cfg(test)]
fn command_panel_query(value: &str) -> Option<String> {
    if value.is_empty() || !value.starts_with('/') || value.contains('\n') {
        return None;
    }

    let query = raw_command_panel_query(value)?;
    if !filter_base_command_panel_items(&query, false).is_empty() || query.chars().count() == 1 {
        return Some(query);
    }

    None
}

fn command_panel_command_column_width(state: &CommandPanelState, visible_rows: usize) -> usize {
    state
        .items
        .iter()
        .skip(state.scroll)
        .take(visible_rows)
        .map(|item| display_width(&item.name))
        .max()
        .unwrap_or(0)
}

fn raw_command_panel_query(value: &str) -> Option<String> {
    if value.is_empty() || !value.starts_with('/') || value.contains('\n') {
        return None;
    }

    let raw_query = value.trim_start_matches('/');
    if raw_query.is_empty() {
        return Some(String::new());
    }
    if raw_query.chars().any(char::is_whitespace) {
        return None;
    }

    Some(raw_query.to_lowercase())
}

fn filter_base_command_panel_items(
    query: &str,
    debug_commands_enabled: bool,
) -> Vec<CommandPanelItem> {
    let mut items = vec![CommandPanelItem {
        name: "/exit".to_string(),
        aliases: vec!["/quit".to_string()],
        description: "Exit the application".to_string(),
        action: CommandPanelAction::Exit,
    }];

    items.push(CommandPanelItem {
        name: "/models".to_string(),
        aliases: Vec::new(),
        description: "Select model for this session".to_string(),
        action: CommandPanelAction::OpenModelPanel,
    });
    if debug_commands_enabled {
        items.extend(debug::command_panel_items());
    }
    items.push(CommandPanelItem {
        name: "/clear".to_string(),
        aliases: vec!["/new".to_string()],
        description: "Clear conversation context".to_string(),
        action: CommandPanelAction::Clear,
    });

    if query.is_empty() {
        return items;
    }

    items
        .into_iter()
        .filter(|item| command_panel_item_matches_query(item, query))
        .collect()
}

#[cfg(test)]
fn base_command_panel_items_for_query(query: &str) -> Vec<CommandPanelItem> {
    filter_base_command_panel_items(query, false)
}

fn command_panel_completion_text(item: &CommandPanelItem) -> String {
    item.name.clone()
}

fn command_panel_item_matches_query(item: &CommandPanelItem, query: &str) -> bool {
    let primary = item.name.trim_start_matches('/').to_lowercase();
    if normalized_text_matches_subsequence_query(&primary, query) {
        return true;
    }

    item.aliases.iter().any(|alias| {
        let alias_name = alias.trim_start_matches('/').to_lowercase();
        normalized_text_matches_subsequence_query(&alias_name, query)
    })
}

fn normalized_text_matches_subsequence_query(text: &str, query: &str) -> bool {
    let mut query_chars = query.chars();
    let Some(mut expected) = query_chars.next() else {
        return true;
    };

    for character in text.chars() {
        if character != expected {
            continue;
        }

        let Some(next_expected) = query_chars.next() else {
            return true;
        };
        expected = next_expected;
    }

    false
}

fn pad_display_width_right(text: &str, width: usize) -> String {
    let text = truncate_display_width_with_ellipsis(text, width);
    let padding = width.saturating_sub(display_width(&text));
    format!("{text}{}", " ".repeat(padding))
}

fn command_panel_selectable_range(plain_line: &str, width: usize) -> SelectableLineRange {
    let end_column = display_width(plain_line.trim_end());
    if end_column <= COMMAND_PANEL_INSET_WIDTH {
        return SelectableLineRange::blank_hit_range(0, width);
    }

    SelectableLineRange::new(COMMAND_PANEL_INSET_WIDTH, end_column)
}

#[cfg(test)]
mod tests;
