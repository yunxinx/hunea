use super::super::{
    Model, acp_tool_preview::ToolApprovalPreview, tool_approval_panel::ToolApprovalSource,
};

/// `PendingAcpPermission` 保存当前等待用户确认的 ACP 权限请求。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::frontend::tui) struct PendingAcpPermission {
    pub(in crate::frontend::tui) request_id: String,
    pub(in crate::frontend::tui) tool_call_id: Option<String>,
    pub(in crate::frontend::tui) tool_call_item_index: Option<usize>,
}

/// `AcpPermissionPanelRequest` 汇总打开 ACP 审批面板需要的前端状态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::frontend::tui) struct AcpPermissionPanelRequest {
    pub(in crate::frontend::tui) request_id: String,
    pub(in crate::frontend::tui) tool_call_id: Option<String>,
    pub(in crate::frontend::tui) title: Option<String>,
    pub(in crate::frontend::tui) allow_option_id: Option<String>,
    pub(in crate::frontend::tui) allow_always_option_id: Option<String>,
    pub(in crate::frontend::tui) reject_option_id: Option<String>,
    pub(in crate::frontend::tui) reject_always_option_id: Option<String>,
    pub(in crate::frontend::tui) preview: Option<ToolApprovalPreview>,
    pub(in crate::frontend::tui) tool_call_item_index: Option<usize>,
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
        self.show_acp_permission_request_with_preview(AcpPermissionPanelRequest {
            request_id,
            tool_call_id: None,
            title,
            allow_option_id,
            allow_always_option_id,
            reject_option_id,
            reject_always_option_id,
            preview: None,
            tool_call_item_index: None,
        });
    }

    pub(in crate::frontend::tui) fn show_acp_permission_request_with_preview(
        &mut self,
        request: AcpPermissionPanelRequest,
    ) {
        let AcpPermissionPanelRequest {
            request_id,
            tool_call_id,
            title,
            allow_option_id,
            allow_always_option_id,
            reject_option_id,
            reject_always_option_id,
            preview,
            tool_call_item_index,
        } = request;
        self.pending_acp_permission = Some(PendingAcpPermission {
            request_id: request_id.clone(),
            tool_call_id,
            tool_call_item_index,
        });
        let title = title.as_deref().unwrap_or("");
        self.clear_status_notice();
        self.open_tool_approval_panel_with_preview(
            ToolApprovalSource::AcpPermission {
                request_id,
                allow_option_id,
                allow_always_option_id,
                reject_option_id,
                reject_always_option_id,
            },
            title.to_string(),
            Vec::new(),
            preview,
        );
    }
}
