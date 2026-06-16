use super::*;
use crate::theme::build_page_rule;

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
        frame.render_widget(Clear, area);
        if area.is_empty() || area.height < ENTRY_TREE_CHROME_HEIGHT {
            return;
        }

        let body_height = area.height.saturating_sub(ENTRY_TREE_CHROME_HEIGHT);
        let page_size = entry_tree_page_size_for_height(area.height);
        let header_area = Rect::new(area.x, area.y, area.width, ENTRY_TREE_HEADER_HEIGHT);
        let header_rule_area = Rect::new(
            area.x,
            area.y + ENTRY_TREE_HEADER_HEIGHT,
            area.width,
            ENTRY_TREE_HEADER_RULE_HEIGHT,
        );
        let body_area = Rect::new(
            area.x,
            area.y + ENTRY_TREE_HEADER_HEIGHT + ENTRY_TREE_HEADER_RULE_HEIGHT,
            area.width,
            body_height,
        );
        let page_rule_area = Rect::new(
            area.x,
            area.y
                + area
                    .height
                    .saturating_sub(ENTRY_TREE_PAGE_RULE_HEIGHT + ENTRY_TREE_FOOTER_HEIGHT),
            area.width,
            ENTRY_TREE_PAGE_RULE_HEIGHT,
        );
        let footer_area = Rect::new(
            area.x,
            area.y + area.height.saturating_sub(ENTRY_TREE_FOOTER_HEIGHT),
            area.width,
            ENTRY_TREE_FOOTER_HEIGHT,
        );

        frame.render_widget(
            Paragraph::new(self.entry_tree_header_line(state, usize::from(area.width))),
            header_area,
        );
        frame.render_widget(
            Paragraph::new(subtle_rule_line(usize::from(area.width), self.palette)),
            header_rule_area,
        );

        let lines =
            self.entry_tree_body_lines(state, usize::from(area.width), usize::from(body_height));
        frame.render_widget(EntryTreeWidget { lines: &lines }, body_area);

        frame.render_widget(
            Paragraph::new(build_page_rule(
                area.width,
                state.page_number(page_size),
                state.page_count(page_size),
                self.palette,
            )),
            page_rule_area,
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
            footer_area,
        );

        if state.branch_picker.is_some() {
            self.render_entry_tree_branch_picker(frame, area);
        }
    }

    pub(super) fn entry_tree_page_size(&self) -> usize {
        entry_tree_page_size_for_height(self.height)
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

    fn entry_tree_body_lines(
        &self,
        state: &EntryTreeState,
        width: usize,
        body_height: usize,
    ) -> Vec<Line<'static>> {
        let width = width.max(1);
        let page_size = entry_tree_page_size_for_height(
            u16::try_from(body_height)
                .unwrap_or(u16::MAX)
                .saturating_add(ENTRY_TREE_CHROME_HEIGHT),
        );
        let mut lines = Vec::new();

        if state.is_loading {
            lines.push(Line::styled(
                "  Loading session tree...",
                tertiary_text_style(self.palette),
            ));
        } else if let Some(error) = state.error.as_deref() {
            lines.push(Line::styled(
                truncate_display_width_with_ellipsis(&format!("  {error}"), width),
                tertiary_text_style(self.palette),
            ));
        } else if state.rows.is_empty() {
            lines.push(Line::styled(
                "  No messages yet",
                tertiary_text_style(self.palette),
            ));
        } else {
            let graph_lines = entry_tree_graph_lines(&state.rows, state.selected, width);
            let page_start = state.page_start(page_size);
            for (visible_position, row_index) in state.page_indices(page_size).enumerate() {
                let row = &state.rows[row_index];
                let absolute_position = page_start + visible_position;
                let graph_line = graph_lines.get(row_index).cloned().unwrap_or_default();
                lines.push(self.entry_tree_row_line(
                    row,
                    width,
                    graph_line,
                    absolute_position == state.selected,
                ));
            }
        }

        lines.truncate(body_height);
        lines
    }

    fn entry_tree_row_line(
        &self,
        row: &SessionTreeRow,
        width: usize,
        graph_line: EntryTreeGraphLine,
        is_selected: bool,
    ) -> Line<'static> {
        let kind_prefix = entry_tree_kind_prefix(row.kind);
        let left_padding = " ".repeat(ENTRY_TREE_BODY_HORIZONTAL_PADDING);
        let prefix_width =
            display_width(&left_padding) + graph_line.display_width() + display_width(&kind_prefix);
        let text_width = width
            .saturating_sub(prefix_width)
            .saturating_sub(ENTRY_TREE_BODY_HORIZONTAL_PADDING);
        let text_style = entry_tree_content_style(row, self.palette, is_selected);
        let selected_text_style = if is_selected {
            text_style.add_modifier(Modifier::REVERSED)
        } else {
            text_style
        };
        let kind_style = entry_tree_kind_style(row, self.palette, is_selected);

        let mut spans = Vec::new();
        spans.push(Span::raw(left_padding));
        spans.extend(graph_line.spans.into_iter().map(|span| {
            let style = entry_tree_graph_span_style(&span.text, self.palette);
            Span::styled(span.text, style)
        }));
        spans.extend([
            Span::styled(kind_prefix, kind_style),
            Span::styled(
                truncate_display_width_with_ellipsis(&row.summary, text_width),
                selected_text_style,
            ),
        ]);

        Line::from(spans)
    }

    fn render_entry_tree_branch_tree(&self, frame: &mut RenderFrame<'_>, area: Rect) {
        let Some(branch_tree) = self
            .entry_tree
            .as_ref()
            .and_then(|state| state.branch_tree.as_ref())
        else {
            return;
        };
        frame.render_widget(Clear, area);
        if area.is_empty() || area.height < ENTRY_TREE_CHROME_HEIGHT {
            return;
        }

        let body_height = area.height.saturating_sub(ENTRY_TREE_CHROME_HEIGHT);
        let page_size = entry_tree_branch_tree_page_size_for_height(area.height);
        let header_area = Rect::new(area.x, area.y, area.width, ENTRY_TREE_HEADER_HEIGHT);
        let header_rule_area = Rect::new(
            area.x,
            area.y + ENTRY_TREE_HEADER_HEIGHT,
            area.width,
            ENTRY_TREE_HEADER_RULE_HEIGHT,
        );
        let body_area = Rect::new(
            area.x,
            area.y + ENTRY_TREE_HEADER_HEIGHT + ENTRY_TREE_HEADER_RULE_HEIGHT,
            area.width,
            body_height,
        );
        let page_rule_area = Rect::new(
            area.x,
            area.y
                + area
                    .height
                    .saturating_sub(ENTRY_TREE_PAGE_RULE_HEIGHT + ENTRY_TREE_FOOTER_HEIGHT),
            area.width,
            ENTRY_TREE_PAGE_RULE_HEIGHT,
        );
        let footer_area = Rect::new(
            area.x,
            area.y + area.height.saturating_sub(ENTRY_TREE_FOOTER_HEIGHT),
            area.width,
            ENTRY_TREE_FOOTER_HEIGHT,
        );

        frame.render_widget(
            Paragraph::new(
                self.entry_tree_branch_tree_header_line(branch_tree, usize::from(area.width)),
            ),
            header_area,
        );
        frame.render_widget(
            Paragraph::new(subtle_rule_line(usize::from(area.width), self.palette)),
            header_rule_area,
        );

        let lines = self.entry_tree_branch_tree_body_lines(
            branch_tree,
            usize::from(area.width),
            usize::from(body_height),
            page_size,
        );
        frame.render_widget(EntryTreeWidget { lines: &lines }, body_area);
        if let Some(selected_visible_row) = branch_tree.selected_visible_row(page_size) {
            let selected_y = body_area
                .y
                .saturating_add(1)
                .saturating_add(u16::try_from(selected_visible_row).unwrap_or(u16::MAX));
            if selected_y < body_area.bottom() {
                frame.buffer_mut().set_style(
                    Rect::new(body_area.x, selected_y, body_area.width, 1),
                    self.entry_tree_branch_picker_selected_style(),
                );
            }
        }

        frame.render_widget(
            Paragraph::new(build_page_rule(
                area.width,
                branch_tree.page_number(page_size),
                branch_tree.page_count(page_size),
                self.palette,
            )),
            page_rule_area,
        );
        frame.render_widget(
            Paragraph::new(Line::styled(
                entry_tree_branch_tree_footer_hint(
                    area.width,
                    branch_tree
                        .selected_node()
                        .is_some_and(|node| node.branch.is_current),
                ),
                tertiary_text_style(self.palette).add_modifier(Modifier::ITALIC),
            )),
            footer_area,
        );
    }

    fn entry_tree_branch_tree_header_line(
        &self,
        state: &EntryTreeBranchTreeState,
        width: usize,
    ) -> Line<'static> {
        let title = format!(
            "Branch Tree ({} of {})",
            state.selected_position_label(),
            state.nodes.len(),
        );
        Line::from(vec![
            Span::raw("  "),
            Span::styled(
                truncate_display_width_with_ellipsis(&title, width.saturating_sub(2).max(1)),
                primary_text_style(self.palette).bold(),
            ),
        ])
    }

    fn entry_tree_branch_tree_body_lines(
        &self,
        state: &EntryTreeBranchTreeState,
        width: usize,
        body_height: usize,
        page_size: usize,
    ) -> Vec<Line<'static>> {
        let width = width.max(1);
        let mut lines = Vec::new();
        if state.is_loading {
            lines.push(Line::styled(
                "  Loading branch tree...",
                tertiary_text_style(self.palette),
            ));
        } else if let Some(error) = state.error.as_deref() {
            lines.push(Line::styled(
                truncate_display_width_with_ellipsis(&format!("  {error}"), width),
                tertiary_text_style(self.palette),
            ));
        } else {
            lines.push(Line::from(vec![
                Span::raw(" ".repeat(ENTRY_TREE_BODY_HORIZONTAL_PADDING)),
                Span::styled(".", tertiary_text_style(self.palette)),
            ]));
            if state.nodes.is_empty() {
                lines.push(Line::styled(
                    "  No branches yet",
                    tertiary_text_style(self.palette),
                ));
            } else {
                let prefixes = branch_tree_connector_prefixes(&state.nodes);
                let page_start = state.page_start(page_size);
                for (visible_position, node_index) in state.page_indices(page_size).enumerate() {
                    let node = &state.nodes[node_index];
                    let prefix = prefixes.get(node_index).map(String::as_str).unwrap_or("");
                    lines.push(self.entry_tree_branch_tree_node_line(
                        node,
                        prefix,
                        width,
                        page_start + visible_position == state.selected,
                    ));
                }
            }
            lines.push(Line::raw(""));
            lines.push(Line::styled(
                branch_tree_summary_label(state.nodes.len(), state.total_message_count, width),
                tertiary_text_style(self.palette),
            ));
        }

        lines.truncate(body_height);
        lines
    }

    fn entry_tree_branch_tree_node_line(
        &self,
        node: &SessionBranchTreeNode,
        connector_prefix: &str,
        width: usize,
        is_selected: bool,
    ) -> Line<'static> {
        let left_padding = " ".repeat(ENTRY_TREE_BODY_HORIZONTAL_PADDING);
        let prefix_width = display_width(&left_padding) + display_width(connector_prefix);
        let text_width = width
            .saturating_sub(prefix_width)
            .saturating_sub(ENTRY_TREE_BODY_HORIZONTAL_PADDING);
        let node_text = branch_tree_node_text(node, text_width);
        let node_style = if node.branch.is_current {
            command_accent_text_style(self.palette)
        } else if is_selected {
            primary_text_style(self.palette).bold()
        } else {
            secondary_text_style(self.palette)
        };

        Line::from(vec![
            Span::raw(left_padding),
            Span::styled(
                connector_prefix.to_string(),
                tertiary_text_style(self.palette),
            ),
            Span::styled(node_text, node_style),
        ])
    }

    fn render_entry_tree_branch_picker(&self, frame: &mut RenderFrame<'_>, area: Rect) {
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
                    self.entry_tree_branch_picker_selected_style(),
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
            self.entry_tree_branch_picker_selected_style()
        } else if item.branch.is_current {
            command_accent_text_style(self.palette)
        } else {
            secondary_text_style(self.palette)
        };
        Line::styled(truncate_display_width_with_ellipsis(&text, width), style)
    }

    fn entry_tree_branch_picker_selected_style(&self) -> Style {
        primary_text_style(self.palette)
            .bold()
            .add_modifier(Modifier::REVERSED)
    }

    fn render_entry_tree_branch_preview(&self, frame: &mut RenderFrame<'_>, area: Rect) {
        let Some(preview) = self
            .entry_tree
            .as_ref()
            .and_then(|state| state.branch_preview.as_ref())
        else {
            return;
        };
        frame.render_widget(Clear, area);
        if area.is_empty() || area.height < ENTRY_TREE_CHROME_HEIGHT {
            return;
        }

        let body_height = area.height.saturating_sub(ENTRY_TREE_CHROME_HEIGHT);
        let page_size = entry_tree_page_size_for_height(area.height);
        let header_area = Rect::new(area.x, area.y, area.width, ENTRY_TREE_HEADER_HEIGHT);
        let header_rule_area = Rect::new(
            area.x,
            area.y + ENTRY_TREE_HEADER_HEIGHT,
            area.width,
            ENTRY_TREE_HEADER_RULE_HEIGHT,
        );
        let body_area = Rect::new(
            area.x,
            area.y + ENTRY_TREE_HEADER_HEIGHT + ENTRY_TREE_HEADER_RULE_HEIGHT,
            area.width,
            body_height,
        );
        let page_rule_area = Rect::new(
            area.x,
            area.y
                + area
                    .height
                    .saturating_sub(ENTRY_TREE_PAGE_RULE_HEIGHT + ENTRY_TREE_FOOTER_HEIGHT),
            area.width,
            ENTRY_TREE_PAGE_RULE_HEIGHT,
        );
        let footer_area = Rect::new(
            area.x,
            area.y + area.height.saturating_sub(ENTRY_TREE_FOOTER_HEIGHT),
            area.width,
            ENTRY_TREE_FOOTER_HEIGHT,
        );

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
            header_area,
        );
        frame.render_widget(
            Paragraph::new(subtle_rule_line(usize::from(area.width), self.palette)),
            header_rule_area,
        );

        let state = EntryTreeState {
            rows: preview.rows.clone(),
            selected: preview.selected,
            is_loading: preview.is_loading,
            error: preview.error.clone(),
            preview: None,
            branch_picker: None,
            branch_tree: None,
            branch_preview: None,
        };
        let lines =
            self.entry_tree_body_lines(&state, usize::from(area.width), usize::from(body_height));
        frame.render_widget(EntryTreeWidget { lines: &lines }, body_area);

        frame.render_widget(
            Paragraph::new(build_page_rule(
                area.width,
                state.page_number(page_size),
                state.page_count(page_size),
                self.palette,
            )),
            page_rule_area,
        );
        frame.render_widget(
            Paragraph::new(Line::styled(
                self.entry_tree_branch_preview_footer_hint(),
                tertiary_text_style(self.palette).add_modifier(Modifier::ITALIC),
            )),
            footer_area,
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

    fn render_entry_tree_preview(&mut self, frame: &mut RenderFrame<'_>, area: Rect) {
        let palette = self.palette;
        let content_height = area.height.saturating_sub(2).max(1) as usize;
        let Some(preview) = self.entry_tree_message_preview_mut() else {
            return;
        };
        if preview.is_following_bottom {
            preview.overlay.scroll_offset =
                latest_entry_tree_preview_offset(&mut preview.transcript, content_height);
        }
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
        Span::styled("─".to_string(), rule_style),
        Span::styled(" ".to_string(), footer_style),
        Span::styled(footer, footer_style),
        Span::raw(" "),
        Span::styled("─".repeat(trailing_rule_width), rule_style),
    ])
}

