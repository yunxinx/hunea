use super::{shared::entry_tree_preview_footer_hint, *};

impl Model {
    pub(in crate::entry_tree::render) fn render_entry_tree_preview(
        &mut self,
        frame: &mut RenderFrame<'_>,
        area: Rect,
    ) {
        let palette = self.palette;
        let content_height = usize::from(area.height.saturating_sub(2).max(1));
        let Some(preview) = self.entry_tree_message_preview_mut() else {
            return;
        };
        render_transcript_overlay_view(
            frame,
            area,
            &mut preview.transcript,
            &mut preview.overlay,
            TranscriptOverlayRenderOptions {
                palette,
                content_height,
                footer_hint: entry_tree_preview_footer_hint(area.width),
                progress_style: TranscriptOverlayProgressStyle::Page,
            },
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
