use super::render_rows::*;
use super::*;

impl Model {
    pub(crate) fn render_prompt_overlay(&mut self, frame: &mut RenderFrame<'_>, area: Rect) {
        let Some(state) = self.prompt_overlay.as_ref() else {
            return;
        };
        if state.preview.is_some() {
            self.render_prompt_overlay_preview(frame, area);
            return;
        }

        frame.render_widget(Clear, area);
        let Some(layout) = prompt_overlay_layout_rects(area) else {
            return;
        };

        frame.render_widget(
            Paragraph::new(
                self.prompt_overlay_header_line(usize::from(area.width), state.inactive_tab),
            ),
            layout.chrome.header,
        );
        frame.render_widget(
            Paragraph::new(subtle_rule_line(usize::from(area.width), self.palette)),
            layout.chrome.header_rule,
        );
        let gutter_x = layout.left_pane.x.saturating_add(layout.left_pane.width);
        let gutter = Rect::new(gutter_x, layout.left_pane.y, 1, layout.left_pane.height);

        if gutter.width > 0 {
            frame.render_widget(
                PromptOverlayVerticalRule {
                    palette: self.palette,
                },
                gutter,
            );
        }

        self.render_prompt_overlay_active_pane(frame, layout.left_pane, state);
        self.render_prompt_overlay_inactive_pane(frame, layout.right_pane, state);

        let focused_page = self.prompt_overlay_focused_page_label(state, area.height);
        frame.render_widget(
            Paragraph::new(build_labeled_rule(area.width, focused_page, self.palette)),
            layout.chrome.page_rule,
        );
        frame.render_widget(
            Paragraph::new(self.prompt_overlay_footer_line(area.width)),
            layout.chrome.footer,
        );
        if state.shortcut_help_open {
            let shortcut_help_lines = self.prompt_overlay_shortcut_help_lines();
            let popover = ShortcutHelpPopover {
                title: Some("More"),
                lines: &shortcut_help_lines,
            };
            popover.render(frame, layout.chrome.body, self.palette);
        }

        if let Some(dialog) = state.dialog.as_ref() {
            self.render_prompt_overlay_dialog(frame, layout.right_pane, dialog);
        }
    }

    pub(super) fn prompt_overlay_header_line(
        &self,
        width: usize,
        active_tab: PromptOverlayInactiveTab,
    ) -> Line<'static> {
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

        let mut spans = vec![
            Span::raw(" ".repeat(PROMPT_OVERLAY_HEADER_INSET)),
            Span::styled(title, primary_text_style(self.palette).bold()),
            Span::raw(" ".repeat(padding)),
        ];
        spans.extend(self.prompt_overlay_tabs_spans(active_tab));
        spans.push(Span::raw(
            " ".repeat(PROMPT_OVERLAY_HEADER_TRAILING_PADDING),
        ));

