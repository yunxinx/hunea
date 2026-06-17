use super::*;

impl Model {
    pub(crate) fn entry_tree_active(&self) -> bool {
        self.entry_tree.is_some()
    }

    pub(crate) fn open_entry_tree_loading(&mut self) {
        self.entry_tree = Some(EntryTreeState {
            is_loading: true,
            ..EntryTreeState::default()
        });
    }

    pub(crate) fn apply_entry_tree_payload(&mut self, payload: SessionTreePayload) {
        let mut state = self.entry_tree.take().unwrap_or_default();
        let current_row_id = payload.current_row_id;
        state.rows = payload.rows;
        state.is_loading = false;
        state.error = None;
        state.preview = None;
        state.branch_picker = None;
        state.branch_tree = None;
        state.branch_preview = None;
        if !state.select_row_by_id(current_row_id.as_deref()) {
            state.select_latest_row();
        }
        self.entry_tree = Some(state);
    }

    pub(crate) fn show_entry_tree_error(&mut self, message: &str) {
        let mut state = self.entry_tree.take().unwrap_or_default();
        state.is_loading = false;
        state.error = Some(message.to_string());
        self.entry_tree = Some(state);
    }

    pub(crate) fn move_entry_tree_selection_by_delta(&mut self, delta: isize) {
        let Some(direction) = ListNavigationDirection::from_delta(delta) else {
            return;
        };
        self.move_entry_tree_selection(direction);
    }

    fn move_entry_tree_selection(&mut self, direction: ListNavigationDirection) {
        if self.entry_tree_branch_preview_active() {
            self.move_entry_tree_branch_preview_selection(direction);
            return;
        }
        if self.entry_tree_branch_picker_active() {
            self.move_entry_tree_branch_picker_selection(direction);
            return;
        }
        if self.entry_tree_branch_tree_active() {
            self.move_entry_tree_branch_tree_selection(direction);
            return;
        }
        if let Some(state) = self.entry_tree.as_mut() {
            state.move_selection(direction);
        }
    }

    pub(crate) fn move_entry_tree_preview_page(&mut self, direction: isize) {
        let content_height = self.transcript_overlay_content_height();
        if let Some(preview) = self
            .entry_tree
            .as_mut()
            .and_then(|state| state.preview.as_mut())
        {
            preview.overlay.scroll_offset = entry_tree_preview_page_offset(
                &mut preview.transcript,
                content_height,
                preview.overlay.scroll_offset,
                direction,
            );
            preview.is_following_bottom = false;
            return;
        }

        if let Some(preview) = self
            .entry_tree
            .as_mut()
            .and_then(|state| state.branch_preview.as_mut())
            .and_then(|preview| preview.message_preview.as_mut())
        {
            preview.overlay.scroll_offset = entry_tree_preview_page_offset(
                &mut preview.transcript,
                content_height,
                preview.overlay.scroll_offset,
                direction,
            );
            preview.is_following_bottom = false;
        }
    }

