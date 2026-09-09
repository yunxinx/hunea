use runtime_domain::agent::{
    AgentId, AgentObservationId, AgentObservationRequestId, AgentOverviewRow,
    AgentRuntimeGeneration, AgentViewSnapshot,
};

use crate::{
    agents_panel::{
        AGENTS_ACTIVITY_FOLD_MIN_WIDTH, agent_status_is_running, agent_status_is_settled,
        agents_activity_fold_entries,
        groups::{AgentsRowGroupKind, agents_panel_row_groups, agents_row_display_order},
    },
    fullscreen_search_list::FullscreenSearchListState,
    list_selection::ListNavigationDirection,
    text_search::CaseInsensitiveQuery,
    transcript::Transcript,
    transcript_overlay::TranscriptOverlayState,
};

/// `x` 二次确认绑定的目标与动作语义：running 行是 stop（停止 subtree），
/// settled 投影行是 delete（销毁并移除行）。动作类别与行状态类别脱节即取消，
/// 防止一次 armed 的 stop 确认被静默转换为 delete。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AgentsPanelStopConfirmation {
    Stop(AgentId),
    Delete(AgentId),
}

impl AgentsPanelStopConfirmation {
    pub(crate) fn agent_id(&self) -> AgentId {
        match self {
            Self::Stop(agent_id) | Self::Delete(agent_id) => *agent_id,
        }
    }
}

/// `/agents` panel 的全部 TUI 侧状态。
///
/// 打开即 owner-bound 到一次 overview observation：observation_id/generation 只在
/// 匹配的 snapshot 回包后建立，后续 delta 必须完全匹配才应用。
#[derive(Debug, Clone)]
pub(crate) struct AgentsPanelState {
    pub(super) list: FullscreenSearchListState<AgentOverviewRow, AgentId>,
    /// loading 期等待的 snapshot 请求；与 runtime 回显的 request_id 匹配才建立 projection。
    pub(super) pending_request_id: Option<AgentObservationRequestId>,
    pub(super) is_loading: bool,
    pub(super) error: Option<String>,
    /// 当前 overview observation 的 identity；不匹配的增量一律 fail closed。
    pub(super) observation_id: Option<AgentObservationId>,
    pub(super) generation: Option<AgentRuntimeGeneration>,
    /// `x` 二次确认绑定的目标与动作；selection/generation 变化或动作类别失效即取消。
    pub(super) stop_confirmation: Option<AgentsPanelStopConfirmation>,
    /// loading 期按下 `x` 后置位：footer 呈现"状态未就绪"提示，按键不静默吞掉。
    /// snapshot 或 error 到达即清除（此时 stop 的可用性由列表/错误行自述）。
    pub(super) stop_unavailable_notice: bool,
    /// panel 打开期间访问过的 per-agent view observation 记录。
    ///
    /// Enter/Space 反复进出同一 agent 复用同一记录（不建第二个 observer）；
    /// panel 关闭时全部记录统一注销。
    pub(super) agent_views: Vec<AgentsPanelAgentView>,
    /// 层内子模式；`None` 即 overview list。
    pub(super) surface: Option<AgentsPanelSurface>,
    /// 选中 agent 的活动折叠区缓存（见 `AgentsPanelActivityFold`）。
    pub(super) activity_fold: AgentsPanelActivityFold,
    /// 折叠区是否展开。仅"选中行上按 Tab"触发展开；任何 selection 变化重置，
    /// 鼠标 hover / 快照更新不改变该状态。
    pub(super) activity_fold_expanded: bool,
}

/// 选中 agent 的活动折叠区缓存：最近 delivery-safe 活动摘要 + 折叠计数。
///
/// 数据取自已建立的 per-agent view observation snapshot（与 transcript surface
/// 同源数据通路），不为折叠区派发新的 observation；选中 agent 无 snapshot 时
/// 缓存为空，折叠区不渲染。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct AgentsPanelActivityFold {
    /// 缓存归属；与当前 selection 不一致即视为陈旧，不参与渲染。
    pub(super) agent_id: Option<AgentId>,
    /// 最近活动单行摘要（旧 → 新，渲染顺序一致）。
    pub(super) entries: Vec<String>,
    /// 未展示的更早活动条数（`+N more` 行）。
    pub(super) more_count: usize,
}

