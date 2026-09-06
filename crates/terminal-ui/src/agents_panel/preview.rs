use crossterm::event::{KeyCode, KeyEvent};
use runtime_domain::agent::{
    AgentId, AgentPermissionRequest, AgentPermissionState, AgentPermissionTarget,
};

use crate::{
    AppEffect, Model, agents_panel::agent_activity_summary_text,
    overlay_input_result::OverlayInputResult, relative_age::left_pad_display_width,
    status_line::truncate_display_width_with_ellipsis, transcript::wrap_plain_text,
};

use super::{
    AGENTS_ELAPSED_COLUMN_WIDTH, AgentsPanelPreviewPermissionChoice, AgentsPanelSurface,
    agent_status_label, format_agent_elapsed_ms,
};

/// quick preview 无 committed answer 时的中性 empty state 首行。
pub(super) const AGENTS_PREVIEW_EMPTY_ANSWER_TEXT: &str = "No committed answer yet";
/// quick preview 左右留白（与 message history preview 一致）。
pub(super) const AGENTS_PREVIEW_HORIZONTAL_PADDING: usize = 2;

/// Enter 提交的一次性输入快照：读取与写回两阶段间传递。
struct PreviewPermissionSubmission {
    request_id: String,
    option_id: String,
    target: AgentPermissionTarget,
}

impl Model {
    pub(crate) fn agents_panel_preview_active(&self) -> bool {
        self.agents_panel
            .as_ref()
            .is_some_and(|panel| matches!(panel.surface, Some(AgentsPanelSurface::Preview { .. })))
    }

    /// preview 正文区高度：header 1 + page rule 1 + footer 1 之外的部分。
    pub(crate) fn agents_panel_preview_content_height(&self) -> usize {
        usize::from(self.height.saturating_sub(3).max(1))
    }

    pub(crate) fn move_agents_panel_preview_page(&mut self, direction: isize) {
        // permission 区块不进滚动区：翻页步长按扣除区块高度后的正文区计算。
        let content_height = self.agents_panel_preview_content_height();
        let permission_block = self.agents_panel_preview_permission_block(self.width);
        let block_height = super::preview_render::agents_panel_preview_permission_block_height(
            permission_block.as_ref(),
            content_height,
        );
        let page_size = (content_height - block_height).max(1);
        let line_count = self
            .agents_panel_preview_body_lines()
            .map_or(0, |lines| lines.len());
        if let Some(panel) = self.agents_panel.as_mut()
            && let Some(AgentsPanelSurface::Preview { scroll_offset, .. }) = panel.surface.as_mut()
        {
            let max_offset = line_count.saturating_sub(page_size);
            let delta = direction.signum() * isize::try_from(page_size).unwrap_or(0);
            let next = isize::try_from(*scroll_offset)
                .unwrap_or(0)
                .saturating_add(delta);
            let max_offset_isize = isize::try_from(max_offset).unwrap_or(0);
            *scroll_offset = usize::try_from(next.clamp(0, max_offset_isize)).unwrap_or(0);
        }
    }

    /// preview 正文行：committed answer 的按词换行，或中性 fallback 两行。
    fn agents_panel_preview_body_lines(&self) -> Option<Vec<String>> {
        let panel = self.agents_panel.as_ref()?;
        let AgentsPanelSurface::Preview { agent_id, .. } = panel.surface.as_ref()? else {
            return None;
        };
        let record = panel.agent_view_for_agent(*agent_id)?;
        let preview = record.snapshot.as_ref().map(|snapshot| &snapshot.preview)?;
        let wrap_width = agents_panel_preview_wrap_width(self.width);
        Some(match preview.latest_committed_answer.as_deref() {
            // 与 fallback 行同样携带左缩进，正文块对齐（wrap 宽度已预留该缩进）。
            Some(answer) => wrap_plain_text(answer, wrap_width, 0)
                .into_iter()
                .map(|line| format!("  {line}"))
                .collect(),
            None => vec![
                format!("  {AGENTS_PREVIEW_EMPTY_ANSWER_TEXT}"),
                format!(
                    "  Latest activity: {}",
                    agent_activity_summary_text(&preview.latest_activity)
                ),
            ],
        })
    }

