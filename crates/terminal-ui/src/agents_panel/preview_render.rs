use ratatui::{
    layout::Rect,
    style::Modifier,
    text::{Line, Span},
    widgets::{Clear, Paragraph},
};

use runtime_domain::agent::{AgentPermissionRequest, AgentPermissionState, AgentPreviewSnapshot};
use runtime_domain::session::RuntimePermissionOption;

use crate::{
    Model,
    agents_panel::{
        AgentsPanelPreviewPermissionChoice, AgentsPanelSurface, agent_activity_summary_text,
    },
    display_width::display_width,
    render_frame::RenderFrame,
    status_line::truncate_display_width_with_ellipsis,
    styled_text::render_line_with_full_width_background,
    theme::{
        TerminalPalette, build_page_rule, muted_text_style, primary_text_style,
        secondary_text_style, tertiary_text_style,
    },
};

use super::preview::{
    AGENTS_PREVIEW_HORIZONTAL_PADDING, agents_panel_preview_header, agents_panel_preview_wrap_width,
};

/// permission 区块的纯布局结果：渲染与 body 高度/滚动钳制共用。
pub(super) struct AgentsPanelPreviewPermissionBlock {
    /// delivery-safe 请求行（已按 wrap_width 截断为单行）。
    request_line: String,
    /// option 条目（编号在渲染时生成）；空集表示 runtime 未发出任何 option。
    option_labels: Vec<AgentsPanelPreviewPermissionOptionLabel>,
    /// 横排（全部 option 一行放得下）还是降级多行。
    is_single_line_options: bool,
}

struct AgentsPanelPreviewPermissionOptionLabel {
    label: String,
    is_marked: bool,
}

/// preview footer 的状态输入：按 back / overflow / permission 三档组合最小 hint。
pub(super) enum AgentsPanelPreviewFooterPermission {
    Pending,
    Submitted,
}

pub(super) struct AgentsPanelPreviewFooterState {
    pub(super) has_overflow: bool,
    pub(super) permission: Option<AgentsPanelPreviewFooterPermission>,
}

impl AgentsPanelPreviewPermissionBlock {
    /// 区块占用的正文行数：请求行 1 行 + option 横排 1 行或多行。
    pub(super) fn line_count(&self) -> usize {
        1 + self.option_line_count()
    }

    fn option_line_count(&self) -> usize {
        if self.option_labels.is_empty() {
            0
        } else if self.is_single_line_options {
            1
        } else {
            self.option_labels.len()
        }
    }
}

/// permission 区块占用正文的高度（正文至少保留 1 行；区块超高时截尾）。
pub(super) fn agents_panel_preview_permission_block_height(
    block: Option<&AgentsPanelPreviewPermissionBlock>,
    content_height: usize,
) -> usize {
    block
        .map_or(0, AgentsPanelPreviewPermissionBlock::line_count)
        .min(content_height.saturating_sub(1))
}

