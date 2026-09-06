use crossterm::event::MouseButton;
use runtime_domain::agent::{
    AgentId, AgentObservationRequestId, AgentOverviewDeltaKind, AgentProjectionStatus,
};
use runtime_domain::session::{
    RuntimeCommand, RuntimeEvent, RuntimeTarget, SessionResumePayload, TranscriptReplayItem,
    TranscriptReplayRole,
};

use crate::{
    AppEffect, Model, StartupBannerOptions,
    agents_panel::PendingAgentObservationStops,
    modal_layer::ModalLayer,
    runner::{
        NoopUiRuntimePort, dispatch_agents_observation_stops_if_needed,
        run_open_agents_panel_effect,
    },
    runtime::RuntimeEventApply,
};

use super::common::{
    FIXTURE_GENERATION, FIXTURE_OBSERVATION_ID, RecordingRuntimePort, apply_overview_delta,
    apply_overview_rejection, apply_overview_snapshot, apply_view_snapshot_loaded, press_key,
    ready_panel_model, sample_rows, view_snapshot,
};

#[test]
fn open_agents_panel_starts_loading_with_pending_request() {
    let mut model = Model::new(StartupBannerOptions::default());
    let request_id = model.open_agents_panel_loading();

    assert!(model.agents_panel_active());
    let panel = model.agents_panel.as_ref().unwrap();
    assert!(panel.is_loading);
    assert_eq!(panel.pending_request_id, Some(request_id));
    assert_eq!(request_id.get(), 1);
}

#[test]
fn snapshot_with_matching_request_builds_projection() {
    let mut model = Model::new(StartupBannerOptions::default());
    let request_id = model.open_agents_panel_loading();

    apply_overview_snapshot(&mut model, request_id, sample_rows());

    let panel = model.agents_panel.as_ref().unwrap();
    assert!(!panel.is_loading);
    assert!(panel.pending_request_id.is_none());
    assert_eq!(
        panel.observation_id.map(|id| id.get()),
        Some(FIXTURE_OBSERVATION_ID)
    );
    assert_eq!(
        panel.generation.map(|generation| generation.get()),
        Some(FIXTURE_GENERATION)
    );
    assert_eq!(panel.filtered_count(), 2);
    // 首行被选中，selection 绑定 stable AgentId。
    assert_eq!(
        panel.selected_row().map(|row| row.agent_id),
        Some(AgentId::new(2))
    );
}

#[test]
fn snapshot_from_previous_open_does_not_replace_current_panel() {
    let mut model = Model::new(StartupBannerOptions::default());
    let stale_request_id = model.open_agents_panel_loading();
    let current_request_id = model.open_agents_panel_loading();

    apply_overview_snapshot(&mut model, stale_request_id, sample_rows());

    let panel = model.agents_panel.as_ref().unwrap();
    assert!(panel.is_loading);
    assert_eq!(
        panel.pending_request_id,
        Some(current_request_id),
        "stale request snapshot must not build a projection"
    );
}

#[test]
fn late_snapshot_after_close_does_not_reopen_panel() {
    let mut model = Model::new(StartupBannerOptions::default());
    let request_id = model.open_agents_panel_loading();
    press_key(&mut model, crossterm::event::KeyCode::Esc);
    assert!(!model.agents_panel_active());

    apply_overview_snapshot(&mut model, request_id, sample_rows());

    assert!(
        !model.agents_panel_active(),
        "late snapshot must not reopen the panel"
    );
}

#[test]
fn rejection_with_matching_request_shows_closed_error() {
    let mut model = Model::new(StartupBannerOptions::default());
    let request_id = model.open_agents_panel_loading();

    apply_overview_rejection(
        &mut model,
        request_id,
        runtime_domain::agent::AgentObservationRejection::StaleGeneration,
    );

    let panel = model.agents_panel.as_ref().unwrap();
    assert!(!panel.is_loading);
    assert_eq!(
        panel.error.as_deref(),
        Some("Agent runtime was replaced; reopen /agents")
    );
}

