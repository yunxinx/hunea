use runtime_domain::session::{
    ContextBudgetLoadErrorPayload, ContextBudgetProjectionErrorKind, ContextBudgetSnapshotPayload,
    SessionLoadRequestId,
};

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ContextBudgetState {
    pub(crate) revision: usize,
    pub(crate) loading: bool,
    pub(crate) pending_request_id: Option<SessionLoadRequestId>,
    pub(crate) error: Option<ContextBudgetLoadErrorPayload>,
    pub(crate) snapshot: Option<ContextBudgetSnapshotPayload>,
}

impl Default for ContextBudgetState {
    fn default() -> Self {
        Self {
            revision: 1,
            loading: true,
            pending_request_id: None,
            error: None,
            snapshot: None,
        }
    }
}

impl ContextBudgetState {
    pub(crate) fn apply_snapshot(&mut self, payload: ContextBudgetSnapshotPayload) {
        self.revision = self.revision.saturating_add(1);
        self.loading = false;
        self.pending_request_id = None;
        self.error = None;
        self.snapshot = Some(payload);
    }

    pub(crate) fn set_error(&mut self, error: ContextBudgetLoadErrorPayload) {
        self.revision = self.revision.saturating_add(1);
        self.loading = false;
        self.pending_request_id = None;
        self.error = Some(error);
        self.snapshot = None;
    }
}

pub(crate) fn context_budget_error_message(error: &ContextBudgetLoadErrorPayload) -> String {
    match error {
        ContextBudgetLoadErrorPayload::UnknownProvider { provider_id } => {
            format!("Context budget could not find provider `{provider_id}`")
        }
        ContextBudgetLoadErrorPayload::UnsupportedProvider { provider_kind } => {
            format!("{provider_kind} cannot show context budget; use OpenAI/OpenAI-compatible.")
        }
        ContextBudgetLoadErrorPayload::RuntimeInternal { .. } => {
            "Failed to load context budget in the runtime.".to_string()
        }
        ContextBudgetLoadErrorPayload::ProjectionFailed { kind, .. } => match kind {
            ContextBudgetProjectionErrorKind::Protocol => {
                "Failed to build context budget from provider payload.".to_string()
            }
            ContextBudgetProjectionErrorKind::Transport => {
                "Failed to build context budget because the provider is unavailable.".to_string()
            }
            ContextBudgetProjectionErrorKind::Provider => {
                "Failed to build context budget because the provider rejected the request."
                    .to_string()
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projection_error_message_uses_stable_short_copy() {
        let text = context_budget_error_message(&ContextBudgetLoadErrorPayload::ProjectionFailed {
            kind: runtime_domain::session::ContextBudgetProjectionErrorKind::Protocol,
            status: None,
            detail: Some("protocol error: inconsistent message fragments".to_string()),
        });

        assert_eq!(
            text,
            "Failed to build context budget from provider payload."
        );
    }

    #[test]
    fn runtime_internal_error_message_uses_runtime_copy() {
        let text = context_budget_error_message(&ContextBudgetLoadErrorPayload::RuntimeInternal {
            detail: Some("dispatch failed".to_string()),
        });

        assert_eq!(text, "Failed to load context budget in the runtime.");
    }
}
