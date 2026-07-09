//! 预检 step 共用的布局常量与 helper。
//!
//! 与主 TUI 的 inline 菜单约定对齐：
//! - 内容左侧 2 cell 缩进（字符串 `"  "`，`Wrap { trim: false }` 保留）
//! - 标题与正文之间用全宽 `accent_rule_line`（`━`），分割线本身不缩进
//! - 选项 marker 固定占 2 列（`"> "` / `"  "`），文字列左对齐

use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};
use terminal_ui::theme::{
    TerminalPalette, accent_rule_line, accent_text_style, primary_text_style,
};

/// 左侧内容缩进，与 `panel_block` 的 `Padding::horizontal(2)` / model panel 一致。
pub(super) const LEFT_INSET: &str = "  ";

pub(super) fn title_line(text: &str, palette: TerminalPalette) -> Line<'static> {
    Line::from(vec![
        Span::raw(LEFT_INSET),
        Span::styled(
            text.to_string(),
            primary_text_style(palette).add_modifier(Modifier::BOLD),
        ),
    ])
}

/// 全宽分割线：不缩进，与 `/models` inline 菜单顶线一致。
pub(super) fn rule_line(width: u16, palette: TerminalPalette) -> Line<'static> {
    accent_rule_line(usize::from(width), palette)
}

pub(super) fn inset_styled(text: impl Into<String>, style: Style) -> Line<'static> {
    Line::from(vec![
        Span::raw(LEFT_INSET),
        Span::styled(text.into(), style),
    ])
}

pub(super) fn inset_spans(spans: Vec<Span<'static>>) -> Line<'static> {
    let mut line = vec![Span::raw(LEFT_INSET)];
    line.extend(spans);
    Line::from(line)
}

/// 选项行：`  > label` / `    label`，label 列对齐。
pub(super) fn option_line(
    selected: bool,
    label: &str,
    selected_style: Style,
    unselected_style: Style,
    palette: TerminalPalette,
) -> Line<'static> {
    let marker = if selected { ">" } else { " " };
    let label_style = if selected {
        selected_style
    } else {
        unselected_style
    };
    Line::from(vec![
        Span::raw(LEFT_INSET),
        Span::styled(marker.to_string(), accent_text_style(palette)),
        Span::raw(" "),
        Span::styled(label.to_string(), label_style),
    ])
}
