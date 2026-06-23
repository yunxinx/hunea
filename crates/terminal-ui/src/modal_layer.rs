use crate::{Model, runner::TerminalMouseModePreference};

/// `ModalLayer` 表示当前接管主界面输入与渲染的全屏模态层。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ModalLayer {
    ToolApprovalFullscreenPreview,
    TranscriptOverlay,
    SessionPreview,
    SessionPicker,
    CopyPicker,
    EntryTree,
    MessageHistory,
}

impl ModalLayer {
    /// 是否需要对高频滚轮事件做分页滚动合并。
    pub(crate) const fn has_page_scroll_burst_coalescing(self) -> bool {
        matches!(
            self,
            Self::SessionPreview
                | Self::SessionPicker
                | Self::CopyPicker
                | Self::EntryTree
                | Self::MessageHistory
        )
    }

    /// Transcript 覆盖层保留 `Ctrl-C` 关闭自身的语义，需先于全局退出确认处理。
    pub(crate) const fn handles_key_before_global_shortcuts(self) -> bool {
        matches!(self, Self::TranscriptOverlay)
    }
}

impl Model {
    /// 返回当前视觉和输入优先级最高的全屏模态层。
    pub(crate) fn top_modal_layer(&self) -> Option<ModalLayer> {
        if self.tool_approval_fullscreen_preview_active() {
            return Some(ModalLayer::ToolApprovalFullscreenPreview);
        }
        if self.transcript_overlay_active() {
            return Some(ModalLayer::TranscriptOverlay);
        }
        if self.session_preview_active() {
            return Some(ModalLayer::SessionPreview);
        }
        if self.session_picker_active() {
            return Some(ModalLayer::SessionPicker);
        }
        if self.copy_picker_active() {
            return Some(ModalLayer::CopyPicker);
        }
        if self.entry_tree_active() {
            return Some(ModalLayer::EntryTree);
        }
        if self.message_history_picker_active() {
            return Some(ModalLayer::MessageHistory);
        }
        None
    }

    /// 全屏模态层与 modal panel 激活时，composer 输入及其附属浮层均不应接管事件。
    pub(crate) fn blocks_composer_input(&self) -> bool {
        self.top_modal_layer().is_some()
            || self.model_panel_active()
            || self.tool_approval_panel_active()
    }

    pub(crate) fn modal_blocks_pointer_passthrough(&self) -> bool {
        self.top_modal_layer().is_some()
    }

    pub(crate) fn modal_has_page_scroll_burst_coalescing(&self) -> bool {
        self.top_modal_layer()
            .is_some_and(ModalLayer::has_page_scroll_burst_coalescing)
    }

    /// 全屏模态层渲染时看不到主文档中的 startup banner 目标。
    pub(crate) fn modal_obscures_startup_banner_entrance_target(&self) -> bool {
        self.top_modal_layer().is_some()
    }

    pub(crate) fn modal_mouse_mode_preference(&self) -> Option<TerminalMouseModePreference> {
        match self.top_modal_layer()? {
            ModalLayer::EntryTree if self.entry_tree_preview_active() => {
                Some(TerminalMouseModePreference::NativeWithAlternateScroll)
            }
            ModalLayer::CopyPicker if self.copy_picker_preview_active() => {
                Some(TerminalMouseModePreference::NativeWithAlternateScroll)
            }
            ModalLayer::EntryTree | ModalLayer::CopyPicker | ModalLayer::MessageHistory => {
                Some(TerminalMouseModePreference::CaptureWithAlternateScroll)
            }
            ModalLayer::ToolApprovalFullscreenPreview
            | ModalLayer::TranscriptOverlay
            | ModalLayer::SessionPreview
            | ModalLayer::SessionPicker => {
                Some(TerminalMouseModePreference::NativeWithAlternateScroll)
            }
        }
    }

