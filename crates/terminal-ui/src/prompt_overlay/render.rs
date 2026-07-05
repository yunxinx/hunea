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
                Paragraph::new(vertical_rule_lines(
                    usize::from(gutter.height),
                    self.palette,
                )),
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

pub(super) fn prompt_overlay_matches_resolved_source(
    resolved: &ResolvedPromptSource,
    kind: PromptSourceKind,
    reference_id: &str,
    origin: Option<PromptSourceOrigin>,
) -> bool {
    resolved.kind == kind && resolved.reference_id == reference_id && resolved.origin == origin
}

pub(super) fn prompt_overlay_scope_picker_line(
    scope: PromptAssemblyScope,
    palette: crate::theme::TerminalPalette,
) -> Line<'static> {
    let (project_style, global_style) = match scope {
        PromptAssemblyScope::Project => (
            surface_text_style(palette).bold(),
            secondary_text_style(palette),
        ),
        PromptAssemblyScope::Global => (
            secondary_text_style(palette),
            surface_text_style(palette).bold(),
        ),
    };

    match scope {
        PromptAssemblyScope::Project => Line::from(vec![
            Span::styled("[Project]", project_style),
            Span::raw(" "),
            Span::styled("Global", global_style),
        ]),
        PromptAssemblyScope::Global => Line::from(vec![
            Span::styled("Project", project_style),
            Span::raw(" "),
            Span::styled("[Global]", global_style),
        ]),
    }
}

pub(super) fn prompt_overlay_inactive_row_id(row: &PromptOverlayInactiveRow) -> String {
    match row {
        PromptOverlayInactiveRow::ExtraPromptCandidate { source, .. }
        | PromptOverlayInactiveRow::ExtraPromptShadowedDetail { source } => {
            format!(
                "extra:{}:{}",
                source.reference_id,
                prompt_overlay_origin_label(source.origin)
            )
        }
        PromptOverlayInactiveRow::DiscoveredSkill { skill, .. }
        | PromptOverlayInactiveRow::DiscoveredSkillShadowedDetail { skill } => {
            format!(
                "skill:{}:{}",
                skill.skill_name,
                prompt_overlay_origin_label(skill.origin)
            )
        }
        PromptOverlayInactiveRow::ToolCandidate { tool } => {
            format!("tool:{}", tool.name)
        }
        PromptOverlayInactiveRow::DynamicEnvironmentCandidate { source } => {
            format!("dynamic:{:?}", source.source_kind)
        }
    }
}

pub(super) fn prompt_overlay_left_row_id(row: &PromptOverlayLeftRow) -> String {
    match row {
        PromptOverlayLeftRow::ManagedSource { source, .. } => format!(
            "managed:{}:{}:{}",
            prompt_overlay_kind_label(source.kind),
            source.reference_id,
            source.origin.map_or("none", prompt_overlay_origin_label),
        ),
        PromptOverlayLeftRow::ShadowedDetail { source } => format!(
            "shadowed:{}:{}:{}",
            prompt_overlay_kind_label(source.kind),
            source.reference_id,
            source.origin.map_or("none", prompt_overlay_origin_label),
        ),
    }
}

pub(super) fn prompt_overlay_partition_extra_candidates(
    mut candidates: Vec<PromptAssemblyExtraPromptCandidate>,
) -> (
    PromptAssemblyExtraPromptCandidate,
    Vec<PromptAssemblyExtraPromptCandidate>,
) {
    candidates.sort_by_key(|candidate| prompt_overlay_origin_sort_key(candidate.origin));
    let winner = candidates.remove(0);
    (winner, candidates)
}

pub(super) fn prompt_overlay_extra_candidate_winner(
    candidates: &[PromptAssemblyExtraPromptCandidate],
) -> &PromptAssemblyExtraPromptCandidate {
    candidates
        .iter()
        .min_by_key(|candidate| prompt_overlay_origin_sort_key(candidate.origin))
        .expect("extra prompt group should not be empty")
}

pub(super) fn prompt_overlay_partition_discovered_skills(
    mut skills: Vec<PromptAssemblyDiscoveredSkill>,
) -> (
    PromptAssemblyDiscoveredSkill,
    Vec<PromptAssemblyDiscoveredSkill>,
) {
    skills.sort_by_key(|skill| prompt_overlay_origin_sort_key(skill.origin));
    let winner = skills.remove(0);
    (winner, skills)
}

pub(super) fn prompt_overlay_discovered_skill_winner(
    skills: &[PromptAssemblyDiscoveredSkill],
) -> &PromptAssemblyDiscoveredSkill {
    skills
        .iter()
        .min_by_key(|skill| prompt_overlay_origin_sort_key(skill.origin))
        .expect("discovered skill group should not be empty")
}

pub(super) fn prompt_overlay_origin_sort_key(origin: PromptSourceOrigin) -> u8 {
    match origin {
        PromptSourceOrigin::Project => 0,
        PromptSourceOrigin::Global => 1,
        PromptSourceOrigin::Builtin => 2,
    }
}

pub(super) fn prompt_overlay_layout_rects(area: Rect) -> Option<PromptOverlayLayoutRects> {
    let chrome = fullscreen_list_chrome_rects(area)?;
    let [left_pane, _gutter, right_pane] = Layout::horizontal([
        Constraint::Ratio(
            PROMPT_OVERLAY_LEFT_PANE_RATIO_NUMERATOR,
            PROMPT_OVERLAY_PANE_RATIO_DENOMINATOR,
        ),
        Constraint::Length(1),
        Constraint::Ratio(
            PROMPT_OVERLAY_RIGHT_PANE_RATIO_NUMERATOR,
            PROMPT_OVERLAY_PANE_RATIO_DENOMINATOR,
        ),
    ])
    .areas(chrome.body);
    let [_left_header, left_body] =
        Layout::vertical([Constraint::Length(1), Constraint::Fill(1)]).areas(left_pane);
    let [_right_header, right_body] =
        Layout::vertical([Constraint::Length(1), Constraint::Fill(1)]).areas(right_pane);
    Some(PromptOverlayLayoutRects {
        chrome,
        left_pane,
        left_body,
        right_pane,
        right_body,
    })
}

