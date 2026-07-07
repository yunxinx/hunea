use ratatui::{
    layout::Rect,
    style::Modifier,
    text::{Line, Span},
    widgets::{Clear, Paragraph},
};
use runtime_domain::session::TranscriptReplayItem;

use crate::{
    render_frame::RenderFrame,
    styled_text::render_line_with_full_width_background,
    theme::{
        TerminalPalette, build_page_rule, muted_text_style, primary_text_style, tertiary_text_style,
    },
    transcript::{Transcript, preview_page_offset, wrap_plain_text},
    transcript_overlay::{
        TranscriptOverlayProgressStyle, TranscriptOverlayRenderOptions,
        render::{render_transcript_overlay_view, transcript_overlay_page_progress},
    },
    transcript_preview::TranscriptPreviewState,
};

const PLAIN_TEXT_PREVIEW_HORIZONTAL_PADDING: usize = 2;
const PLAIN_TEXT_PREVIEW_LEFT_PADDING: &str = "  ";

/// `PlainTextPreviewState` 保存纯文本消息预览的内容与分页滚动位置。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PlainTextPreviewState {
    content: String,
    pub(crate) scroll_offset: usize,
}

impl PlainTextPreviewState {
    pub(crate) fn new(content: String) -> Self {
        Self {
            content,
            scroll_offset: 0,
        }
    }

    pub(crate) fn move_page(&mut self, window_width: u16, content_height: usize, direction: isize) {
        let line_count = self.wrapped_lines(window_width).len();
        let max_offset = line_count.saturating_sub(content_height);
        let delta = direction.signum() * isize::try_from(content_height).unwrap_or(0);
        let next = isize::try_from(self.scroll_offset)
            .unwrap_or(0)
            .saturating_add(delta);
        let max_offset_i = isize::try_from(max_offset).unwrap_or(0);
        self.scroll_offset = usize::try_from(next.clamp(0, max_offset_i)).unwrap_or(0);
    }

    pub(crate) fn sync_width(&mut self, window_width: u16, content_height: usize) {
        let wrapped_line_count = self.wrapped_lines(window_width).len();
        let max_offset = wrapped_line_count.saturating_sub(content_height);
        self.scroll_offset = self.scroll_offset.min(max_offset);
    }

    pub(crate) fn wrapped_lines(&self, window_width: u16) -> Vec<String> {
        wrap_plain_text(
            &self.content,
            plain_text_preview_wrap_width(window_width),
            0,
        )
    }
}

/// `MessagePreviewMode` 描述 message preview 的两种有效显示模式。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MessagePreviewMode {
    PlainText(PlainTextPreviewState),
    Transcript(Box<TranscriptPreviewState>),
}

impl MessagePreviewMode {
    pub(crate) fn plain_text(content: String) -> Self {
        Self::PlainText(PlainTextPreviewState::new(content))
    }

    pub(crate) fn transcript(mut transcript: Transcript, content_height: usize) -> Self {
        transcript.set_reasoning_render_mode(crate::transcript::ReasoningRenderMode::Detailed);
        let mut preview = TranscriptPreviewState::following_bottom(transcript);
        preview.sync_follow_bottom(content_height);
        Self::Transcript(Box::new(preview))
    }

    pub(crate) fn move_page(&mut self, window_width: u16, content_height: usize, direction: isize) {
        match self {
            Self::PlainText(preview) => preview.move_page(window_width, content_height, direction),
            Self::Transcript(preview) => {
                preview.overlay.scroll_offset = preview_page_offset(
                    &mut preview.transcript,
                    content_height,
                    preview.overlay.scroll_offset,
                    direction,
                );
                preview.is_following_bottom = false;
            }
        }
    }

    pub(crate) fn sync_follow_bottom(&mut self, content_height: usize) {
        if let Self::Transcript(preview) = self {
            preview.sync_follow_bottom(content_height);
        }
    }

    pub(crate) fn sync_width(&mut self, width: u16, content_height: usize) {
        match self {
            Self::PlainText(preview) => preview.sync_width(width, content_height),
            Self::Transcript(preview) => preview.set_width(width, content_height),
        }
    }

