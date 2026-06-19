use crossterm::event::{KeyCode, KeyEvent};

use crate::{
    AppEffect, Model, overlay_input_result::OverlayInputResult, tool_result::ToolResultKind,
};

use super::{
    ToolApprovalChoice, ToolApprovalPanelState, ToolApprovalSource,
    file_preview::file_preview_fullscreen_content_height, file_preview_fullscreen_max_offset,
    tool_approval_choices,
};

impl Model {
    pub(crate) fn handle_tool_approval_panel_key(&mut self, key: KeyEvent) -> OverlayInputResult {
        if !self.tool_approval_panel_active() {
            return OverlayInputResult::Ignored;
        }

        if self.tool_approval_fullscreen_preview_active() {
            return self.handle_tool_approval_fullscreen_preview_key(key);
        }
        if self.tool_approval_panel.preview.is_some() {
            return self.handle_tool_approval_inline_file_preview_key(key);
        }

        match key.code {
            KeyCode::Up | KeyCode::Down if key.modifiers.is_empty() => {
                move_tool_approval_selection(
                    &mut self.tool_approval_panel,
                    if key.code == KeyCode::Up {
                        ToolApprovalSelectionMove::Up
                    } else {
                        ToolApprovalSelectionMove::Down
                    },
                );
                self.tool_approval_panel_revision =
                    self.tool_approval_panel_revision.saturating_add(1);
                OverlayInputResult::Handled
            }
            KeyCode::Left | KeyCode::Right if key.modifiers.is_empty() => {
                move_tool_approval_selection(
                    &mut self.tool_approval_panel,
                    if key.code == KeyCode::Left {
                        ToolApprovalSelectionMove::Left
                    } else {
                        ToolApprovalSelectionMove::Right
                    },
                );
                self.tool_approval_panel_revision =
                    self.tool_approval_panel_revision.saturating_add(1);
                OverlayInputResult::Handled
            }
            KeyCode::Enter if key.modifiers.is_empty() => {
                let choices = tool_approval_choices(&self.tool_approval_panel);
                let choice = choices.get(self.tool_approval_panel.selected).copied();
                self.apply_tool_approval_choice(choice)
            }
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                self.apply_tool_approval_choice(preferred_tool_approval_choice(
                    &self.tool_approval_panel,
                    &[
                        ToolApprovalChoice::Allow,
                        ToolApprovalChoice::AllowInSession,
                    ],
                ))
            }
            KeyCode::Char('n') | KeyCode::Char('N') => {
                self.apply_tool_approval_choice(preferred_tool_approval_choice(
                    &self.tool_approval_panel,
                    &[ToolApprovalChoice::Deny, ToolApprovalChoice::DenyInSession],
                ))
            }
            KeyCode::Esc if key.modifiers.is_empty() => {
                OverlayInputResult::from_effect(self.cancel_tool_approval_panel())
            }
            _ => OverlayInputResult::Handled, // 模态覆盖层吞掉未绑定输入，防止落入 composer
        }
    }

    fn handle_tool_approval_fullscreen_preview_key(&mut self, key: KeyEvent) -> OverlayInputResult {
        match key.code {
            KeyCode::Up | KeyCode::Char('k') if key.modifiers.is_empty() => {
                self.scroll_tool_approval_fullscreen_preview_by(-1);
                OverlayInputResult::Handled
            }
            KeyCode::Down | KeyCode::Char('j') if key.modifiers.is_empty() => {
                self.scroll_tool_approval_fullscreen_preview_by(1);
                OverlayInputResult::Handled
            }
            KeyCode::PageUp => {
                let page = file_preview_fullscreen_content_height(self)
                    .saturating_sub(1)
                    .max(1);
                self.scroll_tool_approval_fullscreen_preview_by(-(page as isize));
                OverlayInputResult::Handled
            }
            KeyCode::PageDown => {
                let page = file_preview_fullscreen_content_height(self)
                    .saturating_sub(1)
                    .max(1);
                self.scroll_tool_approval_fullscreen_preview_by(page as isize);
                OverlayInputResult::Handled
            }
            KeyCode::Char('u') if key.modifiers == crossterm::event::KeyModifiers::CONTROL => {
                let half_page = file_preview_fullscreen_content_height(self) / 2;
                self.scroll_tool_approval_fullscreen_preview_by(-(half_page.max(1) as isize));
                OverlayInputResult::Handled
            }
            KeyCode::Char('d') if key.modifiers == crossterm::event::KeyModifiers::CONTROL => {
                let half_page = file_preview_fullscreen_content_height(self) / 2;
                self.scroll_tool_approval_fullscreen_preview_by(half_page.max(1) as isize);
                OverlayInputResult::Handled
            }
            KeyCode::Home => {
                self.tool_approval_panel.preview_scroll_offset = 0;
                OverlayInputResult::Handled
            }
            KeyCode::End => {
                self.tool_approval_panel.preview_scroll_offset =
                    file_preview_fullscreen_max_offset(self);
                OverlayInputResult::Handled
            }
            KeyCode::Enter if key.modifiers.is_empty() => {
                self.apply_tool_approval_choice(preferred_tool_approval_choice(
                    &self.tool_approval_panel,
                    &[
                        ToolApprovalChoice::Allow,
                        ToolApprovalChoice::AllowInSession,
                    ],
                ))
            }
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                self.apply_tool_approval_choice(preferred_tool_approval_choice(
                    &self.tool_approval_panel,
                    &[
                        ToolApprovalChoice::Allow,
                        ToolApprovalChoice::AllowInSession,
                    ],
                ))
            }
            KeyCode::Char('n') | KeyCode::Char('N') => {
                self.apply_tool_approval_choice(preferred_tool_approval_choice(
                    &self.tool_approval_panel,
                    &[ToolApprovalChoice::Deny, ToolApprovalChoice::DenyInSession],
                ))
            }
            KeyCode::Esc if key.modifiers.is_empty() => {
                OverlayInputResult::from_effect(self.cancel_tool_approval_panel())
            }
            _ => OverlayInputResult::Handled, // 模态覆盖层吞掉未绑定输入，防止落入 composer
        }
    }

    fn handle_tool_approval_inline_file_preview_key(
        &mut self,
        key: KeyEvent,
    ) -> OverlayInputResult {
        match key.code {
            KeyCode::Enter if key.modifiers.is_empty() => {
                self.apply_tool_approval_choice(preferred_tool_approval_choice(
                    &self.tool_approval_panel,
                    &[
                        ToolApprovalChoice::Allow,
                        ToolApprovalChoice::AllowInSession,
                    ],
                ))
            }
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                self.apply_tool_approval_choice(preferred_tool_approval_choice(
                    &self.tool_approval_panel,
                    &[
                        ToolApprovalChoice::Allow,
                        ToolApprovalChoice::AllowInSession,
                    ],
                ))
            }
            KeyCode::Char('n') | KeyCode::Char('N') => {
                self.apply_tool_approval_choice(preferred_tool_approval_choice(
                    &self.tool_approval_panel,
                    &[ToolApprovalChoice::Deny, ToolApprovalChoice::DenyInSession],
                ))
            }
            KeyCode::Esc if key.modifiers.is_empty() => {
                OverlayInputResult::from_effect(self.cancel_tool_approval_panel())
            }
            _ => OverlayInputResult::Handled, // 模态覆盖层吞掉未绑定输入，防止落入 composer
        }
    }

    fn apply_tool_approval_choice(
        &mut self,
        choice: Option<ToolApprovalChoice>,
    ) -> OverlayInputResult {
        let Some(choice) = choice else {
            return OverlayInputResult::Handled;
        };

        let Some(source) = self.tool_approval_panel.source.clone() else {
            return OverlayInputResult::Handled;
        };

        match source {
            ToolApprovalSource::RuntimePermission {
                target,
                request_id,
                allow_option_id,
                allow_always_option_id,
                reject_option_id,
                reject_always_option_id,
            } => {
                let option_id = match choice {
                    ToolApprovalChoice::Allow => allow_option_id,
                    ToolApprovalChoice::AllowInSession => allow_always_option_id,
                    ToolApprovalChoice::Deny => reject_option_id,
                    ToolApprovalChoice::DenyInSession => reject_always_option_id,
                };
                let Some(option_id) = option_id else {
                    return OverlayInputResult::Handled;
                };

                self.close_tool_approval_panel();
                OverlayInputResult::Effect(AppEffect::RespondRuntimePermission {
                    target,
                    request_id,
                    option_id: Some(option_id),
                })
            }
            ToolApprovalSource::Preview => {
                let title = self.tool_approval_panel.title.clone();
                self.close_tool_approval_panel();
                self.append_tool_result_from_runtime(
                    approval_result_content(choice, &title),
                    approval_result_kind(choice),
                );
                OverlayInputResult::Handled
            }
        }
    }

    fn cancel_tool_approval_panel(&mut self) -> Option<AppEffect> {
        let source = self.tool_approval_panel.source.clone();
        self.close_tool_approval_panel();

        match source {
            Some(ToolApprovalSource::RuntimePermission {
                target, request_id, ..
            }) => Some(AppEffect::RespondRuntimePermission {
                target,
                request_id,
                option_id: None,
            }),
            Some(ToolApprovalSource::Preview) | None => None,
        }
    }
}