    /// preview 正文行（渲染/滚动共用），含 loading/error 分支的中性文案。
    pub(super) fn agents_panel_preview_display_lines(&self) -> Option<Vec<String>> {
        let panel = self.agents_panel.as_ref()?;
        let AgentsPanelSurface::Preview { agent_id, .. } = panel.surface.as_ref()? else {
            return None;
        };
        let record = panel.agent_view_for_agent(*agent_id)?;
        if let Some(error) = record.error.as_deref() {
            return Some(vec![format!("  {error}")]);
        }
        if record.snapshot.is_none() {
            return Some(vec!["  Loading agent preview...".to_string()]);
        }
        self.agents_panel_preview_body_lines()
    }

    /// preview 的 Space/Esc 只返回 overview，不提供 cancel/interrupt/steer（R15 基础形态）。
    ///
    /// permission Pending 时 Up/Down/j/k 移动 option selection（Left/Right/h/l 保持翻页，
    /// 不破坏 R15 滚动语义）；无 pending 时键位与基础形态完全一致。
    pub(super) fn handle_agents_panel_preview_key(&mut self, key: KeyEvent) -> OverlayInputResult {
        match key.code {
            KeyCode::Esc | KeyCode::Char(' ') if key.modifiers.is_empty() => {
                self.close_agents_panel_surface();
                OverlayInputResult::Handled
            }
            KeyCode::Left | KeyCode::Char('h') if key.modifiers.is_empty() => {
                self.move_agents_panel_preview_page(-1);
                OverlayInputResult::Handled
            }
            KeyCode::Right | KeyCode::Char('l') if key.modifiers.is_empty() => {
                self.move_agents_panel_preview_page(1);
                OverlayInputResult::Handled
            }
            KeyCode::Up | KeyCode::Char('k') if key.modifiers.is_empty() => {
                if !self.move_agents_panel_preview_permission_selection(-1) {
                    self.move_agents_panel_preview_page(-1);
                }
                OverlayInputResult::Handled
            }
            KeyCode::Down | KeyCode::Char('j') if key.modifiers.is_empty() => {
                if !self.move_agents_panel_preview_permission_selection(1) {
                    self.move_agents_panel_preview_page(1);
                }
                OverlayInputResult::Handled
            }
            KeyCode::Enter if key.modifiers.is_empty() => {
                match self.submit_agents_panel_preview_permission() {
                    Some(effect) => OverlayInputResult::Effect(effect),
                    None => OverlayInputResult::Handled,
                }
            }
            _ => OverlayInputResult::Handled,
        }
    }

    /// Enter 提交流程：head 存在且 Pending 且非本地 Submitted 时提交。
    ///
    /// 即时置 `Submitted`（防重复派发，不关 preview）；Effect 携带 head 的
    /// `AgentPermissionTarget` 原样——identity 只能来自 FIFO head，
    /// 绝不以 row / "当前 Agent" 推断。
    fn submit_agents_panel_preview_permission(&mut self) -> Option<AppEffect> {
        // 读取阶段：一次只读快照，避免与写回阶段的可变借用交叉。
        let submission = {
            let panel = self.agents_panel.as_ref()?;
            let AgentsPanelSurface::Preview {
                agent_id,
                permission_choice,
                ..
            } = panel.surface.as_ref()?
            else {
                return None;
            };
            // 本地已锁定或 runtime 投影 Submitted：不可重复提交
            //（UI 防呆；runtime 全链校验仍是 fail-closed 权威）。
            if matches!(
                permission_choice,
                AgentsPanelPreviewPermissionChoice::Submitted { .. }
            ) {
                return None;
            }
            let head = panel
                .agent_view_for_agent(*agent_id)?
                .snapshot
                .as_ref()?
                .preview
                .permission
                .as_ref()?;
            if head.state != AgentPermissionState::Pending {
                return None;
            }
            let selected = match permission_choice {
                AgentsPanelPreviewPermissionChoice::Selecting {
                    request_id,
                    selected,
                } if *request_id == head.target.request_id => *selected,
                // choice 与 head 脱节（reconcile 前的理论窗口）：以初始选择兜底。
                _ => 0,
            };
            let option = head.request.options.get(selected)?;
            PreviewPermissionSubmission {
                request_id: head.target.request_id.clone(),
                option_id: option.option_id.clone(),
                target: head.target.clone(),
            }
        };
        // 写回阶段：锁定为本地 Submitted，防止 Enter 重复触发同一 request。
        if let Some(panel) = self.agents_panel.as_mut()
            && let Some(AgentsPanelSurface::Preview {
                permission_choice, ..
            }) = panel.surface.as_mut()
        {
            *permission_choice = AgentsPanelPreviewPermissionChoice::Submitted {
                request_id: submission.request_id,
                option_id: Some(submission.option_id.clone()),
            };
        }
        Some(AppEffect::RespondAgentPermission {
            target: submission.target,
            option_id: submission.option_id,
        })
    }