impl AgentsPanelActivityFold {
    /// 折叠区当前可渲染的行数（活动行 + 可选 `+N more` 行）。
    ///
    /// 归属校验内联在本判定：缓存 `agent_id` 与期望的选中 agent 不一致即视为
    /// 陈旧，返回 0。展开态与宽度由调用方传入：未展开、宽度低于阈值或无条目
    /// 时为 0；渲染与鼠标物理行换算共用本判定，保证两处对"折叠区是否占行"
    /// 的答案一致。
    pub(super) fn visible_line_count(
        &self,
        expected_agent_id: AgentId,
        width: usize,
        expanded: bool,
    ) -> usize {
        if self.agent_id != Some(expected_agent_id)
            || !expanded
            || width < AGENTS_ACTIVITY_FOLD_MIN_WIDTH
            || self.entries.is_empty()
        {
            return 0;
        }
        self.entries.len() + usize::from(self.more_count > 0)
    }
}

/// 一个 per-agent view observation 的绑定记录。
#[derive(Debug, Clone)]
pub(crate) struct AgentsPanelAgentView {
    pub(super) agent_id: AgentId,
    /// 等待 `AgentViewSnapshotLoaded` 回显的请求。
    pub(super) pending_request_id: Option<AgentObservationRequestId>,
    /// observation 建立后用于匹配 `AgentViewUpdated`。
    pub(super) observation_id: Option<AgentObservationId>,
    pub(super) generation: Option<AgentRuntimeGeneration>,
    pub(super) snapshot: Option<AgentViewSnapshot>,
    pub(super) error: Option<String>,
}

/// transcript surface 的 permission 区块交互态。
///
/// 交互态与 head 的 request identity 绑定：`AgentViewUpdated` 到达时按 request_id
/// 对比决定保持还是重置（无关快照更新不得重置用户的选择）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AgentsPanelPermissionChoice {
    /// 无 pending head（或记录尚在 loading）：不渲染 permission 区块。
    None,
    /// FIFO head 处于 Pending：可见 selection，可移动、可提交。
    Selecting { request_id: String, selected: usize },
    /// 已提交的不可重复提交态。
    ///
    /// `option_id` 已知（本地刚提交）时锁定显示该 option；`None` 表示只知道
    /// runtime 投影 Submitted（重开 surface 等场景，投影不携带已选 option），
    /// options 照常渲染但无 marker。
    Submitted {
        request_id: String,
        option_id: Option<String>,
    },
}

/// panel 层内的 transcript surface：完整 child transcript 视图 + permission 交互面。
///
/// 与 per-agent view observation（`AgentViewSnapshot` 聚合 transcript 与 preview 两份
/// 投影）一一对应，因此是层内子模式而非独立 ModalLayer；`None` 即 overview list。
/// Transcript 体积大，Box 化避免撑大整个 panel 状态。
#[derive(Debug, Clone)]
pub(crate) struct AgentsPanelSurface {
    pub(super) agent_id: AgentId,
    pub(super) transcript: Box<Transcript>,
    pub(super) overlay: TranscriptOverlayState,
    pub(super) is_following_bottom: bool,
    /// permission 区块交互态；渲染与提交共用（单一事实源在 surface 内）。
    pub(super) permission_choice: AgentsPanelPermissionChoice,
}

/// panel 关闭后待 runner 派发的 observation 注销集合。
///
/// 关闭多发生在无 runtime 调用上下文的路径（Esc、pill 关层、resume、reset），
/// 先在 Model 侧置位，由 runner effect 循环开头统一消费。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct PendingAgentObservationStops {
    /// panel 级 overview observation。
    pub(crate) overview: Option<(AgentObservationId, AgentRuntimeGeneration)>,
    /// surface 级 per-agent view observation。
    pub(crate) agent_views: Vec<(AgentObservationId, AgentRuntimeGeneration)>,
}

