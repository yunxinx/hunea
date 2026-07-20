//! 斜杠菜单的状态、过滤与渲染逻辑。

use std::fmt::Write as _;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton};
use ratatui::{
    layout::Rect,
    text::{Line, Span},
    widgets::{Clear, Padding, Paragraph},
};

use super::{
    AppEffect, CommandMenuMode, EscRewindMode, Model, debug,
    display_width::display_width,
    overlay_input_result::OverlayInputResult,
    picker_scrollbar::PickerScrollbar,
    render_frame::RenderFrame,
    search_highlight::{
        highlighted_subsequence_spans, search_match_style, subsequence_match_score,
    },
    selection::SelectableLineRange,
    status_line::{
        status_line_gap_before, status_line_pair_height, truncate_display_width_with_ellipsis,
    },
    theme::{
        command_accent_text_style, panel_block, primary_text_style, secondary_text_style,
        tertiary_text_style,
    },
};

const COMMAND_PANEL_VISIBLE_ROWS: usize = 7;
const COMMAND_PANEL_INSET_WIDTH: usize = 2;
const COMMAND_PANEL_DESCRIPTION_GAP: usize = 4;
/// 悬浮命令菜单一次可见命令行数的下限，与内联斜杠菜单默认密度一致。
///
/// 与 `app-config` 的 `COMMAND_MENU_ROWS_*` 数值必须保持一致；
/// `terminal-app` 有同步测试防止两边漂移。
pub const COMMAND_MENU_ROWS_MIN: u16 = 7;
/// 悬浮命令菜单一次可见命令行数的上限，避免遮挡过多上下文。
pub const COMMAND_MENU_ROWS_MAX: u16 = 21;
/// 悬浮命令菜单一次可见命令行数的默认值。
pub const COMMAND_MENU_ROWS_DEFAULT: u16 = 7;
/// 居中命令弹窗的边框 chrome：水平（左右各 1）与垂直（上下各 1）均为 2。
const FLOATING_COMMAND_MENU_BORDER_CHROME: usize = 2;
/// 命令弹窗上下各预留的窗口留白，避免贴边顶到屏幕边界。
const FLOATING_COMMAND_MENU_VERTICAL_MARGIN: usize = 2;
/// 弹窗内容左侧留白，使命令与标题 `Commands` 的 `C` 左对齐（各含 2 空格前导）。
const FLOATING_COMMAND_MENU_LEFT_MARGIN: usize = 2;
/// 弹窗内容右侧留白。
const FLOATING_COMMAND_MENU_RIGHT_MARGIN: usize = 2;
/// 命令名与描述之间的间隔。
const FLOATING_COMMAND_MENU_NAME_GAP: usize = 2;
/// 列表上方固定占用的表头行数：内联输入行 + 分割线。
const FLOATING_COMMAND_MENU_HEADER_ROWS: usize = 2;
/// 弹窗标题，保持 1 空格前导的原始设计。
const FLOATING_COMMAND_MENU_TITLE: &str = " Commands ";
/// 无匹配命令时的占位文案。
const FLOATING_COMMAND_MENU_EMPTY_LABEL: &str = "No commands";
/// 空查询时的输入框占位提示，告知用户可键入筛选。
const FLOATING_COMMAND_MENU_INPUT_PLACEHOLDER: &str = "type to filter";
/// 输入框中标记当前光标位置的细竖条 caret。
const FLOATING_COMMAND_MENU_INPUT_CARET: &str = "▏";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum CommandPanelAction {
    Clear,
    Exit,
    OpenResumePicker,
    OpenCopyPicker,
    OpenPromptOverlay,
    OpenContextBudget,
    OpenMessageHistory,
    OpenEntryRewind,
    OpenCoarseRewind,
    OpenModelPanel,
    OpenToolApprovalDebug,
}

impl CommandPanelAction {
    /// 内联执行后是否清空 composer 中承载的命令文本。
    ///
    /// `Clear` 会整体重置 TUI（含 composer），`Exit` 直接退出，均无需单独清理；
    /// 其余动作打开的面板 / picker 均不拥有 composer 文本，命令文本由此统一清空。
    fn clears_inline_command_query(&self) -> bool {
        !matches!(self, Self::Clear | Self::Exit)
    }
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

/// `FloatingCommandMenuState` 保存 `Ctrl+O` 悬浮命令菜单的查询与导航位置。
///
/// 与内联斜杠菜单不同，它拥有独立的查询缓冲（不复用 composer 文本），
/// 打开后独占键盘输入用于过滤与导航，粘贴文本同样进入查询而非 composer。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct FloatingCommandMenuState {
    query: String,
    selected: usize,
    scroll: usize,
}

/// 居中命令弹窗的几何布局，渲染与鼠标命中检测共用同一来源。
///
/// `popup_area` 为含边框的整体区域，`inner_area` 去掉四周边框，
/// `list_top` 是首条命令行的绝对行坐标，`list_rows` 为固定视口行数。
struct FloatingCommandMenuGeometry {
    popup_area: Rect,
    inner_area: Rect,
    list_top: u16,
    list_rows: usize,
    inner_width: usize,
    name_column: usize,
    description_column: usize,
}

