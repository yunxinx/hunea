use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton};
use runtime_domain::agent::{
    AgentId, AgentObservationRejection, AgentObservationRequestId, AgentOverviewDelta,
    AgentOverviewDeltaKind, AgentOverviewSnapshot, AgentViewSnapshot,
};

use crate::{
    AppEffect, Model,
    agents_panel::{
        AgentsPanelAgentView, AgentsPanelPillNavigation, AgentsPanelPreviewPermissionChoice,
        AgentsPanelState, AgentsPanelSurface, PendingAgentObservationStops,
        agent_status_is_stoppable, agents_panel_rejection_text,
        preview::initial_preview_permission_choice,
    },
    fullscreen_list_chrome::{
        fullscreen_list_body_visible_offset_for_row, fullscreen_list_page_size_for_height,
    },
    list_selection::ListNavigationDirection,
    overlay_input_result::OverlayInputResult,
    text_search::is_picker_search_text_key,
    transcript::{Transcript, latest_preview_offset as latest_transcript_bottom_offset},
    transcript_overlay::TranscriptOverlayState,
};

impl Model {
    pub(crate) fn agents_panel_active(&self) -> bool {
        self.agents_panel.is_some()
    }

    /// 打开 panel 进入 loading 态并分配 observation 请求标识；
    /// 由 runner 持 request_id 派发 `ObserveAgents`。
    ///
    /// 重复打开视为先关闭：直接覆盖旧 state 会跳过 stop 置位，
    /// 让 runtime 侧旧 observation 在 panel 关闭前一直存活。
    pub(crate) fn open_agents_panel_loading(&mut self) -> AgentObservationRequestId {
        self.close_agents_panel();
        let request_id = self.next_agent_observation_request_id();
        self.agents_panel = Some(AgentsPanelState::loading(request_id));
        self.close_composer_attached_ui();
        request_id
    }

    /// loading 态的 snapshot 请求是否与回包 request_id 匹配（陈旧回包丢弃的谓词）。
    pub(crate) fn agents_panel_overview_request_matches(
        &self,
        request_id: AgentObservationRequestId,
    ) -> bool {
        self.agents_panel
            .as_ref()
            .is_some_and(|panel| panel.is_loading && panel.pending_request_id == Some(request_id))
    }

    pub(crate) fn apply_agents_overview_snapshot(
        &mut self,
        request_id: AgentObservationRequestId,
        snapshot: AgentOverviewSnapshot,
    ) {
        let Some(mut panel) = self.agents_panel.take() else {
            return;
        };
        if !panel.is_loading || panel.pending_request_id != Some(request_id) {
            self.agents_panel = Some(panel);
            return;
        }
        panel.observation_id = Some(snapshot.observation_id);
        panel.generation = Some(snapshot.generation);
        panel.pending_request_id = None;
        panel.is_loading = false;
        panel.error = None;
        panel.replace_rows(snapshot.rows);
        self.agents_panel = Some(panel);
        // pill 导航意图消费：panel 打开是异步的，snapshot 投影建立后才能定位目标。
        if let Some(navigation) = self.agents_panel_pill_navigation.take()
            && let Some(AppEffect::ObserveAgentTranscript {
                request_id,
                agent_id,
            }) = self.apply_agents_panel_pill_navigation(navigation)
        {
            // 事件应用点没有 Effect 通道：暂存给 runner effect 循环消费
            //（对齐 pending_stop_observing_agents 的 pending-flag 模式）。
            self.pending_agent_view_observe_requests
                .push((request_id, agent_id));
        }
    }

    /// 同步错误路径（runtime port Err）：按 pending request_id 匹配后呈现于 panel。
    pub(crate) fn show_agents_panel_error(
        &mut self,
        request_id: AgentObservationRequestId,
        message: &str,
    ) {
        let Some(mut panel) = self.agents_panel.take() else {
            return;
        };
        if !panel.is_loading || panel.pending_request_id != Some(request_id) {
            self.agents_panel = Some(panel);
            return;
        }
        panel.is_loading = false;
        panel.pending_request_id = None;
        panel.error = Some(message.to_string());
        panel.replace_rows(Vec::new());
        self.agents_panel = Some(panel);
    }