    pub(crate) fn handle_entry_tree_key(&mut self, key: KeyEvent) -> OverlayKeyResult {
        if !self.entry_tree_active() {
            return OverlayKeyResult::Ignored;
        }

        if self.entry_tree_preview_active() {
            return self.handle_entry_tree_preview_key(key);
        }

        if self.entry_tree_branch_preview_active() {
            return self.handle_entry_tree_branch_preview_key(key);
        }

        if self.entry_tree_branch_picker_active() {
            return self.handle_entry_tree_branch_picker_key(key);
        }

        if self.entry_tree_branch_tree_active() {
            return self.handle_entry_tree_branch_tree_key(key);
        }

        match key.code {
            KeyCode::Esc if key.modifiers.is_empty() => {
                self.entry_tree = None;
                OverlayKeyResult::Handled
            }
            KeyCode::Up | KeyCode::Char('k') if key.modifiers.is_empty() => {
                self.move_entry_tree_selection(ListNavigationDirection::Previous);
                OverlayKeyResult::Handled
            }
            KeyCode::Down | KeyCode::Char('j') if key.modifiers.is_empty() => {
                self.move_entry_tree_selection(ListNavigationDirection::Next);
                OverlayKeyResult::Handled
            }
            KeyCode::Left | KeyCode::Char('h') if key.modifiers.is_empty() => {
                let page_size = self.entry_tree_page_size();
                if let Some(state) = self.entry_tree.as_mut() {
                    state.move_page(ListNavigationDirection::Previous, page_size);
                }
                OverlayKeyResult::Handled
            }
            KeyCode::Right | KeyCode::Char('l') if key.modifiers.is_empty() => {
                let page_size = self.entry_tree_page_size();
                if let Some(state) = self.entry_tree.as_mut() {
                    state.move_page(ListNavigationDirection::Next, page_size);
                }
                OverlayKeyResult::Handled
            }
            KeyCode::Char(' ') if key.modifiers.is_empty() => {
                self.open_entry_tree_preview();
                OverlayKeyResult::Handled
            }
            KeyCode::Tab if key.modifiers.is_empty() => {
                self.open_entry_tree_branch_picker();
                OverlayKeyResult::Handled
            }
            KeyCode::Char('A') if is_entry_tree_branch_tree_shortcut(key) => {
                OverlayKeyResult::Effect(AppEffect::OpenBranchTree)
            }
            KeyCode::Enter if key.modifiers.is_empty() => {
                let selected = self
                    .entry_tree
                    .as_ref()
                    .and_then(EntryTreeState::selected_row);
                if let Some(row) = selected
                    && row.rewind_target_id.is_some()
                {
                    let entry_id = row.row_id.clone();
                    let prefill = row.rewind_prefill.clone();
                    self.entry_tree = None;
                    return OverlayKeyResult::Effect(AppEffect::SelectEntryRewind {
                        entry_id,
                        prefill,
                    });
                }
                OverlayKeyResult::Handled
            }
            _ => OverlayKeyResult::Handled,
        }
    }

    pub(crate) fn handle_entry_tree_mouse_down(
        &mut self,
        button: MouseButton,
        column: u16,
        row: u16,
    ) -> Option<AppEffect> {
        if button != MouseButton::Left
            || !self.entry_tree_active()
            || self.entry_tree_preview_active()
        {
            return None;
        }
        if self.handle_entry_tree_branch_picker_mouse_down(column, row) {
            return None;
        }
        if self.handle_entry_tree_branch_preview_mouse_down(row) {
            return None;
        }
        if self.handle_entry_tree_branch_tree_mouse_down(row) {
            return None;
        }

        let should_close_branch_picker_after_tree_selection =
            self.entry_tree_branch_picker_active() && !self.entry_tree_branch_preview_active();
        let page_size = self.entry_tree_page_size();
        let visible_offset = fullscreen_list_body_visible_offset_for_row(self.height, row)?;
        if let Some(state) = self.entry_tree.as_mut() {
            let selected_visible_row = state.select_visible_row(page_size, visible_offset);
            if selected_visible_row && should_close_branch_picker_after_tree_selection {
                state.branch_picker = None;
            }
        }
        None
    }

    fn handle_entry_tree_branch_picker_mouse_down(&mut self, column: u16, row: u16) -> bool {
        if self.entry_tree_branch_preview_active() {
            return false;
        }

        let area = Rect::new(0, 0, self.width, self.height);
        if area.is_empty() {
            return false;
        }

        let list_rows = self.entry_tree_branch_picker_visible_rows();
        let Some(popup_area) = self
            .entry_tree
            .as_ref()
            .and_then(|state| state.branch_picker.as_ref().map(|_| state))
            .map(|state| entry_tree_branch_picker_area_for_state(state, area, list_rows))
        else {
            return false;
        };

        if !rect_contains(popup_area, column, row) {
            return false;
        }

        let item_rows = popup_area
            .height
            .saturating_sub(BRANCH_PICKER_CHROME_HEIGHT);
        let item_top = popup_area.y.saturating_add(BRANCH_PICKER_ITEM_TOP_OFFSET);
        if row >= item_top && row < item_top.saturating_add(item_rows) {
            let visible_offset = usize::from(row.saturating_sub(item_top));
            if let Some(picker) = self
                .entry_tree
                .as_mut()
                .and_then(|state| state.branch_picker.as_mut())
            {
                picker.select_visible_item(visible_offset, list_rows);
            }
        }

        true
    }

    fn handle_entry_tree_branch_preview_mouse_down(&mut self, row: u16) -> bool {
        if !self.entry_tree_branch_preview_active() {
            return false;
        }
        if let Some(visible_offset) = fullscreen_list_body_visible_offset_for_row(self.height, row)
        {
            let page_size = self.entry_tree_page_size();
            if let Some(preview) = self
                .entry_tree
                .as_mut()
                .and_then(|state| state.branch_preview.as_mut())
            {
                preview.select_visible_row(page_size, visible_offset);
            }
        }

        true
    }

