use crate::theme::build_page_rule;

use super::{
    shared::{EntryTreeRowsRenderState, EntryTreeWidget},
    *,
};

impl Model {
    pub(in crate::entry_tree::render) fn render_entry_tree_branch_preview(
        &self,
        frame: &mut RenderFrame<'_>,
        area: Rect,
    ) {
        let Some(preview) = self
            .entry_tree
            .as_ref()
            .and_then(|state| state.branch_preview.as_ref())
        else {
            return;
        };
        frame.render_widget(Clear, area);
        let Some(chrome) = fullscreen_list_chrome_rects(area) else {
            return;
        };
        let page_size = entry_tree_page_size_for_height(area.height);
        let rows_state = EntryTreeRowsRenderState::from_branch_preview(preview);

        let should_show_branch_metadata =
            matches!(preview.source, EntryTreeBranchPreviewSource::BranchTree);
        let title = entry_tree_branch_preview_title(preview, should_show_branch_metadata);
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::raw("  "),
                Span::styled(
                    truncate_display_width_with_ellipsis(
                        &title,
                        usize::from(area.width).saturating_sub(2).max(1),
                    ),
                    approval_rejected_text_style(self.palette).bold(),
                ),
            ])),
            chrome.header,
        );
        frame.render_widget(
            Paragraph::new(subtle_rule_line(usize::from(area.width), self.palette)),
            chrome.header_rule,
        );

        let lines = self.entry_tree_body_lines(
            rows_state,
            usize::from(area.width),
            usize::from(chrome.body.height),
        );
        frame.render_widget(EntryTreeWidget { lines: &lines }, chrome.body);

        frame.render_widget(
            Paragraph::new(build_page_rule(
                area.width,
                rows_state.page_number(page_size),
                rows_state.page_count(page_size),
                self.palette,
            )),
            chrome.page_rule,
        );
        frame.render_widget(
            Paragraph::new(Line::styled(
                self.entry_tree_branch_preview_footer_hint(),
                tertiary_text_style(self.palette).add_modifier(Modifier::ITALIC),
            )),
            chrome.footer,
        );
    }

    fn entry_tree_branch_preview_footer_hint(&self) -> &'static str {
        if self
            .entry_tree
            .as_ref()
            .and_then(|state| state.branch_preview.as_ref())
            .is_some_and(|preview| {
                matches!(preview.source, EntryTreeBranchPreviewSource::BranchTree)
            })
        {
            "  Esc back to branch tree · Space preview message · ↑/↓/j/k move"
        } else {
            "  Esc back to branch list · Space preview message · ↑/↓/j/k move"
        }
    }
}

fn entry_tree_branch_preview_title(
    preview: &EntryTreeBranchPreviewState,
    should_show_branch_metadata: bool,
) -> String {
    let position = format!(
        "{} of {}",
        preview.selected_position_label(),
        preview.rows.len()
    );
    if !should_show_branch_metadata {
        return format!("Branch Preview ({position})");
    }

    let Some(metadata) = preview.metadata.as_ref() else {
        return format!("Branch Preview ({position})");
    };

    let created = crate::relative_age::relative_age_label(
        metadata.metadata_now_ms,
        metadata.branch_created_at_ms,
    );
    let updated = crate::relative_age::relative_age_label(
        metadata.metadata_now_ms,
        metadata.latest_updated_at_ms,
    );
    format!("Branch Preview ({position} · Created {created} · Updated {updated})")
}
