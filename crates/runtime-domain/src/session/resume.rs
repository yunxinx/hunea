use crate::model_catalog::ModelSelection;

use super::transcript_replay::TranscriptReplayItem;

/// `SessionResumePayload` 是 runtime 恢复 session 后返回给 TUI 的完整可见状态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionResumePayload {
    pub session_id: String,
    pub transcript: Vec<TranscriptReplayItem>,
    pub restored_model: Option<ModelSelection>,
}

/// `SessionPreviewPayload` 是 resume picker 预览 session 所需的完整可见 transcript。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionPreviewPayload {
    pub session_id: String,
    pub transcript: Vec<TranscriptReplayItem>,
}