#[test]
fn rejection_for_other_request_is_dropped() {
    let mut model = ready_panel_model();
    let panel = model.agents_panel.as_ref().unwrap();
    let established_observation = panel.observation_id;

    apply_overview_rejection(
        &mut model,
        AgentObservationRequestId::new(999),
        runtime_domain::agent::AgentObservationRejection::UnknownAgent,
    );

    let panel = model.agents_panel.as_ref().unwrap();
    assert!(panel.error.is_none());
    assert_eq!(panel.observation_id, established_observation);
    assert_eq!(panel.filtered_count(), 2);
}

#[test]
fn delta_with_matching_observation_applies_upsert() {
    let mut model = ready_panel_model();

    apply_overview_delta(
        &mut model,
        AgentOverviewDeltaKind::Upsert(super::common::overview_row(
            2,
            "research task",
            AgentProjectionStatus::WaitingPermission,
        )),
    );

    let panel = model.agents_panel.as_ref().unwrap();
    assert_eq!(panel.filtered_count(), 2);
    let updated = panel
        .list
        .rows()
        .iter()
        .find(|row| row.agent_id == AgentId::new(2))
        .unwrap();
    assert_eq!(updated.status, AgentProjectionStatus::WaitingPermission);
    // selection identity 不随 delta 改变。
    assert_eq!(
        panel.selected_row().map(|row| row.agent_id),
        Some(AgentId::new(2))
    );
}

#[test]
fn delta_with_stale_generation_is_dropped_and_cancels_confirmation() {
    let mut model = ready_panel_model();
    press_key(&mut model, crossterm::event::KeyCode::Char('x'));
    assert!(
        model
            .agents_panel
            .as_ref()
            .unwrap()
            .stop_confirmation
            .is_some()
    );

    super::common::apply_overview_delta_with_identity(
        &mut model,
        AgentOverviewDeltaKind::Upsert(super::common::overview_row(
            2,
            "renamed task",
            AgentProjectionStatus::Failed,
        )),
        FIXTURE_OBSERVATION_ID,
        2,
    );

    let panel = model.agents_panel.as_ref().unwrap();
    assert!(
        panel.stop_confirmation.is_none(),
        "stale generation delta must cancel the stop confirmation"
    );
    let row = panel
        .list
        .rows()
        .iter()
        .find(|row| row.agent_id == AgentId::new(2))
        .unwrap();
    assert_eq!(
        row.status,
        AgentProjectionStatus::Working,
        "stale generation delta must not update the panel"
    );
}

#[test]
fn delta_with_foreign_observation_id_is_dropped() {
    let mut model = ready_panel_model();

    super::common::apply_overview_delta_with_identity(
        &mut model,
        AgentOverviewDeltaKind::Upsert(super::common::overview_row(
            2,
            "renamed task",
            AgentProjectionStatus::Failed,
        )),
        99,
        FIXTURE_GENERATION,
    );

    let panel = model.agents_panel.as_ref().unwrap();
    assert_eq!(panel.filtered_count(), 2);
    let row = panel
        .list
        .rows()
        .iter()
        .find(|row| row.agent_id == AgentId::new(2))
        .unwrap();
    assert_eq!(row.status, AgentProjectionStatus::Working);
}

#[test]
fn esc_close_stages_pending_stop_observing() {
    let mut model = ready_panel_model();
    press_key(&mut model, crossterm::event::KeyCode::Esc);

    assert!(!model.agents_panel_active());
    let stops = model
        .pending_stop_observing_agents
        .as_ref()
        .expect("close must stage pending stops");
    assert_eq!(
        stops.overview,
        Some((
            runtime_domain::agent::AgentObservationId::new(FIXTURE_OBSERVATION_ID),
            runtime_domain::agent::AgentRuntimeGeneration::new(FIXTURE_GENERATION),
        ))
    );
}

#[test]
fn session_resume_closes_panel_and_stages_stop() {
    let mut model = ready_panel_model();

    model.apply_runtime_event(RuntimeEvent::SessionResumed {
        payload: SessionResumePayload {
            session_id: "session-2".to_string(),
            transcript: vec![TranscriptReplayItem::Message {
                role: TranscriptReplayRole::Assistant,
                content: "replayed".to_string(),
            }],
            restored_model: None,
        },
    });

    assert!(!model.agents_panel_active());
    assert!(model.pending_stop_observing_agents.is_some());
}

