use ratatui::{
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::{Clear, Paragraph},
};

use crate::{
    display_width::display_width,
    render_frame::RenderFrame,
    theme::{TerminalPalette, panel_block},
};

const SHORTCUT_HELP_POPOVER_HORIZONTAL_CHROME: usize = 6;
const SHORTCUT_HELP_POPOVER_VERTICAL_CHROME: usize = 2;

/// `ShortcutHelpPopover` 在给定边界内渲染右下角快捷键说明浮窗。
pub(crate) struct ShortcutHelpPopover<'a> {
    pub(crate) title: Option<&'a str>,
    pub(crate) lines: &'a [Line<'a>],
}

/// `ShortcutHelpEntry` 描述单条快捷键帮助，用于统一两列对齐排版。
pub(crate) struct ShortcutHelpEntry<'a> {
    pub(crate) shortcut: &'a str,
    pub(crate) description: &'a str,
}

impl<'a> ShortcutHelpPopover<'a> {
    pub(crate) fn area(&self, bounds: Rect) -> Rect {
        shortcut_help_popover_area(bounds, self.title, self.lines)
    }

    pub(crate) fn render(
        &self,
        frame: &mut RenderFrame<'_>,
        bounds: Rect,
        palette: TerminalPalette,
    ) {
        let area = self.area(bounds);
        if area.is_empty() {
            return;
        }

        let block = match self.title {
            Some(title) => panel_block(palette).title(title),
            None => panel_block(palette),
        };
        let inner_area = block.inner(area);
        frame.render_widget(Clear, area);
        frame.render_widget(block, area);
        frame.render_widget(Paragraph::new(self.lines.to_vec()), inner_area);
    }
}

fn shortcut_help_popover_area(bounds: Rect, title: Option<&str>, lines: &[Line<'_>]) -> Rect {
    if bounds.is_empty() {
        return Rect::ZERO;
    }

    let content_width = lines
        .iter()
        .map(shortcut_help_line_width)
        .chain(title.into_iter().map(display_width))
        .max()
        .unwrap_or(1)
        .max(1);
    let width =
        u16::try_from(content_width.saturating_add(SHORTCUT_HELP_POPOVER_HORIZONTAL_CHROME))
            .unwrap_or(u16::MAX);
    let height = u16::try_from(
        lines
            .len()
            .saturating_add(SHORTCUT_HELP_POPOVER_VERTICAL_CHROME),
    )
    .unwrap_or(u16::MAX);
    let width = width.min(bounds.width);
    let height = height.min(bounds.height);
    let x = bounds.x + bounds.width.saturating_sub(width);
    let y = bounds.y + bounds.height.saturating_sub(height);
    Rect::new(x, y, width, height)
}

fn shortcut_help_line_width(line: &Line<'_>) -> usize {
    line.spans
        .iter()
        .map(|span| display_width(span.content.as_ref()))
        .sum()
}

pub(crate) fn aligned_shortcut_help_lines<'a>(
    entries: &[ShortcutHelpEntry<'a>],
    shortcut_style: Style,
    description_style: Style,
) -> Vec<Line<'a>> {
    let shortcut_width = entries
        .iter()
        .map(|entry| display_width(entry.shortcut))
        .max()
        .unwrap_or(0);
    entries
        .iter()
        .map(|entry| {
            let padding = shortcut_width
                .saturating_sub(display_width(entry.shortcut))
                .saturating_add(2);
            Line::from(vec![
                Span::styled(entry.shortcut, shortcut_style),
                Span::raw(" ".repeat(padding)),
                Span::styled(entry.description, description_style),
            ])
        })
        .collect()
}
