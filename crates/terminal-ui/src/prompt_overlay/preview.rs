use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use runtime_domain::prompt_assembly::PromptAssemblyManagerSource;

use crate::{
    Model,
    markdown_display::markdown_display_content,
    message::assistant_message_content_width,
    overlay_input_result::OverlayInputResult,
    render_frame::RenderFrame,
    transcript::Transcript,
    transcript_overlay::{
        TranscriptOverlayProgressStyle, TranscriptOverlayRenderOptions, TranscriptOverlayState,
        render_transcript_overlay_view,
    },
};

const FOOTER_HINT: &str = "  Esc/Space back · ↑/←/h previous page · ↓/→/l next page";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PromptOverlayPreviewState {
    pub(crate) title: String,
    pub(crate) transcript: Transcript,
    pub(crate) overlay: TranscriptOverlayState,
}

impl Model {
    pub(crate) fn prompt_overlay_preview_active(&self) -> bool {
        self.prompt_overlay
            .as_ref()
            .and_then(|state| state.preview.as_ref())
            .is_some()
    }

    pub(crate) fn open_prompt_overlay_source_preview(
        &mut self,
        source: PromptAssemblyManagerSource,
    ) {
        let title = source.title.clone();
        let content = source.body.unwrap_or_default();
        self.open_prompt_overlay_markdown_preview(title, &content);
    }

    pub(crate) fn open_prompt_overlay_assembled_preview(&mut self) {
        let content = self
            .prompt_assembly
            .prelude
            .effective_system_prompt()
            .unwrap_or_default();
        self.open_prompt_overlay_markdown_preview("Assembled prompt".to_string(), &content);
    }

    pub(crate) fn close_prompt_overlay_preview(&mut self) {
        let Some(state) = self.prompt_overlay.as_mut() else {
            return;
        };
        state.preview = None;
    }

    pub(crate) fn handle_prompt_overlay_preview_key(
        &mut self,
        key: KeyEvent,
    ) -> OverlayInputResult {
        if !self.prompt_overlay_preview_active() {
            return OverlayInputResult::Ignored;
        }

        match key.code {
            KeyCode::Esc | KeyCode::Char(' ') if key.modifiers.is_empty() => {
                self.close_prompt_overlay_preview();
                OverlayInputResult::Handled
            }
            KeyCode::Left | KeyCode::Up | KeyCode::Char('h') if key.modifiers.is_empty() => {
                self.move_prompt_overlay_preview_page(-1);
                OverlayInputResult::Handled
            }
            KeyCode::Right | KeyCode::Down | KeyCode::Char('l') if key.modifiers.is_empty() => {
                self.move_prompt_overlay_preview_page(1);
                OverlayInputResult::Handled
            }
            _ => OverlayInputResult::Handled,
        }
    }

    pub(crate) fn render_prompt_overlay_preview(
        &mut self,
        frame: &mut RenderFrame<'_>,
        area: Rect,
    ) {
        let palette = self.palette;
        let content_height = usize::from(area.height.saturating_sub(2).max(1));
        let Some(preview) = self
            .prompt_overlay
            .as_mut()
            .and_then(|state| state.preview.as_mut())
        else {
            return;
        };
        render_transcript_overlay_view(
            frame,
            area,
            &mut preview.transcript,
            &mut preview.overlay,
            TranscriptOverlayRenderOptions {
                palette,
                content_height,
                footer_hint: FOOTER_HINT,
                progress_style: TranscriptOverlayProgressStyle::Page,
            },
        );
    }

    pub(crate) fn move_prompt_overlay_preview_page(&mut self, direction: isize) {
        let content_height = self.transcript_overlay_content_height();
        let Some(preview) = self
            .prompt_overlay
            .as_mut()
            .and_then(|state| state.preview.as_mut())
        else {
            return;
        };
        preview.overlay.scroll_offset = crate::transcript::preview_page_offset(
            &mut preview.transcript,
            content_height,
            preview.overlay.scroll_offset,
            direction,
        );
    }

    pub(crate) fn open_prompt_overlay_markdown_preview(&mut self, title: String, content: &str) {
        let mut transcript = Transcript::new(self.palette);
        transcript.set_width(
            u16::try_from(assistant_message_content_width(self.width)).unwrap_or(u16::MAX),
        );
        transcript.append_message_with_style_mode(
            crate::Sender::Assistant,
            markdown_display_content(content),
            self.style_mode,
        );

        let Some(state) = self.prompt_overlay.as_mut() else {
            return;
        };
        state.preview = Some(PromptOverlayPreviewState {
            title,
            transcript,
            overlay: TranscriptOverlayState::new(),
        });
    }
}