    pub(crate) fn sync_palette(&mut self, palette: TerminalPalette, content_height: usize) {
        if let Self::Transcript(preview) = self {
            preview.set_palette(palette, content_height);
        }
    }

    pub(crate) fn render(
        &mut self,
        frame: &mut RenderFrame<'_>,
        area: Rect,
        palette: TerminalPalette,
        footer_hint: &str,
    ) {
        match self {
            Self::PlainText(preview) => {
                render_plain_text_preview(frame, area, preview, palette, footer_hint)
            }
            Self::Transcript(preview) => {
                let content_height = usize::from(area.height.saturating_sub(2).max(1));
                render_transcript_overlay_view(
                    frame,
                    area,
                    &mut preview.transcript,
                    &mut preview.overlay,
                    TranscriptOverlayRenderOptions {
                        palette,
                        content_height,
                        footer_hint,
                        progress_style: TranscriptOverlayProgressStyle::Page,
                    },
                );
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn as_transcript_mut(&mut self) -> Option<&mut TranscriptPreviewState> {
        match self {
            Self::Transcript(preview) => Some(preview.as_mut()),
            Self::PlainText(_) => None,
        }
    }

    #[cfg(test)]
    pub(crate) fn as_transcript(&self) -> Option<&TranscriptPreviewState> {
        match self {
            Self::Transcript(preview) => Some(preview.as_ref()),
            Self::PlainText(_) => None,
        }
    }
}

/// `preview_body_text` 为用户消息预览挑选最接近 transcript 可见正文的文本。
pub(crate) fn preview_body_text(
    fallback_content: &str,
    replay_items: &[TranscriptReplayItem],
) -> String {
    replay_items
        .iter()
        .map(TranscriptReplayItem::content_text)
        .find(|text| !text.trim().is_empty())
        .unwrap_or(fallback_content)
        .to_string()
}

pub(crate) fn render_plain_text_preview(
    frame: &mut RenderFrame<'_>,
    area: Rect,
    state: &PlainTextPreviewState,
    palette: TerminalPalette,
    footer_hint: &str,
) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    let wrapped_lines = state.wrapped_lines(area.width);
    frame.render_widget(Clear, area);
    let content_height = usize::from(area.height.saturating_sub(2).max(1));
    let text_style = primary_text_style(palette);
    let max_offset = wrapped_lines.len().saturating_sub(content_height);
    let scroll_offset = state.scroll_offset.min(max_offset);
    let (page_number, page_count) =
        transcript_overlay_page_progress(wrapped_lines.len(), content_height, scroll_offset);

    let content_bottom = area
        .y
        .saturating_add(u16::try_from(content_height).unwrap_or(u16::MAX));
    let mut row = area.y;
    for line in wrapped_lines
        .iter()
        .skip(scroll_offset)
        .take(content_height)
    {
        if row >= content_bottom {
            break;
        }
        render_line_with_full_width_background(
            &Line::from(vec![
                Span::raw(PLAIN_TEXT_PREVIEW_LEFT_PADDING),
                Span::styled(line.as_str(), text_style),
            ]),
            Rect::new(area.x, row, area.width, 1),
            frame.buffer_mut(),
        );
        row = row.saturating_add(1);
    }

    let fill_style = muted_text_style(palette);
    while row < content_bottom {
        frame.render_widget(
            Paragraph::new(Line::styled("~", fill_style)),
            Rect::new(area.x, row, area.width, 1),
        );
        row = row.saturating_add(1);
    }

    if area.height >= 2 {
        let rule_y = area.y + area.height - 2;
        frame.render_widget(
            Paragraph::new(build_page_rule(
                area.width,
                page_number,
                page_count,
                palette,
            )),
            Rect::new(area.x, rule_y, area.width, 1),
        );
    }

    let footer_y = area.y + area.height - 1;
    frame.render_widget(
        Paragraph::new(Line::styled(
            footer_hint.to_string(),
            tertiary_text_style(palette).add_modifier(Modifier::ITALIC),
        )),
        Rect::new(area.x, footer_y, area.width, 1),
    );
}

fn plain_text_preview_wrap_width(window_width: u16) -> usize {
    usize::from(window_width)
        .saturating_sub(PLAIN_TEXT_PREVIEW_HORIZONTAL_PADDING * 2)
        .max(1)
}