    /// per-agent view 请求的同步错误路径：定位 pending 记录并呈现于 surface。
    pub(crate) fn show_agents_panel_agent_view_error(
        &mut self,
        request_id: AgentObservationRequestId,
        message: &str,
    ) {
        if let Some(panel) = self.agents_panel.as_mut()
            && let Some(record) = panel.agent_view_with_pending_request_mut(request_id)
        {
            record.pending_request_id = None;
            record.error = Some(message.to_string());
        }
    }

    /// overview 增量：observation_id 与 generation 完全匹配才应用；
    /// 不匹配说明 runtime 已被替换，确认态随之取消（fail closed）。
    pub(crate) fn apply_agents_overview_delta(&mut self, delta: AgentOverviewDelta) {
        let Some(panel) = self.agents_panel.as_mut() else {
            return;
        };
        if panel.observation_id != Some(delta.observation_id)
            || panel.generation != Some(delta.generation)
        {
            panel.stop_confirmation = None;
            return;
        }
        match delta.kind {
            AgentOverviewDeltaKind::Upsert(row) => panel.upsert_row(row),
            AgentOverviewDeltaKind::Remove { agent_id } => panel.remove_row(agent_id),
        }
        // selection 迁移或目标进入终态后，确认态不再指向可 stop 的 selection，取消。
        if !panel.stop_confirmation_still_valid() {
            panel.stop_confirmation = None;
        }
    }

    /// observation 拒绝按 request_id 匹配：overview 请求、per-agent view 请求各归其位。
    pub(crate) fn apply_agent_observation_rejected(
        &mut self,
        request_id: AgentObservationRequestId,
        reason: AgentObservationRejection,
    ) {
        if self.agents_panel_overview_request_matches(request_id) {
            let rejection_text = agents_panel_rejection_text(reason);
            self.show_agents_panel_error(request_id, &rejection_text);
            return;
        }
        if let Some(panel) = self.agents_panel.as_mut()
            && let Some(record) = panel.agent_view_with_pending_request_mut(request_id)
        {
            record.pending_request_id = None;
            record.error = Some(agents_panel_rejection_text(reason));
            return;
        }
        // panel 已关闭的 pending 请求：rejection 表示 observation 未建立，无需补 stop。
        self.pending_agent_view_stop_requests
            .retain(|pending| *pending != request_id);
    }

    /// per-agent view 初始回包：按 pending request_id 匹配记录并建立 observation 绑定。
    pub(crate) fn apply_agent_view_snapshot_loaded(
        &mut self,
        request_id: AgentObservationRequestId,
        snapshot: AgentViewSnapshot,
    ) {
        let agent_id = snapshot.transcript.agent_id;
        let observation_id = snapshot.observation_id;
        let generation = snapshot.generation;
        let mut matched_live_record = false;
        if let Some(panel) = self.agents_panel.as_mut()
            && let Some(record) = panel.agent_view_with_pending_request_mut(request_id)
        {
            record.pending_request_id = None;
            record.observation_id = Some(observation_id);
            record.generation = Some(generation);
            record.snapshot = Some(snapshot);
            record.error = None;
            matched_live_record = true;
        }
        if matched_live_record {
            self.sync_agents_panel_transcript_surface(agent_id);
            self.sync_agents_panel_preview_permission(agent_id);
            return;
        }
        // panel 已关闭（或记录被覆盖）后到达的回包：若请求在待注销列表中，
        // 现在才知道 observation_id，补一次 stop 防止隐藏订阅泄漏。
        let position = self
            .pending_agent_view_stop_requests
            .iter()
            .position(|pending| *pending == request_id);
        if let Some(position) = position {
            self.pending_agent_view_stop_requests.remove(position);
            let mut stops = PendingAgentObservationStops::default();
            stops.agent_views.push((observation_id, generation));
            self.stage_pending_agent_observation_stops(stops);
        }
    }

    /// per-agent view 增量：observation_id 与 generation 匹配才整快照替换。
    pub(crate) fn apply_agent_view_updated(&mut self, snapshot: AgentViewSnapshot) {
        let agent_id = snapshot.transcript.agent_id;
        let mut matched = false;
        if let Some(panel) = self.agents_panel.as_mut()
            && let Some(record) = panel.agent_views.iter_mut().find(|record| {
                record.observation_id == Some(snapshot.observation_id)
                    && record.generation == Some(snapshot.generation)
            })
        {
            record.snapshot = Some(snapshot);
            matched = true;
        }
        if matched {
            self.sync_agents_panel_transcript_surface(agent_id);
            self.sync_agents_panel_preview_permission(agent_id);
        }
    }