fn branch_picker_message_count_label(message_count: usize) -> String {
    if message_count > 999 {
        "999+".to_string()
    } else {
        message_count.to_string()
    }
}

fn branch_message_count_text(message_count: usize) -> String {
    format!("{} msgs", branch_picker_message_count_label(message_count))
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

    let created =
        branch_picker_relative_age_label(metadata.metadata_now_ms, metadata.branch_created_at_ms);
    let updated =
        branch_picker_relative_age_label(metadata.metadata_now_ms, metadata.latest_updated_at_ms);
    format!("Branch Preview ({position} · Created {created} · Updated {updated})")
}

fn branch_picker_item_text(item: &SessionTreeBranchChoice, now_ms: i64, width: usize) -> String {
    let width = width.max(1);
    let branch = &item.branch;
    let message_count = branch_picker_message_count_label(branch.message_count);
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

fn branch_tree_node_text(node: &SessionBranchTreeNode, width: usize) -> String {
    let width = width.max(1);
    let branch = &node.branch;
    let message_count = branch_message_count_text(branch.message_count);
    let branch_content = if branch.is_current {
        format!("{message_count} (current) {}", branch.display_summary)
    } else {
        format!("{message_count} {}", branch.display_summary)
    };

    truncate_display_width_with_ellipsis(&branch_content, width)
}

fn branch_tree_summary_label(
    branch_count: usize,
    total_message_count: usize,
    width: usize,
) -> String {
    truncate_display_width_with_ellipsis(
        &format!("  {branch_count} branches, {total_message_count} messages"),
        width.max(1),
    )
}

fn entry_tree_branch_tree_footer_hint(width: u16, is_current_branch: bool) -> &'static str {
    match (width < 90, is_current_branch) {
        (true, true) => "  Esc back · Enter switch · j/k · h/l page",
        (true, false) => "  Esc back · Enter switch · Space preview branch · j/k · h/l page",
        (false, true) => "  Esc back · Enter switch · ↑/↓/j/k move · ←/→/h/l page",
        (false, false) => {
            "  Esc back · Enter switch · Space preview branch · ↑/↓/j/k move · ←/→/h/l page"
        }
    }
}

