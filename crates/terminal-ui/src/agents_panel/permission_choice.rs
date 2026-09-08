use runtime_domain::agent::{
    AgentId, AgentPermissionRequest, AgentPermissionState, AgentPermissionTarget,
};

use crate::{AppEffect, Model, agents_panel::AgentsPanelPermissionChoice};

/// Enter 提交的一次性输入快照：读取与写回两阶段间传递。
struct PermissionSubmission {
    request_id: String,
    option_id: String,
    target: AgentPermissionTarget,
}

impl Model {
    /// Enter 提交流程：head 存在且 Pending 且非本地 Submitted 时提交。
    ///
    /// 即时置 `Submitted`（防重复派发，不关 surface）；Effect 携带 head 的
    /// `AgentPermissionTarget` 原样——identity 只能来自 FIFO head，
    /// 绝不以 row / "当前 Agent" 推断。
    pub(super) fn submit_agents_panel_permission(&mut self) -> Option<AppEffect> {
        // 读取阶段：一次只读快照，避免与写回阶段的可变借用交叉。
        let submission = {
            let panel = self.agents_panel.as_ref()?;
            let surface = panel.surface.as_ref()?;
            let permission_choice = &surface.permission_choice;
            // 本地已锁定或 runtime 投影 Submitted：不可重复提交
            //（UI 防呆；runtime 全链校验仍是 fail-closed 权威）。
            if matches!(
                permission_choice,
                AgentsPanelPermissionChoice::Submitted { .. }
            ) {
                return None;
            }
            let head = panel
                .agent_view_for_agent(surface.agent_id)?
                .snapshot
                .as_ref()?
                .preview
                .permission
                .as_ref()?;
            if head.state != AgentPermissionState::Pending {
                return None;
            }
            let selected = match permission_choice {
                AgentsPanelPermissionChoice::Selecting {
                    request_id,
                    selected,
                } if *request_id == head.target.request_id => *selected,
                // choice 与 head 脱节（reconcile 前的理论窗口）：以初始选择兜底。
                _ => 0,
            };
            let option = head.request.options.get(selected)?;
            PermissionSubmission {
                request_id: head.target.request_id.clone(),
                option_id: option.option_id.clone(),
                target: head.target.clone(),
            }
        };
        // 写回阶段：锁定为本地 Submitted，防止 Enter 重复触发同一 request。
        if let Some(panel) = self.agents_panel.as_mut()
            && let Some(surface) = panel.surface.as_mut()
        {
            surface.permission_choice = AgentsPanelPermissionChoice::Submitted {
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
    /// 返回 false 表示当前无 Pending 选择态（调用方回落到翻页/滚动），保证无 pending
    /// 键位零变化。
    pub(super) fn move_agents_panel_permission_selection(&mut self, direction: isize) -> bool {
        // 先以只读视图取全部输入，再一次性写回，避免 surface 可变借用与
        // panel 读取交叉。
        let selection_input = {
            let Some(panel) = self.agents_panel.as_ref() else {
                return false;
            };
            let Some(surface) = panel.surface.as_ref() else {
                return false;
            };
            let AgentsPanelPermissionChoice::Selecting { selected, .. } =
                &surface.permission_choice
            else {
                return false;
            };
            let option_count = panel
                .agent_view_for_agent(surface.agent_id)
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
            && let Some(surface) = panel.surface.as_mut()
            && let AgentsPanelPermissionChoice::Selecting { selected, .. } =
                &mut surface.permission_choice
        {
            *selected = next;
        }
        true
    }

    /// surface permission 区块与 observation snapshot 的 reconcile：
    /// 事件快照是渲染权威——head 消失清空区块、head 变化（request identity 对比）
    /// 重置选择、runtime 拒绝后 snapshot 仍 Pending 时解除本地锁定允许重试。
    pub(crate) fn sync_agents_panel_permission(&mut self, agent_id: AgentId) {
        let Some(panel) = self.agents_panel.as_mut() else {
            return;
        };
        let is_surface_for_agent = panel
            .surface
            .as_ref()
            .is_some_and(|surface| surface.agent_id == agent_id);
        if !is_surface_for_agent {
            return;
        }
        let head_identity = panel
            .agent_view_for_agent(agent_id)
            .and_then(|record| record.snapshot.as_ref())
            .and_then(|snapshot| snapshot.preview.permission.as_ref())
            .map(|head| (head.target.request_id.clone(), head.state));
        let Some(surface) = panel.surface.as_mut() else {
            return;
        };
        surface.permission_choice =
            reconcile_permission_choice(surface.permission_choice.clone(), head_identity);
    }
}

/// 打开 surface 时按 snapshot head 初始化 permission 交互态。
pub(super) fn initial_permission_choice(
    head: Option<&AgentPermissionRequest>,
) -> AgentsPanelPermissionChoice {
    let Some(head) = head else {
        return AgentsPanelPermissionChoice::None;
    };
    match head.state {
        AgentPermissionState::Pending => AgentsPanelPermissionChoice::Selecting {
            request_id: head.target.request_id.clone(),
            selected: 0,
        },
        AgentPermissionState::Submitted => AgentsPanelPermissionChoice::Submitted {
            request_id: head.target.request_id.clone(),
            option_id: None,
        },
    }
}

/// reconcile 规则：事件为权威，本地 Submitted 只在 snapshot 同意时存活。
fn reconcile_permission_choice(
    current: AgentsPanelPermissionChoice,
    head: Option<(String, AgentPermissionState)>,
) -> AgentsPanelPermissionChoice {
    let Some((request_id, state)) = head else {
        // head 收敛或清空：区块消失。
        return AgentsPanelPermissionChoice::None;
    };
    match state {
        AgentPermissionState::Pending => match current {
            // 同一 request 的 selection 保持——无关快照更新（answer/elapsed 等）
            // 不得重置用户的选择。
            AgentsPanelPermissionChoice::Selecting {
                request_id: current_request_id,
                selected,
            } if current_request_id == request_id => AgentsPanelPermissionChoice::Selecting {
                request_id,
                selected,
            },
            // 新 request，或本地 Submitted 被 runtime 拒绝（snapshot 仍 Pending）：
            // 回到初始选择允许重试。
            _ => AgentsPanelPermissionChoice::Selecting {
                request_id,
                selected: 0,
            },
        },
        AgentPermissionState::Submitted => match current {
            // 本地刚提交同一 request：保留已提交 option 的锁定显示。
            AgentsPanelPermissionChoice::Submitted {
                request_id: current_request_id,
                option_id: option_id @ Some(_),
            } if current_request_id == request_id => AgentsPanelPermissionChoice::Submitted {
                request_id,
                option_id,
            },
            // runtime 投影 Submitted（重开 surface / 别处提交）：锁定但不知道具体 option。
            _ => AgentsPanelPermissionChoice::Submitted {
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
