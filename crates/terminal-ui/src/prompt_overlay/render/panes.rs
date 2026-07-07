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

    pub(in crate::prompt_overlay) fn render_prompt_overlay_active_pane(
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

    pub(in crate::prompt_overlay) fn render_prompt_overlay_inactive_pane(
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
}