    pub(crate) fn entry_tree_branch_picker_active(&self) -> bool {
        self.entry_tree
            .as_ref()
            .is_some_and(|state| state.branch_picker.is_some())
    }

    fn open_entry_tree_branch_picker(&mut self) {
        let visible_rows = self.entry_tree_branch_picker_visible_rows();
        let Some(selected_row) = self
            .entry_tree
            .as_ref()
            .and_then(EntryTreeState::selected_row)
            .cloned()
        else {
            return;
        };
        if selected_row.branch_choices.len() < 2 {
            return;
        }

        let selected = 0;
        if let Some(state) = self.entry_tree.as_mut() {
            let mut branch_picker = EntryTreeBranchPickerState {
                items: selected_row.branch_choices,
                selected,
                scroll: 0,
                metadata_now_ms: current_unix_timestamp_ms(),
                error: None,
            };
            branch_picker.scroll_to_selection(visible_rows);
            state.branch_picker = Some(branch_picker);
            state.preview = None;
            state.branch_preview = None;
        }
    }

    fn move_entry_tree_branch_picker_selection(&mut self, direction: ListNavigationDirection) {
        let visible_rows = self.entry_tree_branch_picker_visible_rows();
        let Some(picker) = self
            .entry_tree
            .as_mut()
            .and_then(|state| state.branch_picker.as_mut())
        else {
            return;
        };
        picker.move_selection(direction, visible_rows);
    }

    fn handle_entry_tree_branch_picker_key(&mut self, key: KeyEvent) -> OverlayKeyResult {
        match key.code {
            KeyCode::Esc if key.modifiers.is_empty() => {
                if let Some(state) = self.entry_tree.as_mut() {
                    state.branch_picker = None;
                }
                OverlayKeyResult::Handled
            }
            KeyCode::Up | KeyCode::Char('k') if key.modifiers.is_empty() => {
                self.move_entry_tree_branch_picker_selection(ListNavigationDirection::Previous);
                OverlayKeyResult::Handled
            }
            KeyCode::Down | KeyCode::Char('j') if key.modifiers.is_empty() => {
                self.move_entry_tree_branch_picker_selection(ListNavigationDirection::Next);
                OverlayKeyResult::Handled
            }
            KeyCode::Char(' ') if key.modifiers.is_empty() => {
                let Some((branch_row_id, preview_metadata, is_current)) = self
                    .entry_tree
                    .as_ref()
                    .and_then(|state| state.branch_picker.as_ref())
                    .and_then(|picker| {
                        let selected_branch = picker.selected_item()?;
                        let preview_metadata = EntryTreeBranchPreviewMetadata::from_branch_choice(
                            selected_branch,
                            picker.metadata_now_ms,
                        );
                        Some((
                            selected_branch.branch.branch_row_id.clone(),
                            preview_metadata,
                            selected_branch.branch.is_current,
                        ))
                    })
                else {
                    return OverlayKeyResult::Handled;
                };
                if is_current {
                    return OverlayKeyResult::Handled;
                }
                if let Some(state) = self.entry_tree.as_mut() {
                    state.branch_preview = Some(EntryTreeBranchPreviewState {
                        metadata: Some(preview_metadata),
                        source: EntryTreeBranchPreviewSource::BranchPicker,
                        ..EntryTreeBranchPreviewState::default()
                    });
                    if let Some(picker) = state.branch_picker.as_mut() {
                        picker.error = None;
                    }
                }
                OverlayKeyResult::Effect(AppEffect::OpenBranchPreview { branch_row_id })
            }
            KeyCode::Enter if key.modifiers.is_empty() => {
                let Some(leaf_id) = self
                    .entry_tree
                    .as_ref()
                    .and_then(|state| state.branch_picker.as_ref())
                    .and_then(EntryTreeBranchPickerState::selected_item)
                    .map(|branch| branch.branch.subtree_leaf_id.clone())
                else {
                    return OverlayKeyResult::Handled;
                };
                OverlayKeyResult::Effect(AppEffect::SwitchBranch { leaf_id })
            }
            _ => OverlayKeyResult::Handled,
        }
    }

