use super::*;

impl Model {
    pub(crate) fn handle_prompt_overlay_key(&mut self, key: KeyEvent) -> OverlayInputResult {
        if !self.prompt_overlay_active() {
            return OverlayInputResult::Ignored;
        }
        if self.prompt_overlay_preview_active() {
            return self.handle_prompt_overlay_preview_key(key);
        }
        if self.prompt_overlay_dialog_active() {
            return self.handle_prompt_overlay_dialog_key(key);
        }
        if self.prompt_overlay_shortcut_help_open() {
            if matches!(key.code, KeyCode::Char('?')) {
                self.toggle_prompt_overlay_shortcut_help();
                return OverlayInputResult::Handled;
            }
            self.close_prompt_overlay_shortcut_help();
            if matches!(key.code, KeyCode::Esc) && key.modifiers.is_empty() {
                return OverlayInputResult::Handled;
            }
        }

        match key.code {
            KeyCode::Char('?') => {
                self.toggle_prompt_overlay_shortcut_help();
                OverlayInputResult::Handled
            }
            KeyCode::Esc if key.modifiers.is_empty() => {
                self.close_prompt_overlay();
                OverlayInputResult::Handled
            }
            KeyCode::Left | KeyCode::Char('h') if key.modifiers.is_empty() => {
                if self.move_prompt_overlay_dynamic_snapshot_column(-1) {
                    return OverlayInputResult::Handled;
                }
                self.set_prompt_overlay_focus(PromptOverlayFocus::Active);
                OverlayInputResult::Handled
            }
            KeyCode::Right | KeyCode::Char('l') if key.modifiers.is_empty() => {
                if self.move_prompt_overlay_dynamic_snapshot_column(1) {
                    return OverlayInputResult::Handled;
                }
                self.set_prompt_overlay_focus(PromptOverlayFocus::Inactive);
                OverlayInputResult::Handled
            }
            KeyCode::Tab if key.modifiers.is_empty() => {
                if self
                    .prompt_overlay
                    .as_ref()
                    .is_some_and(|state| state.focus == PromptOverlayFocus::Inactive)
                {
                    self.cycle_prompt_overlay_inactive_tab(1);
                }
                OverlayInputResult::Handled
            }
            KeyCode::BackTab => {
                if self
                    .prompt_overlay
                    .as_ref()
                    .is_some_and(|state| state.focus == PromptOverlayFocus::Inactive)
                {
                    self.cycle_prompt_overlay_inactive_tab(-1);
                }
                OverlayInputResult::Handled
            }
            KeyCode::Up | KeyCode::Char('k') if key.modifiers.is_empty() => {
                self.move_prompt_overlay_selection(ListNavigationDirection::Previous);
                OverlayInputResult::Handled
            }
            KeyCode::Down | KeyCode::Char('j') if key.modifiers.is_empty() => {
                self.move_prompt_overlay_selection(ListNavigationDirection::Next);
                OverlayInputResult::Handled
            }
            KeyCode::PageUp if key.modifiers.is_empty() => {
                self.move_prompt_overlay_page(ListNavigationDirection::Previous);
                OverlayInputResult::Handled
            }
            KeyCode::PageDown if key.modifiers.is_empty() => {
                self.move_prompt_overlay_page(ListNavigationDirection::Next);
                OverlayInputResult::Handled
            }
            KeyCode::Home if key.modifiers.is_empty() => {
                self.jump_prompt_overlay_selection_to_edge(true);
                OverlayInputResult::Handled
            }
            KeyCode::End if key.modifiers.is_empty() => {
                self.jump_prompt_overlay_selection_to_edge(false);
                OverlayInputResult::Handled
            }
            KeyCode::Char(' ') if key.modifiers.is_empty() => {
                self.open_selected_prompt_overlay_preview();
                OverlayInputResult::Handled
            }
            KeyCode::Char('p') if key.modifiers.is_empty() => {
                self.open_prompt_overlay_assembled_preview();
                OverlayInputResult::Handled
            }
            KeyCode::Char('e') if key.modifiers == KeyModifiers::CONTROL => {
                self.toggle_prompt_overlay_expanded_row();
                OverlayInputResult::Handled
            }
            KeyCode::Char('\u{0005}') if key.modifiers.is_empty() => {
                self.toggle_prompt_overlay_expanded_row();
                OverlayInputResult::Handled
            }
            KeyCode::Char('e') if key.modifiers.is_empty() => {
                OverlayInputResult::from_effect(self.open_prompt_overlay_editor_for_selection())
            }
            KeyCode::Char('a') if key.modifiers.is_empty() => {
                self.open_create_extra_prompt_scope_picker();
                OverlayInputResult::Handled
            }
            KeyCode::Char('i') | KeyCode::Char('I') if key.modifiers.is_empty() => {
                OverlayInputResult::Handled
            }
            KeyCode::Char('d') if key.modifiers.is_empty() => {
                OverlayInputResult::from_effect(self.remove_selected_prompt_source())
            }
            KeyCode::Char('x') if key.modifiers.is_empty() => {
                OverlayInputResult::from_effect(self.toggle_selected_prompt_source_enabled())
            }
            KeyCode::Char('K') if allows_shift_only_modifier(key.modifiers) => {
                OverlayInputResult::from_effect(
                    self.move_selected_active_source(PromptAssemblyMoveDirection::Up),
                )
            }
            KeyCode::Char('J') if allows_shift_only_modifier(key.modifiers) => {
                OverlayInputResult::from_effect(
                    self.move_selected_active_source(PromptAssemblyMoveDirection::Down),
                )
            }
            KeyCode::Char('r') if key.modifiers.is_empty() => OverlayInputResult::from_effect(
                self.reset_selected_discovered_skill_order()
                    .or_else(|| self.restore_selected_core_system_override()),
            ),
            _ => OverlayInputResult::Handled,
        }
    }