impl Model {
    pub(crate) fn render_agents_panel_preview(&mut self, frame: &mut RenderFrame<'_>, area: Rect) {
        if area.width == 0 || area.height < 4 {
            // header + 至少 1 行正文 + rule + footer。
            return;
        }
        frame.render_widget(Clear, area);
        let palette = self.palette;

        let Some((agent_id, scroll_offset)) = self.agents_panel_preview_surface_position() else {
            return;
        };

        // header 数据优先用 view snapshot（新鲜）；loading 期回退 overview row。
        let header = self.agents_panel_preview_header_data(agent_id, usize::from(area.width));
        let Some(header) = header else {
            return;
        };
        let mut header_spans = vec![
            Span::raw(" ".repeat(AGENTS_PREVIEW_HORIZONTAL_PADDING)),
            Span::styled(header.status, secondary_text_style(palette)),
        ];
        if !header.title.is_empty() {
            header_spans.push(Span::raw(" "));
            header_spans.push(Span::styled(
                header.title,
                primary_text_style(palette).bold(),
            ));
        }
        if let Some(elapsed) = header.elapsed {
            header_spans.push(Span::raw(" "));
            header_spans.push(Span::styled(elapsed, tertiary_text_style(palette)));
        }
        frame.render_widget(
            Paragraph::new(Line::from(header_spans)),
            Rect::new(area.x, area.y, area.width, 1),
        );

        let Some(display_lines) = self.agents_panel_preview_display_lines() else {
            return;
        };
        // permission 区块恒可见（不进滚动区）：先预留其高度，正文只占剩余行。
        let permission_block = self.agents_panel_preview_permission_block(area.width);
        let content_height = usize::from(area.height.saturating_sub(3).max(1));
        let block_height =
            agents_panel_preview_permission_block_height(permission_block.as_ref(), content_height);
        let body_height = content_height - block_height;
        let page_size = body_height.max(1);
        let max_offset = display_lines.len().saturating_sub(page_size);
        let scroll_offset = scroll_offset.min(max_offset);
        let (page_number, page_count) =
            crate::transcript_overlay::render::transcript_overlay_page_progress(
                display_lines.len(),
                body_height,
                scroll_offset,
            );

        let content_bottom = area
            .y
            .saturating_add(1)
            .saturating_add(u16::try_from(content_height).unwrap_or(u16::MAX));
        let body_bottom = area
            .y
            .saturating_add(1)
            .saturating_add(u16::try_from(body_height).unwrap_or(u16::MAX));
        let mut row = area.y.saturating_add(1);
        let text_style = primary_text_style(palette);
        for line in display_lines.iter().skip(scroll_offset).take(body_height) {
            if row >= body_bottom {
                break;
            }
            render_line_with_full_width_background(
                &Line::from(Span::styled(line.as_str(), text_style)),
                Rect::new(area.x, row, area.width, 1),
                frame.buffer_mut(),
            );
            row = row.saturating_add(1);
        }

        let fill_style = muted_text_style(palette);
        while row < body_bottom {
            frame.render_widget(
                Paragraph::new(Line::styled("~", fill_style)),
                Rect::new(area.x, row, area.width, 1),
            );
            row = row.saturating_add(1);
        }

        if let Some(block) = permission_block {
            let mut block_row = body_bottom;
            for block_line in build_permission_block_lines(&block, palette) {
                if block_row >= content_bottom {
                    break;
                }
                render_line_with_full_width_background(
                    &block_line,
                    Rect::new(area.x, block_row, area.width, 1),
                    frame.buffer_mut(),
                );
                block_row = block_row.saturating_add(1);
            }
        }

        frame.render_widget(
            Paragraph::new(build_page_rule(
                area.width,
                page_number,
                page_count,
                palette,
            )),
            Rect::new(area.x, area.y + area.height - 2, area.width, 1),
        );
        let footer_state = AgentsPanelPreviewFooterState {
            has_overflow: page_count > 1,
            permission: self.agents_panel_preview_footer_permission(),
        };
        frame.render_widget(
            Paragraph::new(Line::styled(
                agents_panel_preview_footer_hint(area.width, &footer_state),
                tertiary_text_style(palette).add_modifier(Modifier::ITALIC),
            )),
            Rect::new(area.x, area.y + area.height - 1, area.width, 1),
        );
    }

    fn agents_panel_preview_surface_position(
        &self,
    ) -> Option<(runtime_domain::agent::AgentId, usize)> {
        let panel = self.agents_panel.as_ref()?;
        match panel.surface.as_ref()? {
            AgentsPanelSurface::Preview {
                agent_id,
                scroll_offset,
                ..
            } => Some((*agent_id, *scroll_offset)),
            _ => None,
        }
    }

    /// permission 区块布局：数据只读 observation snapshot（FIFO head），
    /// surface choice 提供 selection/锁定态；head 不存在即无区块。
    pub(super) fn agents_panel_preview_permission_block(
        &self,
        width: u16,
    ) -> Option<AgentsPanelPreviewPermissionBlock> {
        let panel = self.agents_panel.as_ref()?;
        let AgentsPanelSurface::Preview {
            agent_id,
            permission_choice,
            ..
        } = panel.surface.as_ref()?
        else {
            return None;
        };
        let record = panel.agent_view_for_agent(*agent_id)?;
        let snapshot = record.snapshot.as_ref()?;
        let head = snapshot.preview.permission.as_ref()?;
        Some(build_agents_panel_preview_permission_block(
            head,
            &snapshot.preview,
            permission_choice,
            width,
        ))
    }

