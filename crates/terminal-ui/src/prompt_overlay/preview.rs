use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    layout::Rect,
    style::Modifier,
    text::{Line, Span},
    widgets::{Clear, Paragraph},
};
use runtime_domain::dynamic_environment::DynamicEnvironmentSnapshotKind;
use runtime_domain::prompt_assembly::{
    PromptAssemblyDiscoveredSkill, PromptAssemblyDynamicEnvironmentCandidate,
    PromptAssemblyManagerSource, PromptSourceStatus, ResolvedPromptSource,
};

use crate::{
    Model,
    overlay_input_result::OverlayInputResult,
    render_frame::RenderFrame,
    styled_text::render_line_with_full_width_background,
    theme::{
        approval_rejected_text_style, build_page_rule, muted_text_style, primary_text_style,
        tertiary_text_style,
    },
    transcript::wrap_plain_text,
};

use super::PromptOverlaySelection;

const FOOTER_HINT: &str = "  Esc/Space/p back · ↑/←/h previous page · ↓/→/l next page";
const PROMPT_OVERLAY_PREVIEW_HORIZONTAL_PADDING: usize = 2;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PromptOverlayPreviewState {
    pub(crate) title: String,
    pub(crate) content: String,
    pub(crate) notice: Option<String>,
    pub(crate) scroll_offset: usize,
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
        self.open_prompt_overlay_plain_text_preview(title, &content, None);
    }

    pub(super) fn open_selected_prompt_overlay_preview(&mut self) {
        let Some(selection) = self.selected_prompt_overlay_selection() else {
            return;
        };
        match selection {
            PromptOverlaySelection::ManagedSource(source) => {
                let source = ResolvedPromptSource {
                    reference_id: source.reference_id,
                    kind: source.kind,
                    title: source.title,
                    origin: source.origin,
                    status: PromptSourceStatus::Active {
                        order: source.order,
                    },
                };
                let Some(manager_source) = self.manager_source_for_resolved_source(&source) else {
                    return;
                };
                self.open_prompt_overlay_source_preview(manager_source);
            }
            PromptOverlaySelection::ResolvedSource(source) => {
                let Some(manager_source) = self.manager_source_for_resolved_source(&source) else {
                    return;
                };
                self.open_prompt_overlay_source_preview(manager_source);
            }
            PromptOverlaySelection::ExtraPromptCandidate(candidate) => {
                self.open_prompt_overlay_plain_text_preview(candidate.title, &candidate.body, None);
            }
            PromptOverlaySelection::DiscoveredSkill(skill) => {
                self.open_prompt_overlay_skill_preview(skill);
            }
            PromptOverlaySelection::ToolCandidate(tool) => {
                let body = tool.prompt_guidelines.unwrap_or_default();
                self.open_prompt_overlay_plain_text_preview(
                    tool.label.unwrap_or(tool.name),
                    &body,
                    None,
                );
            }
            PromptOverlaySelection::DynamicEnvironmentCandidate(source) => {
                self.open_prompt_overlay_dynamic_environment_preview(source);
            }
        }
    }

    fn open_prompt_overlay_skill_preview(&mut self, skill: PromptAssemblyDiscoveredSkill) {
        let preview_notice = (!skill.can_select_for_discovery).then(|| {
            "Manual-only skill: `disable-model-invocation: true` keeps this skill out of skill discovery."
                .to_string()
        });
        self.open_prompt_overlay_plain_text_preview(
            skill.title.clone(),
            &skill.body,
            preview_notice,
        );
    }

    fn open_prompt_overlay_dynamic_environment_preview(
        &mut self,
        source: PromptAssemblyDynamicEnvironmentCandidate,
    ) {
        let snapshot_kind = self.prompt_overlay_dynamic_selected_snapshot_kind();
        let title = match snapshot_kind {
            DynamicEnvironmentSnapshotKind::Baseline => {
                format!("{} · Baseline", source.label)
            }
            DynamicEnvironmentSnapshotKind::Changes => {
                format!("{} · Changes", source.label)
            }
        };
        let content = match snapshot_kind {
            DynamicEnvironmentSnapshotKind::Baseline => source.baseline_preview_body,
            DynamicEnvironmentSnapshotKind::Changes => source.changes_preview_body,
        };
        self.open_prompt_overlay_plain_text_preview(title, &content, None);
    }

    pub(crate) fn open_prompt_overlay_assembled_preview(&mut self) {
        let content = self
            .prompt_assembly
            .prelude
            .effective_system_prompt()
            .unwrap_or_default();
        self.open_prompt_overlay_plain_text_preview("Assembled prompt".to_string(), &content, None);
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
            KeyCode::Esc | KeyCode::Char(' ') | KeyCode::Char('p') if key.modifiers.is_empty() => {
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
        let Some(preview_scroll_offset) = self
            .prompt_overlay
            .as_ref()
            .and_then(|state| state.preview.as_ref())
            .map(|preview| preview.scroll_offset)
        else {
            return;
        };
        let Some(wrapped_lines) = self.prompt_overlay_preview_wrapped_lines() else {
            return;
        };
        let preview_notice = self
            .prompt_overlay
            .as_ref()
            .and_then(|state| state.preview.as_ref())
            .and_then(|preview| preview.notice.clone());
        if area.width == 0 || area.height == 0 {
            return;
        }

        frame.render_widget(Clear, area);
        let palette = self.palette;
        let content_height = usize::from(area.height.saturating_sub(2).max(1));
        let text_style = primary_text_style(palette);
        let notice_style = approval_rejected_text_style(palette);
        let max_offset = wrapped_lines.len().saturating_sub(content_height.max(1));
        let scroll_offset = preview_scroll_offset.min(max_offset);
        let (page_number, page_count) =
            crate::transcript_overlay::render::transcript_overlay_page_progress(
                wrapped_lines.len(),
                content_height,
                scroll_offset,
            );

        let content_bottom = area
            .y
            .saturating_add(u16::try_from(content_height).unwrap_or(u16::MAX));
        let mut row = area.y;
        if let Some(notice) = preview_notice {
            render_line_with_full_width_background(
                &Line::from(vec![Span::raw("  "), Span::styled(notice, notice_style)]),
                Rect::new(area.x, row, area.width, 1),
                frame.buffer_mut(),
            );
            row = row.saturating_add(1);
            if row < content_bottom {
                render_line_with_full_width_background(
                    &Line::from(vec![Span::raw("  ")]),
                    Rect::new(area.x, row, area.width, 1),
                    frame.buffer_mut(),
                );
                row = row.saturating_add(1);
            }
        }
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
                    Span::raw("  "),
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
                FOOTER_HINT,
                tertiary_text_style(palette).add_modifier(Modifier::ITALIC),
            )),
            Rect::new(area.x, footer_y, area.width, 1),
        );
    }

    pub(crate) fn move_prompt_overlay_preview_page(&mut self, direction: isize) {
        let page_size = self.prompt_overlay_preview_content_height();
        let line_count = self
            .prompt_overlay_preview_wrapped_lines()
            .map_or(0, |lines| lines.len());
        let Some(preview) = self
            .prompt_overlay
            .as_mut()
            .and_then(|state| state.preview.as_mut())
        else {
            return;
        };
        let max_offset = line_count.saturating_sub(page_size);
        let delta = direction.signum() * isize::try_from(page_size).unwrap_or(0);
        let next = isize::try_from(preview.scroll_offset)
            .unwrap_or(0)
            .saturating_add(delta);
        let max_offset_i = isize::try_from(max_offset).unwrap_or(0);
        preview.scroll_offset = usize::try_from(next.clamp(0, max_offset_i)).unwrap_or(0);
    }

    pub(crate) fn open_prompt_overlay_plain_text_preview(
        &mut self,
        title: String,
        content: &str,
        notice: Option<String>,
    ) {
        let Some(state) = self.prompt_overlay.as_mut() else {
            return;
        };
        state.preview = Some(PromptOverlayPreviewState {
            title,
            content: content.to_string(),
            notice,
            scroll_offset: 0,
        });
    }

    pub(crate) fn sync_prompt_overlay_preview_width(&mut self, width: u16) {
        let line_count = self
            .prompt_overlay
            .as_ref()
            .and_then(|state| state.preview.as_ref())
            .map(|preview| {
                prompt_overlay_preview_total_line_count(
                    preview,
                    prompt_overlay_preview_wrap_width(width),
                )
            })
            .unwrap_or(0);
        let page_size = self.prompt_overlay_preview_content_height();
        let Some(preview) = self
            .prompt_overlay
            .as_mut()
            .and_then(|state| state.preview.as_mut())
        else {
            return;
        };
        let max_offset = line_count.saturating_sub(page_size);
        preview.scroll_offset = preview.scroll_offset.min(max_offset);
    }

    pub(crate) fn prompt_overlay_preview_content_height(&self) -> usize {
        usize::from(self.height.saturating_sub(2).max(1))
    }

    pub(crate) fn prompt_overlay_preview_wrapped_lines(&self) -> Option<Vec<String>> {
        let preview = self.prompt_overlay.as_ref()?.preview.as_ref()?;
        Some(wrap_plain_text(
            &preview.content,
            prompt_overlay_preview_wrap_width(self.width),
            0,
        ))
    }
}

fn prompt_overlay_preview_wrap_width(window_width: u16) -> usize {
    usize::from(window_width)
        .saturating_sub(PROMPT_OVERLAY_PREVIEW_HORIZONTAL_PADDING * 2)
        .max(1)
}

fn prompt_overlay_preview_total_line_count(
    preview: &PromptOverlayPreviewState,
    wrap_width: usize,
) -> usize {
    let body_lines = wrap_plain_text(&preview.content, wrap_width, 0).len();
    if preview.notice.is_some() {
        body_lines.saturating_add(2)
    } else {
        body_lines
    }
}