    pub(crate) fn move_prompt_overlay_selection_by_delta(&mut self, delta: isize) {
        let Some(direction) = ListNavigationDirection::from_delta(delta) else {
            return;
        };
        self.move_prompt_overlay_selection(direction);
    }

    pub(super) fn set_prompt_overlay_focus(&mut self, focus: PromptOverlayFocus) {
        let Some(state) = self.prompt_overlay.as_mut() else {
            return;
        };
        state.focus = focus;
        self.sync_prompt_overlay_state();
    }

    pub(crate) fn handle_prompt_overlay_mouse_down(
        &mut self,
        button: MouseButton,
        column: u16,
        row: u16,
    ) -> OverlayInputResult {
        if !self.prompt_overlay_active() {
            return OverlayInputResult::Ignored;
        }
        if button != MouseButton::Left || self.prompt_overlay_preview_active() {
            return OverlayInputResult::Handled;
        }
        if self.prompt_overlay_dialog_active() {
            return self.handle_prompt_overlay_dialog_mouse_down(column, row);
        }

        let Some((active_tab, _focus)) = self
            .prompt_overlay
            .as_ref()
            .map(|state| (state.inactive_tab, state.focus))
        else {
            return OverlayInputResult::Handled;
        };
        let Some(layout) = prompt_overlay_layout_rects(Rect::new(0, 0, self.width, self.height))
        else {
            return OverlayInputResult::Handled;
        };
        if self.prompt_overlay_shortcut_help_open() {
            let popover_area = self.prompt_overlay_shortcut_help_area(layout.chrome.body);
            if prompt_overlay_rect_contains(popover_area, column, row) {
                return OverlayInputResult::Handled;
            }
            self.close_prompt_overlay_shortcut_help();
        }

        if let Some(tab) =
            self.prompt_overlay_header_tab_at(column, row, layout.chrome.header, active_tab)
        {
            self.set_prompt_overlay_focus(PromptOverlayFocus::Inactive);
            if active_tab != tab {
                self.set_prompt_overlay_inactive_tab(tab);
            }
            return OverlayInputResult::Handled;
        }

        if prompt_overlay_rect_contains(layout.left_pane, column, row) {
            self.set_prompt_overlay_focus(PromptOverlayFocus::Active);
            if let Some(visible_offset) =
                prompt_overlay_visible_offset_for_row(layout.left_body, row)
            {
                self.select_prompt_overlay_active_row(visible_offset);
            }
            return OverlayInputResult::Handled;
        }

        if prompt_overlay_rect_contains(layout.right_pane, column, row) {
            self.set_prompt_overlay_focus(PromptOverlayFocus::Inactive);
            if let Some(visible_offset) =
                prompt_overlay_visible_offset_for_row(layout.right_body, row)
            {
                let snapshot_kind = self.prompt_overlay_dynamic_snapshot_kind_for_mouse_down(
                    column,
                    visible_offset,
                    layout.right_body,
                );
                self.select_prompt_overlay_inactive_row(visible_offset);
                if let Some(snapshot_kind) = snapshot_kind {
                    self.set_prompt_overlay_dynamic_snapshot_kind(snapshot_kind);
                }
            }
            return OverlayInputResult::Handled;
        }

        OverlayInputResult::Handled
    }

    pub(super) fn cycle_prompt_overlay_inactive_tab(&mut self, delta: isize) {
        let Some(state) = self.prompt_overlay.as_mut() else {
            return;
        };
        state.inactive_tab = if delta.is_negative() {
            state.inactive_tab.previous()
        } else {
            state.inactive_tab.next()
        };
        self.sync_prompt_overlay_state();
    }

