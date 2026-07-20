use super::super::{Model, tool_approval_panel::ToolApprovalSource};

impl Model {
    /// 打开 tool approval 的 debug 预览面板；不触碰 composer，
    /// 命令文本的清理由内联命令执行路径统一负责。
    pub(crate) fn open_tool_approval_debug_preview_panel(&mut self) {
        self.open_tool_approval_panel(
            ToolApprovalSource::Preview,
            "sed -n '1,80p' src/main.rs".to_string(),
            Vec::new(),
        );
    }
}