pub(super) fn prompt_overlay_dialog_area(anchor_area: Rect) -> Rect {
    let dialog_width = anchor_area.width.min(52);
    let dialog_height = 7u16.min(anchor_area.height);
    let dialog_x = anchor_area
        .x
        .saturating_add(anchor_area.width.saturating_sub(dialog_width) / 2);
    let dialog_y = anchor_area
        .y
        .saturating_add(anchor_area.height.saturating_sub(dialog_height) / 2);
    Rect::new(dialog_x, dialog_y, dialog_width, dialog_height)
}

pub(super) fn prompt_overlay_rect_contains(area: Rect, column: u16, row: u16) -> bool {
    column >= area.x
        && column < area.x.saturating_add(area.width)
        && row >= area.y
        && row < area.y.saturating_add(area.height)
}

pub(super) fn prompt_overlay_visible_offset_for_row(body_area: Rect, row: u16) -> Option<usize> {
    (row >= body_area.y && row < body_area.y.saturating_add(body_area.height))
        .then(|| usize::from(row.saturating_sub(body_area.y)))
}

pub(super) fn prompt_overlay_selection_styles(
    selected: bool,
    focused: bool,
    palette: crate::theme::TerminalPalette,
) -> (Style, Style, &'static str) {
    let visually_selected = selected && focused;
    let item_style = if visually_selected {
        primary_text_style(palette).bold()
    } else {
        secondary_text_style(palette)
    };
    let marker_style = if visually_selected {
        command_accent_text_style(palette)
    } else {
        tertiary_text_style(palette)
    };
    let marker = if visually_selected { "█" } else { " " };
    (item_style, marker_style, marker)
}

struct PromptOverlayLineListWidget<'a> {
    lines: &'a [Line<'static>],
}

impl Widget for PromptOverlayLineListWidget<'_> {
    fn render(self, area: Rect, buf: &mut ratatui::buffer::Buffer) {
        for (row, line) in self.lines.iter().take(usize::from(area.height)).enumerate() {
            let y = area.y + u16::try_from(row).unwrap_or(u16::MAX);
            render_line_with_full_width_background(line, Rect::new(area.x, y, area.width, 1), buf);
        }
    }
}

pub(super) fn prompt_overlay_active_lines(
    sources: &[PromptOverlayLeftRow],
    selected: usize,
    scroll: usize,
    focused: bool,
    width: usize,
    body_height: usize,
    palette: crate::theme::TerminalPalette,
) -> Vec<Line<'static>> {
    if body_height == 0 {
        return Vec::new();
    }
    if sources.is_empty() {
        return vec![Line::styled(
            truncate_display_width_with_ellipsis("  No active sources", width.max(1)),
            tertiary_text_style(palette),
        )];
    }

    let mut lines = Vec::new();
    for (index, source) in sources.iter().enumerate().skip(scroll).take(body_height) {
        lines.push(prompt_overlay_left_row_line(
            source,
            index == selected,
            focused,
            width,
            palette,
        ));
    }
    lines
}

pub(super) fn prompt_overlay_inactive_lines(
    rows: &[PromptOverlayInactiveRow],
    selected_row_id: Option<&str>,
    scroll: usize,
    focused: bool,
    width: usize,
    body_height: usize,
    palette: crate::theme::TerminalPalette,
) -> Vec<Line<'static>> {
    if body_height == 0 {
        return Vec::new();
    }
    if rows.is_empty() {
        return vec![prompt_overlay_empty_inactive_line(
            "No candidates",
            width,
            palette,
        )];
    }

    let mut lines = Vec::new();
    for row in rows.iter().skip(scroll).take(body_height) {
        lines.push(prompt_overlay_inactive_row_line(
            row,
            selected_row_id == Some(prompt_overlay_inactive_row_id(row).as_str()),
            focused,
            width,
            palette,
        ));
    }
    lines
}

pub(super) fn prompt_overlay_left_row_line(
    row: &PromptOverlayLeftRow,
    selected: bool,
    focused: bool,
    width: usize,
    palette: crate::theme::TerminalPalette,
) -> Line<'static> {
    let content_width = width.saturating_sub(PROMPT_OVERLAY_ROW_PREFIX_WIDTH).max(1);
    let label = match row {
        PromptOverlayLeftRow::ManagedSource {
            source,
            status,
            shadowed_count,
        } => prompt_overlay_active_row_text(source, *status, *shadowed_count, content_width),
        PromptOverlayLeftRow::ShadowedDetail { source } => {
            prompt_overlay_shadowed_detail_row_text(source, content_width)
        }
    };
    let (item_style, marker_style, marker) =
        prompt_overlay_selection_styles(selected, focused, palette);
    prompt_overlay_list_line(
        marker,
        marker_style,
        truncate_display_width_with_ellipsis(&label, content_width),
        item_style,
    )
}

pub(super) fn prompt_overlay_discovered_skill_lines(
    skills: &[PromptOverlayInactiveRow],
    selected_row_id: Option<&str>,
    scroll: usize,
    focused: bool,
    width: usize,
    body_height: usize,
    palette: crate::theme::TerminalPalette,
) -> Vec<Line<'static>> {
    if body_height == 0 {
        return Vec::new();
    }
    if skills.is_empty() {
        return vec![prompt_overlay_empty_inactive_line(
            "No discovered skills",
            width,
            palette,
        )];
    }

    skills
        .iter()
        .map(|row| {
            let selected = selected_row_id == Some(prompt_overlay_inactive_row_id(row).as_str());
            let content_width = width.saturating_sub(PROMPT_OVERLAY_ROW_PREFIX_WIDTH).max(1);
            let (item_style, marker_style, marker) =
                prompt_overlay_selection_styles(selected, focused, palette);
            let label = prompt_overlay_skill_row_text(row, content_width);
            prompt_overlay_list_line(
                marker,
                marker_style,
                truncate_display_width_with_ellipsis(&label, content_width),
                item_style,
            )
        })
        .skip(scroll)
        .take(body_height)
        .collect()
}