    /// pending permission 时移动 option selection（循环移位，对齐 tool approval 约定）。
    ///
    /// 返回 false 表示当前无 Pending 选择态（调用方回落到翻页），保证无 pending
    /// 键位零变化。
    fn move_agents_panel_preview_permission_selection(&mut self, direction: isize) -> bool {
        // 先以只读视图取全部输入，再一次性写回，避免 surface 可变借用与
        // panel 读取交叉。
        let selection_input = {
            let Some(panel) = self.agents_panel.as_ref() else {
                return false;
            };
            let Some(AgentsPanelSurface::Preview {
                permission_choice, ..
            }) = panel.surface.as_ref()
            else {
                return false;
            };
            let AgentsPanelPreviewPermissionChoice::Selecting { selected, .. } = permission_choice
            else {
                return false;
            };
            let option_count = panel
                .surface_agent_id()
                .and_then(|agent_id| panel.agent_view_for_agent(agent_id))
                .and_then(|record| record.snapshot.as_ref())
                .and_then(|snapshot| snapshot.preview.permission.as_ref())
                .map(|head| head.request.options.len())
                .unwrap_or(0);
            (*selected, option_count)
        };
        let (selected, option_count) = selection_input;
        if option_count == 0 {
            return false;
        }
        let next = cyclic_option_index(selected, direction, option_count);
        if let Some(panel) = self.agents_panel.as_mut()
            && let Some(AgentsPanelSurface::Preview {
                permission_choice, ..
            }) = panel.surface.as_mut()
            && let AgentsPanelPreviewPermissionChoice::Selecting { selected, .. } =
                permission_choice
        {
            *selected = next;
        }
        true
    }

    /// preview permission 区块与 observation snapshot 的 reconcile：
    /// 事件快照是渲染权威——head 消失清空区块、head 变化（request identity 对比）
    /// 重置选择、runtime 拒绝后 snapshot 仍 Pending 时解除本地锁定允许重试。
    pub(crate) fn sync_agents_panel_preview_permission(&mut self, agent_id: AgentId) {
        let Some(panel) = self.agents_panel.as_mut() else {
            return;
        };
        let is_preview_surface_for_agent = matches!(
            panel.surface.as_ref(),
            Some(AgentsPanelSurface::Preview {
                agent_id: surface_agent_id,
                ..
            }) if *surface_agent_id == agent_id
        );
        if !is_preview_surface_for_agent {
            return;
        }
        let head_identity = panel
            .agent_view_for_agent(agent_id)
            .and_then(|record| record.snapshot.as_ref())
            .and_then(|snapshot| snapshot.preview.permission.as_ref())
            .map(|head| (head.target.request_id.clone(), head.state));
        let Some(panel_surface) = panel.surface.as_mut() else {
            return;
        };
        let AgentsPanelSurface::Preview {
            permission_choice, ..
        } = panel_surface
        else {
            return;
        };
        *permission_choice =
            reconcile_preview_permission_choice(permission_choice.clone(), head_identity);
    }
}

/// 打开 preview 时按 snapshot head 初始化 permission 交互态。
pub(super) fn initial_preview_permission_choice(
    head: Option<&AgentPermissionRequest>,
) -> AgentsPanelPreviewPermissionChoice {
    let Some(head) = head else {
        return AgentsPanelPreviewPermissionChoice::None;
    };
    match head.state {
        AgentPermissionState::Pending => AgentsPanelPreviewPermissionChoice::Selecting {
            request_id: head.target.request_id.clone(),
            selected: 0,
        },
        AgentPermissionState::Submitted => AgentsPanelPreviewPermissionChoice::Submitted {
            request_id: head.target.request_id.clone(),
            option_id: None,
        },
    }
}