    /// 四条关闭路径（panel Esc、attention pill 关层、session resume、reset）
    /// 统一收敛到这里：清空 generation-bound state 并置 pending stop，
    /// 由 runner 消费派发 `StopObserving*`（runtime 侧幂等）。
    pub(crate) fn close_agents_panel(&mut self) {
        let Some(panel) = self.agents_panel.take() else {
            return;
        };
        let mut stops = PendingAgentObservationStops::default();
        if let (Some(observation_id), Some(generation)) = (panel.observation_id, panel.generation) {
            stops.overview = Some((observation_id, generation));
        }
        for record in &panel.agent_views {
            if let (Some(observation_id), Some(generation)) =
                (record.observation_id, record.generation)
            {
                stops.agent_views.push((observation_id, generation));
            } else if let Some(request_id) = record.pending_request_id {
                // observation id 未知：等回包送达后按 request_id 补 stop。
                self.pending_agent_view_stop_requests.push(request_id);
            }
        }
        self.stage_pending_agent_observation_stops(stops);
    }

    /// runner 消费：取出待派发的 observation 注销集合。
    pub(crate) fn take_pending_agent_observation_stops(
        &mut self,
    ) -> Option<PendingAgentObservationStops> {
        let pending = self.pending_stop_observing_agents.take()?;
        (!pending.is_empty()).then_some(pending)
    }

    /// runner 消费：取出事件应用点暂存的 per-agent view observe 请求。
    pub(crate) fn take_pending_agent_view_observe_requests(
        &mut self,
    ) -> Vec<(AgentObservationRequestId, AgentId)> {
        std::mem::take(&mut self.pending_agent_view_observe_requests)
    }

    #[cfg(test)]
    pub(crate) fn agents_panel_pending_overview_request_id_for_test(
        &self,
    ) -> Option<AgentObservationRequestId> {
        self.agents_panel
            .as_ref()
            .and_then(|panel| panel.pending_request_id)
    }

    #[cfg(test)]
    pub(crate) fn agents_panel_observation_id_for_test(
        &self,
    ) -> Option<runtime_domain::agent::AgentObservationId> {
        self.agents_panel
            .as_ref()
            .and_then(|panel| panel.observation_id)
    }

    #[cfg(test)]
    pub(crate) fn agents_panel_selected_agent_id_for_test(&self) -> Option<AgentId> {
        self.agents_panel
            .as_ref()
            .and_then(|panel| panel.selected_row())
            .map(|row| row.agent_id)
    }

    fn stage_pending_agent_observation_stops(&mut self, stops: PendingAgentObservationStops) {
        if stops.is_empty() {
            return;
        }
        self.pending_stop_observing_agents
            .get_or_insert_with(PendingAgentObservationStops::default)
            .merge(stops);
    }

    pub(crate) fn move_agents_panel_selection_by_delta(&mut self, delta: isize) {
        let Some(direction) = ListNavigationDirection::from_delta(delta) else {
            return;
        };
        if let Some(panel) = self.agents_panel.as_mut() {
            panel.move_selection(direction);
        }
    }

    pub(crate) fn handle_agents_panel_key(&mut self, key: KeyEvent) -> OverlayInputResult {
        if self.agents_panel.is_none() {
            return OverlayInputResult::Ignored;
        }
        if self.agents_panel_preview_active() {
            return self.handle_agents_panel_preview_key(key);
        }
        if self.agents_panel_transcript_active() {
            return self.handle_agents_panel_transcript_key(key);
        }
        self.handle_agents_panel_list_key(key)
    }