    /// footer 的 permission 档位（None / Pending / Submitted）。
    fn agents_panel_preview_footer_permission(&self) -> Option<AgentsPanelPreviewFooterPermission> {
        let panel = self.agents_panel.as_ref()?;
        let AgentsPanelSurface::Preview {
            agent_id,
            permission_choice,
            ..
        } = panel.surface.as_ref()?
        else {
            return None;
        };
        let head = panel
            .agent_view_for_agent(*agent_id)?
            .snapshot
            .as_ref()?
            .preview
            .permission
            .as_ref()?;
        let choice_matches_head = match permission_choice {
            AgentsPanelPreviewPermissionChoice::Selecting { request_id, .. }
            | AgentsPanelPreviewPermissionChoice::Submitted { request_id, .. } => {
                *request_id == head.target.request_id
            }
            AgentsPanelPreviewPermissionChoice::None => false,
        };
        match head.state {
            // 本地 Submitted 锁定（runtime 尚未投影）也按已提交态提示——不可重复提交。
            AgentPermissionState::Pending => {
                let locally_submitted = matches!(
                    permission_choice,
                    AgentsPanelPreviewPermissionChoice::Submitted { .. }
                ) && choice_matches_head;
                if locally_submitted {
                    Some(AgentsPanelPreviewFooterPermission::Submitted)
                } else {
                    Some(AgentsPanelPreviewFooterPermission::Pending)
                }
            }
            AgentPermissionState::Submitted => Some(AgentsPanelPreviewFooterPermission::Submitted),
        }
    }

    /// preview header 数据：view snapshot 优先，loading 期回退 overview row，
    /// 让 header 在快照到达前就有 status/title 可读。
    fn agents_panel_preview_header_data(
        &self,
        agent_id: runtime_domain::agent::AgentId,
        width: usize,
    ) -> Option<super::preview::AgentsPanelPreviewHeader> {
        let panel = self.agents_panel.as_ref()?;
        let record = panel.agent_view_for_agent(agent_id)?;
        if let Some(preview) = record.snapshot.as_ref().map(|snapshot| &snapshot.preview) {
            return Some(agents_panel_preview_header(
                preview.status,
                preview.title.as_str(),
                preview.elapsed_ms,
                width,
            ));
        }
        let row = panel
            .list
            .rows()
            .iter()
            .find(|row| row.agent_id == agent_id)?;
        Some(agents_panel_preview_header(
            row.status,
            row.title.as_str(),
            row.elapsed_ms,
            width,
        ))
    }
}

/// 构造 permission 区块的纯布局：请求行 + runtime-issued options 全集直渲染。
///
/// 不做 main 流的 kind→option 推导；label 用 runtime-issued `option.name`
/// （空 name 用短 fallback，保持横排几何稳定）。
fn build_agents_panel_preview_permission_block(
    head: &AgentPermissionRequest,
    preview: &AgentPreviewSnapshot,
    choice: &AgentsPanelPreviewPermissionChoice,
    width: u16,
) -> AgentsPanelPreviewPermissionBlock {
    let wrap_width = agents_panel_preview_wrap_width(width);
    let request_line = truncate_display_width_with_ellipsis(
        &permission_request_display_title(head, preview),
        wrap_width,
    );

    // marker 归属：Selecting 标记当前 selection；Submitted（本地已知 option）固定
    // 标记已提交 option；Submitted（投影未知 option）与 Pending/choice 脱节时无 marker。
    let marked_index = match (head.state, choice) {
        (
            AgentPermissionState::Pending,
            AgentsPanelPreviewPermissionChoice::Selecting {
                request_id,
                selected,
            },
        ) if *request_id == head.target.request_id => Some(*selected),
        (
            AgentPermissionState::Submitted,
            AgentsPanelPreviewPermissionChoice::Submitted {
                request_id,
                option_id: Some(option_id),
            },
        ) if *request_id == head.target.request_id => head
            .request
            .options
            .iter()
            .position(|option| option.option_id == *option_id),
        _ => None,
    };

    let option_labels: Vec<AgentsPanelPreviewPermissionOptionLabel> = head
        .request
        .options
        .iter()
        .enumerate()
        .map(|(index, option)| AgentsPanelPreviewPermissionOptionLabel {
            label: permission_option_display_label(option, index),
            is_marked: Some(index) == marked_index,
        })
        .collect();

    // 横排预算：全部 entry 以两空格间隔拼成一行后不超过 wrap_width 才横排，
    // 否则稳定降级为多行（每 option 一行，对齐 tool approval 竖排约定）。
    let is_single_line_options = !option_labels.is_empty() && {
        let joined_width: usize = option_labels
            .iter()
            .enumerate()
            .map(|(index, entry)| display_width(&permission_option_entry_text(entry, index)))
            .sum::<usize>()
            .saturating_add(2 * option_labels.len().saturating_sub(1));
        joined_width <= wrap_width
    };

    AgentsPanelPreviewPermissionBlock {
        request_line,
        option_labels,
        is_single_line_options,
    }
}

