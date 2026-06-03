use super::super::{Model, tool_approval_panel::ToolApprovalSource};

impl Model {
    pub(crate) fn open_tool_approval_debug_preview_panel(&mut self) {
        let old_value = self.composer_text().to_string();
        let old_line = self.composer.line();
        let old_column = self.composer.column();

        self.composer.reset_text_and_move_to_end(String::new());
        self.open_tool_approval_panel(
            ToolApprovalSource::Preview,
            "sed -n '1,80p' src/main.rs".to_string(),
            Vec::new(),
        );
        self.sync_external_editor_helper_after_draft_change(&old_value);
        self.sync_document_viewport_after_composer_interaction(&old_value, old_line, old_column);
    }
}