    pub(crate) fn apply_entry_tree_branch_preview_payload(&mut self, payload: SessionTreePayload) {
        let Some(state) = self.entry_tree.as_mut() else {
            return;
        };
        let mut preview = state.branch_preview.take().unwrap_or_default();
        let current_row_id = payload.current_row_id;
        preview.rows = payload.rows;
        preview.is_loading = false;
        preview.error = None;
        preview.message_preview = None;
        if !preview.select_row_by_id(current_row_id.as_deref()) {
            preview.select_latest_row();
        }
        state.branch_preview = Some(preview);
    }

    pub(crate) fn entry_tree_branch_preview_active(&self) -> bool {
        self.entry_tree
            .as_ref()
            .is_some_and(|state| state.branch_preview.is_some())
    }

    pub(crate) fn show_entry_tree_branch_picker_error(&mut self, message: &str) {
        let Some(picker) = self
            .entry_tree
            .as_mut()
            .and_then(|state| state.branch_picker.as_mut())
        else {
            self.show_transient_status_notice(message);
            return;
        };
        picker.error = Some(message.to_string());
    }

    fn move_entry_tree_branch_preview_selection(&mut self, direction: ListNavigationDirection) {
        if let Some(preview) = self
            .entry_tree
            .as_mut()
            .and_then(|state| state.branch_preview.as_mut())
        {
            preview.move_selection(direction);
        }
    }

    fn handle_entry_tree_branch_preview_key(&mut self, key: KeyEvent) -> OverlayKeyResult {
        match key.code {
            KeyCode::Esc if key.modifiers.is_empty() => {
                if let Some(state) = self.entry_tree.as_mut() {
                    state.branch_preview = None;
                }
                OverlayKeyResult::Handled
            }
            KeyCode::Up | KeyCode::Char('k') if key.modifiers.is_empty() => {
                self.move_entry_tree_branch_preview_selection(ListNavigationDirection::Previous);
                OverlayKeyResult::Handled
            }
            KeyCode::Down | KeyCode::Char('j') if key.modifiers.is_empty() => {
                self.move_entry_tree_branch_preview_selection(ListNavigationDirection::Next);
                OverlayKeyResult::Handled
            }
            KeyCode::Left | KeyCode::Char('h') if key.modifiers.is_empty() => {
                let page_size = self.entry_tree_page_size();
                if let Some(preview) = self
                    .entry_tree
                    .as_mut()
                    .and_then(|state| state.branch_preview.as_mut())
                {
                    preview.move_page(ListNavigationDirection::Previous, page_size);
                }
                OverlayKeyResult::Handled
            }
            KeyCode::Right | KeyCode::Char('l') if key.modifiers.is_empty() => {
                let page_size = self.entry_tree_page_size();
                if let Some(preview) = self
                    .entry_tree
                    .as_mut()
                    .and_then(|state| state.branch_preview.as_mut())
                {
                    preview.move_page(ListNavigationDirection::Next, page_size);
                }
                OverlayKeyResult::Handled
            }
            KeyCode::Char(' ') if key.modifiers.is_empty() => {
                self.open_entry_tree_preview();
                OverlayKeyResult::Handled
            }
            KeyCode::Enter if key.modifiers.is_empty() => OverlayKeyResult::Handled,
            _ => OverlayKeyResult::Handled,
        }
    }

    pub(crate) fn open_entry_tree_branch_tree_loading(&mut self) {
        let mut state = self.entry_tree.take().unwrap_or_default();
        state.preview = None;
        state.branch_picker = None;
        state.branch_preview = None;
        state.branch_tree = Some(EntryTreeBranchTreeState {
            is_loading: true,
            metadata_now_ms: current_unix_timestamp_ms(),
            ..EntryTreeBranchTreeState::default()
        });
        self.entry_tree = Some(state);
    }

    pub(crate) fn apply_entry_tree_branch_tree_payload(
        &mut self,
        payload: SessionBranchTreePayload,
    ) {
        let Some(state) = self.entry_tree.as_mut() else {
            return;
        };
        let mut branch_tree = state.branch_tree.take().unwrap_or_default();
        branch_tree.nodes = branch_tree_display_order_nodes(payload.nodes);
        branch_tree.selected = 0;
        branch_tree.is_loading = false;
        branch_tree.metadata_now_ms = current_unix_timestamp_ms();
        branch_tree.current_branch_row_id = payload.current_branch_row_id;
        branch_tree.total_message_count = payload.total_message_count;
        branch_tree.error = None;
        branch_tree.select_current_or_first();
        state.preview = None;
        state.branch_picker = None;
        state.branch_preview = None;
        state.branch_tree = Some(branch_tree);
    }

