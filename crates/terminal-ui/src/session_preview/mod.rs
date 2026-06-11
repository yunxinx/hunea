use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use runtime_domain::session::SessionPreviewPayload;

use crate::{
    AppEffect, Model,
    render_frame::RenderFrame,
    transcript::Transcript,
    transcript_overlay::{
        TranscriptOverlayProgressStyle, TranscriptOverlayRenderOptions, TranscriptOverlayState,
        render_transcript_overlay_view,
    },
};

const COMPACT_FOOTER_HINT: &str = "  Esc/Space back · Enter resume · ↑/←/h prev · ↓/→/l next";
const FULL_FOOTER_HINT: &str =
    "  Esc/Space back · Enter resume · ↑/←/h previous page · ↓/→/l next page";

/// `SessionPreviewState` 保存 resume picker 的完整 session 预览状态。
#[derive(Debug, Clone)]
pub(crate) struct SessionPreviewState {
    session_id: String,
    pub(crate) transcript: Transcript,
    overlay: TranscriptOverlayState,
    is_following_bottom: bool,
}

impl PartialEq for SessionPreviewState {
    fn eq(&self, other: &Self) -> bool {
        self.session_id == other.session_id
            && self.transcript == other.transcript
            && self.overlay == other.overlay
    }
}

impl Eq for SessionPreviewState {}

impl Model {
    pub(crate) fn session_preview_active(&self) -> bool {
        self.session_preview.is_some()
    }

    pub(crate) fn open_session_preview(&mut self, session_id: String, transcript: Transcript) {
        let content_height = self.transcript_overlay_content_height();
        let mut preview = SessionPreviewState {
            session_id,
            transcript,
            overlay: TranscriptOverlayState::new(),
            is_following_bottom: true,
        };
        preview.overlay.scroll_offset =
            latest_session_preview_offset(&mut preview.transcript, content_height);
        self.session_preview = Some(preview);
    }

    pub(crate) fn apply_session_preview_payload(&mut self, payload: SessionPreviewPayload) {
        let transcript = self.transcript_from_replay_items(payload.transcript);
        self.open_session_preview(payload.session_id, transcript);
    }

    pub(crate) fn close_session_preview(&mut self) {
        self.session_preview = None;
    }

    pub(crate) fn handle_session_preview_key(
        &mut self,
        key: KeyEvent,
    ) -> Option<Option<AppEffect>> {
        if !self.session_preview_active() {
            return None;
        }

        match key.code {
            KeyCode::Enter if key.modifiers.is_empty() => {
                let session_id = self
                    .session_preview
                    .as_ref()
                    .map(|preview| preview.session_id.clone())?;
                self.close_session_preview();
                self.session_picker = None;
                Some(Some(AppEffect::ResumeSession { session_id }))
            }
            KeyCode::Esc | KeyCode::Char(' ') if key.modifiers.is_empty() => {
                self.close_session_preview();
                Some(None)
            }
            KeyCode::Left | KeyCode::Up | KeyCode::Char('h') if key.modifiers.is_empty() => {
                self.move_session_preview_page(-1);
                Some(None)
            }
            KeyCode::Right | KeyCode::Down | KeyCode::Char('l') if key.modifiers.is_empty() => {
                self.move_session_preview_page(1);
                Some(None)
            }
            _ => Some(None),
        }
    }

    pub(crate) fn render_session_preview(&mut self, frame: &mut RenderFrame<'_>, area: Rect) {
        let palette = self.palette;
        let content_height = area.height.saturating_sub(2).max(1) as usize;
        let Some(preview) = self.session_preview.as_mut() else {
            return;
        };
        if preview.is_following_bottom {
            preview.overlay.scroll_offset =
                latest_session_preview_offset(&mut preview.transcript, content_height);
        }
        render_transcript_overlay_view(
            frame,
            area,
            &mut preview.transcript,
            &mut preview.overlay,
            TranscriptOverlayRenderOptions {
                palette,
                content_height,
                footer_hint: session_preview_footer_hint(area.width),
                progress_style: TranscriptOverlayProgressStyle::Page,
            },
        );
    }

    pub(crate) fn move_session_preview_page(&mut self, direction: isize) {
        let content_height = self.transcript_overlay_content_height();
        let Some(preview) = self.session_preview.as_mut() else {
            return;
        };
        preview.overlay.scroll_offset = session_preview_page_offset(
            &mut preview.transcript,
            content_height,
            preview.overlay.scroll_offset,
            direction,
        );
        preview.is_following_bottom = false;
    }
}

fn session_preview_footer_hint(width: u16) -> &'static str {
    if width < 76 {
        COMPACT_FOOTER_HINT
    } else {
        FULL_FOOTER_HINT
    }
}

fn latest_session_preview_offset(transcript: &mut Transcript, content_height: usize) -> usize {
    let content_height = content_height.max(1);
    let mut index = transcript.progressive_item_metrics_index();
    if index.line_count == 0 {
        return 0;
    }

    let mut offset = index.line_count.saturating_sub(content_height);
    let mut remaining_exactization_passes = index.metrics.len().saturating_add(1);
    while remaining_exactization_passes > 0 {
        let effective_total = index.line_count;
        if effective_total == 0 {
            return 0;
        }

        let next_offset = effective_total.saturating_sub(content_height);
        let visible_line_count = content_height.min(effective_total.saturating_sub(next_offset));
        let window = transcript.materialize_line_window(next_offset, visible_line_count);
        let exact_offset = window.index.line_count.saturating_sub(content_height);
        if exact_offset == offset {
            return exact_offset;
        }

        offset = exact_offset;
        index = window.index;
        remaining_exactization_passes -= 1;
    }

    offset
}

fn session_preview_page_offset(
    transcript: &mut Transcript,
    content_height: usize,
    current_offset: usize,
    direction: isize,
) -> usize {
    let content_height = content_height.max(1);
    let latest_offset = latest_session_preview_offset(transcript, content_height);
    let index = transcript.progressive_item_metrics_index();
    let total_lines = index.line_count;
    if total_lines == 0 {
        return 0;
    }

    let page_count = total_lines.saturating_sub(1) / content_height + 1;
    let current_page = if current_offset >= latest_offset {
        page_count
    } else {
        current_offset / content_height + 1
    };
    let next_page = if direction.is_negative() {
        current_page.saturating_sub(1).max(1)
    } else {
        current_page.saturating_add(1).min(page_count)
    };

    if next_page >= page_count {
        latest_offset
    } else {
        (next_page - 1) * content_height
    }
}
