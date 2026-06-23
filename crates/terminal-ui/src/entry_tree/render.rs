use super::*;

mod branch_picker_view;
mod branch_preview_view;
mod branch_tree_view;
mod main_list;
mod message_preview;
mod shared;

#[cfg(test)]
pub(super) use shared::branch_picker_relative_age_label;

impl Model {
    pub(crate) fn render_entry_tree(&mut self, frame: &mut RenderFrame<'_>, area: Rect) {
        if self.entry_tree_preview_active() {
            self.render_entry_tree_preview(frame, area);
            return;
        }

        if self.entry_tree_branch_preview_active() {
            self.render_entry_tree_branch_preview(frame, area);
            return;
        }

        if self.entry_tree_branch_tree_active() {
            self.render_entry_tree_branch_tree(frame, area);
            return;
        }

        let Some(state) = self.entry_tree.as_ref() else {
            return;
        };
        self.render_entry_tree_main_list(frame, area, state);
    }

    pub(super) fn entry_tree_page_size(&self) -> usize {
        entry_tree_page_size_for_height(self.height)
    }
}
