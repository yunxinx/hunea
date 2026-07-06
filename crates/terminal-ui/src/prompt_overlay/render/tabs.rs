use super::*;

impl Model {
    pub(in crate::prompt_overlay) fn prompt_overlay_tabs_plain(
        &self,
        active_tab: PromptOverlayInactiveTab,
    ) -> String {
        prompt_overlay_tab_labels(active_tab)
            .into_iter()
            .map(|(_, _, label)| label)
            .collect::<Vec<_>>()
            .join(" ")
    }

    pub(in crate::prompt_overlay) fn prompt_overlay_tabs_spans(
        &self,
        active_tab: PromptOverlayInactiveTab,
    ) -> Vec<Span<'static>> {
        let mut spans = Vec::new();

        for (index, (_, is_active, label)) in prompt_overlay_tab_labels(active_tab)
            .into_iter()
            .enumerate()
        {
            if index > 0 {
                spans.push(Span::raw(" "));
            }
            let style = if is_active {
                surface_text_style(self.palette).bold()
            } else {
                tertiary_text_style(self.palette)
            }
            .add_modifier(Modifier::UNDERLINED);
            spans.push(Span::styled(label, style));
        }

        spans
    }

    pub(in crate::prompt_overlay) fn prompt_overlay_header_tab_at(
        &self,
        column: u16,
        row: u16,
        header_area: Rect,
        active_tab: PromptOverlayInactiveTab,
    ) -> Option<PromptOverlayInactiveTab> {
        if row != header_area.y
            || column < header_area.x
            || column >= header_area.x.saturating_add(header_area.width)
        {
            return None;
        }

        let width = usize::from(header_area.width);
        let title = "Prompt Assembly";
        let tabs = self.prompt_overlay_tabs_plain(active_tab);
        let available_width = width.saturating_sub(PROMPT_OVERLAY_HEADER_INSET);
        let tabs_width = display_width(&tabs) + PROMPT_OVERLAY_HEADER_TRAILING_PADDING;
        let title_width = available_width
            .saturating_sub(tabs_width)
            .saturating_sub(1)
            .max(1);
        let title = truncate_display_width_with_ellipsis(title, title_width);
        let padding = available_width
            .saturating_sub(display_width(&title))
            .saturating_sub(tabs_width)
            .max(1);
        let mut current_column = usize::from(header_area.x)
            .saturating_add(PROMPT_OVERLAY_HEADER_INSET)
            .saturating_add(display_width(&title))
            .saturating_add(padding);
        let clicked_column = usize::from(column);

        for (index, (tab, _, label)) in prompt_overlay_tab_labels(active_tab)
            .into_iter()
            .enumerate()
        {
            if index > 0 {
                current_column = current_column.saturating_add(1);
            }
            let label_end = current_column.saturating_add(display_width(&label));
            if clicked_column >= current_column && clicked_column < label_end {
                return Some(tab);
            }
            current_column = label_end;
        }

        None
    }
}

fn prompt_overlay_tab_labels(
    active_tab: PromptOverlayInactiveTab,
) -> Vec<(PromptOverlayInactiveTab, bool, String)> {
    PromptOverlayInactiveTab::ALL
        .iter()
        .copied()
        .map(|tab| {
            let is_active = tab == active_tab;
            let label = if is_active {
                format!("[{}]", tab.label())
            } else {
                tab.label().to_string()
            };
            (tab, is_active, label)
        })
        .collect()
}
