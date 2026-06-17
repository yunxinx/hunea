use super::*;

impl Model {
    pub(crate) fn render_copy_picker(&mut self, frame: &mut RenderFrame<'_>, area: Rect) {
        if self.copy_picker_preview_active() {
            self.render_copy_picker_preview(frame, area);
            return;
        }

        let Some(state) = self.copy_picker.as_ref() else {
            return;
        };
        frame.render_widget(Clear, area);
        let Some(chrome) = fullscreen_list_chrome_rects(area) else {
            return;
        };
        let page_size = fullscreen_list_page_size_for_height(area.height);

        frame.render_widget(
            Paragraph::new(self.copy_picker_header_line(state, usize::from(area.width))),
            chrome.header,
        );
        frame.render_widget(
            Paragraph::new(subtle_rule_line(usize::from(area.width), self.palette)),
            chrome.header_rule,
        );

        let lines = self.copy_picker_body_lines(
            state,
            usize::from(area.width),
            usize::from(chrome.body.height),
            page_size,
        );
        frame.render_widget(CopyPickerWidget { lines: &lines }, chrome.body);

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
                copy_picker_footer_hint(area.width, state.selected_count()),
                tertiary_text_style(self.palette).add_modifier(Modifier::ITALIC),
            )),
            chrome.footer,
        );
    }

    fn render_copy_picker_preview(&mut self, frame: &mut RenderFrame<'_>, area: Rect) {
        let palette = self.palette;
        let content_height = usize::from(area.height.saturating_sub(2).max(1));
        let Some(preview) = self
            .copy_picker
            .as_mut()
            .and_then(|state| state.preview.as_mut())
        else {
            return;
        };
        render_transcript_overlay_view(
            frame,
            area,
            &mut preview.transcript_preview.transcript,
            &mut preview.transcript_preview.overlay,
            TranscriptOverlayRenderOptions {
                palette,
                content_height,
                footer_hint: copy_picker_preview_footer_hint(area.width),
                progress_style: TranscriptOverlayProgressStyle::Page,
            },
        );
    }

    fn copy_picker_header_line(&self, state: &CopyPickerState, width: usize) -> Line<'static> {
        let title = format!(
            "Copy Messages ({} of {})",
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

    fn copy_picker_body_lines(
        &self,
        state: &CopyPickerState,
        width: usize,
        body_height: usize,
        page_size: usize,
    ) -> Vec<Line<'static>> {
        let width = width.max(1);
        let mut lines = Vec::new();

        if state.is_loading {
            lines.push(Line::styled(
                "  Loading copy picker...",
                tertiary_text_style(self.palette),
            ));
        } else if let Some(error) = state.error.as_deref() {
            lines.push(Line::styled(
                truncate_display_width_with_ellipsis(&format!("  {error}"), width),
                tertiary_text_style(self.palette),
            ));
        } else if state.rows.is_empty() {
            lines.push(Line::styled(
                "  No user or assistant messages",
                tertiary_text_style(self.palette),
            ));
        } else {
            let page_start = state.page_start(page_size);
            for (visible_position, row_index) in state.page_indices(page_size).enumerate() {
                let row = &state.rows[row_index];
                let absolute_position = page_start + visible_position;
                lines.push(self.copy_picker_row_line(
                    row,
                    width,
                    absolute_position == state.selected,
                    state.is_row_selected(row_index),
                    absolute_position.is_multiple_of(2),
                ));
            }
        }

        lines.truncate(body_height);
        lines
    }

    fn copy_picker_row_line(
        &self,
        row: &CopyPickerRow,
        width: usize,
        is_cursor: bool,
        is_selected: bool,
        is_even: bool,
    ) -> Line<'static> {
        let left_padding = " ".repeat(COPY_PICKER_BODY_HORIZONTAL_PADDING);
        let marker = if is_selected { "█ " } else { "  " };
        let kind_prefix = session_tree_row_kind_prefix(
            row.kind.session_tree_kind(),
            TreeRowKindPrefixAlignment::Left,
        );
        let prefix_width =
            display_width(&left_padding) + COPY_PICKER_MARKER_WIDTH + display_width(&kind_prefix);
        let text_width = width
            .saturating_sub(prefix_width)
            .saturating_sub(COPY_PICKER_BODY_HORIZONTAL_PADDING);
        let row_style = copy_picker_row_style(self.palette, is_even);
        let text_style = copy_picker_content_style(row.kind, self.palette, is_cursor);
        let summary_style = if is_cursor {
            text_style.bg(Color::Reset).add_modifier(Modifier::REVERSED)
        } else {
            text_style
        };

        Line::from(vec![
            Span::raw(left_padding),
            Span::styled(
                marker.to_string(),
                approval_rejected_text_style(self.palette),
            ),
            Span::styled(
                kind_prefix,
                session_tree_row_kind_label_style(row.kind.session_tree_kind(), self.palette),
            ),
            Span::styled(
                truncate_display_width_with_ellipsis(&row.summary, text_width),
                summary_style,
            ),
        ])
        .style(row_style)
    }
}

struct CopyPickerWidget<'a> {
    lines: &'a [Line<'static>],
}

impl Widget for CopyPickerWidget<'_> {
    fn render(self, area: Rect, buf: &mut ratatui::buffer::Buffer) {
        for (row, line) in self.lines.iter().take(usize::from(area.height)).enumerate() {
            let y = area.y + u16::try_from(row).unwrap_or(u16::MAX);
            render_line_with_full_width_background(line, Rect::new(area.x, y, area.width, 1), buf);
        }
    }
}

fn copy_picker_content_style(
    kind: CopyableSessionTreeRowKind,
    palette: crate::theme::TerminalPalette,
    is_cursor: bool,
) -> Style {
    match kind {
        CopyableSessionTreeRowKind::User => command_accent_text_style(palette),
        CopyableSessionTreeRowKind::Assistant if is_cursor => primary_text_style(palette).bold(),
        CopyableSessionTreeRowKind::Assistant => secondary_text_style(palette),
    }
}

fn copy_picker_row_style(palette: crate::theme::TerminalPalette, is_even: bool) -> Style {
    if is_even {
        surface_text_style(palette)
    } else {
        Style::new()
    }
}

fn copy_picker_footer_hint(width: u16, selected_count: usize) -> String {
    let selection_label = if selected_count == 0 {
        "A: select all"
    } else {
        "A: invert selection"
    };
    let selected_label = if selected_count == 0 {
        String::new()
    } else {
        format!(" · {selected_count} selected")
    };
    if width < 90 {
        format!(
            "  Esc close · Space preview · Tab select · {selection_label} · C raw · c display · j/k · h/l page{selected_label}"
        )
    } else {
        format!(
            "  Esc close · Space preview · Tab select · {selection_label} · C copy raw · c copy display · ↑/↓/j/k move · ←/→/h/l page{selected_label}"
        )
    }
}

fn copy_picker_preview_footer_hint(width: u16) -> &'static str {
    if width < 90 {
        "  Esc back · Space back · C raw · c display · h/l page"
    } else {
        "  Esc back to copy list · Space back · C copy raw · c copy display · ←/→/h/l page"
    }
}