#[test]
fn session_reset_closes_panel_and_stages_stop() {
    let mut model = ready_panel_model();

    model.reset_to_initial_tui_state();

    assert!(!model.agents_panel_active());
    assert!(model.pending_stop_observing_agents.is_some());
}

#[test]
fn runtime_stopped_closes_panel_and_stages_stop() {
    let mut model = ready_panel_model();

    model.apply_runtime_event(RuntimeEvent::Stopped {
        target: RuntimeTarget::provider("local", "qwen3"),
        message: None,
    });

    assert!(!model.agents_panel_active());
    assert!(model.pending_stop_observing_agents.is_some());
}

#[test]
fn attention_pill_close_closes_panel_and_stages_stop() {
    let mut model = ready_panel_model();
    // 审批 pill 置位后点击左上角 pill：逐层关闭全部非审批全屏层。
    model.mark_tool_approval_attention_pending();
    let _ = model.handle_attention_pill_mouse_down(MouseButton::Left, 0, 0);

    assert!(!model.agents_panel_active());
    assert!(model.pending_stop_observing_agents.is_some());
}

#[test]
fn close_is_idempotent_across_repeated_paths() {
    let mut model = ready_panel_model();
    press_key(&mut model, crossterm::event::KeyCode::Esc);
    let staged: PendingAgentObservationStops = model
        .pending_stop_observing_agents
        .clone()
        .expect("first close stages stops");

    // 重复关闭不再叠加。
    model.close_agents_panel();
    assert_eq!(model.pending_stop_observing_agents.as_ref(), Some(&staged));
}

#[test]
fn reopen_while_established_stages_previous_observation_stop() {
    let mut model = ready_panel_model();

    // observation 已建立后再次打开：旧 observation 必须进入待注销集合，
    // 不能被 loading 覆盖后泄漏存活。
    let _ = model.open_agents_panel_loading();

    assert!(model.agents_panel.as_ref().unwrap().is_loading);
    let stops = model
        .pending_stop_observing_agents
        .as_ref()
        .expect("reopen must stage the previous observation stop");
    assert_eq!(
        stops.overview,
        Some((
            runtime_domain::agent::AgentObservationId::new(FIXTURE_OBSERVATION_ID),
            runtime_domain::agent::AgentRuntimeGeneration::new(FIXTURE_GENERATION),
        ))
    );
}

#[test]
fn runner_consumes_pending_stops_and_dispatches_stop_observing() {
    let mut model = ready_panel_model();
    // 打开一个 preview surface 让 panel 同时持有 per-agent view observation。
    let effect = press_key(&mut model, crossterm::event::KeyCode::Char(' '));
    let AppEffect::ObserveAgentTranscript { request_id, .. } =
        effect.expect("preview dispatches observe")
    else {
        panic!("unexpected effect");
    };
    apply_view_snapshot_loaded(&mut model, request_id, view_snapshot(2, 21, Some("answer")));
    // Esc 先退 preview 回 list，再 Esc 关闭 panel。
    press_key(&mut model, crossterm::event::KeyCode::Esc);
    press_key(&mut model, crossterm::event::KeyCode::Esc);
    assert!(!model.agents_panel_active());

    let mut port = RecordingRuntimePort::default();
    dispatch_agents_observation_stops_if_needed(&mut model, &mut port);

    assert!(
        port.commands
            .contains(&RuntimeCommand::StopObservingAgents {
                observation_id: runtime_domain::agent::AgentObservationId::new(
                    FIXTURE_OBSERVATION_ID
                ),
                generation: runtime_domain::agent::AgentRuntimeGeneration::new(FIXTURE_GENERATION),
            })
    );
    assert!(
        port.commands
            .iter()
            .any(|command| matches!(command, RuntimeCommand::StopObservingAgentTranscript { .. })),
        "per-agent view observation must also be stopped: {:?}",
        port.commands
    );
    // 消费后标志清空，重复消费幂等。
    assert!(model.pending_stop_observing_agents.is_none());
    dispatch_agents_observation_stops_if_needed(&mut model, &mut port);
    assert_eq!(port.commands.len(), 2);
}

