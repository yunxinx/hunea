use super::{
    shared::{EntryTreeWidget, branch_message_count_label, branch_picker_relative_age_label},
    *,
};

impl Model {
    pub(in crate::entry_tree::render) fn render_entry_tree_branch_picker(
        &self,
        frame: &mut RenderFrame<'_>,
        area: Rect,
    ) {
        let Some(state) = self.entry_tree.as_ref() else {
            return;
        };
        let Some(picker) = state.branch_picker.as_ref() else {
            return;
        };
        if area.is_empty() {
            return;
        }

        let list_rows = self.entry_tree_branch_picker_visible_rows();
        let popup_area = entry_tree_branch_picker_area_for_state(state, area, list_rows);
        frame.render_widget(Clear, popup_area);

        let has_scrollbar = picker.items.len() > list_rows && popup_area.width > 0;
        let chrome_width = popup_area.width.saturating_sub(u16::from(has_scrollbar));
        let content_width = chrome_width.saturating_sub(BRANCH_PICKER_RIGHT_PADDING);
        if chrome_width > 0 {
            let chrome_area =
                Rect::new(popup_area.x, popup_area.y, chrome_width, popup_area.height);
            let lines = self.entry_tree_branch_picker_lines(
                picker,
                usize::from(content_width),
                usize::from(chrome_width),
            );
            frame.render_widget(EntryTreeWidget { lines: &lines }, chrome_area);
        }

        if chrome_width > 0
            && let Some(selected_visible_row) = picker
                .selected
                .checked_sub(picker.scroll)
                .filter(|selected_visible_row| *selected_visible_row < list_rows)
        {
            let selected_y = popup_area
                .y
                .saturating_add(BRANCH_PICKER_ITEM_TOP_OFFSET)
                .saturating_add(u16::try_from(selected_visible_row).unwrap_or(u16::MAX));
            if selected_y < popup_area.y.saturating_add(popup_area.height) {
                frame.buffer_mut().set_style(
                    Rect::new(popup_area.x, selected_y, chrome_width, 1),
                    self.entry_tree_selected_row_style(),
                );
            }
        }

        if has_scrollbar {
            let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .begin_symbol(None)
                .end_symbol(None)
                .track_symbol(Some("┃"))
                .thumb_symbol("█")
                .thumb_style(secondary_text_style(self.palette))
                .track_style(tertiary_text_style(self.palette));
            let mut scrollbar_state = ScrollbarState::new(picker.items.len())
                .position(picker.scroll)
                .viewport_content_length(list_rows);
            scrollbar.render(popup_area, frame.buffer_mut(), &mut scrollbar_state);
        }
    }

    fn entry_tree_branch_picker_lines(
        &self,
        picker: &EntryTreeBranchPickerState,
        content_width: usize,
        chrome_width: usize,
    ) -> Vec<Line<'static>> {
        let content_width = content_width.max(1);
        let chrome_width = chrome_width.max(1);
        let list_rows = self.entry_tree_branch_picker_visible_rows();
        let mut lines =
            Vec::with_capacity(list_rows.saturating_add(usize::from(BRANCH_PICKER_CHROME_HEIGHT)));
        lines.push(entry_tree_branch_picker_title_rule_line(
            "Switch branch",
            chrome_width,
            self.palette,
        ));
        lines.push(entry_tree_branch_picker_header_line(
            content_width,
            self.palette,
        ));

        for row in 0..list_rows {
            let index = picker.scroll + row;
            let Some(item) = picker.items.get(index) else {
                lines.push(Line::raw(""));
                continue;
            };
            lines.push(self.entry_tree_branch_picker_item_line(
                item,
                index == picker.selected,
                picker.metadata_now_ms,
                content_width,
            ));
        }

