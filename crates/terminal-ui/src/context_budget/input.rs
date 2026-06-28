use crossterm::event::{KeyCode, KeyEvent};
use runtime_domain::session::SessionLoadRequestId;

use super::ContextBudgetState;
use crate::{Model, overlay_input_result::OverlayInputResult};

impl Model {
    pub(crate) fn context_budget_active(&self) -> bool {
        self.context_budget.is_some()
    }

    pub(crate) fn open_context_budget_loading(&mut self) -> SessionLoadRequestId {
        let request_id = self.next_session_load_request_id();
        self.close_composer_attached_ui();
        self.close_tool_approval_panel();
        self.close_model_panel();
        self.context_budget = Some(ContextBudgetState {
            pending_request_id: Some(request_id),
            ..ContextBudgetState::default()
        });
        self.sync_composer_height();
        self.sync_document_viewport_for_composer_cursor();
        request_id
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
        request_id: SessionLoadRequestId,
        payload: runtime_domain::session::ContextBudgetSnapshotPayload,
    ) {
        let Some(mut state) = self.context_budget.take() else {
            return;
        };
        if !state.loading || state.pending_request_id != Some(request_id) {
            self.context_budget = Some(state);
            return;
        }
        state.apply_snapshot(payload);
        self.context_budget = Some(state);
    }

    pub(crate) fn show_context_budget_error(
        &mut self,
        request_id: SessionLoadRequestId,
        message: &str,
    ) {
        let Some(mut state) = self.context_budget.take() else {
            return;
        };
        if !state.loading || state.pending_request_id != Some(request_id) {
            self.context_budget = Some(state);
            return;
        }
        state.set_error(message.to_string());
        self.context_budget = Some(state);
    }

    pub(crate) fn context_budget_load_request_matches(
        &self,
        request_id: SessionLoadRequestId,
    ) -> bool {
        self.context_budget
            .as_ref()
            .is_some_and(|state| state.loading && state.pending_request_id == Some(request_id))
    }

    #[cfg(test)]
    pub(crate) fn context_budget_pending_request_id_for_test(
        &self,
    ) -> Option<SessionLoadRequestId> {
        self.context_budget
            .as_ref()
            .and_then(|state| state.pending_request_id)
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
