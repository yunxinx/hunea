use runtime_domain::agent::{
    AgentId, AgentObservationId, AgentObservationRequestId, AgentOverviewRow,
    AgentRuntimeGeneration, AgentViewSnapshot,
};

use crate::{
    agents_panel::agent_status_is_stoppable, fullscreen_search_list::FullscreenSearchListState,
    list_selection::ListNavigationDirection, text_search::CaseInsensitiveQuery,
    transcript::Transcript, transcript_overlay::TranscriptOverlayState,
};

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
    /// `x` 二次确认绑定的 AgentId；selection/generation 变化即取消。
    pub(super) stop_confirmation: Option<AgentId>,
    /// panel 打开期间访问过的 per-agent view observation 记录。
    ///
    /// Enter/Space 反复进出同一 agent 复用同一记录（不建第二个 observer）；
    /// panel 关闭时全部记录统一注销。
    pub(super) agent_views: Vec<AgentsPanelAgentView>,
    /// 层内子模式；`None` 即 overview list。
    pub(super) surface: Option<AgentsPanelSurface>,
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

/// preview 与 transcript 共用同一 per-agent view observation（`AgentViewSnapshot`
/// 聚合两份数据），因此两者都是层内子模式而非独立 ModalLayer。
///
/// Transcript 体积大且两个 variant 尺寸悬殊，Box 化避免撑大整个 panel 状态。
#[derive(Debug, Clone)]
pub(crate) enum AgentsPanelSurface {
    Preview {
        agent_id: AgentId,
        scroll_offset: usize,
    },
    Transcript {
        agent_id: AgentId,
        transcript: Box<Transcript>,
        overlay: TranscriptOverlayState,
        is_following_bottom: bool,
    },
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
            agent_views: Vec::new(),
            surface: None,
        }
    }

    // —— overview list 委托 ——

    pub(super) fn replace_rows(&mut self, rows: Vec<AgentOverviewRow>) {
        self.list
            .replace_rows(rows, agents_row_matches, agents_row_id);
    }

    /// delta upsert：stable AgentId 原位替换（新行追加），selection identity 不变。
    pub(super) fn upsert_row(&mut self, row: AgentOverviewRow) {
        self.list.upsert_row(row, agents_row_matches, agents_row_id);
    }

    /// delta remove：被移除的 selected agent 由 `restore_selected_id_or_clamp` 迁移。
    pub(super) fn remove_row(&mut self, agent_id: AgentId) {
        self.list
            .remove_row(agent_id, agents_row_matches, agents_row_id);
    }

    pub(super) fn move_selection(&mut self, direction: ListNavigationDirection) {
        self.list.move_selection(direction, agents_row_id);
    }

    pub(super) fn move_page(&mut self, direction: ListNavigationDirection, page_size: usize) {
        self.list.move_page(direction, page_size, agents_row_id);
    }

    pub(super) fn push_search_character(&mut self, character: char) {
        self.list
            .push_search_character(character, agents_row_matches, agents_row_id);
    }

    pub(super) fn backspace_search(&mut self) {
        self.list
            .backspace_search(agents_row_matches, agents_row_id);
    }

    pub(super) fn clear_search(&mut self) -> bool {
        self.list.clear_search(agents_row_matches, agents_row_id)
    }

    pub(super) fn exit_search(&mut self) -> bool {
        self.list.exit_search(agents_row_matches, agents_row_id)
    }

    pub(super) fn start_search(&mut self) {
        self.list.start_search();
    }

    pub(super) fn page_start(&self, page_size: usize) -> usize {
        self.list.page_start(page_size)
    }

    pub(super) fn page_indices(&self, page_size: usize) -> impl Iterator<Item = usize> + '_ {
        self.list.page_indices(page_size)
    }

    pub(super) fn page_number(&self, page_size: usize) -> usize {
        self.list.page_number(page_size)
    }

    pub(super) fn page_count(&self, page_size: usize) -> usize {
        self.list.page_count(page_size)
    }

    pub(super) fn select_visible_row(&mut self, page_size: usize, visible_offset: usize) -> bool {
        self.list
            .select_visible_row(page_size, visible_offset, agents_row_id)
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

    pub(super) fn row(&self, row_index: usize) -> Option<&AgentOverviewRow> {
        self.list.rows().get(row_index)
    }

    /// 按 AgentId 查找 frozen title（stop 确认提示等展示用）。
    pub(super) fn row_title(&self, agent_id: AgentId) -> Option<&str> {
        self.list
            .rows()
            .iter()
            .find(|row| row.agent_id == agent_id)
            .map(|row| row.title.as_str())
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

    /// 当前 surface 绑定的 AgentId（preview/transcript 共用）。
    pub(super) fn surface_agent_id(&self) -> Option<AgentId> {
        match self.surface.as_ref() {
            Some(AgentsPanelSurface::Preview { agent_id, .. })
            | Some(AgentsPanelSurface::Transcript { agent_id, .. }) => Some(*agent_id),
            None => None,
        }
    }

    /// stop 确认态是否仍然成立：确认目标仍是当前 selection 且仍可 stop。
    ///
    /// "selection 改变即取消"由此判定——delta remove 后 selection 按 clamp 规则迁移，
    /// 迁移结果不指向被确认的 AgentId 时确认自动失效。
    pub(super) fn stop_confirmation_still_valid(&self) -> bool {
        self.stop_confirmation.is_some_and(|confirmed| {
            self.list.selected_row().is_some_and(|row| {
                row.agent_id == confirmed && agent_status_is_stoppable(row.status)
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
