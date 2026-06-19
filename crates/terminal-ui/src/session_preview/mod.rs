use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use runtime_domain::session::SessionPreviewPayload;

use crate::{
    AppEffect, Model,
    overlay_input_result::OverlayInputResult,
    render_frame::RenderFrame,
    transcript::{
        Transcript, latest_preview_offset as latest_session_preview_offset,
        preview_page_offset as session_preview_page_offset,
    },
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
        self.close_composer_attached_ui();
    }

    pub(crate) fn apply_session_preview_payload(&mut self, payload: SessionPreviewPayload) {
        let transcript = self.transcript_from_replay_items(payload.transcript);
        self.open_session_preview(payload.session_id, transcript);
    }

    pub(crate) fn close_session_preview(&mut self) {
        self.session_preview = None;
    }

    pub(crate) fn handle_session_preview_key(&mut self, key: KeyEvent) -> OverlayInputResult {
        if !self.session_preview_active() {
            return OverlayInputResult::Ignored;
        }

        match key.code {
            KeyCode::Enter if key.modifiers.is_empty() => {
                let Some(session_id) = self
                    .session_preview
                    .as_ref()
                    .map(|preview| preview.session_id.clone())
                else {
                    return OverlayInputResult::Ignored;
                };
                self.close_session_preview();
                self.session_picker = None;
                OverlayInputResult::Effect(AppEffect::ResumeSession { session_id })
            }
            KeyCode::Esc | KeyCode::Char(' ') if key.modifiers.is_empty() => {
                self.close_session_preview();
                OverlayInputResult::Handled
            }
            KeyCode::Left | KeyCode::Up | KeyCode::Char('h') if key.modifiers.is_empty() => {
                self.move_session_preview_page(-1);
                OverlayInputResult::Handled
            }
            KeyCode::Right | KeyCode::Down | KeyCode::Char('l') if key.modifiers.is_empty() => {
                self.move_session_preview_page(1);
                OverlayInputResult::Handled
            }
            _ => OverlayInputResult::Handled, // 模态覆盖层吞掉未绑定输入，防止落入 composer
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
