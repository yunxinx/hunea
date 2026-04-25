use crossterm::event::{KeyCode, KeyEvent};

use super::{AppEffect, Model};

/// `PendingAcpPermission` 保存当前等待用户确认的 ACP 权限请求。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PendingAcpPermission {
    pub(super) request_id: String,
    pub(super) title: Option<String>,
    pub(super) allow_option_id: Option<String>,
    pub(super) reject_option_id: Option<String>,
}

impl Model {
    pub(crate) fn show_acp_permission_request(
        &mut self,
        request_id: String,
        title: Option<String>,
        allow_option_id: Option<String>,
        reject_option_id: Option<String>,
    ) {
        self.pending_acp_permission = Some(PendingAcpPermission {
            request_id,
            title: title.clone(),
            allow_option_id,
            reject_option_id,
        });
        let title = title.as_deref().unwrap_or("agent action");
        self.show_persistent_status_notice(&format!("ACP permission: {title} — y allow / n deny"));
    }

    pub(crate) fn handle_acp_permission_key(&mut self, key: KeyEvent) -> Option<Option<AppEffect>> {
        let pending = self.pending_acp_permission.as_ref()?;
        let response = match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => Some(pending.allow_option_id.clone()),
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                Some(pending.reject_option_id.clone())
            }
            _ => return Some(None),
        };

        let request_id = pending.request_id.clone();
        self.pending_acp_permission = None;
        self.clear_status_notice();
        Some(Some(AppEffect::RespondAcpPermission {
            request_id,
            option_id: response.flatten(),
        }))
    }
}
