use super::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum PromptOverlayDialog {
    CreateExtraPromptScope {
        selected_scope: PromptAssemblyScope,
    },
    ConfirmDeleteExtraPrompt {
        scope: PromptAssemblyScope,
        reference_id: String,
        title: String,
    },
}

impl Model {
    pub(super) fn prompt_overlay_dialog_active(&self) -> bool {
        self.prompt_overlay
            .as_ref()
            .and_then(|state| state.dialog.as_ref())
            .is_some()
    }

    pub(super) fn handle_prompt_overlay_dialog_key(&mut self, key: KeyEvent) -> OverlayInputResult {
        match key.code {
            KeyCode::Esc if key.modifiers.is_empty() => {
                self.close_prompt_overlay_dialog();
                OverlayInputResult::Handled
            }
            KeyCode::Left
            | KeyCode::Char('h')
            | KeyCode::Up
            | KeyCode::Char('k')
            | KeyCode::Right
            | KeyCode::Char('l')
            | KeyCode::Down
            | KeyCode::Char('j')
                if key.modifiers.is_empty() =>
            {
                self.toggle_prompt_overlay_dialog_scope();
                OverlayInputResult::Handled
            }
            KeyCode::Enter if key.modifiers.is_empty() => {
                OverlayInputResult::from_effect(self.confirm_prompt_overlay_dialog())
            }
            _ => OverlayInputResult::Handled,
        }
    }

    pub(super) fn handle_prompt_overlay_dialog_mouse_down(
        &mut self,
        column: u16,
        row: u16,
    ) -> OverlayInputResult {
        let Some(layout) = prompt_overlay_layout_rects(Rect::new(0, 0, self.width, self.height))
        else {
            return OverlayInputResult::Handled;
        };
        let dialog_area = prompt_overlay_dialog_area(layout.right_pane);
        if !prompt_overlay_rect_contains(dialog_area, column, row) {
            return OverlayInputResult::Handled;
        }

        let Some(state) = self.prompt_overlay.as_mut() else {
            return OverlayInputResult::Handled;
        };
        match state.dialog.as_mut() {
            Some(PromptOverlayDialog::CreateExtraPromptScope { selected_scope }) => {
                if let Some(next_scope) = select_prompt_overlay_dialog_scope_at_column(
                    *selected_scope,
                    column,
                    row,
                    panel_block(self.palette).inner(dialog_area),
                ) {
                    *selected_scope = next_scope;
                }
                OverlayInputResult::Handled
            }
            Some(PromptOverlayDialog::ConfirmDeleteExtraPrompt { .. }) | None => {
                OverlayInputResult::Handled
            }
        }
    }

    pub(super) fn open_create_extra_prompt_scope_picker(&mut self) {
        let Some(state) = self.prompt_overlay.as_mut() else {
            return;
        };
        if state.focus != PromptOverlayFocus::Inactive
            || state.inactive_tab != PromptOverlayInactiveTab::ExtraPrompts
        {
            return;
        }
        state.dialog = Some(PromptOverlayDialog::CreateExtraPromptScope {
            selected_scope: PromptAssemblyScope::Project,
        });
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

    fn close_prompt_overlay_dialog(&mut self) {
        if let Some(state) = self.prompt_overlay.as_mut() {
            state.dialog = None;
        }
    }

    fn toggle_prompt_overlay_dialog_scope(&mut self) {
        let Some(state) = self.prompt_overlay.as_mut() else {
            return;
        };
        let Some(PromptOverlayDialog::CreateExtraPromptScope { selected_scope }) =
            state.dialog.as_mut()
        else {
            return;
        };
        *selected_scope = match selected_scope {
            PromptAssemblyScope::Global => PromptAssemblyScope::Project,
            PromptAssemblyScope::Project => PromptAssemblyScope::Global,
        };
    }

    fn confirm_prompt_overlay_dialog(&mut self) -> Option<AppEffect> {
        let dialog = self
            .prompt_overlay
            .as_ref()
            .and_then(|state| state.dialog.clone())?;
        self.close_prompt_overlay_dialog();
        match dialog {
            PromptOverlayDialog::CreateExtraPromptScope { selected_scope } => {
                self.create_extra_prompt_from_overlay(selected_scope)
            }
            PromptOverlayDialog::ConfirmDeleteExtraPrompt {
                scope,
                reference_id,
                ..
            } => Some(self.delete_extra_prompt_effect(scope, reference_id)),
        }
    }
}

fn select_prompt_overlay_dialog_scope_at_column(
    current_scope: PromptAssemblyScope,
    column: u16,
    row: u16,
    inner_area: Rect,
) -> Option<PromptAssemblyScope> {
    let scope_row = inner_area.y.saturating_add(2);
    if row != scope_row {
        return None;
    }

    let project_label = prompt_overlay_dialog_scope_label(
        PromptAssemblyScope::Project,
        current_scope == PromptAssemblyScope::Project,
    );
    let global_label = prompt_overlay_dialog_scope_label(
        PromptAssemblyScope::Global,
        current_scope == PromptAssemblyScope::Global,
    );
    let project_end = inner_area
        .x
        .saturating_add(u16::try_from(display_width(project_label)).unwrap_or(u16::MAX));
    let global_start = project_end.saturating_add(1);
    let global_end =
        global_start.saturating_add(u16::try_from(display_width(global_label)).unwrap_or(u16::MAX));

    if column >= inner_area.x && column < project_end {
        Some(PromptAssemblyScope::Project)
    } else if column >= global_start && column < global_end {
        Some(PromptAssemblyScope::Global)
    } else {
        None
    }
}

fn prompt_overlay_dialog_scope_label(
    scope: PromptAssemblyScope,
    is_selected: bool,
) -> &'static str {
    match (scope, is_selected) {
        (PromptAssemblyScope::Project, true) => "[Project]",
        (PromptAssemblyScope::Project, false) => "Project",
        (PromptAssemblyScope::Global, true) => "[Global]",
        (PromptAssemblyScope::Global, false) => "Global",
    }
}
