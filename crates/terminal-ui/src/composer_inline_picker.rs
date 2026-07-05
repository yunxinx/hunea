use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

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
    if item_count == 0 {
        return 0;
    }
    let visible_rows = visible_rows.max(1);
    let max_scroll = item_count.saturating_sub(visible_rows);
    let mut scroll = scroll.min(max_scroll);
    if selected < scroll {
        scroll = selected;
    }
    if selected >= scroll + visible_rows {
        scroll = selected + 1 - visible_rows;
    }
    scroll.min(max_scroll)
}
