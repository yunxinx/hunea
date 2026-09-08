use ratatui::{
    layout::Rect,
    style::Modifier,
    text::{Line, Span},
    widgets::{Clear, Paragraph},
};

use runtime_domain::agent::{
    AgentId, AgentPermissionRequest, AgentPermissionState, AgentPreviewSnapshot,
    AgentProjectionStatus,
};
use runtime_domain::session::RuntimePermissionOption;

use crate::{
    Model,
    agents_panel::{AgentsPanelPermissionChoice, agent_activity_summary_text},
    display_width::display_width,
    render_frame::RenderFrame,
    status_line::truncate_display_width_with_ellipsis,
    styled_text::render_line_with_full_width_background,
    theme::{
        TerminalPalette, build_page_rule, primary_text_style, secondary_text_style,
        subtle_rule_line, tertiary_text_style,
    },
    transcript_overlay::{
        TranscriptOverlayProgressStyle, TranscriptOverlayRenderOptions,
        render_transcript_overlay_view,
    },
};

/// surface 标题行与 permission 区块共用的左右留白。
pub(super) const AGENTS_SURFACE_HORIZONTAL_PADDING: usize = 2;
/// 标题行 title 的保底宽度；低于此值时 elapsed 让位。
pub(super) const AGENTS_SURFACE_TITLE_MIN_WIDTH: usize = 8;

/// permission 区块的纯布局结果：渲染与正文高度/滚动钳制共用。
pub(super) struct AgentsPanelPermissionBlock {
    /// delivery-safe 请求行（已按 wrap 宽度截断为单行）。
    request_line: String,
    /// option 条目（编号在渲染时生成）；空集表示 runtime 未发出任何 option。
    option_labels: Vec<AgentsPanelPermissionOptionLabel>,
    /// 横排（全部 option 一行放得下）还是降级多行。
    is_single_line_options: bool,
}

struct AgentsPanelPermissionOptionLabel {
    label: String,
    is_marked: bool,
}

