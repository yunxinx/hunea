use crate::{transcript::Transcript, transcript_overlay::TranscriptOverlayState};

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
}
