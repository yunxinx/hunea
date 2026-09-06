use crossterm::event::KeyCode;
use runtime_domain::agent::{AgentId, AgentObservationId};

use crate::{AppEffect, agents_panel::AgentsPanelSurface, runner::run_stop_agent_effect};

use super::common::{
    RecordingRuntimePort, apply_view_snapshot_loaded, apply_view_updated, press_key,
    ready_panel_model, view_snapshot,
};

fn surface_transcript_items(model: &mut crate::Model) -> Vec<String> {
    match model
        .agents_panel
        .as_ref()
        .and_then(|panel| panel.surface.as_ref())
    {
        Some(AgentsPanelSurface::Transcript { transcript, .. }) => transcript.plain_items(),
        _ => Vec::new(),
    }
}

#[test]
fn enter_opens_transcript_surface_and_dispatches_observe() {
    let mut model = ready_panel_model();

    let effect = press_key(&mut model, KeyCode::Enter);

    let AppEffect::ObserveAgentTranscript {
        request_id,
        agent_id,
    } = effect.expect("enter dispatches observe")
    else {
        panic!("unexpected effect");
    };
    assert_eq!(agent_id, AgentId::new(2));
    assert!(model.agents_panel_transcript_active());
    assert!(
        model
            .agents_panel
            .as_ref()
            .unwrap()
            .agent_view_for_agent(agent_id)
            .is_some()
    );
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
fn view_snapshot_builds_delivery_safe_transcript() {
    let mut model = ready_panel_model();
    let effect = press_key(&mut model, KeyCode::Enter);
    let AppEffect::ObserveAgentTranscript { request_id, .. } = effect.unwrap() else {
        panic!("unexpected effect");
    };

    apply_view_snapshot_loaded(
        &mut model,
        request_id,
        view_snapshot(2, 21, Some("final answer")),
    );

    let items = surface_transcript_items(&mut model).join("\n");
    assert!(items.contains("summarize the repo"), "user item: {items}");
    // 工具标题经 tool activity 展示层规整（如 `ReadFile: x` → `Read x`），断言稳定子串。
    assert!(items.contains("QueryDatabase"), "tool title: {items}");
    assert!(items.contains("3 rows returned"), "tool content: {items}");
    assert!(items.contains("final answer"), "assistant item: {items}");
}

#[test]
fn esc_returns_to_list_restoring_selection() {
    let mut model = ready_panel_model();
    press_key(&mut model, KeyCode::Char('j'));
    let effect = press_key(&mut model, KeyCode::Enter);
    let AppEffect::ObserveAgentTranscript { request_id, .. } = effect.unwrap() else {
        panic!("unexpected effect");
    };
    apply_view_snapshot_loaded(&mut model, request_id, view_snapshot(3, 21, None));
    assert!(model.agents_panel_transcript_active());

    assert_eq!(press_key(&mut model, KeyCode::Esc), None);

    assert!(!model.agents_panel_transcript_active());
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
}

#[test]
fn view_updated_rebuilds_bound_transcript_surface() {
    let mut model = ready_panel_model();
    let effect = press_key(&mut model, KeyCode::Enter);
    let AppEffect::ObserveAgentTranscript { request_id, .. } = effect.unwrap() else {
        panic!("unexpected effect");
    };
    apply_view_snapshot_loaded(&mut model, request_id, view_snapshot(2, 21, Some("first")));

    apply_view_updated(&mut model, view_snapshot(2, 21, Some("second")));

    let items = surface_transcript_items(&mut model).join("\n");
    assert!(
        items.contains("second"),
        "AgentViewUpdated must rebuild the bound transcript surface: {items}"
    );
}

#[test]
fn view_updated_for_unbound_agent_does_not_touch_surface() {
    let mut model = ready_panel_model();
    let effect = press_key(&mut model, KeyCode::Enter);
    let AppEffect::ObserveAgentTranscript { request_id, .. } = effect.unwrap() else {
        panic!("unexpected effect");
    };
    apply_view_snapshot_loaded(
        &mut model,
        request_id,
        view_snapshot(2, 21, Some("bound answer")),
    );

    // agent 3 的 view 更新不绑定当前 surface（agent 2），不刷新。
    apply_view_updated(&mut model, view_snapshot(3, 31, Some("other answer")));

    let items = surface_transcript_items(&mut model).join("\n");
    assert!(items.contains("bound answer"));
    assert!(!items.contains("other answer"));
}

#[test]
fn stale_observation_view_updated_is_dropped() {
    let mut model = ready_panel_model();
    let effect = press_key(&mut model, KeyCode::Enter);
    let AppEffect::ObserveAgentTranscript { request_id, .. } = effect.unwrap() else {
        panic!("unexpected effect");
    };
    apply_view_snapshot_loaded(&mut model, request_id, view_snapshot(2, 21, Some("fresh")));

    // observation id 不匹配的更新被丢弃。
    let mut foreign = view_snapshot(2, 99, Some("foreign"));
    foreign.transcript.observation_id = AgentObservationId::new(99);
    apply_view_updated(&mut model, foreign);

    let items = surface_transcript_items(&mut model).join("\n");
    assert!(items.contains("fresh"));
    assert!(!items.contains("foreign"));
}

#[test]
fn transcript_surface_pending_state_renders_loading() {
    let mut model = ready_panel_model();
    press_key(&mut model, KeyCode::Enter);

    let buffer = crate::test_helpers::render_model_buffer(&mut model, 100, 24);
    let rows = crate::test_helpers::rendered_rows(&buffer);
    assert!(
        rows.iter()
            .any(|row| row.contains("Loading agent transcript")),
        "pending transcript surface should render a loading hint: {rows:?}"
    );
}

#[test]
fn transcript_surface_renders_items_via_overlay_view() {
    let mut model = ready_panel_model();
    let effect = press_key(&mut model, KeyCode::Enter);
    let AppEffect::ObserveAgentTranscript { request_id, .. } = effect.unwrap() else {
        panic!("unexpected effect");
    };
    apply_view_snapshot_loaded(
        &mut model,
        request_id,
        view_snapshot(2, 21, Some("final answer")),
    );

    let buffer = crate::test_helpers::render_model_buffer(&mut model, 100, 24);
    let rows = crate::test_helpers::rendered_rows(&buffer);
    assert!(
        rows.iter().any(|row| row.contains("summarize the repo")),
        "committed user item should be visible: {rows:?}"
    );
    assert!(
        rows.iter().any(|row| row.contains("final answer")),
        "committed assistant answer should be visible: {rows:?}"
    );
    assert!(
        rows.iter().any(|row| row.contains("Esc back")),
        "transcript footer should render the back hint: {rows:?}"
    );
}

#[test]
fn stop_agent_dispatch_error_shows_toast() {
    let mut model = ready_panel_model();
    let mut port = crate::runner::NoopUiRuntimePort;

    run_stop_agent_effect(
        &mut model,
        &mut port,
        AgentId::new(2),
        runtime_domain::agent::AgentRuntimeGeneration::new(1),
    );

    assert_eq!(
        model.active_toast_text_for_test(),
        Some("Runtime is not available")
    );
}

#[test]
fn stop_agent_dispatch_reaches_runtime() {
    let mut model = ready_panel_model();
    let mut port = RecordingRuntimePort::default();

    run_stop_agent_effect(
        &mut model,
        &mut port,
        AgentId::new(2),
        runtime_domain::agent::AgentRuntimeGeneration::new(1),
    );

    assert_eq!(
        port.commands,
        vec![runtime_domain::session::RuntimeCommand::StopAgent {
            agent_id: AgentId::new(2),
            generation: runtime_domain::agent::AgentRuntimeGeneration::new(1),
        }]
    );
}

#[test]
fn stop_result_reflects_via_overview_delta_only() {
    let mut model = ready_panel_model();
    press_key(&mut model, KeyCode::Char('x'));
    let effect = press_key(&mut model, KeyCode::Char('x'));
    assert!(matches!(effect, Some(AppEffect::StopAgent { .. })));

    // stop 结果经既有 overview delta 体现，不新增 stop 专用事件。
    super::common::apply_overview_delta(
        &mut model,
        runtime_domain::agent::AgentOverviewDeltaKind::Upsert(super::common::overview_row(
            2,
            "research task",
            runtime_domain::agent::AgentProjectionStatus::Cancelled,
        )),
    );
    let row = model
        .agents_panel
        .as_ref()
        .unwrap()
        .list
        .rows()
        .iter()
        .find(|row| row.agent_id == AgentId::new(2))
        .unwrap();
    assert_eq!(
        row.status,
        runtime_domain::agent::AgentProjectionStatus::Cancelled
    );
}