impl Model {
    pub(crate) fn command_panel_active(&self) -> bool {
        // both 模式下悬浮菜单打开时独占命令 UI，避免与内联斜杠面板叠层。
        if self.floating_command_menu_active() {
            return false;
        }
        if !self.slash_command_menu_enabled() {
            return false;
        }
        if self.blocks_composer_input() {
            return false;
        }

        let Some(query) = raw_command_panel_query(self.composer_text()) else {
            return false;
        };
        if self.dismissed_command_panel_query.as_ref() == Some(&query) {
            return false;
        }
        !self.filter_command_panel_items(&query).is_empty() || query.chars().count() == 1
    }

    pub(crate) fn sync_command_panel_navigation(&mut self) {
        if raw_command_panel_query(self.composer_text()).is_none() {
            self.dismissed_command_panel_query = None;
        }
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

    pub(crate) fn handle_command_panel_key(&mut self, key: KeyEvent) -> OverlayInputResult {
        let Some(state) = self.current_command_panel_state() else {
            return OverlayInputResult::Ignored;
        };

        match key.code {
            KeyCode::Esc if key.modifiers.is_empty() => {
                self.dismissed_command_panel_query = Some(state.query);
                self.command_panel_selected = 0;
                self.command_panel_scroll = 0;
                OverlayInputResult::Handled
            }
            KeyCode::Up if key.modifiers.is_empty() => {
                if state.items.len() <= 1 {
                    return OverlayInputResult::Ignored;
                }
                self.move_command_panel_selection(-1);
                OverlayInputResult::Handled
            }
            KeyCode::Down if key.modifiers.is_empty() => {
                if state.items.len() <= 1 {
                    return OverlayInputResult::Ignored;
                }
                self.move_command_panel_selection(1);
                OverlayInputResult::Handled
            }
            KeyCode::Tab if key.modifiers.is_empty() => {
                let Some(item) = state.items.get(state.selected) else {
                    return OverlayInputResult::Ignored;
                };
                let completion_text = command_panel_completion_text(item);
                self.complete_command_panel_selection(&completion_text);
                OverlayInputResult::Handled
            }
            KeyCode::Enter if key.modifiers.is_empty() => {
                // 命令面板的输入是筛选条件，Enter 执行当前选中项；
                // 子序列匹配出的命令只要已被选中，就和前缀匹配一样可执行。
                let Some(item) = state.items.get(state.selected).cloned() else {
                    return OverlayInputResult::Ignored;
                };
                OverlayInputResult::from_effect(self.execute_command_panel_item(item))
            }
            KeyCode::Char('p') if key.modifiers == KeyModifiers::CONTROL => {
                OverlayInputResult::Ignored
            }
            _ => OverlayInputResult::Ignored,
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
        if self.floating_command_menu_active() {
            return None;
        }
        if !self.slash_command_menu_enabled() {
            return None;
        }
        if self.blocks_composer_input() {
            return None;
        }

        let query = raw_command_panel_query(self.composer_text())?;
        if self.dismissed_command_panel_query.as_ref() == Some(&query) {
            return None;
        }
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
                    &state.query,
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
        query: &str,
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
        let highlighted_name_style = search_match_style(name_style, self.palette.surface);
        let description_style = if selected {
            primary_text_style(self.palette)
        } else {
            secondary_text_style(self.palette)
        };
        let mut spans = vec![Span::raw(" ".repeat(inset_width))];
        spans.extend(highlighted_subsequence_spans(
            &command_text,
            query,
            name_style,
            highlighted_name_style,
        ));
        spans.push(Span::raw(gap_text));
        spans.push(Span::styled(description_text, description_style));
        spans.push(Span::raw(" ".repeat(padding)));

        (
            Line::from(spans),
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
            .replace_text_and_move_to_end_for_edit(next_value.to_string());
        self.sync_command_panel_navigation();
        self.sync_external_editor_helper_after_draft_change(&old_value);
        self.sync_composer_height();
        self.sync_document_viewport_after_composer_interaction(&old_value, old_line, old_column);
    }

    /// 内联斜杠菜单的执行入口：composer 中承载的是命令文本本身，
    /// 随执行一并清空（按动作语义豁免，见 `clears_inline_command_query`）。
    fn execute_command_panel_item(&mut self, item: CommandPanelItem) -> Option<AppEffect> {
        if item.action.clears_inline_command_query() {
            self.clear_inline_command_query();
        }
        self.dispatch_command_panel_action(item.action)
    }

    /// 清空 composer 中的内联命令文本并同步派生 UI 状态。
    ///
    /// 仅供内联执行路径使用；悬浮菜单查询独立于 composer，不得借此触碰用户草稿。
    /// 同步集与 `complete_command_panel_selection` 一致：命令文本变化同样要
    /// 刷新 external editor helper 与文档 viewport。
    fn clear_inline_command_query(&mut self) {
        let old_value = self.composer_text().to_string();
        let old_line = self.composer.line();
        let old_column = self.composer.column();
        self.composer_mut().clear();
        self.sync_command_panel_navigation();
        self.sync_composer_attached_picker_state();
        self.sync_external_editor_helper_after_draft_change(&old_value);
        self.sync_composer_height();
        self.sync_document_viewport_after_composer_interaction(&old_value, old_line, old_column);
    }

    /// 派发命令动作本身，不触碰 composer；内联与悬浮菜单共用。
    fn dispatch_command_panel_action(&mut self, action: CommandPanelAction) -> Option<AppEffect> {
        match action {
            CommandPanelAction::Clear => {
                self.reset_to_initial_tui_state();
                Some(AppEffect::ResetRuntimeSession)
            }
            CommandPanelAction::Exit => {
                self.mark_quitting();
                None
            }
            CommandPanelAction::OpenResumePicker => Some(AppEffect::OpenResumePicker),
            CommandPanelAction::OpenContextBudget => Some(AppEffect::OpenContextBudget),
            CommandPanelAction::OpenCopyPicker => Some(AppEffect::OpenCopyPicker),
            CommandPanelAction::OpenPromptOverlay => {
                self.open_prompt_overlay();
                Some(AppEffect::BeginPromptAssemblyEdit)
            }
            CommandPanelAction::OpenMessageHistory => Some(AppEffect::OpenMessageHistory),
            CommandPanelAction::OpenEntryRewind => Some(AppEffect::OpenEntryRewind),
            CommandPanelAction::OpenCoarseRewind => self.open_coarse_rewind_from_command(),
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
        let items = all_command_panel_items(self.debug_commands_enabled, self.esc_rewind_mode);
        rank_command_panel_items(items, query)
    }
}

impl Model {
    /// `/` 内联斜杠菜单是否启用（`Slash` / `Both`）。
    pub(crate) fn slash_command_menu_enabled(&self) -> bool {
        matches!(
            self.command_menu_mode,
            CommandMenuMode::Slash | CommandMenuMode::Both
        )
    }

    /// `Ctrl+O` 悬浮命令菜单是否启用（`Floating` / `Both`）。
    pub(crate) fn floating_command_menu_enabled(&self) -> bool {
        matches!(
            self.command_menu_mode,
            CommandMenuMode::Floating | CommandMenuMode::Both
        )
    }

    pub(crate) fn floating_command_menu_active(&self) -> bool {
        self.floating_command_menu.is_some()
    }

    pub(crate) fn open_floating_command_menu(&mut self) {
        if !self.floating_command_menu_enabled() || self.blocks_composer_input() {
            return;
        }
        // composer 附属浮层（file/skill/custom prompt picker）与悬浮菜单互斥：
        // 菜单打开后独占键盘输入，保留它们只会冻结在下层。
        self.close_composer_attached_ui();
        self.floating_command_menu = Some(FloatingCommandMenuState::default());
    }

    pub(crate) fn close_floating_command_menu(&mut self) {
        self.floating_command_menu = None;
    }

    pub(crate) fn toggle_floating_command_menu(&mut self) {
        if self.floating_command_menu.is_some() {
            self.close_floating_command_menu();
        } else {
            self.open_floating_command_menu();
        }
    }

    /// 悬浮菜单打开期间独占键盘输入：识别的键做过滤与导航，其余一律消费，
    /// 防止编辑键（Delete / 方向键 / Shift+Enter 等）落入下层 composer 或文档。
    /// 更早的全局分支（模态层、Ctrl+C 退出确认等）不受影响。
    pub(crate) fn handle_floating_command_menu_key(&mut self, key: KeyEvent) -> OverlayInputResult {
        if self.floating_command_menu.is_none() {
            return OverlayInputResult::Ignored;
        }

        match key.code {
            KeyCode::Esc if key.modifiers.is_empty() => {
                self.close_floating_command_menu();
                OverlayInputResult::Handled
            }
            KeyCode::Char('o') if is_ctrl_only(key.modifiers) => {
                self.close_floating_command_menu();
                OverlayInputResult::Handled
            }
            KeyCode::Up if key.modifiers.is_empty() => {
                self.move_floating_command_menu_selection(-1);
                OverlayInputResult::Handled
            }
            KeyCode::Down if key.modifiers.is_empty() => {
                self.move_floating_command_menu_selection(1);
                OverlayInputResult::Handled
            }
            KeyCode::Backspace if key.modifiers.is_empty() => {
                self.edit_floating_command_menu_query(|query| {
                    query.pop();
                });
                OverlayInputResult::Handled
            }
            KeyCode::Tab if key.modifiers.is_empty() => {
                let Some(state) = self.resolved_floating_command_menu_state() else {
                    return OverlayInputResult::Handled;
                };
                // 悬浮菜单 query 独立于 composer：Tab 只把选中命令名写入过滤框，
                // 与内联路径 complete 到 composer 的语义对齐（补全而非执行）。
                let Some(completion) = state
                    .items
                    .get(state.selected)
                    .map(|item| floating_command_menu_display_name(&item.name).to_string())
                else {
                    return OverlayInputResult::Handled;
                };
                self.edit_floating_command_menu_query(|query| {
                    *query = completion;
                });
                OverlayInputResult::Handled
            }
            KeyCode::Enter if key.modifiers.is_empty() => {
                let Some(state) = self.resolved_floating_command_menu_state() else {
                    return OverlayInputResult::Handled;
                };
                // 只取动作派发：composer 中可能是用户草稿，悬浮路径不得触碰。
                let Some(action) = state
                    .items
                    .get(state.selected)
                    .map(|item| item.action.clone())
                else {
                    return OverlayInputResult::Handled;
                };
                self.close_floating_command_menu();
                OverlayInputResult::from_effect(self.dispatch_command_panel_action(action))
            }
            KeyCode::Char(c)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                self.edit_floating_command_menu_query(|query| {
                    query.push(c);
                });
                OverlayInputResult::Handled
            }
            _ => OverlayInputResult::Handled,
        }
    }

    /// 粘贴文本进入过滤查询而非下层 composer；换行等控制字符不参与命令名匹配，
    /// 直接剔除。即使剔除后为空也消费该事件，保持菜单打开期间的输入独占。
    pub(crate) fn handle_floating_command_menu_paste(&mut self, text: &str) -> OverlayInputResult {
        if self.floating_command_menu.is_none() {
            return OverlayInputResult::Ignored;
        }

        let sanitized: String = text.chars().filter(|c| !c.is_control()).collect();
        if !sanitized.is_empty() {
            self.edit_floating_command_menu_query(|query| query.push_str(&sanitized));
        }
        OverlayInputResult::Handled
    }

    /// 滚轮在菜单打开期间作用于命令列表：`MouseWheel` 事件不携带坐标，
    /// 菜单即当前焦点面，统一消费以免滚动下层文档。与其他 picker 一致，
    /// 每次滚动移动一格选中项，由选中项带动视口滚动。
    pub(crate) fn handle_floating_command_menu_mouse_wheel(
        &mut self,
        delta_lines: isize,
    ) -> OverlayInputResult {
        if self.floating_command_menu.is_none() {
            return OverlayInputResult::Ignored;
        }

        self.move_floating_command_menu_selection(delta_lines.signum());
        OverlayInputResult::Handled
    }

    /// 菜单打开期间统一吞掉 MouseUp / MouseDrag，与 MouseDown 的拦截对称：
    /// 菜单没有拖拽交互，抬起与拖拽不应驱动下层文档选区
    /// （含打开菜单前已处于拖拽中的边缘情况）。语义对齐模态层的
    /// `handle_overlay_pointer_passthrough_blocker`。
    pub(crate) fn handle_floating_command_menu_pointer_blocker(&self) -> OverlayInputResult {
        if self.floating_command_menu_active() {
            OverlayInputResult::Handled
        } else {
            OverlayInputResult::Ignored
        }
    }

    /// 修改查询后，selected/scroll 归零，让第一个匹配项高亮。
    fn edit_floating_command_menu_query(&mut self, edit: impl FnOnce(&mut String)) {
        if let Some(menu) = self.floating_command_menu.as_mut() {
            edit(&mut menu.query);
            menu.selected = 0;
            menu.scroll = 0;
        }
    }

    fn move_floating_command_menu_selection(&mut self, delta: isize) {
        let all_items = self.floating_command_menu_all_items();
        let visible_rows = self.floating_command_menu_list_rows(all_items.len());
        let Some(state) = self.resolved_floating_command_menu_state_with(all_items) else {
            return;
        };
        if state.items.is_empty() {
            return;
        }

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

        if let Some(menu) = self.floating_command_menu.as_mut() {
            menu.selected = next_selected;
            menu.scroll = next_scroll;
        }
    }

    /// 依据当前查询过滤命令并 clamp selected/scroll，得到可渲染的 `CommandPanelState`。
    ///
    /// 查询统一小写，与内联菜单的匹配/高亮语义保持一致。
    fn resolved_floating_command_menu_state(&self) -> Option<CommandPanelState> {
        self.resolved_floating_command_menu_state_with(self.floating_command_menu_all_items())
    }

    /// 同 `resolved_floating_command_menu_state`，但由调用方传入已构建的全部命令，
    /// 供同一事件内与几何计算共享，避免重复重建命令表。
    fn resolved_floating_command_menu_state_with(
        &self,
        all_items: Vec<CommandPanelItem>,
    ) -> Option<CommandPanelState> {
        let menu = self.floating_command_menu.as_ref()?;
        let visible_rows = self.floating_command_menu_list_rows(all_items.len());
        let query = menu.query.to_lowercase();
        let items = rank_command_panel_items(all_items, &query);
        if items.is_empty() {
            return Some(CommandPanelState {
                query,
                items,
                selected: 0,
                scroll: 0,
            });
        }

        let selected = menu.selected.min(items.len().saturating_sub(1));
        let max_scroll = items.len().saturating_sub(visible_rows);
        let mut scroll = menu.scroll.min(max_scroll);
        if selected < scroll {
            scroll = selected;
        }
        if selected >= scroll + visible_rows {
            scroll = selected + 1 - visible_rows;
        }

        Some(CommandPanelState {
            query,
            items,
            selected,
            scroll,
        })
    }

    /// 全部命令条目（不随查询过滤），用于计算固定的弹窗尺寸。
    fn floating_command_menu_all_items(&self) -> Vec<CommandPanelItem> {
        all_command_panel_items(self.debug_commands_enabled, self.esc_rewind_mode)
    }

    /// 命令列表的固定视口行数：由全部命令数、窗口高度与配置的行数上限共同决定，
    /// 不随查询变化，作为导航滚动与渲染共用来源，避免筛选造成高度跳动。至少 1 行。
    fn floating_command_menu_list_rows(&self, total_commands: usize) -> usize {
        let total = total_commands.max(1);
        let reserved = FLOATING_COMMAND_MENU_BORDER_CHROME
            + FLOATING_COMMAND_MENU_VERTICAL_MARGIN
            + FLOATING_COMMAND_MENU_HEADER_ROWS;
        let available = usize::from(self.height).saturating_sub(reserved).max(1);
        let configured = usize::from(self.command_menu_rows).max(1);
        total.min(available).min(configured)
    }

    /// 居中弹窗的几何布局：渲染与鼠标命中检测共用同一来源，保证两者一致。
    ///
    /// 尺寸只依赖全部命令与窗口（不随查询变化），在 `area` 内水平垂直居中。
    #[cfg(test)]
    fn floating_command_menu_geometry(&self, area: Rect) -> Option<FloatingCommandMenuGeometry> {
        self.floating_command_menu_geometry_with(area, &self.floating_command_menu_all_items())
    }

    /// 同 `floating_command_menu_geometry`，但由调用方传入已构建的全部命令，
    /// 供同一事件内与过滤态计算共享，避免重复重建命令表。
    fn floating_command_menu_geometry_with(
        &self,
        area: Rect,
        all_items: &[CommandPanelItem],
    ) -> Option<FloatingCommandMenuGeometry> {
        if area.is_empty() || !self.floating_command_menu_active() {
            return None;
        }

        let list_rows = self.floating_command_menu_list_rows(all_items.len());
        let (name_column, description_column) = floating_command_menu_metrics(all_items);
        let content_width = FLOATING_COMMAND_MENU_LEFT_MARGIN
            + name_column
            + FLOATING_COMMAND_MENU_NAME_GAP
            + description_column
            + FLOATING_COMMAND_MENU_RIGHT_MARGIN;
        let title_width = display_width(FLOATING_COMMAND_MENU_TITLE);
        let inner_max_width = usize::from(area.width)
            .saturating_sub(FLOATING_COMMAND_MENU_BORDER_CHROME)
            .max(1);
        let inner_width = content_width.max(title_width).min(inner_max_width);
        let total_rows = FLOATING_COMMAND_MENU_HEADER_ROWS + list_rows;

        let border = u16::try_from(FLOATING_COMMAND_MENU_BORDER_CHROME).unwrap_or(2);
        let header = u16::try_from(FLOATING_COMMAND_MENU_HEADER_ROWS).unwrap_or(2);
        let width = u16::try_from(inner_width + FLOATING_COMMAND_MENU_BORDER_CHROME)
            .unwrap_or(u16::MAX)
            .min(area.width);
        let height = u16::try_from(total_rows + FLOATING_COMMAND_MENU_BORDER_CHROME)
            .unwrap_or(u16::MAX)
            .min(area.height);
        let x = area.x + area.width.saturating_sub(width) / 2;
        let y = area.y + area.height.saturating_sub(height) / 2;
        let popup_area = Rect::new(x, y, width, height);
        // padding 归零，内框仅去掉四周边框各 1。
        let inner_area = Rect::new(
            popup_area.x.saturating_add(1),
            popup_area.y.saturating_add(1),
            popup_area.width.saturating_sub(border),
            popup_area.height.saturating_sub(border),
        );
        let list_top = inner_area.y.saturating_add(header);

        Some(FloatingCommandMenuGeometry {
            popup_area,
            inner_area,
            list_top,
            list_rows,
            inner_width,
            name_column,
            description_column,
        })
    }

    /// 把 `Ctrl+O` 命令菜单渲染为窗口居中的带边框弹窗。
    ///
    /// 采用与 `/prompt` 界面 `?` which-key 弹窗一致的设计：`panel_block` 圆角边框
    /// 容器 + `Clear` 擦除；水平 padding 归零使命令与标题 `Commands` 对齐。
    /// 顶部为内联输入行（含 caret 与占位提示）+ 一条分割线与命令列表区隔，
    /// 随后固定列宽的命令列表（去掉 `/`），尺寸只依赖全部命令与窗口，
    /// 查询过滤不改变宽高。
    pub(crate) fn render_floating_command_menu(&self, frame: &mut RenderFrame<'_>, area: Rect) {
        let all_items = self.floating_command_menu_all_items();
        let Some(geometry) = self.floating_command_menu_geometry_with(area, &all_items) else {
            return;
        };
        let Some(state) = self.resolved_floating_command_menu_state_with(all_items) else {
            return;
        };
        let Some(typed_query) = self
            .floating_command_menu
            .as_ref()
            .map(|menu| menu.query.clone())
        else {
            return;
        };

        let inner_width = geometry.inner_width;
        let mut lines = Vec::with_capacity(geometry.list_rows + FLOATING_COMMAND_MENU_HEADER_ROWS);
        lines.push(self.floating_command_menu_input_line(&typed_query, inner_width));
        lines.push(self.floating_command_menu_divider_line(inner_width));
        for row in 0..geometry.list_rows {
            let index = state.scroll + row;
            if let Some(item) = state.items.get(index) {
                lines.push(self.floating_command_menu_command_line(
                    item,
                    &state.query,
                    index == state.selected,
                    geometry.name_column,
                    geometry.description_column,
                    inner_width,
                ));
            } else if state.items.is_empty() && row == 0 {
                lines.push(self.floating_command_menu_empty_line(inner_width));
            } else {
                lines.push(floating_command_menu_blank_line(inner_width));
            }
        }

        let block = panel_block(self.palette)
            .padding(Padding::ZERO)
            .title(FLOATING_COMMAND_MENU_TITLE);
        frame.render_widget(Clear, geometry.popup_area);
        frame.render_widget(block, geometry.popup_area);
        frame.render_widget(Paragraph::new(lines), geometry.inner_area);

        // 过滤结果超出可见行数时，把滚动条画在弹窗右侧边框列上（覆盖该段边框），
        // 而非内联进内容区，使内容左右留白保持一致的 2 cell。
        if state.items.len() > geometry.list_rows && geometry.popup_area.width > 0 {
            let scrollbar_area = Rect::new(
                geometry.popup_area.right().saturating_sub(1),
                geometry.list_top,
                1,
                u16::try_from(geometry.list_rows).unwrap_or(u16::MAX),
            );
            PickerScrollbar::new(
                state.items.len(),
                geometry.list_rows,
                state.scroll,
                secondary_text_style(self.palette),
                tertiary_text_style(self.palette),
            )
            .render(scrollbar_area, frame.buffer_mut());
        }
    }

    /// 处理悬浮命令菜单的左键点击：点内部命令行选中该项（形如 branch picker），
    /// 点浮窗外部关闭菜单（形如 `?` 快捷键弹窗）；浮窗内的点击一律消费，
    /// 避免落入下层 composer / 文档。
    pub(crate) fn handle_floating_command_menu_mouse_down(
        &mut self,
        button: MouseButton,
        column: u16,
        row: u16,
    ) -> OverlayInputResult {
        if !self.floating_command_menu_active() {
            return OverlayInputResult::Ignored;
        }
        let area = Rect::new(0, 0, self.width, self.height);
        let all_items = self.floating_command_menu_all_items();
        let Some(geometry) = self.floating_command_menu_geometry_with(area, &all_items) else {
            return OverlayInputResult::Ignored;
        };

        if !floating_command_menu_rect_contains(geometry.popup_area, column, row) {
            self.close_floating_command_menu();
            return OverlayInputResult::Handled;
        }

        let list_bottom = geometry
            .list_top
            .saturating_add(u16::try_from(geometry.list_rows).unwrap_or(u16::MAX));
        if button == MouseButton::Left && row >= geometry.list_top && row < list_bottom {
            let visible_offset = usize::from(row - geometry.list_top);
            if let Some(state) = self.resolved_floating_command_menu_state_with(all_items) {
                let index = state.scroll + visible_offset;
                if index < state.items.len()
                    && let Some(menu) = self.floating_command_menu.as_mut()
                {
                    menu.selected = index;
                }
            }
        }

        OverlayInputResult::Handled
    }

    /// 构建内联输入行：与命令列表同为 `LEFT_MARGIN` 左留白；非空查询显示文本
    /// 并在末尾附 caret，空查询时 caret 在前、其后为占位提示，右侧补空格填满。
    fn floating_command_menu_input_line(&self, query: &str, inner_width: usize) -> Line<'static> {
        let inner_width = inner_width.max(1);
        let caret_width = display_width(FLOATING_COMMAND_MENU_INPUT_CARET);
        let caret_style = command_accent_text_style(self.palette);
        let text_budget = inner_width
            .saturating_sub(FLOATING_COMMAND_MENU_LEFT_MARGIN)
            .saturating_sub(FLOATING_COMMAND_MENU_RIGHT_MARGIN)
            .saturating_sub(caret_width)
            .max(1);

        let mut spans = vec![Span::raw(" ".repeat(FLOATING_COMMAND_MENU_LEFT_MARGIN))];
        let used = if query.is_empty() {
            spans.push(Span::styled(FLOATING_COMMAND_MENU_INPUT_CARET, caret_style));
            let placeholder = truncate_display_width_with_ellipsis(
                FLOATING_COMMAND_MENU_INPUT_PLACEHOLDER,
                text_budget,
            );
            let placeholder_width = display_width(&placeholder);
            spans.push(Span::styled(placeholder, tertiary_text_style(self.palette)));
            FLOATING_COMMAND_MENU_LEFT_MARGIN + caret_width + placeholder_width
        } else {
            let shown = truncate_display_width_with_ellipsis(query, text_budget);
            let shown_width = display_width(&shown);
            spans.push(Span::styled(shown, primary_text_style(self.palette)));
            spans.push(Span::styled(FLOATING_COMMAND_MENU_INPUT_CARET, caret_style));
            FLOATING_COMMAND_MENU_LEFT_MARGIN + shown_width + caret_width
        };

        if used < inner_width {
            spans.push(Span::raw(" ".repeat(inner_width - used)));
        }
        Line::from(spans)
    }

    /// 构建输入行下方的分割线：横贯内框宽度的 `─`，与命令列表区隔。
    fn floating_command_menu_divider_line(&self, inner_width: usize) -> Line<'static> {
        Line::styled(
            "─".repeat(inner_width.max(1)),
            secondary_text_style(self.palette),
        )
    }

    /// 构建单条命令行：去掉前导 `/`，命令名与描述按固定列宽排布，选中项高亮。
    fn floating_command_menu_command_line(
        &self,
        item: &CommandPanelItem,
        query: &str,
        selected: bool,
        name_column: usize,
        description_column: usize,
        inner_width: usize,
    ) -> Line<'static> {
        let name = floating_command_menu_display_name(&item.name);
        let name_text = truncate_display_width_with_ellipsis(name, name_column);
        let name_padding = name_column.saturating_sub(display_width(&name_text));
        let description_text =
            truncate_display_width_with_ellipsis(&item.description, description_column);

        let name_style = if selected {
            command_accent_text_style(self.palette).bold()
        } else {
            secondary_text_style(self.palette)
        };
        let highlighted_name_style = search_match_style(name_style, self.palette.surface);
        let description_style = if selected {
            primary_text_style(self.palette)
        } else {
            secondary_text_style(self.palette)
        };

        let mut spans = vec![Span::raw(" ".repeat(FLOATING_COMMAND_MENU_LEFT_MARGIN))];
        spans.extend(highlighted_subsequence_spans(
            &name_text,
            query,
            name_style,
            highlighted_name_style,
        ));
        spans.push(Span::raw(
            " ".repeat(name_padding + FLOATING_COMMAND_MENU_NAME_GAP),
        ));
        spans.push(Span::styled(description_text.clone(), description_style));

        let used = FLOATING_COMMAND_MENU_LEFT_MARGIN
            + display_width(&name_text)
            + name_padding
            + FLOATING_COMMAND_MENU_NAME_GAP
            + display_width(&description_text);
        if used < inner_width {
            spans.push(Span::raw(" ".repeat(inner_width - used)));
        }
        Line::from(spans)
    }

