use crossterm::event::{KeyCode, KeyEvent};
use runtime_domain::agent::AgentTranscriptItem;
use runtime_domain::session::{
    RuntimeToolActivity, RuntimeToolActivityContent, RuntimeToolActivityStatus, RuntimeToolKind,
};

use crate::{
    Model,
    overlay_input_result::OverlayInputResult,
    sender::Sender,
    tool_result::ToolActivityRenderMode,
    transcript::{
        Transcript, latest_preview_offset as transcript_bottom_offset,
        preview_page_offset as transcript_page_offset,
    },
};

use super::AgentsPanelSurface;

impl Model {
    pub(crate) fn agents_panel_transcript_active(&self) -> bool {
        self.agents_panel.as_ref().is_some_and(|panel| {
            matches!(panel.surface, Some(AgentsPanelSurface::Transcript { .. }))
        })
    }

    pub(crate) fn move_agents_panel_transcript_page(&mut self, direction: isize) {
        let content_height = self.transcript_overlay_content_height();
        if let Some(panel) = self.agents_panel.as_mut()
            && let Some(AgentsPanelSurface::Transcript {
                transcript,
                overlay,
                is_following_bottom,
                ..
            }) = panel.surface.as_mut()
        {
            overlay.scroll_offset = transcript_page_offset(
                transcript,
                content_height,
                overlay.scroll_offset,
                direction,
            );
            *is_following_bottom = false;
        }
    }

    /// record 快照更新后刷新 transcript surface（仅当 surface 绑定同一 agent）。
    pub(super) fn sync_agents_panel_transcript_surface(
        &mut self,
        agent_id: runtime_domain::agent::AgentId,
    ) {
        let surface_bound = self
            .agents_panel
            .as_ref()
            .is_some_and(|panel| panel.surface_agent_id() == Some(agent_id))
            && self.agents_panel_transcript_active();
        if !surface_bound {
            return;
        }
        let items = self
            .agents_panel
            .as_ref()
            .and_then(|panel| panel.agent_view_for_agent(agent_id))
            .and_then(|record| {
                record
                    .snapshot
                    .as_ref()
                    .map(|snapshot| snapshot.transcript.items.clone())
            });
        let Some(items) = items else {
            return;
        };
        let transcript = self.transcript_from_agent_items(&items);
        let content_height = self.transcript_overlay_content_height();
        if let Some(panel) = self.agents_panel.as_mut()
            && let Some(AgentsPanelSurface::Transcript {
                transcript: surface_transcript,
                overlay,
                is_following_bottom,
                ..
            }) = panel.surface.as_mut()
        {
            **surface_transcript = transcript;
            if *is_following_bottom {
                overlay.scroll_offset =
                    transcript_bottom_offset(surface_transcript, content_height);
            }
        }
    }

    /// 窗口宽度变化时同步 transcript surface 的换行缓存。
    pub(crate) fn sync_agents_panel_surface_width(&mut self, width: u16) {
        if let Some(panel) = self.agents_panel.as_mut()
            && let Some(AgentsPanelSurface::Transcript { transcript, .. }) = panel.surface.as_mut()
        {
            transcript.set_width(width);
        }
    }

    /// 主题变化时同步 transcript surface 的调色板。
    pub(crate) fn sync_agents_panel_surface_palette(
        &mut self,
        palette: crate::theme::TerminalPalette,
    ) {
        if let Some(panel) = self.agents_panel.as_mut()
            && let Some(AgentsPanelSurface::Transcript { transcript, .. }) = panel.surface.as_mut()
        {
            transcript.set_palette(palette);
        }
    }

    /// transcript surface 的 Esc 只返回 overview；其余未绑定键吞掉防落 composer。
    pub(super) fn handle_agents_panel_transcript_key(
        &mut self,
        key: KeyEvent,
    ) -> OverlayInputResult {
        match key.code {
            KeyCode::Esc if key.modifiers.is_empty() => {
                self.close_agents_panel_surface();
                OverlayInputResult::Handled
            }
            KeyCode::Left | KeyCode::Up | KeyCode::Char('h') if key.modifiers.is_empty() => {
                self.move_agents_panel_transcript_page(-1);
                OverlayInputResult::Handled
            }
            KeyCode::Right | KeyCode::Down | KeyCode::Char('l') if key.modifiers.is_empty() => {
                self.move_agents_panel_transcript_page(1);
                OverlayInputResult::Handled
            }
            _ => OverlayInputResult::Handled,
        }
    }

    /// 由 delivery-safe 的 typed items 构建 transcript surface 视图。
    ///
    /// 不展示 streaming partial、control-only instructions、provider prompt 或 raw
    /// payload：这些内容在 runtime 侧就不会进入 `AgentTranscriptItem`。
    pub(super) fn transcript_from_agent_items(&self, items: &[AgentTranscriptItem]) -> Transcript {
        let mut transcript = Transcript::new(self.palette, self.working_dir.clone());
        transcript.set_gap(1);
        transcript.set_diff_display(self.diff_display);
        // 与 Ctrl+T transcript overlay 一致：完整视图展示 tool activity 详情，
        // Compact 模式会折叠 delivery-safe 的 tool content。
        transcript.set_tool_activity_render_mode(ToolActivityRenderMode::Detailed);
        if self.has_window {
            transcript.set_width(self.width);
        }
        for (index, item) in items.iter().enumerate() {
            match item {
                AgentTranscriptItem::User { content } => {
                    transcript.append_message_with_style_mode(
                        Sender::User,
                        content.clone(),
                        self.style_mode,
                    );
                }
                AgentTranscriptItem::Assistant { content } => {
                    transcript.append_message_with_style_mode(
                        Sender::Assistant,
                        content.clone(),
                        self.style_mode,
                    );
                }
                AgentTranscriptItem::Tool { title, content } => {
                    // Tool item 已是 delivery-safe 投影；用 runtime tool activity 形态渲染
                    // 以复用 transcript overlay 的标题 + 正文视图。
                    transcript.append_runtime_tool_activity(RuntimeToolActivity {
                        activity_id: format!("agent-view-tool-{index}"),
                        title: title.clone(),
                        kind: RuntimeToolKind::Other,
                        status: RuntimeToolActivityStatus::Completed,
                        content: vec![RuntimeToolActivityContent::Text(content.clone())],
                        locations: Vec::new(),
                        raw_input: None,
                        raw_output: None,
                    });
                }
            }
        }
        transcript
    }
}
