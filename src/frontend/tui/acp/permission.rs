use super::super::{Model, tool_approval_panel::ToolApprovalSource};

/// `PendingAcpPermission` 保存当前等待用户确认的 ACP 权限请求。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::frontend::tui) struct PendingAcpPermission {
    pub(in crate::frontend::tui) request_id: String,
    pub(in crate::frontend::tui) reject_option_id: Option<String>,
}

impl Model {
    pub(crate) fn show_acp_permission_request(
        &mut self,
        request_id: String,
        title: Option<String>,
        allow_option_id: Option<String>,
        allow_always_option_id: Option<String>,
        reject_option_id: Option<String>,
        reject_always_option_id: Option<String>,
    ) {
        self.pending_acp_permission = Some(PendingAcpPermission {
            request_id: request_id.clone(),
            reject_option_id: reject_option_id
                .clone()
                .or_else(|| reject_always_option_id.clone()),
        });
        let title = title.as_deref().unwrap_or("");
        self.clear_status_notice();
        self.open_tool_approval_panel(
            ToolApprovalSource::AcpPermission {
                request_id,
                allow_option_id,
                allow_always_option_id,
                reject_option_id,
                reject_always_option_id,
            },
            title.to_string(),
            Vec::new(),
        );
    }
}