    /// 无匹配命令时的占位行。
    fn floating_command_menu_empty_line(&self, inner_width: usize) -> Line<'static> {
        let label =
            truncate_display_width_with_ellipsis(FLOATING_COMMAND_MENU_EMPTY_LABEL, inner_width);
        let mut text = format!("{}{}", " ".repeat(FLOATING_COMMAND_MENU_LEFT_MARGIN), label);
        let padding = inner_width.saturating_sub(display_width(&text));
        text.push_str(&" ".repeat(padding));
        Line::styled(text, tertiary_text_style(self.palette))
    }
}

#[cfg(test)]
fn command_panel_query(value: &str) -> Option<String> {
    if value.is_empty() || !value.starts_with('/') || value.contains('\n') {
        return None;
    }

    let query = raw_command_panel_query(value)?;
    if !base_command_panel_items_for_query(&query).is_empty() || query.chars().count() == 1 {
        return Some(query);
    }

    None
}

fn floating_command_menu_display_name(name: &str) -> &str {
    name.trim_start_matches('/')
}

/// 由全部命令得出固定的 (命令名列宽, 描述列宽)；命令名不含前导 `/`。
fn floating_command_menu_metrics(items: &[CommandPanelItem]) -> (usize, usize) {
    let name_column = items
        .iter()
        .map(|item| display_width(floating_command_menu_display_name(&item.name)))
        .max()
        .unwrap_or(1)
        .max(1);
    let description_column = items
        .iter()
        .map(|item| display_width(&item.description))
        .max()
        .unwrap_or(1)
        .max(1);
    (name_column, description_column)
}

