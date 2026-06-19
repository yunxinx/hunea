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
}

impl ModalLayer {
    /// 是否需要对高频滚轮事件做分页滚动合并。
    pub(crate) const fn has_page_scroll_burst_coalescing(self) -> bool {
        matches!(
            self,
            Self::SessionPreview | Self::SessionPicker | Self::CopyPicker | Self::EntryTree
        )
    }

    /// 全屏模态层激活时，指针事件不能穿透到底层 transcript/composer。
    pub(crate) const fn blocks_pointer_passthrough(self) -> bool {
        true
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
        None
    }

    /// 粘贴会修改 composer；全屏模态层或 model panel 激活时应吞掉粘贴。
    pub(crate) fn blocks_main_paste(&self) -> bool {
        self.top_modal_layer().is_some() || self.model_panel_active()
    }

    pub(crate) fn modal_blocks_pointer_passthrough(&self) -> bool {
        self.top_modal_layer()
            .is_some_and(ModalLayer::blocks_pointer_passthrough)
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
            ModalLayer::EntryTree | ModalLayer::CopyPicker => {
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
}

#[cfg(test)]
mod tests {
    use runtime_domain::session::SessionTreeRowKind;

    use crate::{
        Model, StartupBannerOptions, runner::TerminalMouseModePreference, test_helpers::tree_row,
        theme::default_palette, transcript::Transcript,
    };

    use super::ModalLayer;

    #[test]
    fn top_modal_layer_uses_single_priority_order() {
        let mut model = Model::new(StartupBannerOptions::default());

        model.open_session_picker_loading();
        assert_eq!(model.top_modal_layer(), Some(ModalLayer::SessionPicker));

        model.open_session_preview("session-1".to_string(), Transcript::new(default_palette()));
        assert_eq!(model.top_modal_layer(), Some(ModalLayer::SessionPreview));

        model.open_transcript_overlay();
        assert_eq!(model.top_modal_layer(), Some(ModalLayer::TranscriptOverlay));
    }

    #[test]
    fn model_panel_blocks_paste_without_becoming_fullscreen_modal() {
        let mut model = Model::new(StartupBannerOptions::default());

        model.open_model_panel();

        assert_eq!(model.top_modal_layer(), None);
        assert!(model.blocks_main_paste());
        assert!(!model.modal_blocks_pointer_passthrough());
    }

    #[test]
    fn fullscreen_modal_blocks_main_paste_and_pointer_passthrough() {
        let mut model = Model::new(StartupBannerOptions::default());

        model.open_session_picker_loading();

        assert!(model.blocks_main_paste());
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
}