        let footer = picker
            .error
            .as_deref()
            .map(str::to_string)
            .unwrap_or_else(|| {
                if picker
                    .selected_item()
                    .is_some_and(|selected_item| selected_item.branch.is_current)
                {
                    "Enter switch · Esc back".to_string()
                } else {
                    "Enter switch · Space preview branch · Esc back".to_string()
                }
            });
        let footer_style = if picker.error.is_some() {
            system_error_text_style(self.palette)
        } else {
            tertiary_text_style(self.palette).add_modifier(Modifier::ITALIC)
        };
        lines.push(entry_tree_branch_picker_footer_rule_line(
            &footer,
            footer_style,
            chrome_width,
            accent_text_style(self.palette),
        ));
        lines
    }

    fn entry_tree_branch_picker_item_line(
        &self,
        item: &SessionTreeBranchChoice,
        is_selected: bool,
        now_ms: i64,
        width: usize,
    ) -> Line<'static> {
        let text = branch_picker_item_text(item, now_ms, width);
        let style = if is_selected {
            self.entry_tree_selected_row_style()
        } else if item.branch.is_current {
            command_accent_text_style(self.palette)
        } else {
            secondary_text_style(self.palette)
        };
        Line::styled(truncate_display_width_with_ellipsis(&text, width), style)
    }
}

fn entry_tree_branch_picker_title_rule_line(
    label: &str,
    width: usize,
    palette: crate::theme::TerminalPalette,
) -> Line<'static> {
    let width = width.max(1);
    let rule_style = accent_text_style(palette).bold();
    let prefix = format!("─ {label} ");
    let prefix_width = display_width(&prefix);
    if prefix_width >= width {
        return Line::styled(
            truncate_display_width_with_ellipsis(&prefix, width),
            rule_style,
        );
    }

    Line::styled(
        format!("{prefix}{}", "─".repeat(width - prefix_width)),
        rule_style,
    )
}

fn entry_tree_branch_picker_header_line(
    width: usize,
    palette: crate::theme::TerminalPalette,
) -> Line<'static> {
    let text = format!(
        "{:padding$}{:<msgs_width$} {:<time_width$} {:<time_width$}",
        "",
        "Msgs",
        "Created",
        "Updated",
        padding = BRANCH_PICKER_METADATA_LEFT_PADDING,
        msgs_width = BRANCH_PICKER_MSGS_WIDTH,
        time_width = BRANCH_PICKER_TIME_WIDTH,
    );

    Line::styled(
        truncate_display_width_with_ellipsis(&text, width.max(1)),
        table_header_text_style(palette),
    )
}

fn entry_tree_branch_picker_footer_rule_line(
    footer: &str,
    footer_style: Style,
    width: usize,
    rule_style: Style,
) -> Line<'static> {
    let width = width.max(1);
    if width <= 3 {
        return Line::styled("─".repeat(width), rule_style);
    }

    let footer = truncate_display_width_with_ellipsis(footer, width.saturating_sub(4));
    let footer_width = display_width(&footer);
    let trailing_rule_width = width
        .saturating_sub(2)
        .saturating_sub(footer_width)
        .saturating_sub(1);

    Line::from(vec![
        Span::styled("─", rule_style),
        Span::styled(" ", footer_style),
        Span::styled(footer, footer_style),
        Span::raw(" "),
        Span::styled("─".repeat(trailing_rule_width), rule_style),
    ])
}

fn branch_picker_item_text(item: &SessionTreeBranchChoice, now_ms: i64, width: usize) -> String {
    let width = width.max(1);
    let branch = &item.branch;
    let message_count = branch_message_count_label(branch.message_count);
    let created = branch_picker_relative_age_label(now_ms, branch.branch_created_at_ms);
    let updated = branch_picker_relative_age_label(now_ms, branch.latest_updated_at_ms);
    let metadata_prefix = format!(
        "{:padding$}{message_count:<msgs_width$} {created:<time_width$} {updated:<time_width$} ",
        "",
        padding = BRANCH_PICKER_METADATA_LEFT_PADDING,
        msgs_width = BRANCH_PICKER_MSGS_WIDTH,
        time_width = BRANCH_PICKER_TIME_WIDTH,
    );
    let branch_content = if branch.is_current {
        format!(
            "{metadata_prefix}{:<ENTRY_TREE_KIND_WIDTH$} {}",
            "(current)", branch.display_summary
        )
    } else {
        format!("{metadata_prefix}{}", branch.display_summary)
    };

    truncate_display_width_with_ellipsis(&branch_content, width)
}