/// 当前页 body 内一个物理行的种类：组头行（不可选）或数据行。
///
/// 列头行（body 首行）与选中行下方的折叠区行不在此列——它们分别由渲染入口
/// 固定在首位、由渲染/鼠标按选中行计入，语义与数据行的页换算不同。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AgentsPanelPageBodyLine {
    /// 组头行；`row_count` 是该组在过滤视图中的总行数（组头计数语义）。
    GroupHeader {
        kind: AgentsRowGroupKind,
        row_count: usize,
    },
    /// 数据行；`position` 是过滤视图中的显示位置（可选目标）。
    Row { position: usize },
}

impl PendingAgentObservationStops {
    pub(crate) fn is_empty(&self) -> bool {
        self.overview.is_none() && self.agent_views.is_empty()
    }

    /// 合并另一批注销：关闭路径可能分多次置位（panel 关闭 + 后到的 pending 回包补 stop）。
    pub(crate) fn merge(&mut self, other: Self) {
        if self.overview.is_none() {
            self.overview = other.overview;
        }
        self.agent_views.extend(other.agent_views);
    }
}

impl AgentsPanelState {
    pub(super) fn loading(request_id: AgentObservationRequestId) -> Self {
        Self {
            list: FullscreenSearchListState::default(),
            pending_request_id: Some(request_id),
            is_loading: true,
            error: None,
            observation_id: None,
            generation: None,
            stop_confirmation: None,
            stop_unavailable_notice: false,
            agent_views: Vec::new(),
            surface: None,
            activity_fold: AgentsPanelActivityFold::default(),
            activity_fold_expanded: false,
        }
    }

    // —— overview list 委托 ——

    /// selection 变化即折叠态重置：展开只由"选中该行时按下 Tab"触发，
    /// 不是常驻状态——离开（哪怕稍后回来）后必须重新按 Tab。
    fn reset_activity_fold_if_selection_changed(&mut self, previous: Option<AgentId>) {
        let current = self.list.selected_row().map(|row| row.agent_id);
        if current != previous {
            self.activity_fold_expanded = false;
        }
    }

    pub(super) fn replace_rows(&mut self, rows: Vec<AgentOverviewRow>) {
        let previous_selection = self.list.selected_row().map(|row| row.agent_id);
        self.list
            .replace_rows(rows, agents_row_matches, agents_row_id);
        self.reset_activity_fold_if_selection_changed(previous_selection);
        self.refresh_selected_activity_fold();
    }

    /// delta upsert：stable AgentId 原位替换（新行追加），selection identity 不变。
    /// 过滤视图变化导致 selected 行被移出时 selection 迁移，折叠态随之重置。
    pub(super) fn upsert_row(&mut self, row: AgentOverviewRow) {
        let previous_selection = self.list.selected_row().map(|row| row.agent_id);
        self.list.upsert_row(row, agents_row_matches, agents_row_id);
        self.reset_activity_fold_if_selection_changed(previous_selection);
        self.refresh_selected_activity_fold();
    }

    /// delta remove：被移除的 selected agent 由 `restore_selected_id_or_clamp` 迁移。
    pub(super) fn remove_row(&mut self, agent_id: AgentId) {
        let previous_selection = self.list.selected_row().map(|row| row.agent_id);
        self.list
            .remove_row(agent_id, agents_row_matches, agents_row_id);
        self.reset_activity_fold_if_selection_changed(previous_selection);
        // selection 迁移后旧缓存归属脱节，必须立即重建（可能清空）。
        self.refresh_selected_activity_fold();
    }

    pub(super) fn move_selection(&mut self, direction: ListNavigationDirection) {
        let previous_selection = self.list.selected_row().map(|row| row.agent_id);
        self.list.move_selection(direction, agents_row_id);
        self.reset_activity_fold_if_selection_changed(previous_selection);
        self.refresh_selected_activity_fold();
    }