    pub(crate) fn entry_tree_branch_tree_active(&self) -> bool {
        self.entry_tree
            .as_ref()
            .is_some_and(|state| state.branch_tree.is_some())
    }

    fn move_entry_tree_branch_tree_selection(&mut self, direction: ListNavigationDirection) {
        if let Some(branch_tree) = self
            .entry_tree
            .as_mut()
            .and_then(|state| state.branch_tree.as_mut())
        {
            branch_tree.move_selection(direction);
        }
    }

    fn handle_entry_tree_branch_tree_key(&mut self, key: KeyEvent) -> OverlayKeyResult {
        match key.code {
            KeyCode::Esc if key.modifiers.is_empty() => {
                if let Some(state) = self.entry_tree.as_mut() {
                    state.branch_tree = None;
                    state.branch_preview = None;
                }
                OverlayKeyResult::Handled
            }
            KeyCode::Up | KeyCode::Char('k') if key.modifiers.is_empty() => {
                self.move_entry_tree_branch_tree_selection(ListNavigationDirection::Previous);
                OverlayKeyResult::Handled
            }
            KeyCode::Down | KeyCode::Char('j') if key.modifiers.is_empty() => {
                self.move_entry_tree_branch_tree_selection(ListNavigationDirection::Next);
                OverlayKeyResult::Handled
            }
            KeyCode::Left | KeyCode::Char('h') if key.modifiers.is_empty() => {
                let page_size = entry_tree_branch_tree_page_size_for_height(self.height);
                if let Some(branch_tree) = self
                    .entry_tree
                    .as_mut()
                    .and_then(|state| state.branch_tree.as_mut())
                {
                    branch_tree.move_page(ListNavigationDirection::Previous, page_size);
                }
                OverlayKeyResult::Handled
            }
            KeyCode::Right | KeyCode::Char('l') if key.modifiers.is_empty() => {
                let page_size = entry_tree_branch_tree_page_size_for_height(self.height);
                if let Some(branch_tree) = self
                    .entry_tree
                    .as_mut()
                    .and_then(|state| state.branch_tree.as_mut())
                {
                    branch_tree.move_page(ListNavigationDirection::Next, page_size);
                }
                OverlayKeyResult::Handled
            }
            KeyCode::Char(' ') if key.modifiers.is_empty() => {
                let Some((branch_row_id, preview_metadata, is_current)) = self
                    .entry_tree
                    .as_ref()
                    .and_then(|state| state.branch_tree.as_ref())
                    .and_then(|branch_tree| {
                        let selected_node = branch_tree.selected_node()?;
                        let preview_metadata =
                            EntryTreeBranchPreviewMetadata::from_branch_tree_node(
                                selected_node,
                                branch_tree.metadata_now_ms,
                            );
                        Some((
                            selected_node.branch.branch_row_id.clone(),
                            preview_metadata,
                            selected_node.branch.is_current,
                        ))
                    })
                else {
                    return OverlayKeyResult::Handled;
                };
                if is_current {
                    return OverlayKeyResult::Handled;
                }
                if let Some(state) = self.entry_tree.as_mut() {
                    state.branch_preview = Some(EntryTreeBranchPreviewState {
                        metadata: Some(preview_metadata),
                        source: EntryTreeBranchPreviewSource::BranchTree,
                        ..EntryTreeBranchPreviewState::default()
                    });
                }
                OverlayKeyResult::Effect(AppEffect::OpenBranchPreview { branch_row_id })
            }
            KeyCode::Enter if key.modifiers.is_empty() => {
                let Some(selected_node) = self
                    .entry_tree
                    .as_ref()
                    .and_then(|state| state.branch_tree.as_ref())
                    .and_then(EntryTreeBranchTreeState::selected_node)
                else {
                    return OverlayKeyResult::Handled;
                };
                if selected_node.branch.is_current {
                    return OverlayKeyResult::Handled;
                }
                OverlayKeyResult::Effect(AppEffect::SwitchBranch {
                    leaf_id: selected_node.branch.subtree_leaf_id.clone(),
                })
            }
            _ => OverlayKeyResult::Handled,
        }
    }

