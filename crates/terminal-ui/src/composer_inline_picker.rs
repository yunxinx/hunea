use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{style::Style, text::Line};

use super::{
    display_width::display_width,
    list_selection::VisibleWindowSelection,
    selection::{SelectableLineRange, selectable_range_for_plain_line},
    status_line::truncate_display_width_with_ellipsis,
};

/// `ComposerInlinePickerState` 保存 composer 内联选择器的共享导航状态。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct ComposerInlinePickerState<T> {
    pub(crate) query: String,
    pub(crate) items: Vec<T>,
    pub(crate) selected: usize,
    pub(crate) scroll: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ComposerInlinePickerKey {
    MovePrevious,
    MoveNext,
    Dismiss,
    Complete,
    Accept,
}

/// `ComposerInlinePickerRenderedRows` 是内联 picker 弹层渲染后的三套同步行数据。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ComposerInlinePickerRenderedRows {
    pub(crate) lines: Vec<Line<'static>>,
    pub(crate) plain_lines: Vec<String>,
    pub(crate) selectable: Vec<SelectableLineRange>,
}

pub(crate) fn reconcile_composer_inline_picker_state<T>(
    query: String,
    items: Vec<T>,
    previous: Option<&ComposerInlinePickerState<T>>,
    visible_rows: usize,
    initial_selected: usize,
) -> ComposerInlinePickerState<T> {
    let query_changed = previous.is_none_or(|state| state.query != query);
    let mut selected = if query_changed {
        initial_selected
    } else {
        previous.map(|state| state.selected).unwrap_or(0)
    };
    let mut scroll = if query_changed {
        0
    } else {
        previous.map(|state| state.scroll).unwrap_or(0)
    };

    if items.is_empty() {
        selected = 0;
        scroll = 0;
    } else {
        selected = selected.min(items.len() - 1);
        scroll = clamp_composer_inline_picker_scroll(scroll, selected, items.len(), visible_rows);
    }

    ComposerInlinePickerState {
        query,
        items,
        selected,
        scroll,
    }
}

pub(crate) fn classify_composer_inline_picker_key(
    key: KeyEvent,
) -> Option<ComposerInlinePickerKey> {
    match key.code {
        KeyCode::Up if key.modifiers.is_empty() => Some(ComposerInlinePickerKey::MovePrevious),
        KeyCode::Down if key.modifiers.is_empty() => Some(ComposerInlinePickerKey::MoveNext),
        KeyCode::Char('p') if key.modifiers == KeyModifiers::CONTROL => {
            Some(ComposerInlinePickerKey::MovePrevious)
        }
        KeyCode::Char('n') if key.modifiers == KeyModifiers::CONTROL => {
            Some(ComposerInlinePickerKey::MoveNext)
        }
        KeyCode::Esc if key.modifiers.is_empty() => Some(ComposerInlinePickerKey::Dismiss),
        KeyCode::Tab if key.modifiers.is_empty() => Some(ComposerInlinePickerKey::Complete),
        KeyCode::Enter if key.modifiers.is_empty() => Some(ComposerInlinePickerKey::Accept),
        _ => None,
    }
}

pub(crate) fn move_composer_inline_picker_selection<T>(
    state: &mut ComposerInlinePickerState<T>,
    delta: isize,
    visible_rows: usize,
) {
    if state.items.is_empty() {
        return;
    }

    let last = state.items.len() - 1;
    if delta.is_negative() {
        state.selected = state.selected.saturating_sub(delta.unsigned_abs());
    } else {
        state.selected = state.selected.saturating_add(delta as usize).min(last);
    }
    state.scroll = clamp_composer_inline_picker_scroll(
        state.scroll,
        state.selected,
        state.items.len(),
        visible_rows,
    );
}

pub(crate) fn clamp_composer_inline_picker_scroll(
    scroll: usize,
    selected: usize,
    item_count: usize,
    visible_rows: usize,
) -> usize {
    VisibleWindowSelection::new(selected, item_count)
        .scroll_start_for_selection(scroll, visible_rows)
}