    fn handle_agents_panel_list_key(&mut self, key: KeyEvent) -> OverlayInputResult {
        // 确认态只对下一次 `x` 有效：任何其他按键（选区移动、进入 surface、搜索）
        // 都取消，防止跨 selection 生效。
        let is_plain_stop_key = key.code == KeyCode::Char('x') && key.modifiers.is_empty();
        if !is_plain_stop_key {
            self.clear_agents_panel_stop_confirmation();
        }

        let is_searching = self
            .agents_panel
            .as_ref()
            .is_some_and(AgentsPanelState::is_searching);

        match key.code {
            KeyCode::Esc if key.modifiers.is_empty() => {
                if let Some(panel) = self.agents_panel.as_mut()
                    && panel.exit_search()
                {
                    return OverlayInputResult::Handled;
                }
                self.close_agents_panel();
                // panel 被用户关闭：未消费的 pill 导航意图作废（fail closed）。
                self.agents_panel_pill_navigation = None;
                OverlayInputResult::Handled
            }
            KeyCode::Char(character) if is_searching && is_picker_search_text_key(&key) => {
                if let Some(panel) = self.agents_panel.as_mut() {
                    panel.push_search_character(character);
                }
                OverlayInputResult::Handled
            }
            KeyCode::Backspace if key.modifiers.is_empty() => {
                if let Some(panel) = self.agents_panel.as_mut() {
                    panel.backspace_search();
                }
                OverlayInputResult::Handled
            }
            KeyCode::Char('u')
                if key.modifiers.contains(KeyModifiers::CONTROL)
                    && !key.modifiers.contains(KeyModifiers::ALT) =>
            {
                if let Some(panel) = self.agents_panel.as_mut() {
                    panel.clear_search();
                }
                OverlayInputResult::Handled
            }
            KeyCode::Char('/') if key.modifiers.is_empty() => {
                if let Some(panel) = self.agents_panel.as_mut() {
                    panel.start_search();
                }
                OverlayInputResult::Handled
            }
            KeyCode::Up | KeyCode::Char('k') if key.modifiers.is_empty() => {
                if let Some(panel) = self.agents_panel.as_mut() {
                    panel.move_selection(ListNavigationDirection::Previous);
                }
                OverlayInputResult::Handled
            }
            KeyCode::Down | KeyCode::Char('j') if key.modifiers.is_empty() => {
                if let Some(panel) = self.agents_panel.as_mut() {
                    panel.move_selection(ListNavigationDirection::Next);
                }
                OverlayInputResult::Handled
            }
            KeyCode::Left | KeyCode::Char('h') if key.modifiers.is_empty() => {
                let page_size = fullscreen_list_page_size_for_height(self.height);
                if let Some(panel) = self.agents_panel.as_mut() {
                    panel.move_page(ListNavigationDirection::Previous, page_size);
                }
                OverlayInputResult::Handled
            }
            KeyCode::Right | KeyCode::Char('l') if key.modifiers.is_empty() => {
                let page_size = fullscreen_list_page_size_for_height(self.height);
                if let Some(panel) = self.agents_panel.as_mut() {
                    panel.move_page(ListNavigationDirection::Next, page_size);
                }
                OverlayInputResult::Handled
            }
            KeyCode::Char(' ') if key.modifiers.is_empty() => self.open_agents_panel_preview(),
            KeyCode::Enter => self.open_agents_panel_transcript_surface(),
            KeyCode::Char('x') if key.modifiers.is_empty() => self.handle_agents_panel_stop_key(),
            _ => OverlayInputResult::Handled,
        }
    }

    /// `x` 二次确认：仅对 selected running child 生效；第二次 `x` 且 selection 未变
    /// 才派发携带 identity+generation 的 `StopAgent`。
    fn handle_agents_panel_stop_key(&mut self) -> OverlayInputResult {
        let Some(panel) = self.agents_panel.as_ref() else {
            return OverlayInputResult::Handled;
        };
        if panel.is_loading || panel.error.is_some() {
            return OverlayInputResult::Handled;
        }
        let Some(row) = panel.selected_row() else {
            return OverlayInputResult::Handled;
        };
        if !agent_status_is_stoppable(row.status) {
            return OverlayInputResult::Handled;
        }
        let agent_id = row.agent_id;
        let Some(generation) = panel.generation else {
            return OverlayInputResult::Handled;
        };
        if panel.stop_confirmation == Some(agent_id) {
            self.clear_agents_panel_stop_confirmation();
            OverlayInputResult::Effect(AppEffect::StopAgent {
                agent_id,
                generation,
            })
        } else {
            self.set_agents_panel_stop_confirmation(agent_id);
            OverlayInputResult::Handled
        }
    }

    fn clear_agents_panel_stop_confirmation(&mut self) {
        if let Some(panel) = self.agents_panel.as_mut() {
            panel.stop_confirmation = None;
        }
    }

    fn set_agents_panel_stop_confirmation(&mut self, agent_id: AgentId) {
        if let Some(panel) = self.agents_panel.as_mut() {
            panel.stop_confirmation = Some(agent_id);
        }
    }

