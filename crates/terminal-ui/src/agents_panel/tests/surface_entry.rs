use crossterm::event::KeyCode;
use runtime_domain::agent::{AgentId, AgentObservationRequestId};

use crate::{
    AppEffect,
    runner::{NoopUiRuntimePort, run_observe_agent_transcript_effect},
    runtime::RuntimeEventApply,
};

use super::common::{apply_view_snapshot_loaded, press_key, ready_panel_model, view_snapshot};

/// `Space` 与 `Enter` 是同一 transcript surface 入口（quick preview 双轨已删除）。
#[test]
fn space_opens_transcript_surface_and_dispatches_observe() {
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
    assert!(model.agents_panel_transcript_active());
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
fn space_and_enter_open_the_same_transcript_surface() {
    let mut model = ready_panel_model();

    // Space 进入后返回，再 Enter 进入：同一 surface 语义（同 effect、同 active 态）。
    let space_effect = press_key(&mut model, KeyCode::Char(' '));
    assert!(matches!(
        space_effect,
        Some(AppEffect::ObserveAgentTranscript { .. })
    ));
    assert!(model.agents_panel_transcript_active());
    assert_eq!(press_key(&mut model, KeyCode::Char(' ')), None);
    assert!(!model.agents_panel_transcript_active());

    let enter_effect = press_key(&mut model, KeyCode::Enter);
    assert!(matches!(
        enter_effect,
        Some(AppEffect::ObserveAgentTranscript { .. })
    ));
    assert!(model.agents_panel_transcript_active());
    assert_eq!(press_key(&mut model, KeyCode::Esc), None);
    assert!(!model.agents_panel_transcript_active());
}

#[test]
fn space_and_esc_return_to_list_restoring_selection() {
    let mut model = ready_panel_model();
    press_key(&mut model, KeyCode::Char('j'));
    let effect = press_key(&mut model, KeyCode::Char(' '));
    let AppEffect::ObserveAgentTranscript { request_id, .. } = effect.unwrap() else {
        panic!("unexpected effect");
    };
    apply_view_snapshot_loaded(&mut model, request_id, view_snapshot(3, 21, None));
    assert!(model.agents_panel_transcript_active());

    // Space 只返回 list，不派发任何 effect。
    assert_eq!(press_key(&mut model, KeyCode::Char(' ')), None);
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

    // 再次进入后用 Esc 返回。
    press_key(&mut model, KeyCode::Char(' '));
    assert!(model.agents_panel_transcript_active());
    assert_eq!(press_key(&mut model, KeyCode::Esc), None);
    assert!(!model.agents_panel_transcript_active());
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
fn reopened_surface_reuses_record_without_second_observer() {
    let mut model = ready_panel_model();
    let effect = press_key(&mut model, KeyCode::Char(' '));
    let AppEffect::ObserveAgentTranscript { request_id, .. } = effect.unwrap() else {
        panic!("unexpected effect");
    };
    apply_view_snapshot_loaded(&mut model, request_id, view_snapshot(2, 21, None));
    press_key(&mut model, KeyCode::Char(' '));

    // record 已有快照：再次打开 surface 复用 observation，不派发第二个 observer。
    let second = press_key(&mut model, KeyCode::Enter);
    assert_eq!(
        second, None,
        "reopening a surface with a cached snapshot must not dispatch ObserveAgentTranscript"
    );
    assert!(model.agents_panel_transcript_active());
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
    let rows = crate::test_helpers::rendered_rows(&crate::test_helpers::render_model_buffer(
        &mut model, 100, 24,
    ));
    assert!(
        rows.iter()
            .any(|row| row.contains("Runtime is not available")),
        "record error must render in the surface pending view: {rows:?}"
    );
}

#[test]
fn transcript_surface_swallow_unknown_keys() {
    let mut model = ready_panel_model();
    press_key(&mut model, KeyCode::Char(' '));

    assert_eq!(press_key(&mut model, KeyCode::Char('z')), None);
    assert!(
        model.agents_panel_transcript_active(),
        "unknown keys must not close the surface"
    );
}

#[test]
fn surface_title_renders_rule_and_status_dot() {
    let mut model = ready_panel_model();
    let effect = press_key(&mut model, KeyCode::Char(' '));
    let AppEffect::ObserveAgentTranscript { request_id, .. } = effect.unwrap() else {
        panic!("unexpected effect");
    };
    apply_view_snapshot_loaded(
        &mut model,
        request_id,
        view_snapshot(2, 21, Some("committed answer body")),
    );

    let rows = crate::test_helpers::rendered_rows(&crate::test_helpers::render_model_buffer(
        &mut model, 100, 24,
    ));
    // 标题行：状态点 + 状态文字 + 标题 + elapsed，` · ` 紧凑分隔。
    let title_row = rows
        .iter()
        .find(|row| row.contains("research task"))
        .expect("surface title should render the frozen title: {rows:?}");
    assert!(
        title_row.contains("●") && title_row.contains("Working"),
        "title line carries the status dot and label: {title_row}"
    );
    assert!(
        title_row.contains(" · "),
        "title line must compact segments with the `·` separator: {title_row}"
    );
    assert!(
        title_row.contains("1m 23s"),
        "title line carries the elapsed label: {title_row}"
    );
    // 标题行与内容之间的项目统一分割线。
    let rule_row = rows
        .get(1)
        .expect("the row right below the title is the subtle rule");
    assert!(
        rule_row.chars().all(|c| c == '╌'),
        "a subtle rule must render below the title line: {rule_row}"
    );
    // 完整 transcript 内容经 Markdown 管线渲染。
    assert!(
        rows.iter().any(|row| row.contains("committed answer body")),
        "transcript content should render the full items: {rows:?}"
    );
    assert!(
        rows.iter().any(|row| row.contains("summarize the repo")),
        "committed user item should be visible: {rows:?}"
    );
}