    pub(crate) fn close_fullscreen_modal_layers(&mut self) {
        self.close_transcript_overlay();
        self.session_preview = None;
        self.session_picker = None;
        self.copy_picker = None;
        self.entry_tree = None;
        self.message_history_picker = None;
    }
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyEvent};
    use runtime_domain::session::{RuntimeTarget, SessionTreeRowKind};

    use crate::{
        AppEffect, AppEvent, Model, StartupBannerOptions, runner::TerminalMouseModePreference,
        runtime::tool_activity_preview::ToolApprovalPreview, test_helpers::tree_row,
        theme::default_palette, tool_approval_panel::ToolApprovalSource, transcript::Transcript,
    };

    use super::ModalLayer;

    #[test]
    fn top_modal_layer_uses_single_priority_order() {
        let mut model = Model::new(StartupBannerOptions::default());
        model.set_window(80, 12);

        model.open_entry_tree_loading();
        assert_eq!(model.top_modal_layer(), Some(ModalLayer::EntryTree));

        model.open_copy_picker_loading();
        assert_eq!(model.top_modal_layer(), Some(ModalLayer::CopyPicker));

        model.open_session_picker_loading();
        assert_eq!(model.top_modal_layer(), Some(ModalLayer::SessionPicker));

        model.open_session_preview("session-1".to_string(), Transcript::new(default_palette()));
        assert_eq!(model.top_modal_layer(), Some(ModalLayer::SessionPreview));

        model.open_transcript_overlay();
        assert_eq!(model.top_modal_layer(), Some(ModalLayer::TranscriptOverlay));

        open_fullscreen_tool_approval(&mut model);
        assert_eq!(
            model.top_modal_layer(),
            Some(ModalLayer::ToolApprovalFullscreenPreview)
        );
    }

    #[test]
    fn key_dispatch_closes_only_the_top_fullscreen_modal_layer() {
        let mut model = Model::new(StartupBannerOptions::default());
        model.set_window(80, 12);
        model.open_entry_tree_loading();
        model.open_copy_picker_loading();
        model.open_session_picker_loading();
        model.open_session_preview(
            "session-under-approval".to_string(),
            Transcript::new(default_palette()),
        );
        model.open_transcript_overlay();

        assert_eq!(model.top_modal_layer(), Some(ModalLayer::TranscriptOverlay));
        assert_eq!(press_key(&mut model, KeyCode::Esc), None);
        assert_eq!(model.top_modal_layer(), Some(ModalLayer::SessionPreview));
        assert_eq!(press_key(&mut model, KeyCode::Esc), None);
        assert_eq!(model.top_modal_layer(), Some(ModalLayer::SessionPicker));
        assert_eq!(press_key(&mut model, KeyCode::Esc), None);
        assert_eq!(model.top_modal_layer(), Some(ModalLayer::CopyPicker));
        assert_eq!(press_key(&mut model, KeyCode::Esc), None);
        assert_eq!(model.top_modal_layer(), Some(ModalLayer::EntryTree));
        assert_eq!(press_key(&mut model, KeyCode::Esc), None);
        assert_eq!(model.top_modal_layer(), None);
    }

    #[test]
    fn message_history_modal_closes_with_esc_when_top_layer() {
        let mut model = Model::new(StartupBannerOptions::default());
        model.set_window(80, 12);
        model.open_message_history_picker_loading();
        assert_eq!(model.top_modal_layer(), Some(ModalLayer::MessageHistory));
        assert_eq!(press_key(&mut model, KeyCode::Esc), None);
        assert_eq!(model.top_modal_layer(), None);
    }

    #[test]
    fn key_dispatch_routes_top_fullscreen_tool_approval_effect() {
        let mut model = Model::new(StartupBannerOptions::default());
        model.set_window(80, 12);
        model.open_session_preview(
            "session-under-approval".to_string(),
            Transcript::new(default_palette()),
        );
        open_fullscreen_tool_approval(&mut model);

        let effect = press_key(&mut model, KeyCode::Enter);
        assert_eq!(
            effect,
            Some(AppEffect::RespondRuntimePermission {
                target: RuntimeTarget::provider("local", "qwen3"),
                request_id: "permission-write".to_string(),
                option_id: Some("allow-once".to_string()),
            })
        );
        assert_eq!(model.top_modal_layer(), None);
    }

    #[test]
    fn inline_tool_approval_replaces_hidden_fullscreen_modal_input() {
        let mut model = Model::new(StartupBannerOptions::default());
        model.set_window(80, 12);
        model.open_session_picker_loading();
        assert_eq!(model.top_modal_layer(), Some(ModalLayer::SessionPicker));

        open_inline_tool_approval(&mut model);

        assert!(model.tool_approval_panel_active());
        assert_eq!(model.top_modal_layer(), None);
        assert_eq!(
            press_key(&mut model, KeyCode::Enter),
            Some(AppEffect::RespondRuntimePermission {
                target: RuntimeTarget::provider("local", "qwen3"),
                request_id: "permission-write".to_string(),
                option_id: Some("allow-once".to_string()),
            })
        );
    }

    #[test]
    fn fullscreen_modal_blocks_composer_command_panel() {
        let mut model = Model::new(StartupBannerOptions::default());
        press_key(&mut model, KeyCode::Char('/'));
        assert!(model.command_panel_active());

        model.open_session_picker_loading();

        assert!(!model.command_panel_active());
    }

    #[test]
    fn model_panel_blocks_composer_input_without_becoming_fullscreen_modal() {
        let mut model = Model::new(StartupBannerOptions::default());

        model.open_model_panel();

        assert_eq!(model.top_modal_layer(), None);
        assert!(model.blocks_composer_input());
        assert!(!model.modal_blocks_pointer_passthrough());
    }

    #[test]
    fn fullscreen_modal_blocks_composer_input_and_pointer_passthrough() {
        let mut model = Model::new(StartupBannerOptions::default());

        model.open_session_picker_loading();

        assert!(model.blocks_composer_input());
        assert!(model.modal_blocks_pointer_passthrough());
        assert!(model.modal_obscures_startup_banner_entrance_target());
    }

    #[test]
    fn modal_mouse_mode_policy_is_defined_by_top_layer() {
        let mut model = Model::new(StartupBannerOptions::default());

        assert_eq!(model.modal_mouse_mode_preference(), None);

        model.open_copy_picker_loading();
        assert_eq!(model.top_modal_layer(), Some(ModalLayer::CopyPicker));
        assert_eq!(
            model.modal_mouse_mode_preference(),
            Some(TerminalMouseModePreference::CaptureWithAlternateScroll)
        );
        assert!(model.modal_has_page_scroll_burst_coalescing());

        model.open_session_picker_loading();
        assert_eq!(model.top_modal_layer(), Some(ModalLayer::SessionPicker));
        assert_eq!(
            model.modal_mouse_mode_preference(),
            Some(TerminalMouseModePreference::NativeWithAlternateScroll)
        );
        assert!(model.modal_has_page_scroll_burst_coalescing());

        model.open_transcript_overlay();
        assert_eq!(model.top_modal_layer(), Some(ModalLayer::TranscriptOverlay));
        assert_eq!(
            model.modal_mouse_mode_preference(),
            Some(TerminalMouseModePreference::NativeWithAlternateScroll)
        );
        assert!(!model.modal_has_page_scroll_burst_coalescing());
    }

    #[test]
    fn entry_tree_list_mode_uses_capture_with_alternate_scroll() {
        let mut model = Model::new(StartupBannerOptions::default());
        model.open_entry_tree_loading();
        model.apply_entry_tree_payload(runtime_domain::session::SessionTreePayload {
            rows: vec![tree_row(
                "user-1",
                SessionTreeRowKind::User,
                "hello",
                Some("hello".to_string()),
                Some("user-1"),
            )],
            current_row_id: Some("user-1".to_string()),
        });

        assert_eq!(model.top_modal_layer(), Some(ModalLayer::EntryTree));
        assert_eq!(
            model.modal_mouse_mode_preference(),
            Some(TerminalMouseModePreference::CaptureWithAlternateScroll)
        );
        assert!(model.modal_has_page_scroll_burst_coalescing());
    }

    fn open_fullscreen_tool_approval(model: &mut Model) {
        let content = (1..=30)
            .map(|line| format!("line {line}"))
            .collect::<Vec<_>>()
            .join("\n");
        model.open_tool_approval_panel_with_preview(
            ToolApprovalSource::RuntimePermission {
                target: RuntimeTarget::provider("local", "qwen3"),
                request_id: "permission-write".to_string(),
                allow_option_id: Some("allow-once".to_string()),
                allow_always_option_id: None,
                reject_option_id: Some("reject-once".to_string()),
                reject_always_option_id: None,
            },
            "WriteFile: temp.md".to_string(),
            Vec::new(),
            Some(ToolApprovalPreview::create_file(
                "temp.md".to_string(),
                content,
            )),
        );
        assert!(model.tool_approval_fullscreen_preview_active());
    }

    fn open_inline_tool_approval(model: &mut Model) {
        model.open_tool_approval_panel_with_preview(
            ToolApprovalSource::RuntimePermission {
                target: RuntimeTarget::provider("local", "qwen3"),
                request_id: "permission-write".to_string(),
                allow_option_id: Some("allow-once".to_string()),
                allow_always_option_id: None,
                reject_option_id: Some("reject-once".to_string()),
                reject_always_option_id: None,
            },
            "WriteFile: temp.md".to_string(),
            Vec::new(),
            None,
        );
        assert!(model.tool_approval_panel_active());
        assert!(!model.tool_approval_fullscreen_preview_active());
    }

    fn press_key(model: &mut Model, code: KeyCode) -> Option<AppEffect> {
        model.update(AppEvent::Key(KeyEvent::from(code)))
    }
}