pub(super) fn prompt_overlay_inactive_row_line(
    row: &PromptOverlayInactiveRow,
    selected: bool,
    focused: bool,
    width: usize,
    palette: crate::theme::TerminalPalette,
) -> Line<'static> {
    let content_width = width.saturating_sub(PROMPT_OVERLAY_ROW_PREFIX_WIDTH).max(1);
    let (item_style, marker_style, marker) =
        prompt_overlay_selection_styles(selected, focused, palette);
    let label = match row {
        PromptOverlayInactiveRow::ExtraPromptCandidate {
            source,
            shadowed_count,
        } => prompt_overlay_extra_row_text(source, *shadowed_count, content_width),
        PromptOverlayInactiveRow::ExtraPromptShadowedDetail { source } => {
            prompt_overlay_extra_shadowed_detail_row_text(source, content_width)
        }
        PromptOverlayInactiveRow::DiscoveredSkill {
            shadowed_count: _, ..
        } => prompt_overlay_skill_row_text(row, content_width),
        PromptOverlayInactiveRow::DiscoveredSkillShadowedDetail { .. } => {
            prompt_overlay_skill_row_text(row, content_width)
        }
        PromptOverlayInactiveRow::ToolCandidate { .. } => {
            prompt_overlay_tool_row_text(row, content_width)
        }
        PromptOverlayInactiveRow::DynamicEnvironmentCandidate { .. } => {
            prompt_overlay_dynamic_plain_row_text(row, content_width)
        }
    };
    prompt_overlay_list_line(
        marker,
        marker_style,
        truncate_display_width_with_ellipsis(&label, content_width),
        item_style,
    )
}

pub(super) fn prompt_overlay_origin_label(origin: PromptSourceOrigin) -> &'static str {
    match origin {
        PromptSourceOrigin::Builtin => "builtin",
        PromptSourceOrigin::Global => "global",
        PromptSourceOrigin::Project => "project",
    }
}

pub(super) fn prompt_overlay_kind_label(kind: PromptSourceKind) -> &'static str {
    match kind {
        PromptSourceKind::CoreSystemPrompt => "system",
        PromptSourceKind::InstructionsFile => "instructions",
        PromptSourceKind::ExtraPrompt => "custom",
        PromptSourceKind::SkillDiscovery => "discovery",
        PromptSourceKind::LongLivedSkill => "skill",
        PromptSourceKind::ToolGuidelines => "tools",
        PromptSourceKind::DynamicEnvironmentBaseline => "dynamic",
        PromptSourceKind::DynamicEnvironmentChanges => "dynamic",
    }
}

pub(super) fn prompt_overlay_active_header_text(width: usize) -> String {
    let source_width = prompt_overlay_left_source_width(width);
    let left_pad = " ".repeat(PROMPT_OVERLAY_OUTER_PADDING);
    let sel = prompt_overlay_center_text("Sel", PROMPT_OVERLAY_LEFT_SEL_WIDTH);
    let ord = left_pad_display_width("Ord", PROMPT_OVERLAY_LEFT_ORD_WIDTH);
    let source = format!(
        "{:<width$}",
        truncate_display_width_with_ellipsis("Source", source_width),
        width = source_width
    );
    let kind = format!("{:<width$}", "Type", width = PROMPT_OVERLAY_LEFT_KIND_WIDTH);
    let scope = format!(
        "{:<width$}",
        "Scope",
        width = PROMPT_OVERLAY_LEFT_SCOPE_WIDTH
    );
    let gap = " ".repeat(PROMPT_OVERLAY_COLUMN_GAP);
    let trailing = " ".repeat(PROMPT_OVERLAY_SCOPE_TRAILING_PADDING);
    format!("{left_pad}{sel}{gap}{ord}{gap}{source}{gap}{kind}{gap}{scope}{trailing}")
}

pub(super) fn prompt_overlay_extra_header_text(width: usize) -> String {
    let name_width = prompt_overlay_right_extra_name_width(width);
    let left_pad = " ".repeat(PROMPT_OVERLAY_OUTER_PADDING);
    let sel = prompt_overlay_center_text("Sel", PROMPT_OVERLAY_LEFT_SEL_WIDTH);
    let name = format!(
        "{:<width$}",
        truncate_display_width_with_ellipsis("Name", name_width),
        width = name_width
    );
    let scope = format!(
        "{:<width$}",
        "Scope",
        width = PROMPT_OVERLAY_RIGHT_SCOPE_WIDTH
    );
    let gap = " ".repeat(PROMPT_OVERLAY_COLUMN_GAP);
    let trailing = " ".repeat(PROMPT_OVERLAY_SCOPE_TRAILING_PADDING);
    format!("{left_pad}{sel}{gap}{name}{gap}{scope}{trailing}")
}

pub(super) fn prompt_overlay_skill_header_text(width: usize) -> String {
    let name_width = prompt_overlay_right_inactive_name_width(width);
    let left_pad = " ".repeat(PROMPT_OVERLAY_OUTER_PADDING);
    let sel = prompt_overlay_center_text("Sel", PROMPT_OVERLAY_LEFT_SEL_WIDTH);
    let ord = left_pad_display_width("Ord", PROMPT_OVERLAY_RIGHT_ORD_WIDTH);
    let name = format!(
        "{:<width$}",
        truncate_display_width_with_ellipsis("Name", name_width),
        width = name_width
    );
    let scope = format!(
        "{:<width$}",
        "Scope",
        width = PROMPT_OVERLAY_RIGHT_SCOPE_WIDTH
    );
    let gap = " ".repeat(PROMPT_OVERLAY_COLUMN_GAP);
    let trailing = " ".repeat(PROMPT_OVERLAY_SCOPE_TRAILING_PADDING);
    format!("{left_pad}{sel}{gap}{ord}{gap}{name}{gap}{scope}{trailing}")
}

pub(super) fn prompt_overlay_active_row_text(
    source: &PromptAssemblyManagedSource,
    status: PromptOverlayManagedStatus,
    shadowed_count: usize,
    width: usize,
) -> String {
    let source_width = prompt_overlay_left_source_width(width);
    let left_pad = " ".repeat(PROMPT_OVERLAY_OUTER_PADDING);
    let sel = prompt_overlay_center_text(
        if source.enabled { "●" } else { "○" },
        PROMPT_OVERLAY_LEFT_SEL_WIDTH,
    );
    let ord = left_pad_display_width(&source.order.to_string(), PROMPT_OVERLAY_LEFT_ORD_WIDTH);
    let status_marker = prompt_overlay_managed_status_marker(status, shadowed_count);
    let source_name = prompt_overlay_cell_with_trailing_marker(
        &source.title,
        status_marker.as_deref(),
        source_width,
    );
    let kind = format!(
        "{:<width$}",
        prompt_overlay_kind_label(source.kind),
        width = PROMPT_OVERLAY_LEFT_KIND_WIDTH
    );
    let scope = format!(
        "{:<width$}",
        source
            .origin
            .map(prompt_overlay_origin_label)
            .unwrap_or("-"),
        width = PROMPT_OVERLAY_LEFT_SCOPE_WIDTH
    );
    let gap = " ".repeat(PROMPT_OVERLAY_COLUMN_GAP);
    let trailing = " ".repeat(PROMPT_OVERLAY_SCOPE_TRAILING_PADDING);
    format!("{left_pad}{sel}{gap}{ord}{gap}{source_name}{gap}{kind}{gap}{scope}{trailing}")
}

