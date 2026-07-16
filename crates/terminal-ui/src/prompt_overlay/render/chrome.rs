use super::*;

impl Model {
    pub(in crate::prompt_overlay) fn prompt_overlay_header_line(
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

    pub(in crate::prompt_overlay) fn prompt_overlay_footer_line(
        &self,
        width: u16,
    ) -> Line<'static> {
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
        let selected_tool_candidate = matches!(
            self.selected_prompt_overlay_selection(),
            Some(PromptOverlaySelection::ToolCandidate(_))
        );
        let selected_previewable = self.selected_prompt_overlay_selection().is_some();
        if actions.can_edit() {
            parts.push("e/ctrl+g edit");
        }
        if actions.can_add_custom() {
            parts.push("a create prompt");
        }
        if actions.can_remove() {
            parts.push("d remove");
        }
        if actions.can_toggle_selection() {
            if selected_tool_candidate {
                // Tools tab 是列选择交互：←/→ 切列，x 切换当前列。
                parts.push("←/→ column");
                parts.push("x toggle");
            } else {
                parts.push("x disable");
            }
        }
        if selected_core {
            parts.push("r restore");
        }
        if selected_discovered_skill {
            parts.push("r reset order");
        }
        if actions.can_reorder_active() && width >= 120 {
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

    pub(in crate::prompt_overlay) fn prompt_overlay_active_header_line(
        &self,
        width: usize,
    ) -> Line<'static> {
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

    pub(in crate::prompt_overlay) fn prompt_overlay_inactive_header_line(
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

    pub(in crate::prompt_overlay) fn prompt_overlay_shortcut_help_open(&self) -> bool {
        self.prompt_overlay
            .as_ref()
            .is_some_and(|state| state.shortcut_help_open)
    }

    pub(in crate::prompt_overlay) fn toggle_prompt_overlay_shortcut_help(&mut self) {
        let Some(state) = self.prompt_overlay.as_mut() else {
            return;
        };
        state.shortcut_help_open = !state.shortcut_help_open;
    }

    pub(in crate::prompt_overlay) fn close_prompt_overlay_shortcut_help(&mut self) {
        let Some(state) = self.prompt_overlay.as_mut() else {
            return;
        };
        state.shortcut_help_open = false;
    }

    pub(in crate::prompt_overlay) fn prompt_overlay_shortcut_help_area(
        &self,
        bounds: Rect,
    ) -> Rect {
        let lines = self.prompt_overlay_shortcut_help_lines();
        ShortcutHelpPopover {
            title: Some("More"),
            lines: &lines,
        }
        .area(bounds)
    }

    pub(in crate::prompt_overlay) fn prompt_overlay_shortcut_help_lines(
        &self,
    ) -> Vec<Line<'static>> {
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

    pub(in crate::prompt_overlay) fn prompt_overlay_focused_page_label(
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
}