    fn agents_panel_selected_agent_id(&self) -> Option<AgentId> {
        let panel = self.agents_panel.as_ref()?;
        if panel.is_loading || panel.error.is_some() {
            return None;
        }
        panel.selected_row().map(|row| row.agent_id)
    }

    /// `Space` 打开 quick preview 基础形态：只读、仅返回，无 cancel/interrupt/steer。
    fn open_agents_panel_preview(&mut self) -> OverlayInputResult {
        let Some(agent_id) = self.agents_panel_selected_agent_id() else {
            return OverlayInputResult::Handled;
        };
        match self.open_agents_panel_preview_for_agent(agent_id) {
            Some(request_id) => OverlayInputResult::Effect(AppEffect::ObserveAgentTranscript {
                request_id,
                agent_id,
            }),
            None => OverlayInputResult::Handled,
        }
    }

    /// 为指定 agent 打开 quick preview surface（`Space` 与 Agent approval pill 导航共用）。
    ///
    /// permission 区块交互态按 record 当前 snapshot 初始化；snapshot 未就绪时由
    /// snapshot 应用路径的 reconcile 接管。返回需要派发的 observation 请求。
    pub(crate) fn open_agents_panel_preview_for_agent(
        &mut self,
        agent_id: AgentId,
    ) -> Option<AgentObservationRequestId> {
        let dispatch_request_id = self.stage_agents_panel_agent_view(agent_id);
        let permission_choice = self
            .agents_panel
            .as_ref()
            .and_then(|panel| panel.agent_view_for_agent(agent_id))
            .and_then(|record| record.snapshot.as_ref())
            .map(|snapshot| {
                initial_preview_permission_choice(snapshot.preview.permission.as_ref())
            });
        if let Some(panel) = self.agents_panel.as_mut() {
            panel.surface = Some(AgentsPanelSurface::Preview {
                agent_id,
                scroll_offset: 0,
                permission_choice: permission_choice
                    .unwrap_or(AgentsPanelPreviewPermissionChoice::None),
            });
        }
        dispatch_request_id
    }

    /// 执行 pill 导航意图：预选目标 agent；`OpenPreview` 追加打开 preview surface。
    ///
    /// 目标 agent 不在当前投影时意图失效停在 list（fail closed，不猜临近行）；
    /// rows 尚未建立（loading）时意图保留，等 snapshot 应用点再消费。
    /// 返回值是"需要派发的 ObserveAgentTranscript"——点击路径直接作为 Effect
    /// 返回，snapshot 应用路径暂存给 runner 消费。
    pub(crate) fn apply_agents_panel_pill_navigation(
        &mut self,
        navigation: AgentsPanelPillNavigation,
    ) -> Option<AppEffect> {
        let agent_id = match navigation {
            AgentsPanelPillNavigation::OpenPreview { agent_id }
            | AgentsPanelPillNavigation::Preselect { agent_id } => agent_id,
        };
        let panel_ready = self
            .agents_panel
            .as_ref()
            .is_some_and(|panel| !panel.is_loading && panel.error.is_none());
        if !panel_ready {
            // rows 未建立（loading）保留意图延后消费；error/已关则新旧意图一并作废
            //（fail closed：残留的旧意图不得在下一次手动重开时突然导航）。
            if self
                .agents_panel
                .as_ref()
                .is_some_and(|panel| panel.is_loading)
            {
                self.agents_panel_pill_navigation = Some(navigation);
            } else {
                self.agents_panel_pill_navigation = None;
            }
            return None;
        }
        let selected = self
            .agents_panel
            .as_mut()
            .is_some_and(|panel| panel.select_agent(agent_id));
        if !selected {
            return None;
        }
        match navigation {
            AgentsPanelPillNavigation::Preselect { .. } => None,
            AgentsPanelPillNavigation::OpenPreview { agent_id } => self
                .open_agents_panel_preview_for_agent(agent_id)
                .map(|request_id| AppEffect::ObserveAgentTranscript {
                    request_id,
                    agent_id,
                }),
        }
    }

    /// `Enter` 进入 child transcript surface：消费 committed delivery-safe items，
    /// 渲染复用 transcript overlay 视图。
    fn open_agents_panel_transcript_surface(&mut self) -> OverlayInputResult {
        let Some(agent_id) = self.agents_panel_selected_agent_id() else {
            return OverlayInputResult::Handled;
        };
        let dispatch_request_id = self.stage_agents_panel_agent_view(agent_id);
        self.install_agents_panel_transcript_surface(agent_id);
        match dispatch_request_id {
            Some(request_id) => OverlayInputResult::Effect(AppEffect::ObserveAgentTranscript {
                request_id,
                agent_id,
            }),
            None => OverlayInputResult::Handled,
        }
    }