    pub(super) fn move_page(&mut self, direction: ListNavigationDirection, page_size: usize) {
        let previous_selection = self.list.selected_row().map(|row| row.agent_id);
        self.list.move_page(direction, page_size, agents_row_id);
        self.reset_activity_fold_if_selection_changed(previous_selection);
        self.refresh_selected_activity_fold();
    }

    pub(super) fn push_search_character(&mut self, character: char) {
        let previous_selection = self.list.selected_row().map(|row| row.agent_id);
        self.list
            .push_search_character(character, agents_row_matches, agents_row_id);
        self.reset_activity_fold_if_selection_changed(previous_selection);
        self.refresh_selected_activity_fold();
    }

    pub(super) fn backspace_search(&mut self) {
        let previous_selection = self.list.selected_row().map(|row| row.agent_id);
        self.list
            .backspace_search(agents_row_matches, agents_row_id);
        self.reset_activity_fold_if_selection_changed(previous_selection);
        self.refresh_selected_activity_fold();
    }

    pub(super) fn clear_search(&mut self) -> bool {
        let previous_selection = self.list.selected_row().map(|row| row.agent_id);
        let cleared = self.list.clear_search(agents_row_matches, agents_row_id);
        if cleared {
            self.reset_activity_fold_if_selection_changed(previous_selection);
            self.refresh_selected_activity_fold();
        }
        cleared
    }

    pub(super) fn exit_search(&mut self) -> bool {
        let previous_selection = self.list.selected_row().map(|row| row.agent_id);
        let exited = self.list.exit_search(agents_row_matches, agents_row_id);
        if exited {
            self.reset_activity_fold_if_selection_changed(previous_selection);
            self.refresh_selected_activity_fold();
        }
        exited
    }

    pub(super) fn start_search(&mut self) {
        self.list.start_search();
    }

    pub(super) fn page_start(&self, page_size: usize) -> usize {
        self.list.page_start(page_size)
    }

    pub(super) fn page_number(&self, page_size: usize) -> usize {
        self.list.page_number(page_size)
    }

    pub(super) fn page_count(&self, page_size: usize) -> usize {
        self.list.page_count(page_size)
    }

    // —— 分组显示顺序 ——

    /// 把行存储归一到当前分组显示顺序（selection 以 stable id 重锚）。
    ///
    /// 分组随墙钟迁移（Just finished → Completed），没有事件驱动；渲染与输入
    /// 在读取顺序敏感状态（分页、导航、物理行换算）前调用。归一只改顺序，
    /// 不改行集合与选中行，幂等；折叠缓存与确认态都以 id 绑定，无需重置。
    pub(super) fn refresh_display_order(&mut self, now_ms: i64) {
        self.list.reorder_rows(
            |a, b| agents_row_display_order(a, b, now_ms),
            agents_row_matches,
            agents_row_id,
        );
    }

    /// 过滤视图指定显示位置上的行。
    pub(super) fn filtered_row_at(&self, position: usize) -> Option<&AgentOverviewRow> {
        self.list.filtered_row_at(position)
    }

    /// 当前页 body 的物理行序列：组头行 + 数据行。
    ///
    /// 组头行只在该组首行落入本页时出现（跨页续组不重复表头）；组头计数是
    /// 该组在过滤视图中的总行数。页行预算已为组头恒定预留，行数不溢出。
    /// 渲染与鼠标物理行换算共用本计划，两侧对"哪个物理行是什么"的答案一致。
    pub(super) fn page_body_line_plan(
        &self,
        page_size: usize,
        now_ms: i64,
    ) -> Vec<AgentsPanelPageBodyLine> {
        let page_start = self.page_start(page_size);
        let page_end = page_start
            .saturating_add(page_size)
            .min(self.filtered_count());
        let mut plan = Vec::new();
        for group in agents_panel_row_groups(&self.list.filtered_rows(), now_ms) {
            let header_starts_in_page = group
                .row_indices
                .first()
                .is_some_and(|&first| page_start <= first && first < page_end);
            if header_starts_in_page {
                plan.push(AgentsPanelPageBodyLine::GroupHeader {
                    kind: group.kind,
                    row_count: group.row_indices.len(),
                });
            }
            for &position in &group.row_indices {
                if page_start <= position && position < page_end {
                    plan.push(AgentsPanelPageBodyLine::Row { position });
                }
            }
        }
        plan
    }

