use super::*;

impl Model {
    pub(crate) fn sync_prompt_overlay_state(&mut self) {
        let inactive_tab = match self.prompt_overlay.as_ref() {
            Some(state) => state.inactive_tab,
            None => return,
        };
        let inactive_source_count = self.prompt_overlay_inactive_source_count(inactive_tab);

        let active_count = self.prompt_overlay_left_rows().len();
        let (
            current_active_selected,
            current_active_scroll,
            current_active_selected_row_id,
            current_inactive_selected,
            current_inactive_scroll,
            current_inactive_reference_id,
        ) = match self.prompt_overlay.as_ref() {
            Some(state) => (
                state.active_selected,
                state.active_scroll,
                state.active_selected_row_id.clone(),
                state.inactive_selected,
                state.inactive_scroll,
                state.inactive_selected_row_id.clone(),
            ),
            None => return,
        };

        let active_rows = self.prompt_overlay_left_rows();
        let mut next_active_selected = current_active_selected;
        if let Some(row_id) = current_active_selected_row_id.as_deref()
            && let Some(index) = active_rows
                .iter()
                .position(|row| prompt_overlay_left_row_id(row) == row_id)
        {
            next_active_selected = index;
        }
        next_active_selected = next_active_selected.min(active_count.saturating_sub(1));
        let next_active_selected_row_id = active_rows
            .get(next_active_selected)
            .map(prompt_overlay_left_row_id);
        let next_active_scroll = clamp_scroll(
            current_active_scroll,
            next_active_selected,
            active_count,
            prompt_overlay_active_visible_rows(self.height),
        );

        let mut next_inactive_selected = current_inactive_selected;
        let inactive_rows = self.prompt_overlay_inactive_rows(inactive_tab);
        if let Some(reference_id) = current_inactive_reference_id.as_deref() {
            let matched_index = inactive_rows
                .iter()
                .position(|row| prompt_overlay_inactive_row_id(row) == reference_id);
            if let Some(index) = matched_index {
                next_inactive_selected = index;
            }
        }
        next_inactive_selected =
            next_inactive_selected.min(inactive_source_count.saturating_sub(1));
        let next_inactive_reference_id = inactive_rows
            .get(next_inactive_selected)
            .map(prompt_overlay_inactive_row_id);

        let next_inactive_scroll = clamp_scroll(
            current_inactive_scroll,
            next_inactive_selected,
            inactive_source_count,
            prompt_overlay_inactive_visible_rows(self.height),
        );

        let Some(state) = self.prompt_overlay.as_mut() else {
            return;
        };
        state.active_selected = next_active_selected;
        state.active_scroll = next_active_scroll;
        state.active_selected_row_id = next_active_selected_row_id;
        state.inactive_selected = next_inactive_selected;
        state.inactive_selected_row_id = next_inactive_reference_id;
        state.inactive_scroll = next_inactive_scroll;
    }
}
