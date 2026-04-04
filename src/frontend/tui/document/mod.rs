mod anchor_match;
mod append;
mod cache;
mod layout;
mod line_access;
mod manual_scroll;
mod slot_frame;
mod slot_viewport;
mod sync;

pub(super) use self::cache::{
    DocumentAnchorRegion, DocumentLayout, DocumentLayoutCache, DocumentLayoutKey,
    DocumentLayoutLine, DocumentLineAnchor, DocumentTranscriptCache, DocumentTranscriptItemLines,
    DocumentTranscriptKey, DocumentTranscriptSnapshot, DocumentViewport, DocumentViewportAnchor,
    DocumentViewportCache, DocumentViewportKey, ManualDocumentScrollRestoreTarget,
};
pub(super) use self::cache::{
    DocumentLayoutCache as LayoutCache, DocumentTranscriptCache as TranscriptCache,
    DocumentViewportCache as ViewportCache,
};
pub(super) use self::manual_scroll::ManualScrollRestoreState as RestoreState;
pub(crate) use self::slot_viewport::{
    bottom_follow_viewport_line_indices, offset_viewport_line_indices,
};

#[cfg(test)]
mod tests;