    pub(super) fn select_visible_row(&mut self, page_size: usize, visible_offset: usize) -> bool {
        let previous_selection = self.list.selected_row().map(|row| row.agent_id);
        let selected = self
            .list
            .select_visible_row(page_size, visible_offset, agents_row_id);
        if selected {
            self.reset_activity_fold_if_selection_changed(previous_selection);
            self.refresh_selected_activity_fold();
        }
        selected
    }

    /// 按物理行偏移选行：组头行与选中行的折叠区都计入物理行预算。
    ///
    /// 渲染把组头画在组首行前、折叠行画在选中行下方；鼠标点击的物理行号需要
    /// 同一换算才能命中正确行。组头行与折叠行都不是独立可选目标——点击落在
    /// 组头上直接吞掉，落在折叠区上归属选中行本身。
    pub(super) fn select_physical_body_line(
        &mut self,
        page_size: usize,
        physical_offset: usize,
        width: usize,
        now_ms: i64,
    ) -> bool {
        // 折叠行只挂在当前 selection 名下：归属脱节的陈旧缓存不占物理行
        // （与渲染侧共用 `visible_line_count` 的归属校验）。
        let fold_line_count = self.selected_row().map_or(0, |row| {
            self.activity_fold
                .visible_line_count(row.agent_id, width, self.activity_fold_expanded)
        });
        let page_start = self.page_start(page_size);
        let mut remaining = physical_offset;
        let mut logical_offset = None;
        for line in self.page_body_line_plan(page_size, now_ms) {
            match line {
                AgentsPanelPageBodyLine::GroupHeader { .. } => {
                    // 组头不可选：命中组头的点击不改变 selection。
                    if remaining == 0 {
                        return false;
                    }
                    remaining -= 1;
                }
                AgentsPanelPageBodyLine::Row { position } => {
                    let row_line_count =
                        1 + usize::from(self.is_selected_visible_position(position))
                            * fold_line_count;
                    if remaining < row_line_count {
                        logical_offset = Some(position - page_start);
                        break;
                    }
                    remaining -= row_line_count;
                }
            }
        }
        let Some(logical_offset) = logical_offset else {
            return false;
        };
        self.select_visible_row(page_size, logical_offset)
    }

    /// 按 AgentId 预选（pill 导航等显式定位）；目标不在 filtered 视图时保持原 selection。
    pub(super) fn select_agent(&mut self, agent_id: AgentId) -> bool {
        let previous_selection = self.list.selected_row().map(|row| row.agent_id);
        let selected = self.list.select_id(agent_id, agents_row_id);
        if selected {
            self.reset_activity_fold_if_selection_changed(previous_selection);
            self.refresh_selected_activity_fold();
        }
        selected
    }

    pub(super) fn selected_position_label(&self) -> usize {
        self.list.selected_position_label()
    }

    pub(super) fn filtered_count(&self) -> usize {
        self.list.filtered_count()
    }

    pub(super) fn has_rows(&self) -> bool {
        self.list.has_rows()
    }

    pub(super) fn has_filtered_rows(&self) -> bool {
        self.list.has_filtered_rows()
    }

    pub(super) fn is_selected_visible_position(&self, visible_position: usize) -> bool {
        self.list.is_selected_visible_position(visible_position)
    }

    pub(super) fn selected_row(&self) -> Option<&AgentOverviewRow> {
        self.list.selected_row()
    }