/// reconcile 规则（design 决策 #8）：事件为权威，本地 Submitted 只在
/// snapshot 同意时存活。
fn reconcile_preview_permission_choice(
    current: AgentsPanelPreviewPermissionChoice,
    head: Option<(String, AgentPermissionState)>,
) -> AgentsPanelPreviewPermissionChoice {
    let Some((request_id, state)) = head else {
        // head 收敛或清空：区块消失。
        return AgentsPanelPreviewPermissionChoice::None;
    };
    match state {
        AgentPermissionState::Pending => match current {
            // 同一 request 的 selection 保持——无关快照更新（answer/elapsed 等）
            // 不得重置用户的选择。
            AgentsPanelPreviewPermissionChoice::Selecting {
                request_id: current_request_id,
                selected,
            } if current_request_id == request_id => {
                AgentsPanelPreviewPermissionChoice::Selecting {
                    request_id,
                    selected,
                }
            }
            // 新 request，或本地 Submitted 被 runtime 拒绝（snapshot 仍 Pending）：
            // 回到初始选择允许重试。
            _ => AgentsPanelPreviewPermissionChoice::Selecting {
                request_id,
                selected: 0,
            },
        },
        AgentPermissionState::Submitted => match current {
            // 本地刚提交同一 request：保留已提交 option 的锁定显示。
            AgentsPanelPreviewPermissionChoice::Submitted {
                request_id: current_request_id,
                option_id: option_id @ Some(_),
            } if current_request_id == request_id => {
                AgentsPanelPreviewPermissionChoice::Submitted {
                    request_id,
                    option_id,
                }
            }
            // runtime 投影 Submitted（重开 preview / 别处提交）：锁定但不知道具体 option。
            _ => AgentsPanelPreviewPermissionChoice::Submitted {
                request_id,
                option_id: None,
            },
        },
    }
}

/// option selection 的循环移位（无边界钳制，首尾循环）。
fn cyclic_option_index(current: usize, direction: isize, count: usize) -> usize {
    if count == 0 {
        return 0;
    }
    let current = current % count;
    match direction.signum() {
        direction if direction < 0 => (current + count - 1) % count,
        direction if direction > 0 => (current + 1) % count,
        _ => current,
    }
}

/// preview header 单行布局：status(固定列) + title(弹性截断) + elapsed(空间不足先隐藏)。
pub(super) struct AgentsPanelPreviewHeader {
    pub(super) status: String,
    pub(super) title: String,
    pub(super) elapsed: Option<String>,
}

/// preview header 的 title 保底宽度；低于此值时 elapsed 让位。
pub(super) const AGENTS_PREVIEW_TITLE_MIN_WIDTH: usize = 8;

pub(super) fn agents_panel_preview_header(
    status: runtime_domain::agent::AgentProjectionStatus,
    title: &str,
    elapsed_ms: Option<u64>,
    width: usize,
) -> AgentsPanelPreviewHeader {
    use crate::display_width::display_width;
    let status_budget = width.saturating_sub(AGENTS_PREVIEW_HORIZONTAL_PADDING);
    let status = super::pad_agents_status_column(agent_status_label(status), status_budget);
    let status_width = display_width(&status);
    let elapsed_label = elapsed_ms.map(|ms| {
        left_pad_display_width(&format_agent_elapsed_ms(ms), AGENTS_ELAPSED_COLUMN_WIDTH)
    });
    let title_budget = width
        .saturating_sub(
            AGENTS_PREVIEW_HORIZONTAL_PADDING
                + status_width
                + 1
                + AGENTS_PREVIEW_HORIZONTAL_PADDING,
        )
        .max(1);
    // 宽度不足时先隐藏 elapsed，再对 title 做 display-width 安全截断。
    let elapsed_width = elapsed_label.as_deref().map(display_width).unwrap_or(0);
    let show_elapsed = elapsed_label.is_some()
        && title_budget > elapsed_width + 1 + AGENTS_PREVIEW_TITLE_MIN_WIDTH;
    let title_width = if show_elapsed {
        title_budget - elapsed_width - 1
    } else {
        title_budget
    };
    AgentsPanelPreviewHeader {
        status,
        title: truncate_display_width_with_ellipsis(title, title_width),
        elapsed: show_elapsed.then(|| elapsed_label.unwrap_or_default()),
    }
}

pub(super) fn agents_panel_preview_wrap_width(window_width: u16) -> usize {
    usize::from(window_width)
        .saturating_sub(AGENTS_PREVIEW_HORIZONTAL_PADDING * 2)
        .max(1)
}