pub(super) fn branch_picker_relative_age_label(now_ms: i64, timestamp_ms: i64) -> String {
    const SECONDS_PER_MINUTE: i64 = 60;
    const MINUTES_PER_HOUR: i64 = 60;
    const HOURS_PER_DAY: i64 = 24;
    const DAYS_PER_MONTH: i64 = 30;
    const DAYS_PER_YEAR: i64 = 365;

    let elapsed_seconds = now_ms.saturating_sub(timestamp_ms).max(0) / 1_000;
    if elapsed_seconds < SECONDS_PER_MINUTE {
        return format!("{elapsed_seconds}s");
    }

    let elapsed_minutes = elapsed_seconds / SECONDS_PER_MINUTE;
    if elapsed_minutes < MINUTES_PER_HOUR {
        return format!(
            "{}m·{}s",
            elapsed_minutes,
            elapsed_seconds % SECONDS_PER_MINUTE
        );
    }

    let elapsed_hours = elapsed_minutes / MINUTES_PER_HOUR;
    if elapsed_hours < HOURS_PER_DAY {
        return format!("{}h·{}m", elapsed_hours, elapsed_minutes % MINUTES_PER_HOUR);
    }

    let elapsed_days = elapsed_hours / HOURS_PER_DAY;
    if elapsed_days < DAYS_PER_MONTH {
        return format!("{}d·{}m", elapsed_days, elapsed_minutes % MINUTES_PER_HOUR);
    }
    if elapsed_days < DAYS_PER_YEAR {
        return format!(
            "{}mo·{}d",
            elapsed_days / DAYS_PER_MONTH,
            elapsed_days % DAYS_PER_MONTH
        );
    }

    format!(
        "{}y·{}mo",
        elapsed_days / DAYS_PER_YEAR,
        (elapsed_days % DAYS_PER_YEAR) / DAYS_PER_MONTH
    )
}