    pub(super) fn is_searching(&self) -> bool {
        self.list.is_searching()
    }

    pub(super) fn search_query(&self) -> &str {
        self.list.search_query()
    }

    // —— per-agent view 记录 ——

    pub(super) fn agent_view_for_agent(&self, agent_id: AgentId) -> Option<&AgentsPanelAgentView> {
        self.agent_views
            .iter()
            .find(|record| record.agent_id == agent_id)
    }

    pub(super) fn agent_view_for_agent_mut(
        &mut self,
        agent_id: AgentId,
    ) -> Option<&mut AgentsPanelAgentView> {
        self.agent_views
            .iter_mut()
            .find(|record| record.agent_id == agent_id)
    }

    pub(super) fn agent_view_with_pending_request_mut(
        &mut self,
        request_id: AgentObservationRequestId,
    ) -> Option<&mut AgentsPanelAgentView> {
        self.agent_views
            .iter_mut()
            .find(|record| record.pending_request_id == Some(request_id))
    }

    // —— 活动折叠区 ——

    pub(super) fn selected_activity_fold(&self) -> &AgentsPanelActivityFold {
        &self.activity_fold
    }

    /// Tab 切换折叠区展开态（列表态下的唯一展开入口）。
    pub(super) fn toggle_activity_fold_expanded(&mut self) {
        self.activity_fold_expanded = !self.activity_fold_expanded;
    }

    /// 刷新选中 agent 的活动折叠区缓存。
    ///
    /// 缓存与 selection、per-agent view snapshot 双绑定：任一变化后必须调用。
    /// 选中 agent 无 row 或 snapshot 未就绪时清空（折叠区不渲染，
    /// 不为折叠区派发新的 observation）。
    pub(super) fn refresh_selected_activity_fold(&mut self) {
        let fold = match self.selected_row().map(|row| row.agent_id) {
            Some(agent_id) => {
                match self
                    .agent_view_for_agent(agent_id)
                    .and_then(|record| record.snapshot.as_ref())
                {
                    Some(snapshot) => {
                        let (entries, more_count) =
                            agents_activity_fold_entries(&snapshot.transcript.items);
                        AgentsPanelActivityFold {
                            agent_id: Some(agent_id),
                            entries,
                            more_count,
                        }
                    }
                    None => AgentsPanelActivityFold::default(),
                }
            }
            None => AgentsPanelActivityFold::default(),
        };
        self.activity_fold = fold;
    }

    /// 当前 surface 绑定的 AgentId。
    pub(super) fn surface_agent_id(&self) -> Option<AgentId> {
        self.surface.as_ref().map(|surface| surface.agent_id)
    }

    /// stop 确认态是否仍然成立：确认目标仍是当前 selection，且行的状态类别与
    /// armed 的动作语义一致（running ↔ Stop、settled ↔ Delete）。
    ///
    /// "selection 改变即取消"由此判定——delta remove 后 selection 按 clamp 规则迁移，
    /// 迁移结果不指向被确认的 AgentId 时确认自动失效；running 确认后行自然进入
    /// 终态也取消（armed 的是 stop，不得静默转为 delete）。
    pub(super) fn stop_confirmation_still_valid(&self) -> bool {
        self.stop_confirmation.is_some_and(|confirmation| {
            self.list.selected_row().is_some_and(|row| {
                row.agent_id == confirmation.agent_id()
                    && match confirmation {
                        AgentsPanelStopConfirmation::Stop(_) => agent_status_is_running(row.status),
                        AgentsPanelStopConfirmation::Delete(_) => {
                            agent_status_is_settled(row.status)
                        }
                    }
            })
        })
    }
}

/// 搜索按 frozen title 匹配；runtime 文本在进入行渲染前由截断函数保证安全。
fn agents_row_matches(row: &AgentOverviewRow, query: &CaseInsensitiveQuery<'_>) -> bool {
    query.matches(row.title.as_str())
}

fn agents_row_id(row: &AgentOverviewRow) -> AgentId {
    row.agent_id
}
