use crossterm::event::{KeyCode, KeyEvent};

use super::ContextBudgetState;
use crate::{Model, overlay_input_result::OverlayInputResult};

impl Model {
    pub(crate) fn context_budget_active(&self) -> bool {
        self.context_budget.is_some()
    }

    pub(crate) fn open_context_budget_loading(&mut self) {
        self.close_composer_attached_ui();
        self.close_tool_approval_panel();
        self.close_model_panel();
        self.context_budget = Some(ContextBudgetState::default());
        self.sync_composer_height();
        self.sync_document_viewport_for_composer_cursor();
    }

    pub(crate) fn close_context_budget(&mut self) {
        if self.context_budget.is_none() {
            return;
        }
        self.context_budget = None;
        self.sync_composer_height();
        self.sync_document_viewport_for_composer_cursor();
    }

    pub(crate) fn apply_context_budget_snapshot(
        &mut self,
        payload: runtime_domain::session::ContextBudgetSnapshotPayload,
    ) {
        let Some(mut state) = self.context_budget.take() else {
            return;
        };
        state.apply_snapshot(payload);
        self.context_budget = Some(state);
    }

    pub(crate) fn show_context_budget_error(&mut self, message: &str) {
        let Some(mut state) = self.context_budget.take() else {
            return;
        };
        state.set_error(message.to_string());
        self.context_budget = Some(state);
    }

    pub(crate) fn handle_context_budget_key(&mut self, key: KeyEvent) -> OverlayInputResult {
        if self.context_budget.is_none() {
            return OverlayInputResult::Ignored;
        }
        if key.code == KeyCode::Esc && key.modifiers.is_empty() {
            self.close_context_budget();
            return OverlayInputResult::Handled;
        }
        OverlayInputResult::Handled
    }
}