struct EntryTreeWidget<'a> {
    lines: &'a [Line<'static>],
}

impl Widget for EntryTreeWidget<'_> {
    fn render(self, area: Rect, buf: &mut ratatui::buffer::Buffer) {
        for (row, line) in self.lines.iter().take(usize::from(area.height)).enumerate() {
            let y = area.y + u16::try_from(row).unwrap_or(u16::MAX);
            let row_area = Rect::new(area.x, y, area.width, 1);
            if line.style != Style::new() {
                buf.set_style(row_area, line.style);
            }
            buf.set_line(area.x, y, line, area.width);
        }
    }
}

fn entry_tree_kind_label(kind: SessionTreeRowKind) -> &'static str {
    match kind {
        SessionTreeRowKind::User => "user",
        SessionTreeRowKind::Assistant => "assistant",
        SessionTreeRowKind::Tool => "tool",
        SessionTreeRowKind::Reasoning => "reasoning",
    }
}

fn entry_tree_kind_prefix(kind: SessionTreeRowKind) -> String {
    let label = entry_tree_kind_label(kind);
    match kind {
        SessionTreeRowKind::Tool => format!("{label:^ENTRY_TREE_KIND_WIDTH$} "),
        SessionTreeRowKind::User
        | SessionTreeRowKind::Assistant
        | SessionTreeRowKind::Reasoning => {
            format!("{label:<ENTRY_TREE_KIND_WIDTH$} ")
        }
    }
}

fn entry_tree_content_style(
    row: &SessionTreeRow,
    palette: crate::theme::TerminalPalette,
    is_selected: bool,
) -> Style {
    match row.kind {
        SessionTreeRowKind::User => command_accent_text_style(palette),
        SessionTreeRowKind::Reasoning => tertiary_text_style(palette).italic(),
        SessionTreeRowKind::Tool => muted_text_style(palette),
        SessionTreeRowKind::Assistant if is_selected => primary_text_style(palette).bold(),
        SessionTreeRowKind::Assistant if row.is_active_path => primary_text_style(palette),
        SessionTreeRowKind::Assistant => secondary_text_style(palette),
    }
}

fn entry_tree_kind_style(
    row: &SessionTreeRow,
    palette: crate::theme::TerminalPalette,
    is_selected: bool,
) -> Style {
    let content_style = entry_tree_content_style(row, palette, is_selected);
    match content_style.fg {
        Some(color) => Style::new().fg(color),
        None => Style::new(),
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

fn entry_tree_preview_footer_hint(width: u16) -> &'static str {
    if width < 76 {
        "  Esc/Space back · ↑/←/h prev · ↓/→/l next"
    } else {
        "  Esc/Space back · ↑/←/h previous page · ↓/→/l next page"
    }
}