pub(super) fn prompt_overlay_shadowed_detail_row_text(
    source: &ResolvedPromptSource,
    width: usize,
) -> String {
    let source_width = prompt_overlay_left_source_width(width);
    let left_pad = " ".repeat(PROMPT_OVERLAY_OUTER_PADDING);
    let sel = prompt_overlay_center_text("↳", PROMPT_OVERLAY_LEFT_SEL_WIDTH);
    let ord = left_pad_display_width("", PROMPT_OVERLAY_LEFT_ORD_WIDTH);
    let source_name = prompt_overlay_cell_with_trailing_marker(
        &format!(
            "shadowed {}",
            source
                .origin
                .map(prompt_overlay_origin_label)
                .unwrap_or("-")
        ),
        None,
        source_width,
    );
    let kind = format!(
        "{:<width$}",
        prompt_overlay_kind_label(source.kind),
        width = PROMPT_OVERLAY_LEFT_KIND_WIDTH
    );
    let scope = format!(
        "{:<width$}",
        source
            .origin
            .map(prompt_overlay_origin_label)
            .unwrap_or("-"),
        width = PROMPT_OVERLAY_LEFT_SCOPE_WIDTH
    );
    let gap = " ".repeat(PROMPT_OVERLAY_COLUMN_GAP);
    let trailing = " ".repeat(PROMPT_OVERLAY_SCOPE_TRAILING_PADDING);
    format!("{left_pad}{sel}{gap}{ord}{gap}{source_name}{gap}{kind}{gap}{scope}{trailing}")
}

pub(super) fn prompt_overlay_extra_row_text(
    source: &PromptAssemblyExtraPromptCandidate,
    shadowed_count: usize,
    width: usize,
) -> String {
    let name_width = prompt_overlay_right_extra_name_width(width);
    let left_pad = " ".repeat(PROMPT_OVERLAY_OUTER_PADDING);
    let sel = prompt_overlay_center_text(
        if source.selected { "●" } else { "○" },
        PROMPT_OVERLAY_LEFT_SEL_WIDTH,
    );
    let name = prompt_overlay_cell_with_trailing_marker(
        &source.title,
        prompt_overlay_shadowed_count_marker(shadowed_count).as_deref(),
        name_width,
    );
    let scope = format!(
        "{:<width$}",
        prompt_overlay_origin_label(source.origin),
        width = PROMPT_OVERLAY_RIGHT_SCOPE_WIDTH
    );
    let gap = " ".repeat(PROMPT_OVERLAY_COLUMN_GAP);
    let trailing = " ".repeat(PROMPT_OVERLAY_SCOPE_TRAILING_PADDING);
    format!("{left_pad}{sel}{gap}{name}{gap}{scope}{trailing}")
}

pub(super) fn prompt_overlay_skill_row_text(
    row: &PromptOverlayInactiveRow,
    width: usize,
) -> String {
    let name_width = prompt_overlay_right_inactive_name_width(width);
    let left_pad = " ".repeat(PROMPT_OVERLAY_OUTER_PADDING);
    let sel = prompt_overlay_center_text(
        &prompt_overlay_skill_sel_label(row),
        PROMPT_OVERLAY_LEFT_SEL_WIDTH,
    );
    let ord = left_pad_display_width(
        &prompt_overlay_skill_order_label(row),
        PROMPT_OVERLAY_RIGHT_ORD_WIDTH,
    );
    let name = prompt_overlay_skill_name_cell(row, name_width);
    let scope = format!(
        "{:<width$}",
        prompt_overlay_origin_label(prompt_overlay_inactive_skill_origin(row)),
        width = PROMPT_OVERLAY_RIGHT_SCOPE_WIDTH
    );
    let gap = " ".repeat(PROMPT_OVERLAY_COLUMN_GAP);
    let trailing = " ".repeat(PROMPT_OVERLAY_SCOPE_TRAILING_PADDING);
    format!("{left_pad}{sel}{gap}{ord}{gap}{name}{gap}{scope}{trailing}")
}

pub(super) fn prompt_overlay_extra_shadowed_detail_row_text(
    source: &PromptAssemblyExtraPromptCandidate,
    width: usize,
) -> String {
    let name_width = prompt_overlay_right_extra_name_width(width);
    let left_pad = " ".repeat(PROMPT_OVERLAY_OUTER_PADDING);
    let sel = prompt_overlay_center_text(
        if source.selected { "●" } else { "○" },
        PROMPT_OVERLAY_LEFT_SEL_WIDTH,
    );
    let name = prompt_overlay_cell_with_trailing_marker(
        &format!("shadowed {}", prompt_overlay_origin_label(source.origin)),
        None,
        name_width,
    );
    let scope = format!(
        "{:<width$}",
        prompt_overlay_origin_label(source.origin),
        width = PROMPT_OVERLAY_RIGHT_SCOPE_WIDTH
    );
    let gap = " ".repeat(PROMPT_OVERLAY_COLUMN_GAP);
    let trailing = " ".repeat(PROMPT_OVERLAY_SCOPE_TRAILING_PADDING);
    format!("{left_pad}{sel}{gap}{name}{gap}{scope}{trailing}")
}

pub(super) fn prompt_overlay_skill_sel_label(row: &PromptOverlayInactiveRow) -> String {
    match row {
        PromptOverlayInactiveRow::DiscoveredSkill { skill, .. }
        | PromptOverlayInactiveRow::DiscoveredSkillShadowedDetail { skill } => {
            if !skill.can_select_for_discovery {
                "-".to_string()
            } else if skill.selected {
                "●".to_string()
            } else {
                "○".to_string()
            }
        }
        _ => "-".to_string(),
    }
}