#[test]
fn dismissed_pending_view_request_stops_after_late_snapshot() {
    let mut model = ready_panel_model();
    // Enter 进入 transcript surface 后立即关闭 panel：请求仍在飞行中。
    let effect = press_key(&mut model, crossterm::event::KeyCode::Enter);
    let AppEffect::ObserveAgentTranscript { request_id, .. } =
        effect.expect("enter dispatches observe")
    else {
        panic!("unexpected effect");
    };
    // Esc 先退 transcript surface 回 list，再 Esc 关闭 panel：请求仍在飞行中。
    press_key(&mut model, crossterm::event::KeyCode::Esc);
    press_key(&mut model, crossterm::event::KeyCode::Esc);
    assert!(!model.agents_panel_active());
    assert!(model.pending_agent_view_stop_requests.contains(&request_id));

    // 回包到达：panel 已关，按回包携带的 observation id 补 stop。
    apply_view_snapshot_loaded(&mut model, request_id, view_snapshot(2, 31, None));

    assert!(model.pending_agent_view_stop_requests.is_empty());
    let stops = model
        .pending_stop_observing_agents
        .as_ref()
        .expect("late snapshot stages a stop for the leaked observation");
    assert_eq!(
        stops.agent_views,
        vec![(
            runtime_domain::agent::AgentObservationId::new(31),
            runtime_domain::agent::AgentRuntimeGeneration::new(FIXTURE_GENERATION),
        )]
    );
}

#[test]
fn noop_port_open_reports_unavailable_in_panel_error_state() {
    let mut model = Model::new(StartupBannerOptions::default());
    model.set_window(80, 24);
    let mut port = NoopUiRuntimePort;

    run_open_agents_panel_effect(&mut model, &mut port);

    let panel = model.agents_panel.as_ref().unwrap();
    assert!(!panel.is_loading);
    assert_eq!(panel.error.as_deref(), Some("Runtime is not available"));
    assert!(!panel.has_rows());
}

#[test]
fn modal_layer_agents_overview_is_lowest_priority() {
    let mut model = Model::new(StartupBannerOptions::default());
    model.set_window(80, 12);
    model.open_agents_panel_loading();
    assert_eq!(model.top_modal_layer(), Some(ModalLayer::AgentsOverview));

    // MessageHistory 与 EntryTree 都叠在 agents panel 之上。
    model.open_message_history_picker_loading();
    assert_eq!(model.top_modal_layer(), Some(ModalLayer::MessageHistory));
    model.message_history_picker = None;
    model.open_entry_tree_loading();
    assert_eq!(model.top_modal_layer(), Some(ModalLayer::EntryTree));
    model.entry_tree = None;
    assert_eq!(model.top_modal_layer(), Some(ModalLayer::AgentsOverview));
}

#[test]
fn blocked_keys_do_not_leak_to_composer_while_panel_open() {
    let mut model = ready_panel_model();
    // 未绑定键一律被模态吞掉：不得改写 composer、不得触发 Esc interrupt。
    model.update(crate::AppEvent::Key(crossterm::event::KeyEvent::from(
        crossterm::event::KeyCode::Char('q'),
    )));
    assert_eq!(model.composer_text(), "");
}

#[test]
fn ctrl_t_is_swallowed_like_other_fullscreen_pickers() {
    // 全屏 picker 家族约定：模态吞掉全部未绑定键（含 Ctrl-T），
    // transcript overlay 的全局 toggle 不在模态之上生效。
    let mut model = ready_panel_model();
    let _ = model.update(crate::AppEvent::Key(crossterm::event::KeyEvent::new(
        crossterm::event::KeyCode::Char('t'),
        crossterm::event::KeyModifiers::CONTROL,
    )));

    assert!(model.agents_panel_active(), "agents panel must stay open");
    assert!(!model.transcript_overlay_active());
}