fn approval_result_kind(choice: ToolApprovalChoice) -> ToolResultKind {
    match choice {
        ToolApprovalChoice::Allow | ToolApprovalChoice::AllowInSession => ToolResultKind::Ran,
        ToolApprovalChoice::Deny | ToolApprovalChoice::DenyInSession => ToolResultKind::Rejected,
    }
}

fn approval_result_content(choice: ToolApprovalChoice, title: &str) -> String {
    let verb = match approval_result_kind(choice) {
        ToolResultKind::Ran => "Ran",
        ToolResultKind::Rejected => "Reject",
    };
    let title = title.trim();
    if title.is_empty() {
        verb.to_string()
    } else {
        format!("{verb} {title}")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ToolApprovalSelectionMove {
    Up,
    Down,
    Left,
    Right,
}

fn move_tool_approval_selection(
    state: &mut ToolApprovalPanelState,
    direction: ToolApprovalSelectionMove,
) {
    let choices = tool_approval_choices(state);
    let choice_count = choices.len();
    if choice_count == 0 {
        state.selected = 0;
        return;
    }

    state.selected = match direction {
        ToolApprovalSelectionMove::Up | ToolApprovalSelectionMove::Left => state
            .selected
            .checked_sub(1)
            .unwrap_or(choice_count.saturating_sub(1)),
        ToolApprovalSelectionMove::Down | ToolApprovalSelectionMove::Right => {
            (state.selected + 1) % choice_count
        }
    };
}

fn preferred_tool_approval_choice(
    state: &ToolApprovalPanelState,
    preferred: &[ToolApprovalChoice],
) -> Option<ToolApprovalChoice> {
    let choices = tool_approval_choices(state);
    preferred
        .iter()
        .copied()
        .find(|preferred_choice| choices.contains(preferred_choice))
}
