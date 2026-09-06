use crossterm::event::KeyCode;
use runtime_domain::agent::{AgentId, AgentObservationRequestId};

use crate::{
    AppEffect,
    agents_panel::AgentsPanelSurface,
    runner::{NoopUiRuntimePort, run_observe_agent_transcript_effect},
    runtime::RuntimeEventApply,
};

use super::common::{
    apply_view_snapshot_loaded, apply_view_updated, press_key, ready_panel_model, view_snapshot,
};

#[test]
fn space_opens_preview_and_dispatches_observe() {
    let mut model = ready_panel_model();

    let effect = press_key(&mut model, KeyCode::Char(' '));

    let AppEffect::ObserveAgentTranscript {
        request_id,
        agent_id,
    } = effect.expect("space dispatches observe")
    else {
        panic!("unexpected effect");
    };
    assert_eq!(agent_id, AgentId::new(2));
    assert!(model.agents_panel_preview_active());
    assert_eq!(
        model
            .agents_panel
            .as_ref()
            .unwrap()
            .agent_view_for_agent(agent_id)
            .and_then(|record| record.pending_request_id),
        Some(request_id)
    );
}

#[test]
fn view_snapshot_loads_preview_record() {
    let mut model = ready_panel_model();
    let effect = press_key(&mut model, KeyCode::Char(' '));
    let AppEffect::ObserveAgentTranscript { request_id, .. } = effect.unwrap() else {
        panic!("unexpected effect");
    };

    apply_view_snapshot_loaded(
        &mut model,
        request_id,
        view_snapshot(2, 21, Some("committed answer")),
    );

    let record = model
        .agents_panel
        .as_ref()
        .unwrap()
        .agent_view_for_agent(AgentId::new(2))
        .unwrap();
    assert!(record.pending_request_id.is_none());
    assert_eq!(record.observation_id.map(|id| id.get()), Some(21));
    assert!(record.snapshot.is_some());
    assert!(record.error.is_none());

    let lines = model.agents_panel_preview_display_lines().unwrap();
    assert!(
        lines.iter().any(|line| line.contains("committed answer")),
        "preview body should render the committed answer: {lines:?}"
    );
}

#[test]
fn preview_without_committed_answer_shows_neutral_fallback() {
    let mut model = ready_panel_model();
    let effect = press_key(&mut model, KeyCode::Char(' '));
    let AppEffect::ObserveAgentTranscript { request_id, .. } = effect.unwrap() else {
        panic!("unexpected effect");
    };

    apply_view_snapshot_loaded(&mut model, request_id, view_snapshot(2, 21, None));

    let lines = model.agents_panel_preview_display_lines().unwrap();
    assert!(
        lines
            .iter()
            .any(|line| line.contains("No committed answer yet")),
        "missing answer must render the neutral empty state: {lines:?}"
    );
    assert!(
        lines.iter().any(|line| line.contains("Latest activity")),
        "fallback should include the safe activity summary: {lines:?}"
    );
}

#[test]
fn preview_shows_loading_before_snapshot() {
    let mut model = ready_panel_model();
    press_key(&mut model, KeyCode::Char(' '));

    let lines = model.agents_panel_preview_display_lines().unwrap();
    assert!(
        lines
            .iter()
            .any(|line| line.contains("Loading agent preview")),
        "pending record must render a loading state: {lines:?}"
    );
}

#[test]
fn esc_and_space_return_to_list_restoring_selection() {
    let mut model = ready_panel_model();
    press_key(&mut model, KeyCode::Char('j'));
    let effect = press_key(&mut model, KeyCode::Char(' '));
    let AppEffect::ObserveAgentTranscript { request_id, .. } = effect.unwrap() else {
        panic!("unexpected effect");
    };
    apply_view_snapshot_loaded(&mut model, request_id, view_snapshot(3, 21, None));
    assert!(model.agents_panel_preview_active());

    // Space 返回 list。
    assert_eq!(press_key(&mut model, KeyCode::Char(' ')), None);
    assert!(!model.agents_panel_preview_active());
    assert_eq!(
        model
            .agents_panel
            .as_ref()
            .unwrap()
            .selected_row()
            .map(|r| r.agent_id),
        Some(AgentId::new(3)),
        "returning to the list must restore the original AgentId selection"
    );

    // 再次进入后用 Esc 返回。
    press_key(&mut model, KeyCode::Char(' '));
    assert!(model.agents_panel_preview_active());
    assert_eq!(press_key(&mut model, KeyCode::Esc), None);
    assert!(!model.agents_panel_preview_active());
}

#[test]
fn stale_view_snapshot_request_is_dropped() {
    let mut model = ready_panel_model();
    press_key(&mut model, KeyCode::Char(' '));

    apply_view_snapshot_loaded(
        &mut model,
        AgentObservationRequestId::new(999),
        view_snapshot(2, 21, Some("stale")),
    );

    let record = model
        .agents_panel
        .as_ref()
        .unwrap()
        .agent_view_for_agent(AgentId::new(2))
        .unwrap();
    assert!(
        record.snapshot.is_none(),
        "stale request must not fill the record"
    );
    assert!(record.pending_request_id.is_some());
}