pub(super) fn prompt_overlay_skill_name_cell(
    row: &PromptOverlayInactiveRow,
    width: usize,
) -> String {
    match row {
        PromptOverlayInactiveRow::DiscoveredSkill {
            skill,
            shadowed_count,
        } => {
            let trailing = if *shadowed_count > 0 {
                prompt_overlay_shadowed_count_marker(*shadowed_count)
            } else if !skill.can_select_for_discovery {
                Some("(manual)".to_string())
            } else {
                None
            };
            prompt_overlay_cell_with_trailing_marker(&skill.title, trailing.as_deref(), width)
        }
        PromptOverlayInactiveRow::DiscoveredSkillShadowedDetail { skill } => {
            prompt_overlay_cell_with_trailing_marker(
                &format!("shadowed {}", prompt_overlay_origin_label(skill.origin)),
                None,
                width,
            )
        }
        _ => prompt_overlay_fill_cell("", width),
    }
}

pub(super) fn prompt_overlay_inactive_skill_origin(
    row: &PromptOverlayInactiveRow,
) -> PromptSourceOrigin {
    match row {
        PromptOverlayInactiveRow::DiscoveredSkill { skill, .. }
        | PromptOverlayInactiveRow::DiscoveredSkillShadowedDetail { skill } => skill.origin,
        _ => PromptSourceOrigin::Project,
    }
}

pub(super) fn prompt_overlay_skill_order_label(row: &PromptOverlayInactiveRow) -> String {
    match row {
        PromptOverlayInactiveRow::DiscoveredSkill { skill, .. } => skill
            .selected_order
            .map(|order| order.to_string())
            .unwrap_or_else(|| "-".to_string()),
        PromptOverlayInactiveRow::DiscoveredSkillShadowedDetail { .. } => String::new(),
        _ => "-".to_string(),
    }
}

pub(super) fn prompt_overlay_tool_lines(
    rows: &[PromptOverlayInactiveRow],
    selected_row_id: Option<&str>,
    scroll: usize,
    focused: bool,
    width: usize,
    body_height: usize,
    palette: crate::theme::TerminalPalette,
) -> Vec<Line<'static>> {
    if body_height == 0 {
        return Vec::new();
    }
    if rows.is_empty() {
        return vec![prompt_overlay_empty_inactive_line(
            "No tools available",
            width,
            palette,
        )];
    }

    rows.iter()
        .map(|row| {
            let selected = selected_row_id == Some(prompt_overlay_inactive_row_id(row).as_str());
            let content_width = width.saturating_sub(PROMPT_OVERLAY_ROW_PREFIX_WIDTH).max(1);
            let (item_style, marker_style, marker) =
                prompt_overlay_selection_styles(selected, focused, palette);
            let label = prompt_overlay_tool_row_text(row, content_width);
            prompt_overlay_list_line(
                marker,
                marker_style,
                truncate_display_width_with_ellipsis(&label, content_width),
                item_style,
            )
        })
        .skip(scroll)
        .take(body_height)
        .collect()
}

pub(super) fn prompt_overlay_tool_row_text(row: &PromptOverlayInactiveRow, width: usize) -> String {
    let name_width = prompt_overlay_right_inactive_name_width(width);
    let left_pad = " ".repeat(PROMPT_OVERLAY_OUTER_PADDING);
    let sel = prompt_overlay_center_text(
        &prompt_overlay_tool_sel_label(row),
        PROMPT_OVERLAY_LEFT_SEL_WIDTH,
    );
    let ord = left_pad_display_width(
        &prompt_overlay_tool_order_label(row),
        PROMPT_OVERLAY_RIGHT_ORD_WIDTH,
    );
    let name = prompt_overlay_tool_name_cell(row, name_width);
    let scope = format!(
        "{:<width$}",
        prompt_overlay_origin_label(prompt_overlay_tool_origin(row)),
        width = PROMPT_OVERLAY_RIGHT_SCOPE_WIDTH
    );
    let gap = " ".repeat(PROMPT_OVERLAY_COLUMN_GAP);
    let trailing = " ".repeat(PROMPT_OVERLAY_SCOPE_TRAILING_PADDING);
    format!("{left_pad}{sel}{gap}{ord}{gap}{name}{gap}{scope}{trailing}")
}

pub(super) fn prompt_overlay_tool_sel_label(row: &PromptOverlayInactiveRow) -> String {
    match row {
        PromptOverlayInactiveRow::ToolCandidate { tool } => {
            if !tool.can_select {
                "-".to_string()
            } else if tool.selected {
                "●".to_string()
            } else {
                "○".to_string()
            }
        }
        _ => "-".to_string(),
    }
}

pub(super) fn prompt_overlay_tool_name_cell(
    row: &PromptOverlayInactiveRow,
    width: usize,
) -> String {
    match row {
        PromptOverlayInactiveRow::ToolCandidate { tool } => {
            let label = tool.label.as_deref().unwrap_or(&tool.name);
            prompt_overlay_cell_with_trailing_marker(label, None, width)
        }
        _ => prompt_overlay_fill_cell("", width),
    }
}

pub(super) fn prompt_overlay_tool_order_label(row: &PromptOverlayInactiveRow) -> String {
    match row {
        PromptOverlayInactiveRow::ToolCandidate { tool } => tool
            .selected_order
            .map(|order| order.to_string())
            .unwrap_or_else(|| "-".to_string()),
        _ => "-".to_string(),
    }
}

pub(super) fn prompt_overlay_tool_origin(row: &PromptOverlayInactiveRow) -> PromptSourceOrigin {
    match row {
        PromptOverlayInactiveRow::ToolCandidate { tool } => tool.origin,
        _ => PromptSourceOrigin::Project,
    }
}

#[derive(Clone, Copy)]
pub(super) struct PromptOverlayDynamicSelection<'a> {
    row_id: Option<&'a str>,
    snapshot_kind: DynamicEnvironmentSnapshotKind,
}

pub(super) fn prompt_overlay_dynamic_lines(
    rows: &[PromptOverlayInactiveRow],
    selection: PromptOverlayDynamicSelection<'_>,
    scroll: usize,
    focused: bool,
    width: usize,
    body_height: usize,
    palette: crate::theme::TerminalPalette,
) -> Vec<Line<'static>> {
    if body_height == 0 {
        return Vec::new();
    }
    if rows.is_empty() {
        return vec![prompt_overlay_empty_inactive_line(
            "No dynamic sources",
            width,
            palette,
        )];
    }

    rows.iter()
        .map(|row| {
            let selected = selection.row_id == Some(prompt_overlay_inactive_row_id(row).as_str());
            prompt_overlay_dynamic_row_line(
                row,
                selected,
                focused,
                selection.snapshot_kind,
                width,
                palette,
            )
        })
        .skip(scroll)
        .take(body_height)
        .collect()
}

