use runtime_domain::agent::{AgentId, AgentPermissionState, AgentRuntimeGeneration};
use runtime_domain::session::{
    RuntimeEvent, RuntimeTarget, SessionResumePayload, TranscriptReplayItem, TranscriptReplayRole,
};

use crate::{
    Model, StartupBannerOptions, agents_panel::AgentsPanelPillNavigation,
    runtime::RuntimeEventApply,
};

use super::common::{
    FIXTURE_GENERATION, apply_permission_update, permission_request, ready_panel_model,
    ready_panel_model_with_rows, sample_rows,
};

fn fresh_model() -> Model {
    let mut model = Model::new(StartupBannerOptions::default());
    model.set_window(100, 24);
    model
}

#[test]
fn some_update_upserts_head_and_none_update_removes() {
    let mut model = fresh_model();
    apply_permission_update(
        &mut model,
        2,
        FIXTURE_GENERATION,
        Some(permission_request(
            2,
            "req-1",
            AgentPermissionState::Pending,
            100,
        )),
    );
    assert_eq!(
        model
            .agent_pending_permission_head_for_test(AgentId::new(2))
            .map(|head| head.target.request_id.as_str()),
        Some("req-1")
    );

    // None 表示收敛或清空：entry 移除。
    apply_permission_update(&mut model, 2, FIXTURE_GENERATION, None);
    assert!(
        model
            .agent_pending_permission_head_for_test(AgentId::new(2))
            .is_none()
    );
    assert_eq!(model.pending_agent_permission_count(), 0);
}

#[test]
fn same_generation_updates_head_in_place() {
    let mut model = fresh_model();
    apply_permission_update(
        &mut model,
        2,
        FIXTURE_GENERATION,
        Some(permission_request(
            2,
            "req-1",
            AgentPermissionState::Pending,
            100,
        )),
    );
    // 同 generation 的新 head 原位替换。
    apply_permission_update(
        &mut model,
        2,
        FIXTURE_GENERATION,
        Some(permission_request(
            2,
            "req-2",
            AgentPermissionState::Submitted,
            200,
        )),
    );
    let head = model
        .agent_pending_permission_head_for_test(AgentId::new(2))
        .unwrap();
    assert_eq!(head.target.request_id, "req-2");
    assert_eq!(head.state, AgentPermissionState::Submitted);
}

#[test]
fn generation_mismatch_resets_whole_map_to_update_result() {
    let mut model = fresh_model();
    apply_permission_update(
        &mut model,
        2,
        FIXTURE_GENERATION,
        Some(permission_request(
            2,
            "req-1",
            AgentPermissionState::Pending,
            100,
        )),
    );
    apply_permission_update(
        &mut model,
        3,
        FIXTURE_GENERATION,
        Some(permission_request(
            3,
            "req-old",
            AgentPermissionState::Pending,
            50,
        )),
    );
    assert_eq!(model.pending_agent_permission_count(), 2);

    // 不同 generation 的事件：旧 generation 的全部 entry 不再可信，整 map 重置。
    apply_permission_update(
        &mut model,
        9,
        FIXTURE_GENERATION + 1,
        Some(permission_request(
            9,
            "req-new",
            AgentPermissionState::Pending,
            300,
        )),
    );
    assert_eq!(
        model.agent_pending_permission_generation_for_test(),
        Some(AgentRuntimeGeneration::new(FIXTURE_GENERATION + 1))
    );
    assert!(
        model
            .agent_pending_permission_head_for_test(AgentId::new(2))
            .is_none()
    );
    assert!(
        model
            .agent_pending_permission_head_for_test(AgentId::new(3))
            .is_none()
    );
    assert_eq!(
        model
            .agent_pending_permission_head_for_test(AgentId::new(9))
            .map(|head| head.target.request_id.as_str()),
        Some("req-new")
    );
}

#[test]
fn pending_count_ignores_submitted_heads() {
    let mut model = fresh_model();
    apply_permission_update(
        &mut model,
        2,
        FIXTURE_GENERATION,
        Some(permission_request(
            2,
            "req-1",
            AgentPermissionState::Submitted,
            100,
        )),
    );
    apply_permission_update(
        &mut model,
        3,
        FIXTURE_GENERATION,
        Some(permission_request(
            3,
            "req-2",
            AgentPermissionState::Pending,
            200,
        )),
    );

    // Submitted head 保留在 map（preview 对账用），但不参与 pill 的 pending 判定。
    assert_eq!(model.pending_agent_permission_count(), 1);
    assert!(
        model
            .agent_pending_permission_head_for_test(AgentId::new(2))
            .is_some()
    );
}

#[test]
fn single_pending_routes_to_preview_and_multiple_preselects_earliest() {
    let mut model = fresh_model();
    apply_permission_update(
        &mut model,
        2,
        FIXTURE_GENERATION,
        Some(permission_request(
            2,
            "req-late",
            AgentPermissionState::Pending,
            300,
        )),
    );

    // 单 pending：直达该 agent 的 preview。
    assert_eq!(
        model.agents_panel_pill_navigation_target(),
        Some(AgentsPanelPillNavigation::OpenPreview {
            agent_id: AgentId::new(2)
        })
    );

    // 多 pending：预选 (occurred_at_ms, request_id) 升序最早的 owner。
    apply_permission_update(
        &mut model,
        3,
        FIXTURE_GENERATION,
        Some(permission_request(
            3,
            "req-early",
            AgentPermissionState::Pending,
            100,
        )),
    );
    assert_eq!(
        model.agents_panel_pill_navigation_target(),
        Some(AgentsPanelPillNavigation::Preselect {
            agent_id: AgentId::new(3)
        })
    );
}

