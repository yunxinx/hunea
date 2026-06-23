use runtime_domain::session::{RuntimeEvent, SessionLoadRequestId, SessionTreePayload};

/// Session tree 的数据源相同，但不同覆盖层必须收到不同事件，避免迟到 payload 错投。
#[derive(Debug, Clone, Copy)]
pub(super) enum SessionTreeLoadConsumer {
    EntryTree,
    CopyPicker,
}

impl SessionTreeLoadConsumer {
    pub(super) fn loaded_event(
        self,
        request_id: SessionLoadRequestId,
        payload: SessionTreePayload,
    ) -> RuntimeEvent {
        match self {
            Self::EntryTree => RuntimeEvent::SessionTreeLoaded {
                request_id,
                payload,
            },
            Self::CopyPicker => RuntimeEvent::CopyPickerTreeLoaded {
                request_id,
                payload,
            },
        }
    }

    pub(super) fn failed_event(
        self,
        request_id: SessionLoadRequestId,
        message: String,
    ) -> RuntimeEvent {
        match self {
            Self::EntryTree => RuntimeEvent::SessionTreeLoadFailed {
                request_id,
                message,
            },
            Self::CopyPicker => RuntimeEvent::CopyPickerTreeLoadFailed {
                request_id,
                message,
            },
        }
    }

    pub(super) fn empty_tree_event(self, request_id: SessionLoadRequestId) -> RuntimeEvent {
        let payload = SessionTreePayload {
            rows: Vec::new(),
            current_row_id: None,
        };
        self.loaded_event(request_id, payload)
    }

    pub(super) const fn no_session_error(self) -> &'static str {
        match self {
            Self::EntryTree => "No active persisted session to show tree",
            Self::CopyPicker => "No active persisted session to copy from",
        }
    }
}