pub(super) fn prompt_overlay_dynamic_row_line(
    row: &PromptOverlayInactiveRow,
    selected: bool,
    focused: bool,
    selected_snapshot_kind: DynamicEnvironmentSnapshotKind,
    width: usize,
    palette: crate::theme::TerminalPalette,
) -> Line<'static> {
    let content_width = width.saturating_sub(PROMPT_OVERLAY_ROW_PREFIX_WIDTH).max(1);
    let (item_style, marker_style, marker) =
        prompt_overlay_selection_styles(selected, focused, palette);
    let highlighted_cell_style = if selected && focused {
        command_accent_text_style(palette)
            .bold()
            .add_modifier(Modifier::UNDERLINED)
    } else {
        item_style
    };
    let name_width = prompt_overlay_dynamic_name_width(content_width);
    let left_pad = " ".repeat(PROMPT_OVERLAY_OUTER_PADDING);
    let gap = " ".repeat(PROMPT_OVERLAY_COLUMN_GAP);
    let scope = format!(
        "{:<width$}",
        prompt_overlay_origin_label(prompt_overlay_dynamic_origin(row)),
        width = PROMPT_OVERLAY_RIGHT_SCOPE_WIDTH
    );
    let trailing = " ".repeat(PROMPT_OVERLAY_SCOPE_TRAILING_PADDING);
    let baseline_style = if selected_snapshot_kind == DynamicEnvironmentSnapshotKind::Baseline {
        highlighted_cell_style
    } else {
        item_style
    };
    let changes_style = if selected_snapshot_kind == DynamicEnvironmentSnapshotKind::Changes {
        highlighted_cell_style
    } else {
        item_style
    };
    let baseline = prompt_overlay_dynamic_checkbox_spans(
        row,
        DynamicEnvironmentSnapshotKind::Baseline,
        baseline_style,
        item_style,
    );
    let changes = prompt_overlay_dynamic_checkbox_spans(
        row,
        DynamicEnvironmentSnapshotKind::Changes,
        changes_style,
        item_style,
    );
    let name = prompt_overlay_dynamic_name_cell(row, name_width);

    Line::from(vec![
        Span::styled(marker.to_string(), marker_style),
        Span::styled(left_pad, item_style),
        baseline.0,
        baseline.1,
        baseline.2,
        Span::styled(gap.clone(), item_style),
        changes.0,
        changes.1,
        changes.2,
        Span::styled(gap.clone(), item_style),
        Span::styled(name, item_style),
        Span::styled(gap, item_style),
        Span::styled(scope, item_style),
        Span::styled(trailing, item_style),
    ])
}

pub(super) fn prompt_overlay_dynamic_plain_row_text(
    row: &PromptOverlayInactiveRow,
    width: usize,
) -> String {
    let name_width = prompt_overlay_dynamic_name_width(width);
    let left_pad = " ".repeat(PROMPT_OVERLAY_OUTER_PADDING);
    let baseline = prompt_overlay_center_text(
        &prompt_overlay_dynamic_checkbox_label(row, DynamicEnvironmentSnapshotKind::Baseline),
        PROMPT_OVERLAY_DYNAMIC_CHECKBOX_WIDTH,
    );
    let changes = prompt_overlay_center_text(
        &prompt_overlay_dynamic_checkbox_label(row, DynamicEnvironmentSnapshotKind::Changes),
        PROMPT_OVERLAY_DYNAMIC_CHECKBOX_WIDTH,
    );
    let name = prompt_overlay_dynamic_name_cell(row, name_width);
    let scope = format!(
        "{:<width$}",
        prompt_overlay_origin_label(prompt_overlay_dynamic_origin(row)),
        width = PROMPT_OVERLAY_RIGHT_SCOPE_WIDTH
    );
    let gap = " ".repeat(PROMPT_OVERLAY_COLUMN_GAP);
    let trailing = " ".repeat(PROMPT_OVERLAY_SCOPE_TRAILING_PADDING);
    format!("{left_pad}{baseline}{gap}{changes}{gap}{name}{gap}{scope}{trailing}")
}

pub(super) fn prompt_overlay_dynamic_checkbox_label(
    row: &PromptOverlayInactiveRow,
    snapshot_kind: DynamicEnvironmentSnapshotKind,
) -> String {
    match row {
        PromptOverlayInactiveRow::DynamicEnvironmentCandidate { source } => {
            if prompt_overlay_dynamic_source_selected(source, snapshot_kind) {
                "[x]".to_string()
            } else {
                "[ ]".to_string()
            }
        }
        _ => "-".to_string(),
    }
}

pub(super) fn prompt_overlay_dynamic_checkbox_spans(
    row: &PromptOverlayInactiveRow,
    snapshot_kind: DynamicEnvironmentSnapshotKind,
    checkbox_style: Style,
    padding_style: Style,
) -> (Span<'static>, Span<'static>, Span<'static>) {
    let label = prompt_overlay_dynamic_checkbox_label(row, snapshot_kind);
    let label_width = display_width(&label).min(PROMPT_OVERLAY_DYNAMIC_CHECKBOX_WIDTH);
    let left_padding = PROMPT_OVERLAY_DYNAMIC_CHECKBOX_WIDTH.saturating_sub(label_width) / 2;
    let right_padding = PROMPT_OVERLAY_DYNAMIC_CHECKBOX_WIDTH
        .saturating_sub(label_width)
        .saturating_sub(left_padding);

    (
        Span::styled(" ".repeat(left_padding), padding_style),
        Span::styled(label, checkbox_style),
        Span::styled(" ".repeat(right_padding), padding_style),
    )
}