#[test]
fn multiple_pendings_tie_break_on_request_id() {
    let mut model = fresh_model();
    // occurred_at 相同：stable request identity 决定预选归属。
    apply_permission_update(
        &mut model,
        5,
        FIXTURE_GENERATION,
        Some(permission_request(
            5,
            "req-b",
            AgentPermissionState::Pending,
            100,
        )),
    );
    apply_permission_update(
        &mut model,
        6,
        FIXTURE_GENERATION,
        Some(permission_request(
            6,
            "req-a",
            AgentPermissionState::Pending,
            100,
        )),
    );

    assert_eq!(
        model.agents_panel_pill_navigation_target(),
        Some(AgentsPanelPillNavigation::Preselect {
            agent_id: AgentId::new(6)
        })
    );
}

#[test]
fn navigation_target_requires_pending_head() {
    let mut model = fresh_model();
    // Submitted-only：不引导点击（attention 已处理待收敛）。
    apply_permission_update(
        &mut model,
        2,
        FIXTURE_GENERATION,
        Some(permission_request(
            2,
            "req-1",
            AgentPermissionState::Submitted,
            100,
        )),
    );
    assert_eq!(model.agents_panel_pill_navigation_target(), None);
}

// ---- 四清空路径 ----

#[test]
fn session_resume_clears_pending_projection() {
    let mut model = ready_panel_model();
    apply_permission_update(
        &mut model,
        2,
        FIXTURE_GENERATION,
        Some(permission_request(
            2,
            "req-1",
            AgentPermissionState::Pending,
            100,
        )),
    );
    assert_eq!(model.pending_agent_permission_count(), 1);

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

    assert_eq!(model.pending_agent_permission_count(), 0);
    assert_eq!(model.agent_pending_permission_generation_for_test(), None);
}

#[test]
fn model_reset_clears_pending_projection() {
    let mut model = fresh_model();
    apply_permission_update(
        &mut model,
        2,
        FIXTURE_GENERATION,
        Some(permission_request(
            2,
            "req-1",
            AgentPermissionState::Pending,
            100,
        )),
    );

    model.reset_to_initial_tui_state();

    assert_eq!(model.pending_agent_permission_count(), 0);
    assert_eq!(model.agent_pending_permission_generation_for_test(), None);
}

#[test]
fn runtime_stopped_clears_pending_projection() {
    let mut model = ready_panel_model_with_rows(sample_rows());
    apply_permission_update(
        &mut model,
        2,
        FIXTURE_GENERATION,
        Some(permission_request(
            2,
            "req-1",
            AgentPermissionState::Pending,
            100,
        )),
    );

    model.apply_runtime_event(RuntimeEvent::Stopped {
        target: RuntimeTarget::provider("local", "qwen3"),
        message: None,
    });

    assert_eq!(model.pending_agent_permission_count(), 0);
    assert_eq!(model.agent_pending_permission_generation_for_test(), None);
}

#[test]
fn permission_projection_does_not_append_document_items() {
    // 回归守护：pending map 是 pill 数据源，不得进入 document timeline。
    let mut model = ready_panel_model();
    let item_count = model.transcript_plain_items().len();

    apply_permission_update(
        &mut model,
        2,
        FIXTURE_GENERATION,
        Some(permission_request(
            2,
            "req-1",
            AgentPermissionState::Pending,
            100,
        )),
    );

    assert_eq!(
        model.transcript_plain_items().len(),
        item_count,
        "permission projections must not append document items"
    );
}

#[test]
fn stale_lower_generation_event_does_not_update_fresh_state() {
    let mut model = fresh_model();
    apply_permission_update(
        &mut model,
        2,
        FIXTURE_GENERATION + 1,
        Some(permission_request(
            2,
            "req-fresh",
            AgentPermissionState::Pending,
            100,
        )),
    );

    // 旧 generation 的迟到事件：fresh state 不受污染（PRD stale guard）。
    apply_permission_update(
        &mut model,
        3,
        FIXTURE_GENERATION,
        Some(permission_request(
            3,
            "req-stale",
            AgentPermissionState::Pending,
            100,
        )),
    );
    apply_permission_update(&mut model, 2, FIXTURE_GENERATION, None);

    assert_eq!(
        model.agent_pending_permission_generation_for_test(),
        Some(AgentRuntimeGeneration::new(FIXTURE_GENERATION + 1))
    );
    assert_eq!(
        model
            .agent_pending_permission_head_for_test(AgentId::new(2))
            .map(|head| head.target.request_id.as_str()),
        Some("req-fresh")
    );
    assert!(
        model
            .agent_pending_permission_head_for_test(AgentId::new(3))
            .is_none()
    );
    assert_eq!(model.pending_agent_permission_count(), 1);
}