/// 判断 `(column, row)` 是否落在 `area` 内，用于弹窗内外命中区分。
const fn floating_command_menu_rect_contains(area: Rect, column: u16, row: u16) -> bool {
    column >= area.x && column < area.right() && row >= area.y && row < area.bottom()
}

fn floating_command_menu_blank_line(inner_width: usize) -> Line<'static> {
    Line::from(Span::raw(" ".repeat(inner_width)))
}

/// 与 `update` / composer 的 Ctrl 快捷键约定一致：要求 CONTROL，排除 ALT。
fn is_ctrl_only(modifiers: KeyModifiers) -> bool {
    modifiers.contains(KeyModifiers::CONTROL) && !modifiers.contains(KeyModifiers::ALT)
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

/// 返回当前可用的全部命令面板条目，按定义顺序排列。
fn all_command_panel_items(
    debug_commands_enabled: bool,
    esc_rewind_mode: EscRewindMode,
) -> Vec<CommandPanelItem> {
    let mut items = vec![CommandPanelItem {
        name: "/exit".to_string(),
        aliases: vec!["/quit".to_string()],
        description: "Exit the application".to_string(),
        action: CommandPanelAction::Exit,
    }];

    items.push(CommandPanelItem {
        name: "/resume".to_string(),
        aliases: Vec::new(),
        description: "Resume a previous session".to_string(),
        action: CommandPanelAction::OpenResumePicker,
    });
    items.push(CommandPanelItem {
        name: "/context".to_string(),
        aliases: Vec::new(),
        description: "Inspect context budget for the next turn".to_string(),
        action: CommandPanelAction::OpenContextBudget,
    });
    items.push(CommandPanelItem {
        name: "/copy".to_string(),
        aliases: Vec::new(),
        description: "Pick messages to copy".to_string(),
        action: CommandPanelAction::OpenCopyPicker,
    });
    items.push(CommandPanelItem {
        name: "/resend".to_string(),
        aliases: Vec::new(),
        description: "Recall a previously sent message".to_string(),
        action: CommandPanelAction::OpenMessageHistory,
    });
    if matches!(esc_rewind_mode, EscRewindMode::Entry) {
        items.push(CommandPanelItem {
            name: "/sends-back".to_string(),
            aliases: Vec::new(),
            description: "Edit a previous user message".to_string(),
            action: CommandPanelAction::OpenCoarseRewind,
        });
    } else {
        items.push(CommandPanelItem {
            name: "/tree".to_string(),
            aliases: Vec::new(),
            description: "Rewind to a precise session entry".to_string(),
            action: CommandPanelAction::OpenEntryRewind,
        });
    }

    items.push(CommandPanelItem {
        name: "/models".to_string(),
        aliases: Vec::new(),
        description: "Select model for this session".to_string(),
        action: CommandPanelAction::OpenModelPanel,
    });
    items.push(CommandPanelItem {
        name: "/prompt".to_string(),
        aliases: Vec::new(),
        description: "Inspect prompt assembly for the next new session".to_string(),
        action: CommandPanelAction::OpenPromptOverlay,
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

    items
}

#[cfg(test)]
fn base_command_panel_items_for_query(query: &str) -> Vec<CommandPanelItem> {
    let items = all_command_panel_items(false, EscRewindMode::Coarse);
    rank_command_panel_items(items, query)
}

fn command_panel_completion_text(item: &CommandPanelItem) -> String {
    item.name.clone()
}

/// 计算命令条目相对查询的匹配分数。lower is better。
///
/// 命令名与别名都参与匹配，取最佳分数；都不匹配时返回 `None`。
fn command_panel_item_match_score(item: &CommandPanelItem, query: &str) -> Option<i32> {
    let primary = item.name.trim_start_matches('/');
    let primary_score = subsequence_match_score(primary, query);

    let best_alias_score = item
        .aliases
        .iter()
        .filter_map(|alias| subsequence_match_score(alias.trim_start_matches('/'), query))
        .min();

    // 用 iterator chain 而非 Option::min：后者把 None 当作最小值，
    // 会在别名缺席时把 primary 的 Some 分数也吞成 None。
    primary_score.into_iter().chain(best_alias_score).min()
}

/// 按 (score, 原始定义顺序) 对命令条目打分排序。
///
/// lower score 排前面；同分时保持 items 中的原始顺序。
/// 空 query 直接原样返回（命令面板展开全部命令）。
fn rank_command_panel_items(items: Vec<CommandPanelItem>, query: &str) -> Vec<CommandPanelItem> {
    if query.is_empty() {
        return items;
    }

    let mut scored = items
        .into_iter()
        .enumerate()
        .filter_map(|(index, item)| {
            command_panel_item_match_score(&item, query).map(|score| (score, index, item))
        })
        .collect::<Vec<_>>();
    scored.sort_by(
        |(left_score, left_index, _), (right_score, right_index, _)| {
            left_score
                .cmp(right_score)
                .then_with(|| left_index.cmp(right_index))
        },
    );
    scored.into_iter().map(|(_, _, item)| item).collect()
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