#[test]
fn view_updated_refreshes_record_for_matching_observation() {
    let mut model = ready_panel_model();
    let effect = press_key(&mut model, KeyCode::Char(' '));
    let AppEffect::ObserveAgentTranscript { request_id, .. } = effect.unwrap() else {
        panic!("unexpected effect");
    };
    apply_view_snapshot_loaded(
        &mut model,
        request_id,
        view_snapshot(2, 21, Some("first answer")),
    );

    apply_view_updated(&mut model, view_snapshot(2, 21, Some("second answer")));

    let lines = model.agents_panel_preview_display_lines().unwrap();
    assert!(
        lines.iter().any(|line| line.contains("second answer")),
        "AgentViewUpdated must refresh the preview body: {lines:?}"
    );
}

#[test]
fn view_updated_with_foreign_observation_is_dropped() {
    let mut model = ready_panel_model();
    let effect = press_key(&mut model, KeyCode::Char(' '));
    let AppEffect::ObserveAgentTranscript { request_id, .. } = effect.unwrap() else {
        panic!("unexpected effect");
    };
    apply_view_snapshot_loaded(
        &mut model,
        request_id,
        view_snapshot(2, 21, Some("first answer")),
    );

    apply_view_updated(&mut model, view_snapshot(2, 99, Some("foreign answer")));

    let lines = model.agents_panel_preview_display_lines().unwrap();
    assert!(
        lines.iter().any(|line| line.contains("first answer")),
        "foreign observation updates must be dropped: {lines:?}"
    );
}

#[test]
fn reopened_preview_reuses_record_without_second_observer() {
    let mut model = ready_panel_model();
    let effect = press_key(&mut model, KeyCode::Char(' '));
    let AppEffect::ObserveAgentTranscript { request_id, .. } = effect.unwrap() else {
        panic!("unexpected effect");
    };
    apply_view_snapshot_loaded(&mut model, request_id, view_snapshot(2, 21, None));
    press_key(&mut model, KeyCode::Char(' '));

    // record 已有快照：再次打开 surface 复用 observation，不派发第二个 observer。
    let second = press_key(&mut model, KeyCode::Char(' '));
    assert_eq!(
        second, None,
        "reopening a surface with a cached snapshot must not dispatch ObserveAgentTranscript"
    );
    assert!(model.agents_panel_preview_active());
    let lines = model.agents_panel_preview_display_lines().unwrap();
    assert!(
        lines
            .iter()
            .any(|line| line.contains("No committed answer yet"))
    );
}

#[test]
fn reopen_after_rejection_redispatches() {
    let mut model = ready_panel_model();
    let effect = press_key(&mut model, KeyCode::Char(' '));
    let AppEffect::ObserveAgentTranscript { request_id, .. } = effect.unwrap() else {
        panic!("unexpected effect");
    };
    // runtime 拒绝：record 进入 error 态。
    model.apply_runtime_event(runtime_domain::session::RuntimeEvent::AgentProjection(
        Box::new(
            runtime_domain::agent::AgentProjectionEvent::AgentObservationRejected {
                request_id,
                reason: runtime_domain::agent::AgentObservationRejection::UnknownAgent,
            },
        ),
    ));
    press_key(&mut model, KeyCode::Char(' '));

    // 上次失败：重新打开必须重新派发请求。
    let second = press_key(&mut model, KeyCode::Char(' '));
    assert!(
        matches!(second, Some(AppEffect::ObserveAgentTranscript { .. })),
        "reopening after a rejection must dispatch a fresh observe request"
    );
}

#[test]
fn noop_observe_transcript_error_surfaces_in_record() {
    let mut model = ready_panel_model();
    press_key(&mut model, KeyCode::Char(' '));
    let request_id = model
        .agents_panel
        .as_ref()
        .unwrap()
        .agent_view_for_agent(AgentId::new(2))
        .and_then(|record| record.pending_request_id)
        .expect("record should hold the pending request");

    let mut port = NoopUiRuntimePort;
    run_observe_agent_transcript_effect(&mut model, &mut port, request_id, AgentId::new(2));

    let record = model
        .agents_panel
        .as_ref()
        .unwrap()
        .agent_view_for_agent(AgentId::new(2))
        .unwrap();
    assert_eq!(record.error.as_deref(), Some("Runtime is not available"));
    let lines = model.agents_panel_preview_display_lines().unwrap();
    assert!(
        lines
            .iter()
            .any(|line| line.contains("Runtime is not available"))
    );
}

#[test]
fn preview_surface_swallow_unknown_keys() {
    let mut model = ready_panel_model();
    press_key(&mut model, KeyCode::Char(' '));

    assert_eq!(press_key(&mut model, KeyCode::Char('z')), None);
    assert!(
        model.agents_panel_preview_active(),
        "unknown keys must not close the surface"
    );
    assert!(matches!(
        model.agents_panel.as_ref().unwrap().surface,
        Some(AgentsPanelSurface::Preview { .. })
    ));
}