    /// 安装 transcript surface；record 已有快照时立即构建 transcript 并贴底，
    /// 未就绪时保持空 transcript，由渲染层呈现 loading。
    fn install_agents_panel_transcript_surface(&mut self, agent_id: AgentId) {
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
        let transcript = items.map(|items| self.transcript_from_agent_items(&items));
        let palette = self.palette;
        let working_dir = self.working_dir.clone();
        let content_height = self.transcript_overlay_content_height();
        if let Some(panel) = self.agents_panel.as_mut() {
            let mut surface_transcript = transcript
                .map(Box::new)
                .unwrap_or_else(|| Box::new(Transcript::new(palette, working_dir)));
            let mut overlay = TranscriptOverlayState::new();
            overlay.scroll_offset =
                latest_transcript_bottom_offset(&mut surface_transcript, content_height);
            panel.surface = Some(AgentsPanelSurface::Transcript {
                agent_id,
                transcript: surface_transcript,
                overlay,
                is_following_bottom: true,
            });
        }
    }

    /// 为 surface 打开准备 per-agent view observation 记录。
    ///
    /// 已有快照的记录直接复用（不建第二个 observer）；仍在 loading 或上次失败的
    /// 记录重新派发一次请求，旧 pending 请求转入待注销列表等回包后补 stop。
    fn stage_agents_panel_agent_view(
        &mut self,
        agent_id: AgentId,
    ) -> Option<AgentObservationRequestId> {
        let has_snapshot = self
            .agents_panel
            .as_ref()
            .and_then(|panel| panel.agent_view_for_agent(agent_id))
            .is_some_and(|record| record.snapshot.is_some());
        if has_snapshot {
            return None;
        }
        let request_id = self.next_agent_observation_request_id();
        let existing_record = self
            .agents_panel
            .as_mut()
            .and_then(|panel| panel.agent_view_for_agent_mut(agent_id));
        match existing_record {
            Some(record) => {
                if let Some(previous) = record.pending_request_id.replace(request_id) {
                    self.pending_agent_view_stop_requests.push(previous);
                }
                record.error = None;
            }
            None => {
                if let Some(panel) = self.agents_panel.as_mut() {
                    panel.agent_views.push(AgentsPanelAgentView {
                        agent_id,
                        pending_request_id: Some(request_id),
                        observation_id: None,
                        generation: None,
                        snapshot: None,
                        error: None,
                    });
                }
            }
        }
        Some(request_id)
    }

    /// surface 只回到 list；record 保留以便复用 observation（panel 关闭时统一注销）。
    pub(super) fn close_agents_panel_surface(&mut self) {
        if let Some(panel) = self.agents_panel.as_mut() {
            panel.surface = None;
        }
    }

    pub(crate) fn handle_agents_panel_mouse_down(
        &mut self,
        button: MouseButton,
        _column: u16,
        row: u16,
    ) -> OverlayInputResult {
        if !self.agents_panel_active() {
            return OverlayInputResult::Ignored;
        }
        if button != MouseButton::Left {
            return OverlayInputResult::Handled;
        }
        if self
            .agents_panel
            .as_ref()
            .is_some_and(|panel| panel.surface.is_some())
        {
            // surface 模式没有行点选，吞掉点击避免落入下层。
            return OverlayInputResult::Handled;
        }
        let Some(visible_offset) = fullscreen_list_body_visible_offset_for_row(self.height, row)
        else {
            return OverlayInputResult::Handled;
        };
        let page_size = fullscreen_list_page_size_for_height(self.height);
        let selection_before = self
            .agents_panel
            .as_ref()
            .and_then(|panel| panel.selected_row())
            .map(|row| row.agent_id);
        if let Some(panel) = self.agents_panel.as_mut() {
            panel.select_visible_row(page_size, visible_offset);
        }
        let selection_after = self
            .agents_panel
            .as_ref()
            .and_then(|panel| panel.selected_row())
            .map(|row| row.agent_id);
        if selection_before != selection_after {
            self.clear_agents_panel_stop_confirmation();
        }
        OverlayInputResult::Handled
    }
}
