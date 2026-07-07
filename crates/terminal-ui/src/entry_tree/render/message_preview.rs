use super::{shared::entry_tree_preview_footer_hint, *};

impl Model {
    pub(in crate::entry_tree::render) fn render_entry_tree_preview(
        &mut self,
        frame: &mut RenderFrame<'_>,
        area: Rect,
    ) {
        let palette = self.palette;
        let Some(preview) = self.entry_tree_message_preview_mut() else {
            return;
        };
        preview.mode.render(
            frame,
            area,
            palette,
            entry_tree_preview_footer_hint(area.width),
        );
    }

    fn entry_tree_message_preview_mut(&mut self) -> Option<&mut EntryTreePreviewState> {
        let state = self.entry_tree.as_mut()?;
        if state.preview.is_some() {
            return state.preview.as_mut();
        }
        state.branch_preview.as_mut()?.message_preview.as_mut()
    }
}
