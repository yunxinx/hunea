use crate::theme::build_page_rule;

use super::{
    shared::{EntryTreeRowsRenderState, EntryTreeWidget},
    *,
};

impl Model {
    pub(in crate::entry_tree::render) fn render_entry_tree_main_list(
        &self,
        frame: &mut RenderFrame<'_>,
        area: Rect,
        state: &EntryTreeState,
    ) {
        frame.render_widget(Clear, area);
        let Some(chrome) = fullscreen_list_chrome_rects(area) else {
            return;
        };
        let page_size = entry_tree_page_size_for_height(area.height);
        let rows_state = EntryTreeRowsRenderState::from_tree(state);

        frame.render_widget(
            Paragraph::new(self.entry_tree_header_line(state, usize::from(area.width))),
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
                state.page_number(page_size),
                state.page_count(page_size),
                self.palette,
            )),
            chrome.page_rule,
        );
        frame.render_widget(
            Paragraph::new(Line::styled(
                entry_tree_footer_hint(
                    area.width,
                    state
                        .selected_row()
                        .is_some_and(|row| row.rewind_target_id.is_some()),
                    state
                        .selected_row()
                        .is_some_and(|row| row.branch_choices.len() >= 2),
                ),
                tertiary_text_style(self.palette).add_modifier(Modifier::ITALIC),
            )),
            chrome.footer,
        );

        if state.branch_picker.is_some() {
            self.render_entry_tree_branch_picker(frame, area);
        }
    }

    fn entry_tree_header_line(&self, state: &EntryTreeState, width: usize) -> Line<'static> {
        let title = format!(
            "Session Tree ({} of {})",
            state.selected_position_label(),
            state.rows.len()
        );
        Line::from(vec![
            Span::raw("  "),
            Span::styled(
                truncate_display_width_with_ellipsis(&title, width.saturating_sub(2).max(1)),
                primary_text_style(self.palette).bold(),
            ),
        ])
    }
}

fn entry_tree_footer_hint(
    width: u16,
    can_rewind_selected_row: bool,
    can_open_branch_picker: bool,
) -> &'static str {
    match (width < 90, can_rewind_selected_row, can_open_branch_picker) {
        (true, true, true) => {
            "  Esc close · Space preview · Tab branch · A branch tree · Enter rewind · j/k · h/l page"
        }
        (true, true, false) => {
            "  Esc close · Space preview · A branch tree · Enter rewind · j/k · h/l page"
        }
        (true, false, true) => {
            "  Esc close · Space preview · Tab branch · A branch tree · j/k · h/l page"
        }
        (true, false, false) => "  Esc close · Space preview · A branch tree · j/k · h/l page",
        (false, true, true) => {
            "  Esc close · Space preview · Tab branch · A branch tree · Enter rewind · ↑/↓/j/k move · ←/→/h/l page"
        }
        (false, true, false) => {
            "  Esc close · Space preview · A branch tree · Enter rewind · ↑/↓/j/k move · ←/→/h/l page"
        }
        (false, false, true) => {
            "  Esc close · Space preview · Tab branch · A branch tree · ↑/↓/j/k move · ←/→/h/l page"
        }
        (false, false, false) => {
            "  Esc close · Space preview · A branch tree · ↑/↓/j/k move · ←/→/h/l page"
        }
    }
}
