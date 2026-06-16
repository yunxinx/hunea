use std::path::PathBuf;

use provider_protocol::Role;

use crate::{
    SESSION_MESSAGE_PREVIEW_CHAR_LIMIT, SESSION_TITLE_FALLBACK_CHAR_LIMIT, SessionEntry,
    SessionEntryKind, SessionHeader, SessionMeta, SessionStoreError,
    util::{normalize_project_dir, truncate_chars},
};

#[derive(Default)]
pub(crate) struct SessionMetaDeriver {
    header_entry: Option<(SessionHeader, i64)>,
    first_user_message: Option<String>,
    latest_user_message: Option<String>,
    latest_assistant_message: Option<String>,
    latest_model: Option<String>,
    updated_at: Option<i64>,
}

impl SessionMetaDeriver {
    pub(crate) fn observe(&mut self, entry: &SessionEntry) {
        self.updated_at = Some(entry.timestamp);

        match &entry.kind {
            SessionEntryKind::Header(header) if self.header_entry.is_none() => {
                self.header_entry = Some((header.clone(), entry.timestamp));
            }
            SessionEntryKind::Header(_) => {}
            SessionEntryKind::Item(item) if item.role() == Some(Role::User) => {
                let text = item.text_content();
                if self.first_user_message.is_none() {
                    self.first_user_message = Some(text.clone());
                }
                self.latest_user_message = Some(text);
            }
            SessionEntryKind::Item(item) if item.role() == Some(Role::Assistant) => {
                self.latest_assistant_message = Some(item.text_content());
            }
            SessionEntryKind::ConfigChange(snapshot) => {
                self.latest_model = Some(snapshot.model.clone());
            }
            _ => {}
        }
    }

    pub(crate) fn finish(
        self,
        jsonl_path: PathBuf,
        size_bytes: Option<u64>,
        missing_header_message: String,
    ) -> Result<SessionMeta, SessionStoreError> {
        let (header, created_at) =
            self.header_entry
                .ok_or_else(|| SessionStoreError::MissingHeader {
                    message: missing_header_message,
                })?;
        let title = header
            .session_name
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .or_else(|| {
                self.first_user_message
                    .as_deref()
                    .map(|text| truncate_chars(text, SESSION_TITLE_FALLBACK_CHAR_LIMIT))
            })
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| header.session_id.to_string());
        let preview = self
            .latest_user_message
            .as_deref()
            .map(|text| truncate_chars(text, SESSION_MESSAGE_PREVIEW_CHAR_LIMIT))
            .filter(|value| !value.is_empty());
        let first_user_preview = self
            .first_user_message
            .as_deref()
            .map(|text| truncate_chars(text, SESSION_MESSAGE_PREVIEW_CHAR_LIMIT))
            .filter(|value| !value.is_empty());
        let last_assistant_preview = self
            .latest_assistant_message
            .as_deref()
            .map(|text| truncate_chars(text, SESSION_MESSAGE_PREVIEW_CHAR_LIMIT))
            .filter(|value| !value.is_empty());

        Ok(SessionMeta {
            session_id: header.session_id.clone(),
            project_dir: normalize_project_dir(&header.work_dir),
            title,
            preview,
            first_user_preview,
            last_assistant_preview,
            total_tokens: 0,
            model: self
                .latest_model
                .or_else(|| Some(header.initial_model.clone())),
            created_at,
            updated_at: self.updated_at.unwrap_or(created_at),
            git_head: header.git_head.clone(),
            work_dir: header.work_dir.clone(),
            jsonl_path,
            size_bytes,
        })
    }
}

pub(crate) fn derive_session_meta(
    entries: &[SessionEntry],
    jsonl_path: PathBuf,
    size_bytes: Option<u64>,
    missing_header_message: String,
) -> Result<SessionMeta, SessionStoreError> {
    let mut deriver = SessionMetaDeriver::default();
    for entry in entries {
        deriver.observe(entry);
    }
    deriver.finish(jsonl_path, size_bytes, missing_header_message)
}