pub(super) fn prompt_overlay_dynamic_checkbox_hit_test(
    column: u16,
    body_area: Rect,
) -> Option<DynamicEnvironmentSnapshotKind> {
    if body_area.width == 0 || column < body_area.x {
        return None;
    }

    let relative_column = usize::from(column.saturating_sub(body_area.x));
    let baseline_start = PROMPT_OVERLAY_ROW_PREFIX_WIDTH + PROMPT_OVERLAY_OUTER_PADDING;
    let baseline_end = baseline_start.saturating_add(PROMPT_OVERLAY_DYNAMIC_CHECKBOX_WIDTH);
    if relative_column >= baseline_start && relative_column < baseline_end {
        return Some(DynamicEnvironmentSnapshotKind::Baseline);
    }

    let changes_start = baseline_end.saturating_add(PROMPT_OVERLAY_COLUMN_GAP);
    let changes_end = changes_start.saturating_add(PROMPT_OVERLAY_DYNAMIC_CHECKBOX_WIDTH);
    if relative_column >= changes_start && relative_column < changes_end {
        return Some(DynamicEnvironmentSnapshotKind::Changes);
    }

    None
}

pub(super) fn prompt_overlay_dynamic_source_selected(
    source: &PromptAssemblyDynamicEnvironmentCandidate,
    snapshot_kind: DynamicEnvironmentSnapshotKind,
) -> bool {
    match snapshot_kind {
        DynamicEnvironmentSnapshotKind::Baseline => source.baseline_selected,
        DynamicEnvironmentSnapshotKind::Changes => source.changes_selected,
    }
}

pub(super) fn prompt_overlay_dynamic_name_cell(
    row: &PromptOverlayInactiveRow,
    width: usize,
) -> String {
    match row {
        PromptOverlayInactiveRow::DynamicEnvironmentCandidate { source } => {
            prompt_overlay_cell_with_trailing_marker(&source.label, None, width)
        }
        _ => prompt_overlay_fill_cell("", width),
    }
}

pub(super) fn prompt_overlay_dynamic_origin(row: &PromptOverlayInactiveRow) -> PromptSourceOrigin {
    match row {
        PromptOverlayInactiveRow::DynamicEnvironmentCandidate { source } => source.origin,
        _ => PromptSourceOrigin::Project,
    }
}

pub(super) fn prompt_overlay_dynamic_header_text(width: usize) -> String {
    let name_width = prompt_overlay_dynamic_name_width(width);
    let left_pad = " ".repeat(PROMPT_OVERLAY_OUTER_PADDING);
    let baseline = prompt_overlay_center_text("Base", PROMPT_OVERLAY_DYNAMIC_CHECKBOX_WIDTH);
    let changes = prompt_overlay_center_text("Change", PROMPT_OVERLAY_DYNAMIC_CHECKBOX_WIDTH);
    let name = format!(
        "{:<width$}",
        truncate_display_width_with_ellipsis("Source", name_width),
        width = name_width
    );
    let scope = format!(
        "{:<width$}",
        "Scope",
        width = PROMPT_OVERLAY_RIGHT_SCOPE_WIDTH
    );
    let gap = " ".repeat(PROMPT_OVERLAY_COLUMN_GAP);
    let trailing = " ".repeat(PROMPT_OVERLAY_SCOPE_TRAILING_PADDING);
    format!("{left_pad}{baseline}{gap}{changes}{gap}{name}{gap}{scope}{trailing}")
}

pub(super) fn prompt_overlay_tool_header_text(width: usize) -> String {
    let name_width = prompt_overlay_right_inactive_name_width(width);
    let left_pad = " ".repeat(PROMPT_OVERLAY_OUTER_PADDING);
    let sel = prompt_overlay_center_text("Sel", PROMPT_OVERLAY_LEFT_SEL_WIDTH);
    let ord = left_pad_display_width("Ord", PROMPT_OVERLAY_RIGHT_ORD_WIDTH);
    let name = format!(
        "{:<width$}",
        truncate_display_width_with_ellipsis("Name", name_width),
        width = name_width
    );
    let scope = format!(
        "{:<width$}",
        "Scope",
        width = PROMPT_OVERLAY_RIGHT_SCOPE_WIDTH
    );
    let gap = " ".repeat(PROMPT_OVERLAY_COLUMN_GAP);
    let trailing = " ".repeat(PROMPT_OVERLAY_SCOPE_TRAILING_PADDING);
    format!("{left_pad}{sel}{gap}{ord}{gap}{name}{gap}{scope}{trailing}")
}

pub(super) fn prompt_overlay_shadowed_count_marker(shadowed_count: usize) -> Option<String> {
    (shadowed_count > 0).then(|| format!("+{shadowed_count} shadowed"))
}

pub(super) fn prompt_overlay_managed_status_marker(
    status: PromptOverlayManagedStatus,
    shadowed_count: usize,
) -> Option<String> {
    match status {
        PromptOverlayManagedStatus::Active => prompt_overlay_shadowed_count_marker(shadowed_count),
        PromptOverlayManagedStatus::Missing => Some("missing".to_string()),
        PromptOverlayManagedStatus::Shadowed => Some("shadowed".to_string()),
        PromptOverlayManagedStatus::Disabled => None,
    }
}

pub(super) fn prompt_overlay_cell_with_trailing_marker(
    text: &str,
    trailing_marker: Option<&str>,
    width: usize,
) -> String {
    let width = width.max(1);
    let Some(trailing_marker) = trailing_marker.filter(|marker| !marker.is_empty()) else {
        return prompt_overlay_fill_cell(text, width);
    };

    let trailing_width = display_width(trailing_marker);
    let reserved_width = trailing_width.saturating_add(1);
    if width <= reserved_width {
        return truncate_display_width_with_ellipsis(trailing_marker, width);
    }

    let text_width = width.saturating_sub(reserved_width);
    let visible_text = truncate_display_width_with_ellipsis(text, text_width);
    let padding = text_width.saturating_sub(display_width(&visible_text));
    format!("{visible_text}{} {trailing_marker}", " ".repeat(padding))
}

pub(super) fn prompt_overlay_fill_cell(text: &str, width: usize) -> String {
    let visible_text = truncate_display_width_with_ellipsis(text, width);
    let padding = width.saturating_sub(display_width(&visible_text));
    format!("{visible_text}{}", " ".repeat(padding))
}

