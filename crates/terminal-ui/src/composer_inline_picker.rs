use std::borrow::Cow;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{style::Style, text::Line};

use super::{
    display_width::display_width,
    inline_panel::InlinePanelRenderResult,
    list_selection::VisibleWindowSelection,
    selection::{SelectableLineRange, selectable_range_for_plain_line},
    status_line::truncate_display_width_with_ellipsis,
    text_search::CaseInsensitiveQuery,
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
enum ComposerInlinePickerKey {
    MovePrevious,
    MoveNext,
    Dismiss,
    Complete,
    Accept,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ComposerInlinePickerCommand {
    Dismiss,
    Complete,
    Accept,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ComposerInlinePickerInputResult {
    Ignored,
    Handled,
    Command(ComposerInlinePickerCommand),
}

/// `ComposerInlinePickerRenderedRows` 是内联 picker 弹层渲染后的三套同步行数据。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ComposerInlinePickerRenderedRows {
    pub(crate) lines: Vec<Line<'static>>,
    pub(crate) plain_lines: Vec<String>,
    pub(crate) selectable: Vec<SelectableLineRange>,
}

/// `ComposerInlinePickerSearchText` 描述内联 picker 一行参与搜索的文本字段。
pub(crate) struct ComposerInlinePickerSearchText<'a> {
    pub(crate) prefix_terms: Vec<Cow<'a, str>>,
    pub(crate) fuzzy_terms: Vec<Cow<'a, str>>,
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

pub(crate) fn filter_composer_inline_picker_items<'a, T: Clone>(
    items: &'a [T],
    query: &str,
    search_text_for_item: impl Fn(&'a T) -> ComposerInlinePickerSearchText<'a>,
) -> Vec<T> {
    let trimmed_query = query.trim();
    if trimmed_query.is_empty() {
        return items.to_vec();
    }

    let query = CaseInsensitiveQuery::new(trimmed_query);
    let mut prefix_matches = Vec::new();
    let mut fuzzy_matches = Vec::new();

    for item in items {
        let search_text = search_text_for_item(item);
        if search_text
            .prefix_terms
            .iter()
            .any(|term| query.starts_with(term.as_ref()))
        {
            prefix_matches.push(item.clone());
        } else if search_text
            .prefix_terms
            .iter()
            .chain(search_text.fuzzy_terms.iter())
            .any(|term| query.matches(term.as_ref()))
        {
            fuzzy_matches.push(item.clone());
        }
    }

    prefix_matches.extend(fuzzy_matches);
    prefix_matches
}

pub(crate) fn common_composer_inline_picker_completion_prefix<'a>(
    values: impl IntoIterator<Item = &'a str>,
) -> String {
    let mut iter = values.into_iter();
    let Some(first) = iter.next() else {
        return String::new();
    };

    let mut prefix = first.to_string();
    for value in iter {
        let next_len = prefix
            .chars()
            .zip(value.chars())
            .take_while(|(left, right)| left.eq_ignore_ascii_case(right))
            .count();
        prefix = prefix.chars().take(next_len).collect();
        if prefix.is_empty() {
            break;
        }
    }

    prefix
}

fn classify_composer_inline_picker_key(key: KeyEvent) -> Option<ComposerInlinePickerKey> {
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

fn move_composer_inline_picker_selection<T>(
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

pub(crate) fn handle_composer_inline_picker_input<T>(
    state: &mut ComposerInlinePickerState<T>,
    key: KeyEvent,
    visible_rows: usize,
) -> ComposerInlinePickerInputResult {
    match classify_composer_inline_picker_key(key) {
        Some(ComposerInlinePickerKey::MovePrevious) => {
            move_composer_inline_picker_selection(state, -1, visible_rows);
            ComposerInlinePickerInputResult::Handled
        }
        Some(ComposerInlinePickerKey::MoveNext) => {
            move_composer_inline_picker_selection(state, 1, visible_rows);
            ComposerInlinePickerInputResult::Handled
        }
        Some(ComposerInlinePickerKey::Dismiss) => {
            ComposerInlinePickerInputResult::Command(ComposerInlinePickerCommand::Dismiss)
        }
        Some(ComposerInlinePickerKey::Complete) => {
            ComposerInlinePickerInputResult::Command(ComposerInlinePickerCommand::Complete)
        }
        Some(ComposerInlinePickerKey::Accept) => {
            ComposerInlinePickerInputResult::Command(ComposerInlinePickerCommand::Accept)
        }
        None => ComposerInlinePickerInputResult::Ignored,
    }
}

fn clamp_composer_inline_picker_scroll(
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

pub(crate) fn render_composer_inline_picker_panel<T>(
    state: Option<&ComposerInlinePickerState<T>>,
    viewport_width: u16,
    visible_rows: usize,
    render_rows: impl FnOnce(
        &ComposerInlinePickerState<T>,
        usize,
        usize,
    ) -> ComposerInlinePickerRenderedRows,
) -> InlinePanelRenderResult {
    let Some(state) = state else {
        return InlinePanelRenderResult::default();
    };

    let width = usize::from(viewport_width.max(1));
    let has_scrollbar = state.items.len() > visible_rows;
    let content_width = width.saturating_sub(usize::from(has_scrollbar && width > 1));
    let rows = render_rows(state, content_width, visible_rows);

    InlinePanelRenderResult {
        lines: rows.lines,
        plain_lines: rows.plain_lines,
        selectable: rows.selectable,
        has_content: true,
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
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

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

    #[test]
    fn filter_composer_inline_picker_items_keeps_prefix_matches_before_fuzzy_matches() {
        let items = vec![
            ("review-rules", "Review Rules", "audit every diff"),
            (
                "release-checklist",
                "Release Checklist",
                "review release notes",
            ),
            ("smoke-test", "Smoke Test", "quick verification"),
        ];

        let filtered = filter_composer_inline_picker_items(&items, "review", |item| {
            ComposerInlinePickerSearchText {
                prefix_terms: vec![item.0.into(), item.1.into()],
                fuzzy_terms: vec![item.2.into()],
            }
        });

        assert_eq!(
            filtered,
            vec![
                ("review-rules", "Review Rules", "audit every diff"),
                (
                    "release-checklist",
                    "Release Checklist",
                    "review release notes"
                ),
            ]
        );
    }

    #[test]
    fn common_completion_prefix_can_filter_values_by_query_prefix() {
        let values = ["repo-bootstrap", "repo-build", "review-rules"];
        let query = CaseInsensitiveQuery::new("repo");
        let prefix = common_composer_inline_picker_completion_prefix(
            values.into_iter().filter(|value| query.starts_with(value)),
        );

        assert_eq!(prefix, "repo-b");
    }

    #[test]
    fn handle_composer_inline_picker_input_moves_selection_and_reports_commands() {
        let mut state = ComposerInlinePickerState {
            query: String::new(),
            items: vec!["one", "two", "three"],
            selected: 1,
            scroll: 0,
        };

        assert_eq!(
            handle_composer_inline_picker_input(
                &mut state,
                KeyEvent::new(KeyCode::Down, KeyModifiers::NONE),
                2,
            ),
            ComposerInlinePickerInputResult::Handled
        );
        assert_eq!(state.selected, 2);
        assert_eq!(state.scroll, 1);

        assert_eq!(
            handle_composer_inline_picker_input(
                &mut state,
                KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
                2,
            ),
            ComposerInlinePickerInputResult::Command(ComposerInlinePickerCommand::Accept)
        );
        assert_eq!(state.selected, 2);
        assert_eq!(state.scroll, 1);

        assert_eq!(
            handle_composer_inline_picker_input(
                &mut state,
                KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE),
                2,
            ),
            ComposerInlinePickerInputResult::Ignored
        );
    }

    #[test]
    fn render_composer_inline_picker_panel_reserves_scrollbar_column() {
        let state = ComposerInlinePickerState {
            query: String::new(),
            items: vec!["one", "two", "three"],
            selected: 0,
            scroll: 0,
        };

        let panel =
            render_composer_inline_picker_panel(Some(&state), 5, 2, |state, width, rows| {
                assert_eq!(state.items.len(), 3);
                assert_eq!(width, 4);
                assert_eq!(rows, 2);
                ComposerInlinePickerRenderedRows {
                    lines: vec![Line::raw("one")],
                    plain_lines: vec!["one".to_string()],
                    selectable: vec![SelectableLineRange::new(0, 3)],
                }
            });

        assert!(panel.has_content);
        assert_eq!(panel.plain_lines, vec!["one"]);
        assert_eq!(panel.selectable[0].content_columns(), Some((0, 3)));
    }
}