        Line::from(spans)
    }

    pub(super) fn prompt_overlay_footer_line(&self, width: u16) -> Line<'static> {
        let actions = self.prompt_overlay_action_availability();
        let mut parts = if width < 120 {
            Vec::new()
        } else {
            vec!["p assembled"]
        };
        let show_shadowed_toggle = matches!(
            self.selected_prompt_overlay_left_row(),
            Some(PromptOverlayLeftRow::ManagedSource { shadowed_count, .. }) if shadowed_count > 0
        ) || matches!(
            self.selected_prompt_overlay_left_row(),
            Some(PromptOverlayLeftRow::ShadowedDetail { .. })
        ) || matches!(
            self.selected_prompt_overlay_inactive_row(),
            Some(PromptOverlayInactiveRow::ExtraPromptCandidate { shadowed_count, .. })
                if shadowed_count > 0
        ) || matches!(
            self.selected_prompt_overlay_inactive_row(),
            Some(PromptOverlayInactiveRow::DiscoveredSkill { shadowed_count, .. })
                if shadowed_count > 0
        ) || matches!(
            self.selected_prompt_overlay_inactive_row(),
            Some(
                PromptOverlayInactiveRow::ExtraPromptShadowedDetail { .. }
                    | PromptOverlayInactiveRow::DiscoveredSkillShadowedDetail { .. }
            )
        );
        let selected_core = self
            .selected_prompt_overlay_source()
            .is_some_and(|source| source.kind == PromptSourceKind::CoreSystemPrompt);
        let selected_discovered_skill = matches!(
            self.selected_prompt_overlay_selection(),
            Some(PromptOverlaySelection::DiscoveredSkill(_))
        );
        let selected_previewable = self.selected_prompt_overlay_selection().is_some();
        if actions.can_edit {
            parts.push("e/ctrl+g edit");
        }
        if actions.can_add_custom {
            parts.push("a create prompt");
        }
        if actions.can_remove {
            parts.push("d remove");
        }
        if actions.can_toggle_selection {
            parts.push("x disable");
        }
        if selected_core {
            parts.push("r restore");
        }
        if selected_discovered_skill {
            parts.push("r reset order");
        }
        if actions.can_reorder_active && width >= 120 {
            parts.push("J/K reorder");
        }
        if selected_previewable {
            parts.push("Space preview");
        }
        if show_shadowed_toggle {
            parts.push("ctrl+e shadowed");
        }
        let focus_right = self
            .prompt_overlay
            .as_ref()
            .is_some_and(|state| state.focus == PromptOverlayFocus::Inactive);
        if focus_right {
            parts.push("Tab tabs");
        }
        parts.push(PROMPT_OVERLAY_FOOTER_MORE_LABEL);
        let text = if parts.is_empty() {
            String::new()
        } else {
            format!("  {}", parts.join(" · "))
        };
        if parts.len() == 1 {
            return Line::styled(
                truncate_display_width_with_ellipsis(&text, usize::from(width).max(1)),
                command_accent_text_style(self.palette).add_modifier(Modifier::ITALIC),
            );
        }
        let more_separator = " · ? more";
        let separator_index = text.rfind(more_separator).unwrap_or(text.len());
        let prefix = text[..separator_index].to_string();
        Line::from(vec![
            Span::styled(
                truncate_display_width_with_ellipsis(&prefix, usize::from(width).max(1)),
                tertiary_text_style(self.palette).add_modifier(Modifier::ITALIC),
            ),
            Span::styled(
                truncate_display_width_with_ellipsis(more_separator, usize::from(width).max(1)),
                command_accent_text_style(self.palette).add_modifier(Modifier::ITALIC),
            ),
        ])
    }

    pub(super) fn render_prompt_overlay_active_pane(
        &self,
        frame: &mut RenderFrame<'_>,
        area: Rect,
        state: &PromptOverlayState,
    ) {
        if area.is_empty() {
            return;
        }
        let [header_area, body_area] =
            Layout::vertical([Constraint::Length(1), Constraint::Fill(1)]).areas(area);
        frame.render_widget(
            Paragraph::new(self.prompt_overlay_active_header_line(usize::from(header_area.width))),
            header_area,
        );

        let sources = self.prompt_overlay_left_rows();
        let lines = prompt_overlay_active_lines(
            &sources,
            state.active_selected,
            state.active_scroll,
            state.focus == PromptOverlayFocus::Active,
            usize::from(body_area.width),
            usize::from(body_area.height),
            self.palette,
        );
        frame.render_widget(PromptOverlayLineListWidget { lines: &lines }, body_area);
    }

    pub(super) fn render_prompt_overlay_inactive_pane(
        &self,
        frame: &mut RenderFrame<'_>,
        area: Rect,
        state: &PromptOverlayState,
    ) {
        if area.is_empty() {
            return;
        }
        let [header_area, body_area] =
            Layout::vertical([Constraint::Length(1), Constraint::Fill(1)]).areas(area);
        frame.render_widget(
            Paragraph::new(self.prompt_overlay_inactive_header_line(
                state.inactive_tab,
                usize::from(header_area.width),
            )),
            header_area,
        );

        let lines = match state.inactive_tab {
            PromptOverlayInactiveTab::LongLivedSkills => prompt_overlay_discovered_skill_lines(
                &self.prompt_overlay_inactive_rows(PromptOverlayInactiveTab::LongLivedSkills),
                state.inactive_selected_row_id.as_deref(),
                state.inactive_scroll,
                state.focus == PromptOverlayFocus::Inactive,
                usize::from(body_area.width),
                usize::from(body_area.height),
                self.palette,
            ),
            PromptOverlayInactiveTab::Tools => prompt_overlay_tool_lines(
                &self.prompt_overlay_inactive_rows(PromptOverlayInactiveTab::Tools),
                state.inactive_selected_row_id.as_deref(),
                state.inactive_scroll,
                state.focus == PromptOverlayFocus::Inactive,
                usize::from(body_area.width),
                usize::from(body_area.height),
                self.palette,
            ),
            PromptOverlayInactiveTab::Dynamic => prompt_overlay_dynamic_lines(
                &self.prompt_overlay_inactive_rows(PromptOverlayInactiveTab::Dynamic),
                PromptOverlayDynamicSelection {
                    row_id: state.inactive_selected_row_id.as_deref(),
                    snapshot_kind: state.dynamic_selected_snapshot_kind,
                },
                state.inactive_scroll,
                state.focus == PromptOverlayFocus::Inactive,
                usize::from(body_area.width),
                usize::from(body_area.height),
                self.palette,
            ),
            PromptOverlayInactiveTab::ExtraPrompts => prompt_overlay_inactive_lines(
                &self.prompt_overlay_inactive_rows(PromptOverlayInactiveTab::ExtraPrompts),
                state.inactive_selected_row_id.as_deref(),
                state.inactive_scroll,
                state.focus == PromptOverlayFocus::Inactive,
                usize::from(body_area.width),
                usize::from(body_area.height),
                self.palette,
            ),
        };
        frame.render_widget(PromptOverlayLineListWidget { lines: &lines }, body_area);
    }

    pub(super) fn prompt_overlay_active_header_line(&self, width: usize) -> Line<'static> {
        let content_width = width.saturating_sub(PROMPT_OVERLAY_ROW_PREFIX_WIDTH);
        let text = format!(
            "{}{}",
            " ".repeat(PROMPT_OVERLAY_ROW_PREFIX_WIDTH),
            truncate_display_width_with_ellipsis(
                &prompt_overlay_active_header_text(content_width),
                content_width.max(1),
            )
        );
        Line::styled(
            truncate_display_width_with_ellipsis(&text, width.max(1)),
            table_header_text_style(self.palette),
        )
    }

    pub(super) fn prompt_overlay_inactive_header_line(
        &self,
        active_tab: PromptOverlayInactiveTab,
        width: usize,
    ) -> Line<'static> {
        let content_width = width.saturating_sub(PROMPT_OVERLAY_ROW_PREFIX_WIDTH);
        let label = match active_tab {
            PromptOverlayInactiveTab::ExtraPrompts => {
                prompt_overlay_extra_header_text(content_width)
            }
            PromptOverlayInactiveTab::LongLivedSkills => {
                prompt_overlay_skill_header_text(content_width)
            }
            PromptOverlayInactiveTab::Tools => prompt_overlay_tool_header_text(content_width),
            PromptOverlayInactiveTab::Dynamic => prompt_overlay_dynamic_header_text(content_width),
        };
        let text = format!(
            "{}{}",
            " ".repeat(PROMPT_OVERLAY_ROW_PREFIX_WIDTH),
            truncate_display_width_with_ellipsis(&label, content_width.max(1))
        );
        Line::styled(
            truncate_display_width_with_ellipsis(&text, width.max(1)),
            table_header_text_style(self.palette),
        )
    }

    pub(super) fn prompt_overlay_tabs_plain(&self, active_tab: PromptOverlayInactiveTab) -> String {
        PromptOverlayInactiveTab::ALL
            .iter()
            .copied()
            .map(|tab| {
                if tab == active_tab {
                    format!("[{}]", tab.label())
                } else {
                    tab.label().to_string()
                }
            })
            .collect::<Vec<_>>()
            .join(" ")
    }

    pub(super) fn prompt_overlay_tabs_spans(
        &self,
        active_tab: PromptOverlayInactiveTab,
    ) -> Vec<Span<'static>> {
        let mut spans = Vec::new();

        for (index, tab) in PromptOverlayInactiveTab::ALL.iter().copied().enumerate() {
            if index > 0 {
                spans.push(Span::raw(" "));
            }
            let is_active = tab == active_tab;
            let label = if is_active {
                format!("[{}]", tab.label())
            } else {
                tab.label().to_string()
            };
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

    pub(super) fn prompt_overlay_shortcut_help_open(&self) -> bool {
        self.prompt_overlay
            .as_ref()
            .is_some_and(|state| state.shortcut_help_open)
    }

    pub(super) fn toggle_prompt_overlay_shortcut_help(&mut self) {
        let Some(state) = self.prompt_overlay.as_mut() else {
            return;
        };
        state.shortcut_help_open = !state.shortcut_help_open;
    }

    pub(super) fn close_prompt_overlay_shortcut_help(&mut self) {
        let Some(state) = self.prompt_overlay.as_mut() else {
            return;
        };
        state.shortcut_help_open = false;
    }

    pub(super) fn prompt_overlay_shortcut_help_area(&self, bounds: Rect) -> Rect {
        let lines = self.prompt_overlay_shortcut_help_lines();
        ShortcutHelpPopover {
            title: Some("More"),
            lines: &lines,
        }
        .area(bounds)
    }

    pub(super) fn prompt_overlay_shortcut_help_lines(&self) -> Vec<Line<'static>> {
        let key_style = command_accent_text_style(self.palette).add_modifier(Modifier::ITALIC);
        let text_style = tertiary_text_style(self.palette).add_modifier(Modifier::ITALIC);
        let mut lines = aligned_shortcut_help_lines(
            &[
                ShortcutHelpEntry {
                    shortcut: "Esc",
                    description: "close",
                },
                ShortcutHelpEntry {
                    shortcut: "←/→/h/l",
                    description: "focus panes",
                },
                ShortcutHelpEntry {
                    shortcut: "↑/↓/j/k",
                    description: "move",
                },
                ShortcutHelpEntry {
                    shortcut: "Space",
                    description: "preview",
                },
            ],
            key_style,
            text_style,
        );
        lines.push(Line::raw(""));
        lines.extend(aligned_shortcut_help_lines(
            &[ShortcutHelpEntry {
                shortcut: "? / Esc",
                description: "close help",
            }],
            key_style,
            text_style,
        ));
        lines
    }

    pub(super) fn prompt_overlay_focused_page_label(
        &self,
        state: &PromptOverlayState,
        _height: u16,
    ) -> String {
        match state.focus {
            PromptOverlayFocus::Active => selection_label(
                Some("Active"),
                state.active_selected,
                self.prompt_overlay_left_rows().len(),
            ),
            PromptOverlayFocus::Inactive => selection_label(
                None,
                state.inactive_selected,
                self.prompt_overlay_inactive_source_count(state.inactive_tab),
            ),
        }
    }

    pub(crate) fn prompt_overlay_inactive_source_count(
        &self,
        tab: PromptOverlayInactiveTab,
    ) -> usize {
        self.prompt_overlay_inactive_rows(tab).len()
    }

    pub(super) fn render_prompt_overlay_dialog(
        &self,
        frame: &mut RenderFrame<'_>,
        anchor_area: Rect,
        dialog: &PromptOverlayDialog,
    ) {
        let dialog_area = prompt_overlay_dialog_area(anchor_area);
        frame.render_widget(Clear, dialog_area);

        let lines = match dialog {
            PromptOverlayDialog::CreateExtraPromptScope { selected_scope } => vec![
                Line::styled(
                    "Create custom prompt in",
                    primary_text_style(self.palette).bold(),
                ),
                Line::raw(""),
                prompt_overlay_scope_picker_line(*selected_scope, self.palette),
                Line::raw(""),
                Line::styled(
                    "←/→/h/l select · Enter confirm · Esc cancel",
                    tertiary_text_style(self.palette),
                ),
            ],
            PromptOverlayDialog::ConfirmDeleteExtraPrompt { title, .. } => vec![
                Line::styled(
                    "Delete custom prompt",
                    primary_text_style(self.palette).bold(),
                ),
                Line::raw(""),
                Line::from(vec![
                    Span::raw("Delete "),
                    Span::styled(title.clone(), command_accent_text_style(self.palette)),
                    Span::raw(" permanently?"),
                ]),
                Line::raw(""),
                Line::styled(
                    "Enter confirm · Esc cancel",
                    tertiary_text_style(self.palette),
                ),
            ],
        };

        let block = panel_block(self.palette);
        let inner_area = block.inner(dialog_area);
        frame.render_widget(block, dialog_area);
        frame.render_widget(Paragraph::new(lines), inner_area);
    }

    pub(super) fn handle_prompt_overlay_dialog_mouse_down(
        &mut self,
        column: u16,
        row: u16,
    ) -> OverlayInputResult {
        let Some(state) = self.prompt_overlay.as_mut() else {
            return OverlayInputResult::Handled;
        };
        let Some(layout) = prompt_overlay_layout_rects(Rect::new(0, 0, self.width, self.height))
        else {
            return OverlayInputResult::Handled;
        };
        let dialog_area = prompt_overlay_dialog_area(layout.right_pane);
        if !prompt_overlay_rect_contains(dialog_area, column, row) {
            return OverlayInputResult::Handled;
        }

        match state.dialog.as_mut() {
            Some(PromptOverlayDialog::CreateExtraPromptScope { selected_scope }) => {
                let inner_area = panel_block(self.palette).inner(dialog_area);
                let scope_row = inner_area.y.saturating_add(2);
                if row != scope_row {
                    return OverlayInputResult::Handled;
                }

                let project_label = if *selected_scope == PromptAssemblyScope::Project {
                    "[Project]"
                } else {
                    "Project"
                };
                let global_label = if *selected_scope == PromptAssemblyScope::Global {
                    "[Global]"
                } else {
                    "Global"
                };
                let project_end = inner_area.x.saturating_add(
                    u16::try_from(display_width(project_label)).unwrap_or(u16::MAX),
                );
                let global_start = project_end.saturating_add(1);
                let global_end = global_start
                    .saturating_add(u16::try_from(display_width(global_label)).unwrap_or(u16::MAX));

                if column >= inner_area.x && column < project_end {
                    *selected_scope = PromptAssemblyScope::Project;
                } else if column >= global_start && column < global_end {
                    *selected_scope = PromptAssemblyScope::Global;
                }
                OverlayInputResult::Handled
            }
            Some(PromptOverlayDialog::ConfirmDeleteExtraPrompt { .. }) => {
                OverlayInputResult::Handled
            }
            None => OverlayInputResult::Handled,
        }
    }

    pub(super) fn prompt_overlay_header_tab_at(
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

        for (index, tab) in PromptOverlayInactiveTab::ALL.iter().copied().enumerate() {
            if index > 0 {
                current_column = current_column.saturating_add(1);
            }
            let label = if tab == active_tab {
                format!("[{}]", tab.label())
            } else {
                tab.label().to_string()
            };
            let label_end = current_column.saturating_add(display_width(&label));
            if clicked_column >= current_column && clicked_column < label_end {
                return Some(tab);
            }
            current_column = label_end;
        }

        None
    }
}