pub(crate) fn render_composer_inline_picker_rows<T>(
    state: &ComposerInlinePickerState<T>,
    width: usize,
    visible_rows: usize,
    empty_text: &str,
    empty_style: Style,
    mut render_item: impl FnMut(&T, &str, bool, usize) -> (Line<'static>, String),
    selectable_range_for_item: impl Fn(&str, usize) -> SelectableLineRange,
) -> ComposerInlinePickerRenderedRows {
    let width = width.max(1);
    let visible_rows = visible_rows.max(1);
    let mut lines = Vec::with_capacity(visible_rows);
    let mut plain_lines = Vec::with_capacity(visible_rows);
    let mut selectable = Vec::with_capacity(visible_rows);

    if state.items.is_empty() {
        let plain_line = pad_display_width_right(empty_text, width);
        lines.push(Line::styled(plain_line.clone(), empty_style));
        plain_lines.push(plain_line.clone());
        selectable.push(selectable_range_for_plain_line(&plain_line));
        return ComposerInlinePickerRenderedRows {
            lines,
            plain_lines,
            selectable,
        };
    }

    for row in 0..visible_rows {
        let index = state.scroll + row;
        let Some(item) = state.items.get(index) else {
            lines.push(Line::raw(""));
            plain_lines.push(String::new());
            selectable.push(SelectableLineRange::default());
            continue;
        };

        let selected = index == state.selected;
        let (line, plain_line) = render_item(item, &state.query, selected, width);
        selectable.push(selectable_range_for_item(&plain_line, width));
        lines.push(line);
        plain_lines.push(plain_line);
    }

    ComposerInlinePickerRenderedRows {
        lines,
        plain_lines,
        selectable,
    }
}

fn pad_display_width_right(text: &str, width: usize) -> String {
    let text = truncate_display_width_with_ellipsis(text, width);
    let padding = width.saturating_sub(display_width(&text));
    format!("{text}{}", " ".repeat(padding))
}

#[cfg(test)]
mod tests {
    use ratatui::{style::Style, text::Line};

    use super::*;
    use crate::selection::{SelectableLineRange, selectable_range_for_plain_line};

    #[test]
    fn render_composer_inline_picker_rows_pads_empty_state() {
        let state = ComposerInlinePickerState::<String> {
            query: "none".to_string(),
            items: Vec::new(),
            selected: 0,
            scroll: 0,
        };

        let rows = render_composer_inline_picker_rows(
            &state,
            10,
            3,
            "  Empty",
            Style::default(),
            |_, _, _, _| unreachable!("empty state should not render items"),
            |plain, _| selectable_range_for_plain_line(plain),
        );

        assert_eq!(rows.plain_lines, vec!["  Empty   "]);
        assert_eq!(
            rows.selectable[0].content_columns(),
            Some((0, "  Empty   ".len()))
        );
    }

    #[test]
    fn render_composer_inline_picker_rows_uses_scroll_and_selected_index() {
        let state = ComposerInlinePickerState {
            query: "item".to_string(),
            items: vec!["zero".to_string(), "one".to_string(), "two".to_string()],
            selected: 2,
            scroll: 1,
        };

        let rows = render_composer_inline_picker_rows(
            &state,
            12,
            3,
            "  Empty",
            Style::default(),
            |item, query, selected, width| {
                let marker = if selected { "*" } else { " " };
                let plain = format!("{marker}{query}:{item}:{width}");
                (Line::raw(plain.clone()), plain)
            },
            |plain, _| SelectableLineRange::new(1, plain.len()),
        );

        assert_eq!(
            rows.plain_lines,
            vec![
                " item:one:12".to_string(),
                "*item:two:12".to_string(),
                String::new(),
            ]
        );
        assert_eq!(rows.selectable[0].content_columns(), Some((1, 12)));
        assert_eq!(rows.selectable[2], SelectableLineRange::default());
    }
}
