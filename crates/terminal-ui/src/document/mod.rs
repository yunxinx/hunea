mod anchor_match;
mod cache;
mod layout;
mod line_access;
mod manual_scroll;
mod selection_semantic_cache;
mod slot_frame;
mod slot_viewport;
mod sync;
mod tail;
mod viewport_state;

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
pub(super) use self::tail::{
    DocumentStableTailLayoutCache as StableTailLayoutCache,
    DocumentTailLayoutCache as TailLayoutCache,
};
pub(super) use self::tail::{DocumentTailLayout, offset_slot_frame};
pub(super) use self::viewport_state::ViewportState;
#[cfg(test)]
pub(super) use self::viewport_state::{
    TranscriptSemanticPosition, ViewAnchor, document_viewport_anchor_at_line,
};

#[cfg(test)]
mod tests;