pub(super) fn prompt_overlay_left_source_width(width: usize) -> usize {
    width
        .saturating_sub(PROMPT_OVERLAY_OUTER_PADDING)
        .saturating_sub(PROMPT_OVERLAY_LEFT_SEL_WIDTH)
        .saturating_sub(PROMPT_OVERLAY_LEFT_ORD_WIDTH)
        .saturating_sub(PROMPT_OVERLAY_LEFT_KIND_WIDTH)
        .saturating_sub(PROMPT_OVERLAY_LEFT_SCOPE_WIDTH)
        .saturating_sub(PROMPT_OVERLAY_SCOPE_TRAILING_PADDING)
        .saturating_sub(PROMPT_OVERLAY_COLUMN_GAP * 4)
        .max(12)
}

pub(super) fn prompt_overlay_right_extra_name_width(width: usize) -> usize {
    width
        .saturating_sub(PROMPT_OVERLAY_OUTER_PADDING)
        .saturating_sub(PROMPT_OVERLAY_LEFT_SEL_WIDTH)
        .saturating_sub(PROMPT_OVERLAY_RIGHT_SCOPE_WIDTH)
        .saturating_sub(PROMPT_OVERLAY_SCOPE_TRAILING_PADDING)
        .saturating_sub(PROMPT_OVERLAY_COLUMN_GAP * 2)
        .max(12)
}

pub(super) fn prompt_overlay_right_inactive_name_width(width: usize) -> usize {
    width
        .saturating_sub(PROMPT_OVERLAY_OUTER_PADDING)
        .saturating_sub(PROMPT_OVERLAY_LEFT_SEL_WIDTH)
        .saturating_sub(PROMPT_OVERLAY_RIGHT_ORD_WIDTH)
        .saturating_sub(PROMPT_OVERLAY_RIGHT_SCOPE_WIDTH)
        .saturating_sub(PROMPT_OVERLAY_SCOPE_TRAILING_PADDING)
        .saturating_sub(PROMPT_OVERLAY_COLUMN_GAP * 3)
        .max(12)
}

pub(super) fn prompt_overlay_dynamic_name_width(width: usize) -> usize {
    width
        .saturating_sub(PROMPT_OVERLAY_OUTER_PADDING)
        .saturating_sub(PROMPT_OVERLAY_DYNAMIC_CHECKBOX_WIDTH * 2)
        .saturating_sub(PROMPT_OVERLAY_RIGHT_SCOPE_WIDTH)
        .saturating_sub(PROMPT_OVERLAY_SCOPE_TRAILING_PADDING)
        .saturating_sub(PROMPT_OVERLAY_COLUMN_GAP * 3)
        .max(12)
}

pub(super) fn prompt_overlay_list_line(
    marker: &str,
    marker_style: Style,
    content: String,
    content_style: Style,
) -> Line<'static> {
    Line::from(vec![
        Span::styled(marker.to_string(), marker_style),
        Span::styled(content, content_style),
    ])
}

pub(super) fn prompt_overlay_empty_inactive_line(
    message: &str,
    width: usize,
    palette: crate::theme::TerminalPalette,
) -> Line<'static> {
    let content_width = width.saturating_sub(PROMPT_OVERLAY_ROW_PREFIX_WIDTH).max(1);
    let right_prefix = " ".repeat(PROMPT_OVERLAY_OUTER_PADDING);
    let content = format!(
        "{}{}",
        right_prefix,
        truncate_display_width_with_ellipsis(
            message,
            content_width
                .saturating_sub(PROMPT_OVERLAY_OUTER_PADDING)
                .max(1)
        ),
    );

    prompt_overlay_list_line(
        " ",
        tertiary_text_style(palette),
        truncate_display_width_with_ellipsis(&content, content_width),
        tertiary_text_style(palette),
    )
}

pub(super) fn prompt_overlay_center_text(value: &str, width: usize) -> String {
    let value_width = display_width(value).min(width);
    let left = width.saturating_sub(value_width) / 2;
    let right = width.saturating_sub(value_width).saturating_sub(left);
    format!("{}{}{}", " ".repeat(left), value, " ".repeat(right))
}

pub(super) fn selection_label(label: Option<&str>, selected: usize, total: usize) -> String {
    let position = if total == 0 {
        0
    } else {
        selected.min(total.saturating_sub(1)) + 1
    };

    match label {
        Some(label) => format!(" {label} {position}/{total} "),
        None => format!(" {position}/{total} "),
    }
}

pub(super) fn clamp_scroll(
    current_scroll: usize,
    selected: usize,
    total: usize,
    visible_rows: usize,
) -> usize {
    if total == 0 {
        return 0;
    }
    let visible_rows = visible_rows.max(1);
    let max_scroll = total.saturating_sub(visible_rows);
    let mut scroll = current_scroll.min(max_scroll);
    if selected < scroll {
        scroll = selected;
    }
    if selected >= scroll.saturating_add(visible_rows) {
        scroll = selected + 1 - visible_rows;
    }
    scroll.min(max_scroll)
}

pub(super) fn prompt_overlay_active_visible_rows(height: u16) -> usize {
    let chrome = fullscreen_list_chrome_rects(Rect::new(0, 0, 1, height));
    let body_height = chrome.map(|rects| rects.body.height).unwrap_or_default();
    usize::from(body_height.saturating_sub(1)).max(1)
}

pub(super) fn prompt_overlay_inactive_visible_rows(height: u16) -> usize {
    let chrome = fullscreen_list_chrome_rects(Rect::new(0, 0, 1, height));
    let body_height = chrome.map(|rects| rects.body.height).unwrap_or_default();
    usize::from(body_height.saturating_sub(1)).max(1)
}

pub(super) fn vertical_rule_lines(
    height: usize,
    palette: crate::theme::TerminalPalette,
) -> Vec<Line<'static>> {
    (0..height)
        .map(|_| Line::styled("│", tertiary_text_style(palette)))
        .collect()
}

pub(super) fn prompt_scope_from_origin(origin: PromptSourceOrigin) -> Option<PromptAssemblyScope> {
    match origin {
        PromptSourceOrigin::Builtin => None,
        PromptSourceOrigin::Global => Some(PromptAssemblyScope::Global),
        PromptSourceOrigin::Project => Some(PromptAssemblyScope::Project),
    }
}

pub(super) fn allows_shift_only_modifier(modifiers: KeyModifiers) -> bool {
    modifiers.is_empty() || modifiers == KeyModifiers::SHIFT
}

pub(super) fn normalize_prompt_overlay_external_editor_draft(content: &str) -> String {
    content.replace("\r\n", "\n").replace('\r', "\n")
}
