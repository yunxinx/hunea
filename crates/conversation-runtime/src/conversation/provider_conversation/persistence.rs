use std::sync::Arc;

use provider_protocol::ConversationItem;
use runtime_domain::session::TranscriptReplayItem;
use session_store::{ConfigSnapshot, SessionHeader, SessionId, SessionStore};

#[derive(Clone)]
pub(crate) struct PreparedConversationPersistence {
    pub(crate) store: Arc<dyn SessionStore>,
    pub(crate) session_id: Option<SessionId>,
    pub(crate) header_template: SessionHeader,
    pub(crate) config_snapshot: ConfigSnapshot,
    pub(crate) current_user_message: ConversationItem,
    pub(crate) transcript_user_message: ConversationItem,
    pub(crate) transcript_replay_after_user: Vec<TranscriptReplayItem>,
}

pub(super) struct ProviderConversationPersistence {
    pub(super) store: Arc<dyn SessionStore>,
    pub(super) session_id: Option<SessionId>,
    pub(super) header_template: SessionHeader,
}
