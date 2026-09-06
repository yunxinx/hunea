use ratatui::{
    layout::Rect,
    text::Line,
    widgets::{Clear, Paragraph},
};

use crate::{
    Model,
    agents_panel::AgentsPanelSurface,
    render_frame::RenderFrame,
    theme::{build_page_rule, tertiary_text_style},
    transcript_overlay::{
        TranscriptOverlayProgressStyle, TranscriptOverlayRenderOptions,
        render_transcript_overlay_view,
    },
};

impl Model {
    pub(crate) fn render_agents_panel_transcript(
        &mut self,
        frame: &mut RenderFrame<'_>,
        area: Rect,
    ) {
        if area.width == 0 || area.height < 4 {
            return;
        }
        frame.render_widget(Clear, area);
        let palette = self.palette;

        let surface_agent_id = self
            .agents_panel
            .as_ref()
            .and_then(|panel| panel.surface.as_ref())
            .and_then(|surface| match surface {
                AgentsPanelSurface::Transcript { agent_id, .. } => Some(*agent_id),
                _ => None,
            });
        let Some(agent_id) = surface_agent_id else {
            return;
        };
        let record_state = self
            .agents_panel
            .as_ref()
            .and_then(|panel| panel.agent_view_for_agent(agent_id))
            .map(|record| {
                if let Some(error) = record.error.as_deref() {
                    TranscriptSurfaceRecordState::Error(error.to_string())
                } else if record.snapshot.is_none() {
                    TranscriptSurfaceRecordState::Loading
                } else {
                    TranscriptSurfaceRecordState::Ready
                }
            });

        match record_state {
            Some(TranscriptSurfaceRecordState::Ready) => {
                let content_height = usize::from(area.height.saturating_sub(2).max(1));
                if let Some(panel) = self.agents_panel.as_mut()
                    && let Some(AgentsPanelSurface::Transcript {
                        transcript,
                        overlay,
                        is_following_bottom,
                        ..
                    }) = panel.surface.as_mut()
                {
                    if *is_following_bottom {
                        overlay.scroll_offset =
                            crate::transcript::latest_preview_offset(transcript, content_height);
                    }
                    render_transcript_overlay_view(
                        frame,
                        area,
                        transcript,
                        overlay,
                        TranscriptOverlayRenderOptions {
                            palette,
                            content_height,
                            footer_hint: agents_panel_transcript_footer_hint(area.width),
                            progress_style: TranscriptOverlayProgressStyle::Page,
                        },
                    );
                }
            }
            Some(TranscriptSurfaceRecordState::Loading) => {
                self.render_agents_panel_transcript_pending_view(
                    frame,
                    area,
                    "  Loading agent transcript...",
                );
            }
            Some(TranscriptSurfaceRecordState::Error(error)) => {
                self.render_agents_panel_transcript_pending_view(
                    frame,
                    area,
                    &format!("  {error}"),
                );
            }
            None => {}
        }
    }

    /// snapshot 未就绪时的占位视图：单行状态文案 + page rule + footer。
    fn render_agents_panel_transcript_pending_view(
        &mut self,
        frame: &mut RenderFrame<'_>,
        area: Rect,
        message: &str,
    ) {
        let palette = self.palette;
        frame.render_widget(
            Paragraph::new(Line::styled(message, tertiary_text_style(palette))),
            Rect::new(area.x, area.y, area.width, 1),
        );
        frame.render_widget(
            Paragraph::new(build_page_rule(area.width, 1, 1, palette)),
            Rect::new(area.x, area.y + area.height - 2, area.width, 1),
        );
        frame.render_widget(
            Paragraph::new(Line::styled(
                agents_panel_transcript_footer_hint(area.width),
                tertiary_text_style(palette).add_modifier(ratatui::style::Modifier::ITALIC),
            )),
            Rect::new(area.x, area.y + area.height - 1, area.width, 1),
        );
    }
}

enum TranscriptSurfaceRecordState {
    Loading,
    Error(String),
    Ready,
}

fn agents_panel_transcript_footer_hint(width: u16) -> &'static str {
    if width < 90 {
        "  Esc back · ←/→/h/l page"
    } else {
        "  Esc back to agents overview · ↑/←/h previous page · ↓/→/l next page"
    }
}