    fn handle_entry_tree_branch_tree_mouse_down(&mut self, row: u16) -> bool {
        if !self.entry_tree_branch_tree_active() {
            return false;
        }
        if self.height < ENTRY_TREE_CHROME_HEIGHT {
            return true;
        }

        let body_top = ENTRY_TREE_HEADER_HEIGHT + ENTRY_TREE_HEADER_RULE_HEIGHT;
        let body_height = self.height.saturating_sub(ENTRY_TREE_CHROME_HEIGHT);
        if row > body_top && row < body_top.saturating_add(body_height) {
            let page_size = entry_tree_branch_tree_page_size_for_height(self.height);
            let visible_offset = usize::from(row.saturating_sub(body_top).saturating_sub(1));
            if let Some(branch_tree) = self
                .entry_tree
                .as_mut()
                .and_then(|state| state.branch_tree.as_mut())
            {
                branch_tree.select_visible_node(page_size, visible_offset);
            }
        }

        true
    }

    pub(super) fn entry_tree_branch_picker_visible_rows(&self) -> usize {
        usize::from(
            self.branch_picker_list_rows
                .clamp(BRANCH_PICKER_LIST_ROWS_MIN, BRANCH_PICKER_LIST_ROWS_MAX),
        )
    }

    pub(crate) fn entry_tree_preview_active(&self) -> bool {
        self.entry_tree.as_ref().is_some_and(|state| {
            state.preview.is_some()
                || state
                    .branch_preview
                    .as_ref()
                    .is_some_and(|preview| preview.message_preview.is_some())
        })
    }

    fn open_entry_tree_preview(&mut self) {
        let from_branch_preview = self.entry_tree_branch_preview_active();
        let selected_row = if from_branch_preview {
            self.entry_tree
                .as_ref()
                .and_then(|state| state.branch_preview.as_ref())
                .and_then(EntryTreeBranchPreviewState::selected_row)
                .cloned()
        } else {
            self.entry_tree
                .as_ref()
                .and_then(EntryTreeState::selected_row)
                .cloned()
        };
        let Some(row) = selected_row else {
            return;
        };

        let mut transcript = self.transcript_from_replay_items_with_tool_activity_render_mode(
            entry_tree_preview_replay_items(&row),
            ToolActivityRenderMode::DebugDetailed,
        );
        transcript.set_reasoning_render_mode(ReasoningRenderMode::Detailed);
        let content_height = self.transcript_overlay_content_height();
        let mut preview = EntryTreePreviewState::following_bottom(transcript);
        preview.overlay.scroll_offset =
            latest_entry_tree_preview_offset(&mut preview.transcript, content_height);

        if let Some(preview_state) = self
            .entry_tree
            .as_mut()
            .and_then(|state| state.branch_preview.as_mut())
            .filter(|_| from_branch_preview)
        {
            preview_state.message_preview = Some(preview);
        } else if let Some(state) = self.entry_tree.as_mut() {
            state.preview = Some(preview);
        }
    }

    pub(crate) fn sync_entry_tree_preview_follow_bottom(&mut self) {
        let content_height = self.transcript_overlay_content_height();
        let Some(state) = self.entry_tree.as_mut() else {
            return;
        };
        if let Some(preview) = state
            .preview
            .as_mut()
            .filter(|preview| preview.is_following_bottom)
        {
            preview.overlay.scroll_offset =
                latest_entry_tree_preview_offset(&mut preview.transcript, content_height);
        }
        if let Some(preview) = state
            .branch_preview
            .as_mut()
            .and_then(|preview| preview.message_preview.as_mut())
            .filter(|preview| preview.is_following_bottom)
        {
            preview.overlay.scroll_offset =
                latest_entry_tree_preview_offset(&mut preview.transcript, content_height);
        }
    }

    fn close_entry_tree_preview(&mut self) {
        if let Some(state) = self.entry_tree.as_mut() {
            if state.preview.is_some() {
                state.preview = None;
                return;
            }
            if let Some(preview) = state.branch_preview.as_mut() {
                preview.message_preview = None;
            }
        }
    }

    fn handle_entry_tree_preview_key(&mut self, key: KeyEvent) -> OverlayKeyResult {
        match key.code {
            KeyCode::Esc | KeyCode::Char(' ') if key.modifiers.is_empty() => {
                self.close_entry_tree_preview();
                OverlayKeyResult::Handled
            }
            KeyCode::Left | KeyCode::Up | KeyCode::Char('h') if key.modifiers.is_empty() => {
                self.move_entry_tree_preview_page(-1);
                OverlayKeyResult::Handled
            }
            KeyCode::Right | KeyCode::Down | KeyCode::Char('l') if key.modifiers.is_empty() => {
                self.move_entry_tree_preview_page(1);
                OverlayKeyResult::Handled
            }
            _ => OverlayKeyResult::Handled,
        }
    }
}