    pub(super) fn set_prompt_overlay_inactive_tab(&mut self, tab: PromptOverlayInactiveTab) {
        let Some(state) = self.prompt_overlay.as_mut() else {
            return;
        };
        state.inactive_tab = tab;
        self.sync_prompt_overlay_state();
    }

    pub(super) fn move_prompt_overlay_dynamic_snapshot_column(&mut self, delta: isize) -> bool {
        let Some(state) = self.prompt_overlay.as_mut() else {
            return false;
        };
        if state.focus != PromptOverlayFocus::Inactive
            || state.inactive_tab != PromptOverlayInactiveTab::Dynamic
        {
            return false;
        }

        let snapshot_kind = if delta.is_negative() {
            DynamicEnvironmentSnapshotKind::Baseline
        } else {
            DynamicEnvironmentSnapshotKind::Changes
        };
        state.dynamic_selected_snapshot_kind = snapshot_kind;
        true
    }

    pub(super) fn set_prompt_overlay_dynamic_snapshot_kind(
        &mut self,
        snapshot_kind: DynamicEnvironmentSnapshotKind,
    ) {
        let Some(state) = self.prompt_overlay.as_mut() else {
            return;
        };
        if state.inactive_tab != PromptOverlayInactiveTab::Dynamic {
            return;
        }
        state.dynamic_selected_snapshot_kind = snapshot_kind;
    }

    pub(super) fn prompt_overlay_dynamic_snapshot_kind_for_mouse_down(
        &self,
        column: u16,
        visible_offset: usize,
        body_area: Rect,
    ) -> Option<DynamicEnvironmentSnapshotKind> {
        let state = self.prompt_overlay.as_ref()?;
        if state.inactive_tab != PromptOverlayInactiveTab::Dynamic {
            return None;
        }

        let row_index = state.inactive_scroll.saturating_add(visible_offset);
        let rows = self.prompt_overlay_inactive_rows(PromptOverlayInactiveTab::Dynamic);
        if !matches!(
            rows.get(row_index),
            Some(PromptOverlayInactiveRow::DynamicEnvironmentCandidate { .. })
        ) {
            return None;
        }

        prompt_overlay_dynamic_checkbox_hit_test(column, body_area)
    }

    pub(super) fn prompt_overlay_dynamic_selected_snapshot_kind(
        &self,
    ) -> DynamicEnvironmentSnapshotKind {
        self.prompt_overlay
            .as_ref()
            .map(|state| state.dynamic_selected_snapshot_kind)
            .unwrap_or(DynamicEnvironmentSnapshotKind::Baseline)
    }

    pub(super) fn move_prompt_overlay_selection(&mut self, direction: ListNavigationDirection) {
        let focus = match self.prompt_overlay.as_ref() {
            Some(state) => state.focus,
            None => return,
        };

        match focus {
            PromptOverlayFocus::Active => self.move_prompt_overlay_active_selection(direction),
            PromptOverlayFocus::Inactive => self.move_prompt_overlay_inactive_selection(direction),
        }
    }

    pub(super) fn move_prompt_overlay_page(&mut self, direction: ListNavigationDirection) {
        let focus = match self.prompt_overlay.as_ref() {
            Some(state) => state.focus,
            None => return,
        };

        let page_size = match focus {
            PromptOverlayFocus::Active => prompt_overlay_active_visible_rows(self.height),
            PromptOverlayFocus::Inactive => prompt_overlay_inactive_visible_rows(self.height),
        };
        for _ in 0..page_size.max(1) {
            self.move_prompt_overlay_selection(direction);
        }
    }

    pub(super) fn jump_prompt_overlay_selection_to_edge(&mut self, first: bool) {
        let (focus, inactive_tab) = match self.prompt_overlay.as_ref() {
            Some(state) => (state.focus, state.inactive_tab),
            None => return,
        };
        let active_rows = self.prompt_overlay_left_rows();
        let active_count = active_rows.len();
        let active_row_id = if matches!(focus, PromptOverlayFocus::Active) {
            active_rows
                .get(if first {
                    0
                } else {
                    active_count.saturating_sub(1)
                })
                .map(prompt_overlay_left_row_id)
        } else {
            None
        };
        let inactive_reference_id = if matches!(focus, PromptOverlayFocus::Inactive) {
            let rows = self.prompt_overlay_inactive_rows(inactive_tab);
            let source_count = rows.len();
            rows.get(if first {
                0
            } else {
                source_count.saturating_sub(1)
            })
            .map(prompt_overlay_inactive_row_id)
        } else {
            None
        };
        let inactive_count = if matches!(focus, PromptOverlayFocus::Inactive) {
            self.prompt_overlay_inactive_source_count(inactive_tab)
        } else {
            0
        };

        let Some(state) = self.prompt_overlay.as_mut() else {
            return;
        };

        match focus {
            PromptOverlayFocus::Active => {
                let last_index = active_count.saturating_sub(1);
                state.active_selected = if first { 0 } else { last_index };
                state.active_selected_row_id = active_row_id;
            }
            PromptOverlayFocus::Inactive => {
                let last_index = inactive_count.saturating_sub(1);
                state.inactive_selected = if first { 0 } else { last_index };
                state.inactive_selected_row_id = inactive_reference_id;
            }
        }
        self.sync_prompt_overlay_state();
    }

