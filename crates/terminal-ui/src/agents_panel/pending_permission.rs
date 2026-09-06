//! Agent permission 的全局 pending 权威投影与 pill 导航意图。
//!
//! 数据源单一：只由 `AgentPermissionUpdated` 事件（observation-independent 投影）驱动，
//! session reset/resume、runtime replacement（`RuntimeEvent::Stopped`）与 stale generation
//! 守卫四条路径负责清空。它只服务 attention pill 的可见性与点击路由；
//! preview 的 permission 区块继续读 observation snapshot（`snapshot.preview.permission`），
//! 两面对同一 runtime FIFO、各自事件驱动，互不同步。

use std::collections::BTreeMap;

use runtime_domain::agent::{
    AgentId, AgentPermissionRequest, AgentPermissionState, AgentPermissionUpdate,
    AgentRuntimeGeneration,
};

use crate::Model;

/// 全局 Agent pending permission 投影：generation-scoped 的 per-agent FIFO head。
///
/// `AgentPermissionUpdate` 只携带 head（不携带队列深度），因此 `heads` 的语义是
/// "每个 agent 当前 head 是谁"——多 pending 判定按有 Pending head 的 agent 数计。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct AgentPendingPermissionProjection {
    /// 投影归属的 runtime generation；更高 generation 的事件到达即整 map 重建，
    /// 更低 generation 的迟到事件被丢弃。
    pub(super) generation: Option<AgentRuntimeGeneration>,
    /// agent_id → 当前 FIFO head（Pending 或 Submitted）。
    ///
    /// Submitted entry 保留（preview submitted 呈现对账用），但 pill 判定只统计 Pending。
    pub(super) heads: BTreeMap<AgentId, AgentPermissionRequest>,
}

impl AgentPendingPermissionProjection {
    /// 应用一次 head 投影：`Some` upsert、`None` remove。
    ///
    /// generation 守卫（stale fail closed）：runtime replacement 时 generation 单调
    /// 递增，因此更高 generation 的首个事件意味着旧 map 整体失效、按该事件重建；
    /// 更低 generation 的迟到事件不更新 fresh state（PRD stale guard）。
    fn apply(&mut self, update: &AgentPermissionUpdate) {
        match self.generation {
            Some(current) if current > update.generation => {
                // 旧 generation 的迟到事件：fresh state 不受污染。
                return;
            }
            Some(current) if current == update.generation => {}
            _ => {
                // 新 generation 首个事件（或 map 为空）：旧 entry 全部失效，整 map 重建。
                self.generation = Some(update.generation);
                self.heads.clear();
            }
        }
        match &update.request {
            Some(request) => {
                self.heads.insert(update.agent_id, request.clone());
            }
            None => {
                self.heads.remove(&update.agent_id);
            }
        }
    }
}

/// pill 点击产生的 panel 导航意图。
///
/// panel 打开是异步的（loading → snapshot 回包），意图在 snapshot 投影建立后
/// 由 `apply_agents_overview_snapshot` 末尾消费；目标 agent 不存在或 panel 被关闭时
/// 意图作废停在 list（fail closed，不猜"当前 selection"）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AgentsPanelPillNavigation {
    /// 单 pending：直达该 agent 的 quick preview surface。
    OpenPreview { agent_id: AgentId },
    /// 多 pending：overview list 预选最早的 pending owner。
    Preselect { agent_id: AgentId },
}

impl Model {
    /// `AgentPermissionUpdated` 的唯一吸收入口：更新全局 pending 投影。
    pub(crate) fn apply_agent_permission_update(&mut self, update: AgentPermissionUpdate) {
        self.agent_pending_permissions.apply(&update);
    }

    /// 四清空路径（resume / reset / Stopped / generation 重置）共用的投影清空。
    pub(crate) fn clear_agent_pending_permissions(&mut self) {
        self.agent_pending_permissions = AgentPendingPermissionProjection::default();
    }

    /// 有 Pending head 的 agent 数（pill 可见性与单/多判定；Submitted 不参与）。
    pub(crate) fn pending_agent_permission_count(&self) -> usize {
        self.agent_pending_permissions
            .heads
            .values()
            .filter(|head| head.state == AgentPermissionState::Pending)
            .count()
    }

    /// Pending entries 按 `(occurred_at_ms, target.request_id)` 升序迭代；
    /// pill 单/多路由与多 pending 预选共用同一稳定序。
    fn pending_agent_permissions_in_arrival_order(
        &self,
    ) -> Vec<(AgentId, &AgentPermissionRequest)> {
        let mut pending: Vec<(AgentId, &AgentPermissionRequest)> = self
            .agent_pending_permissions
            .heads
            .iter()
            .filter(|(_, head)| head.state == AgentPermissionState::Pending)
            .map(|(agent_id, head)| (*agent_id, head))
            .collect();
        // BTreeMap 已按 AgentId 稳定排序；occurred_at + stable request identity 决定最终序。
        pending.sort_by(|(agent_id, head), (other_id, other_head)| {
            head.occurred_at_ms
                .cmp(&other_head.occurred_at_ms)
                .then_with(|| head.target.request_id.cmp(&other_head.target.request_id))
                .then_with(|| agent_id.cmp(other_id))
        });
        pending
    }

    /// pill 点击的路由目标：单 Pending 直达 preview，多 Pending 预选最早 owner。
    ///
    /// 归属只来自 pending 投影（FIFO head 的 AgentId），不以 row position / 当前 Agent 推断。
    pub(crate) fn agents_panel_pill_navigation_target(&self) -> Option<AgentsPanelPillNavigation> {
        let mut pending = self
            .pending_agent_permissions_in_arrival_order()
            .into_iter();
        let (agent_id, _) = pending.next()?;
        if pending.next().is_none() {
            Some(AgentsPanelPillNavigation::OpenPreview { agent_id })
        } else {
            Some(AgentsPanelPillNavigation::Preselect { agent_id })
        }
    }

    #[cfg(test)]
    pub(crate) fn agent_pending_permission_head_for_test(
        &self,
        agent_id: AgentId,
    ) -> Option<&AgentPermissionRequest> {
        self.agent_pending_permissions.heads.get(&agent_id)
    }

    #[cfg(test)]
    pub(crate) fn agent_pending_permission_generation_for_test(
        &self,
    ) -> Option<AgentRuntimeGeneration> {
        self.agent_pending_permissions.generation
    }
}