/// delivery-safe 请求行文案：runtime-issued title 优先，
/// 缺失时回退 `WaitingPermission` activity summary；不解析 raw_input/raw_output。
fn permission_request_display_title(
    head: &AgentPermissionRequest,
    preview: &AgentPreviewSnapshot,
) -> String {
    let title = head
        .request
        .title
        .as_deref()
        .map(str::trim)
        .filter(|title| !title.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| match &preview.latest_activity {
            runtime_domain::agent::AgentActivitySummary::WaitingPermission { .. } => {
                agent_activity_summary_text(&preview.latest_activity)
            }
            _ => "waiting for approval".to_string(),
        });
    // title 理论上是单行 runtime 文本；防御性只取首行，保证区块几何稳定。
    title
        .lines()
        .next()
        .unwrap_or("waiting for approval")
        .to_string()
}

/// option 的显示 label：runtime-issued name 优先；空 name 用短 fallback
/// （不复制 main 流的长 fallback 文案——横排需要短标签）。
fn permission_option_display_label(option: &RuntimePermissionOption, index: usize) -> String {
    let name = option.name.trim();
    if name.is_empty() {
        format!("Option {}", index + 1)
    } else {
        name.to_string()
    }
}

/// 单个 option entry 的行内文本：marker（`➜ ` / 两空格）+ 编号 + label。
fn permission_option_entry_text(
    entry: &AgentsPanelPreviewPermissionOptionLabel,
    number: usize,
) -> String {
    let marker = if entry.is_marked { "➜ " } else { "  " };
    format!("{marker}{}. {}", number + 1, entry.label)
}

/// permission 区块的渲染行：请求行（secondary）+ option 行
/// （横排一行 / 降级多行；marked BOLD primary，其余 secondary）。
fn build_permission_block_lines(
    block: &AgentsPanelPreviewPermissionBlock,
    palette: TerminalPalette,
) -> Vec<Line<'static>> {
    let mut lines = vec![Line::from(Span::styled(
        format!("  {}", block.request_line),
        secondary_text_style(palette).bold(),
    ))];

    if block.option_labels.is_empty() {
        return lines;
    }

    let option_style = |is_marked: bool| {
        if is_marked {
            primary_text_style(palette).bold()
        } else {
            secondary_text_style(palette)
        }
    };

    if block.is_single_line_options {
        // 紧凑横排：全部 option 拼成一行，两空格间隔。
        let mut spans = vec![Span::raw(" ".repeat(AGENTS_PREVIEW_HORIZONTAL_PADDING))];
        for (index, entry) in block.option_labels.iter().enumerate() {
            if index > 0 {
                spans.push(Span::raw("  "));
            }
            spans.push(Span::styled(
                permission_option_entry_text(entry, index),
                option_style(entry.is_marked),
            ));
        }
        lines.push(Line::from(spans));
        return lines;
    }

    // 降级多行：每 option 一行，marker/编号/样式约定与 main 审批面板一致。
    for (index, entry) in block.option_labels.iter().enumerate() {
        let mut spans = vec![Span::raw(" ".repeat(AGENTS_PREVIEW_HORIZONTAL_PADDING))];
        spans.push(Span::styled(
            permission_option_entry_text(entry, index),
            option_style(entry.is_marked),
        ));
        lines.push(Line::from(spans));
    }
    lines
}

/// footer hint 按 back / overflow / permission 三档组合最小 hint：
/// 普通无 overflow 仅 back；overflow 加 page；pending 加 choose/confirm
/// （Submitted 显示已提交态提示，无可执行动作）。
fn agents_panel_preview_footer_hint(width: u16, state: &AgentsPanelPreviewFooterState) -> String {
    let back = if width < 90 {
        "Esc back"
    } else {
        "Esc back to agents overview"
    };
    let mut hint = format!("  {back} · Space back");
    if state.has_overflow {
        hint.push_str(" · ←/→/h/l page");
    }
    match state.permission {
        Some(AgentsPanelPreviewFooterPermission::Pending) => {
            hint.push_str(" · ↑/↓ choose · Enter confirm");
        }
        Some(AgentsPanelPreviewFooterPermission::Submitted) => {
            hint.push_str(" · submitted");
        }
        None => {}
    }
    hint
}
