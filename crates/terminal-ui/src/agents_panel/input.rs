use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton};
use runtime_domain::agent::{
    AgentId, AgentObservationRejection, AgentObservationRequestId, AgentOverviewDelta,
    AgentOverviewDeltaKind, AgentOverviewSnapshot, AgentViewSnapshot,
};

use crate::{
    AppEffect, Model,
    agents_panel::{
        AgentsPanelAgentView, AgentsPanelPermissionChoice, AgentsPanelPillNavigation,
        AgentsPanelState, AgentsPanelStopConfirmation, AgentsPanelSurface,
        PendingAgentObservationStops, agent_status_is_running, agent_status_is_settled,
        agents_panel_list_page_size, agents_panel_rejection_text, groups::agents_panel_now_unix_ms,
        permission_choice::initial_permission_choice,
    },
    fullscreen_list_chrome::fullscreen_list_body_visible_offset_for_row,
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
        panel.stop_unavailable_notice = false;
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
        panel.stop_unavailable_notice = false;
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
            // view snapshot 到达可能让选中 agent 的折叠区首次可渲染。
            if let Some(panel) = self.agents_panel.as_mut() {
                panel.refresh_selected_activity_fold();
            }
            self.sync_agents_panel_transcript_surface(agent_id);
            self.sync_agents_panel_permission(agent_id);
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
            // 选中 agent 的 snapshot 内容更新即折叠区数据更新。
            if let Some(panel) = self.agents_panel.as_mut() {
                panel.refresh_selected_activity_fold();
            }
            self.sync_agents_panel_transcript_surface(agent_id);
            self.sync_agents_panel_permission(agent_id);
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
            // 滚轮位移按显示顺序逐行移动：分组可能在上一帧之后迁移，先归一行序。
            panel.refresh_display_order(agents_panel_now_unix_ms());
            panel.move_selection(direction);
        }
    }

    pub(crate) fn handle_agents_panel_key(&mut self, key: KeyEvent) -> OverlayInputResult {
        if self.agents_panel.is_none() {
            return OverlayInputResult::Ignored;
        }
        if self.agents_panel_transcript_active() {
            return self.handle_agents_panel_transcript_key(key);
        }
        self.handle_agents_panel_list_key(key)
    }

    fn handle_agents_panel_list_key(&mut self, key: KeyEvent) -> OverlayInputResult {
        // 分组可能在上一帧之后迁移（Just finished → Completed）：按键触发的
        // 导航/分页须按当前分组顺序计算，先归一行序（幂等，selection 不变）。
        if let Some(panel) = self.agents_panel.as_mut() {
            panel.refresh_display_order(agents_panel_now_unix_ms());
        }

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
                let page_size = agents_panel_list_page_size(self.height);
                if let Some(panel) = self.agents_panel.as_mut() {
                    panel.move_page(ListNavigationDirection::Previous, page_size);
                }
                OverlayInputResult::Handled
            }
            KeyCode::Right | KeyCode::Char('l') if key.modifiers.is_empty() => {
                let page_size = agents_panel_list_page_size(self.height);
                if let Some(panel) = self.agents_panel.as_mut() {
                    panel.move_page(ListNavigationDirection::Next, page_size);
                }
                OverlayInputResult::Handled
            }
            // Tab 切换选中行的活动折叠区；Space 与 Enter 同一 transcript surface 入口。
            KeyCode::Tab if key.modifiers.is_empty() => {
                if let Some(panel) = self.agents_panel.as_mut() {
                    panel.toggle_activity_fold_expanded();
                }
                OverlayInputResult::Handled
            }
            KeyCode::Char(' ') if key.modifiers.is_empty() => {
                self.open_agents_panel_transcript_surface()
            }
            KeyCode::Enter => self.open_agents_panel_transcript_surface(),
            KeyCode::Char('x') if key.modifiers.is_empty() => self.handle_agents_panel_stop_key(),
            _ => OverlayInputResult::Handled,
        }
    }

    /// `x` 二次确认：running 行是 stop（停止 subtree），settled 投影行是 delete
    /// （销毁并移除行）；第二次 `x` 且 selection 与动作类别未变才派发携带
    /// identity+generation 的 `StopAgent`。CleanupBlocked 行不可操作。
    fn handle_agents_panel_stop_key(&mut self) -> OverlayInputResult {
        // loading 期 stop 无目标可寻址：置位 footer 提示给出可见反馈，不静默吞掉按键。
        if self
            .agents_panel
            .as_ref()
            .is_some_and(|panel| panel.is_loading)
        {
            if let Some(panel) = self.agents_panel.as_mut() {
                panel.stop_unavailable_notice = true;
            }
            return OverlayInputResult::Handled;
        }
        let Some(panel) = self.agents_panel.as_ref() else {
            return OverlayInputResult::Handled;
        };
        // error 态的不可用由 body 的错误行自述，无需重复提示。
        if panel.error.is_some() {
            return OverlayInputResult::Handled;
        }
        let Some(row) = panel.selected_row() else {
            return OverlayInputResult::Handled;
        };
        let agent_id = row.agent_id;
        // 动作类别由行的当前状态决定；Stop/Delete 派发的是同一 runtime 命令，
        // runtime 侧按行 lifecycle 区分停止与删除。
        let confirmation = if agent_status_is_running(row.status) {
            AgentsPanelStopConfirmation::Stop(agent_id)
        } else if agent_status_is_settled(row.status) {
            AgentsPanelStopConfirmation::Delete(agent_id)
        } else {
            return OverlayInputResult::Handled;
        };
        let Some(generation) = panel.generation else {
            return OverlayInputResult::Handled;
        };
        if panel.stop_confirmation == Some(confirmation) {
            self.clear_agents_panel_stop_confirmation();
            OverlayInputResult::Effect(AppEffect::StopAgent {
                agent_id,
                generation,
            })
        } else {
            self.set_agents_panel_stop_confirmation(confirmation);
            OverlayInputResult::Handled
        }
    }

    fn clear_agents_panel_stop_confirmation(&mut self) {
        if let Some(panel) = self.agents_panel.as_mut() {
            panel.stop_confirmation = None;
        }
    }

    fn set_agents_panel_stop_confirmation(&mut self, confirmation: AgentsPanelStopConfirmation) {
        if let Some(panel) = self.agents_panel.as_mut() {
            panel.stop_confirmation = Some(confirmation);
        }
    }

    fn agents_panel_selected_agent_id(&self) -> Option<AgentId> {
        let panel = self.agents_panel.as_ref()?;
        if panel.is_loading || panel.error.is_some() {
            return None;
        }
        panel.selected_row().map(|row| row.agent_id)
    }

    /// `Enter`/`Space` 进入 child transcript surface：完整 transcript 视图（Markdown
    /// 管线渲染）+ permission 交互面；Space 与 Enter 是同一入口。
    fn open_agents_panel_transcript_surface(&mut self) -> OverlayInputResult {
        let Some(agent_id) = self.agents_panel_selected_agent_id() else {
            return OverlayInputResult::Handled;
        };
        match self.open_agents_panel_transcript_for_agent(agent_id) {
            Some(request_id) => OverlayInputResult::Effect(AppEffect::ObserveAgentTranscript {
                request_id,
                agent_id,
            }),
            None => OverlayInputResult::Handled,
        }
    }

    /// 为指定 agent 打开 transcript surface（`Space`/`Enter` 与 Agent approval pill
    /// 导航共用）。
    ///
    /// permission 区块交互态按 record 当前 snapshot 初始化；snapshot 未就绪时由
    /// snapshot 应用路径的 reconcile 接管。返回需要派发的 observation 请求。
    pub(crate) fn open_agents_panel_transcript_for_agent(
        &mut self,
        agent_id: AgentId,
    ) -> Option<AgentObservationRequestId> {
        let dispatch_request_id = self.stage_agents_panel_agent_view(agent_id);
        let permission_choice = self
            .agents_panel
            .as_ref()
            .and_then(|panel| panel.agent_view_for_agent(agent_id))
            .and_then(|record| record.snapshot.as_ref())
            .map(|snapshot| initial_permission_choice(snapshot.preview.permission.as_ref()))
            .unwrap_or(AgentsPanelPermissionChoice::None);
        self.install_agents_panel_transcript_surface(agent_id, permission_choice);
        dispatch_request_id
    }

    /// 执行 pill 导航意图：预选目标 agent；`OpenTranscript` 追加打开 transcript surface。
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
            AgentsPanelPillNavigation::OpenTranscript { agent_id }
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
            AgentsPanelPillNavigation::OpenTranscript { agent_id } => self
                .open_agents_panel_transcript_for_agent(agent_id)
                .map(|request_id| AppEffect::ObserveAgentTranscript {
                    request_id,
                    agent_id,
                }),
        }
    }

    /// 安装 transcript surface；record 已有快照时立即构建 transcript 并贴底，
    /// 未就绪时保持空 transcript，由渲染层呈现 loading。
    fn install_agents_panel_transcript_surface(
        &mut self,
        agent_id: AgentId,
        permission_choice: AgentsPanelPermissionChoice,
    ) {
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
        let content_height = self.agents_panel_surface_content_height();
        if let Some(panel) = self.agents_panel.as_mut() {
            let mut surface_transcript = transcript
                .map(Box::new)
                .unwrap_or_else(|| Box::new(Transcript::new(palette, working_dir)));
            let mut overlay = TranscriptOverlayState::new();
            overlay.scroll_offset =
                latest_transcript_bottom_offset(&mut surface_transcript, content_height);
            panel.surface = Some(AgentsPanelSurface {
                agent_id,
                transcript: surface_transcript,
                overlay,
                is_following_bottom: true,
                permission_choice,
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
        // body 首行是列头行：不是可选目标，点击直接吞掉。
        let Some(physical_offset) = visible_offset.checked_sub(1) else {
            return OverlayInputResult::Handled;
        };
        let page_size = agents_panel_list_page_size(self.height);
        // 点击命中换算须与本帧渲染的分组布局一致：同一墙钟先归一行序再换算。
        let now_ms = agents_panel_now_unix_ms();
        if let Some(panel) = self.agents_panel.as_mut() {
            panel.refresh_display_order(now_ms);
        }
        let selection_before = self
            .agents_panel
            .as_ref()
            .and_then(|panel| panel.selected_row())
            .map(|row| row.agent_id);
        if let Some(panel) = self.agents_panel.as_mut() {
            // 组头行与折叠行计入物理行预算：点击命中行须与渲染布局一致。
            panel.select_physical_body_line(
                page_size,
                physical_offset,
                usize::from(self.width),
                now_ms,
            );
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