impl AgentsPanelPermissionBlock {
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

/// 标题行的纯布局结果：状态列、title 与（可选）elapsed 标签。
pub(super) struct AgentsPanelSurfaceHeader {
    pub(super) status: AgentProjectionStatus,
    pub(super) status_label: String,
    pub(super) title: String,
    pub(super) elapsed: Option<String>,
}

impl Model {
    pub(crate) fn render_agents_panel_transcript(
        &mut self,
        frame: &mut RenderFrame<'_>,
        area: Rect,
    ) {
        if area.width == 0 || area.height < 5 {
            // 标题 + 分割线 + 至少 1 行正文 + page rule + footer。
            return;
        }
        frame.render_widget(Clear, area);
        let palette = self.palette;

        let Some(agent_id) = self
            .agents_panel
            .as_ref()
            .and_then(|panel| panel.surface_agent_id())
        else {
            return;
        };
        let width = usize::from(area.width);
        // 标题数据优先用 view snapshot（新鲜）；loading 期回退 overview row。
        let Some(header) = self.agents_panel_surface_header_data(agent_id, width) else {
            return;
        };
        self.render_agents_panel_surface_title(frame, area, &header, palette);

        // permission 区块恒可见（不进滚动区）：先预留其高度，正文只占剩余行。
        let permission_block = self.agents_panel_permission_block(area.width);
        let frame_height = usize::from(area.height.saturating_sub(4).max(1));
        let block_height =
            agents_panel_permission_block_height(permission_block.as_ref(), frame_height);
        let content_height = frame_height.saturating_sub(block_height).max(1);

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
                let content_rect = Rect::new(
                    area.x,
                    area.y + 2,
                    area.width,
                    area.height.saturating_sub(2),
                );
                let overflow = self.agents_panel_surface_has_overflow(content_height);
                let footer_hint = self.agents_panel_surface_footer_hint(area.width, overflow);
                if let Some(panel) = self.agents_panel.as_mut()
                    && let Some(surface) = panel.surface.as_mut()
                {
                    if surface.is_following_bottom {
                        surface.overlay.scroll_offset = crate::transcript::latest_preview_offset(
                            &mut surface.transcript,
                            content_height,
                        );
                    }
                    render_transcript_overlay_view(
                        frame,
                        content_rect,
                        &mut surface.transcript,
                        &mut surface.overlay,
                        TranscriptOverlayRenderOptions {
                            palette,
                            content_height,
                            footer_hint: &footer_hint,
                            progress_style: TranscriptOverlayProgressStyle::Page,
                        },
                    );
                }
                if let Some(block) = permission_block {
                    let rule_y = area.y + area.height - 2;
                    for (offset, block_line) in build_permission_block_lines(&block, palette)
                        .into_iter()
                        .enumerate()
                    {
                        let row =
                            area.y + 2 + u16::try_from(content_height + offset).unwrap_or(u16::MAX);
                        if row >= rule_y {
                            break;
                        }
                        render_line_with_full_width_background(
                            &block_line,
                            Rect::new(area.x, row, area.width, 1),
                            frame.buffer_mut(),
                        );
                    }
                }
            }
            Some(TranscriptSurfaceRecordState::Loading) => {
                self.render_agents_panel_surface_pending_view(
                    frame,
                    area,
                    "  Loading agent transcript...",
                );
            }
            Some(TranscriptSurfaceRecordState::Error(error)) => {
                self.render_agents_panel_surface_pending_view(frame, area, &format!("  {error}"));
            }
            None => {}
        }
    }

    /// 标题行 + 分割线：状态点/文字（状态语义色）+ primary bold 标题 + elapsed。
    fn render_agents_panel_surface_title(
        &self,
        frame: &mut RenderFrame<'_>,
        area: Rect,
        header: &AgentsPanelSurfaceHeader,
        palette: TerminalPalette,
    ) {
        let status_style = crate::agents_panel::agent_status_dot_style(header.status, &palette);
        let mut spans = vec![
            Span::raw(" ".repeat(AGENTS_SURFACE_HORIZONTAL_PADDING)),
            Span::styled(
                crate::agents_panel::agent_status_dot_symbol(header.status, &palette),
                status_style,
            ),
            Span::raw(" "),
            Span::styled(header.status_label.clone(), status_style),
        ];
        if !header.title.is_empty() {
            spans.push(Span::raw(" "));
            spans.push(Span::styled(
                header.title.clone(),
                primary_text_style(palette).bold(),
            ));
        }
        if let Some(elapsed) = &header.elapsed {
            spans.push(Span::raw(" "));
            spans.push(Span::styled(elapsed.clone(), tertiary_text_style(palette)));
        }
        frame.render_widget(
            Paragraph::new(Line::from(spans)),
            Rect::new(area.x, area.y, area.width, 1),
        );
        frame.render_widget(
            Paragraph::new(subtle_rule_line(usize::from(area.width), palette)),
            Rect::new(area.x, area.y + 1, area.width, 1),
        );
    }

    /// snapshot 未就绪时的占位视图：标题 + rule + 单行状态文案 + page rule + footer。
    fn render_agents_panel_surface_pending_view(
        &self,
        frame: &mut RenderFrame<'_>,
        area: Rect,
        message: &str,
    ) {
        let palette = self.palette;
        frame.render_widget(
            Paragraph::new(Line::styled(message, tertiary_text_style(palette))),
            Rect::new(area.x, area.y + 2, area.width, 1),
        );
        frame.render_widget(
            Paragraph::new(build_page_rule(area.width, 1, 1, palette)),
            Rect::new(area.x, area.y + area.height - 2, area.width, 1),
        );
        frame.render_widget(
            Paragraph::new(Line::styled(
                self.agents_panel_surface_footer_hint(area.width, false),
                tertiary_text_style(palette).add_modifier(Modifier::ITALIC),
            )),
            Rect::new(area.x, area.y + area.height - 1, area.width, 1),
        );
    }

    /// permission 区块布局：数据只读 observation snapshot（FIFO head），
    /// surface choice 提供 selection/锁定态；head 不存在即无区块。
    pub(super) fn agents_panel_permission_block(
        &self,
        width: u16,
    ) -> Option<AgentsPanelPermissionBlock> {
        let panel = self.agents_panel.as_ref()?;
        let surface = panel.surface.as_ref()?;
        let record = panel.agent_view_for_agent(surface.agent_id)?;
        let snapshot = record.snapshot.as_ref()?;
        let head = snapshot.preview.permission.as_ref()?;
        Some(build_agents_panel_permission_block(
            head,
            &snapshot.preview,
            &surface.permission_choice,
            width,
        ))
    }

    /// permission 区块按输入侧窗口高度预留的高度（正文至少保留 1 行；区块超高时截尾）。
    pub(super) fn agents_panel_permission_block_height(&self) -> usize {
        let frame_height = usize::from(self.height.saturating_sub(4).max(1));
        agents_panel_permission_block_height(
            self.agents_panel_permission_block(self.width).as_ref(),
            frame_height,
        )
    }

    /// surface 的 permission 档位（None / Pending / Submitted）。
    fn agents_panel_surface_permission_tier(&self) -> Option<AgentsPanelSurfacePermissionTier> {
        let panel = self.agents_panel.as_ref()?;
        let surface = panel.surface.as_ref()?;
        let head = panel
            .agent_view_for_agent(surface.agent_id)?
            .snapshot
            .as_ref()?
            .preview
            .permission
            .as_ref()?;
        let choice_matches_head = match &surface.permission_choice {
            AgentsPanelPermissionChoice::Selecting { request_id, .. }
            | AgentsPanelPermissionChoice::Submitted { request_id, .. } => {
                *request_id == head.target.request_id
            }
            AgentsPanelPermissionChoice::None => false,
        };
        match head.state {
            // 本地 Submitted 锁定（runtime 尚未投影）也按已提交态提示——不可重复提交。
            AgentPermissionState::Pending => {
                let locally_submitted = matches!(
                    &surface.permission_choice,
                    AgentsPanelPermissionChoice::Submitted { .. }
                ) && choice_matches_head;
                if locally_submitted {
                    Some(AgentsPanelSurfacePermissionTier::Submitted)
                } else {
                    Some(AgentsPanelSurfacePermissionTier::Pending)
                }
            }
            AgentPermissionState::Submitted => Some(AgentsPanelSurfacePermissionTier::Submitted),
        }
    }

    /// transcript 是否超出正文区（footer 的 page 档位输入）。
    fn agents_panel_surface_has_overflow(&mut self, content_height: usize) -> bool {
        let Some(panel) = self.agents_panel.as_mut() else {
            return false;
        };
        let Some(surface) = panel.surface.as_mut() else {
            return false;
        };
        let line_count = surface
            .transcript
            .progressive_item_metrics_index()
            .line_count;
        line_count > content_height
    }

    /// footer hint 按 back / overflow / permission 三档组合最小 hint：
    /// 普通无 overflow 仅 back；overflow 加 page；pending 加 choose/confirm
    /// （Submitted 显示已提交态提示，无可执行动作）。
    fn agents_panel_surface_footer_hint(&self, width: u16, has_overflow: bool) -> String {
        let back = if width < 90 {
            "Esc back"
        } else {
            "Esc back to agents overview"
        };
        let mut hint = format!("  {back} · Space back");
        if has_overflow {
            hint.push_str(" · ←/→/h/l page");
        }
        match self.agents_panel_surface_permission_tier() {
            Some(AgentsPanelSurfacePermissionTier::Pending) => {
                hint.push_str(" · ↑/↓ choose · Enter confirm");
            }
            Some(AgentsPanelSurfacePermissionTier::Submitted) => {
                hint.push_str(" · submitted");
            }
            None => {}
        }
        hint
    }

    /// 标题行数据：view snapshot 优先，loading 期回退 overview row，
    /// 让标题在快照到达前就有 status/title 可读。
    fn agents_panel_surface_header_data(
        &self,
        agent_id: AgentId,
        width: usize,
    ) -> Option<AgentsPanelSurfaceHeader> {
        let panel = self.agents_panel.as_ref()?;
        let record = panel.agent_view_for_agent(agent_id)?;
        if let Some(preview) = record.snapshot.as_ref().map(|snapshot| &snapshot.preview) {
            return Some(agents_panel_surface_header(
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
        Some(agents_panel_surface_header(
            row.status,
            row.title.as_str(),
            row.elapsed_ms,
            width,
        ))
    }
}

enum TranscriptSurfaceRecordState {
    Loading,
    Error(String),
    Ready,
}

enum AgentsPanelSurfacePermissionTier {
    Pending,
    Submitted,
}

/// permission 区块占用正文的高度（正文至少保留 1 行；区块超高时截尾）。
fn agents_panel_permission_block_height(
    block: Option<&AgentsPanelPermissionBlock>,
    frame_height: usize,
) -> usize {
    block
        .map_or(0, AgentsPanelPermissionBlock::line_count)
        .min(frame_height.saturating_sub(1))
}

/// 标题行单行布局：状态（固定列宽）+ title（弹性截断）+ elapsed（空间不足先隐藏）。
pub(super) fn agents_panel_surface_header(
    status: AgentProjectionStatus,
    title: &str,
    elapsed_ms: Option<u64>,
    width: usize,
) -> AgentsPanelSurfaceHeader {
    use crate::agents_panel::{
        AGENTS_ELAPSED_COLUMN_WIDTH, agent_status_label, format_agent_elapsed_ms,
        pad_agents_status_column,
    };
    use crate::relative_age::left_pad_display_width;

    let status_budget = width.saturating_sub(AGENTS_SURFACE_HORIZONTAL_PADDING);
    let status_label = pad_agents_status_column(agent_status_label(status), status_budget);
    let status_width = display_width(&status_label);
    let elapsed_label = elapsed_ms.map(|ms| {
        left_pad_display_width(&format_agent_elapsed_ms(ms), AGENTS_ELAPSED_COLUMN_WIDTH)
    });
    let title_budget = width
        .saturating_sub(
            AGENTS_SURFACE_HORIZONTAL_PADDING
                + 1
                + status_width
                + 1
                + AGENTS_SURFACE_HORIZONTAL_PADDING,
        )
        .max(1);
    // 宽度不足时先隐藏 elapsed，再对 title 做 display-width 安全截断。
    let elapsed_width = elapsed_label.as_deref().map_or(0, display_width);
    let show_elapsed = elapsed_label.is_some()
        && title_budget > elapsed_width + 1 + AGENTS_SURFACE_TITLE_MIN_WIDTH;
    let title_width = if show_elapsed {
        title_budget - elapsed_width - 1
    } else {
        title_budget
    };
    AgentsPanelSurfaceHeader {
        status,
        status_label,
        title: truncate_display_width_with_ellipsis(title, title_width),
        elapsed: show_elapsed.then(|| elapsed_label.unwrap_or_default()),
    }
}

/// 构造 permission 区块的纯布局：请求行 + runtime-issued options 全集直渲染。
///
/// 不做 main 流的 kind→option 推导；label 用 runtime-issued `option.name`
/// （空 name 用短 fallback，保持横排几何稳定）。
fn build_agents_panel_permission_block(
    head: &AgentPermissionRequest,
    preview: &AgentPreviewSnapshot,
    choice: &AgentsPanelPermissionChoice,
    width: u16,
) -> AgentsPanelPermissionBlock {
    let wrap_width = agents_panel_permission_wrap_width(width);
    let request_line = truncate_display_width_with_ellipsis(
        &permission_request_display_title(head, preview),
        wrap_width,
    );

    // marker 归属：Selecting 标记当前 selection；Submitted（本地已知 option）固定
    // 标记已提交 option；Submitted（投影未知 option）与 Pending/choice 脱节时无 marker。
    let marked_index = match (head.state, choice) {
        (
            AgentPermissionState::Pending,
            AgentsPanelPermissionChoice::Selecting {
                request_id,
                selected,
            },
        ) if *request_id == head.target.request_id => Some(*selected),
        (
            AgentPermissionState::Submitted,
            AgentsPanelPermissionChoice::Submitted {
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

    let option_labels: Vec<AgentsPanelPermissionOptionLabel> = head
        .request
        .options
        .iter()
        .enumerate()
        .map(|(index, option)| AgentsPanelPermissionOptionLabel {
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

    AgentsPanelPermissionBlock {
        request_line,
        option_labels,
        is_single_line_options,
    }
}

fn agents_panel_permission_wrap_width(window_width: u16) -> usize {
    usize::from(window_width)
        .saturating_sub(AGENTS_SURFACE_HORIZONTAL_PADDING * 2)
        .max(1)
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
fn permission_option_entry_text(entry: &AgentsPanelPermissionOptionLabel, number: usize) -> String {
    let marker = if entry.is_marked { "➜ " } else { "  " };
    format!("{marker}{}. {}", number + 1, entry.label)
}

/// permission 区块的渲染行：请求行（secondary）+ option 行
/// （横排一行 / 降级多行；marked BOLD primary，其余 secondary）。
fn build_permission_block_lines(
    block: &AgentsPanelPermissionBlock,
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
        let mut spans = vec![Span::raw(" ".repeat(AGENTS_SURFACE_HORIZONTAL_PADDING))];
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
        let mut spans = vec![Span::raw(" ".repeat(AGENTS_SURFACE_HORIZONTAL_PADDING))];
        spans.push(Span::styled(
            permission_option_entry_text(entry, index),
            option_style(entry.is_marked),
        ));
        lines.push(Line::from(spans));
    }
    lines
}
