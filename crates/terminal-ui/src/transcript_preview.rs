use crate::{
    theme::TerminalPalette,
    transcript::{Transcript, latest_preview_offset},
    transcript_overlay::TranscriptOverlayState,
};

/// Transcript 覆盖层预览的共享状态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TranscriptPreviewState {
    pub(crate) transcript: Transcript,
    pub(crate) overlay: TranscriptOverlayState,
    pub(crate) is_following_bottom: bool,
}

impl TranscriptPreviewState {
    pub(crate) fn following_bottom(transcript: Transcript) -> Self {
        Self {
            transcript,
            overlay: TranscriptOverlayState::new(),
            is_following_bottom: true,
        }
    }

    /// 刷新预览 transcript 宽度，并保持“跟随底部”语义。
    pub(crate) fn set_width(&mut self, width: u16, content_height: usize) {
        self.transcript.set_width(width);
        self.sync_follow_bottom(content_height);
    }

    /// 刷新预览 transcript 配色，并失效依赖 palette 的渲染缓存。
    pub(crate) fn set_palette(&mut self, palette: TerminalPalette, content_height: usize) {
        self.transcript.set_palette(palette);
        self.sync_follow_bottom(content_height);
    }

    /// 当预览处于底部跟随模式时，按当前 transcript metrics 重算 offset。
    pub(crate) fn sync_follow_bottom(&mut self, content_height: usize) {
        if self.is_following_bottom {
            self.overlay.scroll_offset =
                latest_preview_offset(&mut self.transcript, content_height);
        }
    }
}