    pub(super) fn move_prompt_overlay_active_selection(
        &mut self,
        direction: ListNavigationDirection,
    ) {
        let rows = self.prompt_overlay_left_rows();
        let count = rows.len();
        let Some(state) = self.prompt_overlay.as_mut() else {
            return;
        };
        if count == 0 {
            state.active_selected = 0;
            state.active_scroll = 0;
            state.active_selected_row_id = None;
            return;
        }

        let next = match direction {
            ListNavigationDirection::Previous => state.active_selected.saturating_sub(1),
            ListNavigationDirection::Next => state
                .active_selected
                .saturating_add(1)
                .min(count.saturating_sub(1)),
        };
        state.active_selected = next;
        state.active_selected_row_id = rows.get(next).map(prompt_overlay_left_row_id);
        self.sync_prompt_overlay_state();
    }

    pub(super) fn select_prompt_overlay_active_row(&mut self, visible_offset: usize) {
        let rows = self.prompt_overlay_left_rows();
        let total = rows.len();
        let current_scroll = self
            .prompt_overlay
            .as_ref()
            .map(|state| state.active_scroll)
            .unwrap_or_default();
        let selected = current_scroll.saturating_add(visible_offset);
        let Some(state) = self.prompt_overlay.as_mut() else {
            return;
        };
        if total == 0 {
            state.active_selected = 0;
            state.active_scroll = 0;
            state.active_selected_row_id = None;
            return;
        }
        let next_selected = selected.min(total.saturating_sub(1));
        state.active_selected = next_selected;
        state.active_selected_row_id = rows.get(next_selected).map(prompt_overlay_left_row_id);
        self.sync_prompt_overlay_state();
    }

    pub(super) fn move_prompt_overlay_inactive_selection(
        &mut self,
        direction: ListNavigationDirection,
    ) {
        let inactive_tab = match self.prompt_overlay.as_ref() {
            Some(state) => state.inactive_tab,
            None => return,
        };
        let source_count = self.prompt_overlay_inactive_source_count(inactive_tab);

        if source_count == 0 {
            let Some(state) = self.prompt_overlay.as_mut() else {
                return;
            };
            state.inactive_selected = 0;
            state.inactive_scroll = 0;
            state.inactive_selected_row_id = None;
            return;
        }

        let current_selected = match self.prompt_overlay.as_ref() {
            Some(state) => state.inactive_selected,
            None => return,
        };
        let next = match direction {
            ListNavigationDirection::Previous => current_selected.saturating_sub(1),
            ListNavigationDirection::Next => current_selected
                .saturating_add(1)
                .min(source_count.saturating_sub(1)),
        };
        let next_reference_id = self
            .prompt_overlay_inactive_rows(inactive_tab)
            .get(next)
            .map(prompt_overlay_inactive_row_id);

        let Some(state) = self.prompt_overlay.as_mut() else {
            return;
        };
        state.inactive_selected = next;
        state.inactive_selected_row_id = next_reference_id;
        self.sync_prompt_overlay_state();
    }

    pub(super) fn select_prompt_overlay_inactive_row(&mut self, visible_offset: usize) {
        let (inactive_tab, current_scroll, rows) = match self.prompt_overlay.as_ref() {
            Some(state) => (
                state.inactive_tab,
                state.inactive_scroll,
                self.prompt_overlay_inactive_rows(state.inactive_tab),
            ),
            None => return,
        };
        if rows.is_empty() {
            let Some(state) = self.prompt_overlay.as_mut() else {
                return;
            };
            state.inactive_selected = 0;
            state.inactive_scroll = 0;
            state.inactive_selected_row_id = None;
            return;
        }

        let selected = current_scroll
            .saturating_add(visible_offset)
            .min(rows.len().saturating_sub(1));
        let row_id = rows.get(selected).map(prompt_overlay_inactive_row_id);
        let Some(state) = self.prompt_overlay.as_mut() else {
            return;
        };
        if state.inactive_tab != inactive_tab {
            return;
        }
        state.inactive_selected = selected;
        state.inactive_selected_row_id = row_id;
        self.sync_prompt_overlay_state();
    }
}
